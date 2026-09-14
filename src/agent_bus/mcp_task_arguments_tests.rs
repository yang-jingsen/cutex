use super::super::{invoke, tools, Transport};
use super::*;

#[derive(Default)]
struct Wire {
    sent: Vec<Value>,
}
impl Transport for Wire {
    fn post(&mut self, _: &str, body: &Value) -> anyhow::Result<Value> {
        self.sent.push(body.clone());
        Ok(
            json!({"schema":"cutex/task-service-director-receipt/v1","operation":body["operation"],"action_id":body["action_id"],"assignment_id":"assignment-1","project_id":"project-1","status":"committed"}),
        )
    }
}

#[test]
fn decisions_identify_forbidden_fields_without_sending_a_request() {
    for operation in ["accept_result", "request_changes", "fail_result", "cancel"] {
        for field in ["summary", "project_id", "task_id", "task_revision"] {
            let mut args = json!({"operation":operation,"action_id":"accept-1","assignment_id":"assignment-1"});
            args[field] = json!("PRIVATE_VALUE_MUST_NOT_BE_ECHOED");
            let mut wire = Wire::default();
            let receipt = invoke(DIRECTOR, args, &mut wire);
            assert_eq!(receipt["status"], "no_write");
            assert_eq!(receipt["code"], "field_not_allowed");
            assert_eq!(receipt["operation"], operation);
            assert_eq!(receipt["fields"], json!([field]));
            assert_eq!(
                receipt["allowed_fields"],
                json!([
                    "operation",
                    "action_id",
                    "assignment_id",
                    "decision_reference"
                ])
            );
            assert!(!receipt.to_string().contains("PRIVATE_VALUE"));
            assert!(wire.sent.is_empty());
        }
    }
}

#[test]
fn minimal_acceptance_and_optional_reference_reach_provider_unchanged() {
    for reference in [None, Some("/acceptance.md")] {
        let mut args = json!({"operation":"accept_result","action_id":"accept-1","assignment_id":"assignment-1"});
        if let Some(reference) = reference {
            args["decision_reference"] = json!(reference);
        }
        let mut wire = Wire::default();
        assert_eq!(
            invoke(DIRECTOR, args.clone(), &mut wire)["status"],
            "committed"
        );
        args["schema"] = json!("cutex/task-service-director-action/v2");
        assert_eq!(wire.sent, vec![args]);
    }
}

#[test]
fn published_field_help_matches_decision_and_worker_preflight() {
    let schema = tools()
        .into_iter()
        .find(|tool| tool["name"] == DIRECTOR)
        .unwrap();
    let help = schema["description"].as_str().unwrap();
    assert!(help.contains("accept_result: required [assignment_id]; optional [decision_reference]"));
    assert!(
        schema["inputSchema"]["properties"]["summary"]["description"]
            .as_str()
            .unwrap()
            .contains("Required for [assign, create_and_assign]")
    );
    let mut wire = Wire::default();
    let error = invoke(
        WORKER,
        json!({"operation":"submit","action_id":"submit-1","assignment_id":"assignment-1","summary":"not allowed","result_sha256":"a".repeat(64),"result_reference":"/result"}),
        &mut wire,
    );
    assert_eq!(error["fields"], json!(["summary"]));
    assert!(wire.sent.is_empty());
}
