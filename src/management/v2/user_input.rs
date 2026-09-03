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
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::config::atomic::write_private_pretty_json_atomic;
use crate::config::paths::runtime_dir;
use crate::management::v2::model::MAX_SAFE_SEQUENCE;
use crate::management::v2::native_requests::canonical_request_sha256;

const USER_INPUT_STATE_FILE: &str = "user-input-state.json";
const USER_INPUT_LOCK_FILE: &str = "user-input-state.lock";

static USER_INPUT_REPOSITORY: OnceLock<UserInputRepository> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UserInputOriginKind {
    Android,
    Backend,
    Tui,
    AgentBus,
    Automation,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UserInputOrigin {
    pub kind: UserInputOriginKind,
    pub client_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UserInputStrategy {
    Auto,
    Queue,
    Interrupt,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UserInputSubmitParams {
    pub client_user_message_id: String,
    pub origin: UserInputOrigin,
    pub strategy: UserInputStrategy,
    pub input: Vec<Value>,
    pub expected_turn_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UserInputSubmitCommand {
    pub management_request_id: String,
    pub cutex_session_id: String,
    pub thread_id: String,
    pub params: UserInputSubmitParams,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UserInputDisposition {
    Started,
    Steered,
    Queued,
    Deduplicated,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UserInputSubmitExecution {
    pub disposition: UserInputDisposition,
    pub app_server_accepted: bool,
    pub native_request_id: Option<Value>,
    pub native_method: Option<String>,
    pub turn_id: Option<String>,
    pub queue: Option<UserInputQueueItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UserInputExecutionError {
    pub stage: String,
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub details: Value,
    pub outcome_unknown: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UserInputQueueItem {
    pub queue_id: String,
    pub client_user_message_id: String,
    pub origin: UserInputOrigin,
    pub input: Vec<Value>,
    pub position: u64,
    pub revision: u64,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum QueueEnqueueDecision {
    Queued(UserInputQueueItem),
    Deduplicated(UserInputQueueItem),
    ClientMessageConflict,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClientIdentityDecision {
    New,
    Existing,
    Conflict,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredQueueItem {
    management_request_id: String,
    thread_id: String,
    item: UserInputQueueItem,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientInputIdentity {
    canonical_sha256: String,
    first_seen_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionInputState {
    queue_revision: u64,
    #[serde(default)]
    items: Vec<StoredQueueItem>,
    #[serde(default)]
    identities: BTreeMap<String, ClientInputIdentity>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserInputStateStore {
    version: u8,
    #[serde(default)]
    sessions: BTreeMap<String, SessionInputState>,
}

impl Default for UserInputStateStore {
    fn default() -> Self {
        Self {
            version: 1,
            sessions: BTreeMap::new(),
        }
    }
}

pub struct UserInputRepository {
    root: PathBuf,
    process_lock: Mutex<()>,
}

pub fn user_input_repository() -> anyhow::Result<&'static UserInputRepository> {
    if let Some(repository) = USER_INPUT_REPOSITORY.get() {
        return Ok(repository);
    }
    let repository = UserInputRepository::open(runtime_dir()?.join("management-v2"))?;
    let _ = USER_INPUT_REPOSITORY.set(repository);
    USER_INPUT_REPOSITORY
        .get()
        .context("management v2 user-input repository initialization raced")
}

pub fn parse_user_input_submit_params(value: &Value) -> Result<UserInputSubmitParams, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "cutex/userInput/submit params must be an object".to_string())?;
    let allowed = [
        "clientUserMessageId",
        "origin",
        "strategy",
        "input",
        "expectedTurnId",
    ];
    if object.keys().any(|key| !allowed.contains(&key.as_str()))
        || !["clientUserMessageId", "origin", "strategy", "input"]
            .iter()
            .all(|key| object.contains_key(*key))
    {
        return Err("cutex/userInput/submit params contain missing or unknown fields".to_string());
    }
    let client_user_message_id = object
        .get("clientUserMessageId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "clientUserMessageId must be a non-empty string".to_string())?
        .to_string();
    let origin: UserInputOrigin = serde_json::from_value(
        object
            .get("origin")
            .cloned()
            .ok_or_else(|| "origin is required".to_string())?,
    )
    .map_err(|error| format!("invalid origin: {error}"))?;
    let origin_object = object
        .get("origin")
        .and_then(Value::as_object)
        .ok_or_else(|| "origin must be an object".to_string())?;
    if origin_object.len() != 2
        || !origin_object.contains_key("kind")
        || !origin_object.contains_key("clientId")
        || origin.client_id.is_empty()
    {
        return Err("origin requires exactly non-empty kind and clientId".to_string());
    }
    let strategy: UserInputStrategy = serde_json::from_value(
        object
            .get("strategy")
            .cloned()
            .ok_or_else(|| "strategy is required".to_string())?,
    )
    .map_err(|error| format!("invalid strategy: {error}"))?;
    let input = object
        .get("input")
        .and_then(Value::as_array)
        .filter(|input| !input.is_empty())
        .ok_or_else(|| "input must be a non-empty array".to_string())?
        .clone();
    for item in &input {
        validate_user_input(item)?;
    }
    let expected_turn_id = match object.get("expectedTurnId") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
        Some(_) => {
            return Err("expectedTurnId must be a non-empty string or null".to_string());
        }
    };
    Ok(UserInputSubmitParams {
        client_user_message_id,
        origin,
        strategy,
        input,
        expected_turn_id,
    })
}

pub fn validate_user_input(value: &Value) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| "each input item must be an object".to_string())?;
    let input_type = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "each input item requires a string type".to_string())?;
    let required_string = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(|_| ())
            .ok_or_else(|| format!("{input_type} input requires non-empty {key}"))
    };
    match input_type {
        "text" => {
            if object.get("text").and_then(Value::as_str).is_none() {
                return Err("text input requires string text".to_string());
            }
            if object
                .get("text_elements")
                .is_some_and(|elements| !elements.is_array())
            {
                return Err("text_elements must be an array when present".to_string());
            }
        }
        "image" => required_string("url")?,
        "localImage" => required_string("path")?,
        "skill" | "mention" => {
            required_string("name")?;
            required_string("path")?;
        }
        _ => return Err(format!("unsupported native UserInput type: {input_type}")),
    }
    if matches!(input_type, "image" | "localImage")
        && object
            .get("detail")
            .is_some_and(|detail| !detail.is_null() && !detail.is_string())
    {
        return Err("image detail must be a string or null".to_string());
    }
    Ok(())
}

impl UserInputRepository {
    pub fn open(root: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        secure_directory(&root)?;
        Ok(Self {
            root,
            process_lock: Mutex::new(()),
        })
    }

    pub fn register_identity(
        &self,
        cutex_session_id: &str,
        params: &UserInputSubmitParams,
    ) -> anyhow::Result<ClientIdentityDecision> {
        let identity_sha256 = input_identity_sha256(params)?;
        self.mutate(|store| {
            let state = store
                .sessions
                .entry(cutex_session_id.to_string())
                .or_default();
            match state.identities.get(&params.client_user_message_id) {
                Some(identity) if identity.canonical_sha256 == identity_sha256 => {
                    Ok(ClientIdentityDecision::Existing)
                }
                Some(_) => Ok(ClientIdentityDecision::Conflict),
                None => {
                    state.identities.insert(
                        params.client_user_message_id.clone(),
                        ClientInputIdentity {
                            canonical_sha256: identity_sha256,
                            first_seen_at: Utc::now().to_rfc3339(),
                        },
                    );
                    Ok(ClientIdentityDecision::New)
                }
            }
        })
    }

    pub fn enqueue(
        &self,
        cutex_session_id: &str,
        thread_id: &str,
        management_request_id: &str,
        params: &UserInputSubmitParams,
    ) -> anyhow::Result<QueueEnqueueDecision> {
        let identity_sha256 = input_identity_sha256(params)?;
        self.mutate(|store| {
            let state = store
                .sessions
                .entry(cutex_session_id.to_string())
                .or_default();
            if let Some(identity) = state.identities.get(&params.client_user_message_id) {
                if identity.canonical_sha256 != identity_sha256 {
                    return Ok(QueueEnqueueDecision::ClientMessageConflict);
                }
            } else {
                state.identities.insert(
                    params.client_user_message_id.clone(),
                    ClientInputIdentity {
                        canonical_sha256: identity_sha256,
                        first_seen_at: Utc::now().to_rfc3339(),
                    },
                );
            }
            if let Some(existing) = state
                .items
                .iter()
                .find(|stored| stored.item.client_user_message_id == params.client_user_message_id)
            {
                return Ok(QueueEnqueueDecision::Deduplicated(existing.item.clone()));
            }
            state.queue_revision = next_revision(state.queue_revision)?;
            let now = Utc::now().to_rfc3339();
            let item = UserInputQueueItem {
                queue_id: Uuid::new_v4().to_string(),
                client_user_message_id: params.client_user_message_id.clone(),
                origin: params.origin.clone(),
                input: params.input.clone(),
                position: state.items.len() as u64,
                revision: 1,
                state: "queued".to_string(),
                created_at: now.clone(),
                updated_at: now,
            };
            state.items.push(StoredQueueItem {
                management_request_id: management_request_id.to_string(),
                thread_id: thread_id.to_string(),
                item: item.clone(),
            });
            Ok(QueueEnqueueDecision::Queued(item))
        })
    }

    pub fn list(&self, cutex_session_id: &str) -> anyhow::Result<(u64, Vec<UserInputQueueItem>)> {
        self.read(|store| {
            let Some(state) = store.sessions.get(cutex_session_id) else {
                return Ok((0, Vec::new()));
            };
            Ok((state.queue_revision, positioned_items(state)))
        })
    }

    pub fn front(
        &self,
        cutex_session_id: &str,
    ) -> anyhow::Result<Option<(String, String, UserInputQueueItem)>> {
        self.read(|store| {
            Ok(store
                .sessions
                .get(cutex_session_id)
                .and_then(|state| state.items.first())
                .map(|stored| {
                    (
                        stored.management_request_id.clone(),
                        stored.thread_id.clone(),
                        stored.item.clone(),
                    )
                }))
        })
    }

    pub fn update(
        &self,
        cutex_session_id: &str,
        queue_id: &str,
        expected_revision: u64,
        input: Vec<Value>,
    ) -> anyhow::Result<Option<UserInputQueueItem>> {
        for item in &input {
            validate_user_input(item).map_err(anyhow::Error::msg)?;
        }
        self.mutate(|store| {
            let Some(state) = store.sessions.get_mut(cutex_session_id) else {
                return Ok(None);
            };
            let Some(stored) = state
                .items
                .iter_mut()
                .find(|stored| stored.item.queue_id == queue_id)
            else {
                return Ok(None);
            };
            if stored.item.revision != expected_revision {
                anyhow::bail!(
                    "queue revision conflict: expected {expected_revision}, current {}",
                    stored.item.revision
                );
            }
            stored.item.revision = next_revision(stored.item.revision)?;
            stored.item.input = input;
            stored.item.updated_at = Utc::now().to_rfc3339();
            let client_user_message_id = stored.item.client_user_message_id.clone();
            let identity_sha256 = canonical_request_sha256(&serde_json::json!({
                "origin": stored.item.origin,
                "input": stored.item.input,
            }))?;
            let item = stored.item.clone();
            if let Some(identity) = state.identities.get_mut(&client_user_message_id) {
                identity.canonical_sha256 = identity_sha256;
            }
            state.queue_revision = next_revision(state.queue_revision)?;
            Ok(Some(item))
        })
    }

    pub fn remove(
        &self,
        cutex_session_id: &str,
        queue_id: &str,
        expected_revision: u64,
    ) -> anyhow::Result<Option<UserInputQueueItem>> {
        self.mutate(|store| {
            let Some(state) = store.sessions.get_mut(cutex_session_id) else {
                return Ok(None);
            };
            let Some(index) = state
                .items
                .iter()
                .position(|stored| stored.item.queue_id == queue_id)
            else {
                return Ok(None);
            };
            if state.items[index].item.revision != expected_revision {
                anyhow::bail!(
                    "queue revision conflict: expected {expected_revision}, current {}",
                    state.items[index].item.revision
                );
            }
            let mut removed = state.items.remove(index).item;
            removed.revision = next_revision(removed.revision)?;
            removed.updated_at = Utc::now().to_rfc3339();
            state.queue_revision = next_revision(state.queue_revision)?;
            Ok(Some(removed))
        })
    }

    pub fn snapshot(&self, cutex_session_id: &str) -> anyhow::Result<Vec<Value>> {
        let (_, items) = self.list(cutex_session_id)?;
        items
            .into_iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn clear_session(&self, cutex_session_id: &str) -> anyhow::Result<()> {
        self.mutate(|store| {
            store.sessions.remove(cutex_session_id);
            Ok(())
        })
    }

    fn mutate<T>(
        &self,
        action: impl FnOnce(&mut UserInputStateStore) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.with_lock(|path| {
            let mut store = load_store(path)?;
            let result = action(&mut store)?;
            write_private_pretty_json_atomic(path, &store, "management v2 user-input state")?;
            Ok(result)
        })
    }

    fn read<T>(
        &self,
        action: impl FnOnce(&UserInputStateStore) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.with_lock(|path| action(&load_store(path)?))
    }

    fn with_lock<T>(&self, action: impl FnOnce(&Path) -> anyhow::Result<T>) -> anyhow::Result<T> {
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("management v2 user-input lock was poisoned"))?;
        let lock_file = open_private_lock(&self.root.join(USER_INPUT_LOCK_FILE))?;
        lock_file.lock()?;
        let result = action(&self.root.join(USER_INPUT_STATE_FILE));
        let unlock = lock_file.unlock();
        if result.is_ok() {
            unlock?;
        }
        result
    }
}

fn positioned_items(state: &SessionInputState) -> Vec<UserInputQueueItem> {
    state
        .items
        .iter()
        .enumerate()
        .map(|(position, stored)| {
            let mut item = stored.item.clone();
            item.position = position as u64;
            item
        })
        .collect()
}

fn input_identity_sha256(params: &UserInputSubmitParams) -> anyhow::Result<String> {
    canonical_request_sha256(&serde_json::json!({
        "origin": params.origin,
        "input": params.input,
    }))
}

fn next_revision(current: u64) -> anyhow::Result<u64> {
    if current >= MAX_SAFE_SEQUENCE {
        anyhow::bail!("user-input revision exhausted the JSON-safe integer range");
    }
    Ok(current + 1)
}

fn load_store(path: &Path) -> anyhow::Result<UserInputStateStore> {
    match fs::read(path) {
        Ok(bytes) => {
            let store: UserInputStateStore = serde_json::from_slice(&bytes).with_context(|| {
                format!(
                    "failed to parse management v2 user-input state: {}",
                    path.display()
                )
            })?;
            if store.version != 1 {
                anyhow::bail!("unsupported management v2 user-input state version");
            }
            Ok(store)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(UserInputStateStore::default()),
        Err(error) => Err(error).with_context(|| {
            format!(
                "failed to read management v2 user-input state: {}",
                path.display()
            )
        }),
    }
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
    use serde_json::json;

    use super::*;

    fn params(text: &str) -> UserInputSubmitParams {
        parse_user_input_submit_params(&json!({
            "clientUserMessageId": "client-message-1",
            "origin": { "kind": "android", "clientId": "device-1" },
            "strategy": "queue",
            "input": [{ "type": "text", "text": text, "text_elements": [] }]
        }))
        .expect("valid params")
    }

    #[test]
    fn parser_preserves_native_inputs_and_rejects_unknown_types() {
        let parsed = params("hello");
        assert_eq!(parsed.origin.kind, UserInputOriginKind::Android);
        assert_eq!(parsed.input[0]["text_elements"], json!([]));
        assert!(parse_user_input_submit_params(&json!({
            "clientUserMessageId": "message-1",
            "origin": { "kind": "android", "clientId": "device-1" },
            "strategy": "auto",
            "input": [{ "type": "future" }]
        }))
        .is_err());
    }

    #[test]
    fn queue_revisions_and_client_identity_are_durable() {
        let root = std::env::temp_dir().join(format!("cutex-v2-user-input-{}", Uuid::new_v4()));
        let repository = UserInputRepository::open(&root).expect("open repository");
        let first = repository
            .enqueue("session-1", "thread-1", "request-1", &params("hello"))
            .expect("enqueue");
        let QueueEnqueueDecision::Queued(first) = first else {
            panic!("expected queued item");
        };
        assert_eq!(first.revision, 1);
        assert_eq!(repository.list("session-1").expect("list").0, 1);
        assert!(matches!(
            repository
                .enqueue("session-1", "thread-1", "request-2", &params("hello"))
                .expect("dedupe"),
            QueueEnqueueDecision::Deduplicated(_)
        ));
        assert_eq!(
            repository
                .enqueue("session-1", "thread-1", "request-3", &params("changed"))
                .expect("conflict"),
            QueueEnqueueDecision::ClientMessageConflict
        );
        let updated = repository
            .update(
                "session-1",
                &first.queue_id,
                1,
                vec![json!({ "type": "text", "text": "updated", "text_elements": [] })],
            )
            .expect("update")
            .expect("queue item");
        assert_eq!(updated.revision, 2);
        let removed = repository
            .remove("session-1", &first.queue_id, 2)
            .expect("remove")
            .expect("removed item");
        assert_eq!(removed.revision, 3);
        assert_eq!(repository.list("session-1").expect("empty list").0, 3);
        fs::remove_dir_all(root).expect("remove repository");
    }
}
