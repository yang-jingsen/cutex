use crate::logs::{LogBufferConfig, LogStore, LogSubscription};
use crate::model::*;
use crate::process_backend::{FakeProcessBackend, ProcessBackend, ProcessBackendError};
use crate::protocol::*;
use crate::registry::{
    topological_order, validate_dependency_graph, DependencyGraphError, MemoryRegistry,
    RegistrySnapshot, RegistryStore, RegistryStoreError, StoredDefinition,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::mpsc::{self, Receiver, RecvError, RecvTimeoutError, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct FakeHostConfig {
    pub host_instance_id: String,
    pub log_buffer: LogBufferConfig,
    pub event_history_capacity: usize,
    pub event_subscriber_capacity: usize,
}

impl Default for FakeHostConfig {
    fn default() -> Self {
        Self {
            host_instance_id: "fake-host-1".to_owned(),
            log_buffer: LogBufferConfig::default(),
            event_history_capacity: 4_096,
            event_subscriber_capacity: 256,
        }
    }
}

/// Deterministic Stage 1 supervisor. All state transitions are serialized,
/// while log delivery is separately synchronized and strictly non-blocking.
pub struct FakeHost {
    config: FakeHostConfig,
    registry: Arc<dyn RegistryStore>,
    inner: Mutex<Inner>,
    logs: LogStore,
    events: EventStore,
    pending_event_streams: Mutex<BTreeMap<SubscriptionId, EventSubscription>>,
    process_backend: Arc<dyn ProcessBackend>,
}

struct Inner {
    phase: HostPhase,
    registry_revision: u64,
    services: BTreeMap<ServiceId, ServiceSnapshot>,
    operations: BTreeMap<OperationId, Operation>,
    idempotency: BTreeMap<String, IdempotencyRecord>,
    known_runs: BTreeMap<ServiceId, BTreeSet<RunId>>,
    restart_attempts: BTreeMap<ServiceId, VecDeque<u64>>,
    serial: u64,
    start_order: Vec<ServiceId>,
    stop_order: Vec<ServiceId>,
    wall_clock: bool,
    last_timestamp_ms: u64,
}

#[derive(Clone)]
struct IdempotencyRecord {
    fingerprint: String,
    outcome: Result<Response, ApiError>,
}

struct EventStore {
    capacity: usize,
    state: Mutex<EventState>,
}

#[derive(Default)]
struct EventState {
    next_sequence: u64,
    history: VecDeque<HostEvent>,
    subscribers: BTreeMap<SubscriptionId, mpsc::SyncSender<HostEvent>>,
}

pub struct EventSubscription {
    id: SubscriptionId,
    receiver: Receiver<HostEvent>,
}

pub(crate) struct ExitObservation {
    pub observed: bool,
    pub restart: Option<RestartDirective>,
}

#[derive(Clone)]
pub(crate) struct RestartDirective {
    pub service_id: ServiceId,
    pub failed_run_id: RunId,
    pub backoff_ms: u64,
}

impl EventSubscription {
    pub fn id(&self) -> &SubscriptionId {
        &self.id
    }

    pub fn recv(&self) -> Result<HostEvent, RecvError> {
        self.receiver.recv()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<HostEvent, RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }

    pub fn try_recv(&self) -> Result<HostEvent, TryRecvError> {
        self.receiver.try_recv()
    }
}

impl EventStore {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            state: Mutex::new(EventState::default()),
        }
    }

    fn publish(&self, timestamp_ms: u64, event: HostEventKind) -> HostEvent {
        let mut state = self.state.lock().expect("event store lock poisoned");
        state.next_sequence = state.next_sequence.saturating_add(1);
        let event = HostEvent {
            sequence: state.next_sequence,
            timestamp_ms,
            event,
        };
        while state.history.len() >= self.capacity {
            state.history.pop_front();
        }
        state.history.push_back(event.clone());
        state
            .subscribers
            .retain(|_, sender| match sender.try_send(event.clone()) {
                Ok(()) | Err(TrySendError::Full(_)) => true,
                Err(TrySendError::Disconnected(_)) => false,
            });
        event
    }

    fn subscribe(
        &self,
        subscription_id: SubscriptionId,
        after_sequence: Option<u64>,
        limit: u32,
        capacity: usize,
    ) -> (SubscribeEventsResult, EventSubscription) {
        let mut state = self.state.lock().expect("event store lock poisoned");
        let after = after_sequence.unwrap_or(0);
        let max = usize::try_from(limit.clamp(1, 10_000)).unwrap_or(10_000);
        let replay = state
            .history
            .iter()
            .filter(|event| event.sequence > after)
            .take(max)
            .cloned()
            .collect();
        let oldest_available_sequence = state
            .history
            .front()
            .map_or(state.next_sequence.saturating_add(1), |event| {
                event.sequence
            });
        let replay_gap =
            after_sequence.is_some_and(|after| after.saturating_add(1) < oldest_available_sequence);
        let next_sequence = state.next_sequence;
        let (sender, receiver) = mpsc::sync_channel(capacity.max(1));
        state.subscribers.insert(subscription_id.clone(), sender);
        (
            SubscribeEventsResult {
                subscription_id: subscription_id.clone(),
                replay,
                oldest_available_sequence,
                next_sequence,
                replay_gap,
            },
            EventSubscription {
                id: subscription_id,
                receiver,
            },
        )
    }
}

impl FakeHost {
    pub fn new(config: FakeHostConfig) -> Self {
        Self::with_registry(config, Arc::new(MemoryRegistry::new()))
            .expect("empty memory registry is valid")
    }

    pub fn with_registry(
        config: FakeHostConfig,
        registry: Arc<dyn RegistryStore>,
    ) -> Result<Self, ApiError> {
        Self::build(
            config,
            registry,
            Arc::new(FakeProcessBackend::default()),
            false,
        )
    }

    pub fn with_process_backend(
        config: FakeHostConfig,
        registry: Arc<dyn RegistryStore>,
        process_backend: Arc<dyn ProcessBackend>,
    ) -> Result<Self, ApiError> {
        Self::build(config, registry, process_backend, true)
    }

    fn build(
        config: FakeHostConfig,
        registry: Arc<dyn RegistryStore>,
        process_backend: Arc<dyn ProcessBackend>,
        wall_clock: bool,
    ) -> Result<Self, ApiError> {
        let snapshot = registry.load().map_err(registry_error)?;
        for (service_id, stored) in &snapshot.definitions {
            stored.definition.validate().map_err(definition_error)?;
            if &stored.definition.id != service_id || stored.definition_revision == 0 {
                return Err(ApiError::new(
                    ErrorCode::InvalidArgument,
                    "registry contains an invalid service identity or zero definition revision",
                )
                .detail("service_id", service_id.as_str()));
            }
        }
        validate_dependency_graph(&snapshot.definitions).map_err(graph_error)?;
        let services = snapshot
            .definitions
            .iter()
            .map(|(service_id, stored)| {
                (
                    service_id.clone(),
                    ServiceSnapshot {
                        definition: stored.definition.clone(),
                        runtime: RuntimeState::stopped(stored.definition_revision),
                    },
                )
            })
            .collect();
        let host = Self {
            logs: LogStore::new(config.log_buffer),
            events: EventStore::new(config.event_history_capacity),
            pending_event_streams: Mutex::new(BTreeMap::new()),
            process_backend,
            config,
            registry,
            inner: Mutex::new(Inner {
                phase: HostPhase::Running,
                registry_revision: snapshot.revision,
                services,
                operations: BTreeMap::new(),
                idempotency: BTreeMap::new(),
                known_runs: BTreeMap::new(),
                restart_attempts: BTreeMap::new(),
                serial: 0,
                start_order: Vec::new(),
                stop_order: Vec::new(),
                wall_clock,
                last_timestamp_ms: 0,
            }),
        };
        host.apply_host_start_policy()?;
        Ok(host)
    }

    fn apply_host_start_policy(&self) -> Result<(), ApiError> {
        let mut inner = self.inner.lock().expect("fake host lock poisoned");
        let registry = self.current_registry(&inner)?;
        let order = topological_order(&registry.definitions).map_err(graph_error)?;
        if !order.iter().any(|service_id| {
            inner.services[service_id].definition.start_policy == StartPolicy::OnHostStart
        }) {
            return Ok(());
        }
        let operation_id = self.begin_operation(&mut inner, OperationKind::Reconcile, None);
        for service_id in order {
            if inner.services[&service_id].definition.start_policy == StartPolicy::OnHostStart {
                if let Err(error) = self.start_recursive(&mut inner, &service_id, &operation_id) {
                    self.finish_operation(&mut inner, &operation_id, Some(error.clone()));
                    return Err(error);
                }
            }
        }
        self.finish_operation(&mut inner, &operation_id, None);
        Ok(())
    }

    pub fn handle(&self, envelope: RequestEnvelope) -> ResponseEnvelope {
        if envelope.protocol.major != PROTOCOL_MAJOR {
            return protocol_error(envelope.request_id, envelope.protocol);
        }
        let request_id = envelope.request_id;
        let outcome = if request_id.is_empty() || request_id.len() > 256 {
            Err(ApiError::new(
                ErrorCode::InvalidRequest,
                "request_id must contain between 1 and 256 bytes",
            ))
        } else {
            self.execute(envelope.request)
        };
        ResponseEnvelope {
            protocol: ProtocolVersion::V1,
            capabilities: host_capabilities(),
            request_id,
            outcome: match outcome {
                Ok(response) => ResponseOutcome::Ok {
                    response: Box::new(response),
                },
                Err(error) => ResponseOutcome::Error { error },
            },
        }
    }

    fn execute(&self, request: Request) -> Result<Response, ApiError> {
        if let Some(mutation) = mutation_options(&request) {
            validate_idempotency_key(&mutation.idempotency_key)?;
            let fingerprint = serde_json::to_string(&request).map_err(|error| {
                ApiError::new(
                    ErrorCode::Internal,
                    format!("failed to fingerprint request: {error}"),
                )
            })?;
            let key = mutation.idempotency_key.clone();
            let mut inner = self.inner.lock().expect("fake host lock poisoned");
            if let Some(record) = inner.idempotency.get(&key) {
                if record.fingerprint == fingerprint {
                    return record.outcome.clone();
                }
                return Err(ApiError::new(
                    ErrorCode::IdempotencyConflict,
                    "idempotency key was already used for a different mutation",
                )
                .detail("idempotency_key", key));
            }
            let outcome = self.execute_mutation(&mut inner, request);
            inner.idempotency.insert(
                key,
                IdempotencyRecord {
                    fingerprint,
                    outcome: outcome.clone(),
                },
            );
            return outcome;
        }

        match request {
            Request::GetHostInfo(_) => Ok(Response::GetHostInfo(HostInfo {
                product: "persistent-runtime-host".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                host_instance_id: self.config.host_instance_id.clone(),
                protocol: ProtocolVersion::V1,
                capabilities: host_capabilities(),
                backend: self.process_backend.name().to_owned(),
            })),
            Request::GetHostStatus(_) => {
                let inner = self.inner.lock().expect("fake host lock poisoned");
                Ok(Response::GetHostStatus(host_status(&inner)))
            }
            Request::ListServices(_) => {
                let inner = self.inner.lock().expect("fake host lock poisoned");
                Ok(Response::ListServices(ListServicesResult {
                    services: inner.services.values().cloned().collect(),
                }))
            }
            Request::GetService(params) => {
                let inner = self.inner.lock().expect("fake host lock poisoned");
                Ok(Response::GetService(
                    service(&inner, &params.service_id)?.clone(),
                ))
            }
            Request::GetOperation(params) => {
                let inner = self.inner.lock().expect("fake host lock poisoned");
                let operation = inner.operations.get(&params.operation_id).ok_or_else(|| {
                    ApiError::new(ErrorCode::OperationNotFound, "operation was not found")
                        .detail("operation_id", params.operation_id.as_str())
                })?;
                Ok(Response::GetOperation(operation.clone()))
            }
            Request::ReadLogs(params) => {
                {
                    let inner = self.inner.lock().expect("fake host lock poisoned");
                    service(&inner, &params.service_id)?;
                }
                Ok(Response::ReadLogs(self.logs.read(
                    &params.service_id,
                    params.run_id.as_ref(),
                    params.after_sequence,
                    params.limit,
                )))
            }
            Request::SubscribeEvents(params) => {
                let subscription_id = {
                    let mut inner = self.inner.lock().expect("fake host lock poisoned");
                    SubscriptionId::new(format!(
                        "subscription-{}-{:016x}",
                        self.config.host_instance_id,
                        inner.next_serial()
                    ))
                };
                let (result, stream) = self.events.subscribe(
                    subscription_id.clone(),
                    params.after_sequence,
                    params.replay_limit,
                    self.config.event_subscriber_capacity,
                );
                self.pending_event_streams
                    .lock()
                    .expect("pending event stream lock poisoned")
                    .insert(subscription_id, stream);
                Ok(Response::SubscribeEvents(result))
            }
            _ => Err(ApiError::new(
                ErrorCode::Internal,
                "mutation dispatch classification failed",
            )),
        }
    }

    fn execute_mutation(&self, inner: &mut Inner, request: Request) -> Result<Response, ApiError> {
        match request {
            Request::ShutdownHost(params) => self.shutdown_host(inner, params),
            Request::EnsureRunning(params) => self.ensure_running(inner, params),
            Request::EnsureStopped(params) => self.ensure_stopped(inner, params),
            Request::Restart(params) => self.restart(inner, params),
            Request::Reconcile(params) => self.reconcile(inner, params),
            Request::RegisterService(params) => self.register_service(inner, params),
            Request::UpdateService(params) => self.update_service(inner, params),
            Request::RemoveService(params) => self.remove_service(inner, params),
            _ => Err(ApiError::new(
                ErrorCode::Internal,
                "read request entered mutation dispatch",
            )),
        }
    }

    fn register_service(
        &self,
        inner: &mut Inner,
        params: RegisterServiceParams,
    ) -> Result<Response, ApiError> {
        ensure_host_running(inner)?;
        params.definition.validate().map_err(definition_error)?;
        let mut registry = self.current_registry(inner)?;
        if let Some(expected) = params.mutation.expected_revision {
            ensure_revision(expected, registry.revision, "registry")?;
        }
        if registry.definitions.contains_key(&params.definition.id) {
            return Err(
                ApiError::new(ErrorCode::AlreadyExists, "service is already registered")
                    .detail("service_id", params.definition.id.as_str()),
            );
        }
        let service_id = params.definition.id.clone();
        registry.definitions.insert(
            service_id.clone(),
            StoredDefinition {
                definition_revision: 1,
                definition: params.definition.clone(),
            },
        );
        validate_dependency_graph(&registry.definitions).map_err(graph_error)?;
        let operation_id = self.begin_operation(
            inner,
            OperationKind::RegisterService,
            Some(service_id.clone()),
        );
        let persisted = match self.persist_registry(inner, registry) {
            Ok(persisted) => persisted,
            Err(error) => {
                self.finish_operation(inner, &operation_id, Some(error.clone()));
                return Err(error);
            }
        };
        let mut runtime = RuntimeState::stopped(1);
        runtime.operation_id = Some(operation_id.clone());
        runtime.operation_phase = Some(OperationPhase::Running);
        let snapshot = ServiceSnapshot {
            definition: params.definition,
            runtime,
        };
        inner.services.insert(service_id.clone(), snapshot);
        let time = inner.tick();
        self.events.publish(
            time,
            HostEventKind::DefinitionRegistered {
                service_id: service_id.clone(),
                definition_revision: 1,
            },
        );
        let operation = self.finish_operation(inner, &operation_id, None);
        let snapshot = inner.services[&service_id].clone();
        Ok(Response::RegisterService(DefinitionMutationResult {
            operation,
            service: snapshot,
            registry_revision: persisted.revision,
        }))
    }

    fn update_service(
        &self,
        inner: &mut Inner,
        params: UpdateServiceParams,
    ) -> Result<Response, ApiError> {
        ensure_host_running(inner)?;
        params.definition.validate().map_err(definition_error)?;
        if params.definition.id != params.service_id {
            return Err(ApiError::new(
                ErrorCode::InvalidArgument,
                "definition ID must match the target service ID",
            ));
        }
        let current = service(inner, &params.service_id)?;
        if current.runtime.run_id.is_some() {
            return Err(ApiError::new(
                ErrorCode::ServiceBusy,
                "a running service definition cannot be updated",
            )
            .detail("service_id", params.service_id.as_str()));
        }
        if let Some(expected) = params.mutation.expected_revision {
            ensure_revision(expected, current.runtime.definition_revision, "definition")?;
        }
        let mut registry = self.current_registry(inner)?;
        let stored = registry
            .definitions
            .get(&params.service_id)
            .ok_or_else(|| {
                ApiError::new(ErrorCode::NotFound, "service is not registered")
                    .detail("service_id", params.service_id.as_str())
            })?;
        let next_revision = stored
            .definition_revision
            .checked_add(1)
            .ok_or_else(|| ApiError::new(ErrorCode::Internal, "definition revision overflow"))?;
        registry.definitions.insert(
            params.service_id.clone(),
            StoredDefinition {
                definition_revision: next_revision,
                definition: params.definition.clone(),
            },
        );
        validate_dependency_graph(&registry.definitions).map_err(graph_error)?;
        let operation_id = self.begin_operation(
            inner,
            OperationKind::UpdateService,
            Some(params.service_id.clone()),
        );
        let persisted = match self.persist_registry(inner, registry) {
            Ok(persisted) => persisted,
            Err(error) => {
                self.finish_operation(inner, &operation_id, Some(error.clone()));
                return Err(error);
            }
        };
        let snapshot = inner
            .services
            .get_mut(&params.service_id)
            .expect("runtime and registry remain aligned");
        snapshot.definition = params.definition;
        snapshot.runtime.definition_revision = next_revision;
        snapshot.runtime.operation_id = Some(operation_id.clone());
        snapshot.runtime.operation_phase = Some(OperationPhase::Running);
        let time = inner.tick();
        self.events.publish(
            time,
            HostEventKind::DefinitionUpdated {
                service_id: params.service_id.clone(),
                definition_revision: next_revision,
            },
        );
        let operation = self.finish_operation(inner, &operation_id, None);
        let snapshot = inner.services[&params.service_id].clone();
        Ok(Response::UpdateService(DefinitionMutationResult {
            operation,
            service: snapshot,
            registry_revision: persisted.revision,
        }))
    }

    fn remove_service(
        &self,
        inner: &mut Inner,
        params: RemoveServiceParams,
    ) -> Result<Response, ApiError> {
        ensure_host_running(inner)?;
        let current = service(inner, &params.service_id)?;
        if current.runtime.run_id.is_some() {
            return Err(ApiError::new(
                ErrorCode::ServiceBusy,
                "a running service cannot be removed",
            )
            .detail("service_id", params.service_id.as_str()));
        }
        if let Some(expected) = params.mutation.expected_revision {
            ensure_revision(expected, current.runtime.definition_revision, "definition")?;
        }
        let dependents = registered_dependents(inner, &params.service_id);
        if !dependents.is_empty() {
            return Err(dependency_in_use(&params.service_id, &dependents));
        }
        let mut registry = self.current_registry(inner)?;
        registry.definitions.remove(&params.service_id);
        let operation_id = self.begin_operation(
            inner,
            OperationKind::RemoveService,
            Some(params.service_id.clone()),
        );
        let persisted = match self.persist_registry(inner, registry) {
            Ok(persisted) => persisted,
            Err(error) => {
                self.finish_operation(inner, &operation_id, Some(error.clone()));
                return Err(error);
            }
        };
        inner.services.remove(&params.service_id);
        inner.restart_attempts.remove(&params.service_id);
        let time = inner.tick();
        self.events.publish(
            time,
            HostEventKind::DefinitionRemoved {
                service_id: params.service_id.clone(),
            },
        );
        let operation = self.finish_operation(inner, &operation_id, None);
        Ok(Response::RemoveService(RemoveServiceResult {
            operation,
            removed_service_id: params.service_id,
            registry_revision: persisted.revision,
        }))
    }

    fn ensure_running(
        &self,
        inner: &mut Inner,
        params: EnsureRunningParams,
    ) -> Result<Response, ApiError> {
        ensure_host_running(inner)?;
        self.current_registry(inner)?;
        check_target_revision(inner, &params.service_id, params.mutation.expected_revision)?;
        let operation_id = self.begin_operation(
            inner,
            OperationKind::EnsureRunning,
            Some(params.service_id.clone()),
        );
        if inner.services[&params.service_id].runtime.run_id.is_none() {
            inner
                .services
                .get_mut(&params.service_id)
                .expect("checked above")
                .runtime
                .restart_count = 0;
            inner.restart_attempts.remove(&params.service_id);
        }
        if let Err(error) = self.start_recursive(inner, &params.service_id, &operation_id) {
            self.finish_operation(inner, &operation_id, Some(error.clone()));
            return Err(error);
        }
        let operation = self.finish_operation(inner, &operation_id, None);
        let service = inner.services[&params.service_id].clone();
        Ok(Response::EnsureRunning(ServiceMutationResult {
            operation,
            service,
        }))
    }

    fn ensure_stopped(
        &self,
        inner: &mut Inner,
        params: EnsureStoppedParams,
    ) -> Result<Response, ApiError> {
        ensure_host_running(inner)?;
        self.current_registry(inner)?;
        check_target_revision(inner, &params.service_id, params.mutation.expected_revision)?;
        let dependents = running_dependents(inner, &params.service_id);
        if !params.cascade && !dependents.is_empty() {
            return Err(dependency_in_use(&params.service_id, &dependents));
        }
        let operation_id = self.begin_operation(
            inner,
            OperationKind::EnsureStopped,
            Some(params.service_id.clone()),
        );
        let stop_result = if params.cascade {
            let mut visited = BTreeSet::new();
            self.stop_cascade(inner, &params.service_id, &operation_id, &mut visited)
        } else {
            self.stop_one(inner, &params.service_id, &operation_id)
        };
        if let Err(error) = stop_result {
            self.finish_operation(inner, &operation_id, Some(error.clone()));
            return Err(error);
        }
        let operation = self.finish_operation(inner, &operation_id, None);
        let service = inner.services[&params.service_id].clone();
        Ok(Response::EnsureStopped(ServiceMutationResult {
            operation,
            service,
        }))
    }

    fn restart(&self, inner: &mut Inner, params: RestartParams) -> Result<Response, ApiError> {
        ensure_host_running(inner)?;
        let registry = self.current_registry(inner)?;
        let dependency_order = topological_order(&registry.definitions).map_err(graph_error)?;
        check_target_revision(inner, &params.service_id, params.mutation.expected_revision)?;
        let dependents = running_dependents(inner, &params.service_id);
        if !params.cascade && !dependents.is_empty() {
            return Err(dependency_in_use(&params.service_id, &dependents));
        }
        let previously_running: BTreeSet<_> = if params.cascade {
            transitive_running_dependents(inner, &params.service_id)
        } else {
            BTreeSet::new()
        };
        let operation_id = self.begin_operation(
            inner,
            OperationKind::Restart,
            Some(params.service_id.clone()),
        );
        let stop_result = if params.cascade {
            let mut visited = BTreeSet::new();
            self.stop_cascade(inner, &params.service_id, &operation_id, &mut visited)
        } else {
            self.stop_one(inner, &params.service_id, &operation_id)
        };
        if let Err(error) = stop_result {
            self.finish_operation(inner, &operation_id, Some(error.clone()));
            return Err(error);
        }
        inner
            .services
            .get_mut(&params.service_id)
            .unwrap()
            .runtime
            .restart_count = 0;
        inner.restart_attempts.remove(&params.service_id);
        if let Err(error) = self.start_recursive(inner, &params.service_id, &operation_id) {
            self.finish_operation(inner, &operation_id, Some(error.clone()));
            return Err(error);
        }
        if !previously_running.is_empty() {
            for service_id in dependency_order {
                if previously_running.contains(&service_id) {
                    if let Err(error) = self.start_recursive(inner, &service_id, &operation_id) {
                        self.finish_operation(inner, &operation_id, Some(error.clone()));
                        return Err(error);
                    }
                }
            }
        }
        let operation = self.finish_operation(inner, &operation_id, None);
        let service = inner.services[&params.service_id].clone();
        Ok(Response::Restart(ServiceMutationResult {
            operation,
            service,
        }))
    }

    fn reconcile(&self, inner: &mut Inner, params: ReconcileParams) -> Result<Response, ApiError> {
        ensure_host_running(inner)?;
        let registry = self.current_registry(inner)?;
        if let Some(service_id) = &params.service_id {
            check_target_revision(inner, service_id, params.mutation.expected_revision)?;
        } else if let Some(expected) = params.mutation.expected_revision {
            ensure_revision(expected, inner.registry_revision, "registry")?;
        }
        let operation_id =
            self.begin_operation(inner, OperationKind::Reconcile, params.service_id.clone());
        let selected: BTreeSet<ServiceId> = match &params.service_id {
            Some(service_id) => [service_id.clone()].into_iter().collect(),
            None => registry.definitions.keys().cloned().collect(),
        };
        for service_id in topological_order(&registry.definitions).map_err(graph_error)? {
            if !selected.contains(&service_id) {
                continue;
            }
            let snapshot = &inner.services[&service_id];
            if snapshot.runtime.desired_state == DesiredState::Running
                || snapshot.definition.start_policy == StartPolicy::OnHostStart
            {
                if let Err(error) = self.start_recursive(inner, &service_id, &operation_id) {
                    self.finish_operation(inner, &operation_id, Some(error.clone()));
                    return Err(error);
                }
            }
        }
        let operation = self.finish_operation(inner, &operation_id, None);
        Ok(Response::Reconcile(ReconcileResult {
            operation,
            services: inner.services.values().cloned().collect(),
        }))
    }

    fn shutdown_host(
        &self,
        inner: &mut Inner,
        _params: ShutdownHostParams,
    ) -> Result<Response, ApiError> {
        ensure_host_running(inner)?;
        let registry = self.current_registry(inner)?;
        let mut order = topological_order(&registry.definitions).map_err(graph_error)?;
        order.reverse();
        let operation_id = self.begin_operation(inner, OperationKind::ShutdownHost, None);
        // This phase change is the restart/start fence and occurs before any
        // process is asked to stop.
        inner.phase = HostPhase::ShuttingDown;
        let time = inner.tick();
        self.events.publish(
            time,
            HostEventKind::HostPhaseChanged {
                phase: HostPhase::ShuttingDown,
            },
        );
        let mut first_error = None;
        for service_id in order {
            if let Err(error) = self.stop_one(inner, &service_id, &operation_id) {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        if let Some(error) = first_error {
            self.finish_operation(inner, &operation_id, Some(error.clone()));
            return Err(error);
        }
        inner.phase = HostPhase::Stopped;
        let time = inner.tick();
        self.events.publish(
            time,
            HostEventKind::HostPhaseChanged {
                phase: HostPhase::Stopped,
            },
        );
        let operation = self.finish_operation(inner, &operation_id, None);
        Ok(Response::ShutdownHost(ShutdownHostResult {
            operation,
            final_phase: HostPhase::Stopped,
        }))
    }

    fn start_recursive(
        &self,
        inner: &mut Inner,
        service_id: &ServiceId,
        operation_id: &OperationId,
    ) -> Result<(), ApiError> {
        ensure_host_running(inner)?;
        let dependencies = service(inner, service_id)?.definition.dependencies.clone();
        for dependency_id in dependencies {
            self.start_recursive(inner, &dependency_id, operation_id)?;
        }
        if inner.services[service_id].runtime.run_id.is_some() {
            let runtime = &mut inner.services.get_mut(service_id).unwrap().runtime;
            runtime.desired_state = DesiredState::Running;
            runtime.operation_id = Some(operation_id.clone());
            runtime.operation_phase = Some(OperationPhase::Running);
            return Ok(());
        }

        let serial = inner.next_serial();
        let run_id = RunId::new(format!(
            "run-{}-{:016x}",
            self.config.host_instance_id, serial
        ));
        let timestamp = inner.tick();
        let definition = inner.services[service_id].definition.clone();
        let process = match self.process_backend.start(&definition, &run_id) {
            Ok(process) => process,
            Err(error) => {
                let runtime = &mut inner.services.get_mut(service_id).unwrap().runtime;
                runtime.desired_state = DesiredState::Running;
                runtime.observed_state = ObservedState::Failed;
                runtime.health = HealthState::Unknown;
                runtime.operation_id = Some(operation_id.clone());
                runtime.operation_phase = Some(OperationPhase::Running);
                self.publish_service_state(inner, service_id, timestamp);
                return Err(process_error(error));
            }
        };
        let snapshot = inner.services.get_mut(service_id).expect("checked above");
        snapshot.runtime.desired_state = DesiredState::Running;
        snapshot.runtime.observed_state = ObservedState::Running;
        snapshot.runtime.health = if snapshot.definition.readiness_probe.is_some() {
            HealthState::Healthy
        } else {
            HealthState::NotApplicable
        };
        snapshot.runtime.run_id = Some(run_id.clone());
        snapshot.runtime.operation_id = Some(operation_id.clone());
        snapshot.runtime.operation_phase = Some(OperationPhase::Running);
        snapshot.runtime.process = Some(process);
        snapshot.runtime.started_at_ms = Some(timestamp);
        snapshot.runtime.last_stop_outcome = None;
        let desired_state = snapshot.runtime.desired_state;
        let observed_state = snapshot.runtime.observed_state;
        let health = snapshot.runtime.health;
        inner
            .known_runs
            .entry(service_id.clone())
            .or_default()
            .insert(run_id.clone());
        inner.start_order.push(service_id.clone());
        self.events.publish(
            timestamp,
            HostEventKind::ServiceStateChanged {
                service_id: service_id.clone(),
                run_id: Some(run_id),
                desired_state,
                observed_state,
                health,
            },
        );
        Ok(())
    }

    fn stop_cascade(
        &self,
        inner: &mut Inner,
        service_id: &ServiceId,
        operation_id: &OperationId,
        visited: &mut BTreeSet<ServiceId>,
    ) -> Result<(), ApiError> {
        if !visited.insert(service_id.clone()) {
            return Ok(());
        }
        let dependents = direct_running_dependents(inner, service_id);
        for dependent_id in dependents {
            self.stop_cascade(inner, &dependent_id, operation_id, visited)?;
        }
        self.stop_one(inner, service_id, operation_id)
    }

    fn stop_one(
        &self,
        inner: &mut Inner,
        service_id: &ServiceId,
        operation_id: &OperationId,
    ) -> Result<(), ApiError> {
        let Some(snapshot) = inner.services.get(service_id) else {
            return Ok(());
        };
        let run_id = snapshot.runtime.run_id.clone();
        let shutdown_policy = snapshot.definition.shutdown_policy.clone();
        let time = inner.tick();
        let snapshot = inner.services.get_mut(service_id).expect("checked above");
        snapshot.runtime.desired_state = DesiredState::Stopped;
        snapshot.runtime.operation_id = Some(operation_id.clone());
        snapshot.runtime.operation_phase = Some(OperationPhase::Running);
        let outcome = if let Some(run_id) = &run_id {
            match self
                .process_backend
                .stop(service_id, run_id, &shutdown_policy)
            {
                Ok(outcome) => Some(outcome),
                Err(error) => {
                    let snapshot = inner.services.get_mut(service_id).expect("checked above");
                    snapshot.runtime.observed_state = ObservedState::Failed;
                    snapshot.runtime.health = HealthState::Unknown;
                    self.publish_service_state(inner, service_id, time);
                    return Err(process_error(error));
                }
            }
        } else {
            None
        };
        if outcome == Some(StopOutcome::TimedOut) {
            let snapshot = inner.services.get_mut(service_id).expect("checked above");
            snapshot.runtime.observed_state = ObservedState::Failed;
            snapshot.runtime.health = HealthState::Unknown;
            snapshot.runtime.last_stop_outcome = Some(StopOutcome::TimedOut);
            self.publish_service_state(inner, service_id, time);
            return Err(ApiError::new(
                ErrorCode::Internal,
                "service did not exit within its bounded shutdown policy",
            )
            .detail("service_id", service_id.as_str())
            .detail("run_id", run_id.as_ref().unwrap().as_str())
            .retryable());
        }
        let snapshot = inner.services.get_mut(service_id).expect("checked above");
        snapshot.runtime.observed_state = ObservedState::Stopped;
        snapshot.runtime.health = HealthState::NotApplicable;
        snapshot.runtime.process = None;
        snapshot.runtime.started_at_ms = None;
        snapshot.runtime.run_id = None;
        if let Some(run_id) = run_id {
            let outcome = outcome.expect("a running occurrence has a stop outcome");
            snapshot.runtime.last_exit = Some(LastExit {
                run_id: run_id.clone(),
                exited_at_ms: time,
                exit_code: Some(0),
                unexpected: false,
            });
            snapshot.runtime.last_stop_outcome = Some(outcome);
            inner.stop_order.push(service_id.clone());
            self.events.publish(
                time,
                HostEventKind::ServiceStopped {
                    service_id: service_id.clone(),
                    run_id,
                    outcome,
                },
            );
        }
        self.publish_service_state(inner, service_id, time);
        Ok(())
    }

    fn begin_operation(
        &self,
        inner: &mut Inner,
        kind: OperationKind,
        service_id: Option<ServiceId>,
    ) -> OperationId {
        let operation_id = OperationId::new(format!(
            "operation-{}-{:016x}",
            self.config.host_instance_id,
            inner.next_serial()
        ));
        let created_at_ms = inner.tick();
        let mut operation = Operation {
            id: operation_id.clone(),
            kind,
            service_id,
            phase: OperationPhase::Queued,
            created_at_ms,
            updated_at_ms: created_at_ms,
            error: None,
        };
        inner
            .operations
            .insert(operation_id.clone(), operation.clone());
        self.events.publish(
            created_at_ms,
            HostEventKind::OperationChanged {
                operation: operation.clone(),
            },
        );
        operation.phase = OperationPhase::Running;
        operation.updated_at_ms = inner.tick();
        inner
            .operations
            .insert(operation_id.clone(), operation.clone());
        self.events.publish(
            operation.updated_at_ms,
            HostEventKind::OperationChanged { operation },
        );
        operation_id
    }

    fn finish_operation(
        &self,
        inner: &mut Inner,
        operation_id: &OperationId,
        error: Option<ApiError>,
    ) -> Operation {
        let updated_at_ms = inner.tick();
        let operation = inner
            .operations
            .get_mut(operation_id)
            .expect("operation was created before completion");
        operation.phase = if error.is_some() {
            OperationPhase::Failed
        } else {
            OperationPhase::Succeeded
        };
        operation.updated_at_ms = updated_at_ms;
        operation.error = error;
        let operation = operation.clone();
        for snapshot in inner.services.values_mut() {
            if snapshot.runtime.operation_id.as_ref() == Some(operation_id) {
                snapshot.runtime.operation_phase = Some(operation.phase);
            }
        }
        self.events.publish(
            updated_at_ms,
            HostEventKind::OperationChanged {
                operation: operation.clone(),
            },
        );
        operation
    }

    fn publish_service_state(&self, inner: &Inner, service_id: &ServiceId, timestamp_ms: u64) {
        let runtime = &inner.services[service_id].runtime;
        self.events.publish(
            timestamp_ms,
            HostEventKind::ServiceStateChanged {
                service_id: service_id.clone(),
                run_id: runtime.run_id.clone(),
                desired_state: runtime.desired_state,
                observed_state: runtime.observed_state,
                health: runtime.health,
            },
        );
    }

    fn current_registry(&self, inner: &Inner) -> Result<RegistrySnapshot, ApiError> {
        let registry = self.registry.load().map_err(registry_error)?;
        if registry.revision != inner.registry_revision {
            return Err(ApiError::new(
                ErrorCode::RegistryConflict,
                "registry changed outside this host instance",
            )
            .detail("host_revision", inner.registry_revision)
            .detail("store_revision", registry.revision)
            .retryable());
        }
        Ok(registry)
    }

    fn persist_registry(
        &self,
        inner: &mut Inner,
        registry: RegistrySnapshot,
    ) -> Result<RegistrySnapshot, ApiError> {
        let persisted = self
            .registry
            .replace(registry.revision, registry.definitions)
            .map_err(registry_error)?;
        inner.registry_revision = persisted.revision;
        Ok(persisted)
    }

    /// Test/backend hook representing bytes drained from a concrete process.
    /// Old occurrences remain readable, but only known `(service, run)` pairs
    /// are accepted, which prevents cross-generation attribution.
    pub fn emit_log(
        &self,
        service_id: &ServiceId,
        run_id: &RunId,
        stream: LogStream,
        bytes: &[u8],
    ) -> Result<LogEntry, ApiError> {
        let timestamp = {
            let mut inner = self.inner.lock().expect("fake host lock poisoned");
            if !inner
                .known_runs
                .get(service_id)
                .is_some_and(|runs| runs.contains(run_id))
            {
                return Err(ApiError::new(
                    ErrorCode::LogUnavailable,
                    "service run is not known to this host",
                ));
            }
            inner.tick()
        };
        Ok(self
            .logs
            .append(service_id, run_id, timestamp, stream, bytes))
    }

    pub fn subscribe_logs(
        &self,
        service_id: ServiceId,
        run_id: Option<RunId>,
        capacity: usize,
    ) -> LogSubscription {
        self.logs.subscribe(service_id, run_id, capacity)
    }

    /// Transfers the live stream created by a successful `SubscribeEvents`
    /// request to the transport that owns that request. It can be taken once.
    pub fn take_event_stream(&self, subscription_id: &SubscriptionId) -> Option<EventSubscription> {
        self.pending_event_streams
            .lock()
            .expect("pending event stream lock poisoned")
            .remove(subscription_id)
    }

    pub fn report_log_failure(
        &self,
        service_id: &ServiceId,
        run_id: &RunId,
        message: impl Into<String>,
    ) -> Result<(), ApiError> {
        let message = message.into();
        let timestamp = {
            let mut inner = self.inner.lock().expect("fake host lock poisoned");
            if !inner
                .known_runs
                .get(service_id)
                .is_some_and(|runs| runs.contains(run_id))
            {
                return Err(ApiError::new(
                    ErrorCode::LogUnavailable,
                    "service run is not known to this host",
                ));
            }
            inner.tick()
        };
        self.logs.report_error(service_id, message.clone());
        self.events.publish(
            timestamp,
            HostEventKind::LogFailure {
                service_id: service_id.clone(),
                run_id: run_id.clone(),
                message,
            },
        );
        Ok(())
    }

    pub fn report_containment_failure(
        &self,
        service_id: &ServiceId,
        run_id: &RunId,
        message: impl Into<String>,
    ) -> Result<bool, ApiError> {
        self.report_log_failure(service_id, run_id, message)?;
        let mut inner = self.inner.lock().expect("fake host lock poisoned");
        if service(&inner, service_id)?.runtime.run_id.as_ref() != Some(run_id) {
            return Ok(false);
        }
        let timestamp = inner.tick();
        let runtime = &mut inner
            .services
            .get_mut(service_id)
            .expect("checked above")
            .runtime;
        runtime.observed_state = ObservedState::Failed;
        runtime.health = HealthState::Unknown;
        self.publish_service_state(&inner, service_id, timestamp);
        Ok(true)
    }

    /// Test/backend hook for readiness and health observations. Health is
    /// occurrence-scoped: it neither clears process identity nor starts a
    /// replacement occurrence, and stale observations are ignored.
    pub fn set_health(
        &self,
        service_id: &ServiceId,
        run_id: &RunId,
        health: HealthState,
    ) -> Result<bool, ApiError> {
        let mut inner = self.inner.lock().expect("fake host lock poisoned");
        if service(&inner, service_id)?.runtime.run_id.as_ref() != Some(run_id) {
            return Ok(false);
        }
        let timestamp = inner.tick();
        inner
            .services
            .get_mut(service_id)
            .expect("checked above")
            .runtime
            .health = health;
        self.publish_service_state(&inner, service_id, timestamp);
        Ok(true)
    }

    /// Simulates an asynchronously observed process exit. A notification for
    /// an older run is ignored and cannot change the current occurrence.
    pub fn simulate_exit(
        &self,
        service_id: &ServiceId,
        run_id: &RunId,
        exit_code: Option<i32>,
    ) -> Result<bool, ApiError> {
        let observation = self.observe_backend_exit(service_id, run_id, exit_code)?;
        if let Some(restart) = observation.restart {
            self.execute_restart_directive(&restart)?;
        }
        Ok(observation.observed)
    }

    pub(crate) fn observe_backend_exit(
        &self,
        service_id: &ServiceId,
        run_id: &RunId,
        exit_code: Option<i32>,
    ) -> Result<ExitObservation, ApiError> {
        let mut inner = self.inner.lock().expect("fake host lock poisoned");
        let snapshot = service(&inner, service_id)?;
        if snapshot.runtime.run_id.as_ref() != Some(run_id) {
            return Ok(ExitObservation {
                observed: false,
                restart: None,
            });
        }
        let timestamp = inner.tick();
        let host_running = inner.phase == HostPhase::Running;
        let (desired_state, restart_policy) = {
            let snapshot = inner.services.get_mut(service_id).expect("checked above");
            snapshot.runtime.process = None;
            snapshot.runtime.run_id = None;
            snapshot.runtime.started_at_ms = None;
            snapshot.runtime.health = HealthState::Unknown;
            snapshot.runtime.observed_state = ObservedState::Failed;
            snapshot.runtime.last_exit = Some(LastExit {
                run_id: run_id.clone(),
                exited_at_ms: timestamp,
                exit_code,
                unexpected: true,
            });
            (
                snapshot.runtime.desired_state,
                snapshot.definition.restart_policy.clone(),
            )
        };
        let restart = if host_running && desired_state == DesiredState::Running {
            match restart_policy {
                RestartPolicy::Never => None,
                RestartPolicy::BoundedOnFailure {
                    max_restarts,
                    window_ms,
                    backoff_ms,
                } => {
                    let attempts = inner
                        .restart_attempts
                        .entry(service_id.clone())
                        .or_default();
                    let cutoff = timestamp.saturating_sub(window_ms);
                    attempts.retain(|attempt| *attempt >= cutoff);
                    let allowed = attempts.len() < max_restarts as usize;
                    if allowed {
                        attempts.push_back(timestamp);
                    }
                    inner
                        .services
                        .get_mut(service_id)
                        .expect("checked above")
                        .runtime
                        .restart_count = u32::try_from(attempts.len()).unwrap_or(u32::MAX);
                    allowed.then(|| RestartDirective {
                        service_id: service_id.clone(),
                        failed_run_id: run_id.clone(),
                        backoff_ms,
                    })
                }
            }
        } else {
            None
        };
        self.events.publish(
            timestamp,
            HostEventKind::ServiceExited {
                service_id: service_id.clone(),
                run_id: run_id.clone(),
                exit_code,
                unexpected: true,
            },
        );
        self.publish_service_state(&inner, service_id, timestamp);
        Ok(ExitObservation {
            observed: true,
            restart,
        })
    }

    pub(crate) fn execute_restart_directive(
        &self,
        directive: &RestartDirective,
    ) -> Result<bool, ApiError> {
        let mut inner = self.inner.lock().expect("fake host lock poisoned");
        let Some(snapshot) = inner.services.get(&directive.service_id) else {
            return Ok(false);
        };
        if inner.phase != HostPhase::Running
            || snapshot.runtime.desired_state != DesiredState::Running
            || snapshot.runtime.run_id.is_some()
            || snapshot.runtime.last_exit.as_ref().map(|exit| &exit.run_id)
                != Some(&directive.failed_run_id)
        {
            return Ok(false);
        }
        let operation_id = self.begin_operation(
            &mut inner,
            OperationKind::EnsureRunning,
            Some(directive.service_id.clone()),
        );
        if let Err(error) = self.start_recursive(&mut inner, &directive.service_id, &operation_id) {
            self.finish_operation(&mut inner, &operation_id, Some(error.clone()));
            return Err(error);
        }
        self.finish_operation(&mut inner, &operation_id, None);
        Ok(true)
    }

    pub fn service_snapshot(&self, service_id: &ServiceId) -> Option<ServiceSnapshot> {
        self.inner
            .lock()
            .expect("fake host lock poisoned")
            .services
            .get(service_id)
            .cloned()
    }

    pub fn service_ids(&self) -> Vec<ServiceId> {
        self.inner
            .lock()
            .expect("fake host lock poisoned")
            .services
            .keys()
            .cloned()
            .collect()
    }

    pub fn host_phase(&self) -> HostPhase {
        self.inner.lock().expect("fake host lock poisoned").phase
    }

    pub fn complete_shutdown_if_quiescent(&self) -> bool {
        let mut inner = self.inner.lock().expect("fake host lock poisoned");
        if inner.phase != HostPhase::ShuttingDown
            || inner
                .services
                .values()
                .any(|service| service.runtime.run_id.is_some())
        {
            return false;
        }
        inner.phase = HostPhase::Stopped;
        let timestamp = inner.tick();
        self.events.publish(
            timestamp,
            HostEventKind::HostPhaseChanged {
                phase: HostPhase::Stopped,
            },
        );
        true
    }

    pub fn seed_log_sequence(&self, service_id: &ServiceId, sequence: u64) {
        self.logs.seed_sequence(service_id, sequence);
    }

    pub fn operation_snapshot(&self, operation_id: &OperationId) -> Option<Operation> {
        self.inner
            .lock()
            .expect("fake host lock poisoned")
            .operations
            .get(operation_id)
            .cloned()
    }

    pub fn recorded_start_order(&self) -> Vec<ServiceId> {
        self.inner
            .lock()
            .expect("fake host lock poisoned")
            .start_order
            .clone()
    }

    pub fn recorded_stop_order(&self) -> Vec<ServiceId> {
        self.inner
            .lock()
            .expect("fake host lock poisoned")
            .stop_order
            .clone()
    }

    pub fn clear_order_history(&self) {
        let mut inner = self.inner.lock().expect("fake host lock poisoned");
        inner.start_order.clear();
        inner.stop_order.clear();
    }
}

impl Inner {
    fn next_serial(&mut self) -> u64 {
        self.serial = self.serial.saturating_add(1);
        self.serial
    }

    fn tick(&mut self) -> u64 {
        if !self.wall_clock {
            // The fake backend deliberately uses deterministic logical time.
            return self.next_serial();
        }
        let wall_clock = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        self.last_timestamp_ms = wall_clock.max(self.last_timestamp_ms.saturating_add(1));
        self.last_timestamp_ms
    }
}

fn mutation_options(request: &Request) -> Option<&MutationOptions> {
    match request {
        Request::ShutdownHost(params) => Some(&params.mutation),
        Request::EnsureRunning(params) => Some(&params.mutation),
        Request::EnsureStopped(params) => Some(&params.mutation),
        Request::Restart(params) => Some(&params.mutation),
        Request::Reconcile(params) => Some(&params.mutation),
        Request::RegisterService(params) => Some(&params.mutation),
        Request::UpdateService(params) => Some(&params.mutation),
        Request::RemoveService(params) => Some(&params.mutation),
        _ => None,
    }
}

fn validate_idempotency_key(key: &str) -> Result<(), ApiError> {
    if key.is_empty() || key.len() > 256 || key.contains('\0') {
        return Err(ApiError::new(
            ErrorCode::InvalidArgument,
            "idempotency key must contain between 1 and 256 non-NUL bytes",
        ));
    }
    Ok(())
}

fn host_status(inner: &Inner) -> HostStatus {
    HostStatus {
        phase: inner.phase,
        registry_revision: inner.registry_revision,
        services: inner.services.values().cloned().collect(),
    }
}

fn service<'a>(inner: &'a Inner, service_id: &ServiceId) -> Result<&'a ServiceSnapshot, ApiError> {
    inner.services.get(service_id).ok_or_else(|| {
        ApiError::new(ErrorCode::NotFound, "service is not registered")
            .detail("service_id", service_id.as_str())
    })
}

fn ensure_host_running(inner: &Inner) -> Result<(), ApiError> {
    if inner.phase != HostPhase::Running {
        return Err(ApiError::new(
            ErrorCode::HostShuttingDown,
            "host is shutting down or stopped and rejects new mutations",
        ));
    }
    Ok(())
}

fn ensure_revision(expected: u64, actual: u64, kind: &str) -> Result<(), ApiError> {
    if expected != actual {
        return Err(ApiError::new(
            ErrorCode::RevisionMismatch,
            format!("expected {kind} revision does not match"),
        )
        .detail("expected_revision", expected)
        .detail("actual_revision", actual));
    }
    Ok(())
}

fn check_target_revision(
    inner: &Inner,
    service_id: &ServiceId,
    expected: Option<u64>,
) -> Result<(), ApiError> {
    let snapshot = service(inner, service_id)?;
    if let Some(expected) = expected {
        ensure_revision(expected, snapshot.runtime.definition_revision, "definition")?;
    }
    Ok(())
}

fn registered_dependents(inner: &Inner, service_id: &ServiceId) -> Vec<ServiceId> {
    inner
        .services
        .iter()
        .filter(|(_, snapshot)| snapshot.definition.dependencies.contains(service_id))
        .map(|(candidate, _)| candidate.clone())
        .collect()
}

fn direct_running_dependents(inner: &Inner, service_id: &ServiceId) -> Vec<ServiceId> {
    inner
        .services
        .iter()
        .filter(|(_, snapshot)| {
            snapshot.runtime.run_id.is_some()
                && snapshot.definition.dependencies.contains(service_id)
        })
        .map(|(candidate, _)| candidate.clone())
        .collect()
}

fn transitive_running_dependents(inner: &Inner, service_id: &ServiceId) -> BTreeSet<ServiceId> {
    let mut result = BTreeSet::new();
    let mut pending = direct_running_dependents(inner, service_id);
    while let Some(candidate) = pending.pop() {
        if result.insert(candidate.clone()) {
            pending.extend(direct_running_dependents(inner, &candidate));
        }
    }
    result
}

fn running_dependents(inner: &Inner, service_id: &ServiceId) -> Vec<ServiceId> {
    transitive_running_dependents(inner, service_id)
        .into_iter()
        .collect()
}

fn dependency_in_use(service_id: &ServiceId, dependents: &[ServiceId]) -> ApiError {
    ApiError::new(
        ErrorCode::DependencyInUse,
        "service is required by registered or running dependents",
    )
    .detail("service_id", service_id.as_str())
    .detail(
        "dependents",
        json!(dependents.iter().map(ServiceId::as_str).collect::<Vec<_>>()),
    )
}

fn definition_error(error: DefinitionValidationError) -> ApiError {
    ApiError::new(ErrorCode::InvalidArgument, error.message).detail("field", error.field)
}

fn graph_error(error: DependencyGraphError) -> ApiError {
    match error {
        DependencyGraphError::Missing {
            service_id,
            dependency_id,
        } => ApiError::new(
            ErrorCode::DependencyMissing,
            "service references an unregistered dependency",
        )
        .detail("service_id", service_id.as_str())
        .detail("dependency_id", dependency_id.as_str()),
        DependencyGraphError::Cycle { path } => ApiError::new(
            ErrorCode::DependencyCycle,
            "service dependency graph contains a cycle",
        )
        .detail(
            "cycle",
            json!(path.iter().map(ServiceId::as_str).collect::<Vec<_>>()),
        ),
    }
}

fn registry_error(error: RegistryStoreError) -> ApiError {
    match error {
        RegistryStoreError::Conflict { expected, actual } => ApiError::new(
            ErrorCode::RegistryConflict,
            "registry compare-and-swap failed",
        )
        .detail("expected_revision", expected)
        .detail("actual_revision", actual)
        .retryable(),
        RegistryStoreError::Unavailable(message) => {
            ApiError::new(ErrorCode::Internal, message).retryable()
        }
    }
}

fn process_error(error: ProcessBackendError) -> ApiError {
    let api_error = ApiError::new(ErrorCode::Internal, "process backend operation failed")
        .detail("backend_error", error.message);
    if error.retryable {
        api_error.retryable()
    } else {
        api_error
    }
}
