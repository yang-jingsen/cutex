//! Outbound-only native Task tool surface. All transitions remain provider-owned.
use serde_json::{json, Value};

#[path = "mcp_task_director.rs"]
mod director;
#[path = "mcp_task_terminal.rs"]
mod terminal;
#[path = "mcp_task_worker.rs"]
mod worker;

pub(super) trait Transport {
    fn post(&mut self, path: &str, body: &Value) -> anyhow::Result<Value>;
}

pub(super) const WORKER: &str = "cutex_task_service";
pub(super) const DIRECTOR: &str = "cutex_task_service_director";
pub(super) const TERMINAL: &str = "cutex_task_service_terminal";

pub(super) fn tools() -> Vec<Value> {
    let mut worker = json!({"operation":{"type":"string","enum":["start","report_status","block","resume","submit","decline","abort_attempt"]}});
    for field in [
        "action_id",
        "assignment_id",
        "summary",
        "evidence_sha256",
        "result_sha256",
        "result_reference",
    ] {
        worker[field] = json!({"type":"string"});
    }
    worker["assignment_id"]["description"] =
        json!("Required for every operation; exact verified assignment identity.");
    worker["action_id"]["description"] =
        json!("Stable action identity; reuse only for exact replay.");
    worker["summary"]["description"] =
        json!("Required for report_status (4096 UTF-8 bytes) and block (2048 UTF-8 bytes) only.");
    worker["evidence_sha256"]["description"] =
        json!("Optional for report_status only; 64 lowercase hex characters.");
    worker["result_sha256"]["description"] =
        json!("Required for submit only; immutable result digest, 64 lowercase hex characters.");
    worker["result_reference"]["description"] =
        json!("Required for submit only; stable reference, at most 4096 UTF-8 bytes.");
    let mut director = json!({
        "operation":{"type":"string","enum":["create_revision","assign","create_and_assign","query","accept_result","request_changes","fail_result","cancel"]},
        "task_revision":{"type":"integer","minimum":1,"maximum":9007199254740991u64,"description":"Required for create_revision, assign and create_and_assign."},
        "completion_policy":{"type":"string","enum":["director_acceptance","release_review"]},
        "selector":{"type":"object","properties":{"kind":{"type":"string","enum":["all","task","assignment"]},"task_id":{"type":"string"},"assignment_id":{"type":"string"}},"required":["kind"],"additionalProperties":false}
    });
    for field in [
        "action_id",
        "project_id",
        "workflow_id",
        "task_id",
        "opaque_contract",
        "completion_authority_cutex_session_id",
        "assignment_id",
        "assignee_cutex_session_id",
        "summary",
        "decision_reference",
    ] {
        director[field] = json!({"type":"string"});
    }
    director["assignment_id"]["description"] =
        json!("Required for assign, create_and_assign and all result/cancel decisions.");
    director["opaque_contract"]["description"] = json!("Exact UTF-8 contract, at most 131072 bytes. Trusted adapter computes SHA-256; do not submit a digest.");
    director["completion_authority_cutex_session_id"]["description"] = json!("Optional intended completion target; provider validates its current seat. Never caller authority.");
    vec![
        json!({"name":TERMINAL,"description":"Explicit completion-seat decision (including Release). accept_result/request_changes/fail_result only; request_changes requires decision_reference. Current runtime, seat and mechanical revisions are resolved by Cutex, never tool arguments. Reuse exact action_id/payload after uncertainty.","inputSchema":{"type":"object","properties":{"operation":{"type":"string","enum":["accept_result","request_changes","fail_result"]},"action_id":{"type":"string"},"assignment_id":{"type":"string"},"decision_reference":{"type":"string","maxLength":4096}},"required":["operation","action_id","assignment_id"],"additionalProperties":false}}),
        json!({"name":WORKER,"description":"Semantic Worker action. Requires operation, action_id and assignment_id. Runtime and assignment authority are provider-authenticated, never supplied in arguments. report_status/block require summary; submit requires result_sha256 and result_reference.","inputSchema":{"type":"object","properties":worker,"required":["operation","action_id","assignment_id"],"additionalProperties":false}}),
        json!({"name":DIRECTOR,"description":"Semantic Director action. Create requires project_id, workflow_id, task_id, task_revision, opaque_contract, completion_policy. Assign requires project_id, task_id, task_revision, assignment_id, assignee_cutex_session_id, summary. create_and_assign requires both sets and is a recoverable two-step operation, NOT atomic. Query requires selector. Decisions require assignment_id. Reuse action_id exactly after uncertain response.","inputSchema":{"type":"object","properties":director,"required":["operation","action_id"],"additionalProperties":false}}),
    ]
}

/// Only fixed field names reach diagnostics; never echo user values or tokens.
fn missing_field(name: &str, args: &Value) -> Option<&'static str> {
    let operation = args["operation"].as_str()?;
    let required: &[&str] = if name == WORKER {
        match operation {
            "report_status" | "block" => &["action_id", "assignment_id", "summary"],
            "submit" => &[
                "action_id",
                "assignment_id",
                "result_sha256",
                "result_reference",
            ],
            _ => &["action_id", "assignment_id"],
        }
    } else {
        match operation {
            "create_revision" => &[
                "action_id",
                "project_id",
                "workflow_id",
                "task_id",
                "task_revision",
                "opaque_contract",
                "completion_policy",
            ],
            "assign" => &[
                "action_id",
                "project_id",
                "task_id",
                "task_revision",
                "assignment_id",
                "assignee_cutex_session_id",
                "summary",
            ],
            "create_and_assign" => &[
                "action_id",
                "project_id",
                "workflow_id",
                "task_id",
                "task_revision",
                "opaque_contract",
                "completion_policy",
                "assignment_id",
                "assignee_cutex_session_id",
                "summary",
            ],
            "query" => &["action_id", "selector"],
            _ => &["action_id", "assignment_id"],
        }
    };
    required
        .iter()
        .copied()
        .find(|key| args.get(*key).is_none_or(Value::is_null))
}

pub(super) fn invoke(name: &str, args: Value, transport: &mut impl Transport) -> Value {
    if let Some(field) = missing_field(name, &args) {
        return json!({"schema":if name==WORKER {"cutex/task-service-tool-receipt/v1"} else {"cutex/task-service-director-tool-receipt/v1"},"status":"no_write","code":format!("missing_{field}")});
    }
    match name {
        WORKER => worker::invoke(args, transport),
        DIRECTOR => director::invoke(args, transport),
        TERMINAL => terminal::invoke(args, transport),
        _ => json!({"status":"no_write","code":"unsupported_operation"}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    #[derive(Default)]
    struct Wire {
        sent: Vec<(String, Value)>,
        replies: VecDeque<anyhow::Result<Value>>,
    }
    impl Transport for Wire {
        fn post(&mut self, path: &str, body: &Value) -> anyhow::Result<Value> {
            self.sent.push((path.into(), body.clone()));
            self.replies.pop_front().unwrap_or_else(|| Ok(json!({})))
        }
    }
    fn create() -> Value {
        json!({"operation":"create_and_assign","action_id":"act","project_id":"p","workflow_id":"w","task_id":"t","task_revision":1,"opaque_contract":" exact 合同\n","completion_policy":"director_acceptance","assignment_id":"a","assignee_cutex_session_id":"cutex.00000000-0000-0000-0000-000000000001","summary":"work"})
    }
    #[test]
    fn native_schema_fields_and_missing_diagnostics_never_supply_authority() {
        for name in [WORKER, DIRECTOR] {
            for key in [
                "caller",
                "caller_cutex_session_id",
                "token",
                "seat",
                "threadId",
                "runtime_generation",
                "attempt_token",
                "expected_assignment_revision",
            ] {
                let mut args = if name == WORKER {
                    json!({"operation":"start","action_id":"a","assignment_id":"x"})
                } else {
                    create()
                };
                args[key] = json!("secret");
                let mut wire = Wire::default();
                let result = invoke(name, args, &mut wire);
                assert!(wire.sent.is_empty());
                assert_eq!(result["status"], "no_write");
                assert!(!result.to_string().contains("secret"));
            }
        }
        for field in ["task_revision", "assignment_id"] {
            let mut args = create();
            args.as_object_mut().unwrap().remove(field);
            let mut wire = Wire::default();
            assert_eq!(
                invoke(DIRECTOR, args, &mut wire)["code"],
                format!("missing_{field}")
            );
            assert!(wire.sent.is_empty());
        }
    }
    #[test]
    fn exact_utf8_hash_and_nonatomic_continuation_survive() {
        use sha2::{Digest, Sha256};
        let mut wire = Wire::default();
        wire.replies.push_back(Ok(json!({"schema":"cutex/task-service-director-receipt/v1","action_id":"act","operation":"create_and_assign","status":"response_uncertain","continuation":{"phase":"create_revision_committed","retry_action_id":"act"},"detail":"secret internal diagnostic"})));
        let result = invoke(DIRECTOR, create(), &mut wire);
        assert_eq!(wire.sent.len(), 1);
        assert_eq!(
            wire.sent[0].1["create_revision"]["contract_sha256"],
            format!("{:x}", Sha256::digest(" exact 合同\n".as_bytes()))
        );
        assert_eq!(wire.sent[0].1["assign"]["task_revision"], 1);
        assert_eq!(result["continuation"]["retry_action_id"], "act");
        assert!(!result.to_string().contains("secret"));
        let mut wire = Wire::default();
        wire.replies
            .push_back(Err(anyhow::anyhow!("secret lost response")));
        let result = invoke(DIRECTOR, create(), &mut wire);
        assert_eq!(result["status"], "response_uncertain");
        assert_eq!(result["continuation"]["phase"], "outcome_unknown");
        assert_eq!(wire.sent.len(), 1);
    }
    #[test]
    fn worker_lost_response_reprepares_same_action_and_hides_mechanical_tokens() {
        let action = json!({"operation":"start","body":{"schema":"cutex/task-service-action/v2","action_id":"s","assignment_id":"a"}});
        let receipt = json!({"schema":"cutex/task-service-receipt/v3","action_id":"s","result":{"kind":"attempt","body":{"project_id":"p","assignment_id":"a","attempt_number":1,"phase":"running"}}});
        let mut wire = Wire::default();
        wire.replies.extend([
            Ok(json!({"schema":"cutex/task-service-worker-prepare-response/v2","outcome":{"kind":"prepared","body":{"schema":"cutex/task-service-worker-provider/v2","action":action,"context":{"expected_assignment_revision":1,"attempt":null}}}})),
            Err(anyhow::anyhow!("lost response secret")),
            Ok(json!({"schema":"cutex/task-service-worker-prepare-response/v2","outcome":{"kind":"committed","body":receipt}})),
        ]);
        let result = invoke(
            WORKER,
            json!({"operation":"start","action_id":"s","assignment_id":"a"}),
            &mut wire,
        );
        assert_eq!(result["status"], "committed");
        assert_eq!(result["attempt_number"], 1);
        assert_eq!(wire.sent.len(), 3);
        assert_eq!(wire.sent[0], wire.sent[2]);
        assert!(!result.to_string().contains("expected_assignment_revision"));
    }
    #[test]
    fn worker_payload_limits_and_all_operations_translate_exactly() {
        for op in [
            "start",
            "report_status",
            "block",
            "resume",
            "submit",
            "decline",
            "abort_attempt",
        ] {
            let mut args = json!({"operation":op,"action_id":"a","assignment_id":"x"});
            if matches!(op, "report_status" | "block") {
                args["summary"] = json!("summary");
            }
            if op == "submit" {
                args["result_sha256"] = json!("a".repeat(64));
                args["result_reference"] = json!("result");
            }
            let mut wire = Wire::default();
            invoke(WORKER, args, &mut wire);
            assert_eq!(wire.sent.len(), 1);
            assert_eq!(wire.sent[0].1["action"]["operation"], op);
        }
        for (op, len) in [("block", 2049), ("report_status", 4097)] {
            let mut wire = Wire::default();
            let result = invoke(
                WORKER,
                json!({"operation":op,"action_id":"a","assignment_id":"x","summary":"x".repeat(len)}),
                &mut wire,
            );
            assert!(wire.sent.is_empty());
            assert_eq!(result["status"], "no_write");
        }
    }

    #[test]
    fn director_policy_hash_requiredness_and_cross_operation_fields_are_strict() {
        for (field, value) in [
            ("task_revision", json!(0)),
            ("task_revision", json!(9007199254740992u64)),
            ("contract_sha256", json!("0".repeat(64))),
            ("completion_policy", json!("worker_acceptance")),
            ("opaque_contract", json!("x".repeat(131073))),
        ] {
            let mut args = create();
            args[field] = value;
            let mut wire = Wire::default();
            assert_eq!(invoke(DIRECTOR, args, &mut wire)["status"], "no_write");
            assert!(wire.sent.is_empty());
        }
        for op in ["accept_result", "request_changes", "fail_result", "cancel"] {
            let mut wire = Wire::default();
            invoke(
                DIRECTOR,
                json!({"operation":op,"action_id":"a","assignment_id":"x","project_id":"p"}),
                &mut wire,
            );
            assert!(wire.sent.is_empty());
        }
        let mut args = create();
        args["completion_policy"] = json!("release_review");
        args["completion_authority_cutex_session_id"] = json!("cutex.release");
        let mut wire = Wire::default();
        invoke(DIRECTOR, args, &mut wire);
        assert_eq!(
            wire.sent[0].1["create_revision"]["completion_policy"],
            "release_review"
        );
        assert_eq!(
            wire.sent[0].1["create_revision"]["completion_authority_cutex_session_id"],
            "cutex.release"
        );
    }

    #[test]
    fn composite_create_substep_conflict_is_not_hidden_as_invalid_response() {
        let mut wire = Wire::default();
        wire.replies.push_back(Ok(json!({"schema":"cutex/task-service-director-receipt/v1","action_id":"act","operation":"create_revision","status":"conflict","code":"conflict"})));
        let result = invoke(DIRECTOR, create(), &mut wire);
        assert_eq!(result["status"], "conflict");
        assert_eq!(result["operation"], "create_and_assign");
        let mut wire = Wire::default();
        wire.replies.push_back(Ok(json!({"schema":"cutex/task-service-director-receipt/v1","action_id":"act","operation":"create_revision","status":"committed"})));
        assert_eq!(
            invoke(DIRECTOR, create(), &mut wire)["code"],
            "invalid_provider_response"
        );
    }
}
