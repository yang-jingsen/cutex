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
use serde_json::Map;
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;
use uuid::Uuid;

use crate::config::atomic::write_private_pretty_json_atomic;
use crate::config::paths::runtime_dir;

const IDEMPOTENCY_FILE: &str = "request-idempotency.json";
const IDEMPOTENCY_LOCK_FILE: &str = "request-idempotency.lock";
pub const NATIVE_REQUEST_POLICY_VERSION: u8 = 2;

static IDEMPOTENCY_REPOSITORY: OnceLock<RequestIdempotencyRepository> = OnceLock::new();
static CLIENT_REQUEST_VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();

const CLIENT_REQUEST_SCHEMA: &str = include_str!("schema/client-request-0.144.1-experimental.json");
pub const CLIENT_REQUEST_SCHEMA_SHA256: &str =
    "1ba93c7adaa753e7c2e489f0f23e67844e004eece0bc2b0fb3b056d800ae9eb5";

const SESSION_THREAD_METHODS: &[&str] = &[
    "thread/name/set",
    "thread/goal/set",
    "thread/goal/get",
    "thread/goal/clear",
    "thread/metadata/update",
    "thread/settings/update",
    "thread/memoryMode/set",
    "thread/compact/start",
    "thread/backgroundTerminals/list",
    "thread/read",
    "thread/turns/list",
    "thread/items/list",
    "turn/start",
    "turn/steer",
    "turn/interrupt",
    "review/start",
];

const OWNER_GLOBAL_READ_METHODS: &[&str] = &[
    "model/list",
    "permissionProfile/list",
    "collaborationMode/list",
];

#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedNativeRequest {
    pub request_id: String,
    pub native_message: Value,
    pub method: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeRequestValidationError {
    Invalid(String),
    MethodDenied(String),
    MissingThreadId,
    ForeignThread { requested: String, bound: String },
}

pub fn validate_native_request(
    body: &[u8],
    bound_thread_id: &str,
) -> Result<ValidatedNativeRequest, NativeRequestValidationError> {
    let value: Value = serde_json::from_slice(body).map_err(|error| {
        NativeRequestValidationError::Invalid(format!("invalid JSON request body: {error}"))
    })?;
    let object = value.as_object().ok_or_else(|| {
        NativeRequestValidationError::Invalid("request body must be a JSON object".to_string())
    })?;
    require_exact_keys(object, &["requestId", "native"])?;
    let request_id = object
        .get("requestId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.chars().count() <= 256)
        .ok_or_else(|| {
            NativeRequestValidationError::Invalid(
                "requestId must be a non-empty string of at most 256 characters".to_string(),
            )
        })?
        .to_string();
    let native = object
        .get("native")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            NativeRequestValidationError::Invalid("native must be an object".to_string())
        })?;
    require_exact_keys(native, &["message"])?;
    let native_message = native.get("message").cloned().ok_or_else(|| {
        NativeRequestValidationError::Invalid("native.message is required".to_string())
    })?;
    let message = native_message.as_object().ok_or_else(|| {
        NativeRequestValidationError::Invalid("native.message must be an object".to_string())
    })?;
    let id = message
        .get("id")
        .filter(|id| !id.is_null())
        .ok_or_else(|| {
            NativeRequestValidationError::Invalid("native.message.id is required".to_string())
        })?;
    if !id.is_string() && id.as_i64().is_none() {
        return Err(NativeRequestValidationError::Invalid(
            "native.message.id must be a string or signed 64-bit integer".to_string(),
        ));
    }
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .filter(|method| !method.is_empty() && !method.starts_with("cutex/"))
        .ok_or_else(|| {
            NativeRequestValidationError::Invalid(
                "native.message.method must be a non-empty native method".to_string(),
            )
        })?
        .to_string();

    if SESSION_THREAD_METHODS.contains(&method.as_str()) {
        let thread_id = message
            .get("params")
            .and_then(Value::as_object)
            .and_then(|params| params.get("threadId"))
            .and_then(Value::as_str)
            .filter(|thread_id| !thread_id.is_empty())
            .ok_or(NativeRequestValidationError::MissingThreadId)?;
        if thread_id != bound_thread_id {
            return Err(NativeRequestValidationError::ForeignThread {
                requested: thread_id.to_string(),
                bound: bound_thread_id.to_string(),
            });
        }
    } else if !OWNER_GLOBAL_READ_METHODS.contains(&method.as_str()) {
        return Err(NativeRequestValidationError::MethodDenied(method));
    }
    validate_pinned_client_request_schema(&native_message)?;

    Ok(ValidatedNativeRequest {
        request_id,
        native_message,
        method,
    })
}

fn validate_pinned_client_request_schema(
    native_message: &Value,
) -> Result<(), NativeRequestValidationError> {
    let validator = CLIENT_REQUEST_VALIDATOR.get_or_init(|| {
        let schema: Value = serde_json::from_str(CLIENT_REQUEST_SCHEMA)
            .expect("embedded app-server 0.144.1 ClientRequest schema must be valid JSON");
        jsonschema::options()
            .with_draft(jsonschema::Draft::Draft7)
            .build(&schema)
            .expect("embedded app-server 0.144.1 ClientRequest schema must compile")
    });
    if let Err(error) = validator.validate(native_message) {
        return Err(NativeRequestValidationError::Invalid(format!(
            "native.message does not match app-server 0.144.1 experimental ClientRequest: {error}"
        )));
    }
    Ok(())
}

fn require_exact_keys(
    object: &Map<String, Value>,
    expected: &[&str],
) -> Result<(), NativeRequestValidationError> {
    if object.len() != expected.len() || expected.iter().any(|key| !object.contains_key(*key)) {
        return Err(NativeRequestValidationError::Invalid(format!(
            "object must contain exactly: {}",
            expected.join(", ")
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub enum BeginRequest {
    Forward(RequestClaim),
    InProgress,
    Completed(StoredHttpResponse),
    Conflict,
    OutcomeUnknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StoredHttpResponse {
    pub status: u16,
    pub reason: String,
    pub body: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestClaim {
    cutex_session_id: String,
    request_id: String,
    body_sha256: String,
    process_instance_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdempotencyStore {
    version: u8,
    entries: BTreeMap<String, IdempotencyEntry>,
}

impl Default for IdempotencyStore {
    fn default() -> Self {
        Self {
            version: 1,
            entries: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdempotencyEntry {
    cutex_session_id: String,
    request_id: String,
    body_sha256: String,
    state: IdempotencyState,
    process_instance_id: String,
    created_at: String,
    updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    response: Option<StoredHttpResponse>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum IdempotencyState {
    Forwarded,
    Completed,
    OutcomeUnknown,
}

pub struct RequestIdempotencyRepository {
    root: PathBuf,
    process_instance_id: String,
    process_lock: Mutex<()>,
}

pub fn request_idempotency_repository() -> anyhow::Result<&'static RequestIdempotencyRepository> {
    if let Some(repository) = IDEMPOTENCY_REPOSITORY.get() {
        return Ok(repository);
    }
    let repository = RequestIdempotencyRepository::open(
        runtime_dir()?.join("management-v2"),
        Uuid::new_v4().to_string(),
    )?;
    let _ = IDEMPOTENCY_REPOSITORY.set(repository);
    IDEMPOTENCY_REPOSITORY
        .get()
        .context("management v2 idempotency repository initialization raced")
}

impl RequestIdempotencyRepository {
    pub fn open(root: impl Into<PathBuf>, process_instance_id: String) -> anyhow::Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        secure_directory(&root)?;
        Ok(Self {
            root,
            process_instance_id,
            process_lock: Mutex::new(()),
        })
    }

    pub fn begin(
        &self,
        cutex_session_id: &str,
        request_id: &str,
        canonical_body: &Value,
    ) -> anyhow::Result<BeginRequest> {
        let body_sha256 = canonical_request_sha256(canonical_body)?;
        self.mutate(|store| {
            let key = idempotency_key(cutex_session_id, request_id);
            if let Some(entry) = store.entries.get_mut(&key) {
                if entry.body_sha256 != body_sha256 {
                    return Ok(BeginRequest::Conflict);
                }
                return match entry.state {
                    IdempotencyState::Completed => Ok(BeginRequest::Completed(
                        entry
                            .response
                            .clone()
                            .context("completed idempotency entry omitted response")?,
                    )),
                    IdempotencyState::OutcomeUnknown => Ok(BeginRequest::OutcomeUnknown),
                    IdempotencyState::Forwarded
                        if entry.process_instance_id == self.process_instance_id =>
                    {
                        Ok(BeginRequest::InProgress)
                    }
                    IdempotencyState::Forwarded => {
                        entry.state = IdempotencyState::OutcomeUnknown;
                        entry.response = None;
                        entry.updated_at = Utc::now().to_rfc3339();
                        Ok(BeginRequest::OutcomeUnknown)
                    }
                };
            }
            let now = Utc::now().to_rfc3339();
            let claim = RequestClaim {
                cutex_session_id: cutex_session_id.to_string(),
                request_id: request_id.to_string(),
                body_sha256: body_sha256.clone(),
                process_instance_id: self.process_instance_id.clone(),
            };
            store.entries.insert(
                key,
                IdempotencyEntry {
                    cutex_session_id: cutex_session_id.to_string(),
                    request_id: request_id.to_string(),
                    body_sha256,
                    state: IdempotencyState::Forwarded,
                    process_instance_id: self.process_instance_id.clone(),
                    created_at: now.clone(),
                    updated_at: now,
                    response: None,
                },
            );
            Ok(BeginRequest::Forward(claim))
        })
    }

    pub fn complete(
        &self,
        claim: &RequestClaim,
        status: u16,
        reason: &str,
        body: Value,
    ) -> anyhow::Result<()> {
        self.update_claim(claim, |entry| {
            entry.state = IdempotencyState::Completed;
            entry.response = Some(StoredHttpResponse {
                status,
                reason: reason.to_string(),
                body,
            });
            Ok(())
        })
    }

    pub fn mark_outcome_unknown(&self, claim: &RequestClaim) -> anyhow::Result<()> {
        self.update_claim(claim, |entry| {
            entry.state = IdempotencyState::OutcomeUnknown;
            entry.response = None;
            Ok(())
        })
    }

    pub fn release_before_forward(&self, claim: &RequestClaim) -> anyhow::Result<()> {
        self.mutate(|store| {
            let key = idempotency_key(&claim.cutex_session_id, &claim.request_id);
            let entry = store
                .entries
                .get(&key)
                .context("idempotency claim no longer exists")?;
            validate_claim(entry, claim)?;
            store.entries.remove(&key);
            Ok(())
        })
    }

    fn update_claim(
        &self,
        claim: &RequestClaim,
        update: impl FnOnce(&mut IdempotencyEntry) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        self.mutate(|store| {
            let key = idempotency_key(&claim.cutex_session_id, &claim.request_id);
            let entry = store
                .entries
                .get_mut(&key)
                .context("idempotency claim no longer exists")?;
            validate_claim(entry, claim)?;
            update(entry)?;
            entry.updated_at = Utc::now().to_rfc3339();
            Ok(())
        })
    }

    fn mutate<T>(
        &self,
        action: impl FnOnce(&mut IdempotencyStore) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let _process_guard = self
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("management v2 idempotency lock was poisoned"))?;
        let lock_path = self.root.join(IDEMPOTENCY_LOCK_FILE);
        let lock_file = open_private_lock(&lock_path)?;
        lock_file.lock()?;
        let result = (|| {
            let path = self.root.join(IDEMPOTENCY_FILE);
            let mut store = load_store(&path)?;
            let result = action(&mut store)?;
            write_private_pretty_json_atomic(&path, &store, "management v2 idempotency state")?;
            Ok(result)
        })();
        let unlock = lock_file.unlock();
        if result.is_ok() {
            unlock?;
        }
        result
    }
}

fn validate_claim(entry: &IdempotencyEntry, claim: &RequestClaim) -> anyhow::Result<()> {
    if entry.state != IdempotencyState::Forwarded
        || entry.body_sha256 != claim.body_sha256
        || entry.process_instance_id != claim.process_instance_id
    {
        anyhow::bail!("idempotency claim ownership changed");
    }
    Ok(())
}

fn idempotency_key(cutex_session_id: &str, request_id: &str) -> String {
    serde_json::to_string(&(cutex_session_id, request_id))
        .expect("serializing a pair of strings cannot fail")
}

pub fn canonical_request_sha256(value: &Value) -> anyhow::Result<String> {
    let canonical = canonicalize(value);
    let bytes = serde_json::to_vec(&canonical)?;
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| (key.clone(), canonicalize(value)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(canonicalize).collect()),
        _ => value.clone(),
    }
}

fn load_store(path: &Path) -> anyhow::Result<IdempotencyStore> {
    match fs::read(path) {
        Ok(bytes) => {
            let store: IdempotencyStore = serde_json::from_slice(&bytes).with_context(|| {
                format!(
                    "failed to parse management v2 idempotency state: {}",
                    path.display()
                )
            })?;
            if store.version != 1 {
                anyhow::bail!("unsupported management v2 idempotency state version");
            }
            Ok(store)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(IdempotencyStore::default()),
        Err(error) => Err(error).with_context(|| {
            format!(
                "failed to read management v2 idempotency state: {}",
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

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("cutex-v2-{label}-{}", Uuid::new_v4()))
    }

    #[test]
    fn native_policy_preserves_message_and_enforces_thread_scope() {
        let body = serde_json::to_vec(&json!({
            "requestId": "request-1",
            "native": {
                "message": {
                    "id": i64::MIN,
                    "method": "thread/read",
                    "params": { "threadId": "thread-1", "includeTurns": true },
                    "futureField": null
                }
            }
        }))
        .expect("serialize request");
        let request = validate_native_request(&body, "thread-1").expect("valid request");
        assert_eq!(request.native_message["id"], i64::MIN);
        assert!(request.native_message.get("futureField").is_some());
        assert!(matches!(
            validate_native_request(&body, "thread-2"),
            Err(NativeRequestValidationError::ForeignThread { .. })
        ));
    }

    #[test]
    fn native_policy_denies_unknown_and_unsigned_overflow_ids() {
        let unknown = serde_json::to_vec(&json!({
            "requestId": "request-1",
            "native": { "message": { "id": "native-1", "method": "future/write" } }
        }))
        .expect("serialize request");
        assert!(matches!(
            validate_native_request(&unknown, "thread-1"),
            Err(NativeRequestValidationError::MethodDenied(method)) if method == "future/write"
        ));

        let overflow = format!(
            r#"{{"requestId":"request-1","native":{{"message":{{"id":{},"method":"model/list"}}}}}}"#,
            u64::MAX
        );
        assert!(matches!(
            validate_native_request(overflow.as_bytes(), "thread-1"),
            Err(NativeRequestValidationError::Invalid(_))
        ));
    }

    #[test]
    fn native_policy_uses_exact_pinned_method_parameter_schema() {
        let malformed = serde_json::to_vec(&json!({
            "requestId": "request-1",
            "native": {
                "message": {
                    "id": "native-1",
                    "method": "thread/read",
                    "params": { "threadId": "thread-1", "includeTurns": "yes" }
                }
            }
        }))
        .expect("serialize malformed request");
        assert!(matches!(
            validate_native_request(&malformed, "thread-1"),
            Err(NativeRequestValidationError::Invalid(message))
                if message.contains("ClientRequest")
        ));

        let digest = format!("{:x}", Sha256::digest(CLIENT_REQUEST_SCHEMA.as_bytes()));
        assert_eq!(digest, CLIENT_REQUEST_SCHEMA_SHA256);
    }

    #[test]
    fn idempotency_is_durable_and_restart_uncertainty_is_not_replayed() {
        let root = temp_root("idempotency");
        let request = json!({ "requestId": "request-1", "native": { "message": {} } });
        let first = RequestIdempotencyRepository::open(&root, "process-1".to_string())
            .expect("open first repository");
        let BeginRequest::Forward(claim) = first
            .begin("session-1", "request-1", &request)
            .expect("begin request")
        else {
            panic!("expected forwarding claim");
        };
        assert_eq!(
            first
                .begin("session-1", "request-1", &request)
                .expect("repeat in process"),
            BeginRequest::InProgress
        );
        first
            .complete(
                &claim,
                200,
                "OK",
                json!({ "contractVersion": 2, "ok": true }),
            )
            .expect("complete request");
        assert!(matches!(
            first.begin("session-1", "request-1", &request).expect("replay"),
            BeginRequest::Completed(response) if response.body["ok"] == true
        ));

        let BeginRequest::Forward(_uncertain_claim) = first
            .begin("session-1", "request-2", &json!({ "value": 1 }))
            .expect("begin uncertain request")
        else {
            panic!("expected second forwarding claim");
        };
        drop(first);
        let restarted = RequestIdempotencyRepository::open(&root, "process-2".to_string())
            .expect("open restarted repository");
        assert_eq!(
            restarted
                .begin("session-1", "request-2", &json!({ "value": 1 }))
                .expect("detect uncertainty"),
            BeginRequest::OutcomeUnknown
        );
        assert_eq!(
            restarted
                .begin("session-1", "request-1", &json!({ "different": true }))
                .expect("detect conflict"),
            BeginRequest::Conflict
        );
        fs::remove_dir_all(root).expect("remove temp root");
    }
}
