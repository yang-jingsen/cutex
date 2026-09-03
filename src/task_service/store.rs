use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use fs2::FileExt;

use super::digest::{canonical_command_digest, compact_record_line, make_record};
use super::journal;
use super::model::{
    empty_store, validate_envelope, validate_store, validate_subscription_request, AttemptFence,
    AttemptNumber, EventPage, EventPageRequest, JournalCursor, JournalEvent, JournalRecord,
    ReceiptLookup, ReceiptRecord, ReceiptSchema, ResyncReason, Rfc3339, StoreRevision, TaskAttempt,
    TaskCommand, TaskId, TaskPhase, TaskRecord, TaskRevision, TaskServiceError, TaskStore,
    TransitionEnvelope, TransitionEvent, TransitionOutcome, TransitionResponse, WatchItem,
    WatchReceiveError,
};
use super::persist::{self, AppendFailure, FaultController, FaultPoint, RootHandle};

pub(super) trait TrustedClock: Send + Sync {
    fn now(&self) -> Rfc3339;
}

#[derive(Clone)]
pub struct TaskRepository {
    root: Arc<RootHandle>,
    local_boundary: Arc<Mutex<()>>,
    watchers: Arc<WatchHub>,
    faults: Arc<FaultController>,
    observed_checkpoint: Arc<Mutex<JournalCursor>>,
}

pub struct PageAndSubscription {
    pub page: EventPage,
    pub subscription: Option<EventSubscription>,
}

pub struct EventSubscription {
    state: Arc<SubscriptionState>,
}

struct SubscriptionState {
    inner: Mutex<SubscriptionInner>,
    ready: Condvar,
}

struct SubscriptionInner {
    cursor: JournalCursor,
    task_id: Option<TaskId>,
    capacity: usize,
    queue: VecDeque<JournalRecord>,
    terminal: Option<ResyncReason>,
    terminal_delivered: bool,
    alive: bool,
}

#[derive(Default)]
struct WatchHub {
    subscribers: Mutex<Vec<Weak<SubscriptionState>>>,
}

struct LockedState {
    store: TaskStore,
    records: Vec<JournalRecord>,
    effects: LoadEffects,
}

#[derive(Default)]
struct LoadEffects {
    replayed: bool,
    recovered: bool,
}

struct PlannedTransition {
    tasks: BTreeMap<TaskId, BTreeMap<TaskRevision, TaskRecord>>,
    task_id: TaskId,
    task_revision: TaskRevision,
    attempt_number: Option<AttemptNumber>,
    prior_phase: Option<TaskPhase>,
    resulting_phase: TaskPhase,
}

impl TaskRepository {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, TaskServiceError> {
        let root = root.into();
        let root = Arc::new(persist::validate_root(&root)?);
        Ok(Self {
            root,
            local_boundary: Arc::new(Mutex::new(())),
            watchers: Arc::new(WatchHub::default()),
            faults: Arc::new(FaultController::default()),
            observed_checkpoint: Arc::new(Mutex::new(JournalCursor::genesis())),
        })
    }

    pub(super) fn transition(
        &self,
        envelope: &TransitionEnvelope,
        clock: &dyn TrustedClock,
    ) -> TransitionOutcome {
        if let Err(code) = validate_envelope(envelope) {
            return TransitionOutcome::NoWrite(TaskServiceError::InvalidEnvelope { code });
        }
        let digest = match canonical_command_digest(envelope) {
            Ok(digest) if digest == envelope.request_digest_sha256 => digest,
            Ok(_) => {
                return TransitionOutcome::NoWrite(TaskServiceError::RequestDigestMismatch);
            }
            Err(error) => return TransitionOutcome::NoWrite(error),
        };
        let root = self.root.as_ref();
        if let Err(error) = root.validate_binding() {
            return TransitionOutcome::NoWrite(error);
        }
        let _boundary = self.local_boundary.lock().expect("repository mutex");
        let lock = match persist::open_lock(&root, false) {
            Ok(Some(lock)) => lock,
            Ok(None) => {
                let empty = match load_without_lock(&root) {
                    Ok(store) => store,
                    Err(error) => return TransitionOutcome::NoWrite(error),
                };
                if let Err(error) = preflight_empty_transition(&empty, envelope) {
                    return TransitionOutcome::NoWrite(error);
                }
                match persist::open_lock(&root, true) {
                    Ok(Some(lock)) => lock,
                    Ok(None) => unreachable!("create=true always returns a lock"),
                    Err(error) => return TransitionOutcome::NoWrite(error),
                }
            }
            Err(error) => return TransitionOutcome::NoWrite(error),
        };
        if let Err(error) = FileExt::lock_exclusive(&lock) {
            return TransitionOutcome::NoWrite(TaskServiceError::Io {
                stage: super::model::IoStage::Lock,
                kind: error.kind(),
            });
        }
        let current = match self.load_locked_recovering(root) {
            Ok(current) => current,
            Err(error) => {
                self.mark_load_error(&error);
                return TransitionOutcome::NoWrite(error);
            }
        };
        self.mark_load_effects(&current.effects, &current.store.journal_checkpoint);
        if let Some(record) = current.store.receipts.get(&envelope.receipt_id) {
            if record.request_digest_sha256 != digest {
                return TransitionOutcome::NoWrite(TaskServiceError::ReceiptConflict);
            }
            return TransitionOutcome::Committed(record.response.clone());
        }
        if current.store.store_revision != envelope.expected_store_revision {
            return TransitionOutcome::NoWrite(TaskServiceError::StoreRevisionConflict {
                expected: envelope.expected_store_revision,
                actual: current.store.store_revision,
            });
        }
        let next_revision = match current.store.store_revision.checked_next() {
            Ok(revision) => revision,
            Err(_) => return TransitionOutcome::NoWrite(TaskServiceError::StoreRevisionOverflow),
        };
        let planned = match plan_transition(&current.store, envelope) {
            Ok(planned) => planned,
            Err(error) => return TransitionOutcome::NoWrite(error),
        };

        let preflight_time =
            Rfc3339::new("1970-01-01T00:00:00Z").expect("fixed preflight timestamp is valid");
        let (_, _, preflight_store) = finalize_transition(
            &current.store,
            &planned,
            envelope,
            &digest,
            next_revision,
            preflight_time,
        );
        if let Err(code) = validate_store(&preflight_store) {
            return TransitionOutcome::NoWrite(TaskServiceError::InvalidStore { code });
        }
        let _ = serde_json::to_vec(&preflight_store)
            .expect("TaskStore contains no fallible serde representation");
        if self.faults.hit(FaultPoint::BeforeJournalAppend) {
            return TransitionOutcome::NoWrite(TaskServiceError::InjectedDefiniteNoWrite);
        }
        let mut journal_file = match persist::prepare_journal_append(&root) {
            Ok(file) => file,
            Err(error) => return TransitionOutcome::NoWrite(error),
        };

        // The trusted clock is deliberately below strict reload, receipt replay,
        // CAS, fence, phase, journal-open, and complete deterministic planning.
        let committed_at = clock.now();
        let (response, record, next) = finalize_transition(
            &current.store,
            &planned,
            envelope,
            &digest,
            next_revision,
            committed_at,
        );
        let line = compact_record_line(&record);
        let snapshot =
            serde_json::to_vec(&next).expect("TaskStore contains no fallible serde representation");

        match persist::append_transition(root, &mut journal_file, &line, &self.faults) {
            Ok(()) => {}
            Err(AppendFailure::Unknown(phase)) => {
                self.watchers.mark_all(ResyncReason::PersistenceUnknown);
                return TransitionOutcome::PersistenceUnknown {
                    receipt_id: envelope.receipt_id.clone(),
                    phase,
                };
            }
        }
        if let Err(phase) =
            persist::replace_snapshot_after_transition(root, &snapshot, &self.faults)
        {
            self.watchers.mark_all(ResyncReason::PersistenceUnknown);
            return TransitionOutcome::PersistenceUnknown {
                receipt_id: envelope.receipt_id.clone(),
                phase,
            };
        }
        self.observe_committed(&record.cursor());
        self.watchers.broadcast(&record);
        TransitionOutcome::Committed(response)
    }

    pub fn load(&self) -> Result<TaskStore, TaskServiceError> {
        let root = self.root.as_ref();
        root.validate_binding()?;
        let _boundary = self.local_boundary.lock().expect("repository mutex");
        let Some(lock) = persist::open_lock(root, false)? else {
            return load_without_lock(root);
        };
        FileExt::lock_exclusive(&lock).map_err(|error| TaskServiceError::Io {
            stage: super::model::IoStage::Lock,
            kind: error.kind(),
        })?;
        let state = self.load_locked_recovering(root).map_err(|error| {
            self.mark_load_error(&error);
            error
        })?;
        self.mark_load_effects(&state.effects, &state.store.journal_checkpoint);
        Ok(state.store)
    }

    pub fn get_task(
        &self,
        task_id: &TaskId,
        task_revision: Option<TaskRevision>,
    ) -> Result<Option<TaskRecord>, TaskServiceError> {
        let store = self.load()?;
        let Some(revisions) = store.tasks.get(task_id) else {
            return Ok(None);
        };
        Ok(match task_revision {
            Some(revision) => revisions.get(&revision).cloned(),
            None => revisions
                .iter()
                .next_back()
                .map(|(_, record)| record.clone()),
        })
    }

    pub fn get_attempt(
        &self,
        fence: &AttemptFence,
    ) -> Result<Option<TaskAttempt>, TaskServiceError> {
        let record = self.get_task(&fence.task_id, Some(fence.task_revision))?;
        let Some(record) = record else {
            return Ok(None);
        };
        let Some(attempt) = record.attempt else {
            return Ok(None);
        };
        if !fence_matches(fence, &attempt) {
            return Err(TaskServiceError::StaleFence);
        }
        Ok(Some(attempt))
    }

    pub fn checkpoint(&self) -> Result<JournalCursor, TaskServiceError> {
        Ok(self.load()?.journal_checkpoint)
    }

    pub fn get_receipt(&self, envelope: &TransitionEnvelope) -> ReceiptLookup {
        let digest = match canonical_command_digest(envelope) {
            Ok(digest) if digest == envelope.request_digest_sha256 => digest,
            Ok(_) => return ReceiptLookup::ReceiptConflict,
            Err(error) => return ReceiptLookup::Unavailable(error),
        };
        let store = match self.load() {
            Ok(store) => store,
            Err(error) => return ReceiptLookup::Unavailable(error),
        };
        match store.receipts.get(&envelope.receipt_id) {
            None => ReceiptLookup::NotFound,
            Some(record) if record.request_digest_sha256 != digest => {
                ReceiptLookup::ReceiptConflict
            }
            Some(record) => ReceiptLookup::Committed(record.response.clone()),
        }
    }

    pub fn get_receipt_record(
        &self,
        receipt_id: &super::model::ReceiptId,
    ) -> Result<Option<ReceiptRecord>, TaskServiceError> {
        Ok(self.load()?.receipts.get(receipt_id).cloned())
    }

    pub fn page_events(&self, request: &EventPageRequest) -> Result<EventPage, TaskServiceError> {
        super::model::validate_page_request(request)?;
        let root = self.root.as_ref();
        root.validate_binding()?;
        let _boundary = self.local_boundary.lock().expect("repository mutex");
        let Some(lock) = persist::open_lock(root, false)? else {
            let store = load_without_lock(root)?;
            let _ = store;
            return journal::page_records(&[], request);
        };
        FileExt::lock_exclusive(&lock).map_err(|error| TaskServiceError::Io {
            stage: super::model::IoStage::Lock,
            kind: error.kind(),
        })?;
        let state = self.load_locked_read_only(root).map_err(|error| {
            self.mark_load_error(&error);
            error
        })?;
        self.mark_load_effects(&state.effects, &state.store.journal_checkpoint);
        journal::page_records(&state.records, request)
    }

    pub fn page_and_subscribe(
        &self,
        request: &super::model::SubscriptionRequest,
    ) -> Result<PageAndSubscription, TaskServiceError> {
        validate_subscription_request(request)?;
        let root = self.root.as_ref();
        root.validate_binding()?;
        let _boundary = self.local_boundary.lock().expect("repository mutex");
        let lock = persist::open_lock(root, false)?;
        if let Some(lock) = lock.as_ref() {
            FileExt::lock_exclusive(lock).map_err(|error| TaskServiceError::Io {
                stage: super::model::IoStage::Lock,
                kind: error.kind(),
            })?;
        }
        let (records, effects) = if lock.is_some() {
            let state = self.load_locked_read_only(root).map_err(|error| {
                self.mark_load_error(&error);
                error
            })?;
            (state.records, state.effects)
        } else {
            let _ = load_without_lock(&root)?;
            (Vec::new(), LoadEffects::default())
        };
        let checkpoint = records
            .last()
            .map(JournalRecord::cursor)
            .unwrap_or_else(JournalCursor::genesis);
        self.mark_load_effects(&effects, &checkpoint);
        let page = journal::page_records(&records, &request.page)?;
        let subscription = if page.reached_head {
            Some(self.watchers.install(
                page.continuation.clone(),
                request.page.task_id.clone(),
                request.capacity as usize,
            ))
        } else {
            None
        };
        // Keep the OS file lock alive through the caught-up proof and watcher
        // installation; local writers cannot overtake this exact handoff.
        drop(lock);
        Ok(PageAndSubscription { page, subscription })
    }

    fn load_locked_recovering(&self, root: &RootHandle) -> Result<LockedState, TaskServiceError> {
        let loaded = persist::load_snapshot(root)?.unwrap_or_else(empty_store);
        let journal = persist::read_journal(root)?;
        let journal_present = journal.is_some();
        let journal_bytes = journal.unwrap_or_default();
        let parsed = journal::parse_journal(&journal_bytes)?;
        validate_loaded_prefix(&loaded, &parsed.records)?;
        let recovery = persist::load_recovery(root)?;
        if recovery.is_some() && !journal_present {
            return Err(TaskServiceError::InvalidRecoveryIntent {
                code: super::model::ValidationCode::InvalidRecoveryIntent,
            });
        }
        let recovery_needed = recovery.is_some() || !parsed.suffix.is_empty();
        let recovered = match journal::recover_journal(root, &journal_bytes, recovery, &self.faults)
        {
            Ok(recovered) => recovered,
            Err(error) => {
                if recovery_needed {
                    self.watchers.mark_all(ResyncReason::RecoveryStopped);
                }
                return Err(error);
            }
        };
        let final_store = reconstruct_store(&recovered.records)?;
        let replayed = final_store != loaded;
        if replayed {
            let bytes = serde_json::to_vec(&final_store)
                .expect("TaskStore contains no fallible serde representation");
            if recovered.recovery_applied {
                persist::persist_recovery_snapshot(root, &bytes, &self.faults)?;
            } else {
                persist::persist_replayed_snapshot(root, &bytes)?;
            }
        }
        if recovered.cleanup_intent {
            persist::remove_recovery_intent(root, &self.faults)?;
        }
        Ok(LockedState {
            store: final_store,
            records: recovered.records,
            effects: LoadEffects {
                replayed,
                recovered: recovered.recovery_applied,
            },
        })
    }

    fn load_locked_read_only(&self, root: &RootHandle) -> Result<LockedState, TaskServiceError> {
        let loaded = persist::load_snapshot(root)?.unwrap_or_else(empty_store);
        let journal = persist::read_journal(root)?;
        let journal_present = journal.is_some();
        let journal_bytes = journal.unwrap_or_default();
        let parsed = journal::parse_journal(&journal_bytes)?;
        validate_loaded_prefix(&loaded, &parsed.records)?;
        let recovery = persist::load_recovery(root)?;
        if recovery.is_some() && !journal_present {
            return Err(TaskServiceError::InvalidRecoveryIntent {
                code: super::model::ValidationCode::InvalidRecoveryIntent,
            });
        }
        if recovery.is_some() || !parsed.suffix.is_empty() {
            return Err(TaskServiceError::RecoveryRequired);
        }
        let final_store = reconstruct_store(&parsed.records)?;
        let replayed = final_store != loaded;
        Ok(LockedState {
            store: final_store,
            records: parsed.records,
            effects: LoadEffects {
                replayed,
                recovered: false,
            },
        })
    }

    fn mark_load_effects(&self, effects: &LoadEffects, checkpoint: &JournalCursor) {
        let externally_changed = {
            let mut observed = self
                .observed_checkpoint
                .lock()
                .expect("observed checkpoint mutex");
            let changed = *observed != *checkpoint;
            *observed = checkpoint.clone();
            changed
        };
        if effects.recovered {
            self.watchers.mark_all(ResyncReason::RecoveryApplied);
        } else if effects.replayed || externally_changed {
            self.watchers.mark_all(ResyncReason::RepositoryReloaded);
        }
    }

    fn observe_committed(&self, checkpoint: &JournalCursor) {
        *self
            .observed_checkpoint
            .lock()
            .expect("observed checkpoint mutex") = checkpoint.clone();
    }

    fn mark_load_error(&self, error: &TaskServiceError) {
        let reason = if matches!(
            error,
            TaskServiceError::RecoveryStopped { .. }
                | TaskServiceError::InvalidRecoveryIntent { .. }
        ) {
            ResyncReason::RecoveryStopped
        } else {
            ResyncReason::RepositoryReloaded
        };
        self.watchers.mark_all(reason);
    }

    #[cfg(test)]
    pub(super) fn with_test_fault(
        root: impl Into<PathBuf>,
        point: FaultPoint,
    ) -> Result<Self, TaskServiceError> {
        let root = root.into();
        let root = Arc::new(persist::validate_root(&root)?);
        Ok(Self {
            root,
            local_boundary: Arc::new(Mutex::new(())),
            watchers: Arc::new(WatchHub::default()),
            faults: Arc::new(FaultController::new(point)),
            observed_checkpoint: Arc::new(Mutex::new(JournalCursor::genesis())),
        })
    }
}

impl EventSubscription {
    pub fn cursor(&self) -> JournalCursor {
        self.state
            .inner
            .lock()
            .expect("subscription mutex")
            .cursor
            .clone()
    }

    pub fn try_recv(&self) -> Result<Option<WatchItem>, WatchReceiveError> {
        let mut inner = self.state.inner.lock().expect("subscription mutex");
        receive_ready(&mut inner)
    }

    pub fn recv(&self) -> Result<WatchItem, WatchReceiveError> {
        let mut inner = self.state.inner.lock().expect("subscription mutex");
        loop {
            if let Some(item) = receive_ready(&mut inner)? {
                return Ok(item);
            }
            inner = self.state.ready.wait(inner).expect("subscription mutex");
        }
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<Option<WatchItem>, WatchReceiveError> {
        let inner = self.state.inner.lock().expect("subscription mutex");
        let (mut inner, _) = self
            .state
            .ready
            .wait_timeout_while(inner, timeout, |inner| {
                inner.queue.is_empty() && inner.terminal.is_none()
            })
            .expect("subscription mutex");
        receive_ready(&mut inner)
    }
}

impl Drop for EventSubscription {
    fn drop(&mut self) {
        let mut inner = self.state.inner.lock().expect("subscription mutex");
        inner.alive = false;
        inner
            .terminal
            .get_or_insert(ResyncReason::ReceiverDisconnected);
        self.state.ready.notify_all();
    }
}

impl WatchHub {
    fn install(
        &self,
        cursor: JournalCursor,
        task_id: Option<TaskId>,
        capacity: usize,
    ) -> EventSubscription {
        let state = Arc::new(SubscriptionState {
            inner: Mutex::new(SubscriptionInner {
                cursor,
                task_id,
                capacity,
                queue: VecDeque::new(),
                terminal: None,
                terminal_delivered: false,
                alive: true,
            }),
            ready: Condvar::new(),
        });
        self.subscribers
            .lock()
            .expect("watch hub mutex")
            .push(Arc::downgrade(&state));
        EventSubscription { state }
    }

    fn broadcast(&self, record: &JournalRecord) {
        let mut subscribers = self.subscribers.lock().expect("watch hub mutex");
        subscribers.retain(|weak| {
            let Some(state) = weak.upgrade() else {
                return false;
            };
            let mut inner = state.inner.lock().expect("subscription mutex");
            if !inner.alive {
                return false;
            }
            if inner.terminal.is_none() && journal::deliverable(record, inner.task_id.as_ref()) {
                if inner.queue.len() < inner.capacity {
                    inner.queue.push_back(record.clone());
                } else {
                    inner.terminal = Some(ResyncReason::ReceiverFull);
                }
                state.ready.notify_all();
            }
            true
        });
    }

    fn mark_all(&self, reason: ResyncReason) {
        let mut subscribers = self.subscribers.lock().expect("watch hub mutex");
        subscribers.retain(|weak| {
            let Some(state) = weak.upgrade() else {
                return false;
            };
            let mut inner = state.inner.lock().expect("subscription mutex");
            if !inner.alive {
                return false;
            }
            inner.terminal.get_or_insert(reason);
            state.ready.notify_all();
            true
        });
    }
}

fn receive_ready(inner: &mut SubscriptionInner) -> Result<Option<WatchItem>, WatchReceiveError> {
    if let Some(record) = inner.queue.pop_front() {
        inner.cursor = record.cursor();
        return Ok(Some(WatchItem::Event(record)));
    }
    if let Some(reason) = inner.terminal {
        if inner.terminal_delivered {
            return Err(WatchReceiveError::Disconnected);
        }
        inner.terminal_delivered = true;
        return Ok(Some(WatchItem::ResyncRequired { reason }));
    }
    Ok(None)
}

fn load_without_lock(root: &RootHandle) -> Result<TaskStore, TaskServiceError> {
    let snapshot = persist::load_snapshot(root)?;
    let journal = persist::read_journal(root)?;
    let recovery = persist::load_recovery(root)?;
    if snapshot.is_none() && journal.is_none() && recovery.is_none() {
        return Ok(empty_store());
    }
    Err(TaskServiceError::InvalidStore {
        code: super::model::ValidationCode::InvalidStoreRevision,
    })
}

fn validate_loaded_prefix(
    loaded: &TaskStore,
    records: &[JournalRecord],
) -> Result<(), TaskServiceError> {
    if loaded.journal_checkpoint.sequence > records.len() as u64 {
        return Err(TaskServiceError::SnapshotAheadOfJournal);
    }
    let prefix_length = loaded.journal_checkpoint.sequence as usize;
    let at_checkpoint = reconstruct_store(&records[..prefix_length])?;
    if loaded != &at_checkpoint {
        return Err(TaskServiceError::InvalidStore {
            code: super::model::ValidationCode::InvalidStoreRevision,
        });
    }
    Ok(())
}

fn preflight_empty_transition(
    empty: &TaskStore,
    envelope: &TransitionEnvelope,
) -> Result<(), TaskServiceError> {
    if empty.store_revision != envelope.expected_store_revision {
        return Err(TaskServiceError::StoreRevisionConflict {
            expected: envelope.expected_store_revision,
            actual: empty.store_revision,
        });
    }
    let next_revision = empty
        .store_revision
        .checked_next()
        .map_err(|_| TaskServiceError::StoreRevisionOverflow)?;
    let planned = plan_transition(empty, envelope)?;
    let mut planned_store = empty.clone();
    planned_store.store_revision = next_revision;
    planned_store.tasks = planned.tasks;
    validate_store(&planned_store).map_err(|code| TaskServiceError::InvalidStore { code })
}

fn finalize_transition(
    current: &TaskStore,
    planned: &PlannedTransition,
    envelope: &TransitionEnvelope,
    digest: &super::model::Sha256,
    next_revision: StoreRevision,
    committed_at: Rfc3339,
) -> (TransitionResponse, JournalRecord, TaskStore) {
    let response = TransitionResponse {
        schema: super::model::ResponseSchema::V1,
        receipt_id: envelope.receipt_id.clone(),
        committed_store_revision: next_revision,
        task_id: planned.task_id.clone(),
        task_revision: planned.task_revision,
        attempt_number: planned.attempt_number,
        prior_phase: planned.prior_phase,
        resulting_phase: planned.resulting_phase,
        committed_at,
    };
    let record = make_record(
        current.journal_checkpoint.sequence + 1,
        current.journal_checkpoint.event_sha256.clone(),
        next_revision,
        JournalEvent::Transition(TransitionEvent {
            envelope: envelope.clone(),
            response: response.clone(),
        }),
    )
    .expect("validated transition records serialize");
    let mut next = current.clone();
    next.store_revision = next_revision;
    next.tasks = planned.tasks.clone();
    next.journal_checkpoint = record.cursor();
    next.receipts.insert(
        envelope.receipt_id.clone(),
        ReceiptRecord {
            schema: ReceiptSchema::V1,
            receipt_id: envelope.receipt_id.clone(),
            request_digest_sha256: digest.clone(),
            response: response.clone(),
            event_cursor: record.cursor(),
        },
    );
    (response, record, next)
}

fn reconstruct_store(records: &[JournalRecord]) -> Result<TaskStore, TaskServiceError> {
    let mut store = empty_store();
    for record in records {
        match &record.event {
            JournalEvent::Transition(event) => {
                if store.receipts.contains_key(&event.envelope.receipt_id)
                    || event.envelope.expected_store_revision != store.store_revision
                {
                    return Err(TaskServiceError::InvalidJournal {
                        code: super::model::ValidationCode::InvalidTransitionEvent,
                    });
                }
                let expected_revision = store
                    .store_revision
                    .checked_next()
                    .map_err(|_| TaskServiceError::StoreRevisionOverflow)?;
                if expected_revision != record.store_revision
                    || event.response.committed_store_revision != record.store_revision
                {
                    return Err(TaskServiceError::InvalidJournal {
                        code: super::model::ValidationCode::InvalidStoreRevision,
                    });
                }
                let planned = plan_transition(&store, &event.envelope)?;
                if !response_matches_plan(&event.response, &planned) {
                    return Err(TaskServiceError::InvalidJournal {
                        code: super::model::ValidationCode::InvalidTransitionEvent,
                    });
                }
                store.tasks = planned.tasks;
                store.store_revision = record.store_revision;
                store.journal_checkpoint = record.cursor();
                store.receipts.insert(
                    event.envelope.receipt_id.clone(),
                    ReceiptRecord {
                        schema: ReceiptSchema::V1,
                        receipt_id: event.envelope.receipt_id.clone(),
                        request_digest_sha256: event.envelope.request_digest_sha256.clone(),
                        response: event.response.clone(),
                        event_cursor: record.cursor(),
                    },
                );
            }
            JournalEvent::SystemJournalTailRecovered(_) => {
                if record.store_revision != store.store_revision {
                    return Err(TaskServiceError::InvalidJournal {
                        code: super::model::ValidationCode::InvalidStoreRevision,
                    });
                }
                store.journal_checkpoint = record.cursor();
            }
        }
        validate_store(&store).map_err(|code| TaskServiceError::InvalidStore { code })?;
    }
    Ok(store)
}

fn plan_transition(
    current: &TaskStore,
    envelope: &TransitionEnvelope,
) -> Result<PlannedTransition, TaskServiceError> {
    let mut tasks = current.tasks.clone();
    match &envelope.command {
        TaskCommand::CreateDraft(command) => {
            require_no_fence(envelope)?;
            let specification = &command.specification;
            let revisions = tasks.entry(specification.task_id.clone()).or_default();
            if revisions.contains_key(&specification.task_revision) {
                return Err(TaskServiceError::RevisionConflict);
            }
            if let Some((latest_revision, latest)) = revisions.iter().next_back() {
                if !latest.phase.is_terminal() {
                    return Err(TaskServiceError::ActiveRevisionExists);
                }
                if specification.task_revision <= *latest_revision {
                    return Err(TaskServiceError::RevisionNotIncreasing);
                }
            }
            revisions.insert(
                specification.task_revision,
                TaskRecord {
                    specification: specification.clone(),
                    phase: TaskPhase::Draft,
                    attempt: None,
                },
            );
            Ok(PlannedTransition {
                tasks,
                task_id: specification.task_id.clone(),
                task_revision: specification.task_revision,
                attempt_number: None,
                prior_phase: None,
                resulting_phase: TaskPhase::Draft,
            })
        }
        TaskCommand::Publish(command) => {
            require_no_fence(envelope)?;
            if tasks
                .values()
                .flat_map(|revisions| revisions.values())
                .any(|record| {
                    record
                        .attempt
                        .as_ref()
                        .is_some_and(|attempt| attempt.attempt_token == command.attempt_token)
                })
            {
                return Err(TaskServiceError::StaleFence);
            }
            let record = task_record_mut(&mut tasks, &command.task_id, command.task_revision)?;
            require_phase(record.phase, TaskPhase::Draft)?;
            record.phase = TaskPhase::Published;
            record.attempt = Some(TaskAttempt {
                attempt_number: AttemptNumber::new(1).expect("attempt one is valid"),
                attempt_token: command.attempt_token.clone(),
                owner_session_id: command.owner_session_id.clone(),
                owner_durable_revision: command.owner_durable_revision,
                runtime_generation: command.runtime_generation,
                runtime_agent_id: command.runtime_agent_id.clone(),
                publication_receipt_id: envelope.receipt_id.clone(),
                delivery_receipt_id: None,
                acceptance_receipt_id: None,
                start_receipt_id: None,
                result_receipt_id: None,
            });
            Ok(PlannedTransition {
                tasks,
                task_id: command.task_id.clone(),
                task_revision: command.task_revision,
                attempt_number: Some(AttemptNumber::new(1).expect("attempt one is valid")),
                prior_phase: Some(TaskPhase::Draft),
                resulting_phase: TaskPhase::Published,
            })
        }
        TaskCommand::CancelDraft(command) => {
            require_no_fence(envelope)?;
            let record = task_record_mut(&mut tasks, &command.task_id, command.task_revision)?;
            require_phase(record.phase, TaskPhase::Draft)?;
            record.phase = TaskPhase::Cancelled;
            Ok(PlannedTransition {
                tasks,
                task_id: command.task_id.clone(),
                task_revision: command.task_revision,
                attempt_number: None,
                prior_phase: Some(TaskPhase::Draft),
                resulting_phase: TaskPhase::Cancelled,
            })
        }
        command => plan_attempt_transition(tasks, envelope, command),
    }
}

fn plan_attempt_transition(
    mut tasks: BTreeMap<TaskId, BTreeMap<TaskRevision, TaskRecord>>,
    envelope: &TransitionEnvelope,
    command: &TaskCommand,
) -> Result<PlannedTransition, TaskServiceError> {
    let fence = envelope
        .fence
        .as_ref()
        .ok_or(TaskServiceError::FenceRequired)?;
    let record = tasks
        .get_mut(&fence.task_id)
        .and_then(|revisions| revisions.get_mut(&fence.task_revision))
        .ok_or(TaskServiceError::StaleFence)?;
    let attempt = record
        .attempt
        .as_mut()
        .ok_or(TaskServiceError::AttemptNotFound)?;
    if !fence_matches(fence, attempt) {
        return Err(TaskServiceError::StaleFence);
    }
    let prior = record.phase;
    let (expected, resulting) = command_edge(command);
    require_phase(prior, expected)?;
    match command {
        TaskCommand::RecordDelivery(command) => {
            attempt.delivery_receipt_id = Some(command.external_delivery_receipt_id.clone());
        }
        TaskCommand::Accept(_) => {
            attempt.acceptance_receipt_id = Some(envelope.receipt_id.clone());
        }
        TaskCommand::Start(_) => {
            attempt.start_receipt_id = Some(envelope.receipt_id.clone());
        }
        TaskCommand::CompleteRunning(_)
        | TaskCommand::FailRunning(_)
        | TaskCommand::CancelRunning(_)
        | TaskCommand::CompleteReview(_)
        | TaskCommand::FailReview(_)
        | TaskCommand::CancelReview(_) => {
            attempt.result_receipt_id = Some(envelope.receipt_id.clone());
        }
        _ => {}
    }
    record.phase = resulting;
    Ok(PlannedTransition {
        tasks,
        task_id: fence.task_id.clone(),
        task_revision: fence.task_revision,
        attempt_number: Some(fence.attempt_number),
        prior_phase: Some(prior),
        resulting_phase: resulting,
    })
}

fn command_edge(command: &TaskCommand) -> (TaskPhase, TaskPhase) {
    use TaskCommand::*;
    match command {
        RecordDelivery(_) => (TaskPhase::Published, TaskPhase::Delivered),
        CancelPublished(_) => (TaskPhase::Published, TaskPhase::Cancelled),
        Accept(_) => (TaskPhase::Delivered, TaskPhase::Accepted),
        Reject(_) => (TaskPhase::Delivered, TaskPhase::Rejected),
        CancelDelivered(_) => (TaskPhase::Delivered, TaskPhase::Cancelled),
        Start(_) => (TaskPhase::Accepted, TaskPhase::Running),
        CancelAccepted(_) => (TaskPhase::Accepted, TaskPhase::Cancelled),
        EnterWaiting(_) => (TaskPhase::Running, TaskPhase::Waiting),
        EnterBlocked(_) => (TaskPhase::Running, TaskPhase::Blocked),
        MarkReviewReady(_) => (TaskPhase::Running, TaskPhase::ReviewReady),
        CompleteRunning(_) => (TaskPhase::Running, TaskPhase::Completed),
        FailRunning(_) => (TaskPhase::Running, TaskPhase::Failed),
        CancelRunning(_) => (TaskPhase::Running, TaskPhase::Cancelled),
        ResumeWaiting(_) => (TaskPhase::Waiting, TaskPhase::Running),
        BlockWaiting(_) => (TaskPhase::Waiting, TaskPhase::Blocked),
        CancelWaiting(_) => (TaskPhase::Waiting, TaskPhase::Cancelled),
        ResumeBlocked(_) => (TaskPhase::Blocked, TaskPhase::Running),
        CancelBlocked(_) => (TaskPhase::Blocked, TaskPhase::Cancelled),
        CompleteReview(_) => (TaskPhase::ReviewReady, TaskPhase::Completed),
        FailReview(_) => (TaskPhase::ReviewReady, TaskPhase::Failed),
        CancelReview(_) => (TaskPhase::ReviewReady, TaskPhase::Cancelled),
        CreateDraft(_) | Publish(_) | CancelDraft(_) => {
            unreachable!("attempt planner only receives attempted commands")
        }
    }
}

fn response_matches_plan(response: &TransitionResponse, planned: &PlannedTransition) -> bool {
    response.task_id == planned.task_id
        && response.task_revision == planned.task_revision
        && response.attempt_number == planned.attempt_number
        && response.prior_phase == planned.prior_phase
        && response.resulting_phase == planned.resulting_phase
}

fn task_record_mut<'a>(
    tasks: &'a mut BTreeMap<TaskId, BTreeMap<TaskRevision, TaskRecord>>,
    task_id: &TaskId,
    task_revision: TaskRevision,
) -> Result<&'a mut TaskRecord, TaskServiceError> {
    tasks
        .get_mut(task_id)
        .and_then(|revisions| revisions.get_mut(&task_revision))
        .ok_or(TaskServiceError::TaskNotFound)
}

fn require_no_fence(envelope: &TransitionEnvelope) -> Result<(), TaskServiceError> {
    if envelope.fence.is_some() {
        Err(TaskServiceError::FenceNotAllowed)
    } else {
        Ok(())
    }
}

fn require_phase(actual: TaskPhase, expected: TaskPhase) -> Result<(), TaskServiceError> {
    if actual == expected {
        Ok(())
    } else {
        Err(TaskServiceError::IllegalPhase { actual })
    }
}

fn fence_matches(fence: &AttemptFence, attempt: &TaskAttempt) -> bool {
    fence.attempt_number == attempt.attempt_number
        && fence.attempt_token == attempt.attempt_token
        && fence.owner_session_id == attempt.owner_session_id
        && fence.runtime_generation == attempt.runtime_generation
}
