//! Task Service-owned stale-running watchdog state and decision engine.
//!
//! The watchdog is deliberately separate from semantic Task transitions. It
//! observes authoritative provider state and bounded runtime projections,
//! persists replay/delivery evidence, and emits typed presentation/message
//! intents. It never invokes Worker actions or changes Task phase.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, SecondsFormat, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{AssignmentState, AttemptPhase, TaskServiceSnapshot};

pub const TASK_WATCHDOG_FACT_SCHEMA: &str = "cutex/task-watchdog-fact/v1";
pub const TASK_WATCHDOG_MESSAGE_SCHEMA: &str = "cutex/task-watchdog-message/v1";
pub const TASK_WATCHDOG_CONTRACT_JSON: &str = include_str!("task-watchdog-v1.schema.json");

const STORE_SCHEMA: &str = "cutex/task-watchdog-store/v1";
const STORE_FILE: &str = "task-watchdog-v1.json";
const LOCK_FILE: &str = "task-watchdog-v1.lock";
const DEFAULT_POLL_SECS: u64 = 60;
const DEFAULT_STALE_SECS: u64 = 600;
const DEFAULT_ESCALATION_SECS: u64 = 600;
const MIN_POLL_SECS: u64 = 5;
const MAX_POLL_SECS: u64 = 3_600;
const MIN_STAGE_SECS: u64 = 60;
const MAX_STAGE_SECS: u64 = 86_400;
const MAX_NOTIFICATION_CONTENT_BYTES: usize = 1_024;
const MAX_RETIRED_NOTIFICATIONS: usize = 4_096;

pub fn default_task_watchdog_root() -> anyhow::Result<PathBuf> {
    Ok(crate::config::paths::runtime_dir()?
        .join("task-service")
        .join("task-worker-actions-v1")
        .join("task-service")
        .join("watchdog-v1"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskWatchdogConfig {
    pub poll_interval: Duration,
    pub first_stale_threshold: Duration,
    pub director_escalation_interval: Duration,
}

impl Default for TaskWatchdogConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(DEFAULT_POLL_SECS),
            first_stale_threshold: Duration::from_secs(DEFAULT_STALE_SECS),
            director_escalation_interval: Duration::from_secs(DEFAULT_ESCALATION_SECS),
        }
    }
}

impl TaskWatchdogConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            poll_interval: Duration::from_secs(env_seconds(
                "CUTEX_TASK_WATCHDOG_POLL_SECS",
                DEFAULT_POLL_SECS,
                MIN_POLL_SECS,
                MAX_POLL_SECS,
            )?),
            first_stale_threshold: Duration::from_secs(env_seconds(
                "CUTEX_TASK_WATCHDOG_STALE_SECS",
                DEFAULT_STALE_SECS,
                MIN_STAGE_SECS,
                MAX_STAGE_SECS,
            )?),
            director_escalation_interval: Duration::from_secs(env_seconds(
                "CUTEX_TASK_WATCHDOG_ESCALATION_SECS",
                DEFAULT_ESCALATION_SECS,
                MIN_STAGE_SECS,
                MAX_STAGE_SECS,
            )?),
        })
    }

    #[cfg(test)]
    fn for_tests(first_stale_secs: u64, escalation_secs: u64) -> Self {
        Self {
            poll_interval: Duration::from_secs(1),
            first_stale_threshold: Duration::from_secs(first_stale_secs),
            director_escalation_interval: Duration::from_secs(escalation_secs),
        }
    }
}

fn env_seconds(name: &str, default: u64, minimum: u64, maximum: u64) -> anyhow::Result<u64> {
    let Some(value) = std::env::var_os(name) else {
        return Ok(default);
    };
    let value = value
        .to_str()
        .with_context(|| format!("{name} is not valid UTF-8"))?
        .parse::<u64>()
        .with_context(|| format!("{name} is not a positive integer"))?;
    if !(minimum..=maximum).contains(&value) {
        anyhow::bail!("{name} must be between {minimum} and {maximum} seconds");
    }
    Ok(value)
}

pub trait TaskWatchdogClock: Send + Sync + 'static {
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Debug)]
pub struct SystemTaskWatchdogClock;

impl TaskWatchdogClock for SystemTaskWatchdogClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskWatchdogActivityKind {
    PhaseTransition,
    StatusProgress,
    LastOutput,
    LastToolCall,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskWatchdogActivityProjection {
    pub project_id: Option<crate::agent_management::ProjectId>,
    pub assignment_id: String,
    pub attempt_number: u64,
    pub assignee_cutex_session_id: String,
    pub kind: TaskWatchdogActivityKind,
    pub updated_at: String,
    pub source_sequence: u64,
}

pub fn task_watchdog_activity_projections(
    states: &std::collections::HashMap<
        String,
        crate::management::v2::activity::SessionActivityState,
    >,
) -> Vec<TaskWatchdogActivityProjection> {
    let mut projections = Vec::new();
    for (session_id, state) in states {
        if let Some(output) = state.last_output.as_ref() {
            if let Some(projection) = bounded_projection(
                session_id,
                state.revision,
                &output.association,
                TaskWatchdogActivityKind::LastOutput,
                &output.updated_at,
            ) {
                projections.push(projection);
            }
        }
        if let Some(tool) = state.last_tool_call.as_ref() {
            if let Some(projection) = bounded_projection(
                session_id,
                state.revision,
                &tool.association,
                TaskWatchdogActivityKind::LastToolCall,
                &tool.updated_at,
            ) {
                projections.push(projection);
            }
        }
    }
    projections
}

fn bounded_projection(
    session_id: &str,
    source_sequence: u64,
    association: &crate::observability::ObservationAssociation,
    kind: TaskWatchdogActivityKind,
    updated_at: &str,
) -> Option<TaskWatchdogActivityProjection> {
    if association.cutex_session_id != session_id {
        return None;
    }
    Some(TaskWatchdogActivityProjection {
        project_id: association.project_id.clone(),
        assignment_id: association.assignment_id.clone()?,
        attempt_number: association.attempt_number?,
        assignee_cutex_session_id: session_id.to_string(),
        kind,
        updated_at: updated_at.to_string(),
        source_sequence,
    })
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskWatchdogStage {
    FirstStale,
    DirectorEscalated,
}

impl TaskWatchdogStage {
    pub fn event_key(self) -> &'static str {
        match self {
            Self::FirstStale => "task_watchdog.first_stale",
            Self::DirectorEscalated => "task_watchdog.director_escalated",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskWatchdogFact {
    pub schema: String,
    pub event_key: String,
    pub fact_id: String,
    pub episode_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    pub task_id: String,
    pub task_revision: u64,
    pub assignment_id: String,
    pub attempt_number: u64,
    pub assignee_cutex_session_id: String,
    pub activity_watermark: String,
    pub activity_kind: TaskWatchdogActivityKind,
    pub idle_duration_secs: u64,
    pub stage: TaskWatchdogStage,
    pub source_sequence: u64,
    pub occurred_at: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskWatchdogDeliveryMode {
    Soon,
    AfterTurn,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum TaskWatchdogTarget {
    AssigneeSession(String),
    AuthoritySeat(String),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskWatchdogDeliveryFactKind {
    Queued,
    Delivered,
    Uncertain,
    RetryScheduled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskWatchdogDeliveryFact {
    pub kind: TaskWatchdogDeliveryFactKind,
    pub reference: Option<String>,
    pub recorded_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskWatchdogNotification {
    pub schema: String,
    pub notification_id: String,
    pub external_message_id: String,
    pub episode_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    pub assignment_id: String,
    pub attempt_number: u64,
    pub stage: TaskWatchdogStage,
    pub target: TaskWatchdogTarget,
    pub delivery_mode: TaskWatchdogDeliveryMode,
    pub content: String,
    pub facts: Vec<TaskWatchdogDeliveryFact>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskWatchdogMessageMetadata {
    pub schema: String,
    pub notification_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    pub assignment_id: String,
    pub attempt_number: u64,
    pub stage: TaskWatchdogStage,
}

impl From<&TaskWatchdogNotification> for TaskWatchdogMessageMetadata {
    fn from(notification: &TaskWatchdogNotification) -> Self {
        Self {
            schema: TASK_WATCHDOG_MESSAGE_SCHEMA.to_string(),
            notification_id: notification.notification_id.clone(),
            project_id: notification.project_id.clone(),
            assignment_id: notification.assignment_id.clone(),
            attempt_number: notification.attempt_number,
            stage: notification.stage,
        }
    }
}

impl TaskWatchdogNotification {
    pub fn is_delivered(&self) -> bool {
        self.facts
            .iter()
            .any(|fact| fact.kind == TaskWatchdogDeliveryFactKind::Delivered)
    }

    pub fn is_queued(&self) -> bool {
        self.facts
            .iter()
            .any(|fact| fact.kind == TaskWatchdogDeliveryFactKind::Queued)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TaskWatchdogScanOutcome {
    pub presentations: Vec<TaskWatchdogFact>,
    pub notifications: Vec<TaskWatchdogNotification>,
    pub cancelled_notification_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct TaskWatchdogEpisode {
    episode_id: String,
    activity_watermark: String,
    facts: Vec<TaskWatchdogFact>,
    notifications: Vec<TaskWatchdogNotification>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PersistedWatchdogWatermark {
    encoded: String,
    kind: TaskWatchdogActivityKind,
    source_sequence: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RetiredWatchdogNotification {
    attempt_key: String,
    retired_at: String,
    notification: TaskWatchdogNotification,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct TaskWatchdogStore {
    schema: String,
    episodes: BTreeMap<String, TaskWatchdogEpisode>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    watermarks: BTreeMap<String, PersistedWatchdogWatermark>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    retired_notifications: BTreeMap<String, RetiredWatchdogNotification>,
}

impl Default for TaskWatchdogStore {
    fn default() -> Self {
        Self {
            schema: STORE_SCHEMA.to_string(),
            episodes: BTreeMap::new(),
            watermarks: BTreeMap::new(),
            retired_notifications: BTreeMap::new(),
        }
    }
}

pub struct TaskStaleWatchdog {
    root: PathBuf,
    config: TaskWatchdogConfig,
    clock: Arc<dyn TaskWatchdogClock>,
    process_lock: Mutex<()>,
}

impl TaskStaleWatchdog {
    pub fn open(root: impl Into<PathBuf>, config: TaskWatchdogConfig) -> anyhow::Result<Self> {
        Self::with_clock(root, config, Arc::new(SystemTaskWatchdogClock))
    }

    pub fn with_clock(
        root: impl Into<PathBuf>,
        config: TaskWatchdogConfig,
        clock: Arc<dyn TaskWatchdogClock>,
    ) -> anyhow::Result<Self> {
        let root = root.into();
        prepare_private_root(&root)?;
        let watchdog = Self {
            root,
            config,
            clock,
            process_lock: Mutex::new(()),
        };
        watchdog.with_store(|_| Ok(()))?;
        Ok(watchdog)
    }

    pub fn poll_interval(&self) -> Duration {
        self.config.poll_interval
    }

    pub fn scan(
        &self,
        snapshot: &TaskServiceSnapshot,
        activity: &[TaskWatchdogActivityProjection],
    ) -> anyhow::Result<TaskWatchdogScanOutcome> {
        let now = self.clock.now();
        self.with_store(|store| scan_store(store, snapshot, activity, now, self.config))
    }

    pub fn record_delivery_fact(
        &self,
        notification_id: &str,
        kind: TaskWatchdogDeliveryFactKind,
        reference: Option<String>,
    ) -> anyhow::Result<()> {
        let now = format_time(self.clock.now());
        self.with_store(|store| {
            let active = store
                .episodes
                .values_mut()
                .flat_map(|episode| episode.notifications.iter_mut())
                .find(|notification| notification.notification_id == notification_id);
            let notification = match active {
                Some(notification) => notification,
                None => {
                    &mut store
                        .retired_notifications
                        .get_mut(notification_id)
                        .context("Task watchdog notification is absent")?
                        .notification
                }
            };
            if notification
                .facts
                .iter()
                .any(|fact| fact.kind == kind && fact.reference == reference)
            {
                return Ok(());
            }
            notification.facts.push(TaskWatchdogDeliveryFact {
                kind,
                reference,
                recorded_at: now,
            });
            Ok(())
        })
    }

    pub fn notification(
        &self,
        notification_id: &str,
    ) -> anyhow::Result<Option<TaskWatchdogNotification>> {
        self.with_store(|store| {
            Ok(store
                .episodes
                .values()
                .flat_map(|episode| episode.notifications.iter())
                .find(|notification| notification.notification_id == notification_id)
                .cloned()
                .or_else(|| {
                    store
                        .retired_notifications
                        .get(notification_id)
                        .map(|retired| retired.notification.clone())
                }))
        })
    }

    fn with_store<T>(
        &self,
        operation: impl FnOnce(&mut TaskWatchdogStore) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let _process = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("Task watchdog process lock is poisoned"))?;
        let lock = open_private_lock(&self.root.join(LOCK_FILE))?;
        lock.lock_exclusive()?;
        let path = self.root.join(STORE_FILE);
        let mut store = load_store(&path)?;
        let before = store.clone();
        let result = operation(&mut store)?;
        validate_store(&store)?;
        if store != before || !path.exists() {
            crate::config::atomic::write_private_pretty_json_atomic(
                &path,
                &store,
                "Task watchdog state",
            )?;
        }
        Ok(result)
    }
}

fn scan_store(
    store: &mut TaskWatchdogStore,
    snapshot: &TaskServiceSnapshot,
    activity: &[TaskWatchdogActivityProjection],
    now: DateTime<Utc>,
    config: TaskWatchdogConfig,
) -> anyhow::Result<TaskWatchdogScanOutcome> {
    let mut active_keys = BTreeSet::new();
    let mut presentations = Vec::new();
    let mut notifications = Vec::new();
    let mut cancelled_notification_ids = Vec::new();
    for assignment in snapshot.assignments.values() {
        if assignment.state != AssignmentState::Active {
            continue;
        }
        let Some(attempt_number) = assignment.active_attempt else {
            continue;
        };
        let Some(attempt) = snapshot
            .attempts
            .get(&assignment.assignment_id)
            .and_then(|attempts| attempts.get(&attempt_number))
        else {
            continue;
        };
        if attempt.phase != AttemptPhase::Running || attempt.project_id != assignment.project_id {
            continue;
        }
        let Some(task) = snapshot
            .task_revisions
            .get(&assignment.task_id)
            .and_then(|revisions| revisions.get(&assignment.task_revision))
        else {
            continue;
        };
        if task.project_id != assignment.project_id {
            continue;
        }
        let attempt_key = attempt_key(
            assignment.project_id.as_ref().map(|id| id.as_str()),
            assignment.task_id.as_str(),
            assignment.task_revision.get(),
            assignment.assignment_id.as_str(),
            attempt_number.get(),
        );
        active_keys.insert(attempt_key.clone());
        let observed = authoritative_watermark(snapshot, assignment, attempt, activity)?;
        let watermark = monotonic_watermark(store, &attempt_key, observed)?;
        if store
            .episodes
            .get(&attempt_key)
            .is_some_and(|episode| episode.activity_watermark != watermark.encoded)
        {
            retire_episode(store, &attempt_key, now, &mut cancelled_notification_ids);
        }
        let idle = now
            .signed_duration_since(watermark.time)
            .num_seconds()
            .max(0) as u64;
        if idle < config.first_stale_threshold.as_secs() {
            continue;
        }
        let episode_id = stable_id("twe_", &format!("{attempt_key}|{}", watermark.encoded), 32);
        let episode = store
            .episodes
            .entry(attempt_key)
            .or_insert_with(|| TaskWatchdogEpisode {
                episode_id: episode_id.clone(),
                activity_watermark: watermark.encoded.clone(),
                facts: Vec::new(),
                notifications: Vec::new(),
            });
        ensure_stage(
            episode,
            TaskWatchdogStage::FirstStale,
            idle,
            now,
            snapshot.journal_sequence,
            assignment,
            task.completion_policy.authority_seat_id.as_str(),
            &watermark,
        );
        if idle
            >= config
                .first_stale_threshold
                .as_secs()
                .saturating_add(config.director_escalation_interval.as_secs())
        {
            ensure_stage(
                episode,
                TaskWatchdogStage::DirectorEscalated,
                idle,
                now,
                snapshot.journal_sequence,
                assignment,
                task.completion_policy.authority_seat_id.as_str(),
                &watermark,
            );
        }
        presentations.extend(episode.facts.iter().cloned());
        notifications.extend(
            episode
                .notifications
                .iter()
                .filter(|notification| !notification.is_delivered())
                .cloned(),
        );
    }
    let inactive = store
        .episodes
        .keys()
        .chain(store.watermarks.keys())
        .filter(|key| !active_keys.contains(*key))
        .cloned()
        .collect::<BTreeSet<_>>();
    for key in inactive {
        retire_episode(store, &key, now, &mut cancelled_notification_ids);
        store.watermarks.remove(&key);
    }
    prune_retired_notifications(store);
    Ok(TaskWatchdogScanOutcome {
        presentations,
        notifications,
        cancelled_notification_ids,
    })
}

#[derive(Clone, Debug)]
struct Watermark {
    time: DateTime<Utc>,
    encoded: String,
    kind: TaskWatchdogActivityKind,
    source_sequence: u64,
}

fn monotonic_watermark(
    store: &mut TaskWatchdogStore,
    attempt_key: &str,
    observed: Watermark,
) -> anyhow::Result<Watermark> {
    let persisted = store.watermarks.get(attempt_key).cloned().or_else(|| {
        store.episodes.get(attempt_key).map(|episode| {
            let fact = episode.facts.first();
            PersistedWatchdogWatermark {
                encoded: episode.activity_watermark.clone(),
                kind: fact
                    .map(|fact| fact.activity_kind)
                    .unwrap_or(TaskWatchdogActivityKind::PhaseTransition),
                source_sequence: fact.map(|fact| fact.source_sequence).unwrap_or(0),
            }
        })
    });
    let watermark = if let Some(persisted) = persisted {
        let mut watermark = Watermark {
            time: parse_time(&persisted.encoded)?,
            encoded: persisted.encoded,
            kind: persisted.kind,
            source_sequence: persisted.source_sequence,
        };
        consider_watermark(
            &mut watermark,
            observed.time,
            &observed.encoded,
            observed.kind,
            observed.source_sequence,
        );
        watermark
    } else {
        observed
    };
    store.watermarks.insert(
        attempt_key.to_string(),
        PersistedWatchdogWatermark {
            encoded: watermark.encoded.clone(),
            kind: watermark.kind,
            source_sequence: watermark.source_sequence,
        },
    );
    Ok(watermark)
}

fn retire_episode(
    store: &mut TaskWatchdogStore,
    attempt_key: &str,
    now: DateTime<Utc>,
    cancelled_notification_ids: &mut Vec<String>,
) {
    let Some(episode) = store.episodes.remove(attempt_key) else {
        return;
    };
    for notification in episode.notifications {
        if !notification.is_delivered() {
            cancelled_notification_ids.push(notification.notification_id.clone());
        }
        store.retired_notifications.insert(
            notification.notification_id.clone(),
            RetiredWatchdogNotification {
                attempt_key: attempt_key.to_string(),
                retired_at: format_time(now),
                notification,
            },
        );
    }
}

fn prune_retired_notifications(store: &mut TaskWatchdogStore) {
    while store.retired_notifications.len() > MAX_RETIRED_NOTIFICATIONS {
        let Some(oldest) = store
            .retired_notifications
            .iter()
            .min_by(|left, right| {
                left.1
                    .retired_at
                    .cmp(&right.1.retired_at)
                    .then_with(|| left.0.cmp(right.0))
            })
            .map(|(id, _)| id.clone())
        else {
            break;
        };
        store.retired_notifications.remove(&oldest);
    }
}

fn authoritative_watermark(
    snapshot: &TaskServiceSnapshot,
    assignment: &super::Assignment,
    attempt: &super::Attempt,
    activity: &[TaskWatchdogActivityProjection],
) -> anyhow::Result<Watermark> {
    let mut watermark = Watermark {
        time: parse_time(attempt.updated_at.as_str())?,
        encoded: attempt.updated_at.as_str().to_string(),
        kind: TaskWatchdogActivityKind::PhaseTransition,
        source_sequence: snapshot.journal_sequence,
    };
    if let Some(status) = attempt.status_receipts.last() {
        consider_watermark(
            &mut watermark,
            parse_time(status.recorded_at.as_str())?,
            status.recorded_at.as_str(),
            TaskWatchdogActivityKind::StatusProgress,
            snapshot.journal_sequence,
        );
    }
    for projection in activity {
        if projection.project_id != assignment.project_id
            || projection.assignment_id != assignment.assignment_id.as_str()
            || projection.attempt_number != attempt.attempt_number.get()
            || projection.assignee_cutex_session_id != assignment.assignee_cutex_session.as_str()
        {
            continue;
        }
        let time = parse_time(&projection.updated_at)?;
        if time < parse_time(attempt.started_at.as_str())? {
            continue;
        }
        consider_watermark(
            &mut watermark,
            time,
            &projection.updated_at,
            projection.kind,
            projection.source_sequence,
        );
    }
    Ok(watermark)
}

fn consider_watermark(
    watermark: &mut Watermark,
    candidate: DateTime<Utc>,
    encoded: &str,
    kind: TaskWatchdogActivityKind,
    source_sequence: u64,
) {
    if candidate > watermark.time
        || (candidate == watermark.time && source_sequence > watermark.source_sequence)
        || (candidate == watermark.time
            && source_sequence == watermark.source_sequence
            && watermark.kind == TaskWatchdogActivityKind::PhaseTransition
            && kind == TaskWatchdogActivityKind::StatusProgress)
    {
        watermark.time = candidate;
        watermark.encoded = encoded.to_string();
        watermark.kind = kind;
        watermark.source_sequence = source_sequence;
    }
}

#[allow(clippy::too_many_arguments)]
fn ensure_stage(
    episode: &mut TaskWatchdogEpisode,
    stage: TaskWatchdogStage,
    idle_duration_secs: u64,
    now: DateTime<Utc>,
    _provider_sequence: u64,
    assignment: &super::Assignment,
    authority_seat_id: &str,
    watermark: &Watermark,
) {
    if episode.facts.iter().any(|fact| fact.stage == stage) {
        return;
    }
    let stage_label = stage.event_key();
    let fact_id = stable_id("twf_", &format!("{}|{stage_label}", episode.episode_id), 32);
    let fact = TaskWatchdogFact {
        schema: TASK_WATCHDOG_FACT_SCHEMA.to_string(),
        event_key: stage_label.to_string(),
        fact_id,
        episode_id: episode.episode_id.clone(),
        project_id: assignment.project_id.clone(),
        task_id: assignment.task_id.as_str().to_string(),
        task_revision: assignment.task_revision.get(),
        assignment_id: assignment.assignment_id.as_str().to_string(),
        attempt_number: assignment
            .active_attempt
            .expect("running assignment has an attempt")
            .get(),
        assignee_cutex_session_id: assignment.assignee_cutex_session.as_str().to_string(),
        activity_watermark: watermark.encoded.clone(),
        activity_kind: watermark.kind,
        idle_duration_secs,
        stage,
        source_sequence: watermark.source_sequence,
        occurred_at: format_time(now),
    };
    let notification_id = stable_id("twn_", &format!("{}|{stage_label}", episode.episode_id), 32);
    let (target, delivery_mode, content) = match stage {
        TaskWatchdogStage::FirstStale => (
            TaskWatchdogTarget::AssigneeSession(
                assignment.assignee_cutex_session.as_str().to_string(),
            ),
            TaskWatchdogDeliveryMode::Soon,
            format!(
                "Task Service watchdog: assignment {} attempt {} has no authoritative progress for at least {} seconds. Continue the task or report bounded status.",
                assignment.assignment_id.as_str(),
                assignment.active_attempt.expect("running attempt").get(),
                idle_duration_secs,
            ),
        ),
        TaskWatchdogStage::DirectorEscalated => (
            TaskWatchdogTarget::AuthoritySeat(authority_seat_id.to_string()),
            TaskWatchdogDeliveryMode::AfterTurn,
            format!(
                "Task Service watchdog escalation: assignment {} attempt {} remains inactive after the Worker reminder; review the running task.",
                assignment.assignment_id.as_str(),
                assignment.active_attempt.expect("running attempt").get(),
            ),
        ),
    };
    episode.notifications.push(TaskWatchdogNotification {
        schema: TASK_WATCHDOG_MESSAGE_SCHEMA.to_string(),
        external_message_id: notification_id.clone(),
        notification_id,
        episode_id: episode.episode_id.clone(),
        project_id: assignment.project_id.clone(),
        assignment_id: assignment.assignment_id.as_str().to_string(),
        attempt_number: assignment.active_attempt.expect("running attempt").get(),
        stage,
        target,
        delivery_mode,
        content,
        facts: Vec::new(),
    });
    episode.facts.push(fact);
}

fn attempt_key(
    project_id: Option<&str>,
    task_id: &str,
    task_revision: u64,
    assignment_id: &str,
    attempt_number: u64,
) -> String {
    stable_id(
        "twa_",
        &format!(
            "{}|{task_id}|{task_revision}|{assignment_id}|{attempt_number}",
            project_id.unwrap_or("<legacy>")
        ),
        40,
    )
}

fn stable_id(prefix: &str, material: &str, hex_chars: usize) -> String {
    let digest = format!("{:x}", Sha256::digest(material.as_bytes()));
    format!("{prefix}{}", &digest[..hex_chars])
}

fn parse_time(value: &str) -> anyhow::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .with_context(|| format!("invalid Task watchdog timestamp {value:?}"))
}

fn format_time(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

fn prepare_private_root(root: &Path) -> anyhow::Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(root) {
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            anyhow::bail!("Task watchdog root is not a direct directory");
        }
    } else {
        fs::create_dir_all(root)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    crate::platform::private_fs::secure_tree(root).map_err(anyhow::Error::new)?;
    Ok(())
}

fn open_private_lock(path: &Path) -> anyhow::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    options.open(path).map_err(anyhow::Error::new)
}

fn load_store(path: &Path) -> anyhow::Result<TaskWatchdogStore> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .context("Task watchdog state is not valid canonical JSON"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(TaskWatchdogStore::default())
        }
        Err(error) => Err(error.into()),
    }
}

fn validate_store(store: &TaskWatchdogStore) -> anyhow::Result<()> {
    if store.schema != STORE_SCHEMA {
        anyhow::bail!("unsupported Task watchdog store schema");
    }
    for (key, episode) in &store.episodes {
        if !key.starts_with("twa_")
            || !episode.episode_id.starts_with("twe_")
            || episode.activity_watermark.is_empty()
            || episode.facts.len() > 2
            || episode.notifications.len() > 2
        {
            anyhow::bail!("invalid Task watchdog episode");
        }
        for fact in &episode.facts {
            if fact.schema != TASK_WATCHDOG_FACT_SCHEMA
                || fact.episode_id != episode.episode_id
                || fact.event_key != fact.stage.event_key()
                || fact.fact_id.len() > 64
            {
                anyhow::bail!("invalid Task watchdog fact");
            }
        }
        for notification in &episode.notifications {
            if notification.schema != TASK_WATCHDOG_MESSAGE_SCHEMA
                || notification.episode_id != episode.episode_id
                || notification.notification_id != notification.external_message_id
                || notification.notification_id.len() > 48
                || notification.content.trim().is_empty()
                || notification.content.len() > MAX_NOTIFICATION_CONTENT_BYTES
            {
                anyhow::bail!("invalid Task watchdog notification");
            }
        }
    }
    for (key, watermark) in &store.watermarks {
        if !key.starts_with("twa_") || watermark.encoded.is_empty() {
            anyhow::bail!("invalid persisted Task watchdog watermark");
        }
        parse_time(&watermark.encoded)?;
    }
    if store.retired_notifications.len() > MAX_RETIRED_NOTIFICATIONS {
        anyhow::bail!("Task watchdog retired notification bound exceeded");
    }
    for (notification_id, retired) in &store.retired_notifications {
        if !retired.attempt_key.starts_with("twa_")
            || notification_id != &retired.notification.notification_id
            || retired.notification.notification_id != retired.notification.external_message_id
            || retired.notification.notification_id.len() > 48
        {
            anyhow::bail!("invalid retired Task watchdog notification");
        }
        parse_time(&retired.retired_at)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_management::ProjectId;
    use crate::role_revision::{
        AttemptNumber, CutexSessionId, Rfc3339, Sha256 as TypedSha256, TaskId, TaskRevision,
    };
    use crate::task_service::{
        ActionId, Assignment, AssignmentId, Attempt, CompletionPolicy, CompletionPolicyKind,
        ProviderAttemptToken, ProviderStoreSchema, SeatId, StatusReceipt, TaskRevisionRecord,
        WorkflowId,
    };
    use std::sync::RwLock;

    #[test]
    fn config_defaults_are_conservative() {
        let config = TaskWatchdogConfig::default();
        assert_eq!(config.poll_interval, Duration::from_secs(60));
        assert_eq!(config.first_stale_threshold, Duration::from_secs(600));
        assert_eq!(
            config.director_escalation_interval,
            Duration::from_secs(600)
        );
    }

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("cutex-task-watchdog-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[derive(Debug)]
    struct FakeClock(RwLock<DateTime<Utc>>);

    impl FakeClock {
        fn new(value: &str) -> Self {
            Self(RwLock::new(parse_time(value).unwrap()))
        }

        fn set(&self, value: &str) {
            *self.0.write().unwrap() = parse_time(value).unwrap();
        }
    }

    impl TaskWatchdogClock for FakeClock {
        fn now(&self) -> DateTime<Utc> {
            *self.0.read().unwrap()
        }
    }

    fn typed<T, E: std::fmt::Debug>(
        value: &str,
        constructor: impl FnOnce(String) -> Result<T, E>,
    ) -> T {
        constructor(value.to_string()).unwrap()
    }

    fn snapshot(project: Option<ProjectId>, phase: AttemptPhase) -> TaskServiceSnapshot {
        let assignment_id = typed("assignment-1", AssignmentId::new);
        let task_id = typed("task-1", TaskId::new);
        let revision = TaskRevision::new(1).unwrap();
        let attempt_number = AttemptNumber::new(1).unwrap();
        let session = typed("cutex.worker", CutexSessionId::new);
        let mut snapshot = TaskServiceSnapshot {
            schema: ProviderStoreSchema::V3,
            journal_sequence: 7,
            journal_sha256: typed(
                "0000000000000000000000000000000000000000000000000000000000000000",
                TypedSha256::new,
            ),
            task_revisions: BTreeMap::new(),
            assignments: BTreeMap::new(),
            attempts: BTreeMap::new(),
            send_attempts: BTreeMap::new(),
            completion_notifications: BTreeMap::new(),
            worker_followup_notifications: BTreeMap::new(),
            workflows: BTreeMap::new(),
            receipts: BTreeMap::new(),
            prepared_worker_actions: BTreeMap::new(),
        };
        snapshot
            .task_revisions
            .entry(task_id.clone())
            .or_default()
            .insert(
                revision,
                TaskRevisionRecord {
                    project_id: project.clone(),
                    task_id: task_id.clone(),
                    task_revision: revision,
                    contract_sha256: typed(
                        "1111111111111111111111111111111111111111111111111111111111111111",
                        TypedSha256::new,
                    ),
                    opaque_contract: "contract".to_string(),
                    completion_policy: CompletionPolicy {
                        kind: CompletionPolicyKind::DirectorAcceptance,
                        authority_seat_id: typed("cutex-director", SeatId::new),
                    },
                    workflow_id: typed("workflow-1", WorkflowId::new),
                    created_at: typed("2026-08-29T00:00:00Z", Rfc3339::new),
                    created_by_cutex_session: typed("cutex.director", CutexSessionId::new),
                },
            );
        snapshot.assignments.insert(
            assignment_id.clone(),
            Assignment {
                project_id: project.clone(),
                assignment_id: assignment_id.clone(),
                task_id,
                task_revision: revision,
                assignee_cutex_session: session,
                state: AssignmentState::Active,
                local_revision: 2,
                created_at: typed("2026-08-29T00:00:00Z", Rfc3339::new),
                acknowledged_at: Some(typed("2026-08-29T00:00:00Z", Rfc3339::new)),
                active_attempt: Some(attempt_number),
                retry_authorization: None,
                closure: None,
            },
        );
        snapshot
            .attempts
            .entry(assignment_id.clone())
            .or_default()
            .insert(
                attempt_number,
                Attempt {
                    project_id: project,
                    assignment_id,
                    attempt_number,
                    attempt_token: typed("attempt-token", ProviderAttemptToken::new),
                    phase,
                    local_revision: 1,
                    started_at: typed("2026-08-29T00:00:00Z", Rfc3339::new),
                    updated_at: typed("2026-08-29T00:00:00Z", Rfc3339::new),
                    status_receipts: Vec::new(),
                    result_receipts: Vec::new(),
                    terminal_action_id: None,
                },
            );
        snapshot
    }

    fn harness(now: &str) -> (TestDir, Arc<FakeClock>, TaskStaleWatchdog) {
        let temp = TestDir::new();
        let clock = Arc::new(FakeClock::new(now));
        let watchdog = TaskStaleWatchdog::with_clock(
            temp.path(),
            TaskWatchdogConfig::for_tests(600, 600),
            clock.clone(),
        )
        .unwrap();
        (temp, clock, watchdog)
    }

    #[test]
    fn threshold_cap_reset_and_terminal_suppression_are_hermetic() {
        let (_temp, clock, watchdog) = harness("2026-08-29T00:09:59Z");
        let mut state = snapshot(None, AttemptPhase::Running);
        assert!(watchdog.scan(&state, &[]).unwrap().presentations.is_empty());
        clock.set("2026-08-29T00:10:00Z");
        let first = watchdog.scan(&state, &[]).unwrap();
        assert_eq!(first.presentations.len(), 1);
        assert_eq!(first.notifications.len(), 1);
        assert_eq!(first.presentations[0].stage, TaskWatchdogStage::FirstStale);
        let replay = watchdog.scan(&state, &[]).unwrap();
        assert_eq!(
            replay.presentations[0].fact_id,
            first.presentations[0].fact_id
        );
        clock.set("2026-08-29T00:20:00Z");
        let escalated = watchdog.scan(&state, &[]).unwrap();
        assert_eq!(escalated.presentations.len(), 2);
        assert_eq!(escalated.notifications.len(), 2);
        clock.set("2026-08-29T01:20:00Z");
        assert_eq!(watchdog.scan(&state, &[]).unwrap().presentations.len(), 2);
        state
            .attempts
            .values_mut()
            .next()
            .unwrap()
            .values_mut()
            .next()
            .unwrap()
            .phase = AttemptPhase::ReviewReady;
        let terminal = watchdog.scan(&state, &[]).unwrap();
        assert!(terminal.presentations.is_empty());
        assert!(terminal.notifications.is_empty());
    }

    #[test]
    fn exact_project_output_and_tool_watermarks_reset_episode() {
        let project = ProjectId::new("project-1").unwrap();
        let (_temp, clock, watchdog) = harness("2026-08-29T00:10:00Z");
        let state = snapshot(Some(project.clone()), AttemptPhase::Running);
        let stale = watchdog.scan(&state, &[]).unwrap();
        let first_episode = stale.presentations[0].episode_id.clone();
        let wrong_project = TaskWatchdogActivityProjection {
            project_id: Some(ProjectId::new("project-2").unwrap()),
            assignment_id: "assignment-1".to_string(),
            attempt_number: 1,
            assignee_cutex_session_id: "cutex.worker".to_string(),
            kind: TaskWatchdogActivityKind::LastOutput,
            updated_at: "2026-08-29T00:09:30Z".to_string(),
            source_sequence: 8,
        };
        assert_eq!(
            watchdog
                .scan(&state, &[wrong_project])
                .unwrap()
                .presentations[0]
                .episode_id,
            first_episode
        );
        let output = TaskWatchdogActivityProjection {
            project_id: Some(project.clone()),
            assignment_id: "assignment-1".to_string(),
            attempt_number: 1,
            assignee_cutex_session_id: "cutex.worker".to_string(),
            kind: TaskWatchdogActivityKind::LastOutput,
            updated_at: "2026-08-29T00:09:30Z".to_string(),
            source_sequence: 9,
        };
        assert!(watchdog
            .scan(&state, std::slice::from_ref(&output))
            .unwrap()
            .presentations
            .is_empty());
        clock.set("2026-08-29T00:19:30Z");
        let second = watchdog.scan(&state, &[output]).unwrap();
        assert_ne!(second.presentations[0].episode_id, first_episode);
        assert_eq!(
            second.presentations[0].activity_kind,
            TaskWatchdogActivityKind::LastOutput
        );
    }

    #[test]
    fn restart_replay_delivery_facts_and_ids_are_bounded() {
        let (temp, clock, watchdog) = harness("2026-08-29T00:10:00Z");
        let state = snapshot(None, AttemptPhase::Running);
        let first = watchdog.scan(&state, &[]).unwrap();
        let notification = &first.notifications[0];
        assert!(format!("tsw_{}", notification.external_message_id).len() <= 64);
        watchdog
            .record_delivery_fact(
                &notification.notification_id,
                TaskWatchdogDeliveryFactKind::Queued,
                Some("message-1".to_string()),
            )
            .unwrap();
        drop(watchdog);
        let restarted = TaskStaleWatchdog::with_clock(
            temp.path(),
            TaskWatchdogConfig::for_tests(600, 600),
            clock,
        )
        .unwrap();
        let replay = restarted.scan(&state, &[]).unwrap();
        assert_eq!(
            replay.presentations[0].fact_id,
            first.presentations[0].fact_id
        );
        assert_eq!(
            replay.notifications[0].notification_id,
            notification.notification_id
        );
        restarted
            .record_delivery_fact(
                &notification.notification_id,
                TaskWatchdogDeliveryFactKind::Delivered,
                Some("submission-1".to_string()),
            )
            .unwrap();
        assert!(restarted
            .notification(&notification.notification_id)
            .unwrap()
            .unwrap()
            .is_delivered());
    }

    #[test]
    fn concurrent_store_contenders_produce_one_stable_episode_identity() {
        let temp = TestDir::new();
        let clock = Arc::new(FakeClock::new("2026-08-29T00:10:00Z"));
        let first = TaskStaleWatchdog::with_clock(
            temp.path(),
            TaskWatchdogConfig::for_tests(600, 600),
            clock.clone(),
        )
        .unwrap();
        let second = TaskStaleWatchdog::with_clock(
            temp.path(),
            TaskWatchdogConfig::for_tests(600, 600),
            clock,
        )
        .unwrap();
        let state = snapshot(None, AttemptPhase::Running);
        let (left, right) = std::thread::scope(|scope| {
            let left = scope.spawn(|| first.scan(&state, &[]).unwrap());
            let right = scope.spawn(|| second.scan(&state, &[]).unwrap());
            (left.join().unwrap(), right.join().unwrap())
        });
        assert_eq!(left.presentations.len(), 1);
        assert_eq!(right.presentations.len(), 1);
        assert_eq!(
            left.presentations[0].fact_id,
            right.presentations[0].fact_id
        );
        assert_eq!(
            left.notifications[0].notification_id,
            right.notifications[0].notification_id
        );
        let replay = first.scan(&state, &[]).unwrap();
        assert_eq!(replay.presentations.len(), 1);
        assert_eq!(replay.notifications.len(), 1);
    }

    #[test]
    fn status_and_exact_tool_projections_advance_but_unrelated_activity_does_not() {
        let project = ProjectId::new("project-1").unwrap();
        let (_temp, clock, watchdog) = harness("2026-08-29T00:20:00Z");
        let mut state = snapshot(Some(project.clone()), AttemptPhase::Running);
        let attempt = state
            .attempts
            .values_mut()
            .next()
            .unwrap()
            .values_mut()
            .next()
            .unwrap();
        attempt.updated_at = typed("2026-08-29T00:10:00Z", Rfc3339::new);
        attempt.status_receipts.push(StatusReceipt {
            project_id: Some(project.clone()),
            action_id: typed("status-1", ActionId::new),
            summary: "bounded progress".to_string(),
            evidence_sha256: None,
            recorded_at: typed("2026-08-29T00:10:00Z", Rfc3339::new),
        });
        let first = watchdog.scan(&state, &[]).unwrap();
        assert_eq!(
            first.presentations[0].activity_kind,
            TaskWatchdogActivityKind::StatusProgress
        );

        // Heartbeats, ordinary messages, query traffic and self-events have no
        // input type here. A mismatched bounded projection is equally inert.
        let unrelated = TaskWatchdogActivityProjection {
            project_id: Some(project.clone()),
            assignment_id: "assignment-other".to_string(),
            attempt_number: 1,
            assignee_cutex_session_id: "cutex.worker".to_string(),
            kind: TaskWatchdogActivityKind::LastToolCall,
            updated_at: "2026-08-29T00:19:30Z".to_string(),
            source_sequence: 12,
        };
        assert_eq!(
            watchdog.scan(&state, &[unrelated]).unwrap().presentations[0].activity_kind,
            TaskWatchdogActivityKind::StatusProgress
        );

        let tool = TaskWatchdogActivityProjection {
            project_id: Some(project),
            assignment_id: "assignment-1".to_string(),
            attempt_number: 1,
            assignee_cutex_session_id: "cutex.worker".to_string(),
            kind: TaskWatchdogActivityKind::LastToolCall,
            updated_at: "2026-08-29T00:19:30Z".to_string(),
            source_sequence: 13,
        };
        assert!(watchdog
            .scan(&state, std::slice::from_ref(&tool))
            .unwrap()
            .presentations
            .is_empty());
        clock.set("2026-08-29T00:29:30Z");
        let tool_stale = watchdog.scan(&state, &[tool]).unwrap();
        assert_eq!(
            tool_stale.presentations[0].activity_kind,
            TaskWatchdogActivityKind::LastToolCall
        );
        assert_eq!(tool_stale.presentations[0].source_sequence, 13);
    }

    #[test]
    fn observed_wake_sequence_keeps_watermark_monotonic_and_prevents_false_escalation() {
        let (temp, clock, watchdog) = harness("2026-08-29T00:10:04Z");
        let state = snapshot(None, AttemptPhase::Running);
        let turn_a_output = TaskWatchdogActivityProjection {
            project_id: None,
            assignment_id: "assignment-1".to_string(),
            attempt_number: 1,
            assignee_cutex_session_id: "cutex.worker".to_string(),
            kind: TaskWatchdogActivityKind::LastOutput,
            updated_at: "2026-08-29T00:00:04Z".to_string(),
            source_sequence: 8,
        };
        let first = watchdog
            .scan(&state, std::slice::from_ref(&turn_a_output))
            .unwrap();
        assert_eq!(first.presentations.len(), 1);
        assert_eq!(first.notifications.len(), 1);
        let first_fact_id = first.presentations[0].fact_id.clone();
        let first_notification_id = first.notifications[0].notification_id.clone();
        watchdog
            .record_delivery_fact(
                &first_notification_id,
                TaskWatchdogDeliveryFactKind::Delivered,
                Some("native-submission-1".to_string()),
            )
            .unwrap();

        drop(watchdog);
        let watchdog = TaskStaleWatchdog::with_clock(
            temp.path(),
            TaskWatchdogConfig::for_tests(600, 600),
            clock.clone(),
        )
        .unwrap();
        clock.set("2026-08-29T00:10:20Z");
        let incomplete = watchdog.scan(&state, &[]).unwrap();
        assert_eq!(incomplete.presentations.len(), 1);
        assert_eq!(incomplete.presentations[0].fact_id, first_fact_id);
        assert!(incomplete.notifications.is_empty());

        let regressed = TaskWatchdogActivityProjection {
            updated_at: "2026-08-29T00:00:02Z".to_string(),
            source_sequence: 7,
            ..turn_a_output.clone()
        };
        let regressed_scan = watchdog.scan(&state, &[regressed]).unwrap();
        assert_eq!(regressed_scan.presentations[0].fact_id, first_fact_id);
        assert!(regressed_scan.notifications.is_empty());

        let turn_b_output = TaskWatchdogActivityProjection {
            updated_at: "2026-08-29T00:10:21Z".to_string(),
            source_sequence: 20,
            ..turn_a_output.clone()
        };
        clock.set("2026-08-29T00:10:22Z");
        let recovered = watchdog.scan(&state, &[turn_b_output]).unwrap();
        assert!(recovered.presentations.is_empty());
        assert!(recovered.notifications.is_empty());

        let turn_b_tool = TaskWatchdogActivityProjection {
            kind: TaskWatchdogActivityKind::LastToolCall,
            updated_at: "2026-08-29T00:10:25Z".to_string(),
            source_sequence: 21,
            ..turn_a_output
        };
        assert!(watchdog
            .scan(&state, std::slice::from_ref(&turn_b_tool))
            .unwrap()
            .presentations
            .is_empty());

        // This is the old episode's escalation boundary. A transiently empty
        // activity view must retain the newer Turn-B tool watermark.
        clock.set("2026-08-29T00:20:04Z");
        assert!(watchdog.scan(&state, &[]).unwrap().presentations.is_empty());
        clock.set("2026-08-29T00:20:25Z");
        let genuinely_stale = watchdog.scan(&state, &[]).unwrap();
        assert_eq!(genuinely_stale.presentations.len(), 1);
        assert_eq!(
            genuinely_stale.presentations[0].stage,
            TaskWatchdogStage::FirstStale
        );
        assert_ne!(genuinely_stale.presentations[0].fact_id, first_fact_id);
        let genuine_fact_id = genuinely_stale.presentations[0].fact_id.clone();
        assert_eq!(
            watchdog.scan(&state, &[]).unwrap().presentations[0].fact_id,
            genuine_fact_id
        );
        clock.set("2026-08-29T00:30:24Z");
        assert_eq!(watchdog.scan(&state, &[]).unwrap().presentations.len(), 1);
        clock.set("2026-08-29T00:30:25Z");
        let genuinely_escalated = watchdog.scan(&state, &[]).unwrap();
        assert_eq!(genuinely_escalated.presentations.len(), 2);
        assert!(genuinely_escalated
            .presentations
            .iter()
            .any(|fact| fact.stage == TaskWatchdogStage::DirectorEscalated));
    }

    #[test]
    fn recovery_cancels_queued_reminder_and_late_delivery_fact_replays_after_restart() {
        let (temp, clock, watchdog) = harness("2026-08-29T00:10:00Z");
        let state = snapshot(None, AttemptPhase::Running);
        let first = watchdog.scan(&state, &[]).unwrap();
        let notification_id = first.notifications[0].notification_id.clone();
        watchdog
            .record_delivery_fact(
                &notification_id,
                TaskWatchdogDeliveryFactKind::Queued,
                Some("bus-message-1".to_string()),
            )
            .unwrap();
        let output = TaskWatchdogActivityProjection {
            project_id: None,
            assignment_id: "assignment-1".to_string(),
            attempt_number: 1,
            assignee_cutex_session_id: "cutex.worker".to_string(),
            kind: TaskWatchdogActivityKind::LastOutput,
            updated_at: "2026-08-29T00:10:01Z".to_string(),
            source_sequence: 9,
        };
        clock.set("2026-08-29T00:10:02Z");
        let recovered = watchdog.scan(&state, &[output]).unwrap();
        assert_eq!(
            recovered.cancelled_notification_ids,
            vec![notification_id.clone()]
        );
        assert!(recovered.presentations.is_empty());
        assert!(recovered.notifications.is_empty());

        drop(watchdog);
        let restarted = TaskStaleWatchdog::with_clock(
            temp.path(),
            TaskWatchdogConfig::for_tests(600, 600),
            clock,
        )
        .unwrap();
        for _ in 0..2 {
            restarted
                .record_delivery_fact(
                    &notification_id,
                    TaskWatchdogDeliveryFactKind::Delivered,
                    Some("native-submission-1".to_string()),
                )
                .unwrap();
        }
        let retired = restarted.notification(&notification_id).unwrap().unwrap();
        assert!(retired.is_delivered());
        assert_eq!(
            retired
                .facts
                .iter()
                .filter(|fact| fact.kind == TaskWatchdogDeliveryFactKind::Delivered)
                .count(),
            1
        );
        assert!(restarted
            .scan(&state, &[])
            .unwrap()
            .notifications
            .is_empty());
    }

    #[test]
    fn worker_then_director_delivery_is_capped_and_terminal_scan_cancels_outbox() {
        let (_temp, clock, watchdog) = harness("2026-08-29T00:10:00Z");
        let mut state = snapshot(None, AttemptPhase::Running);
        let first = watchdog.scan(&state, &[]).unwrap();
        assert!(matches!(
            first.notifications[0].target,
            TaskWatchdogTarget::AssigneeSession(ref value) if value == "cutex.worker"
        ));
        assert_eq!(
            first.notifications[0].delivery_mode,
            TaskWatchdogDeliveryMode::Soon
        );
        clock.set("2026-08-29T00:20:00Z");
        let second = watchdog.scan(&state, &[]).unwrap();
        let escalation = second
            .notifications
            .iter()
            .find(|notification| notification.stage == TaskWatchdogStage::DirectorEscalated)
            .unwrap();
        assert!(matches!(
            escalation.target,
            TaskWatchdogTarget::AuthoritySeat(ref value) if value == "cutex-director"
        ));
        assert_eq!(
            escalation.delivery_mode,
            TaskWatchdogDeliveryMode::AfterTurn
        );

        for notification in &second.notifications {
            watchdog
                .record_delivery_fact(
                    &notification.notification_id,
                    TaskWatchdogDeliveryFactKind::Queued,
                    Some(format!("queue:{}", notification.notification_id)),
                )
                .unwrap();
        }
        state
            .attempts
            .values_mut()
            .next()
            .unwrap()
            .values_mut()
            .next()
            .unwrap()
            .phase = AttemptPhase::Blocked;
        let stopped = watchdog.scan(&state, &[]).unwrap();
        assert!(stopped.presentations.is_empty());
        assert!(stopped.notifications.is_empty());
        assert_eq!(stopped.cancelled_notification_ids.len(), 2);
    }

    #[test]
    fn additive_json_schema_accepts_exact_fact_and_message_contract() {
        let (_temp, _clock, watchdog) = harness("2026-08-29T00:10:00Z");
        let outcome = watchdog
            .scan(&snapshot(None, AttemptPhase::Running), &[])
            .unwrap();
        let schema: serde_json::Value = serde_json::from_str(TASK_WATCHDOG_CONTRACT_JSON).unwrap();
        let validator = jsonschema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .build(&schema)
            .unwrap();
        let instance = serde_json::json!({
            "fact": outcome.presentations[0],
            "message": outcome.notifications[0],
        });
        assert!(validator.is_valid(&instance));
    }
}
