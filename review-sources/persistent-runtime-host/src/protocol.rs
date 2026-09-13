use crate::model::{
    DefinitionMutationResult, HostPhase, HostStatus, Operation, OperationId, ReconcileResult,
    RemoveServiceResult, RunId, ServiceDefinition, ServiceId, ServiceMutationResult,
    ServiceSnapshot, ShutdownHostResult, StopOutcome, SubscriptionId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const PROTOCOL_MAJOR: u16 = 1;
pub const PROTOCOL_MINOR: u16 = 0;

pub const CAPABILITY_SERVICE_REGISTRY: &str = "service.registry.v1";
pub const CAPABILITY_SERVICE_CONTROL: &str = "service.control.v1";
pub const CAPABILITY_OPERATIONS: &str = "operations.v1";
pub const CAPABILITY_EVENT_STREAM: &str = "events.subscribe.v1";
pub const CAPABILITY_LOG_READ: &str = "logs.read.v1";
pub const CAPABILITY_IDEMPOTENCY: &str = "mutations.idempotency.v1";
pub const CAPABILITY_EXPECTED_REVISION: &str = "mutations.expected_revision.v1";

pub fn host_capabilities() -> Vec<String> {
    [
        CAPABILITY_SERVICE_REGISTRY,
        CAPABILITY_SERVICE_CONTROL,
        CAPABILITY_OPERATIONS,
        CAPABILITY_EVENT_STREAM,
        CAPABILITY_LOG_READ,
        CAPABILITY_IDEMPOTENCY,
        CAPABILITY_EXPECTED_REVISION,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

impl ProtocolVersion {
    pub const V1: Self = Self {
        major: PROTOCOL_MAJOR,
        minor: PROTOCOL_MINOR,
    };
}

/// Stable v1 client envelope. Unknown fields are intentionally tolerated by
/// Serde, allowing additive optional fields within a protocol major.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub protocol: ProtocolVersion,
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub request_id: String,
    pub request: Request,
}

impl RequestEnvelope {
    pub fn v1(request_id: impl Into<String>, request: Request) -> Self {
        Self {
            protocol: ProtocolVersion::V1,
            capabilities: Vec::new(),
            request_id: request_id.into(),
            request,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum Request {
    GetHostInfo(GetHostInfoParams),
    GetHostStatus(GetHostStatusParams),
    ShutdownHost(ShutdownHostParams),
    ListServices(ListServicesParams),
    GetService(GetServiceParams),
    EnsureRunning(EnsureRunningParams),
    EnsureStopped(EnsureStoppedParams),
    Restart(RestartParams),
    Reconcile(ReconcileParams),
    GetOperation(GetOperationParams),
    ReadLogs(ReadLogsParams),
    SubscribeEvents(SubscribeEventsParams),
    RegisterService(RegisterServiceParams),
    UpdateService(UpdateServiceParams),
    RemoveService(RemoveServiceParams),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetHostInfoParams {}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetHostStatusParams {}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListServicesParams {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationOptions {
    pub idempotency_key: String,
    /// For register this is the registry revision. For mutations targeting an
    /// existing service this is that service's definition revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
}

impl MutationOptions {
    pub fn new(idempotency_key: impl Into<String>) -> Self {
        Self {
            idempotency_key: idempotency_key.into(),
            expected_revision: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShutdownHostParams {
    pub mutation: MutationOptions,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetServiceParams {
    pub service_id: ServiceId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnsureRunningParams {
    pub service_id: ServiceId,
    pub mutation: MutationOptions,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnsureStoppedParams {
    pub service_id: ServiceId,
    #[serde(default)]
    pub cascade: bool,
    pub mutation: MutationOptions,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestartParams {
    pub service_id: ServiceId,
    #[serde(default)]
    pub cascade: bool,
    pub mutation: MutationOptions,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconcileParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_id: Option<ServiceId>,
    pub mutation: MutationOptions,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetOperationParams {
    pub operation_id: OperationId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadLogsParams {
    pub service_id: ServiceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_sequence: Option<u64>,
    pub limit: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeEventsParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_sequence: Option<u64>,
    #[serde(default = "default_replay_limit")]
    pub replay_limit: u32,
}

fn default_replay_limit() -> u32 {
    256
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterServiceParams {
    pub definition: ServiceDefinition,
    pub mutation: MutationOptions,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateServiceParams {
    pub service_id: ServiceId,
    pub definition: ServiceDefinition,
    pub mutation: MutationOptions,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveServiceParams {
    pub service_id: ServiceId,
    pub mutation: MutationOptions,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub protocol: ProtocolVersion,
    pub capabilities: Vec<String>,
    pub request_id: String,
    #[serde(flatten)]
    pub outcome: ResponseOutcome,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ResponseOutcome {
    Ok { response: Box<Response> },
    Error { error: ApiError },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "result")]
pub enum Response {
    GetHostInfo(HostInfo),
    GetHostStatus(HostStatus),
    ShutdownHost(ShutdownHostResult),
    ListServices(ListServicesResult),
    GetService(ServiceSnapshot),
    EnsureRunning(ServiceMutationResult),
    EnsureStopped(ServiceMutationResult),
    Restart(ServiceMutationResult),
    Reconcile(ReconcileResult),
    GetOperation(Operation),
    ReadLogs(LogPage),
    SubscribeEvents(SubscribeEventsResult),
    RegisterService(DefinitionMutationResult),
    UpdateService(DefinitionMutationResult),
    RemoveService(RemoveServiceResult),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostInfo {
    pub product: String,
    pub version: String,
    pub host_instance_id: String,
    pub protocol: ProtocolVersion,
    pub capabilities: Vec<String>,
    pub backend: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListServicesResult {
    pub services: Vec<ServiceSnapshot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    UnsupportedProtocol,
    InvalidRequest,
    InvalidArgument,
    NotFound,
    AlreadyExists,
    RevisionMismatch,
    IdempotencyConflict,
    DependencyMissing,
    DependencyCycle,
    DependencyInUse,
    ServiceBusy,
    HostShuttingDown,
    OperationNotFound,
    RegistryConflict,
    LogUnavailable,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, Value>,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: false,
            details: BTreeMap::new(),
        }
    }

    pub fn detail(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.details.insert(key.into(), value.into());
        self
    }

    pub fn retryable(mut self) -> Self {
        self.retryable = true;
        self
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for ApiError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogStream {
    Stdout,
    Stderr,
    Host,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogEncoding {
    Utf8,
    Base64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub service_id: ServiceId,
    pub run_id: RunId,
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub stream: LogStream,
    pub encoding: LogEncoding,
    pub data: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogDiagnostics {
    pub evicted_entries: u64,
    pub evicted_bytes: u64,
    pub slow_subscriber_drops: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogPage {
    pub service_id: ServiceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    pub entries: Vec<LogEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_sequence: Option<u64>,
    pub diagnostics: LogDiagnostics,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeEventsResult {
    pub subscription_id: SubscriptionId,
    pub replay: Vec<HostEvent>,
    pub oldest_available_sequence: u64,
    pub next_sequence: u64,
    pub replay_gap: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub protocol: ProtocolVersion,
    pub subscription_id: SubscriptionId,
    pub event: HostEvent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostEvent {
    pub sequence: u64,
    pub timestamp_ms: u64,
    #[serde(flatten)]
    pub event: HostEventKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostEventKind {
    HostPhaseChanged {
        phase: HostPhase,
    },
    DefinitionRegistered {
        service_id: ServiceId,
        definition_revision: u64,
    },
    DefinitionUpdated {
        service_id: ServiceId,
        definition_revision: u64,
    },
    DefinitionRemoved {
        service_id: ServiceId,
    },
    ServiceStateChanged {
        service_id: ServiceId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<RunId>,
        desired_state: crate::model::DesiredState,
        observed_state: crate::model::ObservedState,
        health: crate::model::HealthState,
    },
    OperationChanged {
        operation: Operation,
    },
    ServiceExited {
        service_id: ServiceId,
        run_id: RunId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        unexpected: bool,
    },
    ServiceStopped {
        service_id: ServiceId,
        run_id: RunId,
        outcome: StopOutcome,
    },
    LogFailure {
        service_id: ServiceId,
        run_id: RunId,
        message: String,
    },
}

pub fn protocol_error(request_id: impl Into<String>, version: ProtocolVersion) -> ResponseEnvelope {
    ResponseEnvelope {
        protocol: ProtocolVersion::V1,
        capabilities: host_capabilities(),
        request_id: request_id.into(),
        outcome: ResponseOutcome::Error {
            error: ApiError::new(
                ErrorCode::UnsupportedProtocol,
                format!(
                    "protocol major {} is not supported; this host supports major {}",
                    version.major, PROTOCOL_MAJOR
                ),
            )
            .detail("received_major", u64::from(version.major))
            .detail("supported_major", u64::from(PROTOCOL_MAJOR)),
        },
    }
}
