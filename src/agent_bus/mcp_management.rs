//! Fixed K ef53716 native Management semantic/receipt adapter; provider owns effects.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
const CONTRACT: &str = "cutex/agent-management/v1";
const RECEIPT_SCHEMA: &str = "cutex/agent-management-receipt/v1";
const FAILURE_SCHEMA: &str = "cutex/agent-management-failure/v1";

#[derive(Debug, Deserialize, Serialize)]
struct ToolInput {
    action_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project_id: Option<String>,
    #[serde(flatten)]
    operation: Operation,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ManagedAgentSpec {
    name: String,
    cwd: String,
    profile: String,
    runtime_backend: String,
    model: String,
    reasoning: String,
    permissions: String,
    approval_policy: String,
    sandbox_mode: String,
    groups: Vec<String>,
    #[serde(default)]
    expose_to_im: bool,
    #[serde(default)]
    pin: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Create {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bootstrap_intent: Option<String>,
        spec: ManagedAgentSpec,
        start_mode: String,
        #[serde(default)]
        frozen_message: Option<String>,
    },
    QueryManaged,
    Online {
        cutex_session_id: String,
    },
    Offline {
        cutex_session_id: String,
    },
    Restart {
        cutex_session_id: String,
    },
    Close {
        cutex_session_id: String,
    },
    Replace {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bootstrap_intent: Option<String>,
        predecessor_cutex_session_id: String,
        policy: String,
        successor: ManagedAgentSpec,
        start_mode: String,
        #[serde(default)]
        frozen_message: Option<String>,
    },
    DirectorRotate {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bootstrap_intent: Option<String>,
        expected_predecessor_cutex_session: String,
        expected_authority_epoch: u64,
        mode: String,
        successor: ManagedAgentSpec,
        #[serde(default)]
        frozen_message: Option<String>,
    },
}

impl Operation {
    fn name(&self) -> &'static str {
        match self {
            Self::Create { .. } => "create",
            Self::QueryManaged => "query_managed",
            Self::Online { .. } => "online",
            Self::Offline { .. } => "offline",
            Self::Restart { .. } => "restart",
            Self::Close { .. } => "close",
            Self::Replace { .. } => "replace",
            Self::DirectorRotate { .. } => "director_rotate",
        }
    }
}

#[derive(Serialize)]
struct ProviderRequest<'a> {
    schema: &'static str,
    action_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    project_id: Option<&'a str>,
    #[serde(flatten)]
    operation: &'a Operation,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderResponse {
    schema: String,
    action_id: String,
    outcome: ProviderOutcome,
}

#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum ProviderOutcome {
    Complete { receipt: Value },
    NoWrite { code: String, detail: String },
    OwnerActionRequired { failure: Value },
}

fn parse_input(arguments: &str) -> Result<ToolInput, ()> {
    let value: Value = serde_json::from_str(arguments).map_err(|_| ())?;
    let object = value.as_object().ok_or(())?;
    let operation = object.get("operation").and_then(Value::as_str).ok_or(())?;
    let operation_fields: &[&str] = match operation {
        "create" => &["spec", "start_mode", "frozen_message", "bootstrap_intent"],
        "query_managed" => &[],
        "online" | "offline" | "restart" | "close" => &["cutex_session_id"],
        "replace" => &[
            "bootstrap_intent",
            "predecessor_cutex_session_id",
            "policy",
            "successor",
            "start_mode",
            "frozen_message",
        ],
        "director_rotate" => &[
            "bootstrap_intent",
            "expected_predecessor_cutex_session",
            "expected_authority_epoch",
            "mode",
            "successor",
            "frozen_message",
        ],
        _ => return Err(()),
    };
    if object.keys().any(|field| {
        !matches!(field.as_str(), "operation" | "action_id" | "project_id")
            && !operation_fields.contains(&field.as_str())
    }) {
        return Err(());
    }
    serde_json::from_value(value).map_err(|_| ())
}

fn request_bytes(input: &ToolInput) -> Result<(Vec<u8>, String), serde_json::Error> {
    let request = ProviderRequest {
        schema: CONTRACT,
        action_id: &input.action_id,
        project_id: input.project_id.as_deref(),
        operation: &input.operation,
    };
    let bytes = serde_json::to_vec(&request)?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    Ok((bytes, digest))
}

fn sanitize_response(input: &ToolInput, digest: &str, value: Value) -> Value {
    let response: ProviderResponse = match serde_json::from_value::<ProviderResponse>(value.clone())
    {
        Ok(response) if response.schema == CONTRACT && response.action_id == input.action_id => {
            response
        }
        _ => return no_write(&input.action_id, "invalid_provider_response"),
    };
    let valid = match response.outcome {
        ProviderOutcome::Complete { receipt }
            if typed_payload_matches(&receipt, RECEIPT_SCHEMA, input, Some(digest)) =>
        {
            true
        }
        ProviderOutcome::NoWrite { code, detail }
            if bounded_text(&code, 128) && bounded_text(&detail, 4096) =>
        {
            true
        }
        ProviderOutcome::OwnerActionRequired { failure }
            if typed_payload_matches(&failure, FAILURE_SCHEMA, input, None) =>
        {
            true
        }
        _ => false,
    };
    if valid {
        value
    } else {
        no_write(&input.action_id, "invalid_provider_response")
    }
}

fn typed_payload_matches(
    payload: &Value,
    schema: &str,
    input: &ToolInput,
    digest: Option<&str>,
) -> bool {
    let Some(resolved_project) = payload.get("project_id").and_then(Value::as_str) else {
        return false;
    };
    let schema_shape_matches = match schema {
        RECEIPT_SCHEMA => receipt_shape_matches(payload, input),
        FAILURE_SCHEMA => failure_shape_matches(payload),
        _ => false,
    };
    schema_shape_matches
        && payload.get("schema").and_then(Value::as_str) == Some(schema)
        && payload.get("action_id").and_then(Value::as_str) == Some(input.action_id.as_str())
        && valid_project(resolved_project)
        && input
            .project_id
            .as_deref()
            .is_none_or(|selector| selector == resolved_project)
        && project_ids_are_consistent(payload, resolved_project)
        && payload.get("operation").and_then(Value::as_str) == Some(input.operation.name())
        && digest.is_none_or(|digest| {
            payload.get("request_sha256").and_then(Value::as_str) == Some(digest)
        })
}

fn receipt_shape_matches(payload: &Value, input: &ToolInput) -> bool {
    const FIELDS: &[&str] = &[
        "schema",
        "action_id",
        "request_sha256",
        "operation",
        "project_id",
        "completed_at",
        "result",
    ];
    let Some(object) = payload.as_object() else {
        return false;
    };
    if object.len() != FIELDS.len() || !FIELDS.iter().all(|field| object.contains_key(*field)) {
        return false;
    }
    let expected_result = match &input.operation {
        Operation::Create { .. } => "created",
        Operation::QueryManaged => "query_managed",
        Operation::Online { .. }
        | Operation::Offline { .. }
        | Operation::Restart { .. }
        | Operation::Close { .. } => "lifecycle",
        Operation::Replace { .. } => "replaced",
        Operation::DirectorRotate { .. } => "director_rotated",
    };
    object
        .get("completed_at")
        .and_then(Value::as_str)
        .is_some_and(|value| bounded_text(value, 128))
        && object
            .get("result")
            .and_then(Value::as_object)
            .and_then(|result| result.get("kind"))
            .and_then(Value::as_str)
            == Some(expected_result)
}

fn failure_shape_matches(payload: &Value) -> bool {
    const REQUIRED_FIELDS: &[&str] = &[
        "schema",
        "event_id",
        "action_id",
        "project_id",
        "operation",
        "code",
        "detail",
        "routing_status",
        "created_at",
    ];
    const OPTIONAL_FIELDS: &[&str] = &["route_to_director_session", "target_cutex_session_id"];
    let Some(object) = payload.as_object() else {
        return false;
    };
    if !(REQUIRED_FIELDS.len()..=REQUIRED_FIELDS.len() + OPTIONAL_FIELDS.len())
        .contains(&object.len())
        || !REQUIRED_FIELDS
            .iter()
            .all(|field| object.contains_key(*field))
        || object.keys().any(|field| {
            !REQUIRED_FIELDS.contains(&field.as_str()) && !OPTIONAL_FIELDS.contains(&field.as_str())
        })
    {
        return false;
    }
    object
        .get("event_id")
        .and_then(Value::as_str)
        .is_some_and(valid_identity)
        && object
            .get("code")
            .and_then(Value::as_str)
            .is_some_and(|value| bounded_text(value, 128))
        && object
            .get("detail")
            .and_then(Value::as_str)
            .is_some_and(|value| value.len() <= 4096)
        && matches!(
            object.get("routing_status").and_then(Value::as_str),
            Some("routable" | "unrouted")
        )
        && object
            .get("created_at")
            .and_then(Value::as_str)
            .is_some_and(|value| bounded_text(value, 128))
        && OPTIONAL_FIELDS.iter().all(|field| {
            object.get(*field).is_none_or(|value| {
                value.is_null()
                    || value
                        .as_str()
                        .is_some_and(|value| valid_identity(value) && value.len() <= 256)
            })
        })
}

fn project_ids_are_consistent(value: &Value, expected: &str) -> bool {
    match value {
        // Imported/moved roster records retain historical creation provenance:
        // project_id may be null or an old Project. It is NOT current membership.
        // Recognize the exact provider record type rather than weakening all
        // nested project checks; current authority/grants still bind the envelope.
        Value::Object(object)
            if object.contains_key("cutex_session_id")
                && serde_json::from_value::<crate::agent_management::ManagedAgentRecord>(
                    value.clone(),
                )
                .is_ok() =>
        {
            true
        }
        Value::Object(object) => object.iter().all(|(key, value)| {
            if key == "project_id" {
                value.as_str() == Some(expected)
            } else {
                project_ids_are_consistent(value, expected)
            }
        }),
        Value::Array(values) => values
            .iter()
            .all(|value| project_ids_are_consistent(value, expected)),
        _ => true,
    }
}

fn valid_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'@')
        })
}

fn valid_project(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':'))
}

fn bounded_text(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max
}

fn no_write(action_id: &str, code: &str) -> Value {
    serde_json::json!({
        "schema": CONTRACT,
        "action_id": action_id,
        "outcome": {
            "status": "no_write",
            "code": code,
            "detail": "No successful action receipt is available. This is not proof of rollback; an unconfirmed provider outcome requires exact-action retry."
        }
    })
}

fn uncertain(action_id: &str) -> Value {
    let mut failure = no_write(action_id, "response_uncertain");
    failure["outcome"]["detail"] = json!(
        "Outcome unknown; retry the exact action_id and payload, never a replacement action."
    );
    failure
}

pub(super) fn invoke(args: Value, mut post: impl FnMut(&Value) -> anyhow::Result<Value>) -> Value {
    let required: &[&str] = match args["operation"].as_str() {
        Some("create") => &["spec", "start_mode"],
        Some("online" | "offline" | "restart" | "close") => &["cutex_session_id"],
        Some("replace") => &[
            "predecessor_cutex_session_id",
            "policy",
            "successor",
            "start_mode",
        ],
        Some("director_rotate") => &[
            "expected_predecessor_cutex_session",
            "expected_authority_epoch",
            "mode",
            "successor",
        ],
        _ => &[],
    };
    if let Some(field) = required
        .iter()
        .find(|field| args.get(**field).is_none_or(Value::is_null))
    {
        return no_write("invalid-body", &format!("missing_{field}"));
    }
    let input = match parse_input(&args.to_string()) {
        Ok(input) => input,
        Err(_) => return no_write("invalid-body", "invalid_arguments"),
    };
    if !valid_identity(&input.action_id)
        || input
            .project_id
            .as_deref()
            .is_some_and(|p| !valid_project(p))
    {
        return no_write("invalid-body", "invalid_arguments");
    }
    let (bytes, _) = match request_bytes(&input) {
        Ok(value) => value,
        Err(_) => return no_write(&input.action_id, "invalid_arguments"),
    };
    // Serialize the current provider type for its exact semantic digest, rather
    // than relying on JSON object key order or a model-supplied hash.
    let request: crate::agent_management::AgentManagementRequest =
        match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) => return no_write(&input.action_id, "invalid_arguments"),
        };
    let digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&request).expect("typed request"))
    );
    match post(&json!(request)) {
        Ok(value)
            if value["http_status"]
                .as_u64()
                .is_some_and(|s| s >= 500 || s == 408) =>
        {
            uncertain(&input.action_id)
        }
        Ok(value) => sanitize_response(&input, &digest, value),
        Err(_) => uncertain(&input.action_id),
    }
}

pub(super) fn tool() -> Value {
    let required = [
        "name",
        "cwd",
        "profile",
        "runtime_backend",
        "model",
        "reasoning",
        "permissions",
        "approval_policy",
        "sandbox_mode",
        "groups",
    ];
    let mut fields = serde_json::Map::new();
    for field in required {
        fields.insert(field.into(), json!({"type":"string"}));
    }
    fields.insert(
        "groups".into(),
        json!({"type":"array","items":{"type":"string"}}),
    );
    for field in ["expose_to_im", "pin"] {
        fields.insert(field.into(), json!({"type":"boolean"}));
    }
    let spec = json!({"type":"object","properties":fields,"required":required,"additionalProperties":false});
    let mut properties = json!({
        "operation":{"type":"string","enum":["query_managed","create","online","offline","restart","close","replace","director_rotate"]},
        "spec":spec,"successor":spec,
        "expected_authority_epoch":{"type":"integer","minimum":0},
        "start_mode":{"type":"string","enum":["bootstrap_only","custom_message"]},
        "policy":{"type":"string","enum":["close_before_create","close_after_ready","keep_old"]},
        "mode":{"type":"string","enum":["close_predecessor_then_create_with_message","retain_predecessor_with_message","retain_predecessor_bootstrap_only"]}
    });
    for field in [
        "action_id",
        "project_id",
        "cutex_session_id",
        "predecessor_cutex_session_id",
        "expected_predecessor_cutex_session",
        "frozen_message",
        "bootstrap_intent",
    ] {
        properties[field] = json!({"type":"string"});
    }
    properties["cutex_session_id"]["description"]=json!("Required for online/offline/restart/close; exact durable identity, not native thread/name/profile.");
    properties["bootstrap_intent"]["description"]=json!("Optional exact action_id reference to a previously Human-reviewed private bundle intent for create/replace/director_rotate. Does not mint an intent or grant authority.");
    json!({"name":"cutex_agent_management","description":"Typed provider-authorized Management action. create requires spec/start_mode; replace requires predecessor_cutex_session_id/policy/successor/start_mode; director_rotate requires expected_predecessor_cutex_session/expected_authority_epoch/mode/successor. Private lightweight bootstrap requires a previously Human-reviewed exact bootstrap_intent reference. Generic marked online/restart still refuses default fallback. close permanently retires, not reversible archive. Caller authority never comes from arguments. Reuse action_id only for exact replay.","inputSchema":{"type":"object","properties":properties,"required":["operation","action_id"],"additionalProperties":false}})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> Value {
        json!({"name":"Formal Agent","cwd":"/private","profile":"mutable",
            "runtime_backend":"host","model":"fixture","reasoning":"low",
            "permissions":"read-only","approval_policy":"on-request",
            "sandbox_mode":"read-only","groups":["private"]})
    }

    #[test]
    fn all_native_operations_preserve_typed_request_digest_and_receipt() {
        for (operation, fields, kind) in [
            ("query_managed", json!({}), "query_managed"),
            (
                "create",
                json!({"spec":spec(),"start_mode":"bootstrap_only"}),
                "created",
            ),
            (
                "online",
                json!({"cutex_session_id":"cutex.target"}),
                "lifecycle",
            ),
            (
                "offline",
                json!({"cutex_session_id":"cutex.target"}),
                "lifecycle",
            ),
            (
                "restart",
                json!({"cutex_session_id":"cutex.target"}),
                "lifecycle",
            ),
            (
                "close",
                json!({"cutex_session_id":"cutex.target"}),
                "lifecycle",
            ),
            (
                "replace",
                json!({"predecessor_cutex_session_id":"cutex.target","policy":"keep_old","successor":spec(),"start_mode":"custom_message","frozen_message":"exact 私有\n"}),
                "replaced",
            ),
            (
                "director_rotate",
                json!({"expected_predecessor_cutex_session":"cutex.director","expected_authority_epoch":7,"mode":"retain_predecessor_bootstrap_only","successor":spec()}),
                "director_rotated",
            ),
        ] {
            let mut args =
                json!({"action_id":"same-action","project_id":"private","operation":operation});
            args.as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            let mut expected = Value::Null;
            let actual = invoke(args, |request| {
                let typed: crate::agent_management::AgentManagementRequest =
                    serde_json::from_value(request.clone())?;
                assert_eq!(request["operation"], operation);
                assert_eq!(request["project_id"], "private");
                expected = json!({"schema":CONTRACT,"action_id":"same-action","outcome":{"status":"complete","receipt":{
                    "schema":RECEIPT_SCHEMA,"action_id":"same-action","project_id":"private","operation":operation,
                    "request_sha256":format!("{:x}",Sha256::digest(serde_json::to_vec(&typed)?)),
                    "completed_at":"immutable-time","result":{"kind":kind,"historical_snapshot":"unchanged"}}}});
                Ok(expected.clone())
            });
            assert_eq!(actual, expected, "{operation}");
        }
    }

    #[test]
    fn invalid_or_authority_fields_never_reach_transport() {
        for extra in [
            json!({"caller_cutex_session_id":"spoof"}),
            json!({"token":"secret"}),
            json!({"generation":1}),
            json!({"cutex_session_id":"inappropriate"}),
            json!({"operation":"grant_operator"}),
            json!({"operation":"activate"}),
        ] {
            let mut args = json!({"operation":"query_managed","action_id":"check"});
            args.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            assert_eq!(
                invoke(args, |_| panic!("invalid input transported"))["outcome"]["code"],
                "invalid_arguments"
            );
        }
        for (op, field) in [
            ("online", "cutex_session_id"),
            ("create", "spec"),
            ("replace", "predecessor_cutex_session_id"),
            ("director_rotate", "expected_predecessor_cutex_session"),
        ] {
            assert_eq!(
                invoke(json!({"operation":op,"action_id":"check"}), |_| panic!(
                    "missing field transported"
                ))["outcome"]["code"],
                format!("missing_{field}")
            );
        }
        let mut bad = spec();
        bad["principal"] = json!("root");
        assert_eq!(
            invoke(
                json!({"operation":"create","action_id":"check","spec":bad,"start_mode":"bootstrap_only"}),
                |_| panic!("nested control transported")
            )["outcome"]["code"],
            "invalid_arguments"
        );
    }

    #[test]
    fn reviewed_references_preserve_exact_semantics_without_minting_authority() {
        for (operation, fields) in [
            (
                "create",
                json!({"spec":spec(),"start_mode":"bootstrap_only"}),
            ),
            (
                "replace",
                json!({"predecessor_cutex_session_id":"cutex.old","policy":"keep_old","successor":spec(),"start_mode":"bootstrap_only"}),
            ),
            (
                "director_rotate",
                json!({"expected_predecessor_cutex_session":"cutex.old","expected_authority_epoch":1,"mode":"retain_predecessor_bootstrap_only","successor":spec()}),
            ),
        ] {
            let mut args = json!({"operation":operation,"action_id":"reviewed","project_id":"private","bootstrap_intent":"reviewed"});
            args.as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            let result = invoke(args.clone(), |body| {
                assert_eq!(body["bootstrap_intent"], "reviewed");
                assert_eq!(body["operation"], operation);
                Ok(
                    json!({"schema":CONTRACT,"action_id":"reviewed","outcome":{"status":"no_write","code":"bootstrap_intent_missing","detail":"no Human intent"}}),
                )
            });
            assert_eq!(result["outcome"]["code"], "bootstrap_intent_missing");
            args["bootstrap_intent"] = json!("different");
            // Provider validation is also mandatory, before any lifecycle effect.
            let mut request = args.clone();
            request["schema"] = json!(CONTRACT);
            let request: crate::agent_management::AgentManagementRequest =
                serde_json::from_value(request).unwrap();
            assert!(request.validate().is_err());
        }
    }

    #[test]
    fn uncertainty_and_provider_denial_are_not_success_and_receipts_are_bound() {
        let args = json!({"operation":"query_managed","action_id":"check","project_id":"private"});
        let uncertain = invoke(args.clone(), |_| anyhow::bail!("secret transport detail"));
        assert_eq!(uncertain["outcome"]["code"], "response_uncertain");
        assert!(!uncertain.to_string().contains("secret"));
        for status in [408, 500, 503] {
            let value = invoke(args.clone(), |_| {
                Ok(json!({"http_status":status,"error":"private transport detail"}))
            });
            assert_eq!(value["outcome"]["code"], "response_uncertain");
            assert!(!value.to_string().contains("private transport detail"));
        }
        let denied = json!({"schema":CONTRACT,"action_id":"check","outcome":{"status":"no_write","code":"conflict","detail":"exact semantic conflict"}});
        assert_eq!(invoke(args.clone(), |_| Ok(denied.clone())), denied);
        let failure = json!({"schema":CONTRACT,"action_id":"check","outcome":{"status":"owner_action_required","failure":{
            "schema":FAILURE_SCHEMA,"event_id":"event-1","action_id":"check","project_id":"private","operation":"query_managed",
            "code":"owner_action_required","detail":"retained stage","routing_status":"unrouted","created_at":"time"}}});
        assert_eq!(invoke(args.clone(), |_| Ok(failure.clone())), failure);
        for field in ["project_id", "operation", "action_id"] {
            let mut wrong = failure.clone();
            wrong["outcome"]["failure"][field] = json!("foreign");
            assert_eq!(
                invoke(args.clone(), |_| Ok(wrong.clone()))["outcome"]["code"],
                "invalid_provider_response"
            );
        }
        let schema = tool();
        assert_eq!(
            schema["inputSchema"]["required"],
            json!(["operation", "action_id"])
        );
        assert_eq!(schema["inputSchema"]["additionalProperties"], false);
        assert_eq!(
            schema["inputSchema"]["properties"]["spec"]["additionalProperties"],
            false
        );
    }

    #[test]
    fn historical_import_or_move_provenance_is_not_current_membership() {
        let mut record = json!({"project_id":null,"created_by_director_session":null,
            "cutex_session_id":"cutex.imported","native_session_id":"native",
            "spec":spec(),"created_at":"2026-09-09T00:00:00Z","retired_at":null});
        for provenance in [Value::Null, json!("historical-other-project")] {
            record["project_id"] = provenance;
            assert!(project_ids_are_consistent(
                &json!({"project_id":"current","agents":[record.clone()]}),
                "current"
            ));
        }
        record["invented_authority"] = json!(true);
        assert!(!project_ids_are_consistent(&record, "current"));
        assert!(!project_ids_are_consistent(
            &json!({"authority":{"project_id":"foreign"}}),
            "current"
        ));
    }
}
