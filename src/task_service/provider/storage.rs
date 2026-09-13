//! Incremental Task Service storage. SQLite commits live aggregates, exact
//! receipts, and a compact watch event atomically. Immutable JSON nodes share
//! growing attempt history between the live record and historical receipts.
use super::*;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashMap;

pub(super) const DATABASE_FILE: &str = "task-service.sqlite3";
const LEGACY_STORE_FILE: &str = "task-service-provider-v2.json";
const LEGACY_JOURNAL_FILE: &str = "task-service-provider-v2.events.jsonl";
const FORMAT_VERSION: i64 = 1;
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_NODE_CACHE: usize = 32_768;

fn read_current<T: DeserializeOwned>(
    conn: &Connection,
    collection: &str,
    key: &str,
) -> Result<Option<T>, ProviderError> {
    current_root(conn, collection, key)?
        .map(|root| {
            Decoder::new(conn, Instant::now() + MAX_QUERY_DURATION, &mut || false).typed(&root)
        })
        .transpose()
}

type NodeId = String;

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Node {
    Scalar(Value),
    Object(BTreeMap<String, NodeId>),
    Array { len: usize, root: Option<NodeId> },
    Pair(NodeId, NodeId),
}

#[derive(Serialize, Deserialize)]
struct Metadata {
    schema: ProviderStoreSchema,
    sequence: u64,
    event_sha256: Sha256,
}

#[derive(Serialize, Deserialize)]
struct Change {
    collection: String,
    key: String,
    root: Option<NodeId>,
}

pub(super) struct Store {
    root: PathBuf,
}

impl Store {
    /// Does not create directories, a DB, or sidecars. An old-format store must
    /// be explicitly reset at cutover; its presence is never silently ignored.
    pub(super) fn open(root: &Path) -> Result<Self, ProviderError> {
        let store = Self { root: root.into() };
        store.check_path()?;
        Ok(store)
    }

    fn check_path(&self) -> Result<bool, ProviderError> {
        let exists = match fs::symlink_metadata(self.root.join(DATABASE_FILE)) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                metadata.len() > 0
            }
            Ok(_) => return Err(ProviderError::InvalidStore),
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        if !exists {
            for name in [LEGACY_STORE_FILE, LEGACY_JOURNAL_FILE] {
                if self.root.join(name).try_exists()? {
                    return Err(ProviderError::Conflict("legacy_task_store_requires_reset"));
                }
            }
        }
        Ok(exists)
    }

    /// Publish a complete empty database atomically. An interrupted first
    /// initialization leaves only a disposable temporary file, never a named
    /// half-schema database that makes every future read fail.
    fn create_database(&self) -> Result<(), ProviderError> {
        prepare_private_root(&self.root)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        set_private_open_options(&mut options);
        let lock = options.open(self.root.join("task-service-init.lock"))?;
        let deadline = Instant::now() + BUSY_TIMEOUT;
        loop {
            match lock.try_lock_exclusive() {
                Ok(()) => break,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    return Err(ProviderError::PersistenceUnavailable)
                }
                Err(error) => return Err(error.into()),
            }
        }
        if self.check_path()? {
            return Ok(());
        }
        // A zero-byte marker cannot contain committed SQLite data. It may be
        // left by a first-open interruption in an earlier initializer.
        match fs::remove_file(self.root.join(DATABASE_FILE)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let temp = self
            .root
            .join(format!(".task-service-init-{}.sqlite3", Uuid::new_v4()));
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            set_private_open_options(&mut options);
            options.open(&temp)?;
            let mut conn = Connection::open_with_flags(
                &temp,
                OpenFlags::SQLITE_OPEN_READ_WRITE
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX
                    | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            )
            .map_err(sql_error)?;
            conn.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;")
                .map_err(sql_error)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            tx.execute_batch(
                "CREATE TABLE metadata (singleton INTEGER PRIMARY KEY CHECK(singleton=1), format INTEGER NOT NULL, body BLOB NOT NULL);
                 CREATE TABLE nodes (hash TEXT PRIMARY KEY, body BLOB NOT NULL) WITHOUT ROWID;
                 CREATE TABLE current (collection TEXT NOT NULL, key TEXT NOT NULL, root TEXT NOT NULL, PRIMARY KEY(collection,key)) WITHOUT ROWID;
                 CREATE TABLE receipts (action_id TEXT PRIMARY KEY, root TEXT NOT NULL) WITHOUT ROWID;
                 CREATE TABLE events (sequence INTEGER PRIMARY KEY, event_sha256 TEXT NOT NULL, previous_sha256 TEXT NOT NULL, operation TEXT NOT NULL, occurred_at TEXT NOT NULL, changes BLOB NOT NULL);"
            ).map_err(sql_error)?;
            let initial = Metadata {
                schema: ProviderStoreSchema::V2,
                sequence: 0,
                event_sha256: Sha256::new(ZERO_SHA256).unwrap(),
            };
            tx.execute(
                "INSERT INTO metadata VALUES(1,?1,?2)",
                params![FORMAT_VERSION, json_bytes(&initial)?],
            )
            .map_err(sql_error)?;
            tx.commit().map_err(sql_error)?;
            drop(conn);
            File::open(&temp)?.sync_all()?;
            crash_point("initialize-before-publish");
            fs::hard_link(&temp, self.root.join(DATABASE_FILE))?;
            #[cfg(unix)]
            File::open(&self.root)?.sync_all()?;
            Ok(())
        })();
        let _ = fs::remove_file(&temp);
        result
    }

    fn connection(&self, write: bool) -> Result<Option<Connection>, ProviderError> {
        if !self.check_path()? {
            if !write {
                return Ok(None);
            }
            self.create_database()?;
        }
        let flags = if write {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        } | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let conn =
            Connection::open_with_flags(self.root.join(DATABASE_FILE), flags).map_err(sql_error)?;
        conn.busy_timeout(BUSY_TIMEOUT).map_err(sql_error)?;
        if write {
            conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON; PRAGMA wal_autocheckpoint=1000;").map_err(sql_error)?;
        }
        let format: i64 = conn
            .query_row("SELECT format FROM metadata WHERE singleton=1", [], |row| {
                row.get(0)
            })
            .map_err(sql_error)?;
        if format != FORMAT_VERSION {
            return Err(ProviderError::InvalidStore);
        }
        Ok(Some(conn))
    }

    pub(super) fn initialize(&self) -> Result<(), ProviderError> {
        self.connection(true)?;
        Ok(())
    }

    pub(super) fn load_live(&self) -> Result<TaskServiceSnapshot, ProviderError> {
        self.load(false, Instant::now() + MAX_QUERY_DURATION, &mut || false)
    }

    pub(super) fn load_full(&self) -> Result<TaskServiceSnapshot, ProviderError> {
        self.load(true, Instant::now() + MAX_QUERY_DURATION, &mut || false)
    }

    pub(super) fn load_live_cancellable(
        &self,
        deadline: Instant,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<TaskServiceSnapshot, ProviderError> {
        self.load(false, deadline, cancelled)
    }

    pub(super) fn load_full_cancellable(
        &self,
        deadline: Instant,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<TaskServiceSnapshot, ProviderError> {
        self.load(true, deadline, cancelled)
    }

    fn load(
        &self,
        full: bool,
        deadline: Instant,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<TaskServiceSnapshot, ProviderError> {
        check_deadline(deadline, cancelled)?;
        let Some(mut conn) = self.connection(false)? else {
            return Ok(TaskServiceSnapshot::empty());
        };
        let tx = conn.transaction().map_err(sql_error)?;
        let meta = metadata(&tx)?;
        let mut state = TaskServiceSnapshot::empty();
        state.schema = meta.schema;
        state.journal_sequence = meta.sequence;
        state.journal_sha256 = meta.event_sha256;
        let mut decoder = Decoder::new(&tx, deadline, cancelled);
        let mut query = tx
            .prepare("SELECT collection,key,root FROM current ORDER BY collection,key")
            .map_err(sql_error)?;
        let mut rows = query.query([]).map_err(sql_error)?;
        while let Some(row) = rows.next().map_err(sql_error)? {
            let collection: String = row.get(0).map_err(sql_error)?;
            let key: String = row.get(1).map_err(sql_error)?;
            let root: String = row.get(2).map_err(sql_error)?;
            match collection.as_str() {
                "task_revisions" => {
                    let (id, revision): (TaskId, TaskRevision) = decode_key(&key)?;
                    state
                        .task_revisions
                        .entry(id)
                        .or_default()
                        .insert(revision, decoder.typed(&root)?);
                }
                "attempts" => {
                    let (id, number): (AssignmentId, AttemptNumber) = decode_key(&key)?;
                    state
                        .attempts
                        .entry(id)
                        .or_default()
                        .insert(number, decoder.typed(&root)?);
                }
                "assignments" => {
                    state
                        .assignments
                        .insert(decode_key(&key)?, decoder.typed(&root)?);
                }
                "send_attempts" => {
                    state
                        .send_attempts
                        .insert(decode_key(&key)?, decoder.typed(&root)?);
                }
                "completion_notifications" => {
                    state
                        .completion_notifications
                        .insert(decode_key(&key)?, decoder.typed(&root)?);
                }
                "worker_followup_notifications" => {
                    state
                        .worker_followup_notifications
                        .insert(decode_key(&key)?, decoder.typed(&root)?);
                }
                "workflows" => {
                    state
                        .workflows
                        .insert(decode_key(&key)?, decoder.typed(&root)?);
                }
                "prepared_worker_actions" => {
                    state
                        .prepared_worker_actions
                        .insert(decode_key(&key)?, decoder.typed(&root)?);
                }
                _ => return Err(ProviderError::InvalidStore),
            }
        }
        if full {
            let mut query = tx
                .prepare("SELECT action_id,root FROM receipts ORDER BY action_id")
                .map_err(sql_error)?;
            let mut rows = query.query([]).map_err(sql_error)?;
            while let Some(row) = rows.next().map_err(sql_error)? {
                let id: String = row.get(0).map_err(sql_error)?;
                let root: String = row.get(1).map_err(sql_error)?;
                state.receipts.insert(
                    ActionId::new(id).map_err(|_| ProviderError::InvalidStore)?,
                    decoder.typed(&root)?,
                );
            }
        }
        check_deadline(deadline, decoder.cancelled)?;
        validate_state(&state)?;
        Ok(state)
    }

    pub(super) fn active_assignments(&self) -> Result<Vec<Assignment>, ProviderError> {
        let Some(conn) = self.connection(false)? else {
            return Ok(Vec::new());
        };
        let mut query = conn
            .prepare("SELECT root FROM current WHERE collection='assignments' ORDER BY key")
            .map_err(sql_error)?;
        let mut rows = query.query([]).map_err(sql_error)?;
        let mut cancelled = || false;
        let mut decoder = Decoder::new(&conn, Instant::now() + MAX_QUERY_DURATION, &mut cancelled);
        let mut result = Vec::new();
        while let Some(row) = rows.next().map_err(sql_error)? {
            let assignment: Assignment =
                decoder.typed(&row.get::<_, String>(0).map_err(sql_error)?)?;
            if assignment.state != AssignmentState::Closed {
                result.push(assignment);
            }
        }
        Ok(result)
    }

    pub(super) fn mutation_snapshot(
        &self,
        ids: &[AssignmentId],
        action: Option<&ActionId>,
    ) -> Result<TaskServiceSnapshot, ProviderError> {
        let mut state = TaskServiceSnapshot::empty();
        for id in ids {
            let current = self.assignment_snapshot(id)?;
            state.schema = current.schema;
            state.journal_sequence = current.journal_sequence;
            state.journal_sha256 = current.journal_sha256;
            state.assignments.extend(current.assignments);
            state.attempts.extend(current.attempts);
            state.workflows.extend(current.workflows);
            for (id, revisions) in current.task_revisions {
                state
                    .task_revisions
                    .entry(id)
                    .or_default()
                    .extend(revisions);
            }
        }
        if let Some(action) = action {
            if let Some(prepared) = self.prepared(action)? {
                state
                    .prepared_worker_actions
                    .insert(action.clone(), prepared);
            }
        }
        validate_state(&state)?;
        Ok(state)
    }

    /// Count only executable preparations using small identity/phase fields;
    /// never decode status/result arrays from unrelated active attempts.
    pub(super) fn active_prepared_count(&self) -> Result<usize, ProviderError> {
        let Some(conn) = self.connection(false)? else {
            return Ok(0);
        };
        let mut query = conn
            .prepare("SELECT root FROM current WHERE collection='prepared_worker_actions'")
            .map_err(sql_error)?;
        let mut rows = query.query([]).map_err(sql_error)?;
        let mut cancelled = || false;
        let mut decoder = Decoder::new(&conn, Instant::now() + MAX_QUERY_DURATION, &mut cancelled);
        let mut count = 0;
        let mut assignments = BTreeMap::new();
        while let Some(row) = rows.next().map_err(sql_error)? {
            let prepared: PreparedWorkerAction =
                decoder.typed(&row.get::<_, String>(0).map_err(sql_error)?)?;
            if !assignments.contains_key(&prepared.assignment_id) {
                assignments.insert(
                    prepared.assignment_id.clone(),
                    read_current::<Assignment>(
                        &conn,
                        "assignments",
                        &key(&prepared.assignment_id)?,
                    )?,
                );
            }
            let Some(assignment) = assignments[&prepared.assignment_id].as_ref() else {
                continue;
            };
            if assignment.state == AssignmentState::Closed
                || assignment.assignee_cutex_session != prepared.authenticated_cutex_session
            {
                continue;
            }
            let active = match prepared.attempt_binding {
                None => matches!(
                    assignment.state,
                    AssignmentState::AwaitingAck | AssignmentState::RetryPending
                ),
                Some(binding) if assignment.active_attempt == Some(binding.attempt_number) => {
                    let Some(root) = current_root(
                        &conn,
                        "attempts",
                        &key(&(&prepared.assignment_id, binding.attempt_number))?,
                    )?
                    else {
                        return Err(ProviderError::InvalidStore);
                    };
                    let Node::Object(fields) = read_node(&conn, &root)? else {
                        return Err(ProviderError::InvalidStore);
                    };
                    let token: ProviderAttemptToken = decoder.typed(
                        fields
                            .get("attempt_token")
                            .ok_or(ProviderError::InvalidStore)?,
                    )?;
                    let phase: AttemptPhase =
                        decoder.typed(fields.get("phase").ok_or(ProviderError::InvalidStore)?)?;
                    token == binding.attempt_token
                        && matches!(
                            phase,
                            AttemptPhase::Running
                                | AttemptPhase::Blocked
                                | AttemptPhase::ReviewReady
                        )
                }
                Some(_) => false,
            };
            if active {
                count += 1;
            }
        }
        Ok(count)
    }

    pub(super) fn aggregate<T: DeserializeOwned>(
        &self,
        collection: &str,
        id: &impl Serialize,
    ) -> Result<Option<T>, ProviderError> {
        let Some(conn) = self.connection(false)? else {
            return Ok(None);
        };
        read_current(&conn, collection, &key(id)?)
    }

    pub(super) fn prepared(
        &self,
        action: &ActionId,
    ) -> Result<Option<PreparedWorkerAction>, ProviderError> {
        let Some(conn) = self.connection(false)? else {
            return Ok(None);
        };
        read_current(&conn, "prepared_worker_actions", &key(action)?)
    }

    pub(super) fn assignment_snapshot(
        &self,
        id: &AssignmentId,
    ) -> Result<TaskServiceSnapshot, ProviderError> {
        let Some(mut conn) = self.connection(false)? else {
            return Ok(TaskServiceSnapshot::empty());
        };
        let tx = conn.transaction().map_err(sql_error)?;
        let meta = metadata(&tx)?;
        let mut state = TaskServiceSnapshot::empty();
        state.schema = meta.schema;
        state.journal_sequence = meta.sequence;
        state.journal_sha256 = meta.event_sha256;
        let Some(assignment): Option<Assignment> = read_current(&tx, "assignments", &key(id)?)?
        else {
            return Ok(state);
        };
        if let Some(revision) = read_current::<TaskRevisionRecord>(
            &tx,
            "task_revisions",
            &key(&(&assignment.task_id, assignment.task_revision))?,
        )? {
            if let Some(workflow) =
                read_current::<Workflow>(&tx, "workflows", &key(&revision.workflow_id)?)?
            {
                state
                    .workflows
                    .insert(revision.workflow_id.clone(), workflow);
            }
            state
                .task_revisions
                .entry(assignment.task_id.clone())
                .or_default()
                .insert(assignment.task_revision, revision);
        }
        // Composite JSON keys start with the exact serialized assignment ID.
        // A bounded key range uses the current table's primary key index.
        let prefix = format!("[{},", key(id)?);
        let upper = format!("{}\u{10ffff}", prefix);
        let mut query = tx.prepare("SELECT key,root FROM current WHERE collection='attempts' AND key>=?1 AND key<?2 ORDER BY key").map_err(sql_error)?;
        let mut rows = query.query(params![prefix, upper]).map_err(sql_error)?;
        let mut cancelled = || false;
        let mut decoder = Decoder::new(&tx, Instant::now() + MAX_QUERY_DURATION, &mut cancelled);
        while let Some(row) = rows.next().map_err(sql_error)? {
            let (owner, number): (AssignmentId, AttemptNumber) =
                decode_key(&row.get::<_, String>(0).map_err(sql_error)?)?;
            let attempt = decoder.typed(&row.get::<_, String>(1).map_err(sql_error)?)?;
            state
                .attempts
                .entry(owner)
                .or_default()
                .insert(number, attempt);
        }
        state.assignments.insert(id.clone(), assignment);
        validate_state(&state)?;
        Ok(state)
    }

    pub(super) fn receipt(
        &self,
        action: &ActionId,
    ) -> Result<Option<ProviderReceipt>, ProviderError> {
        let Some(mut conn) = self.connection(false)? else {
            return Ok(None);
        };
        let tx = conn.transaction().map_err(sql_error)?;
        let root: Option<String> = tx
            .query_row(
                "SELECT root FROM receipts WHERE action_id=?1",
                [action.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_error)?;
        root.map(|root| {
            Decoder::new(&tx, Instant::now() + MAX_QUERY_DURATION, &mut || false).typed(&root)
        })
        .transpose()
    }

    pub(super) fn watch(
        &self,
        after_sequence: u64,
        limit: usize,
    ) -> Result<Vec<WatchEvent>, ProviderError> {
        if limit == 0 || limit > MAX_WATCH_LIMIT {
            return Err(ProviderError::InvalidRequest("watch_limit"));
        }
        let Some(conn) = self.connection(false)? else {
            return Ok(Vec::new());
        };
        let mut query = conn.prepare("SELECT sequence,event_sha256,operation,occurred_at FROM events WHERE sequence>?1 ORDER BY sequence LIMIT ?2").map_err(sql_error)?;
        let mut rows = query
            .query(params![
                i64::try_from(after_sequence)
                    .map_err(|_| ProviderError::InvalidRequest("watch_sequence"))?,
                limit as i64
            ])
            .map_err(sql_error)?;
        let mut events = Vec::new();
        while let Some(row) = rows.next().map_err(sql_error)? {
            events.push(WatchEvent {
                sequence: u64::try_from(row.get::<_, i64>(0).map_err(sql_error)?)
                    .map_err(|_| ProviderError::InvalidStore)?,
                event_sha256: Sha256::new(row.get::<_, String>(1).map_err(sql_error)?)
                    .map_err(|_| ProviderError::InvalidStore)?,
                operation: row.get(2).map_err(sql_error)?,
                occurred_at: Rfc3339::new(row.get::<_, String>(3).map_err(sql_error)?)
                    .map_err(|_| ProviderError::InvalidStore)?,
            });
        }
        Ok(events)
    }

    pub(super) fn commit(
        &self,
        before: &TaskServiceSnapshot,
        after: &mut TaskServiceSnapshot,
        operation: &str,
        occurred_at: Rfc3339,
    ) -> Result<(), ProviderError> {
        validate_state(after)?;
        let mut conn = self
            .connection(true)?
            .ok_or(ProviderError::PersistenceUnavailable)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let meta = metadata(&tx)?;
        if meta.sequence != before.journal_sequence || meta.event_sha256 != before.journal_sha256 {
            return Err(ProviderError::Conflict("storage_revision_conflict"));
        }
        let sequence = meta
            .sequence
            .checked_add(1)
            .filter(|n| *n <= MAX_JSON_SAFE_INTEGER)
            .ok_or(ProviderError::InvalidStore)?;
        let mut encoder = Encoder { conn: &tx };
        let mut changes = Vec::new();
        sync_map(
            &mut encoder,
            "assignments",
            None::<&str>,
            &before.assignments,
            &after.assignments,
            &mut changes,
        )?;
        sync_map(
            &mut encoder,
            "send_attempts",
            None::<&str>,
            &before.send_attempts,
            &after.send_attempts,
            &mut changes,
        )?;
        sync_map(
            &mut encoder,
            "completion_notifications",
            None::<&str>,
            &before.completion_notifications,
            &after.completion_notifications,
            &mut changes,
        )?;
        sync_map(
            &mut encoder,
            "worker_followup_notifications",
            None::<&str>,
            &before.worker_followup_notifications,
            &after.worker_followup_notifications,
            &mut changes,
        )?;
        sync_map(
            &mut encoder,
            "workflows",
            None::<&str>,
            &before.workflows,
            &after.workflows,
            &mut changes,
        )?;
        sync_map(
            &mut encoder,
            "prepared_worker_actions",
            None::<&str>,
            &before.prepared_worker_actions,
            &after.prepared_worker_actions,
            &mut changes,
        )?;
        for id in before
            .task_revisions
            .keys()
            .chain(after.task_revisions.keys())
            .collect::<std::collections::BTreeSet<_>>()
        {
            sync_map(
                &mut encoder,
                "task_revisions",
                Some(id),
                before.task_revisions.get(id).unwrap_or(&BTreeMap::new()),
                after.task_revisions.get(id).unwrap_or(&BTreeMap::new()),
                &mut changes,
            )?;
        }
        for id in before
            .attempts
            .keys()
            .chain(after.attempts.keys())
            .collect::<std::collections::BTreeSet<_>>()
        {
            let old = before.attempts.get(id);
            let new = after.attempts.get(id);
            if let Some(new) = new {
                for (number, attempt) in new {
                    let previous = old.and_then(|map| map.get(number));
                    if previous == Some(attempt) {
                        continue;
                    }
                    let key = key(&(id, number))?;
                    let previous_root = current_root(&tx, "attempts", &key)?;
                    let root = encoder.attempt(attempt, previous, previous_root.as_deref())?;
                    put_current(&tx, "attempts", &key, &root, &mut changes)?;
                }
            }
            if let Some(old) = old {
                for number in old
                    .keys()
                    .filter(|number| new.is_none_or(|map| !map.contains_key(number)))
                {
                    delete_current(&tx, "attempts", &key(&(id, number))?, &mut changes)?;
                }
            }
        }
        // Receipts are immutable and omitted from load_live. Absence from either
        // in-memory snapshot never means deletion from this index.
        for (action, receipt) in &after.receipts {
            if before.receipts.get(action) == Some(receipt) {
                continue;
            }
            let root = encoder.receipt(receipt, after)?;
            let existing: Option<String> = tx
                .query_row(
                    "SELECT root FROM receipts WHERE action_id=?1",
                    [action.as_str()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(sql_error)?;
            match existing {
                Some(existing) if existing != root => {
                    return Err(ProviderError::Conflict("action_id_payload_conflict"))
                }
                Some(_) => {}
                None => {
                    tx.execute(
                        "INSERT INTO receipts VALUES(?1,?2)",
                        params![action.as_str(), root],
                    )
                    .map_err(sql_error)?;
                    changes.push(Change {
                        collection: "receipts".into(),
                        key: action.as_str().into(),
                        root: Some(root),
                    });
                }
            }
        }
        crash_point("commit-before-finish");
        let digest = hex_sha256(&json_bytes(&(
            "cutex/task-store-delta/v1",
            sequence,
            &meta.event_sha256,
            after.schema,
            operation,
            &occurred_at,
            &changes,
        ))?);
        tx.execute(
            "INSERT INTO events VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                sequence as i64,
                digest.as_str(),
                meta.event_sha256.as_str(),
                operation,
                occurred_at.as_str(),
                json_bytes(&changes)?
            ],
        )
        .map_err(sql_error)?;
        let next = Metadata {
            schema: after.schema,
            sequence,
            event_sha256: digest.clone(),
        };
        tx.execute(
            "UPDATE metadata SET body=?1 WHERE singleton=1",
            [json_bytes(&next)?],
        )
        .map_err(sql_error)?;
        tx.commit().map_err(sql_error)?;
        after.journal_sequence = sequence;
        after.journal_sha256 = digest;
        Ok(())
    }
}

#[cfg(test)]
fn crash_point(name: &str) {
    if std::env::var("CUTEX_STORAGE_CRASH_POINT").as_deref() == Ok(name) {
        std::process::exit(91);
    }
}
#[cfg(not(test))]
fn crash_point(_: &str) {}

fn sql_error(error: rusqlite::Error) -> ProviderError {
    match error {
        rusqlite::Error::SqliteFailure(inner, _)
            if matches!(
                inner.code,
                rusqlite::ErrorCode::DatabaseBusy
                    | rusqlite::ErrorCode::DatabaseLocked
                    | rusqlite::ErrorCode::OperationInterrupted
            ) =>
        {
            ProviderError::PersistenceUnavailable
        }
        rusqlite::Error::SqliteFailure(inner, _)
            if matches!(
                inner.code,
                rusqlite::ErrorCode::DiskFull
                    | rusqlite::ErrorCode::SystemIoFailure
                    | rusqlite::ErrorCode::CannotOpen
            ) =>
        {
            ProviderError::PersistenceUnavailable
        }
        _ => ProviderError::InvalidStore,
    }
}

fn json_bytes(value: &impl Serialize) -> Result<Vec<u8>, ProviderError> {
    serde_json::to_vec(value).map_err(|_| ProviderError::InvalidStore)
}
fn key(value: &impl Serialize) -> Result<String, ProviderError> {
    serde_json::to_string(value).map_err(|_| ProviderError::InvalidStore)
}
fn decode_key<T: DeserializeOwned>(value: &str) -> Result<T, ProviderError> {
    serde_json::from_str(value).map_err(|_| ProviderError::InvalidStore)
}
fn metadata(conn: &Connection) -> Result<Metadata, ProviderError> {
    let bytes: Vec<u8> = conn
        .query_row("SELECT body FROM metadata WHERE singleton=1", [], |row| {
            row.get(0)
        })
        .map_err(sql_error)?;
    let meta: Metadata = serde_json::from_slice(&bytes).map_err(|_| ProviderError::InvalidStore)?;
    if meta.sequence > MAX_JSON_SAFE_INTEGER {
        return Err(ProviderError::InvalidStore);
    }
    Ok(meta)
}
fn current_root(
    conn: &Connection,
    collection: &str,
    key: &str,
) -> Result<Option<NodeId>, ProviderError> {
    conn.query_row(
        "SELECT root FROM current WHERE collection=?1 AND key=?2",
        params![collection, key],
        |row| row.get(0),
    )
    .optional()
    .map_err(sql_error)
}
fn put_current(
    conn: &Connection,
    collection: &str,
    key: &str,
    root: &str,
    changes: &mut Vec<Change>,
) -> Result<(), ProviderError> {
    conn.execute("INSERT INTO current VALUES(?1,?2,?3) ON CONFLICT(collection,key) DO UPDATE SET root=excluded.root", params![collection,key,root]).map_err(sql_error)?;
    changes.push(Change {
        collection: collection.into(),
        key: key.into(),
        root: Some(root.into()),
    });
    Ok(())
}
fn delete_current(
    conn: &Connection,
    collection: &str,
    key: &str,
    changes: &mut Vec<Change>,
) -> Result<(), ProviderError> {
    conn.execute(
        "DELETE FROM current WHERE collection=?1 AND key=?2",
        params![collection, key],
    )
    .map_err(sql_error)?;
    changes.push(Change {
        collection: collection.into(),
        key: key.into(),
        root: None,
    });
    Ok(())
}
fn sync_map<K: Ord + Serialize, V: Serialize + PartialEq, P: Serialize>(
    encoder: &mut Encoder<'_>,
    collection: &str,
    prefix: Option<P>,
    before: &BTreeMap<K, V>,
    after: &BTreeMap<K, V>,
    changes: &mut Vec<Change>,
) -> Result<(), ProviderError> {
    let encode_key = |k: &K| match &prefix {
        Some(prefix) => key(&(prefix, k)),
        None => key(k),
    };
    for (k, value) in after {
        if before.get(k) == Some(value) {
            continue;
        }
        let root = encoder.typed(value)?;
        let encoded_key = encode_key(k)?;
        if !before.contains_key(k)
            && current_root(encoder.conn, collection, &encoded_key)?.is_some()
        {
            return Err(ProviderError::Conflict("aggregate_exists"));
        }
        put_current(encoder.conn, collection, &encoded_key, &root, changes)?;
    }
    for k in before.keys().filter(|k| !after.contains_key(k)) {
        delete_current(encoder.conn, collection, &encode_key(k)?, changes)?;
    }
    Ok(())
}

struct Encoder<'a> {
    conn: &'a Connection,
}
impl Encoder<'_> {
    fn put(&self, node: &Node) -> Result<NodeId, ProviderError> {
        let body = json_bytes(node)?;
        let hash = hex_sha256(&body).as_str().to_string();
        self.conn
            .prepare_cached("INSERT OR IGNORE INTO nodes VALUES(?1,?2)")
            .map_err(sql_error)?
            .execute(params![hash, body])
            .map_err(sql_error)?;
        Ok(hash)
    }
    fn get(&self, id: &str) -> Result<Node, ProviderError> {
        read_node(self.conn, id)
    }
    fn typed(&mut self, value: &impl Serialize) -> Result<NodeId, ProviderError> {
        self.value(serde_json::to_value(value).map_err(|_| ProviderError::InvalidStore)?)
    }
    fn value(&mut self, value: Value) -> Result<NodeId, ProviderError> {
        match value {
            Value::Object(values) => {
                let mut fields = BTreeMap::new();
                for (key, value) in values {
                    fields.insert(key, self.value(value)?);
                }
                self.put(&Node::Object(fields))
            }
            Value::Array(values) => {
                let len = values.len();
                let mut root = None;
                for (index, value) in values.into_iter().enumerate() {
                    let item = self.value(value)?;
                    root = Some(self.append(root, index, item)?);
                }
                self.put(&Node::Array { len, root })
            }
            scalar => self.put(&Node::Scalar(scalar)),
        }
    }
    /// Persistent power-of-two prefix tree: appending one item changes at most
    /// log2(length) pair nodes, retaining every historical prefix without copies.
    fn append(
        &mut self,
        root: Option<NodeId>,
        len: usize,
        item: NodeId,
    ) -> Result<NodeId, ProviderError> {
        if len == 0 {
            return if root.is_none() {
                Ok(item)
            } else {
                Err(ProviderError::InvalidStore)
            };
        }
        let root = root.ok_or(ProviderError::InvalidStore)?;
        if len.is_power_of_two() {
            return self.put(&Node::Pair(root, item));
        }
        let left_len = 1usize << (usize::BITS - 1 - len.leading_zeros());
        let Node::Pair(left, right) = self.get(&root)? else {
            return Err(ProviderError::InvalidStore);
        };
        let right = self.append(Some(right), len - left_len, item)?;
        self.put(&Node::Pair(left, right))
    }
    fn vector<T: Serialize + PartialEq>(
        &mut self,
        after: &[T],
        before: Option<&[T]>,
        previous_root: Option<&NodeId>,
    ) -> Result<NodeId, ProviderError> {
        let (mut len, mut root) = match (before, previous_root) {
            (Some(before), Some(previous)) if after.starts_with(before) => {
                match self.get(previous)? {
                    Node::Array { len, root } if len == before.len() => (len, root),
                    _ => return Err(ProviderError::InvalidStore),
                }
            }
            _ => (0, None),
        };
        for value in &after[len..] {
            let item = self.typed(value)?;
            root = Some(self.append(root, len, item)?);
            len += 1;
        }
        self.put(&Node::Array { len, root })
    }
    fn attempt(
        &mut self,
        after: &Attempt,
        before: Option<&Attempt>,
        previous_root: Option<&str>,
    ) -> Result<NodeId, ProviderError> {
        #[derive(Serialize)]
        struct Head<'a> {
            #[serde(skip_serializing_if = "Option::is_none")]
            project_id: &'a Option<crate::agent_management::ProjectId>,
            assignment_id: &'a AssignmentId,
            attempt_number: AttemptNumber,
            attempt_token: &'a ProviderAttemptToken,
            phase: AttemptPhase,
            local_revision: u64,
            started_at: &'a Rfc3339,
            updated_at: &'a Rfc3339,
            terminal_action_id: &'a Option<ActionId>,
        }
        let head = Head {
            project_id: &after.project_id,
            assignment_id: &after.assignment_id,
            attempt_number: after.attempt_number,
            attempt_token: &after.attempt_token,
            phase: after.phase,
            local_revision: after.local_revision,
            started_at: &after.started_at,
            updated_at: &after.updated_at,
            terminal_action_id: &after.terminal_action_id,
        };
        let head_root = self.typed(&head)?;
        let Node::Object(mut fields) = self.get(&head_root)? else {
            return Err(ProviderError::InvalidStore);
        };
        let previous = match previous_root {
            Some(id) => match self.get(id)? {
                Node::Object(fields) => fields,
                _ => return Err(ProviderError::InvalidStore),
            },
            None => BTreeMap::new(),
        };
        fields.insert(
            "status_receipts".into(),
            self.vector(
                &after.status_receipts,
                before.map(|a| a.status_receipts.as_slice()),
                previous.get("status_receipts"),
            )?,
        );
        fields.insert(
            "result_receipts".into(),
            self.vector(
                &after.result_receipts,
                before.map(|a| a.result_receipts.as_slice()),
                previous.get("result_receipts"),
            )?,
        );
        self.put(&Node::Object(fields))
    }
    fn receipt(
        &mut self,
        receipt: &ProviderReceipt,
        state: &TaskServiceSnapshot,
    ) -> Result<NodeId, ProviderError> {
        #[derive(Serialize)]
        struct Head<'a> {
            schema: ProviderReceiptSchema,
            action_id: &'a ActionId,
            request_sha256: &'a Sha256,
            attempt_binding: &'a Option<DurableAttemptBinding>,
            committed_at: &'a Rfc3339,
            journal_sequence: u64,
        }
        let head = Head {
            schema: receipt.schema,
            action_id: &receipt.action_id,
            request_sha256: &receipt.request_sha256,
            attempt_binding: &receipt.attempt_binding,
            committed_at: &receipt.committed_at,
            journal_sequence: receipt.journal_sequence,
        };
        let head_root = self.typed(&head)?;
        let Node::Object(mut fields) = self.get(&head_root)? else {
            return Err(ProviderError::InvalidStore);
        };
        let result = if let ProviderResult::Attempt(attempt) = &receipt.result {
            let body = if state
                .attempts
                .get(&attempt.assignment_id)
                .and_then(|map| map.get(&attempt.attempt_number))
                == Some(attempt)
            {
                current_root(
                    self.conn,
                    "attempts",
                    &key(&(&attempt.assignment_id, attempt.attempt_number))?,
                )?
                .ok_or(ProviderError::InvalidStore)?
            } else {
                self.attempt(attempt, None, None)?
            };
            let kind = self.typed(&"attempt")?;
            self.put(&Node::Object(BTreeMap::from([
                ("kind".into(), kind),
                ("body".into(), body),
            ])))?
        } else {
            self.typed(&receipt.result)?
        };
        fields.insert("result".into(), result);
        self.put(&Node::Object(fields))
    }
}

fn read_node(conn: &Connection, id: &str) -> Result<Node, ProviderError> {
    let body: Vec<u8> = conn
        .prepare_cached("SELECT body FROM nodes WHERE hash=?1")
        .map_err(sql_error)?
        .query_row([id], |row| row.get(0))
        .map_err(sql_error)?;
    if hex_sha256(&body).as_str() != id {
        return Err(ProviderError::InvalidStore);
    }
    serde_json::from_slice(&body).map_err(|_| ProviderError::InvalidStore)
}
fn check_deadline(
    deadline: Instant,
    cancelled: &mut dyn FnMut() -> bool,
) -> Result<(), ProviderError> {
    if Instant::now() >= deadline || cancelled() {
        Err(ProviderError::PersistenceUnavailable)
    } else {
        Ok(())
    }
}
struct Decoder<'a, 'c> {
    conn: &'a Connection,
    cache: HashMap<NodeId, Node>,
    deadline: Instant,
    cancelled: &'c mut dyn FnMut() -> bool,
}
impl<'a, 'c> Decoder<'a, 'c> {
    fn new(
        conn: &'a Connection,
        deadline: Instant,
        cancelled: &'c mut dyn FnMut() -> bool,
    ) -> Self {
        Self {
            conn,
            cache: HashMap::new(),
            deadline,
            cancelled,
        }
    }
    fn node(&mut self, id: &str) -> Result<Node, ProviderError> {
        check_deadline(self.deadline, self.cancelled)?;
        if let Some(node) = self.cache.get(id) {
            return Ok(node.clone());
        }
        let node = read_node(self.conn, id)?;
        if self.cache.len() >= MAX_NODE_CACHE {
            self.cache.clear();
        }
        self.cache.insert(id.into(), node.clone());
        Ok(node)
    }
    fn typed<T: DeserializeOwned>(&mut self, id: &str) -> Result<T, ProviderError> {
        serde_json::from_value(self.value(id, 0)?).map_err(|_| ProviderError::InvalidStore)
    }
    fn value(&mut self, id: &str, depth: usize) -> Result<Value, ProviderError> {
        if depth > 256 {
            return Err(ProviderError::InvalidStore);
        }
        match self.node(id)? {
            Node::Scalar(value) if !value.is_object() && !value.is_array() => Ok(value),
            Node::Object(fields) => {
                let mut result = serde_json::Map::new();
                for (key, id) in fields {
                    result.insert(key, self.value(&id, depth + 1)?);
                }
                Ok(Value::Object(result))
            }
            Node::Array { len, root } => {
                let mut result = Vec::new();
                if let Some(root) = root {
                    if len == 0 {
                        return Err(ProviderError::InvalidStore);
                    }
                    self.array(&root, len, &mut result, depth + 1)?;
                } else if len != 0 {
                    return Err(ProviderError::InvalidStore);
                }
                Ok(Value::Array(result))
            }
            _ => Err(ProviderError::InvalidStore),
        }
    }
    fn array(
        &mut self,
        root: &str,
        len: usize,
        values: &mut Vec<Value>,
        depth: usize,
    ) -> Result<(), ProviderError> {
        if depth > 256 || len == 0 {
            return Err(ProviderError::InvalidStore);
        }
        if len == 1 {
            values.push(self.value(root, depth + 1)?);
            return Ok(());
        }
        let Node::Pair(left, right) = self.node(root)? else {
            return Err(ProviderError::InvalidStore);
        };
        let left_len = if len.is_power_of_two() {
            len / 2
        } else {
            1usize << (usize::BITS - 1 - len.leading_zeros())
        };
        self.array(&left, left_len, values, depth + 1)?;
        self.array(&right, len - left_len, values, depth + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        root: PathBuf,
        store: Store,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("task-sqlite-{}", Uuid::new_v4()));
            let store = Store::open(&root).unwrap();
            Self { root, store }
        }
        fn initial(&self) -> TaskServiceSnapshot {
            let mut state = TaskServiceSnapshot::empty();
            let task = TaskId::new("storage-task").unwrap();
            let revision = TaskRevision::new(1).unwrap();
            let workflow = WorkflowId::new("workflow").unwrap();
            let id = AssignmentId::new("assignment").unwrap();
            let number = AttemptNumber::new(1).unwrap();
            state.workflows.insert(
                workflow.clone(),
                Workflow {
                    project_id: None,
                    workflow_id: workflow.clone(),
                    coordinator_seat_id: SeatId::new("director").unwrap(),
                    local_revision: 1,
                    execution_guard: WorkflowExecutionGuard::Open,
                },
            );
            state
                .task_revisions
                .entry(task.clone())
                .or_default()
                .insert(
                    revision,
                    TaskRevisionRecord {
                        project_id: None,
                        task_id: task.clone(),
                        task_revision: revision,
                        contract_sha256: hex_sha256(b"contract"),
                        opaque_contract: "contract".into(),
                        completion_policy: CompletionPolicy {
                            kind: CompletionPolicyKind::DirectorAcceptance,
                            authority_seat_id: SeatId::new("director").unwrap(),
                        },
                        workflow_id: workflow,
                        created_at: now(),
                        created_by_cutex_session: CutexSessionId::new("cutex.director").unwrap(),
                    },
                );
            state.assignments.insert(
                id.clone(),
                Assignment {
                    project_id: None,
                    assignment_id: id.clone(),
                    task_id: task,
                    task_revision: revision,
                    assignee_cutex_session: CutexSessionId::new("cutex.worker").unwrap(),
                    state: AssignmentState::Active,
                    local_revision: 1,
                    created_at: now(),
                    acknowledged_at: Some(now()),
                    active_attempt: Some(number),
                    retry_authorization: None,
                    closure: None,
                },
            );
            state.attempts.entry(id.clone()).or_default().insert(
                number,
                Attempt {
                    project_id: None,
                    assignment_id: id,
                    attempt_number: number,
                    attempt_token: ProviderAttemptToken::new("attempt-token").unwrap(),
                    phase: AttemptPhase::Running,
                    local_revision: 1,
                    started_at: now(),
                    updated_at: now(),
                    status_receipts: Vec::new(),
                    result_receipts: Vec::new(),
                    terminal_action_id: None,
                },
            );
            self.store
                .commit(&TaskServiceSnapshot::empty(), &mut state, "initial", now())
                .unwrap();
            state
        }
        fn add_status(&self, index: usize, body: &str) -> ProviderReceipt {
            let before = self.store.load_live().unwrap();
            let mut after = before.clone();
            let action = ActionId::new(format!("status-{index}")).unwrap();
            let timestamp = now();
            let attempt = after
                .attempts
                .values_mut()
                .next()
                .unwrap()
                .values_mut()
                .next()
                .unwrap();
            attempt.status_receipts.push(StatusReceipt {
                project_id: None,
                action_id: action.clone(),
                summary: format!("{index}:{body}"),
                evidence_sha256: None,
                recorded_at: timestamp.clone(),
            });
            attempt.local_revision += 1;
            attempt.updated_at = timestamp.clone();
            let receipt = ProviderReceipt {
                schema: ProviderReceiptSchema::V2,
                action_id: action.clone(),
                request_sha256: hex_sha256(action.as_str().as_bytes()),
                attempt_binding: Some(DurableAttemptBinding {
                    attempt_number: attempt.attempt_number,
                    attempt_token: attempt.attempt_token.clone(),
                }),
                committed_at: timestamp.clone(),
                journal_sequence: before.journal_sequence + 1,
                result: ProviderResult::Attempt(attempt.clone()),
            };
            after.receipts.insert(action, receipt.clone());
            self.store
                .commit(&before, &mut after, "report_status", timestamp)
                .unwrap();
            receipt
        }
        fn count(&self, table: &str) -> i64 {
            self.store
                .connection(false)
                .unwrap()
                .unwrap()
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap()
        }
    }

    #[test]
    fn absent_reads_are_no_write_and_legacy_store_requires_explicit_reset() {
        let f = Fixture::new();
        assert_eq!(f.store.load_live().unwrap(), TaskServiceSnapshot::empty());
        assert_eq!(f.store.load_full().unwrap(), TaskServiceSnapshot::empty());
        assert!(f
            .store
            .receipt(&ActionId::new("missing").unwrap())
            .unwrap()
            .is_none());
        assert!(f.store.watch(0, 10).unwrap().is_empty());
        assert!(!f.root.exists());
        fs::create_dir(&f.root).unwrap();
        fs::write(f.root.join(LEGACY_STORE_FILE), "{}").unwrap();
        assert!(matches!(
            Store::open(&f.root),
            Err(ProviderError::Conflict("legacy_task_store_requires_reset"))
        ));
        assert!(!f.root.join(DATABASE_FILE).exists());
    }

    #[test]
    fn growing_attempt_prefixes_and_exact_receipts_survive_reopen_without_quadratic_storage() {
        let f = Fixture::new();
        f.initial();
        let first = f.add_status(0, &"x".repeat(1024));
        let after_first_nodes = f.count("nodes");
        for index in 1..128 {
            f.add_status(index, &"x".repeat(1024));
        }
        let root_bytes = fs::read_dir(&f.root)
            .unwrap()
            .map(|p| p.unwrap().metadata().unwrap().len())
            .sum::<u64>();
        let reopened = Store::open(&f.root).unwrap();
        assert_eq!(reopened.receipt(&first.action_id).unwrap(), Some(first));
        let live = reopened.load_live().unwrap();
        assert!(live.receipts.is_empty());
        assert_eq!(
            live.attempts
                .values()
                .next()
                .unwrap()
                .values()
                .next()
                .unwrap()
                .status_receipts
                .len(),
            128
        );
        let full = reopened.load_full().unwrap();
        assert_eq!(full.receipts.len(), 128);
        let expanded = json_bytes(&full).unwrap().len() as u64;
        eprintln!(
            "task sqlite: directory={root_bytes} expanded_snapshot={expanded} nodes={}",
            f.count("nodes")
        );
        assert!(
            root_bytes * 2 < expanded,
            "shared storage must be substantially smaller than expanded receipt history"
        );
        // Each appended status stores a bounded scalar/object set plus <=log N
        // array tree nodes; no copy of earlier status payloads is persisted.
        assert!(f.count("nodes") - after_first_nodes < 128 * 45);
        let page = reopened.watch(125, 2).unwrap();
        assert_eq!(
            page.iter().map(|event| event.sequence).collect::<Vec<_>>(),
            vec![126, 127]
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(f.root.join(DATABASE_FILE))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn stale_cas_and_late_receipt_conflict_roll_back_every_table() {
        let f = Fixture::new();
        let initial = f.initial();
        let receipt = f.add_status(0, "one");
        let mut stale = initial.clone();
        assert_eq!(
            f.store.commit(&initial, &mut stale, "stale", now()),
            Err(ProviderError::Conflict("storage_revision_conflict"))
        );
        let before = f.store.load_live().unwrap();
        let nodes = f.count("nodes");
        let mut after = before.clone();
        after.workflows.values_mut().next().unwrap().local_revision += 1;
        let mut conflicting = receipt;
        conflicting.request_sha256 = hex_sha256(b"conflicting input");
        after
            .receipts
            .insert(conflicting.action_id.clone(), conflicting);
        assert_eq!(
            f.store.commit(&before, &mut after, "late-conflict", now()),
            Err(ProviderError::Conflict("action_id_payload_conflict"))
        );
        assert_eq!(f.store.load_live().unwrap(), before);
        assert_eq!(f.count("nodes"), nodes);
        assert_eq!(f.count("events"), 2);
    }

    #[test]
    fn cancelled_reads_and_corrupt_node_bytes_fail_without_resetting_data() {
        let f = Fixture::new();
        f.initial();
        let receipt = f.add_status(0, "one");
        assert_eq!(
            f.store
                .load_live_cancellable(Instant::now() + Duration::from_secs(10), &mut || true),
            Err(ProviderError::PersistenceUnavailable)
        );
        let conn = f.store.connection(true).unwrap().unwrap();
        conn.execute(
            "UPDATE nodes SET body=?1 WHERE hash=(SELECT root FROM receipts LIMIT 1)",
            [b"corrupt".as_slice()],
        )
        .unwrap();
        assert_eq!(
            f.store.receipt(&receipt.action_id),
            Err(ProviderError::InvalidStore)
        );
        // Unrelated current aggregates still read: receipts are not on the hot path.
        assert_eq!(f.store.load_live().unwrap().journal_sequence, 2);
    }

    #[test]
    fn crash_child() {
        let Ok(root) = std::env::var("CUTEX_STORAGE_CRASH_ROOT") else {
            return;
        };
        let store = Store::open(Path::new(&root)).unwrap();
        if std::env::var("CUTEX_STORAGE_CRASH_POINT").as_deref() == Ok("initialize-before-publish")
        {
            store.initialize().unwrap();
        } else {
            let before = store.load_live().unwrap();
            let mut after = before.clone();
            after.workflows.values_mut().next().unwrap().local_revision += 1;
            store
                .commit(&before, &mut after, "interrupted", now())
                .unwrap();
        }
        panic!("child did not hit crash point");
    }

    fn crash_subprocess(root: &Path, point: &str) {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "task_service::provider::storage::tests::crash_child",
                "--nocapture",
            ])
            .env("CUTEX_STORAGE_CRASH_ROOT", root)
            .env("CUTEX_STORAGE_CRASH_POINT", point)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(91));
    }

    #[test]
    fn process_exit_during_initialization_or_commit_recovers_atomically() {
        let f = Fixture::new();
        crash_subprocess(&f.root, "initialize-before-publish");
        assert!(!f.root.join(DATABASE_FILE).exists());
        assert_eq!(f.store.load_live().unwrap(), TaskServiceSnapshot::empty());
        f.initial();
        f.add_status(0, "committed");
        let before = f.store.load_full().unwrap();
        let nodes = f.count("nodes");
        crash_subprocess(&f.root, "commit-before-finish");
        assert_eq!(Store::open(&f.root).unwrap().load_full().unwrap(), before);
        assert_eq!(f.count("nodes"), nodes);
    }

    #[test]
    fn empty_first_open_marker_is_recoverable_but_cannot_hide_legacy_data() {
        let f = Fixture::new();
        fs::create_dir_all(&f.root).unwrap();
        fs::write(f.root.join(DATABASE_FILE), b"").unwrap();
        assert_eq!(f.store.load_live().unwrap(), TaskServiceSnapshot::empty());
        fs::write(f.root.join(LEGACY_STORE_FILE), b"{}").unwrap();
        assert!(matches!(
            Store::open(&f.root),
            Err(ProviderError::Conflict("legacy_task_store_requires_reset"))
        ));
        fs::remove_file(f.root.join(LEGACY_STORE_FILE)).unwrap();
        f.initial();
        assert_eq!(f.store.load_live().unwrap().journal_sequence, 1);
    }

    #[test]
    fn malformed_database_is_not_treated_as_an_empty_store() {
        let f = Fixture::new();
        fs::create_dir_all(&f.root).unwrap();
        fs::write(f.root.join(DATABASE_FILE), b"not a SQLite file").unwrap();
        assert_eq!(f.store.load_live(), Err(ProviderError::InvalidStore));
    }
}
