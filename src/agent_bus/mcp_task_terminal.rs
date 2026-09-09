//! Seated completion decisions only. Mechanical context stays in Cutex.
use super::Transport;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    operation: Operation,
    action_id: crate::task_service::ActionId,
    assignment_id: crate::task_service::AssignmentId,
    decision_reference: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    AcceptResult,
    RequestChanges,
    FailResult,
}

pub(super) fn invoke(args: Value, transport: &mut impl Transport) -> Value {
    let args: Args = match serde_json::from_value(args) {
        Ok(args) => args,
        Err(_) => return json!({"status":"no_write","code":"invalid_arguments"}),
    };
    let failure = |code: &str| json!({"schema":"cutex/task-service-terminal-tool-receipt/v1","status":if code == "response_uncertain" {"response_uncertain"} else {"no_write"},"action_id":args.action_id,"assignment_id":args.assignment_id,"code":code});
    if args
        .decision_reference
        .as_ref()
        .is_some_and(|s| s.trim().is_empty() || s.len() > 4096)
        || matches!(args.operation, Operation::RequestChanges) && args.decision_reference.is_none()
    {
        return failure("invalid_decision_reference");
    }
    let op = match args.operation {
        Operation::AcceptResult => "accept_result",
        Operation::RequestChanges => "request_changes",
        Operation::FailResult => "fail_result",
    };
    let body = json!({"operation":op,"body":{"schema":"cutex/task-service-action/v2","action_id":args.action_id,"assignment_id":args.assignment_id,"decision_reference":args.decision_reference}});
    let value = match transport.post("/api/task/v2/terminal-semantic", &body) {
        Ok(value) if value.get("http_status").is_none() => value,
        _ => return failure("response_uncertain"),
    };
    let response: crate::agent_bus::model::TaskServiceActionResponse =
        match serde_json::from_value(value) {
            Ok(r) => r,
            Err(_) => return failure("invalid_provider_response"),
        };
    if response.action_id != args.action_id {
        return failure("invalid_provider_response");
    }
    match response.outcome {
        crate::agent_bus::model::TaskServiceActionOutcome::NoWrite { code, .. } => {
            if code.len() <= 64
                && code
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
            {
                failure(&code)
            } else {
                failure("provider_rejected")
            }
        }
        crate::agent_bus::model::TaskServiceActionOutcome::Committed(receipt) => {
            if receipt.action_id != args.action_id {
                return failure("invalid_provider_response");
            }
            let crate::task_service::ProviderResult::Attempt(attempt) = receipt.result else {
                return failure("invalid_provider_response");
            };
            let expected_phase = match op {
                "accept_result" => crate::task_service::AttemptPhase::Completed,
                "request_changes" => crate::task_service::AttemptPhase::Running,
                _ => crate::task_service::AttemptPhase::Failed,
            };
            if attempt.assignment_id != args.assignment_id || attempt.phase != expected_phase {
                return failure("invalid_provider_response");
            }
            json!({"schema":"cutex/task-service-terminal-tool-receipt/v1","status":"committed","action_id":args.action_id,"assignment_id":args.assignment_id,"attempt_number":attempt.attempt_number,"attempt_phase":attempt.phase})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Wire {
        sent: Vec<Value>,
    }
    impl Transport for Wire {
        fn post(&mut self, path: &str, body: &Value) -> anyhow::Result<Value> {
            assert_eq!(path, "/api/task/v2/terminal-semantic");
            self.sent.push(body.clone());
            Ok(
                json!({"schema":"cutex/task-service-action-response/v2","action_id":"decision","outcome":{"kind":"no_write","body":{"code":"unauthorized","detail":"private mechanical detail"}}}),
            )
        }
    }
    #[test]
    fn terminal_surface_has_no_caller_or_mechanical_authority() {
        for op in ["accept_result", "request_changes", "fail_result"] {
            let args = json!({"operation":op,"action_id":"decision","assignment_id":"assignment","decision_reference":"exact decision"});
            let mut wire = Wire::default();
            let result = invoke(args.clone(), &mut wire);
            assert_eq!(result["code"], "unauthorized");
            assert!(!result.to_string().contains("private mechanical detail"));
            assert_eq!(wire.sent[0]["operation"], op);
            assert!(wire.sent[0].get("context").is_none());
            for field in [
                "caller",
                "token",
                "seat",
                "context",
                "generation",
                "expected_assignment_revision",
            ] {
                let mut forged = args.clone();
                forged[field] = json!("forged");
                let mut wire = Wire::default();
                assert_eq!(invoke(forged, &mut wire)["code"], "invalid_arguments");
                assert!(wire.sent.is_empty());
            }
        }
        let mut wire = Wire::default();
        assert_eq!(
            invoke(
                json!({"operation":"cancel","action_id":"decision","assignment_id":"assignment"}),
                &mut wire
            )["code"],
            "invalid_arguments"
        );
        assert!(wire.sent.is_empty());
    }
}
