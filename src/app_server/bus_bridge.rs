//! Agent-bus polling and native inter-agent submission for one app-server runtime.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use chrono::Utc;
use sha2::Digest;
use sha2::Sha256;

use crate::agent_bus::client::AgentBusHttpClient;
use crate::agent_bus::model::canonical_recipient_label;
use crate::agent_bus::model::AgentBusEnvelopeKind;
use crate::agent_bus::model::AgentBusMessage;
use crate::agent_bus::model::AgentBusRegisterRequest;
use crate::agent_bus::model::TaskServiceAssignmentMetadata;
use crate::agent_bus::model::TaskServiceCompletionMetadata;
use crate::agent_bus::model::TaskServiceWorkerFollowupMetadata;
use crate::agent_management::{
    AgentManagementMessageMetadata, AgentManagementSchema, AGENT_MANAGEMENT_START_CONTROL_TYPE,
    AGENT_MANAGEMENT_SYSTEM_SENDER,
};
use crate::management::v2::agent_bus_state::agent_bus_message_repository;
use crate::session::identity::default_cutex_session_id_for_codex_session;
use crate::task_service::ProviderReceipt;
use crate::task_service::TASK_SERVICE_PROVIDER_ACTION_SCHEMA;

use super::commands::AppServerCommands;
use super::commands::InterAgentContextPersistedReceipt;
use super::commands::ParticipantPresentationMetadata;
use super::commands::ThreadInterAgentMessageDeliveryState;
use super::commands::ThreadInterAgentMessageParams;
use super::commands::ThreadInterAgentMessageStatusParams;
use super::commands::ThreadInterAgentMessageStatusQuery;
use super::commands::ThreadInterAgentMessageStatusResponse;
use super::participants::{ParticipantMetadataResolver, RegistryParticipantMetadataResolver};

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);
const DEFAULT_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_REGISTRATION_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const TASK_SERVICE_CONTROL_TYPE: &str = "cutex.task_service.assignment.v2";
const TASK_SERVICE_COMPLETION_CONTROL_TYPE: &str = "cutex.task_service.completion.v1";
const TASK_SERVICE_WORKER_FOLLOWUP_CONTROL_TYPE: &str = "cutex.task_service.worker_followup.v1";
const TASK_SERVICE_WATCHDOG_CONTROL_TYPE: &str = "cutex.task_service.watchdog.v1";
const TASK_SERVICE_SYSTEM_SENDER: &str = "cutex-task-service";
const MODEL_VISIBLE_MESSAGE_ID_PREFIX: &str = "amsg_";
const MODEL_VISIBLE_MESSAGE_ID_MAX_CHARS: usize = 64;
const MODEL_VISIBLE_MESSAGE_ID_HASH_DOMAIN: &str = "cutex:model-visible-inter-agent-message:v1\0";
const INTER_AGENT_SEMANTIC_HASH_DOMAIN: &[u8] = b"cutex:inter-agent-message-semantic:v1\0";
const INTER_AGENT_STATUS_SCHEMA: &str = "cutex/inter-agent-delivery-status/v1";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct DeliverySweepOutcome {
    had_messages: bool,
    retained_pending: bool,
    made_progress: bool,
}

#[derive(Debug, Default)]
struct PendingPollBackoff {
    next_delay: Option<Duration>,
}

impl PendingPollBackoff {
    fn wait_after(
        &mut self,
        outcome: DeliverySweepOutcome,
        initial_delay: Duration,
        maximum_delay: Duration,
    ) -> Option<Duration> {
        if outcome.made_progress {
            self.next_delay = None;
        }
        if !outcome.retained_pending {
            self.next_delay = None;
            return None;
        }
        let cap = maximum_delay.max(initial_delay);
        let delay = self.next_delay.unwrap_or(initial_delay).min(cap);
        self.next_delay = Some(delay.saturating_mul(2).min(cap));
        Some(delay)
    }
}

pub trait RuntimeAgentBus: Send + Sync + 'static {
    fn register(&self, request: &AgentBusRegisterRequest) -> anyhow::Result<()>;
    fn unregister(&self, agent_id: &str) -> anyhow::Result<bool>;
    fn poll(&self, agent_id: &str) -> anyhow::Result<Vec<AgentBusMessage>>;
    fn ack(&self, agent_id: &str, message_ids: &[String]) -> anyhow::Result<usize>;
}

impl RuntimeAgentBus for AgentBusHttpClient {
    fn register(&self, request: &AgentBusRegisterRequest) -> anyhow::Result<()> {
        AgentBusHttpClient::register(self, request)
    }

    fn poll(&self, agent_id: &str) -> anyhow::Result<Vec<AgentBusMessage>> {
        AgentBusHttpClient::poll(self, agent_id)
    }

    fn unregister(&self, agent_id: &str) -> anyhow::Result<bool> {
        AgentBusHttpClient::unregister(self, agent_id)
    }

    fn ack(&self, agent_id: &str, message_ids: &[String]) -> anyhow::Result<usize> {
        AgentBusHttpClient::ack(self, agent_id, message_ids)
    }
}

pub trait InterAgentMessageSubmitter: Send + Sync + 'static {
    fn submit_inter_agent_message(
        &self,
        params: &ThreadInterAgentMessageParams,
    ) -> anyhow::Result<String>;

    fn inter_agent_message_status(
        &self,
        params: &ThreadInterAgentMessageStatusParams,
    ) -> anyhow::Result<ThreadInterAgentMessageStatusResponse>;
}

trait TaskServiceContextRecorder {
    fn validate_assignment(&self, _metadata: &TaskServiceAssignmentMetadata) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_context_inserted(
        &self,
        metadata: &TaskServiceAssignmentMetadata,
        agent_bus_message_id: &str,
        native_submission_id: &str,
    ) -> anyhow::Result<Option<ProviderReceipt>>;

    /// Projects optional repository/UI state after the authoritative context
    /// insertion receipt is durable. Failure here must never turn a successful
    /// native submission into an unacknowledged Agent Bus record.
    fn postprocess_context_inserted(
        &self,
        _metadata: &TaskServiceAssignmentMetadata,
        _receipt: Option<&ProviderReceipt>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_completion_context_inserted(
        &self,
        _metadata: &TaskServiceCompletionMetadata,
        _agent_bus_message_id: &str,
        _native_submission_id: &str,
    ) -> anyhow::Result<Option<ProviderReceipt>> {
        Ok(None)
    }

    fn validate_worker_followup(
        &self,
        _metadata: &TaskServiceWorkerFollowupMetadata,
        _recipient_cutex_session: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_worker_followup_context_inserted(
        &self,
        _metadata: &TaskServiceWorkerFollowupMetadata,
        _recipient_cutex_session: &str,
        _agent_bus_message_id: &str,
        _native_submission_id: &str,
    ) -> anyhow::Result<Option<ProviderReceipt>> {
        Ok(None)
    }

    fn record_watchdog_context_inserted(
        &self,
        _metadata: &crate::task_service::TaskWatchdogMessageMetadata,
        _agent_bus_message_id: &str,
        _native_submission_id: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_watchdog_turn_binding(
        &self,
        _metadata: &crate::task_service::TaskWatchdogMessageMetadata,
        _cutex_session_id: &str,
        _thread_id: &str,
        _native_submission_id: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

struct DurableTaskServiceContextRecorder;

impl TaskServiceContextRecorder for DurableTaskServiceContextRecorder {
    fn validate_assignment(&self, metadata: &TaskServiceAssignmentMetadata) -> anyhow::Result<()> {
        crate::task_delivery::provider_adapter::validate_assignment_metadata(metadata)
            .map_err(Into::into)
    }

    fn record_context_inserted(
        &self,
        metadata: &TaskServiceAssignmentMetadata,
        agent_bus_message_id: &str,
        native_submission_id: &str,
    ) -> anyhow::Result<Option<ProviderReceipt>> {
        crate::task_delivery::provider_adapter::record_context_inserted(
            metadata,
            agent_bus_message_id,
            native_submission_id,
        )
        .map(Some)
        .map_err(Into::into)
    }

    fn postprocess_context_inserted(
        &self,
        metadata: &TaskServiceAssignmentMetadata,
        receipt: Option<&ProviderReceipt>,
    ) -> anyhow::Result<()> {
        let (Some(coordinator), Some(receipt)) =
            (metadata.coordinator_cutex_session.as_ref(), receipt)
        else {
            return Ok(());
        };
        let provider = crate::task_service::TaskServiceProvider::open(
            crate::task_delivery::provider_adapter::default_task_service_provider_root()?,
        )?;
        let snapshot = provider.query()?;
        crate::management::v2::integration_events::append_task_service_transition(
            coordinator,
            crate::management::v2::integration_events::TaskAssignmentTransitionKind::AttemptAcknowledged,
            receipt,
            &snapshot,
        )?;
        Ok(())
    }

    fn record_completion_context_inserted(
        &self,
        metadata: &TaskServiceCompletionMetadata,
        agent_bus_message_id: &str,
        native_submission_id: &str,
    ) -> anyhow::Result<Option<ProviderReceipt>> {
        crate::task_delivery::provider_adapter::record_completion_context_inserted(
            metadata,
            agent_bus_message_id,
            native_submission_id,
        )
        .map(Some)
        .map_err(Into::into)
    }

    fn validate_worker_followup(
        &self,
        metadata: &TaskServiceWorkerFollowupMetadata,
        recipient_cutex_session: &str,
    ) -> anyhow::Result<()> {
        crate::task_delivery::provider_adapter::validate_worker_followup_metadata(
            metadata,
            recipient_cutex_session,
        )
        .map_err(Into::into)
    }

    fn record_worker_followup_context_inserted(
        &self,
        metadata: &TaskServiceWorkerFollowupMetadata,
        recipient_cutex_session: &str,
        agent_bus_message_id: &str,
        native_submission_id: &str,
    ) -> anyhow::Result<Option<ProviderReceipt>> {
        crate::task_delivery::provider_adapter::record_worker_followup_context_inserted(
            metadata,
            recipient_cutex_session,
            agent_bus_message_id,
            native_submission_id,
        )
        .map(Some)
        .map_err(Into::into)
    }

    fn record_watchdog_context_inserted(
        &self,
        metadata: &crate::task_service::TaskWatchdogMessageMetadata,
        agent_bus_message_id: &str,
        native_submission_id: &str,
    ) -> anyhow::Result<()> {
        let watchdog = crate::task_service::TaskStaleWatchdog::open(
            crate::task_service::default_task_watchdog_root()?,
            crate::task_service::TaskWatchdogConfig::from_env()?,
        )?;
        watchdog.record_delivery_fact(
            &metadata.notification_id,
            crate::task_service::TaskWatchdogDeliveryFactKind::Delivered,
            Some(format!("{agent_bus_message_id}:{native_submission_id}")),
        )
    }

    fn record_watchdog_turn_binding(
        &self,
        metadata: &crate::task_service::TaskWatchdogMessageMetadata,
        cutex_session_id: &str,
        thread_id: &str,
        native_submission_id: &str,
    ) -> anyhow::Result<()> {
        if metadata.stage != crate::task_service::TaskWatchdogStage::FirstStale {
            return Ok(());
        }
        crate::management::v2::activity::record_task_watchdog_turn_binding(
            cutex_session_id,
            thread_id,
            native_submission_id,
            &metadata.assignment_id,
            &Utc::now().to_rfc3339(),
        )
    }
}

impl InterAgentMessageSubmitter for AppServerCommands {
    fn submit_inter_agent_message(
        &self,
        params: &ThreadInterAgentMessageParams,
    ) -> anyhow::Result<String> {
        let response = self.thread_inter_agent_message(params)?;
        let submission_id = response.submission_id.trim().to_string();
        if submission_id.is_empty() {
            anyhow::bail!("thread/inter_agent_message returned an empty submissionId");
        }
        Ok(submission_id)
    }

    fn inter_agent_message_status(
        &self,
        params: &ThreadInterAgentMessageStatusParams,
    ) -> anyhow::Result<ThreadInterAgentMessageStatusResponse> {
        self.thread_inter_agent_message_status(params)
            .map_err(Into::into)
    }
}

#[derive(Debug, Clone)]
pub struct AppServerAgentBusBridgeOptions {
    pub registration: AgentBusRegisterRequest,
    pub cutex_session_id: String,
    pub thread_id: String,
    pub poll_interval: Duration,
    pub retry_interval: Duration,
    pub registration_refresh_interval: Duration,
}

impl AppServerAgentBusBridgeOptions {
    pub fn new(registration: AgentBusRegisterRequest, thread_id: impl Into<String>) -> Self {
        let thread_id = thread_id.into();
        Self {
            registration,
            cutex_session_id: default_cutex_session_id_for_codex_session(&thread_id),
            thread_id,
            poll_interval: DEFAULT_POLL_INTERVAL,
            retry_interval: DEFAULT_RETRY_INTERVAL,
            registration_refresh_interval: DEFAULT_REGISTRATION_REFRESH_INTERVAL,
        }
    }

    pub fn with_cutex_session_id(mut self, cutex_session_id: impl Into<String>) -> Self {
        self.cutex_session_id = cutex_session_id.into();
        self
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.registration.id.trim().is_empty() {
            anyhow::bail!("agent-bus bridge registration id must not be empty");
        }
        if self.thread_id.trim().is_empty() {
            anyhow::bail!("agent-bus bridge threadId must not be empty");
        }
        if self.cutex_session_id.trim().is_empty() {
            anyhow::bail!("agent-bus bridge cutexSessionId must not be empty");
        }
        if self.registration.session_id.as_deref() != Some(self.thread_id.as_str()) {
            anyhow::bail!("agent-bus registration session_id must match the app-server threadId");
        }
        if self.registration.pid == 0 {
            anyhow::bail!("agent-bus registration pid must be the app-server process id");
        }
        if self.poll_interval.is_zero()
            || self.retry_interval.is_zero()
            || self.registration_refresh_interval.is_zero()
        {
            anyhow::bail!("agent-bus bridge intervals must be positive");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppServerAgentBusBridgeStatus {
    pub runtime_agent_id: String,
    pub thread_id: String,
    pub running: bool,
    pub registered: bool,
    pub pending_ack_count: usize,
    pub submitted_count: u64,
    pub acknowledged_count: u64,
    pub last_poll_at: Option<String>,
    pub last_message_id: Option<String>,
    pub last_submission_id: Option<String>,
    pub last_error: Option<String>,
}

pub struct AppServerAgentBusBridge {
    stop_tx: mpsc::SyncSender<()>,
    worker: Option<JoinHandle<()>>,
    status: Arc<Mutex<AppServerAgentBusBridgeStatus>>,
    shutdown_bus: Arc<dyn RuntimeAgentBus>,
    registration: AgentBusRegisterRequest,
    runtime_agent_id: String,
}

impl AppServerAgentBusBridge {
    pub fn spawn(
        bus: Arc<dyn RuntimeAgentBus>,
        submitter: Arc<dyn InterAgentMessageSubmitter>,
        options: AppServerAgentBusBridgeOptions,
    ) -> anyhow::Result<Self> {
        Self::spawn_with_liveness(bus, submitter, options, Arc::new(AtomicBool::new(true)))
    }

    pub fn spawn_with_liveness(
        bus: Arc<dyn RuntimeAgentBus>,
        submitter: Arc<dyn InterAgentMessageSubmitter>,
        options: AppServerAgentBusBridgeOptions,
        runtime_alive: Arc<AtomicBool>,
    ) -> anyhow::Result<Self> {
        options.validate()?;
        if !runtime_alive.load(Ordering::Acquire) {
            anyhow::bail!("app-server runtime disconnected before agent-bus bridge startup");
        }
        bus.register(&options.registration)
            .context("failed to register app-server runtime on the agent bus")?;
        let runtime_agent_id = options.registration.id.clone();
        let registration = options.registration.clone();
        let status = Arc::new(Mutex::new(AppServerAgentBusBridgeStatus {
            runtime_agent_id: runtime_agent_id.clone(),
            thread_id: options.thread_id.clone(),
            running: true,
            registered: true,
            pending_ack_count: 0,
            submitted_count: 0,
            acknowledged_count: 0,
            last_poll_at: None,
            last_message_id: None,
            last_submission_id: None,
            last_error: None,
        }));
        let (stop_tx, stop_rx) = mpsc::sync_channel(1);
        let worker_status = status.clone();
        let shutdown_bus = bus.clone();
        let worker = thread::spawn(move || {
            run_bridge_worker(
                bus,
                submitter,
                options,
                stop_rx,
                worker_status,
                runtime_alive,
            )
        });
        Ok(Self {
            stop_tx,
            worker: Some(worker),
            status,
            shutdown_bus,
            registration,
            runtime_agent_id,
        })
    }

    pub fn status(&self) -> anyhow::Result<AppServerAgentBusBridgeStatus> {
        self.status
            .lock()
            .map(|status| status.clone())
            .map_err(|_| anyhow::anyhow!("agent-bus bridge status lock was poisoned"))
    }

    /// Synchronously republishes the bridge's frozen identity after Agent Bus
    /// recovery. Registration is idempotent for the same runtime endpoint and
    /// does not create a child or alter the native thread/generation.
    pub fn refresh_registration(&self) -> anyhow::Result<AppServerAgentBusBridgeStatus> {
        match self.shutdown_bus.register(&self.registration) {
            Ok(()) => {
                mark_registration(&self.status, true);
                self.status()
            }
            Err(error) => {
                mark_registration(&self.status, false);
                mark_error(
                    &self.status,
                    format!("agent-bus registration refresh failed: {error:#}"),
                );
                Err(error)
            }
        }
    }

    pub fn shutdown(mut self) -> anyhow::Result<()> {
        self.stop_and_join()
    }

    fn stop_and_join(&mut self) -> anyhow::Result<()> {
        let _ = self.stop_tx.try_send(());
        if self.worker.is_some() {
            // Removing the volatile registration wakes an in-flight long poll,
            // allowing the worker to observe the stop signal immediately.
            let _ = self.shutdown_bus.unregister(&self.runtime_agent_id);
        }
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| anyhow::anyhow!("agent-bus bridge worker panicked"))?;
        }
        Ok(())
    }
}

impl Drop for AppServerAgentBusBridge {
    fn drop(&mut self) {
        let _ = self.stop_and_join();
    }
}

fn run_bridge_worker(
    bus: Arc<dyn RuntimeAgentBus>,
    submitter: Arc<dyn InterAgentMessageSubmitter>,
    options: AppServerAgentBusBridgeOptions,
    stop_rx: mpsc::Receiver<()>,
    status: Arc<Mutex<AppServerAgentBusBridgeStatus>>,
    runtime_alive: Arc<AtomicBool>,
) {
    let mut pending_acks = BTreeSet::new();
    let recipient_label = canonical_recipient_label(
        options.registration.base_name.as_deref(),
        &options.registration.name,
        &options.registration.id,
    )
    .to_string();
    let mut next_registration = Instant::now() + options.registration_refresh_interval;
    let mut pending_poll_backoff = PendingPollBackoff::default();
    loop {
        if !runtime_alive.load(Ordering::Acquire) {
            mark_error(&status, "app-server runtime disconnected".to_string());
            break;
        }
        if stop_requested(&stop_rx, Duration::ZERO) {
            break;
        }
        if !pending_acks.is_empty() {
            if let Err(error) = retry_pending_acks(
                bus.as_ref(),
                &options.registration.id,
                &mut pending_acks,
                &status,
            ) {
                mark_error(&status, error.to_string());
                if stop_requested(&stop_rx, options.retry_interval) {
                    break;
                }
                continue;
            }
        }
        if Instant::now() >= next_registration {
            if let Err(error) = bus.register(&options.registration) {
                mark_registration(&status, false);
                mark_error(
                    &status,
                    format!("agent-bus registration refresh failed: {error:#}"),
                );
                if stop_requested(&stop_rx, options.retry_interval) {
                    break;
                }
                continue;
            }
            mark_registration(&status, true);
            next_registration = Instant::now() + options.registration_refresh_interval;
        }

        let poll_started = Instant::now();
        match bus.poll(&options.registration.id) {
            Ok(messages) => {
                mark_poll(&status);
                let outcome = match deliver_polled_messages(
                    bus.as_ref(),
                    submitter.as_ref(),
                    &DurableTaskServiceContextRecorder,
                    &options.registration.id,
                    &recipient_label,
                    &options.cutex_session_id,
                    &options.thread_id,
                    messages,
                    &mut pending_acks,
                    &status,
                ) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        mark_error(&status, error.to_string());
                        if stop_requested(&stop_rx, options.retry_interval) {
                            break;
                        }
                        continue;
                    }
                };
                let idle_wait = if let Some(delay) = pending_poll_backoff.wait_after(
                    outcome,
                    options.poll_interval,
                    options.retry_interval,
                ) {
                    delay
                } else if outcome.had_messages {
                    Duration::ZERO
                } else {
                    options.poll_interval.saturating_sub(poll_started.elapsed())
                };
                if stop_requested(&stop_rx, idle_wait) {
                    break;
                }
            }
            Err(error) => {
                mark_registration(&status, false);
                mark_error(&status, format!("agent-bus poll failed: {error:#}"));
                next_registration = Instant::now();
                if stop_requested(&stop_rx, options.retry_interval) {
                    break;
                }
            }
        }
    }
    if let Ok(mut status) = status.lock() {
        status.running = false;
        status.registered = false;
        status.pending_ack_count = pending_acks.len();
    }
    if let Err(error) = bus.unregister(&options.registration.id) {
        mark_error(&status, format!("agent-bus unregister failed: {error:#}"));
    }
}

fn deliver_polled_messages(
    bus: &dyn RuntimeAgentBus,
    submitter: &dyn InterAgentMessageSubmitter,
    task_service_context_recorder: &dyn TaskServiceContextRecorder,
    runtime_agent_id: &str,
    recipient_label: &str,
    cutex_session_id: &str,
    thread_id: &str,
    messages: Vec<AgentBusMessage>,
    pending_acks: &mut BTreeSet<String>,
    status: &Arc<Mutex<AppServerAgentBusBridgeStatus>>,
) -> anyhow::Result<DeliverySweepOutcome> {
    let mut outcome = DeliverySweepOutcome {
        had_messages: !messages.is_empty(),
        ..DeliverySweepOutcome::default()
    };
    let mut prepared = Vec::new();
    for message in messages {
        if pending_acks.contains(&message.id) {
            continue;
        }
        let validation = (|| {
            if let Some(metadata) = task_service_metadata(&message)? {
                task_service_context_recorder.validate_assignment(&metadata)?;
            }
            if let Some(metadata) = task_service_worker_followup_metadata(&message)? {
                task_service_context_recorder
                    .validate_worker_followup(&metadata, cutex_session_id)?;
            }
            inter_agent_params(thread_id, recipient_label, cutex_session_id, &message)
        })();
        match validation {
            Ok(params) => {
                let computed = inter_agent_semantic_sha256(&params);
                let digest = agent_bus_message_repository()?
                    .semantic_sha256(&message.id)?
                    .unwrap_or_else(|| computed.clone());
                if digest != computed {
                    let quarantined = agent_bus_message_repository()?.record_quarantined(
                        cutex_session_id,
                        &message.id,
                        serde_json::json!({"code":"canonical_semantic_digest_mismatch","message":"durable envelope no longer matches its committed digest"}),
                        Utc::now(),
                    )?;
                    if quarantined {
                        let ids = [message.id.clone()];
                        let acked = bus.ack(runtime_agent_id, &ids)?;
                        mark_acknowledged(status, acked, pending_acks.len());
                        outcome.made_progress = true;
                    }
                    continue;
                }
                prepared.push((message, params, digest));
            }
            Err(error) => {
                let quarantined = agent_bus_message_repository()?.record_quarantined(
                    cutex_session_id,
                    &message.id,
                    serde_json::json!({"code":"canonical_envelope_invalid","message":error.to_string()}),
                    Utc::now(),
                )?;
                if quarantined {
                    let ids = [message.id.clone()];
                    let acked = bus.ack(runtime_agent_id, &ids)?;
                    mark_acknowledged(status, acked, pending_acks.len());
                    outcome.made_progress = true;
                }
                eprintln!(
                    "warning: quarantined malformed agent-bus message {}: {error:#}",
                    message.id
                );
            }
        }
    }
    if prepared.is_empty() {
        return Ok(outcome);
    }

    for batch in prepared.chunks(256) {
        let initial = query_delivery_statuses_isolated(submitter, thread_id, batch)?;
        let mut malformed = BTreeSet::new();
        for (message, params, _digest) in batch {
            match &initial[params.message_id.as_str()] {
                Ok(status) if status.state == ThreadInterAgentMessageDeliveryState::Unknown => {}
                Ok(_) => continue,
                Err(error) => {
                    quarantine_status_error(
                        bus,
                        runtime_agent_id,
                        cutex_session_id,
                        message,
                        error,
                        pending_acks,
                        status,
                    )?;
                    outcome.made_progress = true;
                    malformed.insert(message.id.clone());
                    continue;
                }
            }
            match submitter.submit_inter_agent_message(params) {
                Ok(submission_id) => {
                    let _ = agent_bus_message_repository()?.record_a2_submission(
                        cutex_session_id,
                        &message.id,
                        &submission_id,
                        Utc::now(),
                    )?;
                    mark_submitted(status, &message.id, &submission_id, pending_acks.len());
                    outcome.made_progress = true;
                }
                Err(error) => {
                    mark_error(
                        status,
                        format!("A2 submission failed for {}: {error:#}", message.id),
                    );
                }
            }
        }

        let current = query_delivery_statuses_isolated(submitter, thread_id, batch)?;
        for (message, params, _digest) in batch {
            if malformed.contains(&message.id) {
                continue;
            }
            let native = match &current[params.message_id.as_str()] {
                Ok(status) => status,
                Err(error) => {
                    quarantine_status_error(
                        bus,
                        runtime_agent_id,
                        cutex_session_id,
                        message,
                        error,
                        pending_acks,
                        status,
                    )?;
                    outcome.made_progress = true;
                    continue;
                }
            };
            match native.state {
                ThreadInterAgentMessageDeliveryState::ContextPersisted => {
                    let receipt = native
                        .receipt
                        .as_ref()
                        .context("context_persisted status omitted its typed A4 receipt")?;
                    commit_a4_facts(
                        task_service_context_recorder,
                        message,
                        cutex_session_id,
                        thread_id,
                        receipt,
                    )?;
                    let _ = agent_bus_message_repository()?.record_delivered(
                        cutex_session_id,
                        &message.id,
                        receipt,
                        Utc::now(),
                    )?;
                    pending_acks.insert(message.id.clone());
                    let ids = [message.id.clone()];
                    let acked = bus.ack(runtime_agent_id, &ids).with_context(|| {
                        format!(
                            "failed to acknowledge A4-proven agent-bus message {}",
                            message.id
                        )
                    })?;
                    pending_acks.remove(&message.id);
                    mark_acknowledged(status, acked, pending_acks.len());
                    outcome.made_progress = true;
                }
                ThreadInterAgentMessageDeliveryState::Conflict => {
                    let quarantined = agent_bus_message_repository()?.record_quarantined(
                    cutex_session_id,
                    &message.id,
                    serde_json::json!({"code":"native_semantic_conflict","message":"receiver reported the message ID with different semantics"}),
                    Utc::now(),
                )?;
                    if quarantined {
                        let ids = [message.id.clone()];
                        let acked = bus.ack(runtime_agent_id, &ids)?;
                        mark_acknowledged(status, acked, pending_acks.len());
                        outcome.made_progress = true;
                    }
                }
                ThreadInterAgentMessageDeliveryState::Unknown
                | ThreadInterAgentMessageDeliveryState::Pending
                | ThreadInterAgentMessageDeliveryState::RetryableError => {
                    outcome.retained_pending = true;
                }
            }
        }
    }
    Ok(outcome)
}

fn query_delivery_statuses_isolated<'a>(
    submitter: &dyn InterAgentMessageSubmitter,
    thread_id: &str,
    prepared: &'a [(AgentBusMessage, ThreadInterAgentMessageParams, String)],
) -> anyhow::Result<BTreeMap<&'a str, Result<super::commands::ThreadInterAgentMessageStatus, String>>>
{
    match query_delivery_statuses(submitter, thread_id, prepared) {
        Ok(statuses) => Ok(statuses
            .into_iter()
            .map(|(id, status)| (id, Ok(status)))
            .collect()),
        Err(error) if prepared.len() > 1 && is_malformed_status_error(&error) => {
            let mut isolated = BTreeMap::new();
            for item in prepared {
                match query_delivery_statuses(submitter, thread_id, std::slice::from_ref(item)) {
                    Ok(mut statuses) => {
                        let id = item.1.message_id.as_str();
                        isolated.insert(
                            id,
                            Ok(statuses
                                .remove(id)
                                .expect("single status response contains its requested id")),
                        );
                    }
                    Err(error) if is_malformed_status_error(&error) => {
                        isolated.insert(item.1.message_id.as_str(), Err(error.to_string()));
                    }
                    Err(error) => return Err(error),
                }
            }
            Ok(isolated)
        }
        Err(error) if is_malformed_status_error(&error) => Ok(BTreeMap::from([(
            prepared[0].1.message_id.as_str(),
            Err(error.to_string()),
        )])),
        Err(error) => Err(error),
    }
}

fn is_malformed_status_error(error: &anyhow::Error) -> bool {
    error.to_string().starts_with("malformed A4 status:")
}

fn quarantine_status_error(
    bus: &dyn RuntimeAgentBus,
    runtime_agent_id: &str,
    cutex_session_id: &str,
    message: &AgentBusMessage,
    error: &str,
    pending_acks: &BTreeSet<String>,
    status: &Arc<Mutex<AppServerAgentBusBridgeStatus>>,
) -> anyhow::Result<()> {
    let quarantined = agent_bus_message_repository()?.record_quarantined(
        cutex_session_id,
        &message.id,
        serde_json::json!({"code":"native_status_malformed","message":error}),
        Utc::now(),
    )?;
    if quarantined {
        let ids = [message.id.clone()];
        let acked = bus.ack(runtime_agent_id, &ids)?;
        mark_acknowledged(status, acked, pending_acks.len());
    }
    mark_error(
        status,
        format!("malformed A4 status for {}: {error}", message.id),
    );
    Ok(())
}

fn query_delivery_statuses<'a>(
    submitter: &dyn InterAgentMessageSubmitter,
    thread_id: &str,
    prepared: &'a [(AgentBusMessage, ThreadInterAgentMessageParams, String)],
) -> anyhow::Result<BTreeMap<&'a str, super::commands::ThreadInterAgentMessageStatus>> {
    let params = ThreadInterAgentMessageStatusParams {
        thread_id: thread_id.to_string(),
        messages: prepared
            .iter()
            .map(|(_, params, digest)| ThreadInterAgentMessageStatusQuery {
                message_id: params.message_id.clone(),
                semantic_sha256: digest.clone(),
            })
            .collect(),
    };
    let response = submitter.inter_agent_message_status(&params)?;
    if response.schema != INTER_AGENT_STATUS_SCHEMA
        || response.thread_id != thread_id
        || response.statuses.len() != prepared.len()
    {
        anyhow::bail!("malformed A4 status: response batch identity is invalid");
    }
    let mut result = BTreeMap::new();
    for ((_, requested, digest), status) in prepared.iter().zip(response.statuses) {
        if status.message_id != requested.message_id || status.semantic_sha256 != *digest {
            anyhow::bail!("malformed A4 status: response changed request identity or order");
        }
        if status.state == ThreadInterAgentMessageDeliveryState::ContextPersisted {
            let receipt = status
                .receipt
                .as_ref()
                .context("malformed A4 status: context_persisted omitted receipt")?;
            if receipt.receipt_id.trim().is_empty()
                || receipt.response_item_id != requested.message_id
                || receipt.turn_id.trim().is_empty()
                || receipt
                    .thread_id
                    .as_deref()
                    .is_some_and(|value| value != thread_id)
                || receipt
                    .message_id
                    .as_deref()
                    .is_some_and(|value| value != requested.message_id)
                || receipt
                    .semantic_sha256
                    .as_deref()
                    .is_some_and(|value| value != digest)
            {
                anyhow::bail!("malformed A4 status: context_persisted receipt is invalid");
            }
        } else if status.receipt.is_some() {
            anyhow::bail!("malformed A4 status: non-A4 state included a receipt");
        }
        result.insert(requested.message_id.as_str(), status);
    }
    Ok(result)
}

fn commit_a4_facts(
    recorder: &dyn TaskServiceContextRecorder,
    message: &AgentBusMessage,
    cutex_session_id: &str,
    thread_id: &str,
    a4: &InterAgentContextPersistedReceipt,
) -> anyhow::Result<()> {
    let assignment = task_service_metadata(message)?;
    let completion = task_service_completion_metadata(message)?;
    let followup = task_service_worker_followup_metadata(message)?;
    let watchdog = task_service_watchdog_metadata(message)?;
    if let Some(metadata) = assignment.as_ref() {
        let receipt = recorder.record_context_inserted(metadata, &message.id, &a4.receipt_id)?;
        if let Err(error) = recorder.postprocess_context_inserted(metadata, receipt.as_ref()) {
            eprintln!("warning: failed to project Task Service A4 context insertion: {error:#}");
        }
        if let (Some(receipt), Some(coordinator)) =
            (receipt, metadata.coordinator_cutex_session.as_ref())
        {
            if let Err(error) =
                crate::management::v2::integration_events::append_task_service_communication(
                    coordinator,
                    &receipt,
                    Some(&message.id),
                )
            {
                eprintln!("warning: failed to project Task Service A4 communication: {error:#}");
            }
        }
    }
    if let Some(metadata) = completion.as_ref() {
        recorder.record_completion_context_inserted(metadata, &message.id, &a4.receipt_id)?;
    }
    if let Some(metadata) = followup.as_ref() {
        recorder.record_worker_followup_context_inserted(
            metadata,
            cutex_session_id,
            &message.id,
            &a4.receipt_id,
        )?;
    }
    if let Some(metadata) = watchdog.as_ref() {
        recorder.record_watchdog_context_inserted(metadata, &message.id, &a4.receipt_id)?;
        if let Err(error) = recorder.record_watchdog_turn_binding(
            metadata,
            cutex_session_id,
            thread_id,
            &a4.receipt_id,
        ) {
            eprintln!("warning: failed to bind Task Service watchdog A4 receipt: {error:#}");
        }
    }
    Ok(())
}

fn task_service_metadata(
    message: &AgentBusMessage,
) -> anyhow::Result<Option<TaskServiceAssignmentMetadata>> {
    if !message.sender_kind.is_task_service_system()
        || message.control_type.as_deref() != Some(TASK_SERVICE_CONTROL_TYPE)
    {
        return Ok(None);
    }
    let payload = message.control_payload.clone().with_context(|| {
        format!(
            "Task Service agent-bus message {} is missing control metadata",
            message.id
        )
    })?;
    let metadata: TaskServiceAssignmentMetadata =
        serde_json::from_value(payload).with_context(|| {
            format!(
                "Task Service agent-bus message {} has invalid control metadata",
                message.id
            )
        })?;
    if metadata.schema != TASK_SERVICE_PROVIDER_ACTION_SCHEMA {
        anyhow::bail!(
            "Task Service agent-bus message {} has an inconsistent metadata schema",
            message.id
        );
    }
    metadata.validate_contract_if_present().with_context(|| {
        format!(
            "Task Service agent-bus message {} has an invalid assignment contract",
            message.id
        )
    })?;
    Ok(Some(metadata))
}

fn task_service_completion_metadata(
    message: &AgentBusMessage,
) -> anyhow::Result<Option<TaskServiceCompletionMetadata>> {
    if !message.sender_kind.is_task_service_system()
        || message.control_type.as_deref() != Some(TASK_SERVICE_COMPLETION_CONTROL_TYPE)
    {
        return Ok(None);
    }
    let payload = message.control_payload.clone().with_context(|| {
        format!(
            "Task Service completion message {} is missing control metadata",
            message.id
        )
    })?;
    let metadata: TaskServiceCompletionMetadata =
        serde_json::from_value(payload).with_context(|| {
            format!(
                "Task Service completion message {} has invalid control metadata",
                message.id
            )
        })?;
    if metadata.schema != TASK_SERVICE_PROVIDER_ACTION_SCHEMA {
        anyhow::bail!(
            "Task Service completion message {} has an inconsistent metadata schema",
            message.id
        );
    }
    Ok(Some(metadata))
}

fn task_service_worker_followup_metadata(
    message: &AgentBusMessage,
) -> anyhow::Result<Option<TaskServiceWorkerFollowupMetadata>> {
    if !message.sender_kind.is_task_service_system()
        || message.control_type.as_deref() != Some(TASK_SERVICE_WORKER_FOLLOWUP_CONTROL_TYPE)
    {
        return Ok(None);
    }
    let payload = message.control_payload.clone().with_context(|| {
        format!(
            "Task Service Worker follow-up message {} is missing control metadata",
            message.id
        )
    })?;
    let metadata: TaskServiceWorkerFollowupMetadata = serde_json::from_value(payload)
        .with_context(|| {
            format!(
                "Task Service Worker follow-up message {} has invalid control metadata",
                message.id
            )
        })?;
    if metadata.schema != TASK_SERVICE_PROVIDER_ACTION_SCHEMA {
        anyhow::bail!(
            "Task Service Worker follow-up message {} has an inconsistent metadata schema",
            message.id
        );
    }
    Ok(Some(metadata))
}

fn task_service_watchdog_metadata(
    message: &AgentBusMessage,
) -> anyhow::Result<Option<crate::task_service::TaskWatchdogMessageMetadata>> {
    if !message.sender_kind.is_task_service_system()
        || message.control_type.as_deref() != Some(TASK_SERVICE_WATCHDOG_CONTROL_TYPE)
    {
        return Ok(None);
    }
    let payload = message.control_payload.clone().with_context(|| {
        format!(
            "Task Service watchdog message {} is missing control metadata",
            message.id
        )
    })?;
    let metadata: crate::task_service::TaskWatchdogMessageMetadata =
        serde_json::from_value(payload).with_context(|| {
            format!(
                "Task Service watchdog message {} has invalid control metadata",
                message.id
            )
        })?;
    if metadata.schema != crate::task_service::TASK_WATCHDOG_MESSAGE_SCHEMA {
        anyhow::bail!(
            "Task Service watchdog message {} has an inconsistent metadata schema",
            message.id
        );
    }
    Ok(Some(metadata))
}

fn retry_pending_acks(
    bus: &dyn RuntimeAgentBus,
    runtime_agent_id: &str,
    pending_acks: &mut BTreeSet<String>,
    status: &Arc<Mutex<AppServerAgentBusBridgeStatus>>,
) -> anyhow::Result<()> {
    let message_ids = pending_acks.iter().cloned().collect::<Vec<_>>();
    let acked = bus.ack(runtime_agent_id, &message_ids)?;
    pending_acks.clear();
    mark_acknowledged(status, acked, 0);
    Ok(())
}

pub fn inter_agent_params(
    thread_id: &str,
    recipient_label: &str,
    recipient_cutex_session_id: &str,
    message: &AgentBusMessage,
) -> anyhow::Result<ThreadInterAgentMessageParams> {
    let recipient_metadata =
        Some(RegistryParticipantMetadataResolver.resolve(recipient_cutex_session_id));
    if message.kind == AgentBusEnvelopeKind::Message
        && (message.from == AGENT_MANAGEMENT_SYSTEM_SENDER
            || message.control_type.as_deref() == Some(AGENT_MANAGEMENT_START_CONTROL_TYPE))
    {
        return agent_management_inter_agent_params(
            thread_id,
            recipient_label,
            recipient_metadata,
            message,
        );
    }
    if message.kind == AgentBusEnvelopeKind::Message && message.sender_kind.is_agent() {
        if message.id.trim().is_empty() {
            anyhow::bail!("agent-bus message id must not be empty");
        }
        if message.content.trim().is_empty() {
            anyhow::bail!("agent-bus message {} has empty content", message.id);
        }
        return Ok(ThreadInterAgentMessageParams {
            thread_id: thread_id.to_string(),
            message_id: model_visible_message_id(&message.id),
            author: agent_path_for_bus_label(&message.from),
            author_metadata: message
                .from_cutex_session_id
                .as_deref()
                .map(|session_id| RegistryParticipantMetadataResolver.resolve(session_id)),
            recipient: "/root".to_string(),
            recipient_metadata,
            other_recipients: Vec::new(),
            content: format!(
                "Message Type: MESSAGE\nTask name: {recipient_label}\nSender: {}\nPayload:\n{}",
                message.from, message.content
            ),
            delivery_mode: message.delivery_mode.clone(),
        });
    }
    if message.kind == AgentBusEnvelopeKind::Message && message.sender_kind.is_task_service_system()
    {
        return task_service_inter_agent_params(
            thread_id,
            recipient_label,
            recipient_metadata,
            message,
        );
    }
    anyhow::bail!(
        "agent-bus message {} is not an inter-agent message",
        message.id
    )
}

fn agent_management_inter_agent_params(
    thread_id: &str,
    recipient_label: &str,
    recipient_metadata: Option<ParticipantPresentationMetadata>,
    message: &AgentBusMessage,
) -> anyhow::Result<ThreadInterAgentMessageParams> {
    if message.from != AGENT_MANAGEMENT_SYSTEM_SENDER {
        anyhow::bail!(
            "Agent Management agent-bus message {} has a noncanonical sender",
            message.id,
        );
    }
    // Agent Management retains the existing Agent Bus wire kind; the opaque
    // in-process principal and reserved control record select this projection.
    if !message.sender_kind.is_agent() {
        anyhow::bail!(
            "Agent Management agent-bus message {} has an invalid sender kind",
            message.id,
        );
    }
    if message.id.trim().is_empty() {
        anyhow::bail!("agent-bus message id must not be empty");
    }
    if message.content.trim().is_empty() {
        anyhow::bail!("agent-bus message {} has empty content", message.id);
    }
    if message.control_type.as_deref() != Some(AGENT_MANAGEMENT_START_CONTROL_TYPE) {
        anyhow::bail!(
            "Agent Management agent-bus message {} has an invalid control type",
            message.id
        );
    }
    let control_payload = message.control_payload.clone().with_context(|| {
        format!(
            "Agent Management agent-bus message {} is missing control metadata",
            message.id
        )
    })?;
    let metadata: AgentManagementMessageMetadata = serde_json::from_value(control_payload)
        .with_context(|| {
            format!(
                "Agent Management agent-bus message {} has invalid control metadata",
                message.id
            )
        })?;
    if metadata.schema != AgentManagementSchema::V1 {
        anyhow::bail!(
            "Agent Management agent-bus message {} has an inconsistent metadata schema",
            message.id
        );
    }
    message
        .external_message_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .with_context(|| {
            format!(
                "Agent Management agent-bus message {} is missing its external message ID",
                message.id
            )
        })?;
    Ok(ThreadInterAgentMessageParams {
        thread_id: thread_id.to_string(),
        message_id: model_visible_message_id(&message.id),
        author: agent_path_for_bus_label(AGENT_MANAGEMENT_SYSTEM_SENDER),
        author_metadata: Some(system_participant("Cutex Agent Management")),
        recipient: "/root".to_string(),
        recipient_metadata,
        other_recipients: Vec::new(),
        content: format!(
            "Message Type: MESSAGE\nTask name: {recipient_label}\nSender: {AGENT_MANAGEMENT_SYSTEM_SENDER}\nRequested By Director: {}\nPayload:\n{}",
            metadata.requested_by_director.as_str(),
            message.content,
        ),
        delivery_mode: message.delivery_mode.clone(),
    })
}

fn task_service_inter_agent_params(
    thread_id: &str,
    recipient_label: &str,
    recipient_metadata: Option<ParticipantPresentationMetadata>,
    message: &AgentBusMessage,
) -> anyhow::Result<ThreadInterAgentMessageParams> {
    if message.from != TASK_SERVICE_SYSTEM_SENDER {
        anyhow::bail!(
            "Task Service agent-bus message {} has a noncanonical sender",
            message.id,
        );
    }
    if message.id.trim().is_empty() {
        anyhow::bail!("agent-bus message id must not be empty");
    }
    if message.content.trim().is_empty() {
        anyhow::bail!("agent-bus message {} has empty content", message.id);
    }
    if message.control_type.as_deref() == Some(TASK_SERVICE_WORKER_FOLLOWUP_CONTROL_TYPE) {
        return task_service_worker_followup_inter_agent_params(
            thread_id,
            recipient_label,
            recipient_metadata,
            message,
        );
    }
    if message.control_type.as_deref() == Some(TASK_SERVICE_COMPLETION_CONTROL_TYPE) {
        return task_service_completion_inter_agent_params(
            thread_id,
            recipient_label,
            recipient_metadata,
            message,
        );
    }
    if message.control_type.as_deref() == Some(TASK_SERVICE_WATCHDOG_CONTROL_TYPE) {
        return task_service_watchdog_inter_agent_params(
            thread_id,
            recipient_label,
            recipient_metadata,
            message,
        );
    }
    if message.control_type.as_deref() != Some(TASK_SERVICE_CONTROL_TYPE) {
        anyhow::bail!(
            "Task Service agent-bus message {} has an invalid control type",
            message.id
        );
    }
    let control_payload = message.control_payload.clone().with_context(|| {
        format!(
            "Task Service agent-bus message {} is missing control metadata",
            message.id
        )
    })?;
    let metadata: TaskServiceAssignmentMetadata = serde_json::from_value(control_payload)
        .with_context(|| {
            format!(
                "Task Service agent-bus message {} has invalid control metadata",
                message.id
            )
        })?;
    if metadata.schema != TASK_SERVICE_PROVIDER_ACTION_SCHEMA {
        anyhow::bail!(
            "Task Service agent-bus message {} has an inconsistent metadata schema",
            message.id
        );
    }
    let opaque_contract = metadata.validate_contract_if_present().with_context(|| {
        format!(
            "Task Service agent-bus message {} has an invalid assignment contract",
            message.id
        )
    })?;
    if let Some(contract) = opaque_contract {
        crate::agent_bus::model::validate_task_service_assignment_summary(
            &message.content,
            contract,
        )
        .with_context(|| {
            format!(
                "Task Service agent-bus message {} duplicates its assignment contract in the summary",
                message.id
            )
        })?;
    }
    let external_action_id =
        required_external_id(message.external_action_id.as_deref(), "action", &message.id)?;
    let external_message_id = required_external_id(
        message.external_message_id.as_deref(),
        "message",
        &message.id,
    )?;
    let coordinator = metadata
        .coordinator_cutex_session
        .as_ref()
        .map(|session| format!("Coordinator: {}\n", session.as_str()))
        .unwrap_or_default();
    let content = if let Some(contract) = opaque_contract {
        format!(
            "Message Type: TASK_SERVICE_ASSIGNMENT\nTask name: {recipient_label}\nSender: {TASK_SERVICE_SYSTEM_SENDER}\n{coordinator}Assignment ID: {}\nTask ID: {}\nTask Revision: {}\nContract SHA-256: {}\nSend Attempt ID: {}\nExternal Action ID: {external_action_id}\nExternal Message ID: {external_message_id}\nSummary:\n{}\nOpaque Contract:\n{contract}",
            metadata.assignment_id.as_str(),
            metadata.task_id.as_str(),
            metadata.task_revision.get(),
            metadata.contract_sha256.as_str(),
            metadata.send_attempt_id.as_str(),
            message.content,
        )
    } else {
        format!(
            "Message Type: TASK_SERVICE_ASSIGNMENT\nTask name: {recipient_label}\nSender: {TASK_SERVICE_SYSTEM_SENDER}\n{coordinator}Assignment ID: {}\nTask ID: {}\nTask Revision: {}\nContract SHA-256: {}\nSend Attempt ID: {}\nExternal Action ID: {external_action_id}\nExternal Message ID: {external_message_id}\nPayload:\n{}",
            metadata.assignment_id.as_str(),
            metadata.task_id.as_str(),
            metadata.task_revision.get(),
            metadata.contract_sha256.as_str(),
            metadata.send_attempt_id.as_str(),
            message.content,
        )
    };
    Ok(ThreadInterAgentMessageParams {
        thread_id: thread_id.to_string(),
        message_id: model_visible_message_id(&message.id),
        author: agent_path_for_bus_label(TASK_SERVICE_SYSTEM_SENDER),
        author_metadata: Some(system_participant("Cutex Task Service")),
        recipient: "/root".to_string(),
        recipient_metadata,
        other_recipients: Vec::new(),
        content,
        delivery_mode: message.delivery_mode.clone(),
    })
}

fn task_service_worker_followup_inter_agent_params(
    thread_id: &str,
    recipient_label: &str,
    recipient_metadata: Option<ParticipantPresentationMetadata>,
    message: &AgentBusMessage,
) -> anyhow::Result<ThreadInterAgentMessageParams> {
    let payload = message.control_payload.clone().with_context(|| {
        format!(
            "Task Service Worker follow-up message {} is missing control metadata",
            message.id
        )
    })?;
    let metadata: TaskServiceWorkerFollowupMetadata = serde_json::from_value(payload)
        .with_context(|| {
            format!(
                "Task Service Worker follow-up message {} has invalid control metadata",
                message.id
            )
        })?;
    if metadata.schema != TASK_SERVICE_PROVIDER_ACTION_SCHEMA
        || message.content != metadata.decision_reference
    {
        anyhow::bail!(
            "Task Service Worker follow-up message {} has inconsistent protected content",
            message.id
        );
    }
    Ok(ThreadInterAgentMessageParams {
        thread_id: thread_id.to_string(),
        message_id: model_visible_message_id(&message.id),
        author: agent_path_for_bus_label(TASK_SERVICE_SYSTEM_SENDER),
        author_metadata: Some(system_participant("Cutex Task Service")),
        recipient: "/root".to_string(),
        recipient_metadata,
        other_recipients: Vec::new(),
        content: format!(
            "Message Type: TASK_SERVICE_REQUEST_CHANGES\nTask name: {recipient_label}\nSender: {TASK_SERVICE_SYSTEM_SENDER}\nAssignment ID: {}\nTask ID: {}\nTask Revision: {}\nAttempt Number: {}\nDecision Reference:\n{}",
            metadata.assignment_id.as_str(),
            metadata.task_id.as_str(),
            metadata.task_revision.get(),
            metadata.attempt_number.get(),
            metadata.decision_reference,
        ),
        delivery_mode: message.delivery_mode.clone(),
    })
}

fn task_service_completion_inter_agent_params(
    thread_id: &str,
    recipient_label: &str,
    recipient_metadata: Option<ParticipantPresentationMetadata>,
    message: &AgentBusMessage,
) -> anyhow::Result<ThreadInterAgentMessageParams> {
    let payload = message.control_payload.clone().with_context(|| {
        format!(
            "Task Service completion message {} is missing control metadata",
            message.id
        )
    })?;
    let metadata: TaskServiceCompletionMetadata =
        serde_json::from_value(payload).with_context(|| {
            format!(
                "Task Service completion message {} has invalid control metadata",
                message.id
            )
        })?;
    if metadata.schema != TASK_SERVICE_PROVIDER_ACTION_SCHEMA {
        anyhow::bail!(
            "Task Service completion message {} has an inconsistent metadata schema",
            message.id
        );
    }
    let external_action_id =
        required_external_id(message.external_action_id.as_deref(), "action", &message.id)?;
    let external_message_id = required_external_id(
        message.external_message_id.as_deref(),
        "message",
        &message.id,
    )?;
    Ok(ThreadInterAgentMessageParams {
        thread_id: thread_id.to_string(),
        message_id: model_visible_message_id(&message.id),
        author: agent_path_for_bus_label(TASK_SERVICE_SYSTEM_SENDER),
        author_metadata: Some(system_participant("Cutex Task Service")),
        recipient: "/root".to_string(),
        recipient_metadata,
        other_recipients: Vec::new(),
        content: format!(
            "Message Type: TASK_SERVICE_COMPLETION\nTask name: {recipient_label}\nSender: {TASK_SERVICE_SYSTEM_SENDER}\nNotification ID: {}\nAssignment ID: {}\nTask ID: {}\nTask Revision: {}\nAttempt Number: {}\nTransition: {:?}\nTarget Seat: {}\nExternal Action ID: {external_action_id}\nExternal Message ID: {external_message_id}\nPayload:\n{}",
            metadata.notification_id.as_str(),
            metadata.assignment_id.as_str(),
            metadata.task_id.as_str(),
            metadata.task_revision.get(),
            metadata
                .attempt_number
                .map(|number| number.get().to_string())
                .unwrap_or_else(|| "none".to_string()),
            metadata.kind,
            metadata.target_seat_id.as_str(),
            message.content,
        ),
        delivery_mode: message.delivery_mode.clone(),
    })
}

fn task_service_watchdog_inter_agent_params(
    thread_id: &str,
    recipient_label: &str,
    recipient_metadata: Option<ParticipantPresentationMetadata>,
    message: &AgentBusMessage,
) -> anyhow::Result<ThreadInterAgentMessageParams> {
    let payload = message.control_payload.clone().with_context(|| {
        format!(
            "Task Service watchdog message {} is missing control metadata",
            message.id
        )
    })?;
    let metadata: crate::task_service::TaskWatchdogMessageMetadata =
        serde_json::from_value(payload).with_context(|| {
            format!(
                "Task Service watchdog message {} has invalid control metadata",
                message.id
            )
        })?;
    if metadata.schema != crate::task_service::TASK_WATCHDOG_MESSAGE_SCHEMA
        || message.content.trim().is_empty()
        || message.external_message_id.as_deref() != Some(metadata.notification_id.as_str())
    {
        anyhow::bail!(
            "Task Service watchdog message {} has inconsistent metadata",
            message.id
        );
    }
    let project = metadata
        .project_id
        .as_ref()
        .map(|id| format!("Project ID: {}\n", id.as_str()))
        .unwrap_or_default();
    Ok(ThreadInterAgentMessageParams {
        thread_id: thread_id.to_string(),
        message_id: model_visible_message_id(&message.id),
        author: agent_path_for_bus_label(TASK_SERVICE_SYSTEM_SENDER),
        author_metadata: Some(system_participant("Cutex Task Service")),
        recipient: "/root".to_string(),
        recipient_metadata,
        other_recipients: Vec::new(),
        content: format!(
            "Message Type: TASK_SERVICE_WATCHDOG\nTask name: {recipient_label}\nSender: {TASK_SERVICE_SYSTEM_SENDER}\n{project}Assignment ID: {}\nAttempt Number: {}\nStage: {}\nPayload:\n{}",
            metadata.assignment_id,
            metadata.attempt_number,
            metadata.stage.event_key(),
            message.content,
        ),
        delivery_mode: message.delivery_mode.clone(),
    })
}

fn system_participant(display_name: &str) -> ParticipantPresentationMetadata {
    ParticipantPresentationMetadata {
        display_name: Some(display_name.to_string()),
        ..Default::default()
    }
}

/// Projects an Agent Bus record ID into the bounded app-server item-ID namespace.
/// Existing bridge IDs remain byte-for-byte stable when they already fit. Only
/// oversized IDs use an `amsg_`-prefixed, domain-separated SHA-256 projection
/// with 236 bits of collision strength so cute-codex retains the native item ID.
pub fn model_visible_message_id(agent_bus_message_id: &str) -> String {
    let current = format!("{MODEL_VISIBLE_MESSAGE_ID_PREFIX}{agent_bus_message_id}");
    if current.chars().count() <= MODEL_VISIBLE_MESSAGE_ID_MAX_CHARS {
        return current;
    }

    let digest_input = format!("{MODEL_VISIBLE_MESSAGE_ID_HASH_DOMAIN}{current}");
    let digest = crate::task_service::sha256_bytes(digest_input.as_bytes());
    let digest_chars = MODEL_VISIBLE_MESSAGE_ID_MAX_CHARS - MODEL_VISIBLE_MESSAGE_ID_PREFIX.len();
    format!(
        "{MODEL_VISIBLE_MESSAGE_ID_PREFIX}{}",
        &digest.as_str()[..digest_chars]
    )
}

pub fn inter_agent_semantic_sha256(params: &ThreadInterAgentMessageParams) -> String {
    fn field(hasher: &mut Sha256, bytes: &[u8]) {
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    }
    let mut hasher = Sha256::new();
    hasher.update(INTER_AGENT_SEMANTIC_HASH_DOMAIN);
    field(&mut hasher, params.message_id.as_bytes());
    field(&mut hasher, params.author.as_bytes());
    field(&mut hasher, params.recipient.as_bytes());
    hasher.update((params.other_recipients.len() as u64).to_be_bytes());
    for recipient in &params.other_recipients {
        field(&mut hasher, recipient.as_bytes());
    }
    field(&mut hasher, params.delivery_mode.event_label().as_bytes());
    field(&mut hasher, params.content.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn required_external_id<'a>(
    value: Option<&'a str>,
    label: &str,
    bus_message_id: &str,
) -> anyhow::Result<&'a str> {
    value
        .filter(|value| !value.trim().is_empty())
        .with_context(|| {
            format!(
                "Task Service agent-bus message {bus_message_id} is missing its external {label} ID"
            )
        })
}

fn agent_path_for_bus_label(label: &str) -> String {
    let mut segment = String::new();
    let mut previous_underscore = false;
    for character in label.chars() {
        let character = if character.is_ascii_alphanumeric() {
            character.to_ascii_lowercase()
        } else {
            '_'
        };
        if character == '_' && previous_underscore {
            continue;
        }
        segment.push(character);
        previous_underscore = character == '_';
        if segment.len() >= 48 {
            break;
        }
    }
    let segment = segment.trim_matches('_');
    let segment = if segment.is_empty() || segment == "root" {
        "external_agent"
    } else {
        segment
    };
    format!("/root/{segment}")
}

fn stop_requested(stop_rx: &mpsc::Receiver<()>, timeout: Duration) -> bool {
    match stop_rx.recv_timeout(timeout) {
        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => true,
        Err(mpsc::RecvTimeoutError::Timeout) => false,
    }
}

fn mark_registration(status: &Arc<Mutex<AppServerAgentBusBridgeStatus>>, registered: bool) {
    if let Ok(mut status) = status.lock() {
        status.registered = registered;
    }
}

fn mark_poll(status: &Arc<Mutex<AppServerAgentBusBridgeStatus>>) {
    if let Ok(mut status) = status.lock() {
        status.last_poll_at = Some(Utc::now().to_rfc3339());
    }
}

fn mark_submitted(
    status: &Arc<Mutex<AppServerAgentBusBridgeStatus>>,
    message_id: &str,
    submission_id: &str,
    pending_ack_count: usize,
) {
    if let Ok(mut status) = status.lock() {
        status.submitted_count = status.submitted_count.saturating_add(1);
        status.last_message_id = Some(message_id.to_string());
        status.last_submission_id = Some(submission_id.to_string());
        status.pending_ack_count = pending_ack_count;
    }
}

fn mark_acknowledged(
    status: &Arc<Mutex<AppServerAgentBusBridgeStatus>>,
    acknowledged: usize,
    pending_ack_count: usize,
) {
    if let Ok(mut status) = status.lock() {
        status.acknowledged_count = status
            .acknowledged_count
            .saturating_add(acknowledged as u64);
        status.pending_ack_count = pending_ack_count;
    }
}

fn mark_error(status: &Arc<Mutex<AppServerAgentBusBridgeStatus>>, error: String) {
    if let Ok(mut status) = status.lock() {
        status.last_error = Some(error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_bus::delivery::AgentDeliveryMode;
    use crate::agent_bus::model::AgentMessageKind;
    use crate::agent_bus::model::AgentRegistrationClass;
    use std::collections::VecDeque;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Condvar;

    #[derive(Default)]
    struct FakeBus {
        events: Mutex<Vec<String>>,
        ack_results: Mutex<VecDeque<anyhow::Result<usize>>>,
    }

    impl RuntimeAgentBus for FakeBus {
        fn register(&self, _request: &AgentBusRegisterRequest) -> anyhow::Result<()> {
            self.events
                .lock()
                .expect("events lock")
                .push("register".to_string());
            Ok(())
        }

        fn unregister(&self, _agent_id: &str) -> anyhow::Result<bool> {
            Ok(true)
        }

        fn poll(&self, _agent_id: &str) -> anyhow::Result<Vec<AgentBusMessage>> {
            Ok(Vec::new())
        }

        fn ack(&self, _agent_id: &str, message_ids: &[String]) -> anyhow::Result<usize> {
            self.events
                .lock()
                .expect("events lock")
                .push(format!("ack:{}", message_ids.join(",")));
            self.ack_results
                .lock()
                .expect("ack results lock")
                .pop_front()
                .unwrap_or_else(|| Ok(message_ids.len()))
        }
    }

    struct FakeSubmitter {
        events: Arc<Mutex<Vec<String>>>,
        result: Mutex<Option<anyhow::Result<String>>>,
    }

    struct SelectiveStatusSubmitter {
        events: Arc<Mutex<Vec<String>>>,
        unavailable: bool,
    }

    struct MalformedFirstStatusSubmitter;

    struct ConflictFirstStatusSubmitter;

    impl InterAgentMessageSubmitter for ConflictFirstStatusSubmitter {
        fn submit_inter_agent_message(
            &self,
            params: &ThreadInterAgentMessageParams,
        ) -> anyhow::Result<String> {
            anyhow::bail!("unexpected submit for {}", params.message_id)
        }

        fn inter_agent_message_status(
            &self,
            params: &ThreadInterAgentMessageStatusParams,
        ) -> anyhow::Result<ThreadInterAgentMessageStatusResponse> {
            Ok(ThreadInterAgentMessageStatusResponse {
                schema: INTER_AGENT_STATUS_SCHEMA.to_string(),
                thread_id: params.thread_id.clone(),
                statuses: params
                    .messages
                    .iter()
                    .map(|query| {
                        let conflict = query.message_id.ends_with("message-1");
                        let state = if conflict {
                            ThreadInterAgentMessageDeliveryState::Conflict
                        } else {
                            ThreadInterAgentMessageDeliveryState::ContextPersisted
                        };
                        super::super::commands::ThreadInterAgentMessageStatus {
                            message_id: query.message_id.clone(),
                            state,
                            semantic_sha256: query.semantic_sha256.clone(),
                            receipt: (!conflict).then(|| InterAgentContextPersistedReceipt {
                                schema: None,
                                receipt_id: format!("a4r_{}", query.message_id),
                                thread_id: None,
                                message_id: None,
                                semantic_sha256: None,
                                response_item_id: query.message_id.clone(),
                                turn_id: "turn-1".to_string(),
                                rollout_ordinal: 1,
                            }),
                        }
                    })
                    .collect(),
            })
        }
    }

    impl InterAgentMessageSubmitter for MalformedFirstStatusSubmitter {
        fn submit_inter_agent_message(
            &self,
            params: &ThreadInterAgentMessageParams,
        ) -> anyhow::Result<String> {
            anyhow::bail!("unexpected submit for {}", params.message_id)
        }

        fn inter_agent_message_status(
            &self,
            params: &ThreadInterAgentMessageStatusParams,
        ) -> anyhow::Result<ThreadInterAgentMessageStatusResponse> {
            Ok(ThreadInterAgentMessageStatusResponse {
                schema: INTER_AGENT_STATUS_SCHEMA.to_string(),
                thread_id: params.thread_id.clone(),
                statuses: params
                    .messages
                    .iter()
                    .map(|query| {
                        let malformed = query.message_id.ends_with("message-1");
                        super::super::commands::ThreadInterAgentMessageStatus {
                            message_id: query.message_id.clone(),
                            state: ThreadInterAgentMessageDeliveryState::ContextPersisted,
                            semantic_sha256: query.semantic_sha256.clone(),
                            receipt: Some(InterAgentContextPersistedReceipt {
                                schema: None,
                                receipt_id: format!("a4r_{}", query.message_id),
                                thread_id: None,
                                message_id: None,
                                semantic_sha256: None,
                                response_item_id: if malformed {
                                    "wrong-response-item".to_string()
                                } else {
                                    query.message_id.clone()
                                },
                                turn_id: "turn-1".to_string(),
                                rollout_ordinal: 1,
                            }),
                        }
                    })
                    .collect(),
            })
        }
    }

    impl InterAgentMessageSubmitter for SelectiveStatusSubmitter {
        fn submit_inter_agent_message(
            &self,
            params: &ThreadInterAgentMessageParams,
        ) -> anyhow::Result<String> {
            self.events
                .lock()
                .unwrap()
                .push(format!("submit:{}", params.message_id));
            Ok(format!("a2_{}", params.message_id))
        }

        fn inter_agent_message_status(
            &self,
            params: &ThreadInterAgentMessageStatusParams,
        ) -> anyhow::Result<ThreadInterAgentMessageStatusResponse> {
            if self.unavailable {
                anyhow::bail!("method not found: thread/inter_agent_message/status");
            }
            let submitted = self.events.lock().unwrap().clone();
            Ok(ThreadInterAgentMessageStatusResponse {
                schema: INTER_AGENT_STATUS_SCHEMA.to_string(),
                thread_id: params.thread_id.clone(),
                statuses: params
                    .messages
                    .iter()
                    .map(|query| {
                        let is_later = query.message_id.ends_with("message-2");
                        let was_submitted = submitted
                            .iter()
                            .any(|event| event == &format!("submit:{}", query.message_id));
                        let state = if is_later && was_submitted {
                            ThreadInterAgentMessageDeliveryState::ContextPersisted
                        } else if is_later {
                            ThreadInterAgentMessageDeliveryState::Unknown
                        } else {
                            ThreadInterAgentMessageDeliveryState::Pending
                        };
                        super::super::commands::ThreadInterAgentMessageStatus {
                            message_id: query.message_id.clone(),
                            state,
                            semantic_sha256: query.semantic_sha256.clone(),
                            receipt: (state
                                == ThreadInterAgentMessageDeliveryState::ContextPersisted)
                                .then(|| InterAgentContextPersistedReceipt {
                                    schema: Some(INTER_AGENT_STATUS_SCHEMA.to_string()),
                                    receipt_id: format!("a4r_{}", query.message_id),
                                    thread_id: Some(params.thread_id.clone()),
                                    message_id: Some(query.message_id.clone()),
                                    semantic_sha256: Some(query.semantic_sha256.clone()),
                                    response_item_id: query.message_id.clone(),
                                    turn_id: "turn-later".to_string(),
                                    rollout_ordinal: 9,
                                }),
                        }
                    })
                    .collect(),
            })
        }
    }

    struct NoopTaskServiceContextRecorder;

    impl TaskServiceContextRecorder for NoopTaskServiceContextRecorder {
        fn record_context_inserted(
            &self,
            _metadata: &TaskServiceAssignmentMetadata,
            _agent_bus_message_id: &str,
            _native_submission_id: &str,
        ) -> anyhow::Result<Option<ProviderReceipt>> {
            Ok(None)
        }
    }

    struct FailingTaskServiceContextRecorder;

    impl TaskServiceContextRecorder for FailingTaskServiceContextRecorder {
        fn record_context_inserted(
            &self,
            _metadata: &TaskServiceAssignmentMetadata,
            _agent_bus_message_id: &str,
            _native_submission_id: &str,
        ) -> anyhow::Result<Option<ProviderReceipt>> {
            anyhow::bail!("context receipt persistence failed")
        }
    }

    struct RecordingWorkerFollowupContextRecorder {
        events: Arc<Mutex<Vec<String>>>,
        fail_record: bool,
    }

    impl TaskServiceContextRecorder for RecordingWorkerFollowupContextRecorder {
        fn record_context_inserted(
            &self,
            _metadata: &TaskServiceAssignmentMetadata,
            _agent_bus_message_id: &str,
            _native_submission_id: &str,
        ) -> anyhow::Result<Option<ProviderReceipt>> {
            Ok(None)
        }

        fn validate_worker_followup(
            &self,
            _metadata: &TaskServiceWorkerFollowupMetadata,
            recipient_cutex_session: &str,
        ) -> anyhow::Result<()> {
            self.events
                .lock()
                .expect("events lock")
                .push(format!("validate:{recipient_cutex_session}"));
            Ok(())
        }

        fn record_worker_followup_context_inserted(
            &self,
            _metadata: &TaskServiceWorkerFollowupMetadata,
            _recipient_cutex_session: &str,
            agent_bus_message_id: &str,
            _native_submission_id: &str,
        ) -> anyhow::Result<Option<ProviderReceipt>> {
            self.events
                .lock()
                .expect("events lock")
                .push(format!("context:{agent_bus_message_id}"));
            if self.fail_record {
                anyhow::bail!("Worker follow-up context receipt persistence failed");
            }
            Ok(None)
        }
    }

    struct ReplayedContextWithFailingProjectionRecorder {
        events: Arc<Mutex<Vec<String>>>,
    }

    impl TaskServiceContextRecorder for ReplayedContextWithFailingProjectionRecorder {
        fn record_context_inserted(
            &self,
            _metadata: &TaskServiceAssignmentMetadata,
            agent_bus_message_id: &str,
            _native_submission_id: &str,
        ) -> anyhow::Result<Option<ProviderReceipt>> {
            self.events
                .lock()
                .expect("events lock")
                .push(format!("context-current-state:{agent_bus_message_id}"));
            Ok(Some(replayed_context_receipt(agent_bus_message_id)))
        }

        fn postprocess_context_inserted(
            &self,
            _metadata: &TaskServiceAssignmentMetadata,
            receipt: Option<&ProviderReceipt>,
        ) -> anyhow::Result<()> {
            assert!(
                receipt.is_some(),
                "current-state replay retains its receipt"
            );
            self.events
                .lock()
                .expect("events lock")
                .push("transition-projection-retry".to_string());
            anyhow::bail!(
                "cutex event does not match management v2: Additional properties are not allowed ('project_id' was unexpected)"
            )
        }

        fn record_watchdog_context_inserted(
            &self,
            _metadata: &crate::task_service::TaskWatchdogMessageMetadata,
            agent_bus_message_id: &str,
            _native_submission_id: &str,
        ) -> anyhow::Result<()> {
            self.events
                .lock()
                .expect("events lock")
                .push(format!("watchdog-delivered:{agent_bus_message_id}"));
            Ok(())
        }

        fn record_watchdog_turn_binding(
            &self,
            _metadata: &crate::task_service::TaskWatchdogMessageMetadata,
            _cutex_session_id: &str,
            _thread_id: &str,
            native_submission_id: &str,
        ) -> anyhow::Result<()> {
            self.events
                .lock()
                .expect("events lock")
                .push(format!("watchdog-binding-retry:{native_submission_id}"));
            anyhow::bail!("activity binding store temporarily unavailable")
        }
    }

    fn replayed_context_receipt(agent_bus_message_id: &str) -> ProviderReceipt {
        ProviderReceipt {
            schema: crate::task_service::ProviderReceiptSchema::V3,
            action_id: crate::task_service::ActionId::new(format!(
                "context-inserted:send-1:{agent_bus_message_id}"
            ))
            .unwrap(),
            request_sha256: crate::role_revision::Sha256::new("1".repeat(64)).unwrap(),
            attempt_binding: None,
            committed_at: crate::role_revision::Rfc3339::new("2026-08-28T23:20:10Z".to_string())
                .unwrap(),
            journal_sequence: 641,
            result: crate::task_service::ProviderResult::SendAttempt(
                crate::task_service::SendAttempt {
                    project_id: None,
                    send_attempt_id: crate::task_service::SendAttemptId::new("send-1").unwrap(),
                    assignment_id: crate::task_service::AssignmentId::new("assignment-1").unwrap(),
                    retry_ordinal: 1,
                    external_message_id: "external-message-1".to_string(),
                    local_revision: 3,
                    events: Vec::new(),
                },
            ),
        }
    }

    impl InterAgentMessageSubmitter for FakeSubmitter {
        fn submit_inter_agent_message(
            &self,
            params: &ThreadInterAgentMessageParams,
        ) -> anyhow::Result<String> {
            self.events
                .lock()
                .expect("events lock")
                .push(format!("submit:{}", params.message_id));
            let mut configured = self.result.lock().expect("result lock");
            let result = configured
                .take()
                .unwrap_or_else(|| Ok("submission-1".to_string()));
            if result.is_ok() {
                *configured = Some(Ok("__a4_persisted__".to_string()));
            }
            result
        }

        fn inter_agent_message_status(
            &self,
            params: &ThreadInterAgentMessageStatusParams,
        ) -> anyhow::Result<ThreadInterAgentMessageStatusResponse> {
            let submitted = self
                .result
                .lock()
                .expect("result lock")
                .as_ref()
                .is_some_and(|result| {
                    result
                        .as_ref()
                        .is_ok_and(|value| value == "__a4_persisted__")
                });
            let state = if submitted {
                ThreadInterAgentMessageDeliveryState::ContextPersisted
            } else {
                ThreadInterAgentMessageDeliveryState::Unknown
            };
            Ok(ThreadInterAgentMessageStatusResponse {
                schema: INTER_AGENT_STATUS_SCHEMA.to_string(),
                thread_id: params.thread_id.clone(),
                statuses: params
                    .messages
                    .iter()
                    .map(
                        |query| super::super::commands::ThreadInterAgentMessageStatus {
                            message_id: query.message_id.clone(),
                            state,
                            semantic_sha256: query.semantic_sha256.clone(),
                            receipt: (state
                                == ThreadInterAgentMessageDeliveryState::ContextPersisted)
                                .then(|| InterAgentContextPersistedReceipt {
                                    schema: None,
                                    receipt_id: format!("a4r_{}", query.message_id),
                                    thread_id: None,
                                    message_id: None,
                                    semantic_sha256: None,
                                    response_item_id: query.message_id.clone(),
                                    turn_id: "turn-1".to_string(),
                                    rollout_ordinal: 1,
                                }),
                        },
                    )
                    .collect(),
            })
        }
    }

    #[test]
    fn inter_agent_params_wrap_payload_once_and_preserve_native_paths() {
        let mut message = test_message(AgentDeliveryMode::Interrupt);
        message.id = "11111111-2222-3333-4444-555555555555".to_string();
        message.content = "Message Type: NEW_TASK\nline two\nμ-tail\n".to_string();
        let params = inter_agent_params("thread-1", "receiver-agent", "cutex.receiver", &message)
            .expect("map message");

        assert_eq!(params.thread_id, "thread-1");
        assert_eq!(
            params.message_id,
            "amsg_11111111-2222-3333-4444-555555555555"
        );
        assert_eq!(params.author, "/root/sender_agent");
        assert_eq!(params.recipient, "/root");
        assert!(params.other_recipients.is_empty());
        assert_eq!(
            params.content,
            "Message Type: MESSAGE\nTask name: receiver-agent\nSender: Sender Agent\nPayload:\nMessage Type: NEW_TASK\nline two\nμ-tail\n"
        );
        assert_eq!(params.delivery_mode, AgentDeliveryMode::Interrupt);
    }

    #[test]
    fn semantic_digest_matches_frozen_cross_runtime_golden_vector() {
        let params = ThreadInterAgentMessageParams {
            thread_id: "presentation-only-thread".to_string(),
            message_id: "mail_a4_restart_1".to_string(),
            author: "/root/sender".to_string(),
            author_metadata: Some(ParticipantPresentationMetadata {
                display_name: Some("Sender presentation".to_string()),
                ..Default::default()
            }),
            recipient: "/root/recipient".to_string(),
            recipient_metadata: Some(ParticipantPresentationMetadata {
                display_name: Some("Recipient presentation".to_string()),
                ..Default::default()
            }),
            other_recipients: vec!["/root/observer".to_string()],
            content: "hello from sender".to_string(),
            delivery_mode: AgentDeliveryMode::Passive,
        };

        assert_eq!(
            inter_agent_semantic_sha256(&params),
            "8b677efc23d0fe2c4b11d23d4fa0c2af7049d9afa37aa5de2b214b9d6c83bdef"
        );
    }

    #[test]
    fn model_visible_message_id_preserves_valid_ids_and_projects_long_ids_stably() {
        let uuid = "11111111-2222-3333-4444-555555555555";
        assert_eq!(
            model_visible_message_id(uuid),
            "amsg_11111111-2222-3333-4444-555555555555"
        );

        let pathological = format!("tsc_tsn-{}", "a".repeat(64));
        assert_eq!(format!("amsg_{pathological}").len(), 77);
        let projected = model_visible_message_id(&pathological);
        assert_eq!(
            projected.chars().count(),
            MODEL_VISIBLE_MESSAGE_ID_MAX_CHARS
        );
        assert!(projected.starts_with(MODEL_VISIBLE_MESSAGE_ID_PREFIX));
        assert!(projected[MODEL_VISIBLE_MESSAGE_ID_PREFIX.len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase()));
        assert_eq!(projected, model_visible_message_id(&pathological));

        let distinct = format!("tsc_tsn-{}", "b".repeat(64));
        assert_ne!(projected, model_visible_message_id(&distinct));
    }

    #[test]
    fn every_bridge_path_uses_the_same_bounded_message_id_projection() {
        let pathological = format!("tsc_tsn-{}", "c".repeat(64));
        let expected = model_visible_message_id(&pathological);
        let mut messages = vec![
            test_message(AgentDeliveryMode::Soon),
            agent_management_message(),
            task_service_message(),
            task_service_completion_message(),
            task_service_watchdog_message(),
        ];

        for message in &mut messages {
            message.id = pathological.clone();
            let params =
                inter_agent_params("thread-1", "receiver-agent", "cutex.receiver", message)
                    .expect("map long Agent Bus message ID");
            assert_eq!(params.message_id, expected);
            assert!(params.message_id.chars().count() <= MODEL_VISIBLE_MESSAGE_ID_MAX_CHARS);
            assert_eq!(message.id, pathological);
        }
    }

    #[test]
    fn agent_management_start_projects_system_sender_and_typed_director_provenance() {
        let message = agent_management_message();
        let params = inter_agent_params("thread-1", "receiver-agent", "cutex.receiver", &message)
            .expect("map authorized Agent Management start message");

        assert_eq!(params.thread_id, "thread-1");
        assert_eq!(params.message_id, "amsg_agent-management-message-1");
        assert_eq!(params.author, "/root/agentmanagementsystem");
        assert_eq!(params.recipient, "/root");
        assert!(params.other_recipients.is_empty());
        assert_eq!(
            params.content,
            "Message Type: MESSAGE\nTask name: receiver-agent\nSender: AgentManagementSystem\nRequested By Director: cutex.director-r11\nPayload:\ncustom start body"
        );
        assert_eq!(params.delivery_mode, AgentDeliveryMode::AfterTurn);
    }

    #[test]
    fn forged_or_inconsistent_agent_management_envelopes_are_rejected() {
        let valid = agent_management_message();
        let mut cases = Vec::new();

        let mut message = valid.clone();
        message.from = "cutex.director-r11".to_string();
        cases.push(("sender", message));

        let mut message = valid.clone();
        message.control_type = Some("cutex.agent_management.start.v0".to_string());
        cases.push(("control type", message));

        let mut message = valid.clone();
        message.control_payload = None;
        cases.push(("missing metadata", message));

        let mut message = valid.clone();
        message.control_payload.as_mut().unwrap()["extra"] = serde_json::json!(true);
        cases.push(("unknown metadata", message));

        let mut message = valid.clone();
        message.control_payload.as_mut().unwrap()["requested_by_director"] = serde_json::json!("");
        cases.push(("invalid Director provenance", message));

        let mut message = valid.clone();
        message.external_message_id = None;
        cases.push(("external message ID", message));

        let mut message = valid.clone();
        message.sender_kind = AgentMessageKind::Owner;
        cases.push(("sender kind", message));

        let mut message = valid;
        message.kind = AgentBusEnvelopeKind::Control;
        cases.push(("envelope kind", message));

        for (label, message) in cases {
            assert!(
                inter_agent_params("thread-1", "receiver-agent", "cutex.receiver", &message)
                    .is_err(),
                "accepted invalid Agent Management {label}"
            );
        }
    }

    #[test]
    fn task_service_assignment_metadata_is_strictly_validated_and_rendered() {
        let message = task_service_message();
        let params = inter_agent_params("thread-1", "receiver-agent", "cutex.receiver", &message)
            .expect("map authenticated Task Service assignment");

        assert_eq!(params.thread_id, "thread-1");
        assert_eq!(params.message_id, "amsg_task-service-message-1");
        assert_eq!(params.author, "/root/cutex_task_service");
        assert_eq!(params.recipient, "/root");
        assert!(params.other_recipients.is_empty());
        assert_eq!(
            params.content,
            format!(
                "Message Type: TASK_SERVICE_ASSIGNMENT\nTask name: receiver-agent\nSender: cutex-task-service\nCoordinator: cutex.director-r11\nAssignment ID: assignment-1\nTask ID: CUTEX-188\nTask Revision: 1\nContract SHA-256: {}\nSend Attempt ID: send-1\nExternal Action ID: action-1\nExternal Message ID: external-message-1\nSummary:\nassignment summary\nOpaque Contract:\n# Exact Contract\nUnicode: λ",
                crate::task_service::sha256_bytes("# Exact Contract\nUnicode: λ".as_bytes())
                    .as_str()
            )
        );
        assert_eq!(params.content.matches("# Exact Contract").count(), 1);
        assert_eq!(params.content.matches("assignment summary").count(), 1);
        assert_eq!(params.delivery_mode, AgentDeliveryMode::Soon);
    }

    #[test]
    fn task_service_completion_is_typed_model_visible_and_preserves_after_turn() {
        let mut message = task_service_completion_message();
        message.id = format!("tsc_tsn-{}", "d".repeat(64));
        let original_bus_id = message.id.clone();
        let original_external_message_id = message.external_message_id.clone();
        let original_metadata = message.control_payload.clone();
        let original_content = message.content.clone();
        let params = inter_agent_params("thread-1", "director", "cutex.director", &message)
            .expect("map authenticated Task Service completion");
        assert_eq!(
            params.message_id,
            model_visible_message_id(&original_bus_id)
        );
        assert!(params.message_id.chars().count() <= MODEL_VISIBLE_MESSAGE_ID_MAX_CHARS);
        assert_eq!(params.author, "/root/cutex_task_service");
        assert_eq!(params.delivery_mode, AgentDeliveryMode::AfterTurn);
        assert!(params
            .content
            .contains("Message Type: TASK_SERVICE_COMPLETION"));
        assert!(params.content.contains("Notification ID: notification-1"));
        assert!(params.content.contains("External Action ID: submit-1"));
        assert!(params
            .content
            .contains("External Message ID: notification-1"));
        assert!(params.content.contains("Transition: ReviewReady"));
        assert_eq!(message.id, original_bus_id);
        assert_eq!(message.external_message_id, original_external_message_id);
        assert_eq!(message.control_payload, original_metadata);
        assert_eq!(message.content, original_content);

        let mut forged = message;
        forged.from = "ordinary-agent".to_string();
        assert!(inter_agent_params("thread-1", "director", "cutex.director", &forged).is_err());
    }

    #[test]
    fn worker_followup_model_projection_is_compact_typed_and_hides_transport_mechanics() {
        let message = task_service_worker_followup_message();
        let params = inter_agent_params("thread-1", "worker", "cutex.worker", &message)
            .expect("map authenticated Task Service Worker follow-up");
        assert_eq!(params.author, "/root/cutex_task_service");
        assert_eq!(params.delivery_mode, AgentDeliveryMode::Soon);
        assert_eq!(
            params.content,
            "Message Type: TASK_SERVICE_REQUEST_CHANGES\nTask name: worker\nSender: cutex-task-service\nAssignment ID: assignment-1\nTask ID: CUTEX-188\nTask Revision: 3\nAttempt Number: 2\nDecision Reference:\nfix the focused regression"
        );
        for forbidden in [
            "notification-1",
            "request-changes-action",
            "external",
            "target_cutex_session",
            "journal",
            "token",
        ] {
            assert!(!params.content.contains(forbidden), "leaked {forbidden}");
        }

        let mut forged = message.clone();
        forged.from = "ordinary-agent".to_string();
        assert!(inter_agent_params("thread-1", "worker", "cutex.worker", &forged).is_err());
        let mut mismatched = message;
        mismatched.content = "different request".to_string();
        assert!(inter_agent_params("thread-1", "worker", "cutex.worker", &mismatched).is_err());
    }

    #[test]
    fn task_service_watchdog_message_is_canonical_and_hides_transport_mechanics() {
        let message = task_service_watchdog_message();
        let params = inter_agent_params("thread-1", "worker", "cutex.worker", &message)
            .expect("map authenticated Task Service watchdog reminder");
        assert_eq!(params.author, "/root/cutex_task_service");
        assert_eq!(params.delivery_mode, AgentDeliveryMode::Soon);
        assert!(params
            .content
            .contains("Message Type: TASK_SERVICE_WATCHDOG"));
        assert!(params.content.contains("Assignment ID: assignment-1"));
        assert!(params.content.contains("Attempt Number: 1"));
        assert!(params.content.contains("Stage: task_watchdog.first_stale"));
        for forbidden in ["twn_", "external_message", "notification_id", "raw tool"] {
            assert!(!params.content.contains(forbidden));
        }
        assert!(params.message_id.chars().count() <= MODEL_VISIBLE_MESSAGE_ID_MAX_CHARS);

        let mut forged = message;
        forged.external_message_id = Some("different".to_string());
        assert!(inter_agent_params("thread-1", "worker", "cutex.worker", &forged).is_err());
    }

    #[test]
    fn queued_legacy_task_service_assignment_without_coordinator_still_delivers() {
        let mut message = task_service_message();
        message
            .control_payload
            .as_mut()
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("coordinator_cutex_session");
        message
            .control_payload
            .as_mut()
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("opaque_contract");
        let params = inter_agent_params("thread-1", "receiver-agent", "cutex.receiver", &message)
            .expect("legacy Task Service metadata remains readable");
        assert!(!params.content.contains("Coordinator:"));
        assert!(params.content.contains("Assignment ID: assignment-1"));
        assert!(params.content.contains("Payload:\nassignment summary"));
        assert!(!params.content.contains("Opaque Contract:"));
    }

    #[test]
    fn forged_or_inconsistent_task_service_envelopes_are_rejected() {
        let valid = task_service_message();
        let mut cases = Vec::new();

        let mut message = valid.clone();
        message.from = "forged-task-service".to_string();
        cases.push(("sender", message));

        let mut message = valid.clone();
        message.control_type = Some("cutex.task_service.assignment.v1".to_string());
        cases.push(("control type", message));

        let mut message = valid.clone();
        message.control_payload = None;
        cases.push(("missing metadata", message));

        let mut message = valid.clone();
        message.control_payload.as_mut().unwrap()["schema"] =
            serde_json::json!("cutex/task-service-action/v1");
        cases.push(("metadata schema", message));

        let mut message = valid.clone();
        message.control_payload.as_mut().unwrap()["extra"] = serde_json::json!(true);
        cases.push(("unknown metadata", message));

        let mut message = valid.clone();
        message.control_payload.as_mut().unwrap()["task_revision"] = serde_json::json!("1");
        cases.push(("metadata field type", message));

        let mut message = valid.clone();
        message.control_payload.as_mut().unwrap()["coordinator_cutex_session"] =
            serde_json::json!("");
        cases.push(("invalid coordinator identity", message));

        let mut message = valid.clone();
        message.control_payload.as_mut().unwrap()["opaque_contract"] =
            serde_json::json!("tampered contract");
        cases.push(("contract digest mismatch", message));

        let mut message = valid.clone();
        message.content = "summary repeats # Exact Contract\nUnicode: λ verbatim".to_string();
        cases.push(("summary duplicates contract", message));

        let mut message = valid.clone();
        message.external_action_id = Some(" \t".to_string());
        cases.push(("external action ID", message));

        let mut message = valid.clone();
        message.external_message_id = None;
        cases.push(("external message ID", message));

        let mut message = valid.clone();
        message.sender_kind = AgentMessageKind::Owner;
        cases.push(("Owner sender kind", message));

        let mut message = valid.clone();
        message.sender_kind = AgentMessageKind::User;
        cases.push(("User sender kind", message));

        let mut message = valid;
        message.kind = AgentBusEnvelopeKind::Control;
        cases.push(("envelope kind", message));

        for (label, message) in cases {
            assert!(
                inter_agent_params("thread-1", "receiver-agent", "cutex.receiver", &message)
                    .is_err(),
                "accepted invalid Task Service {label}"
            );
        }
    }

    #[test]
    fn ordinary_agent_cannot_enter_task_service_mapping_by_imitating_it() {
        let system = task_service_message();
        let mut ordinary = test_message(AgentDeliveryMode::Soon);
        ordinary.from = TASK_SERVICE_SYSTEM_SENDER.to_string();
        ordinary.content =
            "Message Type: TASK_SERVICE_ASSIGNMENT\nAssignment ID: forged".to_string();
        ordinary.control_type = system.control_type;
        ordinary.control_payload = system.control_payload;
        ordinary.external_action_id = system.external_action_id;
        ordinary.external_message_id = system.external_message_id;

        let params = inter_agent_params("thread-1", "receiver-agent", "cutex.receiver", &ordinary)
            .expect("ordinary Agent mapping remains available");
        assert!(params.content.starts_with("Message Type: MESSAGE\n"));
        assert!(params
            .content
            .ends_with("Message Type: TASK_SERVICE_ASSIGNMENT\nAssignment ID: forged"));
        assert!(!params
            .content
            .starts_with("Message Type: TASK_SERVICE_ASSIGNMENT\n"));
    }

    #[test]
    fn invalid_task_service_envelope_is_rejected_before_submit_and_ack() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let bus = RecordingBus {
            inner: FakeBus::default(),
            events: events.clone(),
        };
        let submitter = FakeSubmitter {
            events: events.clone(),
            result: Mutex::new(Some(Ok("submission-1".to_string()))),
        };
        let status = test_status();
        let mut pending_acks = BTreeSet::new();
        let mut message = task_service_message();
        message.control_payload.as_mut().unwrap()["opaque_contract"] =
            serde_json::json!("tampered before native submission");

        deliver_polled_messages(
            &bus,
            &submitter,
            &NoopTaskServiceContextRecorder,
            "runtime-1",
            "agent",
            "cutex.thread-1",
            "thread-1",
            vec![message],
            &mut pending_acks,
            &status,
        )
        .expect("malformed item is quarantined without blocking the batch");
        assert!(events.lock().expect("events lock").is_empty());
        assert!(pending_acks.is_empty());
        assert_eq!(status.lock().expect("status lock").submitted_count, 0);
    }

    #[test]
    fn every_delivery_mode_keeps_its_mode_and_renders_message() {
        for delivery_mode in [
            AgentDeliveryMode::AfterTurn,
            AgentDeliveryMode::Soon,
            AgentDeliveryMode::Passive,
            AgentDeliveryMode::Interrupt,
        ] {
            let message = test_message(delivery_mode.clone());
            let params =
                inter_agent_params("thread-1", "receiver-agent", "cutex.receiver", &message)
                    .expect("map message");

            assert_eq!(params.delivery_mode, delivery_mode);
            assert_eq!(
                params.content,
                "Message Type: MESSAGE\nTask name: receiver-agent\nSender: Sender Agent\nPayload:\nhello"
            );
        }
    }

    #[test]
    fn canonical_recipient_label_uses_frozen_fallback_order() {
        let mut registration = test_options().registration;
        registration.id = "runtime-id".to_string();
        registration.name = "registration-name".to_string();
        registration.base_name = Some("stable-base-name".to_string());
        let label = |registration: &AgentBusRegisterRequest| {
            canonical_recipient_label(
                registration.base_name.as_deref(),
                &registration.name,
                &registration.id,
            )
            .to_string()
        };
        assert_eq!(label(&registration), "stable-base-name");

        registration.base_name = Some(" \t".to_string());
        assert_eq!(label(&registration), "registration-name");

        registration.base_name = None;
        assert_eq!(label(&registration), "registration-name");

        registration.name = " \n".to_string();
        assert_eq!(label(&registration), "runtime-id");
    }

    #[test]
    fn task_service_delivery_ack_occurs_only_after_native_submission() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let bus = FakeBus::default();
        let submitter = FakeSubmitter {
            events: events.clone(),
            result: Mutex::new(Some(Ok("submission-1".to_string()))),
        };
        let status = test_status();
        let mut pending_acks = BTreeSet::new();

        deliver_polled_messages(
            &RecordingBus {
                inner: bus,
                events: events.clone(),
            },
            &submitter,
            &NoopTaskServiceContextRecorder,
            "runtime-1",
            "agent",
            "cutex.thread-1",
            "thread-1",
            vec![task_service_message()],
            &mut pending_acks,
            &status,
        )
        .expect("deliver message");

        assert_eq!(
            *events.lock().expect("events lock"),
            vec![
                "submit:amsg_task-service-message-1",
                "ack:task-service-message-1"
            ]
        );
        assert!(pending_acks.is_empty());
        let status = status.lock().expect("status lock");
        assert_eq!(status.submitted_count, 1);
        assert_eq!(status.acknowledged_count, 1);
        assert_eq!(
            status.last_message_id.as_deref(),
            Some("task-service-message-1")
        );
        assert_eq!(status.last_submission_id.as_deref(), Some("submission-1"));
    }

    #[test]
    fn worker_followup_requires_context_receipt_before_ack() {
        for fail_record in [false, true] {
            let events = Arc::new(Mutex::new(Vec::new()));
            let bus = RecordingBus {
                inner: FakeBus::default(),
                events: events.clone(),
            };
            let submitter = FakeSubmitter {
                events: events.clone(),
                result: Mutex::new(Some(Ok("submission-1".to_string()))),
            };
            let recorder = RecordingWorkerFollowupContextRecorder {
                events: events.clone(),
                fail_record,
            };
            let status = test_status();
            let mut pending_acks = BTreeSet::new();
            let result = deliver_polled_messages(
                &bus,
                &submitter,
                &recorder,
                "runtime-1",
                "worker",
                "cutex.worker",
                "thread-1",
                vec![task_service_worker_followup_message()],
                &mut pending_acks,
                &status,
            );
            let observed = events.lock().expect("events lock").clone();
            if fail_record {
                assert!(result.is_err());
                assert_eq!(
                    observed,
                    vec![
                        "validate:cutex.worker",
                        "submit:amsg_worker-followup-message-1",
                        "context:worker-followup-message-1",
                    ]
                );
            } else {
                result.expect("context receipt permits ACK");
                assert_eq!(
                    observed,
                    vec![
                        "validate:cutex.worker",
                        "submit:amsg_worker-followup-message-1",
                        "context:worker-followup-message-1",
                        "ack:worker-followup-message-1",
                    ]
                );
            }
        }
    }

    #[test]
    fn task_service_context_receipt_failure_is_not_acknowledged() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let bus = RecordingBus {
            inner: FakeBus::default(),
            events: events.clone(),
        };
        let submitter = FakeSubmitter {
            events: events.clone(),
            result: Mutex::new(Some(Ok("submission-1".to_string()))),
        };
        let status = test_status();
        let mut pending_acks = BTreeSet::new();

        assert!(deliver_polled_messages(
            &bus,
            &submitter,
            &FailingTaskServiceContextRecorder,
            "runtime-1",
            "agent",
            "cutex.thread-1",
            "thread-1",
            vec![task_service_message()],
            &mut pending_acks,
            &status,
        )
        .is_err());
        assert_eq!(
            *events.lock().expect("events lock"),
            vec!["submit:amsg_task-service-message-1"]
        );
        assert!(pending_acks.is_empty());
        assert_eq!(status.lock().expect("status lock").acknowledged_count, 0);
    }

    #[test]
    fn replayed_assignment_acks_despite_projection_failure_and_does_not_starve_followers() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let bus = RecordingBus {
            inner: FakeBus::default(),
            events: events.clone(),
        };
        let submitter = FakeSubmitter {
            events: events.clone(),
            // Native insertion is idempotent by the stable model message ID;
            // this is the exact replay/current-state submission identity.
            result: Mutex::new(Some(Ok("submission-current-state".to_string()))),
        };
        let recorder = ReplayedContextWithFailingProjectionRecorder {
            events: events.clone(),
        };
        let status = test_status();
        let mut pending_acks = BTreeSet::new();
        let mut assignment = task_service_message();
        let mut assignment_metadata = task_service_metadata(&assignment).unwrap().unwrap();
        // Keep the regression hermetic; the fake postprocessor models the
        // coordinator projection without opening the real Management store.
        assignment_metadata.coordinator_cutex_session = None;
        assignment.control_payload = Some(serde_json::to_value(assignment_metadata).unwrap());

        deliver_polled_messages(
            &bus,
            &submitter,
            &recorder,
            "runtime-1",
            "agent",
            "cutex.thread-1",
            "thread-1",
            vec![
                assignment,
                task_service_watchdog_message(),
                test_message(AgentDeliveryMode::Soon),
            ],
            &mut pending_acks,
            &status,
        )
        .expect("optional projection failure must not retain submitted messages");

        let events = events.lock().expect("events lock");
        assert_eq!(
            events
                .iter()
                .filter(|event| event.as_str() == "ack:task-service-message-1")
                .count(),
            1
        );
        let assignment_ack = events
            .iter()
            .position(|event| event == "ack:task-service-message-1")
            .unwrap();
        let watchdog_submit = events
            .iter()
            .position(|event| event.starts_with("submit:amsg_tsw_"))
            .unwrap();
        let ordinary_submit = events
            .iter()
            .position(|event| event == "submit:amsg_message-1")
            .unwrap();
        assert!(watchdog_submit < ordinary_submit);
        assert!(ordinary_submit < assignment_ack);
        assert!(events
            .iter()
            .any(|event| event == "transition-projection-retry"));
        assert!(events
            .iter()
            .any(|event| event.starts_with("watchdog-delivered:")));
        assert!(events
            .iter()
            .any(|event| event.starts_with("watchdog-binding-retry:")));
        assert!(pending_acks.is_empty());
        let status = status.lock().expect("status lock");
        assert_eq!(status.submitted_count, 3);
        assert_eq!(status.acknowledged_count, 3);
    }

    #[test]
    fn failed_submission_is_not_acknowledged() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let bus = RecordingBus {
            inner: FakeBus::default(),
            events: events.clone(),
        };
        let submitter = FakeSubmitter {
            events: events.clone(),
            result: Mutex::new(Some(Err(anyhow::anyhow!("submission failed")))),
        };
        let status = test_status();
        let mut pending_acks = BTreeSet::new();

        deliver_polled_messages(
            &bus,
            &submitter,
            &NoopTaskServiceContextRecorder,
            "runtime-1",
            "agent",
            "cutex.thread-1",
            "thread-1",
            vec![test_message(AgentDeliveryMode::Soon)],
            &mut pending_acks,
            &status,
        )
        .expect("retryable A2 failure remains pending without blocking the sweep");
        assert_eq!(
            *events.lock().expect("events lock"),
            vec!["submit:amsg_message-1"]
        );
        assert!(pending_acks.is_empty());
    }

    #[test]
    fn failed_ack_retries_without_resubmitting() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let inner = FakeBus::default();
        inner
            .ack_results
            .lock()
            .expect("ack results lock")
            .extend([Err(anyhow::anyhow!("ack failed")), Ok(1)]);
        let bus = RecordingBus {
            inner,
            events: events.clone(),
        };
        let submitter = FakeSubmitter {
            events: events.clone(),
            result: Mutex::new(Some(Ok("submission-1".to_string()))),
        };
        let status = test_status();
        let mut pending_acks = BTreeSet::new();

        assert!(deliver_polled_messages(
            &bus,
            &submitter,
            &NoopTaskServiceContextRecorder,
            "runtime-1",
            "agent",
            "cutex.thread-1",
            "thread-1",
            vec![test_message(AgentDeliveryMode::Passive)],
            &mut pending_acks,
            &status,
        )
        .is_err());
        assert!(pending_acks.contains("message-1"));
        retry_pending_acks(&bus, "runtime-1", &mut pending_acks, &status)
            .expect("retry acknowledgement");

        assert_eq!(
            *events.lock().expect("events lock"),
            vec!["submit:amsg_message-1", "ack:message-1", "ack:message-1"]
        );
        assert!(pending_acks.is_empty());
    }

    #[test]
    fn pending_after_turn_does_not_block_later_soon_a4_and_selective_ack() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let bus = RecordingBus {
            inner: FakeBus::default(),
            events: events.clone(),
        };
        let submitter = SelectiveStatusSubmitter {
            events: events.clone(),
            unavailable: false,
        };
        let older = test_message(AgentDeliveryMode::AfterTurn);
        let mut later = test_message(AgentDeliveryMode::Soon);
        later.id = "message-2".to_string();
        let status = test_status();
        let mut pending_acks = BTreeSet::new();

        deliver_polled_messages(
            &bus,
            &submitter,
            &NoopTaskServiceContextRecorder,
            "runtime-1",
            "agent",
            "cutex.thread-1",
            "thread-1",
            vec![older, later],
            &mut pending_acks,
            &status,
        )
        .expect("independent A4 item must progress");

        assert_eq!(
            *events.lock().unwrap(),
            vec!["submit:amsg_message-2", "ack:message-2"]
        );
        assert!(pending_acks.is_empty());
    }

    #[test]
    fn malformed_status_is_isolated_and_does_not_block_unrelated_a4() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let bus = RecordingBus {
            inner: FakeBus::default(),
            events: events.clone(),
        };
        let mut malformed = test_message(AgentDeliveryMode::AfterTurn);
        malformed.id = "message-1".to_string();
        let mut valid = test_message(AgentDeliveryMode::Soon);
        valid.id = "message-2".to_string();
        let status = test_status();
        let mut pending_acks = BTreeSet::new();

        deliver_polled_messages(
            &bus,
            &MalformedFirstStatusSubmitter,
            &NoopTaskServiceContextRecorder,
            "runtime-1",
            "agent",
            "cutex.thread-1",
            "thread-1",
            vec![malformed, valid],
            &mut pending_acks,
            &status,
        )
        .expect("malformed status must be isolated from the valid A4 item");

        assert_eq!(*events.lock().unwrap(), vec!["ack:message-2"]);
        assert!(pending_acks.is_empty());
        assert!(status
            .lock()
            .unwrap()
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("message-1")));
    }

    #[test]
    fn conflict_is_durably_quarantined_while_unrelated_a4_progresses() {
        let prefix = uuid::Uuid::new_v4();
        let conflict_id = format!("{prefix}-message-1");
        let valid_id = format!("{prefix}-message-2");
        let mut conflict = test_message(AgentDeliveryMode::AfterTurn);
        conflict.id = conflict_id.clone();
        conflict.from_cutex_session_id = Some("cutex.source".to_string());
        conflict.to_cutex_session_id = Some("cutex.thread-1".to_string());
        let mut valid = test_message(AgentDeliveryMode::Soon);
        valid.id = valid_id.clone();
        valid.from_cutex_session_id = Some("cutex.source".to_string());
        valid.to_cutex_session_id = Some("cutex.thread-1".to_string());
        let params = inter_agent_params("thread-1", "agent", "cutex.thread-1", &conflict).unwrap();
        agent_bus_message_repository()
            .unwrap()
            .record_queued(
                crate::management::v2::agent_bus_state::AgentBusQueuedMessage {
                    owner_cutex_session_id: "cutex.thread-1".to_string(),
                    message_id: conflict_id.clone(),
                    from_cutex_session_id: "cutex.source".to_string(),
                    to_cutex_session_id: "cutex.thread-1".to_string(),
                    from_runtime_agent_id: Some("runtime-source".to_string()),
                    to_runtime_agent_id: Some("runtime-1".to_string()),
                    delivery_mode: "after_turn".to_string(),
                    content: conflict.content.clone(),
                    queued_at: Utc::now(),
                    canonical_envelope: conflict.clone(),
                    semantic_sha256: inter_agent_semantic_sha256(&params),
                },
            )
            .unwrap();
        let valid_params =
            inter_agent_params("thread-1", "agent", "cutex.thread-1", &valid).unwrap();
        agent_bus_message_repository()
            .unwrap()
            .record_queued(
                crate::management::v2::agent_bus_state::AgentBusQueuedMessage {
                    owner_cutex_session_id: "cutex.thread-1".to_string(),
                    message_id: valid_id.clone(),
                    from_cutex_session_id: "cutex.source".to_string(),
                    to_cutex_session_id: "cutex.thread-1".to_string(),
                    from_runtime_agent_id: Some("runtime-source".to_string()),
                    to_runtime_agent_id: Some("runtime-1".to_string()),
                    delivery_mode: "soon".to_string(),
                    content: valid.content.clone(),
                    queued_at: Utc::now(),
                    canonical_envelope: valid.clone(),
                    semantic_sha256: inter_agent_semantic_sha256(&valid_params),
                },
            )
            .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let bus = RecordingBus {
            inner: FakeBus::default(),
            events: events.clone(),
        };
        let status = test_status();
        let mut pending_acks = BTreeSet::new();

        deliver_polled_messages(
            &bus,
            &ConflictFirstStatusSubmitter,
            &NoopTaskServiceContextRecorder,
            "runtime-1",
            "agent",
            "cutex.thread-1",
            "thread-1",
            vec![conflict, valid],
            &mut pending_acks,
            &status,
        )
        .unwrap();

        assert_eq!(
            *events.lock().unwrap(),
            vec![format!("ack:{conflict_id}"), format!("ack:{valid_id}")]
        );
        let snapshot = agent_bus_message_repository()
            .unwrap()
            .snapshot("cutex.thread-1")
            .unwrap();
        let quarantined = snapshot
            .iter()
            .find(|record| record["messageId"] == conflict_id)
            .expect("conflicting durable record remains observable");
        assert_eq!(quarantined["state"], "quarantined");
        assert_eq!(quarantined["error"]["code"], "native_semantic_conflict");
        let delivered = snapshot
            .iter()
            .find(|record| record["messageId"] == valid_id)
            .expect("valid durable record remains observable");
        assert_eq!(delivered["state"], "delivered");
        assert_eq!(
            delivered["a4Receipt"]["receiptId"],
            format!("a4r_amsg_{valid_id}")
        );
    }

    #[test]
    fn stable_recipient_digest_delivers_once_with_a4_and_one_ack() {
        let message_id = format!("recipient-consistency-{}", uuid::Uuid::new_v4());
        let mut message = test_message(AgentDeliveryMode::AfterTurn);
        message.id = message_id.clone();
        message.from_cutex_session_id = Some("cutex.source".to_string());
        message.to_cutex_session_id = Some("cutex.thread-1".to_string());
        let registration = AgentBusRegisterRequest {
            id: "runtime-1".to_string(),
            name: "Agent Display.9f2a".to_string(),
            base_name: Some("agent-stable".to_string()),
            ..test_options().registration
        };
        let recipient_label = canonical_recipient_label(
            registration.base_name.as_deref(),
            &registration.name,
            &registration.id,
        );
        assert_eq!(recipient_label, "agent-stable");
        assert_eq!(registration.name, "Agent Display.9f2a");
        let params =
            inter_agent_params("thread-1", recipient_label, "cutex.thread-1", &message).unwrap();
        agent_bus_message_repository()
            .unwrap()
            .record_queued(
                crate::management::v2::agent_bus_state::AgentBusQueuedMessage {
                    owner_cutex_session_id: "cutex.thread-1".to_string(),
                    message_id: message_id.clone(),
                    from_cutex_session_id: "cutex.source".to_string(),
                    to_cutex_session_id: "cutex.thread-1".to_string(),
                    from_runtime_agent_id: Some("runtime-source".to_string()),
                    to_runtime_agent_id: Some(registration.id.clone()),
                    delivery_mode: "after_turn".to_string(),
                    content: message.content.clone(),
                    queued_at: Utc::now(),
                    canonical_envelope: message.clone(),
                    semantic_sha256: inter_agent_semantic_sha256(&params),
                },
            )
            .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let bus = RecordingBus {
            inner: FakeBus::default(),
            events: events.clone(),
        };
        let submitter = ImmediateA4Submitter::default();
        let status = test_status();
        let mut pending_acks = BTreeSet::new();

        let outcome = deliver_polled_messages(
            &bus,
            &submitter,
            &NoopTaskServiceContextRecorder,
            &registration.id,
            recipient_label,
            "cutex.thread-1",
            "thread-1",
            vec![message],
            &mut pending_acks,
            &status,
        )
        .unwrap();

        assert!(outcome.made_progress);
        assert!(!outcome.retained_pending);
        assert_eq!(*events.lock().unwrap(), vec![format!("ack:{message_id}")]);
        assert_eq!(submitter.submitted.lock().unwrap().len(), 1);
        assert_eq!(status.lock().unwrap().submitted_count, 1);
        assert_eq!(status.lock().unwrap().acknowledged_count, 1);
        let snapshot = agent_bus_message_repository()
            .unwrap()
            .snapshot("cutex.thread-1")
            .unwrap();
        let delivered = snapshot
            .iter()
            .find(|record| record["messageId"] == message_id)
            .unwrap();
        assert_eq!(delivered["state"], "delivered");
        assert_eq!(
            delivered["a4Receipt"]["receiptId"],
            format!("a4r_amsg_{message_id}")
        );
        assert!(delivered.get("error").is_none());
    }

    #[test]
    fn altered_semantic_envelope_is_quarantined_and_never_submitted() {
        let message_id = format!("recipient-tamper-{}", uuid::Uuid::new_v4());
        let mut canonical = test_message(AgentDeliveryMode::Soon);
        canonical.id = message_id.clone();
        canonical.from_cutex_session_id = Some("cutex.source".to_string());
        canonical.to_cutex_session_id = Some("cutex.thread-1".to_string());
        let params =
            inter_agent_params("thread-1", "agent-stable", "cutex.thread-1", &canonical).unwrap();
        agent_bus_message_repository()
            .unwrap()
            .record_queued(
                crate::management::v2::agent_bus_state::AgentBusQueuedMessage {
                    owner_cutex_session_id: "cutex.thread-1".to_string(),
                    message_id: message_id.clone(),
                    from_cutex_session_id: "cutex.source".to_string(),
                    to_cutex_session_id: "cutex.thread-1".to_string(),
                    from_runtime_agent_id: Some("runtime-source".to_string()),
                    to_runtime_agent_id: Some("runtime-1".to_string()),
                    delivery_mode: "soon".to_string(),
                    content: canonical.content.clone(),
                    queued_at: Utc::now(),
                    canonical_envelope: canonical.clone(),
                    semantic_sha256: inter_agent_semantic_sha256(&params),
                },
            )
            .unwrap();
        let mut altered = canonical;
        altered.content = "tampered after durable commit".to_string();
        let events = Arc::new(Mutex::new(Vec::new()));
        let bus = RecordingBus {
            inner: FakeBus::default(),
            events: events.clone(),
        };
        let submitter = ImmediateA4Submitter::default();
        let mut pending_acks = BTreeSet::new();

        deliver_polled_messages(
            &bus,
            &submitter,
            &NoopTaskServiceContextRecorder,
            "runtime-1",
            "agent-stable",
            "cutex.thread-1",
            "thread-1",
            vec![altered],
            &mut pending_acks,
            &test_status(),
        )
        .unwrap();

        assert!(submitter.submitted.lock().unwrap().is_empty());
        assert_eq!(*events.lock().unwrap(), vec![format!("ack:{message_id}")]);
        let snapshot = agent_bus_message_repository()
            .unwrap()
            .snapshot("cutex.thread-1")
            .unwrap();
        let quarantined = snapshot
            .iter()
            .find(|record| record["messageId"] == message_id)
            .unwrap();
        assert_eq!(quarantined["state"], "quarantined");
        assert_eq!(
            quarantined["error"]["code"],
            "canonical_semantic_digest_mismatch"
        );
    }

    #[test]
    fn ordinary_a4_fact_before_ack_replays_without_resubmission() {
        let message_id = format!("{}-message-2", uuid::Uuid::new_v4());
        let mut message = test_message(AgentDeliveryMode::Soon);
        message.id = message_id.clone();
        message.from_cutex_session_id = Some("cutex.source".to_string());
        message.to_cutex_session_id = Some("cutex.thread-1".to_string());
        let params = inter_agent_params("thread-1", "agent", "cutex.thread-1", &message).unwrap();
        agent_bus_message_repository()
            .unwrap()
            .record_queued(
                crate::management::v2::agent_bus_state::AgentBusQueuedMessage {
                    owner_cutex_session_id: "cutex.thread-1".to_string(),
                    message_id: message_id.clone(),
                    from_cutex_session_id: "cutex.source".to_string(),
                    to_cutex_session_id: "cutex.thread-1".to_string(),
                    from_runtime_agent_id: Some("runtime-source".to_string()),
                    to_runtime_agent_id: Some("runtime-1".to_string()),
                    delivery_mode: "soon".to_string(),
                    content: message.content.clone(),
                    queued_at: Utc::now(),
                    canonical_envelope: message.clone(),
                    semantic_sha256: inter_agent_semantic_sha256(&params),
                },
            )
            .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let inner = FakeBus::default();
        inner
            .ack_results
            .lock()
            .unwrap()
            .extend([Err(anyhow::anyhow!("crash before ACK commit")), Ok(1)]);
        let bus = RecordingBus {
            inner,
            events: events.clone(),
        };
        let status = test_status();
        let mut pre_crash_acks = BTreeSet::new();

        assert!(deliver_polled_messages(
            &bus,
            &ConflictFirstStatusSubmitter,
            &NoopTaskServiceContextRecorder,
            "runtime-1",
            "agent",
            "cutex.thread-1",
            "thread-1",
            vec![message.clone()],
            &mut pre_crash_acks,
            &status,
        )
        .is_err());
        assert!(pre_crash_acks.contains(&message_id));

        let mut reopened_acks = BTreeSet::new();
        deliver_polled_messages(
            &bus,
            &ConflictFirstStatusSubmitter,
            &NoopTaskServiceContextRecorder,
            "runtime-1",
            "agent",
            "cutex.thread-1",
            "thread-1",
            vec![message],
            &mut reopened_acks,
            &status,
        )
        .unwrap();

        assert_eq!(
            *events.lock().unwrap(),
            vec![format!("ack:{message_id}"), format!("ack:{message_id}")]
        );
        assert!(reopened_acks.is_empty());
        let snapshot = agent_bus_message_repository()
            .unwrap()
            .snapshot("cutex.thread-1")
            .unwrap();
        let delivered = snapshot
            .iter()
            .find(|record| record["messageId"] == message_id)
            .unwrap();
        assert_eq!(delivered["state"], "delivered");
        assert_eq!(
            delivered["a4Receipt"]["receiptId"],
            format!("a4r_amsg_{message_id}")
        );
    }

    #[test]
    fn missing_a4_status_method_never_falls_back_to_a2_or_ack() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let bus = RecordingBus {
            inner: FakeBus::default(),
            events: events.clone(),
        };
        let submitter = SelectiveStatusSubmitter {
            events: events.clone(),
            unavailable: true,
        };
        let status = test_status();
        let mut pending_acks = BTreeSet::new();

        assert!(deliver_polled_messages(
            &bus,
            &submitter,
            &NoopTaskServiceContextRecorder,
            "runtime-1",
            "agent",
            "cutex.thread-1",
            "thread-1",
            vec![test_message(AgentDeliveryMode::Soon)],
            &mut pending_acks,
            &status,
        )
        .is_err());
        assert!(events.lock().unwrap().is_empty());
        assert!(pending_acks.is_empty());
    }

    #[test]
    fn pending_ack_dedupe_is_keyed_by_original_bus_id() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let bus = RecordingBus {
            inner: FakeBus::default(),
            events: events.clone(),
        };
        let submitter = FakeSubmitter {
            events: events.clone(),
            result: Mutex::new(Some(Ok("submission-1".to_string()))),
        };
        let status = test_status();
        let mut pending_acks = BTreeSet::from(["message-1".to_string()]);

        deliver_polled_messages(
            &bus,
            &submitter,
            &NoopTaskServiceContextRecorder,
            "runtime-1",
            "agent",
            "cutex.thread-1",
            "thread-1",
            vec![test_message(AgentDeliveryMode::AfterTurn)],
            &mut pending_acks,
            &status,
        )
        .expect("skip pending message");

        assert!(events.lock().expect("events lock").is_empty());
        assert_eq!(pending_acks, BTreeSet::from(["message-1".to_string()]));
        assert_eq!(status.lock().expect("status lock").submitted_count, 0);
    }

    #[test]
    fn pending_poll_backoff_is_bounded_deterministic_and_progress_resets_it() {
        let pending = DeliverySweepOutcome {
            had_messages: true,
            retained_pending: true,
            made_progress: false,
        };
        let mut backoff = PendingPollBackoff::default();
        assert_eq!(
            backoff.wait_after(
                pending,
                Duration::from_millis(20),
                Duration::from_millis(70)
            ),
            Some(Duration::from_millis(20))
        );
        assert_eq!(
            backoff.wait_after(
                pending,
                Duration::from_millis(20),
                Duration::from_millis(70)
            ),
            Some(Duration::from_millis(40))
        );
        assert_eq!(
            backoff.wait_after(
                pending,
                Duration::from_millis(20),
                Duration::from_millis(70)
            ),
            Some(Duration::from_millis(70))
        );
        assert_eq!(
            backoff.wait_after(
                DeliverySweepOutcome {
                    made_progress: true,
                    ..pending
                },
                Duration::from_millis(20),
                Duration::from_millis(70),
            ),
            Some(Duration::from_millis(20))
        );
        assert_eq!(
            backoff.wait_after(
                DeliverySweepOutcome {
                    had_messages: true,
                    retained_pending: false,
                    made_progress: true,
                },
                Duration::from_millis(20),
                Duration::from_millis(70),
            ),
            None
        );
    }

    #[test]
    fn pending_native_delivery_backs_off_then_reaches_a4_once() {
        let mut message = test_message(AgentDeliveryMode::AfterTurn);
        message.id = format!("pending-backoff-{}", uuid::Uuid::new_v4());
        message.from_cutex_session_id = Some("cutex.source".to_string());
        message.to_cutex_session_id = Some("cutex.thread-1".to_string());
        let params = inter_agent_params("thread-1", "agent", "cutex.thread-1", &message).unwrap();
        agent_bus_message_repository()
            .unwrap()
            .record_queued(
                crate::management::v2::agent_bus_state::AgentBusQueuedMessage {
                    owner_cutex_session_id: "cutex.thread-1".to_string(),
                    message_id: message.id.clone(),
                    from_cutex_session_id: "cutex.source".to_string(),
                    to_cutex_session_id: "cutex.thread-1".to_string(),
                    from_runtime_agent_id: Some("runtime-source".to_string()),
                    to_runtime_agent_id: Some("runtime-1".to_string()),
                    delivery_mode: "after_turn".to_string(),
                    content: message.content.clone(),
                    queued_at: Utc::now(),
                    canonical_envelope: message.clone(),
                    semantic_sha256: inter_agent_semantic_sha256(&params),
                },
            )
            .unwrap();
        let message_id = message.id.clone();
        let bus = Arc::new(RepeatingWorkerBus {
            message,
            poll_count: AtomicUsize::new(0),
            ack_count: AtomicUsize::new(0),
            acked: AtomicBool::new(false),
        });
        let submitter = Arc::new(ControlledPendingSubmitter {
            context_persisted: AtomicBool::new(false),
            submit_count: AtomicUsize::new(0),
        });
        let mut options = test_options();
        options.poll_interval = Duration::from_millis(120);
        options.retry_interval = Duration::from_millis(480);
        options.registration_refresh_interval = Duration::from_secs(60);
        let bridge = AppServerAgentBusBridge::spawn(bus.clone(), submitter.clone(), options)
            .expect("start pending bridge");

        let deadline = Instant::now() + Duration::from_secs(2);
        while submitter.submit_count.load(Ordering::Acquire) != 1 {
            assert!(
                Instant::now() < deadline,
                "pending message was not submitted"
            );
            thread::sleep(Duration::from_millis(2));
        }
        let polls_after_pending = bus.poll_count.load(Ordering::Acquire);
        thread::sleep(Duration::from_millis(40));
        assert_eq!(
            bus.poll_count.load(Ordering::Acquire),
            polls_after_pending,
            "pending A4 status must not trigger an immediate Agent Bus repoll"
        );

        submitter.context_persisted.store(true, Ordering::Release);
        while bus.ack_count.load(Ordering::Acquire) != 1 {
            assert!(
                Instant::now() < deadline,
                "pending message did not reach A4"
            );
            thread::sleep(Duration::from_millis(5));
        }
        bridge.shutdown().expect("stop pending bridge");
        assert_eq!(submitter.submit_count.load(Ordering::Acquire), 1);
        assert_eq!(bus.ack_count.load(Ordering::Acquire), 1);
        let snapshot = agent_bus_message_repository()
            .unwrap()
            .snapshot("cutex.thread-1")
            .unwrap();
        let delivered = snapshot
            .iter()
            .find(|record| record["messageId"] == message_id)
            .expect("pending item retains its durable A4 delivery fact");
        assert_eq!(delivered["state"], "delivered");
        assert_eq!(
            delivered["a4Receipt"]["receiptId"],
            format!("a4r_amsg_{message_id}")
        );
    }

    #[test]
    fn fully_acked_nonempty_page_immediately_drains_next_page() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut first = test_message(AgentDeliveryMode::Soon);
        first.id = format!("acked-first-{}", uuid::Uuid::new_v4());
        let mut second = test_message(AgentDeliveryMode::Soon);
        second.id = format!("acked-second-{}", uuid::Uuid::new_v4());
        let first_id = first.id.clone();
        let second_id = second.id.clone();
        let bus = Arc::new(WorkerBus {
            events: events.clone(),
            polls: Mutex::new(VecDeque::from([vec![first], vec![second], Vec::new()])),
        });
        let submitter = Arc::new(ImmediateA4Submitter::default());
        let mut options = test_options();
        options.poll_interval = Duration::from_secs(1);
        options.retry_interval = Duration::from_secs(2);
        options.registration_refresh_interval = Duration::from_secs(60);
        let bridge =
            AppServerAgentBusBridge::spawn(bus, submitter, options).expect("start backlog bridge");
        let deadline = Instant::now() + Duration::from_secs(2);
        while bridge.status().unwrap().acknowledged_count != 2 {
            assert!(Instant::now() < deadline, "backlog did not drain");
            thread::sleep(Duration::from_millis(5));
        }
        bridge.shutdown().unwrap();

        let events = events.lock().unwrap();
        let first_ack = events
            .iter()
            .position(|event| event == &format!("ack:{first_id}"))
            .unwrap();
        let second_ack = events
            .iter()
            .position(|event| event == &format!("ack:{second_id}"))
            .unwrap();
        assert_eq!(events[first_ack + 1], "poll");
        assert_eq!(second_ack, first_ack + 2);
    }

    #[test]
    fn pending_after_turn_allows_later_soon_selective_progress_in_worker() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut pending = test_message(AgentDeliveryMode::AfterTurn);
        pending.id = "worker-pending-message-1".to_string();
        let mut later = test_message(AgentDeliveryMode::Soon);
        later.id = "worker-later-message-2".to_string();
        let bus = Arc::new(WorkerBus {
            events: events.clone(),
            polls: Mutex::new(VecDeque::from([
                vec![pending.clone()],
                vec![pending, later.clone()],
                Vec::new(),
            ])),
        });
        let submitter = Arc::new(SelectiveStatusSubmitter {
            events: events.clone(),
            unavailable: false,
        });
        let mut options = test_options();
        options.poll_interval = Duration::from_millis(40);
        options.retry_interval = Duration::from_millis(160);
        options.registration_refresh_interval = Duration::from_secs(60);
        let bridge = AppServerAgentBusBridge::spawn(bus, submitter, options)
            .expect("start selective bridge");
        let deadline = Instant::now() + Duration::from_secs(2);
        while bridge.status().unwrap().acknowledged_count != 1 {
            assert!(
                Instant::now() < deadline,
                "later soon item did not progress"
            );
            thread::sleep(Duration::from_millis(5));
        }
        bridge.shutdown().unwrap();

        let events = events.lock().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.as_str() == "submit:amsg_worker-later-message-2")
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.as_str() == "submit:amsg_worker-pending-message-1")
                .count(),
            0,
            "already-pending native delivery must not be resubmitted"
        );
        assert!(events
            .iter()
            .any(|event| event == "ack:worker-later-message-2"));
        assert!(!events
            .iter()
            .any(|event| event == "ack:worker-pending-message-1"));
    }

    #[test]
    fn bridge_shutdown_interrupts_pending_poll_backoff_promptly() {
        let mut message = test_message(AgentDeliveryMode::AfterTurn);
        message.id = format!("pending-shutdown-{}", uuid::Uuid::new_v4());
        let bus = Arc::new(RepeatingWorkerBus {
            message,
            poll_count: AtomicUsize::new(0),
            ack_count: AtomicUsize::new(0),
            acked: AtomicBool::new(false),
        });
        let submitter = Arc::new(ControlledPendingSubmitter {
            context_persisted: AtomicBool::new(false),
            submit_count: AtomicUsize::new(0),
        });
        let mut options = test_options();
        options.poll_interval = Duration::from_secs(1);
        options.retry_interval = Duration::from_secs(2);
        options.registration_refresh_interval = Duration::from_secs(60);
        let bridge = AppServerAgentBusBridge::spawn(bus, submitter.clone(), options)
            .expect("start pending shutdown bridge");
        let deadline = Instant::now() + Duration::from_secs(2);
        while submitter.submit_count.load(Ordering::Acquire) != 1 {
            assert!(
                Instant::now() < deadline,
                "pending message was not submitted"
            );
            thread::sleep(Duration::from_millis(2));
        }

        let shutdown_started = Instant::now();
        bridge.shutdown().expect("stop bridge during pending wait");
        assert!(shutdown_started.elapsed() < Duration::from_millis(250));
    }

    #[test]
    fn bridge_worker_registers_delivers_and_stops_cleanly() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let bus = Arc::new(WorkerBus {
            events: events.clone(),
            polls: Mutex::new(VecDeque::from([
                vec![test_message(AgentDeliveryMode::Soon)],
                Vec::new(),
            ])),
        });
        let submitter = Arc::new(FakeSubmitter {
            events: events.clone(),
            result: Mutex::new(Some(Ok("submission-1".to_string()))),
        });
        let mut options = test_options();
        options.poll_interval = Duration::from_millis(10);
        options.retry_interval = Duration::from_millis(10);
        options.registration_refresh_interval = Duration::from_secs(1);

        let bridge = AppServerAgentBusBridge::spawn(bus, submitter, options)
            .expect("start agent-bus bridge");
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if bridge.status().expect("bridge status").acknowledged_count == 1 {
                break;
            }
            assert!(Instant::now() < deadline, "bridge did not deliver message");
            thread::sleep(Duration::from_millis(10));
        }
        bridge.shutdown().expect("stop bridge");

        let events = events.lock().expect("events lock");
        assert_eq!(
            &events[..4],
            ["register", "poll", "submit:amsg_message-1", "ack:message-1"]
        );
    }

    #[test]
    fn refreshed_exact_registration_can_receive_a_task_service_assignment() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let bus = Arc::new(WorkerBus {
            events: events.clone(),
            polls: Mutex::new(VecDeque::from([Vec::new()])),
        });
        let submitter = Arc::new(FakeSubmitter {
            events: events.clone(),
            result: Mutex::new(Some(Ok("submission-after-recovery".to_string()))),
        });
        let mut options = test_options();
        options.poll_interval = Duration::from_secs(1);
        options.registration_refresh_interval = Duration::from_secs(60);
        let bridge = AppServerAgentBusBridge::spawn(bus.clone(), submitter.clone(), options)
            .expect("start exact runtime bridge");

        let refreshed = bridge
            .refresh_registration()
            .expect("re-register exact recovered runtime");
        assert!(refreshed.registered);

        let status = test_status();
        let mut pending_acks = BTreeSet::new();
        deliver_polled_messages(
            bus.as_ref(),
            submitter.as_ref(),
            &NoopTaskServiceContextRecorder,
            "runtime-1",
            "agent",
            "cutex.thread-1",
            "thread-1",
            vec![task_service_message()],
            &mut pending_acks,
            &status,
        )
        .expect("deliver Task Service assignment after recovery refresh");
        bridge.shutdown().expect("stop bridge");

        let events = events.lock().expect("events lock");
        let register_positions = events
            .iter()
            .enumerate()
            .filter_map(|(index, event)| (event == "register").then_some(index))
            .collect::<Vec<_>>();
        assert_eq!(register_positions.len(), 2);
        let submit_position = events
            .iter()
            .position(|event| event == "submit:amsg_task-service-message-1")
            .expect("native Task Service submission");
        assert!(register_positions[1] < submit_position);
        assert!(events
            .iter()
            .any(|event| event == "ack:task-service-message-1"));
    }

    #[test]
    fn bridge_worker_stops_when_app_server_disconnects() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let bus = Arc::new(WorkerBus {
            events: events.clone(),
            polls: Mutex::new(VecDeque::from([Vec::new()])),
        });
        let submitter = Arc::new(FakeSubmitter {
            events,
            result: Mutex::new(None),
        });
        let mut options = test_options();
        options.poll_interval = Duration::from_millis(10);
        let runtime_alive = Arc::new(AtomicBool::new(true));
        let bridge = AppServerAgentBusBridge::spawn_with_liveness(
            bus,
            submitter,
            options,
            runtime_alive.clone(),
        )
        .expect("start agent-bus bridge");

        runtime_alive.store(false, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if !bridge.status().expect("bridge status").running {
                break;
            }
            assert!(Instant::now() < deadline, "bridge did not stop");
            thread::sleep(Duration::from_millis(10));
        }
        bridge.shutdown().expect("join bridge");
    }

    #[test]
    fn bridge_shutdown_wakes_a_blocking_poll() {
        let bus = Arc::new(BlockingPollBus::default());
        let observed_bus = bus.clone();
        let submitter = Arc::new(FakeSubmitter {
            events: Arc::new(Mutex::new(Vec::new())),
            result: Mutex::new(None),
        });
        let bridge = AppServerAgentBusBridge::spawn(bus, submitter, test_options())
            .expect("start agent-bus bridge");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !observed_bus.poll_started.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "bridge did not start polling");
            thread::sleep(Duration::from_millis(5));
        }

        let shutdown_started = Instant::now();
        bridge.shutdown().expect("stop blocking bridge");

        assert!(shutdown_started.elapsed() < Duration::from_millis(500));
        assert!(*observed_bus.unregistered.lock().expect("unregistered lock"));
    }

    struct RecordingBus {
        inner: FakeBus,
        events: Arc<Mutex<Vec<String>>>,
    }

    struct WorkerBus {
        events: Arc<Mutex<Vec<String>>>,
        polls: Mutex<VecDeque<Vec<AgentBusMessage>>>,
    }

    struct RepeatingWorkerBus {
        message: AgentBusMessage,
        poll_count: AtomicUsize,
        ack_count: AtomicUsize,
        acked: AtomicBool,
    }

    struct ControlledPendingSubmitter {
        context_persisted: AtomicBool,
        submit_count: AtomicUsize,
    }

    #[derive(Default)]
    struct ImmediateA4Submitter {
        submitted: Mutex<BTreeSet<String>>,
    }

    impl InterAgentMessageSubmitter for ControlledPendingSubmitter {
        fn submit_inter_agent_message(
            &self,
            params: &ThreadInterAgentMessageParams,
        ) -> anyhow::Result<String> {
            self.submit_count.fetch_add(1, Ordering::AcqRel);
            Ok(format!("a2_{}", params.message_id))
        }

        fn inter_agent_message_status(
            &self,
            params: &ThreadInterAgentMessageStatusParams,
        ) -> anyhow::Result<ThreadInterAgentMessageStatusResponse> {
            let submitted = self.submit_count.load(Ordering::Acquire) > 0;
            let persisted = self.context_persisted.load(Ordering::Acquire);
            Ok(status_response(params, |query| {
                if persisted {
                    context_persisted_status(params, query, "turn-pending")
                } else {
                    pending_status(
                        query,
                        if submitted {
                            ThreadInterAgentMessageDeliveryState::Pending
                        } else {
                            ThreadInterAgentMessageDeliveryState::Unknown
                        },
                    )
                }
            }))
        }
    }

    impl InterAgentMessageSubmitter for ImmediateA4Submitter {
        fn submit_inter_agent_message(
            &self,
            params: &ThreadInterAgentMessageParams,
        ) -> anyhow::Result<String> {
            self.submitted
                .lock()
                .unwrap()
                .insert(params.message_id.clone());
            Ok(format!("a2_{}", params.message_id))
        }

        fn inter_agent_message_status(
            &self,
            params: &ThreadInterAgentMessageStatusParams,
        ) -> anyhow::Result<ThreadInterAgentMessageStatusResponse> {
            let submitted = self.submitted.lock().unwrap().clone();
            Ok(status_response(params, |query| {
                if submitted.contains(&query.message_id) {
                    context_persisted_status(params, query, "turn-immediate")
                } else {
                    pending_status(query, ThreadInterAgentMessageDeliveryState::Unknown)
                }
            }))
        }
    }

    fn status_response(
        params: &ThreadInterAgentMessageStatusParams,
        mut status_for: impl FnMut(
            &ThreadInterAgentMessageStatusQuery,
        ) -> super::super::commands::ThreadInterAgentMessageStatus,
    ) -> ThreadInterAgentMessageStatusResponse {
        ThreadInterAgentMessageStatusResponse {
            schema: INTER_AGENT_STATUS_SCHEMA.to_string(),
            thread_id: params.thread_id.clone(),
            statuses: params.messages.iter().map(&mut status_for).collect(),
        }
    }

    fn pending_status(
        query: &ThreadInterAgentMessageStatusQuery,
        state: ThreadInterAgentMessageDeliveryState,
    ) -> super::super::commands::ThreadInterAgentMessageStatus {
        super::super::commands::ThreadInterAgentMessageStatus {
            message_id: query.message_id.clone(),
            state,
            semantic_sha256: query.semantic_sha256.clone(),
            receipt: None,
        }
    }

    fn context_persisted_status(
        params: &ThreadInterAgentMessageStatusParams,
        query: &ThreadInterAgentMessageStatusQuery,
        turn_id: &str,
    ) -> super::super::commands::ThreadInterAgentMessageStatus {
        super::super::commands::ThreadInterAgentMessageStatus {
            message_id: query.message_id.clone(),
            state: ThreadInterAgentMessageDeliveryState::ContextPersisted,
            semantic_sha256: query.semantic_sha256.clone(),
            receipt: Some(InterAgentContextPersistedReceipt {
                schema: Some(INTER_AGENT_STATUS_SCHEMA.to_string()),
                receipt_id: format!("a4r_{}", query.message_id),
                thread_id: Some(params.thread_id.clone()),
                message_id: Some(query.message_id.clone()),
                semantic_sha256: Some(query.semantic_sha256.clone()),
                response_item_id: query.message_id.clone(),
                turn_id: turn_id.to_string(),
                rollout_ordinal: 1,
            }),
        }
    }

    #[derive(Default)]
    struct BlockingPollBus {
        poll_started: AtomicBool,
        unregistered: Mutex<bool>,
        wake: Condvar,
    }

    impl RuntimeAgentBus for BlockingPollBus {
        fn register(&self, _request: &AgentBusRegisterRequest) -> anyhow::Result<()> {
            Ok(())
        }

        fn unregister(&self, _agent_id: &str) -> anyhow::Result<bool> {
            *self.unregistered.lock().expect("unregistered lock") = true;
            self.wake.notify_all();
            Ok(true)
        }

        fn poll(&self, _agent_id: &str) -> anyhow::Result<Vec<AgentBusMessage>> {
            self.poll_started.store(true, Ordering::Release);
            let unregistered = self.unregistered.lock().expect("unregistered lock");
            let _ = self
                .wake
                .wait_timeout_while(unregistered, Duration::from_secs(5), |value| !*value)
                .map_err(|_| anyhow::anyhow!("unregistered lock was poisoned"))?;
            Ok(Vec::new())
        }

        fn ack(&self, _agent_id: &str, _message_ids: &[String]) -> anyhow::Result<usize> {
            Ok(0)
        }
    }

    impl RuntimeAgentBus for WorkerBus {
        fn register(&self, _request: &AgentBusRegisterRequest) -> anyhow::Result<()> {
            self.events
                .lock()
                .expect("events lock")
                .push("register".to_string());
            Ok(())
        }

        fn unregister(&self, _agent_id: &str) -> anyhow::Result<bool> {
            self.events
                .lock()
                .expect("events lock")
                .push("unregister".to_string());
            Ok(true)
        }

        fn poll(&self, _agent_id: &str) -> anyhow::Result<Vec<AgentBusMessage>> {
            self.events
                .lock()
                .expect("events lock")
                .push("poll".to_string());
            Ok(self
                .polls
                .lock()
                .expect("polls lock")
                .pop_front()
                .unwrap_or_default())
        }

        fn ack(&self, _agent_id: &str, message_ids: &[String]) -> anyhow::Result<usize> {
            self.events
                .lock()
                .expect("events lock")
                .push(format!("ack:{}", message_ids.join(",")));
            Ok(message_ids.len())
        }
    }

    impl RuntimeAgentBus for RepeatingWorkerBus {
        fn register(&self, _request: &AgentBusRegisterRequest) -> anyhow::Result<()> {
            Ok(())
        }

        fn unregister(&self, _agent_id: &str) -> anyhow::Result<bool> {
            Ok(true)
        }

        fn poll(&self, _agent_id: &str) -> anyhow::Result<Vec<AgentBusMessage>> {
            self.poll_count.fetch_add(1, Ordering::AcqRel);
            if self.acked.load(Ordering::Acquire) {
                Ok(Vec::new())
            } else {
                Ok(vec![self.message.clone()])
            }
        }

        fn ack(&self, _agent_id: &str, message_ids: &[String]) -> anyhow::Result<usize> {
            self.ack_count.fetch_add(1, Ordering::AcqRel);
            self.acked.store(true, Ordering::Release);
            Ok(message_ids.len())
        }
    }

    impl RuntimeAgentBus for RecordingBus {
        fn register(&self, request: &AgentBusRegisterRequest) -> anyhow::Result<()> {
            self.inner.register(request)
        }

        fn unregister(&self, agent_id: &str) -> anyhow::Result<bool> {
            self.inner.unregister(agent_id)
        }

        fn poll(&self, agent_id: &str) -> anyhow::Result<Vec<AgentBusMessage>> {
            self.inner.poll(agent_id)
        }

        fn ack(&self, agent_id: &str, message_ids: &[String]) -> anyhow::Result<usize> {
            self.events
                .lock()
                .expect("events lock")
                .push(format!("ack:{}", message_ids.join(",")));
            self.inner
                .ack_results
                .lock()
                .expect("ack results lock")
                .pop_front()
                .unwrap_or_else(|| Ok(message_ids.len()))
                .with_context(|| format!("ack failed for {agent_id}"))
        }
    }

    fn test_message(delivery_mode: AgentDeliveryMode) -> AgentBusMessage {
        AgentBusMessage {
            id: "message-1".to_string(),
            kind: AgentBusEnvelopeKind::Message,
            from: "Sender Agent".to_string(),
            to: "runtime-1".to_string(),
            from_cutex_session_id: None,
            to_cutex_session_id: None,
            content: "hello".to_string(),
            delivery_mode,
            trigger_turn: true,
            created_at_epoch_secs: 1,
            sender_kind: AgentMessageKind::Agent,
            display_source: None,
            submit_mode: None,
            control_type: None,
            control_payload: None,
            external_action_id: None,
            external_message_id: None,
        }
    }

    fn task_service_message() -> AgentBusMessage {
        let opaque_contract = "# Exact Contract\nUnicode: λ".to_string();
        let metadata = TaskServiceAssignmentMetadata {
            project_id: None,
            schema: TASK_SERVICE_PROVIDER_ACTION_SCHEMA.to_string(),
            coordinator_cutex_session: Some(
                crate::role_revision::CutexSessionId::new("cutex.director-r11").unwrap(),
            ),
            assignment_id: crate::task_service::AssignmentId::new("assignment-1").unwrap(),
            task_id: crate::role_revision::TaskId::new("CUTEX-188").unwrap(),
            task_revision: crate::role_revision::TaskRevision::new(1).unwrap(),
            contract_sha256: crate::task_service::sha256_bytes(opaque_contract.as_bytes()),
            opaque_contract: Some(opaque_contract),
            send_attempt_id: crate::task_service::SendAttemptId::new("send-1").unwrap(),
        };
        AgentBusMessage {
            id: "task-service-message-1".to_string(),
            kind: AgentBusEnvelopeKind::Message,
            from: TASK_SERVICE_SYSTEM_SENDER.to_string(),
            to: "runtime-1".to_string(),
            from_cutex_session_id: None,
            to_cutex_session_id: None,
            content: "assignment summary".to_string(),
            delivery_mode: AgentDeliveryMode::Soon,
            trigger_turn: true,
            created_at_epoch_secs: 1,
            sender_kind: AgentMessageKind::TaskServiceSystem,
            display_source: Some("Cutex Task Service".to_string()),
            submit_mode: None,
            control_type: Some(TASK_SERVICE_CONTROL_TYPE.to_string()),
            control_payload: Some(serde_json::to_value(metadata).unwrap()),
            external_action_id: Some("action-1".to_string()),
            external_message_id: Some("external-message-1".to_string()),
        }
    }

    fn task_service_completion_message() -> AgentBusMessage {
        let metadata = TaskServiceCompletionMetadata {
            project_id: None,
            schema: TASK_SERVICE_PROVIDER_ACTION_SCHEMA.to_string(),
            notification_id: crate::task_service::NotificationId::new("notification-1").unwrap(),
            assignment_id: crate::task_service::AssignmentId::new("assignment-1").unwrap(),
            task_id: crate::role_revision::TaskId::new("CUTEX-188").unwrap(),
            task_revision: crate::role_revision::TaskRevision::new(1).unwrap(),
            attempt_number: Some(crate::role_revision::AttemptNumber::new(1).unwrap()),
            transition_action_id: crate::task_service::ActionId::new("submit-1").unwrap(),
            kind: crate::task_service::CompletionNotificationKind::ReviewReady,
            target_seat_id: crate::task_service::SeatId::new("cutex-release").unwrap(),
        };
        AgentBusMessage {
            id: "completion-message-1".to_string(),
            kind: AgentBusEnvelopeKind::Message,
            from: TASK_SERVICE_SYSTEM_SENDER.to_string(),
            to: "runtime-1".to_string(),
            from_cutex_session_id: None,
            to_cutex_session_id: None,
            content: "review is ready".to_string(),
            delivery_mode: AgentDeliveryMode::AfterTurn,
            trigger_turn: true,
            created_at_epoch_secs: 1,
            sender_kind: AgentMessageKind::TaskServiceSystem,
            display_source: Some("Cutex Task Service".to_string()),
            submit_mode: None,
            control_type: Some(TASK_SERVICE_COMPLETION_CONTROL_TYPE.to_string()),
            control_payload: Some(serde_json::to_value(metadata).unwrap()),
            external_action_id: Some("submit-1".to_string()),
            external_message_id: Some("notification-1".to_string()),
        }
    }

    fn task_service_worker_followup_message() -> AgentBusMessage {
        let metadata = TaskServiceWorkerFollowupMetadata {
            project_id: None,
            schema: TASK_SERVICE_PROVIDER_ACTION_SCHEMA.to_string(),
            notification_id: crate::task_service::NotificationId::new("notification-1").unwrap(),
            assignment_id: crate::task_service::AssignmentId::new("assignment-1").unwrap(),
            task_id: crate::role_revision::TaskId::new("CUTEX-188").unwrap(),
            task_revision: crate::role_revision::TaskRevision::new(3).unwrap(),
            attempt_number: crate::role_revision::AttemptNumber::new(2).unwrap(),
            decision_reference: "fix the focused regression".to_string(),
        };
        AgentBusMessage {
            id: "worker-followup-message-1".to_string(),
            kind: AgentBusEnvelopeKind::Message,
            from: TASK_SERVICE_SYSTEM_SENDER.to_string(),
            to: "runtime-1".to_string(),
            from_cutex_session_id: None,
            to_cutex_session_id: None,
            content: metadata.decision_reference.clone(),
            delivery_mode: AgentDeliveryMode::Soon,
            trigger_turn: true,
            created_at_epoch_secs: 1,
            sender_kind: AgentMessageKind::TaskServiceSystem,
            display_source: Some("Cutex Task Service".to_string()),
            submit_mode: None,
            control_type: Some(TASK_SERVICE_WORKER_FOLLOWUP_CONTROL_TYPE.to_string()),
            control_payload: Some(serde_json::to_value(metadata).unwrap()),
            external_action_id: Some("request-changes-action".to_string()),
            external_message_id: Some("notification-1".to_string()),
        }
    }

    fn task_service_watchdog_message() -> AgentBusMessage {
        let metadata = crate::task_service::TaskWatchdogMessageMetadata {
            schema: crate::task_service::TASK_WATCHDOG_MESSAGE_SCHEMA.to_string(),
            notification_id: "twn_0123456789abcdef0123456789abcdef".to_string(),
            project_id: Some(crate::agent_management::ProjectId::new("project-1").unwrap()),
            assignment_id: "assignment-1".to_string(),
            attempt_number: 1,
            stage: crate::task_service::TaskWatchdogStage::FirstStale,
        };
        AgentBusMessage {
            id: "tsw_twn_0123456789abcdef0123456789abcdef".to_string(),
            kind: AgentBusEnvelopeKind::Message,
            from: TASK_SERVICE_SYSTEM_SENDER.to_string(),
            to: "runtime-1".to_string(),
            from_cutex_session_id: None,
            to_cutex_session_id: None,
            content: "Task Service watchdog: assignment assignment-1 attempt 1 has no authoritative progress for at least 600 seconds. Continue the task or report bounded status.".to_string(),
            delivery_mode: AgentDeliveryMode::Soon,
            trigger_turn: true,
            created_at_epoch_secs: 1,
            sender_kind: AgentMessageKind::TaskServiceSystem,
            display_source: Some("Cutex Task Service".to_string()),
            submit_mode: None,
            control_type: Some(TASK_SERVICE_WATCHDOG_CONTROL_TYPE.to_string()),
            control_payload: Some(serde_json::to_value(metadata).unwrap()),
            external_action_id: Some("twn_0123456789abcdef0123456789abcdef".to_string()),
            external_message_id: Some("twn_0123456789abcdef0123456789abcdef".to_string()),
        }
    }

    fn agent_management_message() -> AgentBusMessage {
        let metadata = AgentManagementMessageMetadata {
            schema: AgentManagementSchema::V1,
            requested_by_director: crate::role_revision::CutexSessionId::new("cutex.director-r11")
                .unwrap(),
        };
        AgentBusMessage {
            id: "agent-management-message-1".to_string(),
            kind: AgentBusEnvelopeKind::Message,
            from: AGENT_MANAGEMENT_SYSTEM_SENDER.to_string(),
            to: "runtime-1".to_string(),
            from_cutex_session_id: None,
            to_cutex_session_id: None,
            content: "custom start body".to_string(),
            delivery_mode: AgentDeliveryMode::AfterTurn,
            trigger_turn: true,
            created_at_epoch_secs: 1,
            sender_kind: AgentMessageKind::Agent,
            display_source: Some("Agent Management System".to_string()),
            submit_mode: None,
            control_type: Some(AGENT_MANAGEMENT_START_CONTROL_TYPE.to_string()),
            control_payload: Some(serde_json::to_value(metadata).unwrap()),
            external_action_id: None,
            external_message_id: Some("agent-management:create:start".to_string()),
        }
    }

    fn test_status() -> Arc<Mutex<AppServerAgentBusBridgeStatus>> {
        Arc::new(Mutex::new(AppServerAgentBusBridgeStatus {
            runtime_agent_id: "runtime-1".to_string(),
            thread_id: "thread-1".to_string(),
            running: true,
            registered: true,
            pending_ack_count: 0,
            submitted_count: 0,
            acknowledged_count: 0,
            last_poll_at: None,
            last_message_id: None,
            last_submission_id: None,
            last_error: None,
        }))
    }

    fn test_options() -> AppServerAgentBusBridgeOptions {
        AppServerAgentBusBridgeOptions::new(
            AgentBusRegisterRequest {
                id: "runtime-1".to_string(),
                name: "agent".to_string(),
                base_name: Some("agent".to_string()),
                thread_name: None,
                path_key: None,
                session_id: Some("thread-1".to_string()),
                profile: "profile".to_string(),
                cwd: "/tmp".to_string(),
                pid: 42,
                host_id: Some("host".to_string()),
                groups: Vec::new(),
                registration_class: AgentRegistrationClass::Persistent,
            },
            "thread-1",
        )
    }

    #[test]
    fn bridge_options_require_native_thread_identity_and_app_server_pid() {
        let mut options = test_options();
        assert!(options.validate().is_ok());

        options.registration.session_id = Some("thread-2".to_string());
        assert!(options.validate().is_err());
        options.registration.session_id = Some("thread-1".to_string());
        options.registration.pid = 0;
        assert!(options.validate().is_err());
    }
}
