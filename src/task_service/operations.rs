use std::path::PathBuf;
use std::sync::Arc;

use chrono::SecondsFormat;

use super::model::{
    AttemptFence, EventPage, EventPageRequest, JournalCursor, ReceiptLookup, ReceiptQuery,
    ReceiptRecord, Rfc3339, TaskAttempt, TaskId, TaskQuery, TaskRecord, TaskRevision,
    TaskServiceError, TaskStore, TransitionEnvelope, TransitionOutcome,
};
use super::store::{PageAndSubscription, TaskRepository, TrustedClock};

#[derive(Clone)]
pub(crate) struct TaskService {
    repository: TaskRepository,
    clock: Arc<dyn TrustedClock>,
}

impl TaskService {
    pub(crate) fn new(root: impl Into<PathBuf>) -> Result<Self, TaskServiceError> {
        Ok(Self {
            repository: TaskRepository::new(root)?,
            clock: Arc::new(SystemClock),
        })
    }

    pub(crate) fn recover(&self) -> Result<(), TaskServiceError> {
        self.repository.load().map(|_| ())
    }

    pub(crate) fn transition(&self, envelope: &TransitionEnvelope) -> TransitionOutcome {
        self.repository.transition(envelope, self.clock.as_ref())
    }

    pub(crate) fn load(&self) -> Result<TaskStore, TaskServiceError> {
        self.repository.load()
    }

    pub(crate) fn get_task(
        &self,
        task_id: &TaskId,
        task_revision: Option<TaskRevision>,
    ) -> Result<Option<TaskRecord>, TaskServiceError> {
        self.repository.get_task(task_id, task_revision)
    }

    pub(crate) fn get_attempt(
        &self,
        fence: &AttemptFence,
    ) -> Result<Option<TaskAttempt>, TaskServiceError> {
        self.repository.get_attempt(fence)
    }

    pub(crate) fn get_receipt(&self, envelope: &TransitionEnvelope) -> ReceiptLookup {
        self.repository.get_receipt(envelope)
    }

    pub(crate) fn query_task(
        &self,
        query: &TaskQuery,
    ) -> Result<Option<TaskRecord>, TaskServiceError> {
        self.repository
            .get_task(&query.task_id, query.task_revision)
    }

    pub(crate) fn query_receipt(
        &self,
        query: &ReceiptQuery,
    ) -> Result<Option<ReceiptRecord>, TaskServiceError> {
        self.repository.get_receipt_record(&query.receipt_id)
    }

    pub(crate) fn checkpoint(&self) -> Result<JournalCursor, TaskServiceError> {
        self.repository.checkpoint()
    }

    pub(crate) fn page_events(
        &self,
        request: &EventPageRequest,
    ) -> Result<EventPage, TaskServiceError> {
        self.repository.page_events(request)
    }

    pub(crate) fn page_and_subscribe(
        &self,
        request: &super::model::SubscriptionRequest,
    ) -> Result<PageAndSubscription, TaskServiceError> {
        self.repository.page_and_subscribe(request)
    }

    #[cfg(test)]
    pub(super) fn with_clock(
        root: impl Into<PathBuf>,
        clock: Arc<dyn TrustedClock>,
    ) -> Result<Self, TaskServiceError> {
        Ok(Self {
            repository: TaskRepository::new(root)?,
            clock,
        })
    }

    #[cfg(test)]
    pub(super) fn with_clock_and_fault(
        root: impl Into<PathBuf>,
        clock: Arc<dyn TrustedClock>,
        fault: super::persist::FaultPoint,
    ) -> Result<Self, TaskServiceError> {
        Ok(Self {
            repository: TaskRepository::with_test_fault(root, fault)?,
            clock,
        })
    }
}

struct SystemClock;

impl TrustedClock for SystemClock {
    fn now(&self) -> Rfc3339 {
        Rfc3339::new(chrono::Utc::now().to_rfc3339_opts(SecondsFormat::AutoSi, true))
            .expect("UTC clock output is normalized RFC3339")
    }
}
