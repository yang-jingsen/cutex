//! Authenticated local Human/Management control-plane wire contract.
//!
//! The principal is minted only after the Management HTTP server validates the
//! dedicated root bearer.  It is deliberately absent from every request body:
//! a local client can request an operation, but cannot claim an Agent, Director,
//! seat, or durable session identity.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::agent_management::{
    AgentActionId, AgentOperatorAuditEvent, AgentOperatorGrant, AgentOperatorRosterProjection,
    CurrentProjectMembership, CutexProjectSummary, CutexProjectWorkspace,
    EffectiveProjectPresentation, ProjectAgentChoice, ProjectAuditEvent, ProjectId,
    ProjectPresentationInput, ProjectStateRecord, ProjectTombstone,
};
use crate::role_revision::{CutexSessionId, Rfc3339, Sha256};
use crate::task_service::{ActionId, DirectorActionReceipt, DirectorQuerySelector};

/// Proof that a request crossed the dedicated Human/Management bearer boundary.
///
/// There is intentionally no public constructor and no serde implementation.
/// Agent Bus credentials and request payloads therefore cannot manufacture it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HumanManagementPrincipal {
    _private: (),
}

impl HumanManagementPrincipal {
    pub(crate) fn authenticated() -> Self {
        Self { _private: () }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum HumanManagementProjectSchema {
    #[serde(rename = "cutex/human-management-projects/v1")]
    V1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HumanManagementProjectCollection {
    pub schema: HumanManagementProjectSchema,
    pub projects: Vec<CutexProjectSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub archived_projects: Vec<CutexProjectSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub available_agents: Vec<ProjectAgentChoice>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum HumanManagementProjectMutationSchema {
    #[serde(rename = "cutex/human-management-project-mutation/v1")]
    V1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HumanManagementProjectMutationKind {
    Create {
        director_cutex_session_id: CutexSessionId,
        presentation: ProjectPresentationInput,
    },
    RepairDirectorSeat {
        expected_legacy_occupant: CutexSessionId,
        expected_legacy_epoch: u64,
    },
    AddMember {
        cutex_session_id: CutexSessionId,
    },
    DetachMember {
        cutex_session_id: CutexSessionId,
    },
    Archive,
    Restore,
    Remove,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HumanManagementProjectMutationRequest {
    pub schema: HumanManagementProjectMutationSchema,
    pub action_id: AgentActionId,
    pub project_id: ProjectId,
    pub expected_authority_epoch: u64,
    pub expected_project_revision: u64,
    pub operation: HumanManagementProjectMutationKind,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HumanManagementProjectMutationReceipt {
    pub schema: HumanManagementProjectMutationSchema,
    pub action_id: AgentActionId,
    pub request_sha256: Sha256,
    pub project_id: ProjectId,
    pub operation: HumanManagementProjectMutationKind,
    pub previous_project_revision: u64,
    pub project_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_state: Option<ProjectStateRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub membership: Option<CurrentProjectMembership>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tombstone: Option<ProjectTombstone>,
    pub audit_event: ProjectAuditEvent,
    pub committed_at: Rfc3339,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HumanManagementProjectMutationActionRecord {
    pub request_sha256: Sha256,
    pub receipt: HumanManagementProjectMutationReceipt,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum HumanManagementPresentationSchema {
    #[serde(rename = "cutex/human-management-project-presentation/v1")]
    V1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HumanManagementPresentationUpdateRequest {
    pub schema: HumanManagementPresentationSchema,
    pub project_id: ProjectId,
    pub expected_authority_epoch: u64,
    pub expected_presentation_revision: u64,
    pub presentation: ProjectPresentationInput,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum HumanManagementOperatorSchema {
    #[serde(rename = "cutex/human-management-operator-action/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanManagementOperatorKind {
    Grant,
    Revoke,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HumanManagementOperatorActionRequest {
    pub schema: HumanManagementOperatorSchema,
    pub action_id: AgentActionId,
    pub project_id: ProjectId,
    pub expected_authority_epoch: u64,
    pub expected_grant_revision: u64,
    pub operation: HumanManagementOperatorKind,
    pub operator_cutex_session_id: CutexSessionId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HumanManagementOperatorReceipt {
    pub schema: HumanManagementOperatorSchema,
    pub action_id: AgentActionId,
    pub request_sha256: Sha256,
    pub operation: HumanManagementOperatorKind,
    pub project_id: ProjectId,
    pub authority_epoch: u64,
    pub primary_director_cutex_session_id: CutexSessionId,
    pub operator_cutex_session_id: CutexSessionId,
    pub previous_grant_revision: u64,
    pub grant_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant: Option<AgentOperatorGrant>,
    pub roster: AgentOperatorRosterProjection,
    pub audit_event: AgentOperatorAuditEvent,
    pub committed_at: Rfc3339,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HumanManagementOperatorActionRecord {
    pub request_sha256: Sha256,
    pub receipt: HumanManagementOperatorReceipt,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum HumanManagementTaskQuerySchema {
    #[serde(rename = "cutex/human-management-task-query/v1")]
    V1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HumanManagementTaskQueryRequest {
    pub schema: HumanManagementTaskQuerySchema,
    pub action_id: ActionId,
    pub selector: DirectorQuerySelector,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HumanManagementTaskQueryResponse {
    pub schema: HumanManagementTaskQuerySchema,
    /// Exact current seat occupant used only as an authority/scope anchor.
    pub director_seat_occupant: CutexSessionId,
    pub director_seat_epoch: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub director_seats: BTreeMap<ProjectId, HumanManagementDirectorSeat>,
    pub project_ids: Vec<ProjectId>,
    pub project_presentations: BTreeMap<ProjectId, EffectiveProjectPresentation>,
    pub receipt: DirectorActionReceipt,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HumanManagementDirectorSeat {
    pub project_id: ProjectId,
    pub occupant_cutex_session: CutexSessionId,
    pub epoch: u64,
}

/// Complete read projection returned by the authenticated Project detail route.
pub type HumanManagementProjectWorkspace = CutexProjectWorkspace;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn management_requests_cannot_claim_an_agent_or_director_identity() {
        let base = serde_json::json!({
            "schema": "cutex/human-management-task-query/v1",
            "action_id": "management-query-1",
            "selector": { "kind": "all" }
        });
        assert!(serde_json::from_value::<HumanManagementTaskQueryRequest>(base.clone()).is_ok());
        for forbidden in [
            "caller_cutex_session",
            "caller_runtime_agent_id",
            "director_cutex_session",
            "seat_id",
            "project_id",
        ] {
            let mut forged = base.clone();
            forged[forbidden] = serde_json::json!("forged");
            assert!(serde_json::from_value::<HumanManagementTaskQueryRequest>(forged).is_err());
        }
    }

    #[test]
    fn project_mutation_contract_is_strict_and_round_trips_custom_rgb() {
        let base = serde_json::json!({
            "schema": "cutex/human-management-project-mutation/v1",
            "action_id": "create-project-1",
            "project_id": "project-rgb",
            "expected_authority_epoch": 0,
            "expected_project_revision": 0,
            "operation": {
                "kind": "create",
                "director_cutex_session_id": "cutex.director-rgb",
                "presentation": {
                    "display_name": "RGB Project",
                    "badge_label": "CX",
                    "color": "#12AB34"
                }
            }
        });
        let request = serde_json::from_value::<HumanManagementProjectMutationRequest>(base.clone())
            .expect("strict create request");
        assert_eq!(serde_json::to_value(&request).unwrap(), base);

        for forbidden in ["caller_cutex_session", "seat_id", "authority_epoch"] {
            let mut forged = base.clone();
            forged[forbidden] = serde_json::json!("forged");
            assert!(
                serde_json::from_value::<HumanManagementProjectMutationRequest>(forged).is_err()
            );
        }
        let mut invalid = base;
        invalid["operation"]["presentation"]["color"] = serde_json::json!("#123");
        assert!(serde_json::from_value::<HumanManagementProjectMutationRequest>(invalid).is_err());
    }
}

/// Local administration of assignment bookkeeping. No Director seat is needed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum HumanTaskRecoveryRequest {
    Query {
        #[serde(default)]
        assignee: Option<crate::role_revision::CutexSessionId>,
    },
    Reassign {
        action_id: crate::task_service::ActionId,
        assignment_id: crate::task_service::AssignmentId,
        new_assignment_id: crate::task_service::AssignmentId,
        assignee: crate::role_revision::CutexSessionId,
    },
    Cancel {
        action_id: crate::task_service::ActionId,
        assignment_id: crate::task_service::AssignmentId,
    },
}
