use crate::model::{
    HealthState, ProcessIdentity, RunId, ServiceDefinition, ServiceId, ShutdownPolicy, StopOutcome,
};
use crate::protocol::LogStream;
use std::fmt;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Debug)]
pub enum BackendEvent {
    Output {
        service_id: ServiceId,
        run_id: RunId,
        stream: LogStream,
        bytes: Vec<u8>,
    },
    LogFailure {
        service_id: ServiceId,
        run_id: RunId,
        message: String,
    },
    ContainmentFailure {
        service_id: ServiceId,
        run_id: RunId,
        message: String,
    },
    Health {
        service_id: ServiceId,
        run_id: RunId,
        health: HealthState,
    },
    Exit {
        service_id: ServiceId,
        run_id: RunId,
        exit_code: Option<i32>,
    },
}

pub trait ProcessBackend: Send + Sync {
    fn name(&self) -> &'static str;

    fn start(
        &self,
        definition: &ServiceDefinition,
        run_id: &RunId,
    ) -> Result<ProcessIdentity, ProcessBackendError>;

    fn stop(
        &self,
        service_id: &ServiceId,
        run_id: &RunId,
        shutdown_policy: &ShutdownPolicy,
    ) -> Result<StopOutcome, ProcessBackendError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessBackendError {
    pub message: String,
    pub retryable: bool,
}

impl ProcessBackendError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }

    pub fn retryable(mut self) -> Self {
        self.retryable = true;
        self
    }
}

impl fmt::Display for ProcessBackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(f)
    }
}

impl std::error::Error for ProcessBackendError {}

#[derive(Debug, Default)]
pub(crate) struct FakeProcessBackend {
    next_pid: AtomicU32,
}

impl ProcessBackend for FakeProcessBackend {
    fn name(&self) -> &'static str {
        "in_memory_fake"
    }

    fn start(
        &self,
        _definition: &ServiceDefinition,
        run_id: &RunId,
    ) -> Result<ProcessIdentity, ProcessBackendError> {
        let pid = self
            .next_pid
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(10_000);
        Ok(ProcessIdentity {
            pid,
            birth_marker: format!("fake-birth-{run_id}"),
        })
    }

    fn stop(
        &self,
        _service_id: &ServiceId,
        _run_id: &RunId,
        _shutdown_policy: &ShutdownPolicy,
    ) -> Result<StopOutcome, ProcessBackendError> {
        Ok(StopOutcome::Graceful)
    }
}
