use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }
    };
}

string_id!(ServiceId);
string_id!(RunId);
string_id!(OperationId);
string_id!(SubscriptionId);

/// A durable service definition. Its identity is `id`, never a process or UI
/// attribute. Environment values are additions/overrides to the host's
/// controlled base environment; this model intentionally has no shell hooks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceDefinition {
    pub id: ServiceId,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub executable: String,
    #[serde(default)]
    pub arguments: Vec<String>,
    pub working_directory: String,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    pub start_policy: StartPolicy,
    pub restart_policy: RestartPolicy,
    #[serde(default)]
    pub dependencies: Vec<ServiceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readiness_probe: Option<ReadinessProbe>,
    pub shutdown_policy: ShutdownPolicy,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

impl ServiceDefinition {
    pub fn validate(&self) -> Result<(), DefinitionValidationError> {
        validate_service_id(&self.id)?;
        if self.display_name.trim().is_empty() {
            return Err(DefinitionValidationError::new(
                "display_name",
                "display name must not be blank",
            ));
        }
        if self.display_name.len() > 256 {
            return Err(DefinitionValidationError::new(
                "display_name",
                "display name exceeds 256 bytes",
            ));
        }
        if !is_absolute_executable(&self.executable) {
            return Err(DefinitionValidationError::new(
                "executable",
                "executable must be an absolute path",
            ));
        }
        if !is_absolute_executable(&self.working_directory) {
            return Err(DefinitionValidationError::new(
                "working_directory",
                "working directory must be an absolute path",
            ));
        }
        reject_nul("executable", &self.executable)?;
        reject_nul("working_directory", &self.working_directory)?;
        for argument in &self.arguments {
            reject_nul("arguments", argument)?;
        }
        for (name, value) in &self.environment {
            if name.is_empty() || name.contains('=') || name.contains('\0') {
                return Err(DefinitionValidationError::new(
                    "environment",
                    "environment names must be non-empty and contain neither '=' nor NUL",
                ));
            }
            reject_nul("environment", value)?;
        }
        let mut dependencies = BTreeSet::new();
        for dependency in &self.dependencies {
            validate_service_id(dependency)?;
            if dependency == &self.id {
                return Err(DefinitionValidationError::new(
                    "dependencies",
                    "a service cannot depend on itself",
                ));
            }
            if !dependencies.insert(dependency) {
                return Err(DefinitionValidationError::new(
                    "dependencies",
                    "dependencies must not contain duplicates",
                ));
            }
        }
        self.restart_policy.validate()?;
        self.shutdown_policy.validate()?;
        if let Some(probe) = &self.readiness_probe {
            probe.validate()?;
        }
        Ok(())
    }
}

fn validate_service_id(id: &ServiceId) -> Result<(), DefinitionValidationError> {
    let value = id.as_str();
    if value.is_empty() || value.len() > 128 {
        return Err(DefinitionValidationError::new(
            "id",
            "service ID must contain between 1 and 128 bytes",
        ));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        || !value.as_bytes()[0].is_ascii_alphanumeric()
    {
        return Err(DefinitionValidationError::new(
            "id",
            "service ID must start with an ASCII alphanumeric and contain only ASCII alphanumerics, '.', '_' or '-'",
        ));
    }
    Ok(())
}

fn reject_nul(field: &'static str, value: &str) -> Result<(), DefinitionValidationError> {
    if value.contains('\0') {
        return Err(DefinitionValidationError::new(
            field,
            "value must not contain NUL",
        ));
    }
    Ok(())
}

/// Accept both Unix and Windows absolute syntax so definitions can be prepared
/// on a different client platform. The concrete host revalidates for its OS.
fn is_absolute_executable(value: &str) -> bool {
    if value.starts_with('/') || value.starts_with("\\\\") {
        return true;
    }
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefinitionValidationError {
    pub field: &'static str,
    pub message: &'static str,
}

impl DefinitionValidationError {
    fn new(field: &'static str, message: &'static str) -> Self {
        Self { field, message }
    }
}

impl fmt::Display for DefinitionValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

impl std::error::Error for DefinitionValidationError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartPolicy {
    Manual,
    OnHostStart,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum RestartPolicy {
    Never,
    BoundedOnFailure {
        max_restarts: u32,
        window_ms: u64,
        backoff_ms: u64,
    },
}

impl RestartPolicy {
    fn validate(&self) -> Result<(), DefinitionValidationError> {
        if let Self::BoundedOnFailure {
            max_restarts,
            window_ms,
            backoff_ms,
        } = self
        {
            if *max_restarts == 0 || *max_restarts > 1_000 {
                return Err(DefinitionValidationError::new(
                    "restart_policy.max_restarts",
                    "max_restarts must be between 1 and 1000",
                ));
            }
            if *window_ms == 0 || *window_ms > 86_400_000 {
                return Err(DefinitionValidationError::new(
                    "restart_policy.window_ms",
                    "restart window must be between 1 millisecond and 24 hours",
                ));
            }
            if *backoff_ms > *window_ms {
                return Err(DefinitionValidationError::new(
                    "restart_policy.backoff_ms",
                    "restart backoff must not exceed the restart window",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReadinessProbe {
    Tcp {
        host: String,
        port: u16,
        interval_ms: u64,
        timeout_ms: u64,
    },
    Http {
        url: String,
        interval_ms: u64,
        timeout_ms: u64,
        expected_status_min: u16,
        expected_status_max: u16,
    },
}

impl ReadinessProbe {
    fn validate(&self) -> Result<(), DefinitionValidationError> {
        match self {
            Self::Tcp {
                host,
                port,
                interval_ms,
                timeout_ms,
            } => {
                if host.trim().is_empty() || *port == 0 {
                    return Err(DefinitionValidationError::new(
                        "readiness_probe",
                        "TCP probe requires a host and non-zero port",
                    ));
                }
                validate_probe_timings(*interval_ms, *timeout_ms)?;
            }
            Self::Http {
                url,
                interval_ms,
                timeout_ms,
                expected_status_min,
                expected_status_max,
            } => {
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return Err(DefinitionValidationError::new(
                        "readiness_probe.url",
                        "HTTP probe URL must use http or https",
                    ));
                }
                if !(100..=599).contains(expected_status_min)
                    || !(100..=599).contains(expected_status_max)
                    || expected_status_min > expected_status_max
                {
                    return Err(DefinitionValidationError::new(
                        "readiness_probe.expected_status",
                        "HTTP status range must be ordered and between 100 and 599",
                    ));
                }
                validate_probe_timings(*interval_ms, *timeout_ms)?;
            }
        }
        Ok(())
    }
}

fn validate_probe_timings(
    interval_ms: u64,
    timeout_ms: u64,
) -> Result<(), DefinitionValidationError> {
    if interval_ms == 0 || interval_ms > 3_600_000 || timeout_ms == 0 || timeout_ms > interval_ms {
        return Err(DefinitionValidationError::new(
            "readiness_probe",
            "probe timeout must be non-zero and no greater than an interval of at most one hour",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShutdownPolicy {
    pub graceful_timeout_ms: u64,
    pub force_kill_timeout_ms: u64,
}

impl ShutdownPolicy {
    fn validate(&self) -> Result<(), DefinitionValidationError> {
        let total = self
            .graceful_timeout_ms
            .checked_add(self.force_kill_timeout_ms)
            .ok_or_else(|| {
                DefinitionValidationError::new(
                    "shutdown_policy",
                    "shutdown timeout total overflowed",
                )
            })?;
        if total == 0 || total > 3_600_000 {
            return Err(DefinitionValidationError::new(
                "shutdown_policy",
                "shutdown must be bounded between 1 millisecond and 1 hour",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesiredState {
    Running,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    NotApplicable,
    Unknown,
    Starting,
    Healthy,
    Unhealthy,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    /// Platform-specific birth marker used with PID to avoid PID-reuse claims.
    pub birth_marker: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastExit {
    pub run_id: RunId,
    pub exited_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub unexpected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopOutcome {
    Graceful,
    Forced,
    TimedOut,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeState {
    pub desired_state: DesiredState,
    pub observed_state: ObservedState,
    pub health: HealthState,
    pub definition_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<OperationId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_phase: Option<OperationPhase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<ProcessIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_exit: Option<LastExit>,
    pub restart_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_stop_outcome: Option<StopOutcome>,
}

impl RuntimeState {
    pub fn stopped(definition_revision: u64) -> Self {
        Self {
            desired_state: DesiredState::Stopped,
            observed_state: ObservedState::Stopped,
            health: HealthState::NotApplicable,
            definition_revision,
            run_id: None,
            operation_id: None,
            operation_phase: None,
            process: None,
            started_at_ms: None,
            last_exit: None,
            restart_count: 0,
            last_stop_outcome: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceSnapshot {
    pub definition: ServiceDefinition,
    pub runtime: RuntimeState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostPhase {
    Running,
    ShuttingDown,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostStatus {
    pub phase: HostPhase,
    pub registry_revision: u64,
    pub services: Vec<ServiceSnapshot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    ShutdownHost,
    EnsureRunning,
    EnsureStopped,
    Restart,
    Reconcile,
    RegisterService,
    UpdateService,
    RemoveService,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationPhase {
    Queued,
    Running,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operation {
    pub id: OperationId,
    pub kind: OperationKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_id: Option<ServiceId>,
    pub phase: OperationPhase,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<crate::protocol::ApiError>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceMutationResult {
    pub operation: Operation,
    pub service: ServiceSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconcileResult {
    pub operation: Operation,
    pub services: Vec<ServiceSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveServiceResult {
    pub operation: Operation,
    pub removed_service_id: ServiceId,
    pub registry_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShutdownHostResult {
    pub operation: Operation,
    pub final_phase: HostPhase,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefinitionMutationResult {
    pub operation: Operation,
    pub service: ServiceSnapshot,
    pub registry_revision: u64,
}
