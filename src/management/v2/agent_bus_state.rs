use std::collections::BTreeMap;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;

use anyhow::Context;
use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use serde_json::Value;

use crate::agent_bus::model::AgentBusMessage;
use crate::app_server::commands::InterAgentContextPersistedReceipt;
use crate::config::atomic::write_private_pretty_json_atomic;
use crate::config::paths::runtime_dir;
use crate::management::v2::model::CutexMessage;
use crate::management::v2::model::EventCorrelation;
use crate::management::v2::model::EventSource;
use crate::management::v2::model::PendingEvent;
use crate::management::v2::repository::management_v2_repository;
use crate::platform::host::current_host_name;

const AGENT_BUS_STATE_FILE: &str = "agent-bus-message-state.json";
const AGENT_BUS_LOCK_FILE: &str = "agent-bus-message-state.lock";

static AGENT_BUS_MESSAGE_REPOSITORY: OnceLock<AgentBusMessageRepository> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentBusMessageSnapshot {
    pub message_id: String,
    pub from_cutex_session_id: Option<String>,
    pub to_cutex_session_id: String,
    pub delivery_mode: String,
    pub content: String,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub a2_submission_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub a4_receipt: Option<InterAgentContextPersistedReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_input: Option<crate::app_server::external_input::Envelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_input_receipt: Option<crate::app_server::external_input::Receipt>,
    /// Observation only, not a task result or an automatic retry permission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_input_last_observed: Option<crate::app_server::external_input::Status>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_input_commit_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredAgentBusMessage {
    owner_cutex_session_id: String,
    from_runtime_agent_id: Option<String>,
    to_runtime_agent_id: Option<String>,
    #[serde(default)]
    canonical_envelope: Option<AgentBusMessage>,
    snapshot: AgentBusMessageSnapshot,
    updated_at: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentBusMessageStore {
    version: u8,
    #[serde(default)]
    external_recovery_actions:
        BTreeMap<String, crate::app_server::external_recovery::RecoveryReceipt>,
    #[serde(default)]
    messages: BTreeMap<String, StoredAgentBusMessage>,
}

pub struct AgentBusMessageRepository {
    root: PathBuf,
    process_lock: Mutex<()>,
}

#[derive(Debug, Clone)]
pub struct AgentBusQueuedMessage {
    pub owner_cutex_session_id: String,
    pub message_id: String,
    pub from_cutex_session_id: Option<String>,
    pub to_cutex_session_id: String,
    pub from_runtime_agent_id: Option<String>,
    pub to_runtime_agent_id: Option<String>,
    pub delivery_mode: String,
    pub content: String,
    pub queued_at: DateTime<Utc>,
    pub canonical_envelope: AgentBusMessage,
    pub semantic_sha256: String,
}

#[derive(Debug, Clone)]
pub struct PendingAgentBusMessage {
    pub owner_cutex_session_id: String,
    pub target_cutex_session_id: String,
    pub canonical_envelope: AgentBusMessage,
    pub semantic_sha256: String,
}

pub fn agent_bus_message_repository() -> anyhow::Result<&'static AgentBusMessageRepository> {
    if let Some(repository) = AGENT_BUS_MESSAGE_REPOSITORY.get() {
        return Ok(repository);
    }
    #[cfg(test)]
    require_private_test_home()?;
    let repository = AgentBusMessageRepository::open(runtime_dir()?.join("management-v2"))?;
    let _ = AGENT_BUS_MESSAGE_REPOSITORY.set(repository);
    AGENT_BUS_MESSAGE_REPOSITORY
        .get()
        .context("management v2 agent-bus repository initialization raced")
}

#[cfg(test)]
fn require_private_test_home() -> anyhow::Result<()> {
    let home = std::env::var("HOME").context("tests require HOME")?;
    let private = std::env::var("CUTEX_TEST_PRIVATE_HOME")
        .context("tests touching Agent Bus message state require CUTEX_TEST_PRIVATE_HOME")?;
    validate_private_test_home(&home, &private)?;
    if !Path::new(&private)
        .join(".cutex-test-private-home")
        .is_file()
    {
        anyhow::bail!(
            "refusing to open Agent Bus message state outside the verified private test HOME"
        );
    }
    Ok(())
}

#[cfg(test)]
fn validate_private_test_home(home: &str, private: &str) -> anyhow::Result<()> {
    if home.is_empty() || private.is_empty() || home != private || private == "/" {
        anyhow::bail!("Agent Bus repository test HOME is not private and exact");
    }
    Ok(())
}

impl AgentBusMessageRepository {
    pub(crate) fn recovery_action(
        &self,
        action: &str,
    ) -> anyhow::Result<Option<crate::app_server::external_recovery::RecoveryReceipt>> {
        self.read(|s| Ok(s.external_recovery_actions.get(action).cloned()))
    }

    pub(crate) fn save_recovery_action(
        &self,
        receipt: &crate::app_server::external_recovery::RecoveryReceipt,
    ) -> anyhow::Result<()> {
        self.mutate(|s| {
            if let Some(old) = s.external_recovery_actions.get(&receipt.action_id) {
                anyhow::ensure!(
                    old.review == receipt.review && old.retry_id == receipt.retry_id,
                    "recovery action semantic conflict"
                );
                anyhow::ensure!(
                    old.result.is_none() || old.result == receipt.result,
                    "recovery receipt conflict"
                );
            }
            s.external_recovery_actions
                .insert(receipt.action_id.clone(), receipt.clone());
            s.version = 4;
            Ok(())
        })
    }
    pub fn open(root: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        secure_directory(&root)?;
        Ok(Self {
            root,
            process_lock: Mutex::new(()),
        })
    }

    /// Returns `true` only for the first durable commit and `false` for an
    /// exact replay of the same message identity and semantic content.
    pub fn record_queued(&self, message: AgentBusQueuedMessage) -> anyhow::Result<bool> {
        self.record_queued_internal(message, true)
    }

    #[cfg(test)]
    fn record_queued_isolated(&self, message: AgentBusQueuedMessage) -> anyhow::Result<bool> {
        self.record_queued_internal(message, false)
    }

    fn record_queued_internal(
        &self,
        message: AgentBusQueuedMessage,
        record_management_event: bool,
    ) -> anyhow::Result<bool> {
        validate_session_identity(&message.owner_cutex_session_id)?;
        if let Some(from) = message.from_cutex_session_id.as_deref() {
            validate_session_identity(from)?;
        }
        validate_session_identity(&message.to_cutex_session_id)?;
        if message.message_id.is_empty() {
            anyhow::bail!("agent-bus messageId must not be empty");
        }
        validate_semantic_sha256(&message.semantic_sha256)?;
        if message.canonical_envelope.id != message.message_id
            || message.canonical_envelope.to_cutex_session_id.as_deref()
                != Some(message.to_cutex_session_id.as_str())
        {
            anyhow::bail!("agent-bus canonical envelope identity is inconsistent");
        }
        if !matches!(
            message.delivery_mode.as_str(),
            "after_turn" | "soon" | "passive"
        ) {
            anyhow::bail!("agent-bus delivery mode is outside the v2 contract");
        }
        self.with_lock(|path| {
            let mut store = load_store(path)?;
            if let Some(existing) = store.messages.get(&message.message_id) {
                if existing.owner_cutex_session_id == message.owner_cutex_session_id
                    && existing.snapshot.from_cutex_session_id == message.from_cutex_session_id
                    && existing.snapshot.to_cutex_session_id == message.to_cutex_session_id
                    && existing.snapshot.delivery_mode == message.delivery_mode
                    && existing.snapshot.content == message.content
                    && existing.snapshot.semantic_sha256.as_deref()
                        == Some(message.semantic_sha256.as_str())
                {
                    return Ok(false);
                }
                anyhow::bail!("agent-bus messageId was reused with different canonical content");
            }
            if record_management_event && message.from_cutex_session_id.is_some() {
                append_event(
                    &message.owner_cutex_session_id,
                    &message.message_id,
                    "cutex/agentBus/messageQueued",
                    json!({
                        "messageId": message.message_id,
                        "fromCutexSessionId": message.from_cutex_session_id,
                        "toCutexSessionId": message.to_cutex_session_id,
                        "fromRuntimeAgentId": message.from_runtime_agent_id,
                        "toRuntimeAgentId": message.to_runtime_agent_id,
                        "deliveryMode": message.delivery_mode,
                        "content": message.content,
                        "queuedAt": message.queued_at.to_rfc3339(),
                    }),
                )?;
            }
            store.messages.insert(
                message.message_id.clone(),
                StoredAgentBusMessage {
                    owner_cutex_session_id: message.owner_cutex_session_id,
                    from_runtime_agent_id: message.from_runtime_agent_id,
                    to_runtime_agent_id: message.to_runtime_agent_id,
                    canonical_envelope: Some(message.canonical_envelope),
                    snapshot: AgentBusMessageSnapshot {
                        message_id: message.message_id,
                        from_cutex_session_id: message.from_cutex_session_id,
                        to_cutex_session_id: message.to_cutex_session_id,
                        delivery_mode: message.delivery_mode,
                        content: message.content,
                        state: "pending".to_string(),
                        semantic_sha256: Some(message.semantic_sha256),
                        a2_submission_id: None,
                        a4_receipt: None,
                        external_input: None,
                        external_input_receipt: None,
                        external_input_last_observed: None,
                        external_input_commit_generation: None,
                        error: None,
                    },
                    updated_at: message.queued_at.to_rfc3339(),
                },
            );
            write_private_pretty_json_atomic(path, &store, "management v2 agent-bus state")?;
            Ok(true)
        })
    }

    /// Freeze from the authenticated, persisted canonical record, never a poll
    /// payload. V3 prevents older writers from dropping native receipt fields.
    pub(crate) fn freeze_external_input(
        &self,
        owner: &str,
        message_id: &str,
        make: impl FnOnce(
            &AgentBusMessage,
        ) -> anyhow::Result<crate::app_server::external_input::Envelope>,
    ) -> anyhow::Result<crate::app_server::external_input::Envelope> {
        self.mutate(|store| {
            let stored = store
                .messages
                .get_mut(message_id)
                .context("canonical message absent; explicit review required")?;
            anyhow::ensure!(
                stored.snapshot.to_cutex_session_id == owner,
                "canonical recipient changed"
            );
            if let Some(envelope) = &stored.snapshot.external_input {
                envelope.validate()?;
                return Ok(envelope.clone());
            }
            anyhow::ensure!(
                stored.snapshot.state == "pending"
                    && stored.snapshot.a2_submission_id.is_none()
                    && stored.snapshot.a4_receipt.is_none(),
                "legacy native submission ambiguous; explicit review required"
            );
            let canonical = stored
                .canonical_envelope
                .as_ref()
                .context("legacy canonical envelope absent; explicit review required")?;
            let envelope = make(canonical)?;
            envelope.validate()?;
            anyhow::ensure!(
                envelope.owner_id == owner
                    && envelope.message.id == canonical.id
                    && canonical.id == message_id,
                "native envelope recipient/message mismatch"
            );
            stored.snapshot.external_input = Some(envelope.clone());
            store.version = store.version.max(
                if envelope.message.delivery == crate::app_server::external_input::Delivery::Soon {
                    4
                } else {
                    3
                },
            );
            Ok(envelope)
        })
    }

    /// Caller holds current lifecycle/occurrence fences. The frozen envelope
    /// and original receipt are compared again under this repository's lock.
    pub(crate) fn record_external_input_delivered(
        &self,
        owner: &str,
        message_id: &str,
        envelope: &crate::app_server::external_input::Envelope,
        receipt: &crate::app_server::external_input::Receipt,
        generation: u64,
    ) -> anyhow::Result<()> {
        self.mutate(|store| {
            let stored = store
                .messages
                .get_mut(message_id)
                .context("canonical message disappeared")?;
            anyhow::ensure!(
                stored.snapshot.to_cutex_session_id == owner
                    && stored.snapshot.external_input.as_ref() == Some(envelope),
                "native business commit CAS conflict"
            );
            anyhow::ensure!(
                stored.snapshot.state == "pending" || stored.snapshot.state == "delivered",
                "native business state conflict"
            );
            anyhow::ensure!(
                stored
                    .snapshot
                    .external_input_receipt
                    .as_ref()
                    .is_none_or(|r| r == receipt),
                "native receipt replay conflict"
            );
            receipt.validate(
                &crate::launch::stock::ExternalInputBinding {
                    version: 1,
                    owner_id: envelope.owner_id.clone(),
                    thread_id: envelope.thread_id.clone(),
                    runtime_generation: envelope.runtime_generation,
                    canonical_byte_limit: Default::default(),
                },
                &envelope.key(),
            )?;
            stored.snapshot.external_input_receipt = Some(receipt.clone());
            stored
                .snapshot
                .external_input_commit_generation
                .get_or_insert(generation);
            stored.snapshot.state = "delivered".into();
            stored.snapshot.error = None;
            stored.updated_at = Utc::now().to_rfc3339();
            Ok(())
        })
    }

    pub(crate) fn record_external_input_error(
        &self,
        message_id: &str,
        error: &str,
    ) -> anyhow::Result<()> {
        self.mutate(|store| {
            let stored = store
                .messages
                .get_mut(message_id)
                .context("canonical message absent")?;
            if stored.snapshot.state == "pending" {
                stored.snapshot.error =
                    Some(json!({"code":"external_input_pending", "message":error}));
            }
            Ok(())
        })
    }

    pub(crate) fn observe_external_input(
        &self,
        message_id: &str,
        frozen: &crate::app_server::external_input::Envelope,
        observed: &crate::app_server::external_input::Status,
    ) -> anyhow::Result<()> {
        self.mutate(|store| {
            let stored = store
                .messages
                .get_mut(message_id)
                .context("native business record absent")?;
            anyhow::ensure!(
                stored.snapshot.external_input.as_ref() == Some(frozen)
                    && observed.message_id == frozen.message.id
                    && observed.semantic_sha256 == frozen.semantic_sha256,
                "native observation CAS conflict"
            );
            stored.snapshot.external_input_last_observed = Some(observed.clone());
            Ok(())
        })
    }

    pub fn record_delivered(
        &self,
        owner_cutex_session_id: &str,
        message_id: &str,
        receipt: &InterAgentContextPersistedReceipt,
        delivered_at: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        let Some(stored) = self.get(message_id)? else {
            return Ok(false);
        };
        if stored.owner_cutex_session_id != owner_cutex_session_id {
            anyhow::bail!("agent-bus message owner session changed");
        }
        if stored.snapshot.state == "delivered" {
            if stored.snapshot.a4_receipt.as_ref() == Some(receipt) {
                return Ok(true);
            }
            anyhow::bail!("agent-bus message A4 receipt changed after delivery");
        }
        if stored.snapshot.from_cutex_session_id.is_some() {
            append_event(
                owner_cutex_session_id,
                message_id,
                "cutex/agentBus/messageDelivered",
                json!({
                    "messageId": message_id,
                    "fromCutexSessionId": stored.snapshot.from_cutex_session_id,
                    "toCutexSessionId": stored.snapshot.to_cutex_session_id,
                    "fromRuntimeAgentId": stored.from_runtime_agent_id,
                    "toRuntimeAgentId": stored.to_runtime_agent_id,
                    // Keep the frozen management-event schema stable. The v2
                    // ledger above is the authoritative home of the complete A4
                    // receipt; this legacy projection carries its stable identity.
                    "nativeSubmissionId": receipt.receipt_id,
                    "deliveredAt": delivered_at.to_rfc3339(),
                }),
            )?;
        }
        self.mutate(|store| {
            let stored = store.messages.get_mut(message_id).with_context(|| {
                format!("agent-bus v2 message state disappeared for {message_id}")
            })?;
            stored.snapshot.state = "delivered".to_string();
            stored.snapshot.a4_receipt = Some(receipt.clone());
            stored.snapshot.error = None;
            stored.updated_at = delivered_at.to_rfc3339();
            Ok(())
        })?;
        Ok(true)
    }

    pub fn record_a2_submission(
        &self,
        owner_cutex_session_id: &str,
        message_id: &str,
        native_submission_id: &str,
        submitted_at: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        let Some(stored) = self.get(message_id)? else {
            return Ok(false);
        };
        if stored.owner_cutex_session_id != owner_cutex_session_id {
            anyhow::bail!("agent-bus message owner session changed");
        }
        self.mutate(|store| {
            let stored = store
                .messages
                .get_mut(message_id)
                .context("agent-bus message disappeared")?;
            stored.snapshot.a2_submission_id = Some(native_submission_id.to_string());
            stored.updated_at = submitted_at.to_rfc3339();
            Ok(())
        })?;
        Ok(true)
    }

    pub fn record_quarantined(
        &self,
        owner_cutex_session_id: &str,
        message_id: &str,
        error: Value,
        at: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        let Some(stored) = self.get(message_id)? else {
            return Ok(false);
        };
        if stored.owner_cutex_session_id != owner_cutex_session_id {
            anyhow::bail!("agent-bus message owner session changed");
        }
        self.mutate(|store| {
            let stored = store
                .messages
                .get_mut(message_id)
                .context("agent-bus message disappeared")?;
            stored.snapshot.state = "quarantined".to_string();
            stored.snapshot.error = Some(error);
            stored.updated_at = at.to_rfc3339();
            Ok(())
        })?;
        Ok(true)
    }

    pub fn pending_v2(&self) -> anyhow::Result<Vec<PendingAgentBusMessage>> {
        self.read(|store| {
            store
                .messages
                .values()
                .filter(|stored| stored.snapshot.state == "pending")
                .map(|stored| {
                    Ok(PendingAgentBusMessage {
                        owner_cutex_session_id: stored.owner_cutex_session_id.clone(),
                        target_cutex_session_id: stored.snapshot.to_cutex_session_id.clone(),
                        canonical_envelope: stored
                            .canonical_envelope
                            .clone()
                            .context("pending v2 message lacks canonical envelope")?,
                        semantic_sha256: stored
                            .snapshot
                            .semantic_sha256
                            .clone()
                            .context("pending v2 message lacks semantic digest")?,
                    })
                })
                .collect()
        })
    }

    pub fn semantic_sha256(&self, message_id: &str) -> anyhow::Result<Option<String>> {
        Ok(self
            .get(message_id)?
            .and_then(|stored| stored.snapshot.semantic_sha256))
    }

    pub(crate) fn canonical_message(&self, message_id: &str) -> anyhow::Result<AgentBusMessage> {
        self.get(message_id)?
            .and_then(|stored| stored.canonical_envelope)
            .context("canonical business message unavailable; explicit review required")
    }

    pub fn snapshot_by_message_id(
        &self,
        message_id: &str,
    ) -> anyhow::Result<Option<AgentBusMessageSnapshot>> {
        Ok(self.get(message_id)?.map(|stored| stored.snapshot))
    }

    pub fn record_failed(
        &self,
        owner_cutex_session_id: &str,
        message_id: &str,
        error: Value,
        failed_at: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        let Some(stored) = self.get(message_id)? else {
            return Ok(false);
        };
        if stored.snapshot.from_cutex_session_id.is_some() {
            append_event(
                owner_cutex_session_id,
                message_id,
                "cutex/agentBus/messageFailed",
                json!({
                    "messageId": message_id,
                    "fromCutexSessionId": stored.snapshot.from_cutex_session_id,
                    "toCutexSessionId": stored.snapshot.to_cutex_session_id,
                    "fromRuntimeAgentId": stored.from_runtime_agent_id,
                    "toRuntimeAgentId": stored.to_runtime_agent_id,
                    "error": error,
                    "failedAt": failed_at.to_rfc3339(),
                }),
            )?;
        }
        self.mutate(|store| {
            let stored = store.messages.get_mut(message_id).with_context(|| {
                format!("agent-bus v2 message state disappeared for {message_id}")
            })?;
            stored.snapshot.state = "failed".to_string();
            stored.snapshot.error = Some(error);
            stored.updated_at = failed_at.to_rfc3339();
            Ok(())
        })?;
        Ok(true)
    }

    pub fn snapshot(&self, cutex_session_id: &str) -> anyhow::Result<Vec<Value>> {
        self.read(|store| {
            store
                .messages
                .values()
                .filter(|stored| stored.owner_cutex_session_id == cutex_session_id)
                .map(|stored| serde_json::to_value(&stored.snapshot).map_err(Into::into))
                .collect()
        })
    }

    pub fn migrate_legacy_v1(&self) -> anyhow::Result<()> {
        self.with_lock(|path| {
            let mut store = load_store_unchecked(path)?;
            if store.version != 1 {
                return Ok(());
            }
            for stored in store.messages.values_mut() {
                if stored.snapshot.state != "delivered" {
                    stored.snapshot.state = "quarantined".to_string();
                    stored.snapshot.error = Some(json!({
                        "source": "cutex",
                        "code": "legacy_v1_canonical_envelope_unavailable",
                        "message": "legacy ordinary message cannot be safely redriven",
                        "retryable": false,
                        "details": {}
                    }));
                }
            }
            store.version = 2;
            write_private_pretty_json_atomic(path, &store, "management v2 agent-bus state")
        })
    }

    fn get(&self, message_id: &str) -> anyhow::Result<Option<StoredAgentBusMessage>> {
        self.read(|store| Ok(store.messages.get(message_id).cloned()))
    }

    fn mutate<T>(
        &self,
        action: impl FnOnce(&mut AgentBusMessageStore) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.with_lock(|path| {
            let mut store = load_store(path)?;
            let result = action(&mut store)?;
            write_private_pretty_json_atomic(path, &store, "management v2 agent-bus state")?;
            Ok(result)
        })
    }

    fn read<T>(
        &self,
        action: impl FnOnce(&AgentBusMessageStore) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.with_lock(|path| action(&load_store(path)?))
    }

    fn with_lock<T>(&self, action: impl FnOnce(&Path) -> anyhow::Result<T>) -> anyhow::Result<T> {
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("management v2 agent-bus lock was poisoned"))?;
        let lock_file = open_private_lock(&self.root.join(AGENT_BUS_LOCK_FILE))?;
        lock_file.lock()?;
        let result = action(&self.root.join(AGENT_BUS_STATE_FILE));
        let unlock = lock_file.unlock();
        if result.is_ok() {
            unlock?;
        }
        result
    }
}

fn append_event(
    cutex_session_id: &str,
    message_id: &str,
    method: &str,
    params: Value,
) -> anyhow::Result<()> {
    management_v2_repository()?.append(PendingEvent {
        cutex_session_id: cutex_session_id.to_string(),
        host_id: current_host_name(),
        source: EventSource::Cutex,
        schema: None,
        correlation: EventCorrelation {
            agent_bus_message_id: Some(message_id.to_string()),
            ..Default::default()
        },
        native: None,
        cutex: Some(CutexMessage {
            method: method.to_string(),
            params,
        }),
    })?;
    Ok(())
}

fn validate_session_identity(value: &str) -> anyhow::Result<()> {
    if value.is_empty() || value.contains('/') || value.contains('\\') {
        anyhow::bail!("agent-bus cutex session identity is invalid");
    }
    Ok(())
}

fn load_store(path: &Path) -> anyhow::Result<AgentBusMessageStore> {
    let store = load_store_unchecked(path)?;
    if !matches!(store.version, 2 | 3 | 4) {
        anyhow::bail!("unsupported management v2 agent-bus state version");
    }
    Ok(store)
}

fn load_store_unchecked(path: &Path) -> anyhow::Result<AgentBusMessageStore> {
    match fs::read(path) {
        Ok(bytes) => {
            let store: AgentBusMessageStore = serde_json::from_slice(&bytes)?;
            Ok(store)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(AgentBusMessageStore {
            external_recovery_actions: BTreeMap::new(),
            version: 2,
            messages: BTreeMap::new(),
        }),
        Err(error) => Err(error).with_context(|| {
            format!(
                "failed to read management v2 agent-bus state: {}",
                path.display()
            )
        }),
    }
}

fn validate_semantic_sha256(value: &str) -> anyhow::Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        anyhow::bail!("agent-bus semanticSha256 must be 64 lowercase hexadecimal characters");
    }
    Ok(())
}

fn open_private_lock(path: &Path) -> anyhow::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    let file = options.open(path)?;
    secure_file(path)?;
    Ok(file)
}

#[cfg(unix)]
fn secure_directory(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn secure_directory(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn secure_file(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn secure_file(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_bus_freeze_replay_cas_and_native_receipt_survive_reopen() {
        use crate::app_server::external_input::*;
        let root =
            std::env::temp_dir().join(format!("cutex-external-store-{}", uuid::Uuid::new_v4()));
        let repository = AgentBusMessageRepository::open(&root).unwrap();
        let queued = system_queued_message("original");
        let owner = queued.to_cutex_session_id.clone();
        repository.record_queued_isolated(queued).unwrap();
        let frozen = repository
            .freeze_external_input(&owner, "jsc_stable", |m| {
                let mut e = Envelope {
                    version: 1,
                    owner_id: owner.clone(),
                    thread_id: "native-test".into(),
                    runtime_generation: 1,
                    message: Message {
                        id: m.id.clone(),
                        source: Source {
                            kind: SourceKind::Service,
                            id: "cutex-job-service".into(),
                        },
                        event_type: "job_completion".into(),
                        delivery: Delivery::AfterTurn,
                        text: m.content.clone(),
                    },
                    semantic_sha256: String::new(),
                };
                e.semantic_sha256 = e.digest();
                Ok(e)
            })
            .unwrap();
        let reopened = AgentBusMessageRepository::open(&root).unwrap();
        let mut permission = crate::app_server::external_recovery::RecoveryReceipt {
            action_id: "private-recovery".into(),
            retry_id: "private-retry".into(),
            review: crate::app_server::external_recovery::RecoveryReview {
                envelope: frozen.clone(),
                binding: crate::launch::stock::ExternalInputBinding {
                    version: 1,
                    owner_id: owner.clone(),
                    thread_id: frozen.thread_id.clone(),
                    runtime_generation: 1,
                    canonical_byte_limit: Default::default(),
                },
                status: Status {
                    message_id: frozen.message.id.clone(),
                    semantic_sha256: frozen.semantic_sha256.clone(),
                    delivery_state: DeliveryState::Unknown,
                    receipt: None,
                    processing: ProcessingStatus {
                        state: ProcessingState::Held,
                        attempt_id: None,
                        reason: Some(HoldReason::NoOutput),
                    },
                },
                durable_sha256: "private-spec".into(),
                authority_sha256: "private-authority".into(),
                warning: crate::app_server::external_recovery::REPEAT_WARNING.into(),
            },
            result: None,
        };
        reopened.save_recovery_action(&permission).unwrap();
        reopened.save_recovery_action(&permission).unwrap();
        assert_eq!(
            reopened.recovery_action("private-recovery").unwrap(),
            Some(permission.clone())
        );
        let mut conflict = permission.clone();
        conflict.retry_id = "different".into();
        assert!(reopened.save_recovery_action(&conflict).is_err());
        permission.result = Some(RetryResponse {
            version: 1,
            owner_id: owner.clone(),
            thread_id: frozen.thread_id.clone(),
            runtime_generation: 1,
            message_id: frozen.message.id.clone(),
            semantic_sha256: frozen.semantic_sha256.clone(),
            expected_attempt_id: None,
            retry_id: permission.retry_id.clone(),
            disposition: RetryDisposition::Released,
        });
        reopened.save_recovery_action(&permission).unwrap();
        reopened.save_recovery_action(&permission).unwrap();
        conflict = permission.clone();
        conflict.result = None;
        assert!(reopened.save_recovery_action(&conflict).is_err());
        reopened
            .mutate(|s| {
                assert_eq!(s.version, 4);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            reopened
                .freeze_external_input(&owner, "jsc_stable", |_| panic!(
                    "must not reformat frozen record"
                ))
                .unwrap(),
            frozen
        );
        let mut receipt = Receipt {
            schema: "codex.external-input-receipt.v1".into(),
            receipt_id: String::new(),
            owner_id: owner.clone(),
            thread_id: frozen.thread_id.clone(),
            message_id: frozen.message.id.clone(),
            semantic_sha256: frozen.semantic_sha256.clone(),
            response_item_id: frozen.message.id.clone(),
            turn_id: "turn-test".into(),
            ordinal: 1,
        };
        receipt.receipt_id = receipt.digest_id();
        let mut changed = frozen.clone();
        changed.message.text = "changed".into();
        changed.semantic_sha256 = changed.digest();
        assert!(reopened
            .record_external_input_delivered(&owner, "jsc_stable", &changed, &receipt, 2)
            .is_err());
        assert!(reopened
            .record_external_input_delivered("foreign", "jsc_stable", &frozen, &receipt, 2)
            .is_err());
        reopened
            .record_external_input_delivered(&owner, "jsc_stable", &frozen, &receipt, 2)
            .unwrap();
        reopened
            .record_external_input_delivered(&owner, "jsc_stable", &frozen, &receipt, 3)
            .unwrap();
        let snapshot = reopened
            .snapshot_by_message_id("jsc_stable")
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.external_input_receipt, Some(receipt.clone()));
        assert!(snapshot.a4_receipt.is_none());
        assert_eq!(snapshot.state, "delivered");
        assert_eq!(snapshot.external_input_commit_generation, Some(2));
        receipt.ordinal = 2;
        receipt.receipt_id = receipt.digest_id();
        assert!(reopened
            .record_external_input_delivered(&owner, "jsc_stable", &frozen, &receipt, 3)
            .is_err());
        let raw: Value =
            serde_json::from_slice(&fs::read(root.join(AGENT_BUS_STATE_FILE)).unwrap()).unwrap();
        assert_eq!(raw["version"], 4);
        assert_eq!(
            AgentBusMessageRepository::open(&root)
                .unwrap()
                .recovery_action("private-recovery")
                .unwrap(),
            Some(permission)
        );
        fs::remove_dir_all(root).unwrap();
    }

    fn system_queued_message(content: &str) -> AgentBusQueuedMessage {
        let target = "cutex.11111111-1111-4111-8111-111111111111".to_string();
        let envelope = AgentBusMessage {
            id: "jsc_stable".to_string(),
            kind: crate::agent_bus::model::AgentBusEnvelopeKind::Message,
            from: "cutex-job-service".to_string(),
            to: target.clone(),
            from_cutex_session_id: None,
            to_cutex_session_id: Some(target.clone()),
            content: content.to_string(),
            delivery_mode: crate::agent_bus::delivery::AgentDeliveryMode::AfterTurn,
            trigger_turn: true,
            created_at_epoch_secs: 1,
            sender_kind: crate::agent_bus::model::AgentMessageKind::JobServiceSystem,
            display_source: Some("Cutex Job Service".to_string()),
            submit_mode: None,
            control_type: Some(crate::agent_bus::model::JOB_SERVICE_COMPLETION_SCHEMA.to_string()),
            control_payload: None,
            external_action_id: Some("job-1".to_string()),
            external_message_id: Some("event-1".to_string()),
        };
        AgentBusQueuedMessage {
            owner_cutex_session_id: target.clone(),
            message_id: envelope.id.clone(),
            from_cutex_session_id: None,
            to_cutex_session_id: target,
            from_runtime_agent_id: None,
            to_runtime_agent_id: None,
            delivery_mode: "after_turn".to_string(),
            content: content.to_string(),
            queued_at: Utc::now(),
            canonical_envelope: envelope,
            semantic_sha256: crate::task_service::sha256_bytes(content.as_bytes())
                .as_str()
                .to_string(),
        }
    }

    #[test]
    fn system_source_persists_before_redrive_and_changed_replay_conflicts() {
        let root = std::env::temp_dir().join(format!("cutex-js3a-state-{}", uuid::Uuid::new_v4()));
        let repository = AgentBusMessageRepository::open(&root).unwrap();
        let queued = system_queued_message("terminal result");
        repository.record_queued_isolated(queued.clone()).unwrap();
        repository.record_queued_isolated(queued).unwrap();
        let reopened = AgentBusMessageRepository::open(&root).unwrap();
        let pending = reopened.pending_v2().unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0]
            .canonical_envelope
            .from_cutex_session_id
            .is_none());
        let changed = system_queued_message("changed terminal result");
        assert!(reopened.record_queued_isolated(changed).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn queued_and_delivered_updates_one_bootstrap_message() {
        let root =
            std::env::temp_dir().join(format!("cutex-v2-agent-bus-{}", uuid::Uuid::new_v4()));
        let repository = AgentBusMessageRepository::open(&root).expect("open state repository");
        // State behavior is exercised independently from the process-global event repository.
        repository
            .mutate(|store| {
                store.messages.insert(
                    "message-1".to_string(),
                    StoredAgentBusMessage {
                        owner_cutex_session_id: "cutex.target".to_string(),
                        from_runtime_agent_id: Some("runtime-from".to_string()),
                        to_runtime_agent_id: Some("runtime-to".to_string()),
                        canonical_envelope: None,
                        snapshot: AgentBusMessageSnapshot {
                            message_id: "message-1".to_string(),
                            from_cutex_session_id: Some("cutex.source".to_string()),
                            to_cutex_session_id: "cutex.target".to_string(),
                            delivery_mode: "after_turn".to_string(),
                            content: "hello".to_string(),
                            state: "queued".to_string(),
                            semantic_sha256: None,
                            a2_submission_id: None,
                            a4_receipt: None,
                            external_input: None,
                            external_input_receipt: None,
                            external_input_last_observed: None,
                            external_input_commit_generation: None,
                            error: None,
                        },
                        updated_at: Utc::now().to_rfc3339(),
                    },
                );
                Ok(())
            })
            .expect("seed state");
        let snapshot = repository.snapshot("cutex.target").expect("snapshot");
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0]["state"], "queued");
        fs::remove_dir_all(root).expect("remove state repository");
    }

    #[test]
    fn legacy_v1_undelivered_message_is_durably_quarantined() {
        let root =
            std::env::temp_dir().join(format!("cutex-v1-agent-bus-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join(AGENT_BUS_STATE_FILE);
        fs::write(
            &path,
            serde_json::to_vec_pretty(&json!({
                "version": 1,
                "messages": {
                    "legacy-1": {
                        "ownerCutexSessionId": "cutex.target",
                        "fromRuntimeAgentId": "runtime-old",
                        "toRuntimeAgentId": "runtime-target-old",
                        "snapshot": {
                            "messageId": "legacy-1",
                            "fromCutexSessionId": "cutex.source",
                            "toCutexSessionId": "cutex.target",
                            "deliveryMode": "soon",
                            "content": "legacy",
                            "state": "queued"
                        },
                        "updatedAt": "2026-08-30T00:00:00Z"
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let repository = AgentBusMessageRepository::open(&root).unwrap();
        repository.migrate_legacy_v1().unwrap();
        let snapshot = repository.snapshot("cutex.target").unwrap();
        assert_eq!(snapshot[0]["state"], "quarantined");
        assert_eq!(
            snapshot[0]["error"]["code"],
            "legacy_v1_canonical_envelope_unavailable"
        );
        let persisted: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(persisted["version"], 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn process_global_repository_guard_rejects_ambient_or_mismatched_home() {
        assert!(validate_private_test_home("/home/user", "").is_err());
        assert!(validate_private_test_home("/home/user", "/tmp/private").is_err());
        assert!(validate_private_test_home("/", "/").is_err());
        assert!(validate_private_test_home("/tmp/private", "/tmp/private").is_ok());
    }
}
