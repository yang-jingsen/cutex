use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Mutex;
use std::sync::OnceLock;

use anyhow::Context;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;

use crate::config::atomic::write_private_pretty_json_atomic;
use crate::config::paths::runtime_dir;
use crate::platform::host::current_host_name;

use super::model::EventCheckpoint;
use super::model::EventEnvelope;
use super::model::EventPage;
use super::model::EventPageScope;
use super::model::EventStreamBoundary;
use super::model::EventStreamMetadata;
use super::model::PendingEvent;
use super::model::CONTRACT_VERSION;
use super::model::MAX_SAFE_SEQUENCE;

const STATE_FILE_NAME: &str = "stream-state.json";
const EVENTS_FILE_NAME: &str = "events.jsonl";
const LOCK_FILE_NAME: &str = "repository.lock";
const ORPHANED_DIR_NAME: &str = "orphaned";
const PENDING_RESET_FILE_NAME: &str = "pending-stream-reset.json";

static MANAGEMENT_V2_REPOSITORY: OnceLock<EventRepository> = OnceLock::new();

pub fn management_v2_repository() -> anyhow::Result<&'static EventRepository> {
    if let Some(repository) = MANAGEMENT_V2_REPOSITORY.get() {
        return Ok(repository);
    }
    let repository =
        EventRepository::open(runtime_dir()?.join("management-v2"), current_host_name())?;
    let _ = MANAGEMENT_V2_REPOSITORY.set(repository);
    MANAGEMENT_V2_REPOSITORY
        .get()
        .context("management v2 repository initialization raced without a winner")
}

#[derive(Debug, Clone, Copy)]
pub struct RepositoryOptions {
    pub rotate_bytes: u64,
    pub retained_files: usize,
}

impl Default for RepositoryOptions {
    fn default() -> Self {
        Self {
            rotate_bytes: 32 * 1024 * 1024,
            retained_files: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct StreamState {
    version: u8,
    stream_id: String,
    next_sequence: u64,
}

impl StreamState {
    fn new() -> Self {
        Self {
            version: 1,
            stream_id: Uuid::new_v4().to_string(),
            next_sequence: 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StreamRecoveryReset {
    pub previous_stream_id: Option<String>,
    pub previous_last_sequence: Option<u64>,
    pub stream_id: String,
    pub reason: String,
    pub detected_at: String,
    pub diagnostic: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct PendingStreamReset {
    reset: StreamRecoveryReset,
    #[serde(default)]
    emitted_session_ids: HashSet<String>,
}

#[derive(Debug, Clone)]
pub struct ReplayQuery {
    pub stream_id: Option<String>,
    pub after: Option<String>,
    pub limit: usize,
    pub cutex_session_id: Option<String>,
}

impl Default for ReplayQuery {
    fn default() -> Self {
        Self {
            stream_id: None,
            after: None,
            limit: 100,
            cutex_session_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedBoundary {
    pub sequence: u64,
    pub cursor: String,
}

#[derive(Debug)]
pub enum ReplayError {
    InvalidQuery(String),
    ConflictingCursor,
    StreamChanged {
        requested_stream_id: String,
        current_stream_id: String,
    },
    CursorExpired {
        stream_id: String,
        cursor: String,
        earliest: Option<RetainedBoundary>,
        latest: Option<RetainedBoundary>,
    },
    Repository(anyhow::Error),
}

impl fmt::Display for ReplayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidQuery(message) => formatter.write_str(message),
            Self::ConflictingCursor => {
                formatter.write_str("after and Last-Event-ID identify different cursors")
            }
            Self::StreamChanged {
                requested_stream_id,
                current_stream_id,
            } => write!(
                formatter,
                "requested stream {requested_stream_id} differs from current stream {current_stream_id}"
            ),
            Self::CursorExpired { cursor, .. } => {
                write!(formatter, "cursor is no longer retained: {cursor}")
            }
            Self::Repository(error) => write!(formatter, "{error:#}"),
        }
    }
}

impl std::error::Error for ReplayError {}

impl From<anyhow::Error> for ReplayError {
    fn from(error: anyhow::Error) -> Self {
        Self::Repository(error)
    }
}

pub struct EventSubscription {
    pub page: EventPage,
    pub receiver: mpsc::Receiver<EventEnvelope>,
}

pub struct EventRepository {
    root: PathBuf,
    host_id: String,
    options: RepositoryOptions,
    process_lock: Mutex<()>,
    subscribers: Mutex<Vec<mpsc::SyncSender<EventEnvelope>>>,
    recovery_reset: Mutex<Option<PendingStreamReset>>,
    replay_index: Mutex<ReplayIndex>,
}

struct RepositorySnapshot {
    state: StreamState,
    earliest: Option<EventStreamBoundary>,
    latest: Option<EventStreamBoundary>,
}

#[derive(Default)]
struct ReplayIndex {
    stream_id: Option<String>,
    files: Vec<JournalFileStamp>,
    cursors: HashMap<String, ReplayLocation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct JournalFileStamp {
    length: u64,
    first: Option<RetainedBoundary>,
    last: Option<RetainedBoundary>,
}

#[derive(Debug, Clone, Copy)]
struct ReplayLocation {
    file_index: usize,
    next_offset: u64,
    sequence: u64,
}

struct JournalScan {
    stream_id: Option<String>,
    last: Option<RetainedBoundary>,
    records: Vec<EventEnvelope>,
}

impl EventRepository {
    pub fn open(root: impl Into<PathBuf>, host_id: impl Into<String>) -> anyhow::Result<Self> {
        Self::open_with_options(root, host_id, RepositoryOptions::default())
    }

    pub fn open_with_options(
        root: impl Into<PathBuf>,
        host_id: impl Into<String>,
        options: RepositoryOptions,
    ) -> anyhow::Result<Self> {
        if options.rotate_bytes == 0 {
            anyhow::bail!("management v2 rotate_bytes must be positive");
        }
        if options.retained_files == 0 {
            anyhow::bail!("management v2 retained_files must be positive");
        }
        let host_id = host_id.into();
        if host_id.is_empty() {
            anyhow::bail!("management v2 repository hostId must not be empty");
        }
        let repository = Self {
            root: root.into(),
            host_id,
            options,
            process_lock: Mutex::new(()),
            subscribers: Mutex::new(Vec::new()),
            recovery_reset: Mutex::new(None),
            replay_index: Mutex::new(ReplayIndex::default()),
        };
        repository.prepare_root()?;
        repository.with_repository_lock(|repository| {
            let (state, reset) = repository.load_or_recover_state_locked()?;
            if reset.is_some() {
                repository.record_recovery_reset(reset)?;
            } else {
                repository.restore_pending_recovery_reset_locked(&state)?;
            }
            Ok(())
        })?;
        Ok(repository)
    }

    pub fn append(&self, pending: PendingEvent) -> anyhow::Result<EventEnvelope> {
        pending.validate()?;
        if pending.host_id != self.host_id {
            anyhow::bail!(
                "event hostId {} differs from repository hostId {}",
                pending.host_id,
                self.host_id
            );
        }
        let (reset_envelope, envelope) = self.with_repository_lock(|repository| {
            let (mut state, reset) = repository.load_append_state_locked()?;
            repository.record_recovery_reset(reset)?;
            let reset_envelope = repository
                .append_reset_for_session_locked(&mut state, &pending.cutex_session_id)?;
            let envelope = repository.append_pending_locked(&mut state, pending)?;
            Ok((reset_envelope, envelope))
        })?;
        if let Some(reset_envelope) = reset_envelope {
            self.publish(&reset_envelope);
        }
        self.publish(&envelope);
        Ok(envelope)
    }

    pub fn materialize_recovery_reset(
        &self,
        cutex_session_ids: &[String],
        complete: bool,
    ) -> anyhow::Result<Vec<EventEnvelope>> {
        if !self.has_pending_recovery_reset()? {
            return Ok(Vec::new());
        }
        let mut session_ids = cutex_session_ids
            .iter()
            .filter(|session_id| !session_id.is_empty())
            .cloned()
            .collect::<Vec<_>>();
        session_ids.sort();
        session_ids.dedup();
        let envelopes = self.with_repository_lock(|repository| {
            let (mut state, reset) = repository.load_or_recover_state_locked()?;
            repository.record_recovery_reset(reset)?;
            repository.reconcile_emitted_reset_sessions_locked()?;
            let mut envelopes = Vec::new();
            for session_id in session_ids {
                if let Some(envelope) =
                    repository.append_reset_for_session_locked(&mut state, &session_id)?
                {
                    envelopes.push(envelope);
                }
            }
            if complete {
                repository.clear_pending_recovery_reset_locked()?;
            }
            Ok(envelopes)
        })?;
        for envelope in &envelopes {
            self.publish(envelope);
        }
        Ok(envelopes)
    }

    pub fn checkpoint(&self) -> anyhow::Result<EventCheckpoint> {
        self.with_repository_lock(|repository| {
            let (snapshot, reset) = repository.load_snapshot_locked()?;
            repository.record_recovery_reset(reset)?;
            Ok(checkpoint_for_snapshot(&snapshot))
        })
    }

    pub fn stream_metadata(&self) -> anyhow::Result<EventStreamMetadata> {
        self.with_repository_lock(|repository| {
            let (snapshot, reset) = repository.load_snapshot_locked()?;
            repository.record_recovery_reset(reset)?;
            Ok(EventStreamMetadata {
                stream_id: snapshot.state.stream_id,
                earliest: snapshot.earliest,
                latest: snapshot.latest,
            })
        })
    }

    pub fn page(&self, query: ReplayQuery) -> Result<EventPage, ReplayError> {
        self.with_replay_lock(|repository| repository.page_locked(query))
    }

    pub fn page_and_subscribe(
        &self,
        query: ReplayQuery,
        capacity: usize,
    ) -> Result<EventSubscription, ReplayError> {
        if capacity == 0 {
            return Err(ReplayError::InvalidQuery(
                "subscriber capacity must be positive".to_string(),
            ));
        }
        self.with_replay_lock(|repository| {
            let (sender, receiver) = mpsc::sync_channel(capacity);
            repository
                .subscribers
                .lock()
                .map_err(|_| {
                    ReplayError::Repository(anyhow::anyhow!(
                        "management v2 subscriber lock was poisoned"
                    ))
                })?
                .push(sender);
            match repository.page_locked(query) {
                Ok(page) => Ok(EventSubscription { page, receiver }),
                Err(error) => {
                    repository
                        .subscribers
                        .lock()
                        .map_err(|_| {
                            ReplayError::Repository(anyhow::anyhow!(
                                "management v2 subscriber lock was poisoned"
                            ))
                        })?
                        .pop();
                    Err(error)
                }
            }
        })
    }

    pub fn recovery_reset(&self) -> anyhow::Result<Option<StreamRecoveryReset>> {
        Ok(self
            .recovery_reset
            .lock()
            .map_err(|_| anyhow::anyhow!("management v2 recovery-reset lock was poisoned"))?
            .as_ref()
            .map(|pending| pending.reset.clone()))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn has_pending_recovery_reset(&self) -> anyhow::Result<bool> {
        Ok(self
            .recovery_reset
            .lock()
            .map_err(|_| anyhow::anyhow!("management v2 recovery-reset lock was poisoned"))?
            .is_some())
    }

    fn page_locked(&self, query: ReplayQuery) -> Result<EventPage, ReplayError> {
        if !(1..=1000).contains(&query.limit) {
            return Err(ReplayError::InvalidQuery(
                "event page limit must be between 1 and 1000".to_string(),
            ));
        }
        let initial_cursor = query
            .after
            .as_deref()
            .is_none_or(|value| value.is_empty() || value == "0");
        if !initial_cursor && query.stream_id.is_none() {
            return Err(ReplayError::InvalidQuery(
                "streamId is required with a nonzero after cursor".to_string(),
            ));
        }
        match self.page_locked_once(&query, initial_cursor) {
            Err(ReplayError::Repository(_)) => {
                let (_, reset) = self
                    .load_or_recover_state_locked()
                    .map_err(ReplayError::from)?;
                self.record_recovery_reset(reset)
                    .map_err(ReplayError::from)?;
                self.page_locked_once(&query, initial_cursor)
            }
            result => result,
        }
    }

    fn page_locked_once(
        &self,
        query: &ReplayQuery,
        initial_cursor: bool,
    ) -> Result<EventPage, ReplayError> {
        let (snapshot, reset) = self.load_snapshot_locked().map_err(ReplayError::from)?;
        self.record_recovery_reset(reset)
            .map_err(ReplayError::from)?;
        if let Some(requested) = query.stream_id.as_deref() {
            if requested != snapshot.state.stream_id {
                return Err(ReplayError::StreamChanged {
                    requested_stream_id: requested.to_string(),
                    current_stream_id: snapshot.state.stream_id,
                });
            }
        }
        let stream_id = snapshot.state.stream_id.clone();
        self.ensure_replay_index_locked(&stream_id)
            .map_err(ReplayError::from)?;
        let start = {
            let index = self.replay_index.lock().map_err(|_| {
                ReplayError::Repository(anyhow::anyhow!(
                    "management v2 replay-index lock was poisoned"
                ))
            })?;
            if initial_cursor {
                index
                    .files
                    .iter()
                    .position(|file| file.first.is_some())
                    .map(|file_index| {
                        let sequence = index.files[file_index]
                            .first
                            .as_ref()
                            .expect("selected nonempty replay-index file")
                            .sequence;
                        ReplayLocation {
                            file_index,
                            next_offset: 0,
                            sequence: sequence.saturating_sub(1),
                        }
                    })
            } else {
                let cursor = query.after.as_deref().unwrap_or_default();
                Some(
                    *index
                        .cursors
                        .get(cursor)
                        .ok_or_else(|| ReplayError::CursorExpired {
                            stream_id: stream_id.clone(),
                            cursor: cursor.to_string(),
                            earliest: snapshot.earliest.as_ref().map(retained_boundary),
                            latest: snapshot.latest.as_ref().map(retained_boundary),
                        })?,
                )
            }
        };
        if let (false, Some(start)) = (initial_cursor, start) {
            self.validate_indexed_cursor_locked(
                query.after.as_deref().unwrap_or_default(),
                start,
                &stream_id,
            )
            .map_err(ReplayError::from)?;
        }
        let mut scanned = match start {
            Some(start) => self
                .read_indexed_records_locked(start, query.limit.saturating_add(1), &stream_id)
                .map_err(ReplayError::from)?,
            None => Vec::new(),
        };
        let has_more = scanned.len() > query.limit;
        if has_more {
            scanned.truncate(query.limit);
        }
        let next_cursor = scanned
            .last()
            .map(|event| event.cursor.clone())
            .or_else(|| (!initial_cursor).then(|| query.after.clone()).flatten());
        let scanned_count = scanned.len();
        let events = scanned
            .into_iter()
            .filter(|event| {
                query
                    .cutex_session_id
                    .as_deref()
                    .is_none_or(|session_id| event.cutex_session_id == session_id)
            })
            .collect();
        Ok(EventPage {
            contract_version: CONTRACT_VERSION,
            host_id: self.host_id.clone(),
            stream_id,
            scope: EventPageScope {
                cutex_session_id: query.cutex_session_id.clone(),
            },
            events,
            next_cursor,
            checkpoint: checkpoint_for_snapshot(&snapshot),
            scanned_count,
            has_more,
        })
    }

    fn append_pending_locked(
        &self,
        state: &mut StreamState,
        pending: PendingEvent,
    ) -> anyhow::Result<EventEnvelope> {
        if state.next_sequence > MAX_SAFE_SEQUENCE {
            anyhow::bail!("management v2 event sequence exhausted the JSON-safe range");
        }
        let envelope = EventEnvelope {
            contract_version: CONTRACT_VERSION,
            event_id: Uuid::new_v4().to_string(),
            cursor: format!("c2:{}", Uuid::new_v4()),
            stream_id: state.stream_id.clone(),
            sequence: state.next_sequence,
            received_at: Utc::now().to_rfc3339(),
            cutex_session_id: pending.cutex_session_id,
            host_id: pending.host_id,
            source: pending.source,
            sensitivity: "owner".to_string(),
            schema: pending.schema,
            correlation: pending.correlation,
            native: pending.native,
            cutex: pending.cutex,
        };
        envelope.validate()?;
        let mut encoded =
            serde_json::to_vec(&envelope).context("Failed to serialize management v2 event")?;
        encoded.push(b'\n');
        let rotated = self.rotate_if_needed_locked(encoded.len() as u64)?;
        let (start_offset, next_offset) = self.append_line_locked(&encoded)?;
        state.next_sequence = state.next_sequence.saturating_add(1);
        self.write_state_locked(state)?;
        self.record_indexed_append(&envelope, rotated, start_offset, next_offset);
        Ok(envelope)
    }

    fn append_reset_for_session_locked(
        &self,
        state: &mut StreamState,
        cutex_session_id: &str,
    ) -> anyhow::Result<Option<EventEnvelope>> {
        let reset = {
            let pending = self
                .recovery_reset
                .lock()
                .map_err(|_| anyhow::anyhow!("management v2 recovery-reset lock was poisoned"))?;
            pending.as_ref().and_then(|pending| {
                (!pending.emitted_session_ids.contains(cutex_session_id))
                    .then(|| pending.reset.clone())
            })
        };
        let Some(reset) = reset else {
            return Ok(None);
        };
        if reset.stream_id != state.stream_id {
            anyhow::bail!("pending stream reset does not match the active stream");
        }
        let envelope = self.append_pending_locked(
            state,
            PendingEvent {
                cutex_session_id: cutex_session_id.to_string(),
                host_id: self.host_id.clone(),
                source: super::model::EventSource::Cutex,
                schema: None,
                correlation: super::model::EventCorrelation::default(),
                native: None,
                cutex: Some(super::model::CutexMessage {
                    method: "cutex/stream/reset".to_string(),
                    params: serde_json::json!({
                        "previousStreamId": reset.previous_stream_id,
                        "previousLastSequence": reset.previous_last_sequence.unwrap_or(0),
                        "reason": reset.reason,
                        "lostEventCount": null,
                        "resyncRequired": true,
                        "detectedAt": reset.detected_at,
                    }),
                }),
            },
        )?;
        let mut pending = self
            .recovery_reset
            .lock()
            .map_err(|_| anyhow::anyhow!("management v2 recovery-reset lock was poisoned"))?;
        if let Some(pending) = pending.as_mut() {
            pending
                .emitted_session_ids
                .insert(cutex_session_id.to_string());
            self.persist_pending_recovery_reset_locked(pending)?;
        }
        Ok(Some(envelope))
    }

    fn reconcile_emitted_reset_sessions_locked(&self) -> anyhow::Result<()> {
        if !self.has_pending_recovery_reset()? {
            return Ok(());
        }
        let emitted = self
            .read_records_locked()?
            .into_iter()
            .filter(|event| {
                event
                    .cutex
                    .as_ref()
                    .is_some_and(|message| message.method == "cutex/stream/reset")
            })
            .map(|event| event.cutex_session_id)
            .collect::<HashSet<_>>();
        let mut pending = self
            .recovery_reset
            .lock()
            .map_err(|_| anyhow::anyhow!("management v2 recovery-reset lock was poisoned"))?;
        if let Some(pending) = pending.as_mut() {
            pending.emitted_session_ids.extend(emitted);
            self.persist_pending_recovery_reset_locked(pending)?;
        }
        Ok(())
    }

    fn prepare_root(&self) -> anyhow::Result<()> {
        fs::create_dir_all(&self.root).with_context(|| {
            format!(
                "Failed to create management v2 repository: {}",
                self.root.display()
            )
        })?;
        secure_directory(&self.root)?;
        Ok(())
    }

    fn with_repository_lock<T>(
        &self,
        action: impl FnOnce(&Self) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("management v2 process lock was poisoned"))?;
        let lock_path = self.root.join(LOCK_FILE_NAME);
        let file = open_private_file(&lock_path, false)?;
        file.lock().with_context(|| {
            format!(
                "Failed to lock management v2 repository: {}",
                lock_path.display()
            )
        })?;
        let result = action(self);
        if let Err(error) = file.unlock() {
            if result.is_ok() {
                return Err(error).with_context(|| {
                    format!(
                        "Failed to unlock management v2 repository: {}",
                        lock_path.display()
                    )
                });
            }
        }
        result
    }

    fn with_replay_lock<T>(
        &self,
        action: impl FnOnce(&Self) -> Result<T, ReplayError>,
    ) -> Result<T, ReplayError> {
        let _process_guard = self.process_lock.lock().map_err(|_| {
            ReplayError::Repository(anyhow::anyhow!("management v2 process lock was poisoned"))
        })?;
        let lock_path = self.root.join(LOCK_FILE_NAME);
        let file = open_private_file(&lock_path, false).map_err(ReplayError::from)?;
        file.lock().map_err(|error| {
            ReplayError::Repository(anyhow::Error::new(error).context(format!(
                "Failed to lock management v2 repository: {}",
                lock_path.display()
            )))
        })?;
        let result = action(self);
        if let Err(error) = file.unlock() {
            if result.is_ok() {
                return Err(ReplayError::Repository(anyhow::Error::new(error).context(
                    format!(
                        "Failed to unlock management v2 repository: {}",
                        lock_path.display()
                    ),
                )));
            }
        }
        result
    }

    fn load_append_state_locked(
        &self,
    ) -> anyhow::Result<(StreamState, Option<StreamRecoveryReset>)> {
        if let Ok(Some(state)) = self.try_load_append_state_locked() {
            return Ok((state, None));
        }
        self.load_or_recover_state_locked()
    }

    fn load_snapshot_locked(
        &self,
    ) -> anyhow::Result<(RepositorySnapshot, Option<StreamRecoveryReset>)> {
        if let Ok(Some(snapshot)) = self.try_load_snapshot_locked() {
            return Ok((snapshot, None));
        }
        let (state, reset) = self.load_or_recover_state_locked()?;
        let snapshot = self
            .try_snapshot_for_state_locked(state)?
            .context("recovered management v2 state has inconsistent retained boundaries")?;
        Ok((snapshot, reset))
    }

    fn try_load_snapshot_locked(&self) -> anyhow::Result<Option<RepositorySnapshot>> {
        let state = match self.try_read_state_locked()? {
            Some(state) => state,
            None => return Ok(None),
        };
        self.try_snapshot_for_state_locked(state)
    }

    fn try_snapshot_for_state_locked(
        &self,
        state: StreamState,
    ) -> anyhow::Result<Option<RepositorySnapshot>> {
        let earliest = self.read_first_record_locked()?;
        let latest = self.read_last_record_locked()?;
        match (earliest, latest) {
            (None, None) if state.next_sequence == 1 => Ok(Some(RepositorySnapshot {
                state,
                earliest: None,
                latest: None,
            })),
            (Some(earliest), Some(latest))
                if earliest.stream_id == state.stream_id
                    && latest.stream_id == state.stream_id
                    && earliest.sequence <= latest.sequence
                    && state.next_sequence == latest.sequence.saturating_add(1) =>
            {
                Ok(Some(RepositorySnapshot {
                    state,
                    earliest: Some(EventStreamBoundary {
                        sequence: earliest.sequence,
                        cursor: earliest.cursor,
                    }),
                    latest: Some(EventStreamBoundary {
                        sequence: latest.sequence,
                        cursor: latest.cursor,
                    }),
                }))
            }
            _ => Ok(None),
        }
    }

    fn try_read_state_locked(&self) -> anyhow::Result<Option<StreamState>> {
        let state_path = self.root.join(STATE_FILE_NAME);
        let bytes = match fs::read(&state_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("Failed to read stream state: {}", state_path.display())
                })
            }
        };
        Ok(match serde_json::from_slice::<StreamState>(&bytes) {
            Ok(state) if state.version == 1 => Some(state),
            Ok(_) | Err(_) => None,
        })
    }

    fn try_load_append_state_locked(&self) -> anyhow::Result<Option<StreamState>> {
        let state = match self.try_read_state_locked()? {
            Some(state) => state,
            None => return Ok(None),
        };
        let Some(last) = self.read_last_record_locked()? else {
            return Ok((state.next_sequence == 1).then_some(state));
        };
        let expected_next = last.sequence.saturating_add(1);
        Ok(
            (state.stream_id == last.stream_id && state.next_sequence == expected_next)
                .then_some(state),
        )
    }

    fn load_or_recover_state_locked(
        &self,
    ) -> anyhow::Result<(StreamState, Option<StreamRecoveryReset>)> {
        let state_path = self.root.join(STATE_FILE_NAME);
        let mut state_parse_error = None;
        let loaded_state = match fs::read(&state_path) {
            Ok(bytes) => match serde_json::from_slice::<StreamState>(&bytes) {
                Ok(state) => Some(state),
                Err(error) => {
                    state_parse_error = Some(error);
                    None
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("Failed to read stream state: {}", state_path.display())
                })
            }
        };
        let scan = match self.scan_records_locked(false) {
            Ok(scan) => scan,
            Err(error) => {
                return self.reset_uncertain_stream_locked(
                    loaded_state.as_ref(),
                    None,
                    "protocol_violation",
                    format!("failed to read committed journal: {error:#}"),
                );
            }
        };
        if let Some(error) = state_parse_error {
            let prior = scan.last.as_ref().map(|event| StreamState {
                version: 1,
                stream_id: scan
                    .stream_id
                    .clone()
                    .expect("nonempty journal scan has a stream id"),
                next_sequence: event.sequence.saturating_add(1),
            });
            return self.reset_uncertain_stream_locked(
                prior.as_ref(),
                scan.last.as_ref().map(|event| event.sequence),
                "protocol_violation",
                format!("invalid stream state: {error}"),
            );
        }
        if scan.last.is_none() {
            if let Some(state) = loaded_state {
                if state.version == 1 && state.next_sequence == 1 {
                    return Ok((state, None));
                }
                return self.reset_uncertain_stream_locked(
                    Some(&state),
                    None,
                    "state_lost",
                    "stream state exists without its committed journal".to_string(),
                );
            }
            let state = StreamState::new();
            self.write_state_locked(&state)?;
            return Ok((state, None));
        }

        let last = scan.last.as_ref().expect("non-empty journal scan");
        let journal_stream_id = scan
            .stream_id
            .as_ref()
            .expect("non-empty journal scan has a stream id");
        let mut state = match loaded_state {
            Some(state) => state,
            None => StreamState {
                version: 1,
                stream_id: journal_stream_id.clone(),
                next_sequence: last.sequence.saturating_add(1),
            },
        };
        if state.version != 1 || state.stream_id != *journal_stream_id {
            return self.reset_uncertain_stream_locked(
                Some(&state),
                Some(last.sequence),
                "protocol_violation",
                "stream state and journal disagree".to_string(),
            );
        }
        let recovered_next = last.sequence.saturating_add(1);
        if state.next_sequence > recovered_next {
            return self.reset_uncertain_stream_locked(
                Some(&state),
                Some(last.sequence),
                "state_lost",
                "stream state advances beyond the committed journal".to_string(),
            );
        }
        if state.next_sequence != recovered_next {
            state.next_sequence = recovered_next;
            self.write_state_locked(&state)?;
        }
        Ok((state, None))
    }

    fn reset_uncertain_stream_locked(
        &self,
        prior_state: Option<&StreamState>,
        last_sequence: Option<u64>,
        reason: &str,
        diagnostic: String,
    ) -> anyhow::Result<(StreamState, Option<StreamRecoveryReset>)> {
        let orphaned = self.root.join(ORPHANED_DIR_NAME);
        fs::create_dir_all(&orphaned).with_context(|| {
            format!(
                "Failed to create orphaned stream dir: {}",
                orphaned.display()
            )
        })?;
        secure_directory(&orphaned)?;
        let quarantine_id = Uuid::new_v4();
        for (index, path) in self.read_paths().into_iter().enumerate() {
            if !path.exists() {
                continue;
            }
            let destination = orphaned.join(format!("{quarantine_id}-{index}.jsonl"));
            fs::rename(&path, &destination).with_context(|| {
                format!(
                    "Failed to quarantine uncertain stream {} to {}",
                    path.display(),
                    destination.display()
                )
            })?;
        }
        *self
            .replay_index
            .lock()
            .map_err(|_| anyhow::anyhow!("management v2 replay-index lock was poisoned"))? =
            ReplayIndex::default();
        let state = StreamState::new();
        self.write_state_locked(&state)?;
        let reset = StreamRecoveryReset {
            previous_stream_id: prior_state.map(|state| state.stream_id.clone()),
            previous_last_sequence: last_sequence,
            stream_id: state.stream_id.clone(),
            reason: reason.to_string(),
            detected_at: Utc::now().to_rfc3339(),
            diagnostic,
        };
        Ok((state, Some(reset)))
    }

    fn record_recovery_reset(&self, reset: Option<StreamRecoveryReset>) -> anyhow::Result<()> {
        if let Some(reset) = reset {
            let pending = PendingStreamReset {
                reset,
                emitted_session_ids: HashSet::new(),
            };
            self.persist_pending_recovery_reset_locked(&pending)?;
            *self
                .recovery_reset
                .lock()
                .map_err(|_| anyhow::anyhow!("management v2 recovery-reset lock was poisoned"))? =
                Some(pending);
        }
        Ok(())
    }

    fn restore_pending_recovery_reset_locked(&self, state: &StreamState) -> anyhow::Result<()> {
        let path = self.root.join(PENDING_RESET_FILE_NAME);
        let pending = match fs::read(&path) {
            Ok(bytes) => Some(
                serde_json::from_slice::<PendingStreamReset>(&bytes).with_context(|| {
                    format!(
                        "Failed to parse pending management v2 stream reset: {}",
                        path.display()
                    )
                })?,
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "Failed to read pending management v2 stream reset: {}",
                        path.display()
                    )
                })
            }
        };
        match pending {
            Some(pending) if pending.reset.stream_id == state.stream_id => {
                *self.recovery_reset.lock().map_err(|_| {
                    anyhow::anyhow!("management v2 recovery-reset lock was poisoned")
                })? = Some(pending);
                self.reconcile_emitted_reset_sessions_locked()?;
            }
            Some(_) => {
                remove_file_if_present(&path)?;
            }
            None => {}
        }
        Ok(())
    }

    fn persist_pending_recovery_reset_locked(
        &self,
        pending: &PendingStreamReset,
    ) -> anyhow::Result<()> {
        write_private_pretty_json_atomic(
            &self.root.join(PENDING_RESET_FILE_NAME),
            pending,
            "pending management v2 stream reset",
        )
    }

    fn clear_pending_recovery_reset_locked(&self) -> anyhow::Result<()> {
        *self
            .recovery_reset
            .lock()
            .map_err(|_| anyhow::anyhow!("management v2 recovery-reset lock was poisoned"))? = None;
        remove_file_if_present(&self.root.join(PENDING_RESET_FILE_NAME))
    }

    fn read_records_locked(&self) -> anyhow::Result<Vec<EventEnvelope>> {
        Ok(self.scan_records_locked(true)?.records)
    }

    fn scan_records_locked(&self, collect_records: bool) -> anyhow::Result<JournalScan> {
        let mut records = Vec::new();
        let mut files = Vec::with_capacity(self.options.retained_files + 1);
        let mut cursors = HashMap::new();
        let mut stream_id = None;
        let mut last_record = None;
        let mut expected_sequence = None;
        for (file_index, path) in self.read_paths().into_iter().enumerate() {
            let file = match File::open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    files.push(JournalFileStamp {
                        length: 0,
                        first: None,
                        last: None,
                    });
                    continue;
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("Failed to open management v2 log: {}", path.display())
                    })
                }
            };
            let mut reader = BufReader::new(file);
            let mut line = String::new();
            let mut offset = 0_u64;
            let mut first = None;
            let mut last = None;
            loop {
                line.clear();
                let read = reader.read_line(&mut line).with_context(|| {
                    format!("Failed to read management v2 log: {}", path.display())
                })?;
                if read == 0 {
                    break;
                }
                if !line.ends_with('\n') {
                    anyhow::bail!("management v2 log has a partial tail: {}", path.display());
                }
                let event: EventEnvelope =
                    serde_json::from_str(line.trim_end()).with_context(|| {
                        format!("Invalid management v2 event in {}", path.display())
                    })?;
                event.validate().with_context(|| {
                    format!("Invalid management v2 envelope in {}", path.display())
                })?;
                match (stream_id.as_deref(), expected_sequence) {
                    (Some(expected_stream_id), Some(expected)) => {
                        if event.stream_id != expected_stream_id {
                            anyhow::bail!("retained management v2 log mixes stream ids");
                        }
                        if event.sequence != expected {
                            anyhow::bail!(
                                "retained management v2 sequence gap: expected {expected}, found {}",
                                event.sequence
                            );
                        }
                    }
                    (None, None) => stream_id = Some(event.stream_id.clone()),
                    _ => unreachable!("journal scan stream and sequence state diverged"),
                }
                expected_sequence = Some(event.sequence.saturating_add(1));
                offset = offset.saturating_add(read as u64);
                let boundary = boundary_for_event(&event);
                first.get_or_insert_with(|| boundary.clone());
                last = Some(boundary.clone());
                last_record = Some(boundary);
                cursors
                    .entry(event.cursor.clone())
                    .or_insert(ReplayLocation {
                        file_index,
                        next_offset: offset,
                        sequence: event.sequence,
                    });
                if collect_records {
                    records.push(event);
                }
            }
            files.push(JournalFileStamp {
                length: offset,
                first,
                last,
            });
        }
        *self
            .replay_index
            .lock()
            .map_err(|_| anyhow::anyhow!("management v2 replay-index lock was poisoned"))? =
            ReplayIndex {
                stream_id: stream_id.clone(),
                files,
                cursors,
            };
        Ok(JournalScan {
            stream_id,
            last: last_record,
            records,
        })
    }

    fn read_first_record_locked(&self) -> anyhow::Result<Option<EventEnvelope>> {
        for path in self.read_paths() {
            if let Some(encoded) = read_first_complete_line(&path)? {
                return parse_record(&path, &encoded).map(Some);
            }
        }
        Ok(None)
    }

    fn read_last_record_locked(&self) -> anyhow::Result<Option<EventEnvelope>> {
        let paths = std::iter::once(self.root.join(EVENTS_FILE_NAME))
            .chain((1..=self.options.retained_files).map(|index| self.rotated_path(index)));
        for path in paths {
            let Some(encoded) = read_last_complete_line(&path)? else {
                continue;
            };
            return parse_record(&path, &encoded).map(Some);
        }
        Ok(None)
    }

    fn ensure_replay_index_locked(&self, stream_id: &str) -> anyhow::Result<()> {
        let paths = self.read_paths();
        let current_files = paths
            .iter()
            .map(|path| file_stamp(path))
            .collect::<anyhow::Result<Vec<_>>>()?;
        {
            let mut index = self
                .replay_index
                .lock()
                .map_err(|_| anyhow::anyhow!("management v2 replay-index lock was poisoned"))?;
            if index.stream_id.as_deref() == Some(stream_id) && index.files == current_files {
                return Ok(());
            }
            if index.stream_id.as_deref() == Some(stream_id)
                && self.try_extend_replay_index_locked(
                    &mut index,
                    &paths,
                    &current_files,
                    stream_id,
                )?
            {
                return Ok(());
            }
        }

        self.scan_records_locked(false)?;
        let mut index = self
            .replay_index
            .lock()
            .map_err(|_| anyhow::anyhow!("management v2 replay-index lock was poisoned"))?;
        match index.stream_id.as_deref() {
            Some(indexed_stream_id) if indexed_stream_id != stream_id => {
                anyhow::bail!(
                    "management v2 replay index stream {indexed_stream_id} differs from active stream {stream_id}"
                );
            }
            Some(_) => {}
            None => index.stream_id = Some(stream_id.to_string()),
        }
        Ok(())
    }

    fn try_extend_replay_index_locked(
        &self,
        index: &mut ReplayIndex,
        paths: &[PathBuf],
        current_files: &[JournalFileStamp],
        stream_id: &str,
    ) -> anyhow::Result<bool> {
        if index.files.len() != current_files.len() || current_files.is_empty() {
            return Ok(false);
        }
        let active_index = current_files.len() - 1;
        if index.files[..active_index] != current_files[..active_index] {
            return Ok(false);
        }
        let previous = &index.files[active_index];
        let current = &current_files[active_index];
        if current.length <= previous.length
            || (previous.length > 0 && previous.first != current.first)
        {
            return Ok(false);
        }
        if previous.length > 0 {
            let Some(encoded) =
                read_complete_line_ending_at(&paths[active_index], previous.length)?
            else {
                return Ok(false);
            };
            let previous_tail = parse_record(&paths[active_index], &encoded)?;
            if Some(boundary_for_event(&previous_tail)) != previous.last {
                return Ok(false);
            }
        }

        let mut file = File::open(&paths[active_index]).with_context(|| {
            format!(
                "Failed to open management v2 log: {}",
                paths[active_index].display()
            )
        })?;
        file.seek(SeekFrom::Start(previous.length))
            .with_context(|| {
                format!(
                    "Failed to seek management v2 log: {}",
                    paths[active_index].display()
                )
            })?;
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        let mut offset = previous.length;
        let mut expected_sequence = index
            .files
            .iter()
            .rev()
            .find_map(|file| file.last.as_ref().map(|boundary| boundary.sequence))
            .map_or(1, |sequence| sequence.saturating_add(1));
        loop {
            line.clear();
            let read = reader.read_line(&mut line).with_context(|| {
                format!(
                    "Failed to read management v2 log: {}",
                    paths[active_index].display()
                )
            })?;
            if read == 0 {
                break;
            }
            if !line.ends_with('\n') {
                anyhow::bail!(
                    "management v2 log has a partial tail: {}",
                    paths[active_index].display()
                );
            }
            let event = parse_record(&paths[active_index], line.trim_end().as_bytes())?;
            if event.stream_id != stream_id || event.sequence != expected_sequence {
                anyhow::bail!(
                    "management v2 incremental replay index expected stream {stream_id} sequence {expected_sequence}, found stream {} sequence {}",
                    event.stream_id,
                    event.sequence
                );
            }
            offset = offset.saturating_add(read as u64);
            index
                .cursors
                .entry(event.cursor.clone())
                .or_insert(ReplayLocation {
                    file_index: active_index,
                    next_offset: offset,
                    sequence: event.sequence,
                });
            expected_sequence = expected_sequence.saturating_add(1);
        }
        if offset != current.length {
            anyhow::bail!(
                "management v2 incremental replay index stopped at {offset}, expected {}",
                current.length
            );
        }
        index.files[active_index] = current.clone();
        Ok(true)
    }

    fn read_indexed_records_locked(
        &self,
        start: ReplayLocation,
        limit: usize,
        stream_id: &str,
    ) -> anyhow::Result<Vec<EventEnvelope>> {
        let paths = self.read_paths();
        if start.file_index >= paths.len() {
            anyhow::bail!(
                "management v2 replay index file {} is outside the retained journal",
                start.file_index
            );
        }
        let mut records = Vec::with_capacity(limit);
        let mut expected_sequence = start.sequence.saturating_add(1);
        for (file_index, path) in paths.iter().enumerate().skip(start.file_index) {
            let file = match File::open(path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("Failed to open management v2 log: {}", path.display())
                    })
                }
            };
            let mut reader = BufReader::new(file);
            if file_index == start.file_index {
                reader
                    .seek(SeekFrom::Start(start.next_offset))
                    .with_context(|| {
                        format!("Failed to seek management v2 log: {}", path.display())
                    })?;
            }
            let mut line = String::new();
            loop {
                line.clear();
                let read = reader.read_line(&mut line).with_context(|| {
                    format!("Failed to read management v2 log: {}", path.display())
                })?;
                if read == 0 {
                    break;
                }
                if !line.ends_with('\n') {
                    anyhow::bail!("management v2 log has a partial tail: {}", path.display());
                }
                let event = parse_record(path, line.trim_end().as_bytes())?;
                if event.stream_id != stream_id || event.sequence != expected_sequence {
                    anyhow::bail!(
                        "management v2 bounded replay expected stream {stream_id} sequence {expected_sequence}, found stream {} sequence {}",
                        event.stream_id,
                        event.sequence
                    );
                }
                expected_sequence = expected_sequence.saturating_add(1);
                records.push(event);
                if records.len() == limit {
                    return Ok(records);
                }
            }
        }
        Ok(records)
    }

    fn validate_indexed_cursor_locked(
        &self,
        cursor: &str,
        location: ReplayLocation,
        stream_id: &str,
    ) -> anyhow::Result<()> {
        let paths = self.read_paths();
        let path = paths.get(location.file_index).with_context(|| {
            format!(
                "management v2 replay index file {} is outside the retained journal",
                location.file_index
            )
        })?;
        let encoded =
            read_complete_line_ending_at(path, location.next_offset)?.with_context(|| {
                format!(
                "management v2 replay index cursor {cursor} has no complete record at offset {}",
                location.next_offset
            )
            })?;
        let event = parse_record(path, &encoded)?;
        if event.stream_id != stream_id
            || event.cursor != cursor
            || event.sequence != location.sequence
        {
            anyhow::bail!(
                "management v2 replay index cursor {cursor} expected stream {stream_id} sequence {}, found cursor {} stream {} sequence {}",
                location.sequence,
                event.cursor,
                event.stream_id,
                event.sequence
            );
        }
        Ok(())
    }

    fn record_indexed_append(
        &self,
        envelope: &EventEnvelope,
        rotated: bool,
        start_offset: u64,
        next_offset: u64,
    ) {
        let Ok(mut index) = self.replay_index.lock() else {
            return;
        };
        if rotated {
            let file_count = self.options.retained_files + 1;
            if index.files.len() != file_count
                || index.stream_id.as_deref() != Some(envelope.stream_id.as_str())
                || start_offset != 0
            {
                *index = ReplayIndex::default();
                return;
            }
            let boundary = boundary_for_event(envelope);
            let mut shifted_files = index.files[1..].to_vec();
            shifted_files.push(JournalFileStamp {
                length: next_offset,
                first: Some(boundary.clone()),
                last: Some(boundary),
            });
            let actual_files = match self
                .read_paths()
                .iter()
                .map(|path| file_stamp(path))
                .collect::<anyhow::Result<Vec<_>>>()
            {
                Ok(files) => files,
                Err(_) => {
                    *index = ReplayIndex::default();
                    return;
                }
            };
            if shifted_files != actual_files {
                *index = ReplayIndex::default();
                return;
            }
            index.cursors.retain(|_, location| {
                if location.file_index == 0 {
                    false
                } else {
                    location.file_index -= 1;
                    true
                }
            });
            index.files = shifted_files;
            index
                .cursors
                .entry(envelope.cursor.clone())
                .or_insert(ReplayLocation {
                    file_index: file_count - 1,
                    next_offset,
                    sequence: envelope.sequence,
                });
            return;
        }
        let file_count = self.options.retained_files + 1;
        if index.files.len() != file_count {
            return;
        }
        if index.stream_id.is_none()
            && start_offset == 0
            && index.files.iter().all(|file| file.length == 0)
        {
            index.stream_id = Some(envelope.stream_id.clone());
        }
        if index.stream_id.as_deref() != Some(envelope.stream_id.as_str()) {
            return;
        }
        let expected_sequence = index
            .files
            .iter()
            .rev()
            .find_map(|file| file.last.as_ref().map(|boundary| boundary.sequence))
            .map_or(1, |sequence| sequence.saturating_add(1));
        let active_index = file_count - 1;
        if index.files[active_index].length != start_offset
            || envelope.sequence != expected_sequence
        {
            return;
        }
        let boundary = boundary_for_event(envelope);
        let active = &mut index.files[active_index];
        active.length = next_offset;
        active.first.get_or_insert_with(|| boundary.clone());
        active.last = Some(boundary);
        index
            .cursors
            .entry(envelope.cursor.clone())
            .or_insert(ReplayLocation {
                file_index: active_index,
                next_offset,
                sequence: envelope.sequence,
            });
    }

    fn read_paths(&self) -> Vec<PathBuf> {
        let mut paths = Vec::with_capacity(self.options.retained_files + 1);
        for index in (1..=self.options.retained_files).rev() {
            paths.push(self.rotated_path(index));
        }
        paths.push(self.root.join(EVENTS_FILE_NAME));
        paths
    }

    fn rotate_if_needed_locked(&self, incoming_bytes: u64) -> anyhow::Result<bool> {
        let active = self.root.join(EVENTS_FILE_NAME);
        let current_bytes = match fs::metadata(&active) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("Failed to stat management v2 log: {}", active.display())
                })
            }
        };
        if current_bytes == 0
            || current_bytes.saturating_add(incoming_bytes) <= self.options.rotate_bytes
        {
            return Ok(false);
        }
        let oldest = self.rotated_path(self.options.retained_files);
        remove_file_if_present(&oldest)?;
        for index in (1..self.options.retained_files).rev() {
            let from = self.rotated_path(index);
            if !from.exists() {
                continue;
            }
            let to = self.rotated_path(index + 1);
            remove_file_if_present(&to)?;
            fs::rename(&from, &to).with_context(|| {
                format!(
                    "Failed to rotate management v2 log {} to {}",
                    from.display(),
                    to.display()
                )
            })?;
        }
        let first = self.rotated_path(1);
        remove_file_if_present(&first)?;
        fs::rename(&active, &first).with_context(|| {
            format!(
                "Failed to rotate management v2 log {} to {}",
                active.display(),
                first.display()
            )
        })?;
        Ok(true)
    }

    fn rotated_path(&self, index: usize) -> PathBuf {
        self.root.join(format!("{EVENTS_FILE_NAME}.{index}"))
    }

    fn append_line_locked(&self, encoded: &[u8]) -> anyhow::Result<(u64, u64)> {
        let path = self.root.join(EVENTS_FILE_NAME);
        let mut file = open_private_file(&path, true)?;
        let start_offset = file
            .metadata()
            .with_context(|| format!("Failed to stat management v2 event: {}", path.display()))?
            .len();
        file.write_all(encoded)
            .with_context(|| format!("Failed to append management v2 event: {}", path.display()))?;
        file.flush()
            .with_context(|| format!("Failed to flush management v2 event: {}", path.display()))?;
        file.sync_data()
            .with_context(|| format!("Failed to sync management v2 event: {}", path.display()))?;
        Ok((
            start_offset,
            start_offset.saturating_add(encoded.len() as u64),
        ))
    }

    fn write_state_locked(&self, state: &StreamState) -> anyhow::Result<()> {
        write_private_pretty_json_atomic(
            &self.root.join(STATE_FILE_NAME),
            state,
            "management v2 stream state",
        )
    }

    fn publish(&self, envelope: &EventEnvelope) {
        let Ok(mut subscribers) = self.subscribers.lock() else {
            return;
        };
        subscribers.retain(|sender| match sender.try_send(envelope.clone()) {
            Ok(()) => true,
            Err(mpsc::TrySendError::Full(_)) | Err(mpsc::TrySendError::Disconnected(_)) => false,
        });
    }
}

fn read_last_complete_line(path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to open management v2 log: {}", path.display()))
        }
    };
    let length = file
        .metadata()
        .with_context(|| format!("Failed to stat management v2 log: {}", path.display()))?
        .len();
    if length == 0 {
        return Ok(None);
    }
    drop(file);
    read_complete_line_ending_at(path, length)
}

fn read_first_complete_line(path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to open management v2 log: {}", path.display()))
        }
    };
    let mut reader = BufReader::new(file);
    let mut encoded = Vec::new();
    let read = reader
        .read_until(b'\n', &mut encoded)
        .with_context(|| format!("Failed to read management v2 log: {}", path.display()))?;
    if read == 0 {
        return Ok(None);
    }
    if encoded.last() != Some(&b'\n') {
        anyhow::bail!("management v2 log has a partial tail: {}", path.display());
    }
    encoded.pop();
    Ok(Some(encoded))
}

fn read_complete_line_ending_at(path: &Path, end: u64) -> anyhow::Result<Option<Vec<u8>>> {
    if end == 0 {
        return Ok(None);
    }
    let mut file = File::open(path)
        .with_context(|| format!("Failed to open management v2 log: {}", path.display()))?;
    let length = file
        .metadata()
        .with_context(|| format!("Failed to stat management v2 log: {}", path.display()))?
        .len();
    if end > length {
        anyhow::bail!(
            "management v2 log shrank below indexed offset {end}: {}",
            path.display()
        );
    }

    file.seek(SeekFrom::Start(end - 1))
        .with_context(|| format!("Failed to seek management v2 log: {}", path.display()))?;
    let mut terminal = [0_u8; 1];
    file.read_exact(&mut terminal)
        .with_context(|| format!("Failed to read management v2 log: {}", path.display()))?;
    if terminal[0] != b'\n' {
        anyhow::bail!("management v2 log has a partial tail: {}", path.display());
    }

    let line_end = end - 1;
    let mut cursor = line_end;
    let mut scan = [0_u8; 8 * 1024];
    let line_start = loop {
        if cursor == 0 {
            break 0;
        }
        let chunk_start = cursor.saturating_sub(scan.len() as u64);
        let chunk_len = (cursor - chunk_start) as usize;
        file.seek(SeekFrom::Start(chunk_start))
            .with_context(|| format!("Failed to seek management v2 log: {}", path.display()))?;
        file.read_exact(&mut scan[..chunk_len])
            .with_context(|| format!("Failed to read management v2 log: {}", path.display()))?;
        if let Some(index) = scan[..chunk_len].iter().rposition(|byte| *byte == b'\n') {
            break chunk_start + index as u64 + 1;
        }
        cursor = chunk_start;
    };

    let line_len = (line_end - line_start) as usize;
    let mut encoded = vec![0_u8; line_len];
    file.seek(SeekFrom::Start(line_start))
        .with_context(|| format!("Failed to seek management v2 log: {}", path.display()))?;
    file.read_exact(&mut encoded)
        .with_context(|| format!("Failed to read management v2 log: {}", path.display()))?;
    Ok(Some(encoded))
}

fn parse_record(path: &Path, encoded: &[u8]) -> anyhow::Result<EventEnvelope> {
    let event: EventEnvelope = serde_json::from_slice(encoded)
        .with_context(|| format!("Invalid management v2 event in {}", path.display()))?;
    event
        .validate()
        .with_context(|| format!("Invalid management v2 envelope in {}", path.display()))?;
    Ok(event)
}

fn file_stamp(path: &Path) -> anyhow::Result<JournalFileStamp> {
    let length = match fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to stat management v2 log: {}", path.display()))
        }
    };
    if length == 0 {
        return Ok(JournalFileStamp {
            length,
            first: None,
            last: None,
        });
    }
    let first = read_first_complete_line(path)?
        .map(|encoded| parse_record(path, &encoded))
        .transpose()?
        .map(|event| boundary_for_event(&event));
    let last = read_complete_line_ending_at(path, length)?
        .map(|encoded| parse_record(path, &encoded))
        .transpose()?
        .map(|event| boundary_for_event(&event));
    Ok(JournalFileStamp {
        length,
        first,
        last,
    })
}

fn checkpoint_for_snapshot(snapshot: &RepositorySnapshot) -> EventCheckpoint {
    EventCheckpoint {
        stream_id: snapshot.state.stream_id.clone(),
        sequence: snapshot
            .latest
            .as_ref()
            .map_or(0, |boundary| boundary.sequence),
        cursor: snapshot
            .latest
            .as_ref()
            .map(|boundary| boundary.cursor.clone()),
    }
}

fn boundary_for_event(event: &EventEnvelope) -> RetainedBoundary {
    RetainedBoundary {
        sequence: event.sequence,
        cursor: event.cursor.clone(),
    }
}

fn retained_boundary(boundary: &EventStreamBoundary) -> RetainedBoundary {
    RetainedBoundary {
        sequence: boundary.sequence,
        cursor: boundary.cursor.clone(),
    }
}

fn open_private_file(path: &Path, append: bool) -> anyhow::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).append(append);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    let file = options
        .open(path)
        .with_context(|| format!("Failed to open private file: {}", path.display()))?;
    secure_file(path)?;
    Ok(file)
}

fn remove_file_if_present(path: &Path) -> anyhow::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error)
            .with_context(|| format!("Failed to remove management v2 file: {}", path.display())),
    }
}

#[cfg(unix)]
fn secure_directory(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("Failed to secure directory: {}", path.display()))
}

#[cfg(not(unix))]
fn secure_directory(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn secure_file(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("Failed to secure file: {}", path.display()))
}

#[cfg(not(unix))]
fn secure_file(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use serde_json::json;

    use super::*;
    use crate::management::v2::model::CutexMessage;
    use crate::management::v2::model::EventCorrelation;
    use crate::management::v2::model::EventSource;

    fn test_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("cutex-management-v2-{label}-{}", Uuid::new_v4()))
    }

    fn pending(session: &str, value: usize) -> PendingEvent {
        PendingEvent {
            cutex_session_id: session.to_string(),
            host_id: "tethys".to_string(),
            source: EventSource::Cutex,
            schema: None,
            correlation: EventCorrelation::default(),
            native: None,
            cutex: Some(CutexMessage {
                method: "cutex/test/event".to_string(),
                params: json!({ "value": value, "explicitNull": null }),
            }),
        }
    }

    fn reference_page(repository: &EventRepository, query: &ReplayQuery) -> EventPage {
        let state = repository
            .try_read_state_locked()
            .expect("read reference state")
            .expect("reference state");
        let records = repository
            .read_records_locked()
            .expect("read reference records");
        let initial_cursor = query
            .after
            .as_deref()
            .is_none_or(|value| value.is_empty() || value == "0");
        let start = if initial_cursor {
            0
        } else {
            records
                .iter()
                .position(|event| Some(event.cursor.as_str()) == query.after.as_deref())
                .expect("reference cursor")
                + 1
        };
        let end = start.saturating_add(query.limit).min(records.len());
        let scanned = &records[start..end];
        EventPage {
            contract_version: CONTRACT_VERSION,
            host_id: repository.host_id.clone(),
            stream_id: state.stream_id.clone(),
            scope: EventPageScope {
                cutex_session_id: query.cutex_session_id.clone(),
            },
            events: scanned
                .iter()
                .filter(|event| {
                    query
                        .cutex_session_id
                        .as_deref()
                        .is_none_or(|session_id| event.cutex_session_id == session_id)
                })
                .cloned()
                .collect(),
            next_cursor: scanned
                .last()
                .map(|event| event.cursor.clone())
                .or_else(|| (!initial_cursor).then(|| query.after.clone()).flatten()),
            checkpoint: match records.last() {
                Some(event) => EventCheckpoint {
                    stream_id: state.stream_id,
                    sequence: event.sequence,
                    cursor: Some(event.cursor.clone()),
                },
                None => EventCheckpoint {
                    stream_id: state.stream_id,
                    sequence: 0,
                    cursor: None,
                },
            },
            scanned_count: scanned.len(),
            has_more: end < records.len(),
        }
    }

    #[test]
    fn append_replay_and_zero_match_scans_are_monotonic() {
        let root = test_root("replay");
        let repository = EventRepository::open(&root, "tethys").expect("open repository");
        for value in 0..5 {
            repository
                .append(pending("cutex.session-a", value))
                .expect("append event");
        }
        let first = repository
            .page(ReplayQuery {
                limit: 2,
                cutex_session_id: Some("cutex.missing".to_string()),
                ..ReplayQuery::default()
            })
            .expect("first page");
        assert!(first.events.is_empty());
        assert_eq!(first.scanned_count, 2);
        assert!(first.has_more);
        let second = repository
            .page(ReplayQuery {
                stream_id: Some(first.stream_id.clone()),
                after: first.next_cursor.clone(),
                limit: 3,
                cutex_session_id: Some("cutex.session-a".to_string()),
            })
            .expect("second page");
        assert_eq!(second.events.len(), 3);
        assert_eq!(second.events[0].sequence, 3);
        assert_eq!(second.checkpoint.sequence, 5);
        assert!(!second.has_more);
        fs::remove_dir_all(root).expect("remove test repository");
    }

    #[test]
    fn bounded_pages_match_full_reader_for_initial_continuation_and_filtering() {
        let root = test_root("bounded-page-equivalence");
        let repository = EventRepository::open_with_options(
            &root,
            "tethys",
            RepositoryOptions {
                rotate_bytes: 1_400,
                retained_files: 3,
            },
        )
        .expect("open repository");
        for value in 0..20 {
            let session = if value % 2 == 0 {
                "cutex.session-a"
            } else {
                "cutex.session-b"
            };
            repository
                .append(pending(session, value))
                .expect("append event");
        }

        let initial = ReplayQuery {
            limit: 3,
            cutex_session_id: Some("cutex.missing".to_string()),
            ..ReplayQuery::default()
        };
        let expected_initial = reference_page(&repository, &initial);
        let actual_initial = repository.page(initial).expect("read initial page");
        assert_eq!(actual_initial, expected_initial);

        let continuation = ReplayQuery {
            stream_id: Some(actual_initial.stream_id.clone()),
            after: actual_initial.next_cursor.clone(),
            limit: 4,
            cutex_session_id: Some("cutex.session-a".to_string()),
        };
        let expected_continuation = reference_page(&repository, &continuation);
        let actual_continuation = repository
            .page(continuation)
            .expect("read continuation page");
        assert_eq!(actual_continuation, expected_continuation);

        let final_query = ReplayQuery {
            stream_id: Some(actual_continuation.stream_id.clone()),
            after: actual_continuation.checkpoint.cursor.clone(),
            limit: 10,
            cutex_session_id: None,
        };
        let expected_final = reference_page(&repository, &final_query);
        let actual_final = repository.page(final_query).expect("read final page");
        assert_eq!(actual_final, expected_final);
        fs::remove_dir_all(root).expect("remove test repository");
    }

    #[test]
    fn bounded_page_discards_a_stale_cursor_location_and_rebuilds() {
        let root = test_root("bounded-page-stale-index");
        let repository = EventRepository::open(&root, "tethys").expect("open repository");
        for value in 0..5 {
            repository
                .append(pending("cutex.session-a", value))
                .expect("append event");
        }
        let initial = repository
            .page(ReplayQuery {
                limit: 2,
                ..ReplayQuery::default()
            })
            .expect("build replay index");
        let first = &initial.events[0];
        {
            let mut index = repository.replay_index.lock().expect("lock replay index");
            let location = index
                .cursors
                .get_mut(&first.cursor)
                .expect("indexed first cursor");
            location.next_offset = 0;
        }

        let page = repository
            .page(ReplayQuery {
                stream_id: Some(initial.stream_id),
                after: Some(first.cursor.clone()),
                limit: 2,
                cutex_session_id: None,
            })
            .expect("rebuild stale replay index");
        assert_eq!(page.events.len(), 2);
        assert_eq!(page.events[0].sequence, first.sequence + 1);
        assert!(repository.recovery_reset().unwrap().is_none());
        fs::remove_dir_all(root).expect("remove test repository");
    }

    #[test]
    fn bounded_page_discards_a_plausible_wrong_cursor_location_and_rebuilds() {
        let root = test_root("bounded-page-wrong-index");
        let repository = EventRepository::open(&root, "tethys").expect("open repository");
        for value in 0..5 {
            repository
                .append(pending("cutex.session-a", value))
                .expect("append event");
        }
        let initial = repository
            .page(ReplayQuery {
                limit: 3,
                ..ReplayQuery::default()
            })
            .expect("build replay index");
        let first = initial.events[0].clone();
        let second = initial.events[1].clone();
        {
            let mut index = repository.replay_index.lock().expect("lock replay index");
            let wrong_location = *index
                .cursors
                .get(&second.cursor)
                .expect("indexed second cursor");
            index.cursors.insert(first.cursor.clone(), wrong_location);
        }

        let page = repository
            .page(ReplayQuery {
                stream_id: Some(initial.stream_id),
                after: Some(first.cursor),
                limit: 2,
                cutex_session_id: None,
            })
            .expect("rebuild plausible wrong replay index");
        assert_eq!(page.events.len(), 2);
        assert_eq!(page.events[0].sequence, first.sequence + 1);
        assert!(repository.recovery_reset().unwrap().is_none());
        fs::remove_dir_all(root).expect("remove test repository");
    }

    #[test]
    fn bounded_page_refreshes_external_append_and_rotation() {
        let root = test_root("bounded-page-external");
        let options = RepositoryOptions {
            rotate_bytes: 1_400,
            retained_files: 3,
        };
        let reader = EventRepository::open_with_options(&root, "tethys", options)
            .expect("open reader repository");
        let writer = EventRepository::open_with_options(&root, "tethys", options)
            .expect("open writer repository");
        for value in 0..6 {
            writer
                .append(pending("cutex.session-a", value))
                .expect("append initial external event");
        }
        let initial = reader
            .page(ReplayQuery {
                limit: 100,
                ..ReplayQuery::default()
            })
            .expect("read initial external events");
        let retained_cursor = initial.events.last().expect("last initial event").clone();

        for value in 6..8 {
            writer
                .append(pending("cutex.session-a", value))
                .expect("append external continuation");
        }
        let continuation = reader
            .page(ReplayQuery {
                stream_id: Some(initial.stream_id.clone()),
                after: Some(retained_cursor.cursor.clone()),
                limit: 100,
                cutex_session_id: None,
            })
            .expect("read after external rotation");
        assert_eq!(
            continuation
                .events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![retained_cursor.sequence + 1, retained_cursor.sequence + 2]
        );

        for value in 8..30 {
            writer
                .append(pending("cutex.session-a", value))
                .expect("append expiring external event");
        }
        let error = reader
            .page(ReplayQuery {
                stream_id: Some(initial.stream_id),
                after: Some(retained_cursor.cursor),
                limit: 100,
                cutex_session_id: None,
            })
            .expect_err("rotated cursor should expire");
        assert!(matches!(error, ReplayError::CursorExpired { .. }));
        fs::remove_dir_all(root).expect("remove test repository");
    }

    #[test]
    fn metadata_and_checkpoint_match_full_replay_after_rotation() {
        let root = test_root("metadata-rotation");
        let repository = EventRepository::open_with_options(
            &root,
            "tethys",
            RepositoryOptions {
                rotate_bytes: 700,
                retained_files: 2,
            },
        )
        .expect("open repository");
        for value in 0..8 {
            repository
                .append(pending("cutex.session-a", value))
                .expect("append rotated event");
        }

        let metadata = repository.stream_metadata().expect("read metadata");
        let checkpoint = repository.checkpoint().expect("read checkpoint");
        let page = repository
            .page(ReplayQuery {
                limit: 1000,
                ..ReplayQuery::default()
            })
            .expect("read retained events");
        let first = page.events.first().expect("retained first event");
        let last = page.events.last().expect("retained last event");

        assert_eq!(metadata.stream_id, page.stream_id);
        assert_eq!(
            metadata.earliest,
            Some(EventStreamBoundary {
                sequence: first.sequence,
                cursor: first.cursor.clone(),
            })
        );
        assert_eq!(
            metadata.latest,
            Some(EventStreamBoundary {
                sequence: last.sequence,
                cursor: last.cursor.clone(),
            })
        );
        assert_eq!(checkpoint, page.checkpoint);
        fs::remove_dir_all(root).expect("remove test repository");
    }

    #[test]
    fn metadata_observes_external_append_and_rotation() {
        let root = test_root("metadata-external-rotation");
        let options = RepositoryOptions {
            rotate_bytes: 700,
            retained_files: 2,
        };
        let reader = EventRepository::open_with_options(&root, "tethys", options)
            .expect("open reader repository");
        let writer = EventRepository::open_with_options(&root, "tethys", options)
            .expect("open writer repository");

        let mut latest = None;
        for value in 0..8 {
            latest = Some(
                writer
                    .append(pending("cutex.session-a", value))
                    .expect("append external event"),
            );
        }
        let latest = latest.expect("latest external event");
        let metadata = reader.stream_metadata().expect("read external metadata");
        let page = reader
            .page(ReplayQuery {
                limit: 1000,
                ..ReplayQuery::default()
            })
            .expect("read externally rotated events");

        assert_eq!(metadata.stream_id, latest.stream_id);
        assert_eq!(metadata.latest.unwrap().sequence, latest.sequence);
        assert_eq!(
            metadata.earliest.unwrap().sequence,
            page.events.first().unwrap().sequence
        );
        fs::remove_dir_all(root).expect("remove test repository");
    }

    #[test]
    fn metadata_recovers_stale_state_before_returning_a_checkpoint() {
        let root = test_root("metadata-stale-state");
        let repository = EventRepository::open(&root, "tethys").expect("open repository");
        let latest = repository
            .append(pending("cutex.session-a", 1))
            .expect("append event");
        let state_path = root.join(STATE_FILE_NAME);
        let mut state: StreamState =
            serde_json::from_slice(&fs::read(&state_path).expect("read state"))
                .expect("parse state");
        state.next_sequence = latest.sequence;
        fs::write(
            &state_path,
            serde_json::to_vec_pretty(&state).expect("encode stale state"),
        )
        .expect("write stale state");

        let checkpoint = repository.checkpoint().expect("recover checkpoint");
        let recovered: StreamState =
            serde_json::from_slice(&fs::read(&state_path).expect("read recovered state"))
                .expect("parse recovered state");

        assert_eq!(checkpoint.stream_id, latest.stream_id);
        assert_eq!(checkpoint.sequence, latest.sequence);
        assert_eq!(checkpoint.cursor.as_deref(), Some(latest.cursor.as_str()));
        assert_eq!(recovered.next_sequence, latest.sequence + 1);
        assert!(repository.recovery_reset().unwrap().is_none());
        fs::remove_dir_all(root).expect("remove test repository");
    }

    #[test]
    fn metadata_detects_a_partial_current_tail_immediately() {
        let root = test_root("metadata-partial-tail");
        let repository = EventRepository::open(&root, "tethys").expect("open repository");
        let previous = repository
            .append(pending("cutex.session-a", 1))
            .expect("append event");
        let mut file = OpenOptions::new()
            .append(true)
            .open(root.join(EVENTS_FILE_NAME))
            .expect("open journal");
        file.write_all(b"{\"partial\":true}")
            .expect("write partial tail");
        file.sync_all().expect("sync partial tail");

        let metadata = repository
            .stream_metadata()
            .expect("recover metadata after partial tail");
        let reset = repository
            .recovery_reset()
            .expect("read recovery reset")
            .expect("partial tail reset");

        assert_ne!(metadata.stream_id, previous.stream_id);
        assert_eq!(metadata.stream_id, reset.stream_id);
        assert!(metadata.earliest.is_none());
        assert!(metadata.latest.is_none());
        fs::remove_dir_all(root).expect("remove test repository");
    }

    #[test]
    fn old_middle_corruption_is_repaired_when_full_replay_reaches_it() {
        let root = test_root("metadata-middle-corruption");
        let repository = EventRepository::open(&root, "tethys").expect("open repository");
        for value in 0..5 {
            repository
                .append(pending("cutex.session-a", value))
                .expect("append event");
        }
        let before = repository.stream_metadata().expect("read prior metadata");
        let journal_path = root.join(EVENTS_FILE_NAME);
        let mut lines = fs::read_to_string(&journal_path)
            .expect("read journal")
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>();
        lines[2] = "{not-json".to_string();
        fs::write(&journal_path, format!("{}\n", lines.join("\n"))).expect("corrupt middle record");

        let fast = repository
            .stream_metadata()
            .expect("boundary metadata remains readable");
        assert_eq!(fast, before);
        assert!(repository.recovery_reset().unwrap().is_none());

        let page = repository
            .page(ReplayQuery::default())
            .expect("full replay repairs corrupt history");
        let reset = repository
            .recovery_reset()
            .expect("read recovery reset")
            .expect("middle corruption reset");
        assert_ne!(page.stream_id, before.stream_id);
        assert_eq!(page.stream_id, reset.stream_id);
        assert!(page.events.is_empty());
        fs::remove_dir_all(root).expect("remove test repository");
    }

    #[test]
    fn lost_state_write_recovers_from_committed_tail_without_duplicate_sequence() {
        let root = test_root("recover-state");
        let repository = EventRepository::open(&root, "tethys").expect("open repository");
        let first = repository
            .append(pending("cutex.session-a", 1))
            .expect("append first event");
        fs::remove_file(root.join(STATE_FILE_NAME)).expect("remove state");
        let reopened = EventRepository::open(&root, "tethys").expect("reopen repository");
        let second = reopened
            .append(pending("cutex.session-a", 2))
            .expect("append second event");
        assert_eq!(first.stream_id, second.stream_id);
        assert_eq!(second.sequence, first.sequence + 1);
        fs::remove_dir_all(root).expect("remove test repository");
    }

    #[test]
    fn separate_repository_instances_observe_the_latest_committed_tail() {
        let root = test_root("separate-instances");
        let first_repository =
            EventRepository::open(&root, "tethys").expect("open first repository");
        let second_repository =
            EventRepository::open(&root, "tethys").expect("open second repository");

        let first = first_repository
            .append(pending("cutex.session-a", 1))
            .expect("append from first repository");
        let second = second_repository
            .append(pending("cutex.session-a", 2))
            .expect("append from second repository");
        let third = first_repository
            .append(pending("cutex.session-a", 3))
            .expect("append again from first repository");

        assert_eq!(second.stream_id, first.stream_id);
        assert_eq!(second.sequence, first.sequence + 1);
        assert_eq!(third.sequence, second.sequence + 1);
        fs::remove_dir_all(root).expect("remove test repository");
    }

    #[test]
    fn stale_state_falls_back_to_full_tail_recovery_before_append() {
        let root = test_root("stale-state-append");
        let repository = EventRepository::open(&root, "tethys").expect("open repository");
        let first = repository
            .append(pending("cutex.session-a", 1))
            .expect("append first event");
        let state_path = root.join(STATE_FILE_NAME);
        let mut state: StreamState =
            serde_json::from_slice(&fs::read(&state_path).expect("read state"))
                .expect("parse state");
        state.next_sequence = first.sequence;
        fs::write(
            &state_path,
            serde_json::to_vec_pretty(&state).expect("encode state"),
        )
        .expect("write stale state");

        let second = repository
            .append(pending("cutex.session-a", 2))
            .expect("append after stale state");

        assert_eq!(second.stream_id, first.stream_id);
        assert_eq!(second.sequence, first.sequence + 1);
        fs::remove_dir_all(root).expect("remove test repository");
    }

    #[test]
    fn partial_tail_during_append_uses_the_existing_stream_reset_path() {
        let root = test_root("partial-tail-append");
        let repository = EventRepository::open(&root, "tethys").expect("open repository");
        let first = repository
            .append(pending("cutex.session-a", 1))
            .expect("append first event");
        let mut file = OpenOptions::new()
            .append(true)
            .open(root.join(EVENTS_FILE_NAME))
            .expect("open journal");
        file.write_all(b"{\"partial\":true}")
            .expect("write partial tail");
        file.sync_all().expect("sync partial tail");

        let next = repository
            .append(pending("cutex.session-a", 2))
            .expect("append after partial tail");
        let reset = repository
            .recovery_reset()
            .expect("read reset")
            .expect("recovery reset");

        assert_eq!(
            reset.previous_stream_id.as_deref(),
            Some(first.stream_id.as_str())
        );
        assert_ne!(next.stream_id, first.stream_id);
        assert_eq!(next.sequence, 2);
        fs::remove_dir_all(root).expect("remove test repository");
    }

    #[test]
    fn partial_tail_rotates_to_a_new_stream_and_reports_reset() {
        let root = test_root("partial-tail");
        let repository = EventRepository::open(&root, "tethys").expect("open repository");
        let first = repository
            .append(pending("cutex.session-a", 1))
            .expect("append first event");
        let mut file = OpenOptions::new()
            .append(true)
            .open(root.join(EVENTS_FILE_NAME))
            .expect("open journal");
        file.write_all(b"{\"partial\":true}")
            .expect("write partial tail");
        file.sync_all().expect("sync partial tail");
        let reopened = EventRepository::open(&root, "tethys").expect("reopen repository");
        let reset = reopened
            .recovery_reset()
            .expect("read reset")
            .expect("recovery reset");
        assert_eq!(
            reset.previous_stream_id.as_deref(),
            Some(first.stream_id.as_str())
        );
        assert_ne!(reset.stream_id, first.stream_id);
        let next = reopened
            .append(pending("cutex.session-a", 2))
            .expect("append after reset");
        assert_eq!(next.sequence, 2);
        assert_eq!(next.stream_id, reset.stream_id);
        let page = reopened
            .page(ReplayQuery {
                limit: 10,
                ..ReplayQuery::default()
            })
            .expect("read reset stream");
        assert_eq!(page.events[0].sequence, 1);
        assert_eq!(
            page.events[0]
                .cutex
                .as_ref()
                .map(|message| message.method.as_str()),
            Some("cutex/stream/reset")
        );
        assert_eq!(
            page.events[0].cutex.as_ref().unwrap().params["reason"],
            "protocol_violation"
        );
        fs::remove_dir_all(root).expect("remove test repository");
    }

    #[test]
    fn pending_reset_survives_reopen_until_all_active_sessions_are_materialized() {
        let root = test_root("pending-reset-reopen");
        let repository = EventRepository::open(&root, "tethys").expect("open repository");
        repository
            .append(pending("cutex.session-a", 1))
            .expect("append prior event");
        let mut file = OpenOptions::new()
            .append(true)
            .open(root.join(EVENTS_FILE_NAME))
            .expect("open journal");
        file.write_all(b"{\"partial\":true}")
            .expect("write partial tail");
        file.sync_all().expect("sync partial tail");

        let recovered = EventRepository::open(&root, "tethys").expect("recover repository");
        let event_a = recovered
            .append(pending("cutex.session-a", 2))
            .expect("append session a after reset");
        assert_eq!(event_a.sequence, 2);
        drop(recovered);

        let reopened = EventRepository::open(&root, "tethys").expect("reopen reset stream");
        let reset_events = reopened
            .materialize_recovery_reset(
                &["cutex.session-a".to_string(), "cutex.session-b".to_string()],
                true,
            )
            .expect("materialize remaining reset events");
        assert_eq!(reset_events.len(), 1);
        assert_eq!(reset_events[0].cutex_session_id, "cutex.session-b");
        assert_eq!(reset_events[0].sequence, 3);
        assert!(reopened.recovery_reset().unwrap().is_none());

        let event_b = reopened
            .append(pending("cutex.session-b", 3))
            .expect("append session b after reset completion");
        assert_eq!(event_b.sequence, 4);
        let page = reopened
            .page(ReplayQuery {
                limit: 10,
                ..ReplayQuery::default()
            })
            .expect("read replacement stream");
        assert_eq!(
            page.events
                .iter()
                .filter(|event| {
                    event
                        .cutex
                        .as_ref()
                        .is_some_and(|message| message.method == "cutex/stream/reset")
                })
                .count(),
            2
        );
        fs::remove_dir_all(root).expect("remove test repository");
    }

    #[test]
    fn concurrent_appends_allocate_unique_strict_sequences() {
        let root = test_root("concurrent");
        let repository = Arc::new(
            EventRepository::open_with_options(
                &root,
                "tethys",
                RepositoryOptions {
                    rotate_bytes: 1024 * 1024,
                    retained_files: 3,
                },
            )
            .expect("open repository"),
        );
        let mut workers = Vec::new();
        for worker in 0..4 {
            let repository = repository.clone();
            workers.push(thread::spawn(move || {
                for value in 0..25 {
                    repository
                        .append(pending("cutex.session-a", worker * 25 + value))
                        .expect("append concurrent event");
                }
            }));
        }
        for worker in workers {
            worker.join().expect("join worker");
        }
        let page = repository
            .page(ReplayQuery {
                limit: 1000,
                ..ReplayQuery::default()
            })
            .expect("read all events");
        assert_eq!(page.events.len(), 100);
        assert_eq!(
            page.events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            (1..=100).collect::<Vec<_>>()
        );
        drop(repository);
        fs::remove_dir_all(root).expect("remove test repository");
    }

    #[test]
    fn rotation_preserves_stream_and_expires_only_removed_exact_cursors() {
        let root = test_root("rotation");
        let repository = EventRepository::open_with_options(
            &root,
            "tethys",
            RepositoryOptions {
                rotate_bytes: 700,
                retained_files: 1,
            },
        )
        .expect("open repository");
        let first = repository
            .append(pending("cutex.session-a", 1))
            .expect("append first event");
        let mut latest = first.clone();
        let mut retained = first.clone();
        for value in 2..=8 {
            latest = repository
                .append(pending("cutex.session-a", value))
                .expect("append rotated event");
            if value == 7 {
                retained = latest.clone();
            }
        }
        assert_eq!(latest.stream_id, first.stream_id);
        assert_eq!(latest.sequence, 8);
        let continuation = repository
            .page(ReplayQuery {
                stream_id: Some(retained.stream_id.clone()),
                after: Some(retained.cursor),
                limit: 100,
                cutex_session_id: None,
            })
            .expect("recent cursor remains valid across local rotation");
        assert_eq!(continuation.events.len(), 1);
        assert_eq!(continuation.events[0].sequence, latest.sequence);
        let error = repository
            .page(ReplayQuery {
                stream_id: Some(first.stream_id.clone()),
                after: Some(first.cursor.clone()),
                limit: 100,
                cutex_session_id: None,
            })
            .expect_err("first cursor should have expired");
        let ReplayError::CursorExpired {
            earliest, latest, ..
        } = error
        else {
            panic!("expected cursor_expired");
        };
        assert!(earliest.expect("earliest boundary").sequence > first.sequence);
        assert_eq!(latest.expect("latest boundary").sequence, 8);
        let changed = repository
            .page(ReplayQuery {
                stream_id: Some(Uuid::new_v4().to_string()),
                after: Some(first.cursor),
                limit: 100,
                cutex_session_id: None,
            })
            .expect_err("foreign stream should fail");
        assert!(matches!(changed, ReplayError::StreamChanged { .. }));
        fs::remove_dir_all(root).expect("remove test repository");
    }

    #[test]
    fn page_and_subscription_have_no_local_replay_live_gap() {
        let root = test_root("subscription");
        let repository = EventRepository::open(&root, "tethys").expect("open repository");
        repository
            .append(pending("cutex.session-a", 1))
            .expect("append replay event");
        let subscription = repository
            .page_and_subscribe(
                ReplayQuery {
                    limit: 100,
                    ..ReplayQuery::default()
                },
                4,
            )
            .expect("open subscription");
        assert_eq!(subscription.page.events.len(), 1);
        let live = repository
            .append(pending("cutex.session-a", 2))
            .expect("append live event");
        let received = subscription
            .receiver
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("receive live event");
        assert_eq!(received.event_id, live.event_id);
        assert_eq!(received.sequence, subscription.page.checkpoint.sequence + 1);
        fs::remove_dir_all(root).expect("remove test repository");
    }

    #[test]
    fn external_append_requires_replay_for_an_existing_subscription() {
        let root = test_root("external-subscription-replay");
        let reader = EventRepository::open(&root, "tethys").expect("open reader repository");
        let writer = EventRepository::open(&root, "tethys").expect("open writer repository");
        reader
            .append(pending("cutex.session-a", 1))
            .expect("append replay event");
        let subscription = reader
            .page_and_subscribe(
                ReplayQuery {
                    limit: 100,
                    ..ReplayQuery::default()
                },
                4,
            )
            .expect("open subscription");
        let external = writer
            .append(pending("cutex.session-a", 2))
            .expect("append through external repository");
        assert!(matches!(
            subscription
                .receiver
                .recv_timeout(std::time::Duration::from_millis(25)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));

        let replay = reader
            .page(ReplayQuery {
                stream_id: Some(subscription.page.stream_id),
                after: subscription.page.next_cursor,
                limit: 100,
                cutex_session_id: None,
            })
            .expect("replay external append");
        assert_eq!(replay.events.len(), 1);
        assert_eq!(replay.events[0].event_id, external.event_id);
        fs::remove_dir_all(root).expect("remove test repository");
    }

    #[test]
    fn corrupt_state_rotates_instead_of_guessing_continuity() {
        let root = test_root("corrupt-state");
        let repository = EventRepository::open(&root, "tethys").expect("open repository");
        let first = repository
            .append(pending("cutex.session-a", 1))
            .expect("append first event");
        fs::write(root.join(STATE_FILE_NAME), b"{not-json\n").expect("corrupt state");
        let reopened = EventRepository::open(&root, "tethys").expect("reopen repository");
        let reset = reopened
            .recovery_reset()
            .expect("read reset")
            .expect("recovery reset");
        assert_eq!(
            reset.previous_stream_id.as_deref(),
            Some(first.stream_id.as_str())
        );
        assert_ne!(reset.stream_id, first.stream_id);
        fs::remove_dir_all(root).expect("remove test repository");
    }

    #[cfg(unix)]
    #[test]
    fn repository_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let root = test_root("permissions");
        let repository = EventRepository::open(&root, "tethys").expect("open repository");
        repository
            .append(pending("cutex.session-a", 1))
            .expect("append event");
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for path in [
            root.join(LOCK_FILE_NAME),
            root.join(STATE_FILE_NAME),
            root.join(EVENTS_FILE_NAME),
        ] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(root).expect("remove test repository");
    }
}
