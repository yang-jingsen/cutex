//! Root management task recovery, independent of Director seat health.
use std::net::TcpStream;

use serde_json::json;

use crate::http::server::{write_json_response, SimpleHttpRequest};
use crate::management::control_plane::{HumanManagementPrincipal, HumanTaskRecoveryRequest};
use crate::task_service::{ProviderError, TaskServiceProvider};

pub(super) fn handle(stream: &mut TcpStream, request: &SimpleHttpRequest) -> anyhow::Result<()> {
    let payload = match serde_json::from_slice::<HumanTaskRecoveryRequest>(&request.body) {
        Ok(payload) => payload,
        Err(error) => {
            return super::server::write_v2_error(
                stream,
                400,
                "Bad Request",
                "invalid_request",
                &error.to_string(),
                false,
                json!({}),
            )
        }
    };
    // The dispatcher requires the existing management root credential before
    // constructing this principal. No agent identity or Director seat is used.
    let principal = HumanManagementPrincipal::authenticated();
    let result = (|| -> anyhow::Result<serde_json::Value> {
        let provider = TaskServiceProvider::open(
            crate::task_delivery::provider_adapter::default_task_service_provider_root()?,
        )?;
        match payload {
            HumanTaskRecoveryRequest::Query { assignee } => {
                let snapshot = provider.query_live()?;
                let assignments = snapshot
                    .assignments
                    .into_values()
                    .filter(|value| {
                        assignee
                            .as_ref()
                            .is_none_or(|id| &value.assignee_cutex_session == id)
                    })
                    .collect::<Vec<_>>();
                Ok(
                    json!({"assignments": assignments, "journal_sequence": snapshot.journal_sequence}),
                )
            }
            HumanTaskRecoveryRequest::Reassign {
                action_id,
                assignment_id,
                new_assignment_id,
                assignee,
            } => {
                let receipt = provider.human_reassign_assignment(
                    &principal,
                    &action_id,
                    &assignment_id,
                    &new_assignment_id,
                    &assignee,
                )?;
                Ok(
                    json!({"receipt": receipt, "previous_assignment_id": assignment_id,
                    "dispatch": "not_requested"}),
                )
            }
            HumanTaskRecoveryRequest::Cancel {
                action_id,
                assignment_id,
            } => Ok(serde_json::to_value(provider.human_cancel_assignment(
                &principal,
                &action_id,
                &assignment_id,
            )?)?),
        }
    })();
    match result {
        Ok(value) => write_json_response(stream, 200, "OK", &value),
        Err(error) => {
            let (status, reason, code) = match error.downcast_ref::<ProviderError>() {
                Some(ProviderError::NotFound(_)) => (404, "Not Found", "assignment_not_found"),
                Some(ProviderError::Conflict(_)) => (409, "Conflict", "task_action_conflict"),
                _ => (503, "Service Unavailable", "task_recovery_unavailable"),
            };
            super::server::write_v2_error(
                stream,
                status,
                reason,
                code,
                &error.to_string(),
                status == 503,
                json!({}),
            )
        }
    }
}
