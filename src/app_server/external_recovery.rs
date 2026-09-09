//! Explicit root-Human recovery for one frozen native message. Never a model tool.
use anyhow::{ensure, Context};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::external_input::{
    Envelope, ExternalInputClient, ProcessingState, Retry, RetryResponse, Status,
};
use crate::launch::stock::ExternalInputBinding;

pub const REPEAT_WARNING: &str = "Retry may repeat model requests and tool effects. This permission applies only to this message and expected attempt.";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryReview {
    pub envelope: Envelope,
    pub binding: ExternalInputBinding,
    pub status: Status,
    pub durable_sha256: String,
    pub authority_sha256: String,
    pub warning: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryReceipt {
    pub action_id: String,
    pub retry_id: String,
    pub review: RecoveryReview,
    /// None is a prepared/uncertain permission, not success or permission to
    /// choose a different retry ID. Reconcile this exact action only.
    pub result: Option<RetryResponse>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecoveryRequest {
    Status {
        action_id: String,
    },
    Review {
        cutex_session_id: String,
        message_id: String,
    },
    Confirm {
        action_id: String,
        retry_id: String,
        review: RecoveryReview,
        confirm_repeat: bool,
    },
}

fn digest(value: &impl Serialize) -> anyhow::Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
fn durable_digest(record: &crate::session::model::CutexSessionRecord) -> anyhow::Result<String> {
    // Registration heartbeats update these observations without changing the
    // durable specification revision or current occurrence. Do not turn an
    // ordinary heartbeat into stale Human confirmation.
    let mut specification = record.clone();
    specification.last_seen_at = None;
    specification.updated_at.clear();
    digest(&specification)
}
fn action_id(id: &str) -> anyhow::Result<()> {
    ensure!(
        !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control),
        "bounded nonempty action/retry ID required"
    );
    Ok(())
}

/// Invoked only after the existing root Management token check. Taking a typed
/// Human principal keeps this separate from Agent/MCP semantic operations.
pub fn handle(
    _human: &crate::management::control_plane::HumanManagementPrincipal,
    request: &RecoveryRequest,
) -> anyhow::Result<serde_json::Value> {
    let repository = crate::management::v2::agent_bus_state::agent_bus_message_repository()?;
    let (owner, message) = match request {
        RecoveryRequest::Status { action_id: id } => {
            action_id(id)?;
            return Ok(serde_json::to_value(repository.recovery_action(id)?)?);
        }
        RecoveryRequest::Review {
            cutex_session_id,
            message_id,
        } => (cutex_session_id.as_str(), message_id.as_str()),
        RecoveryRequest::Confirm {
            action_id: id,
            retry_id,
            review,
            confirm_repeat,
        } => {
            action_id(id)?;
            action_id(retry_id)?;
            ensure!(
                *confirm_repeat && review.warning == REPEAT_WARNING,
                "explicit repeat warning confirmation required"
            );
            if let Some(old) = repository.recovery_action(id)? {
                ensure!(
                    old.review == *review && old.retry_id == *retry_id,
                    "recovery action semantic conflict"
                );
                if old.result.is_some() {
                    return Ok(serde_json::to_value(old)?);
                }
            }
            (
                review.envelope.owner_id.as_str(),
                review.envelope.message.id.as_str(),
            )
        }
    };
    let management = crate::agent_management::AgentManagementStore::open_default()?;
    let _mutation = management
        .try_lock_delivery_mutations()?
        .context("lifecycle transition in progress; review current owner later")?;
    let roster = management.snapshot()?;
    let id = crate::role_revision::CutexSessionId::new(owner.to_string())
        .map_err(|_| anyhow::anyhow!("invalid durable owner"))?;
    ensure!(
        roster
            .agents
            .get(&id)
            .is_none_or(|a| a.retired_at.is_none()),
        "permanently retired recipient"
    );
    let seats = crate::seat::SeatOccupancyStore::open_default()?;
    let path = crate::session::store::cutex_sessions_path()?;
    seats.with_notification_snapshot(|seat| {
        crate::session::store::with_locked_session_store(&path, |store| {
            let record = store.sessions.get(owner).context("durable owner absent")?;
            let generation = record.runtime_generation;
            let client = ExternalInputClient::connect(&path, owner, generation)?;
            let canonical = repository.canonical_message(message)?;
            super::bus_bridge::validate_external_recovery_target(&canonical, owner, seat, &roster)?;
            let snapshot = repository.snapshot_by_message_id(message)?.context("message absent")?;
            let frozen = snapshot.external_input.context("native envelope absent; no implicit projection during recovery")?;
            frozen.validate()?;
            ensure!(frozen.owner_id == owner && frozen.thread_id == client.binding().thread_id, "frozen recovery target conflict");
            let observed = client.status(&[frozen.key()])?.statuses.remove(0);
            let fresh = RecoveryReview {
                envelope: frozen, binding: client.binding().clone(), status: observed,
                durable_sha256: durable_digest(record)?, authority_sha256: digest(&(&roster, seat))?, warning: REPEAT_WARNING.into(),
            };
            match request {
                RecoveryRequest::Status {..} => unreachable!("read-only receipt lookup returned above"),
                RecoveryRequest::Review {..} => Ok(serde_json::to_value(fresh)?),
                RecoveryRequest::Confirm {action_id, retry_id, review, ..} => {
                    let previous = repository.recovery_action(action_id)?;
                    if previous.is_none() {
                        ensure!(fresh == *review, "stale recovery review; no retry issued");
                        ensure!(fresh.status.processing.state == ProcessingState::Held, "only an explicitly held message may be retried");
                    } else {
                        // Reconcile an uncertain exact permission, never grant a
                        // second permission based on changed observations.
                        ensure!(fresh.envelope == review.envelope && fresh.binding == review.binding && fresh.durable_sha256 == review.durable_sha256 && fresh.authority_sha256 == review.authority_sha256, "uncertain retry occurrence/authority changed; explicit recovery review required");
                    }
                    let mut receipt = RecoveryReceipt {action_id: action_id.clone(), retry_id: retry_id.clone(), review: review.clone(), result: None};
                    repository.save_recovery_action(&receipt)?;
                    let retry = Retry {message_id: review.envelope.message.id.clone(), semantic_sha256: review.envelope.semantic_sha256.clone(), expected_attempt_id: review.status.processing.attempt_id.clone(), retry_id: retry_id.clone()};
                    receipt.result = Some(client.retry(&retry)?);
                    client.fence()?;
                    repository.save_recovery_action(&receipt)?;
                    Ok(serde_json::to_value(receipt)?)
                }
            }
        })
    })?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_digest_ignores_only_heartbeat_observations() {
        let mut record = crate::session::model::CutexSessionRecord::new(
            "cutex.11111111-1111-4111-8111-111111111111".into(),
            None,
            "private".into(),
            "/private".into(),
            None,
        )
        .unwrap();
        let original = durable_digest(&record).unwrap();
        record.last_seen_at = Some("later".into());
        record.updated_at = "later".into();
        assert_eq!(original, durable_digest(&record).unwrap());
        record.runtime_generation += 1;
        assert_ne!(original, durable_digest(&record).unwrap());
        record.runtime_generation -= 1;
        record.profile = Some("different-config".into());
        assert_ne!(original, durable_digest(&record).unwrap());
    }
    #[test]
    fn recovery_requests_reject_caller_authority_and_control_fields() {
        assert!(serde_json::from_value::<RecoveryRequest>(serde_json::json!({"operation":"review","cutex_session_id":"owner","message_id":"message","caller":"root"})).is_err());
        assert!(serde_json::from_value::<RecoveryRequest>(
            serde_json::json!({"operation":"retry_all"})
        )
        .is_err());
        assert!(serde_json::from_value::<RecoveryRequest>(
            serde_json::json!({"operation":"status","action_id":"a"})
        )
        .is_ok());
    }
}
