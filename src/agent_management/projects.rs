//! Provider-authoritative Cutex Project read model and presentation settings.
//!
//! A Cutex Project is an Agent Management ownership boundary. Its identity is
//! only the canonical [`ProjectId`] stored by the provider. Display metadata is
//! deliberately non-authoritative and can never select or grant authority.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthStr;

use crate::management::control_plane::{
    HumanManagementOperatorActionRecord, HumanManagementOperatorActionRequest,
    HumanManagementOperatorKind, HumanManagementOperatorReceipt,
    HumanManagementPresentationUpdateRequest, HumanManagementPrincipal,
    HumanManagementProjectCollection, HumanManagementProjectSchema,
};
use crate::role_revision::{CutexSessionId, Rfc3339};

use super::{
    now, AgentManagementError, AgentManagementInvocation, AgentManagementProvider,
    AgentOperatorGrant, AgentRuntimeObservation, ManagedAgentRecord, ProjectAuthority, ProjectId,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectPaletteColor {
    Cyan,
    Blue,
    Green,
    Magenta,
    Yellow,
    Red,
}

impl ProjectPaletteColor {
    pub const ALL: [Self; 6] = [
        Self::Cyan,
        Self::Blue,
        Self::Green,
        Self::Magenta,
        Self::Yellow,
        Self::Red,
    ];

    pub fn token(self) -> &'static str {
        match self {
            Self::Cyan => "cyan",
            Self::Blue => "blue",
            Self::Green => "green",
            Self::Magenta => "magenta",
            Self::Yellow => "yellow",
            Self::Red => "red",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectPresentationSettings {
    pub display_name: String,
    pub badge_label: String,
    pub color: ProjectPaletteColor,
    pub revision: u64,
    pub updated_at: Rfc3339,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_by_director_session: Option<CutexSessionId>,
    #[serde(default)]
    pub updated_by_human_management: bool,
    /// Presentation records are intentionally forward-compatible. Unknown
    /// fields survive a read/change/write cycle.
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectPresentationInput {
    pub display_name: String,
    pub badge_label: String,
    pub color: ProjectPaletteColor,
}

impl ProjectPresentationInput {
    pub fn validate(&self) -> Result<(), AgentManagementError> {
        let display_name = self.display_name.trim();
        if display_name.is_empty()
            || display_name.chars().count() > 80
            || display_name.chars().any(char::is_control)
        {
            return Err(AgentManagementError::InvalidRequest(
                "invalid_project_display_name",
            ));
        }
        let badge = self.badge_label.trim();
        if !(1..=2).contains(&UnicodeWidthStr::width(badge))
            || badge.chars().any(char::is_control)
            || badge.contains(char::is_whitespace)
        {
            return Err(AgentManagementError::InvalidRequest(
                "invalid_project_badge_label",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectPresentationUpdateRequest {
    pub project_id: ProjectId,
    pub expected_presentation_revision: u64,
    pub presentation: ProjectPresentationInput,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveProjectPresentation {
    pub display_name: String,
    pub badge_label: String,
    pub color: ProjectPaletteColor,
    pub revision: u64,
    pub stored: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectMemberLifecycle {
    Online,
    Offline,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectMemberProjection {
    pub agent: ManagedAgentRecord,
    pub lifecycle: ProjectMemberLifecycle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<AgentRuntimeObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectDirectorProjection {
    pub cutex_session_id: CutexSessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<ProjectMemberProjection>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectAgentOperatorProjection {
    pub grant: AgentOperatorGrant,
    pub member: ProjectMemberProjection,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectAccessRole {
    PrimaryDirector,
    AgentOperator,
    HumanManagement,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyOperatorRepairCandidate {
    pub rotation_action_id: super::AgentActionId,
    pub predecessor_cutex_session_id: CutexSessionId,
    pub successor_cutex_session_id: CutexSessionId,
    pub rotation_mode: super::DirectorRotateMode,
    pub completed_at: Rfc3339,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CutexProjectSummary {
    pub project_id: ProjectId,
    pub authority_epoch: u64,
    pub director_cutex_session_id: CutexSessionId,
    pub access_role: ProjectAccessRole,
    pub operator_count: usize,
    pub presentation: EffectiveProjectPresentation,
    pub active_member_count: usize,
    pub retired_member_count: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CutexProjectWorkspace {
    pub project_id: ProjectId,
    pub authority_epoch: u64,
    pub director: ProjectDirectorProjection,
    pub access_role: ProjectAccessRole,
    pub operator_grant_revision: u64,
    pub agent_operators: Vec<ProjectAgentOperatorProjection>,
    pub presentation: EffectiveProjectPresentation,
    pub active_agents: Vec<ProjectMemberProjection>,
    pub retired_agents: Vec<ProjectMemberProjection>,
    /// Review-only candidates for retained Director rotations committed before
    /// Operator grants existed. Nothing in this projection performs a repair.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub legacy_operator_repair_candidates: Vec<LegacyOperatorRepairCandidate>,
}

pub trait ProjectRuntimeObserver {
    fn observe(
        &self,
        cutex_session_id: &CutexSessionId,
    ) -> Result<AgentRuntimeObservation, AgentManagementError>;
}

impl<F> ProjectRuntimeObserver for F
where
    F: Fn(&CutexSessionId) -> Result<AgentRuntimeObservation, AgentManagementError>,
{
    fn observe(
        &self,
        cutex_session_id: &CutexSessionId,
    ) -> Result<AgentRuntimeObservation, AgentManagementError> {
        self(cutex_session_id)
    }
}

impl AgentManagementProvider {
    /// Lists only the projects for which the authenticated caller currently
    /// occupies the Director seat. No group, cwd, display name, or native
    /// Codex project record participates in selection.
    pub fn list_cutex_projects(
        &self,
        invocation: &AgentManagementInvocation,
    ) -> Result<Vec<CutexProjectSummary>, AgentManagementError> {
        let snapshot = self.store().snapshot()?;
        let mut projects = snapshot
            .projects
            .values()
            .filter_map(|authority| {
                project_access_role(&snapshot, invocation, &authority.project_id)
                    .ok()
                    .map(|role| summary(&snapshot, authority, role))
            })
            .collect::<Vec<_>>();
        projects.sort_by(|left, right| left.project_id.cmp(&right.project_id));
        if projects.is_empty() {
            return Err(AgentManagementError::NotAuthorizedDirector);
        }
        Ok(projects)
    }

    pub fn read_cutex_project(
        &self,
        invocation: &AgentManagementInvocation,
        project_id: &ProjectId,
        observer: &dyn ProjectRuntimeObserver,
    ) -> Result<CutexProjectWorkspace, AgentManagementError> {
        let snapshot = self.store().snapshot()?;
        let (authority, access_role) = authorized_project(&snapshot, invocation, project_id)?;
        project_workspace(&snapshot, authority, access_role, observer)
    }

    pub fn update_project_presentation(
        &self,
        invocation: &AgentManagementInvocation,
        request: &ProjectPresentationUpdateRequest,
    ) -> Result<ProjectPresentationSettings, AgentManagementError> {
        request.presentation.validate()?;
        self.store().with_state(true, |mut state| {
            authorized_primary_authority(&state, invocation, &request.project_id)?;
            let current_revision = state
                .project_presentations
                .get(&request.project_id)
                .map(|settings| settings.revision)
                .unwrap_or(0);
            if current_revision != request.expected_presentation_revision {
                return Err(AgentManagementError::Conflict(
                    "project_presentation_revision_conflict",
                ));
            }
            if let Some(current) = state.project_presentations.get(&request.project_id) {
                if current.display_name == request.presentation.display_name.trim()
                    && current.badge_label == request.presentation.badge_label.trim()
                    && current.color == request.presentation.color
                {
                    let current = current.clone();
                    return Ok((state, current, false));
                }
            }
            let revision =
                current_revision
                    .checked_add(1)
                    .ok_or(AgentManagementError::Conflict(
                        "project_presentation_revision_overflow",
                    ))?;
            let extra = state
                .project_presentations
                .get(&request.project_id)
                .map(|settings| settings.extra.clone())
                .unwrap_or_default();
            let settings = ProjectPresentationSettings {
                display_name: request.presentation.display_name.trim().to_string(),
                badge_label: request.presentation.badge_label.trim().to_string(),
                color: request.presentation.color,
                revision,
                updated_at: now(),
                updated_by_director_session: Some(invocation.caller_cutex_session.clone()),
                updated_by_human_management: false,
                extra,
            };
            state
                .project_presentations
                .insert(request.project_id.clone(), settings.clone());
            Ok((state, settings, true))
        })
    }

    /// Lists every canonical Project after the dedicated Management server
    /// has authenticated the local Human principal. Agent identities are not
    /// accepted by this boundary and are not synthesized here.
    pub fn list_cutex_projects_for_management(
        &self,
        _principal: &HumanManagementPrincipal,
    ) -> Result<HumanManagementProjectCollection, AgentManagementError> {
        let snapshot = self.store().snapshot()?;
        let mut projects = snapshot
            .projects
            .values()
            .map(|authority| summary(&snapshot, authority, ProjectAccessRole::HumanManagement))
            .collect::<Vec<_>>();
        projects.sort_by(|left, right| left.project_id.cmp(&right.project_id));
        Ok(HumanManagementProjectCollection {
            schema: HumanManagementProjectSchema::V1,
            projects,
        })
    }

    pub fn read_cutex_project_for_management(
        &self,
        _principal: &HumanManagementPrincipal,
        project_id: &ProjectId,
        observer: &dyn ProjectRuntimeObserver,
    ) -> Result<CutexProjectWorkspace, AgentManagementError> {
        let snapshot = self.store().snapshot()?;
        let authority = snapshot
            .projects
            .get(project_id)
            .ok_or(AgentManagementError::ProjectNotAuthorized)?;
        project_workspace(
            &snapshot,
            authority,
            ProjectAccessRole::HumanManagement,
            observer,
        )
    }

    pub fn update_project_presentation_for_management(
        &self,
        _principal: &HumanManagementPrincipal,
        request: &HumanManagementPresentationUpdateRequest,
    ) -> Result<ProjectPresentationSettings, AgentManagementError> {
        request.presentation.validate()?;
        let _execution = super::provider::provider_execution_lock()
            .lock()
            .map_err(|_| AgentManagementError::PersistenceUnavailable)?;
        let _mutation = self.store().lock_mutations()?;
        self.store().with_state(true, |mut state| {
            let authority = state
                .projects
                .get(&request.project_id)
                .ok_or(AgentManagementError::ProjectNotAuthorized)?;
            if authority.authority_epoch != request.expected_authority_epoch {
                return Err(AgentManagementError::Conflict("stale_project_authority"));
            }
            let current_revision = state
                .project_presentations
                .get(&request.project_id)
                .map(|settings| settings.revision)
                .unwrap_or(0);
            if current_revision != request.expected_presentation_revision {
                return Err(AgentManagementError::Conflict(
                    "project_presentation_revision_conflict",
                ));
            }
            if let Some(current) = state.project_presentations.get(&request.project_id) {
                if current.display_name == request.presentation.display_name.trim()
                    && current.badge_label == request.presentation.badge_label.trim()
                    && current.color == request.presentation.color
                {
                    let current = current.clone();
                    return Ok((state, current, false));
                }
            }
            let revision = current_revision
                .checked_add(1)
                .filter(|value| *value <= crate::role_revision::MAX_JSON_SAFE_INTEGER)
                .ok_or(AgentManagementError::Conflict(
                    "project_presentation_revision_overflow",
                ))?;
            let extra = state
                .project_presentations
                .get(&request.project_id)
                .map(|settings| settings.extra.clone())
                .unwrap_or_default();
            let settings = ProjectPresentationSettings {
                display_name: request.presentation.display_name.trim().to_string(),
                badge_label: request.presentation.badge_label.trim().to_string(),
                color: request.presentation.color,
                revision,
                updated_at: now(),
                updated_by_director_session: None,
                updated_by_human_management: true,
                extra,
            };
            state
                .project_presentations
                .insert(request.project_id.clone(), settings.clone());
            Ok((state, settings, true))
        })
    }

    /// Grant or revoke an Operator as a distinct Human/Management action.
    /// Both project authority and the complete Operator set are CAS-fenced.
    pub fn execute_operator_action_for_management(
        &self,
        _principal: &HumanManagementPrincipal,
        request: &HumanManagementOperatorActionRequest,
    ) -> Result<HumanManagementOperatorReceipt, AgentManagementError> {
        let _execution = super::provider::provider_execution_lock()
            .lock()
            .map_err(|_| AgentManagementError::PersistenceUnavailable)?;
        let _mutation = self.store().lock_mutations()?;
        let digest = super::store::request_sha256(request)?;
        self.store().with_state(true, |mut state| {
            if let Some(record) = state
                .human_management_operator_actions
                .get(&request.action_id)
                .cloned()
            {
                return if record.request_sha256 == digest {
                    Ok((state, record.receipt, false))
                } else {
                    Err(AgentManagementError::Conflict("action_id_payload_conflict"))
                };
            }
            if state.actions.contains_key(&request.action_id)
                || state.authority_receipts.contains_key(&request.action_id)
                || state
                    .legacy_director_ownership_import_receipts
                    .contains_key(&request.action_id)
                || state
                    .reservation_reconciliation_receipts
                    .contains_key(&request.action_id)
            {
                return Err(AgentManagementError::Conflict("action_id_domain_conflict"));
            }
            let authority = state
                .projects
                .get(&request.project_id)
                .cloned()
                .ok_or(AgentManagementError::ProjectNotAuthorized)?;
            if authority.authority_epoch != request.expected_authority_epoch {
                return Err(AgentManagementError::Conflict("stale_project_authority"));
            }
            if request.operator_cutex_session_id == authority.authorized_director_session {
                return Err(AgentManagementError::Conflict(
                    "primary_director_cannot_be_operator",
                ));
            }
            let current_revision = management_operator_grant_revision(&state, &request.project_id);
            if current_revision != request.expected_grant_revision {
                return Err(AgentManagementError::Conflict(
                    "operator_grant_revision_conflict",
                ));
            }
            let revision = management_next_operator_grant_revision(current_revision)?;
            let committed_at = now();
            let grant = match request.operation {
                HumanManagementOperatorKind::Grant => {
                    let agent = state
                        .agents
                        .get(&request.operator_cutex_session_id)
                        .filter(|agent| {
                            agent.project_id == request.project_id && agent.retired_at.is_none()
                        })
                        .ok_or(AgentManagementError::Conflict(
                            "operator_must_be_active_managed_agent",
                        ))?;
                    if agent.cutex_session_id != request.operator_cutex_session_id {
                        return Err(AgentManagementError::InvalidStore);
                    }
                    if state
                        .operator_grants
                        .get(&request.project_id)
                        .is_some_and(|grants| {
                            grants.contains_key(&request.operator_cutex_session_id)
                        })
                    {
                        return Err(AgentManagementError::Conflict("operator_already_granted"));
                    }
                    let grant = super::AgentOperatorGrant {
                        project_id: request.project_id.clone(),
                        operator_cutex_session_id: request.operator_cutex_session_id.clone(),
                        grant_revision: revision,
                        granted_at: committed_at.clone(),
                        granted_by_primary_director_session: authority
                            .authorized_director_session
                            .clone(),
                        performed_by_human_management: true,
                    };
                    state
                        .operator_grants
                        .entry(request.project_id.clone())
                        .or_default()
                        .insert(request.operator_cutex_session_id.clone(), grant.clone());
                    Some(grant)
                }
                HumanManagementOperatorKind::Revoke => {
                    let removed = state
                        .operator_grants
                        .get_mut(&request.project_id)
                        .and_then(|grants| grants.remove(&request.operator_cutex_session_id))
                        .ok_or(AgentManagementError::Conflict("operator_not_granted"))?;
                    if removed.project_id != request.project_id
                        || removed.operator_cutex_session_id != request.operator_cutex_session_id
                    {
                        return Err(AgentManagementError::InvalidStore);
                    }
                    None
                }
            };
            state
                .operator_grant_revisions
                .insert(request.project_id.clone(), revision);
            let event_id = format!(
                "human-management:{}:operator-grant:{}",
                request.action_id, revision
            );
            let audit_event = super::AgentOperatorAuditEvent {
                event_id: event_id.clone(),
                action_id: request.action_id.clone(),
                project_id: request.project_id.clone(),
                operator_cutex_session_id: request.operator_cutex_session_id.clone(),
                kind: match request.operation {
                    HumanManagementOperatorKind::Grant => super::AgentOperatorAuditKind::Granted,
                    HumanManagementOperatorKind::Revoke => super::AgentOperatorAuditKind::Revoked,
                },
                previous_grant_revision: current_revision,
                grant_revision: revision,
                primary_director_cutex_session_id: authority.authorized_director_session.clone(),
                performed_by_human_management: true,
                committed_at: committed_at.clone(),
            };
            if state
                .operator_audit_events
                .insert(event_id, audit_event.clone())
                .is_some()
            {
                return Err(AgentManagementError::InvalidStore);
            }
            let roster = management_operator_roster(&state, &request.project_id);
            let receipt = HumanManagementOperatorReceipt {
                schema: request.schema,
                action_id: request.action_id.clone(),
                request_sha256: digest.clone(),
                operation: request.operation,
                project_id: request.project_id.clone(),
                authority_epoch: authority.authority_epoch,
                primary_director_cutex_session_id: authority.authorized_director_session,
                operator_cutex_session_id: request.operator_cutex_session_id.clone(),
                previous_grant_revision: current_revision,
                grant_revision: revision,
                grant,
                roster,
                audit_event,
                committed_at,
            };
            state.human_management_operator_actions.insert(
                request.action_id.clone(),
                HumanManagementOperatorActionRecord {
                    request_sha256: digest.clone(),
                    receipt: receipt.clone(),
                },
            );
            Ok((state, receipt, true))
        })
    }
}

fn management_operator_grant_revision(
    snapshot: &super::AgentManagementSnapshot,
    project_id: &ProjectId,
) -> u64 {
    snapshot
        .operator_grant_revisions
        .get(project_id)
        .copied()
        .unwrap_or(0)
}

fn management_next_operator_grant_revision(current: u64) -> Result<u64, AgentManagementError> {
    current
        .checked_add(1)
        .filter(|value| *value <= crate::role_revision::MAX_JSON_SAFE_INTEGER)
        .ok_or(AgentManagementError::Conflict(
            "operator_grant_revision_overflow",
        ))
}

fn management_operator_roster(
    snapshot: &super::AgentManagementSnapshot,
    project_id: &ProjectId,
) -> super::AgentOperatorRosterProjection {
    let mut operators = snapshot
        .operator_grants
        .get(project_id)
        .into_iter()
        .flat_map(|grants| grants.values().cloned())
        .collect::<Vec<_>>();
    operators.sort_by(|left, right| {
        left.operator_cutex_session_id
            .cmp(&right.operator_cutex_session_id)
    });
    super::AgentOperatorRosterProjection {
        grant_revision: management_operator_grant_revision(snapshot, project_id),
        operators,
    }
}

fn project_workspace(
    snapshot: &super::AgentManagementSnapshot,
    authority: &ProjectAuthority,
    access_role: ProjectAccessRole,
    observer: &dyn ProjectRuntimeObserver,
) -> Result<CutexProjectWorkspace, AgentManagementError> {
    let project_id = &authority.project_id;
    let mut active_agents = Vec::new();
    let mut retired_agents = Vec::new();
    let mut agent_operators = Vec::new();
    let mut director_member = None;
    for agent in snapshot
        .agents
        .values()
        .filter(|agent| &agent.project_id == project_id)
    {
        let member = project_member(agent.clone(), observer);
        if agent.cutex_session_id == authority.authorized_director_session {
            director_member = Some(member);
        } else if let Some(grant) = snapshot
            .operator_grants
            .get(project_id)
            .and_then(|grants| grants.get(&agent.cutex_session_id))
        {
            agent_operators.push(ProjectAgentOperatorProjection {
                grant: grant.clone(),
                member,
            });
        } else if agent.retired_at.is_some() {
            retired_agents.push(member);
        } else {
            active_agents.push(member);
        }
    }
    active_agents.sort_by(|left, right| {
        left.agent
            .cutex_session_id
            .cmp(&right.agent.cutex_session_id)
    });
    retired_agents.sort_by(|left, right| {
        left.agent
            .cutex_session_id
            .cmp(&right.agent.cutex_session_id)
    });
    agent_operators.sort_by(|left, right| {
        left.grant
            .operator_cutex_session_id
            .cmp(&right.grant.operator_cutex_session_id)
    });
    Ok(CutexProjectWorkspace {
        project_id: project_id.clone(),
        authority_epoch: authority.authority_epoch,
        director: ProjectDirectorProjection {
            cutex_session_id: authority.authorized_director_session.clone(),
            member: director_member,
        },
        access_role,
        operator_grant_revision: management_operator_grant_revision(snapshot, project_id),
        agent_operators,
        presentation: effective_presentation(
            project_id,
            snapshot.project_presentations.get(project_id),
        ),
        active_agents,
        retired_agents,
        legacy_operator_repair_candidates: legacy_operator_repair_candidates(snapshot, authority),
    })
}

fn legacy_operator_repair_candidates(
    snapshot: &super::AgentManagementSnapshot,
    authority: &ProjectAuthority,
) -> Vec<LegacyOperatorRepairCandidate> {
    let mut candidates = snapshot
        .phase_events
        .values()
        .filter(|event| {
            event.project_id == authority.project_id
                && event.operation == super::AgentOperationKind::DirectorRotate
                && event.phase == super::AgentActionPhase::Complete
                && matches!(
                    event.rotation_mode,
                    Some(
                        super::DirectorRotateMode::RetainPredecessorWithMessage
                            | super::DirectorRotateMode::RetainPredecessorBootstrapOnly
                    )
                )
                && event.successor_cutex_session_id.as_ref()
                    == Some(&authority.authorized_director_session)
                && event.authority_epoch == Some(authority.authority_epoch)
        })
        .filter_map(|event| {
            let predecessor = event.predecessor_cutex_session_id.as_ref()?;
            let successor = event.successor_cutex_session_id.as_ref()?;
            let rotation_mode = event.rotation_mode?;
            let active_owned = snapshot.agents.get(predecessor).is_some_and(|agent| {
                agent.project_id == authority.project_id && agent.retired_at.is_none()
            });
            let already_operator = snapshot
                .operator_grants
                .get(&authority.project_id)
                .is_some_and(|grants| grants.contains_key(predecessor));
            (active_owned && !already_operator).then(|| LegacyOperatorRepairCandidate {
                rotation_action_id: event.action_id.clone(),
                predecessor_cutex_session_id: predecessor.clone(),
                successor_cutex_session_id: successor.clone(),
                rotation_mode,
                completed_at: event.committed_at.clone(),
            })
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        left.completed_at
            .cmp(&right.completed_at)
            .then_with(|| left.rotation_action_id.cmp(&right.rotation_action_id))
    });
    candidates.dedup_by(|left, right| left.rotation_action_id == right.rotation_action_id);
    candidates
}

fn authorized_primary_authority<'a>(
    snapshot: &'a super::AgentManagementSnapshot,
    invocation: &AgentManagementInvocation,
    project_id: &ProjectId,
) -> Result<&'a ProjectAuthority, AgentManagementError> {
    let authority = snapshot
        .projects
        .get(project_id)
        .ok_or(AgentManagementError::ProjectNotAuthorized)?;
    if authority.authorized_director_session != invocation.caller_cutex_session {
        return Err(AgentManagementError::ProjectNotAuthorized);
    }
    Ok(authority)
}

fn authorized_project<'a>(
    snapshot: &'a super::AgentManagementSnapshot,
    invocation: &AgentManagementInvocation,
    project_id: &ProjectId,
) -> Result<(&'a ProjectAuthority, ProjectAccessRole), AgentManagementError> {
    let authority = snapshot
        .projects
        .get(project_id)
        .ok_or(AgentManagementError::ProjectNotAuthorized)?;
    let role = project_access_role(snapshot, invocation, project_id)?;
    Ok((authority, role))
}

fn project_access_role(
    snapshot: &super::AgentManagementSnapshot,
    invocation: &AgentManagementInvocation,
    project_id: &ProjectId,
) -> Result<ProjectAccessRole, AgentManagementError> {
    let authority = snapshot
        .projects
        .get(project_id)
        .ok_or(AgentManagementError::ProjectNotAuthorized)?;
    if authority.authorized_director_session == invocation.caller_cutex_session {
        return Ok(ProjectAccessRole::PrimaryDirector);
    }
    let has_grant = snapshot
        .operator_grants
        .get(project_id)
        .is_some_and(|grants| grants.contains_key(&invocation.caller_cutex_session));
    let active_owned = snapshot
        .agents
        .get(&invocation.caller_cutex_session)
        .is_some_and(|agent| &agent.project_id == project_id && agent.retired_at.is_none());
    if has_grant && active_owned {
        Ok(ProjectAccessRole::AgentOperator)
    } else {
        Err(AgentManagementError::ProjectNotAuthorized)
    }
}

fn summary(
    snapshot: &super::AgentManagementSnapshot,
    authority: &ProjectAuthority,
    access_role: ProjectAccessRole,
) -> CutexProjectSummary {
    let mut active_member_count = 0;
    let mut retired_member_count = 0;
    for agent in snapshot
        .agents
        .values()
        .filter(|agent| agent.project_id == authority.project_id)
    {
        if agent.cutex_session_id == authority.authorized_director_session {
            continue;
        }
        if snapshot
            .operator_grants
            .get(&authority.project_id)
            .is_some_and(|grants| grants.contains_key(&agent.cutex_session_id))
        {
            continue;
        }
        if agent.retired_at.is_some() {
            retired_member_count += 1;
        } else {
            active_member_count += 1;
        }
    }
    CutexProjectSummary {
        project_id: authority.project_id.clone(),
        authority_epoch: authority.authority_epoch,
        director_cutex_session_id: authority.authorized_director_session.clone(),
        access_role,
        operator_count: snapshot
            .operator_grants
            .get(&authority.project_id)
            .map_or(0, BTreeMap::len),
        presentation: effective_presentation(
            &authority.project_id,
            snapshot.project_presentations.get(&authority.project_id),
        ),
        active_member_count,
        retired_member_count,
    }
}

fn project_member(
    agent: ManagedAgentRecord,
    observer: &dyn ProjectRuntimeObserver,
) -> ProjectMemberProjection {
    match observer.observe(&agent.cutex_session_id) {
        Ok(runtime) => {
            let online = runtime.active
                && (!runtime.runtime_agent_ids.is_empty() || runtime.app_server_runtime);
            ProjectMemberProjection {
                agent,
                lifecycle: if online {
                    ProjectMemberLifecycle::Online
                } else {
                    ProjectMemberLifecycle::Offline
                },
                runtime: Some(runtime),
                observation_error: None,
            }
        }
        Err(error) => ProjectMemberProjection {
            agent,
            lifecycle: ProjectMemberLifecycle::Unavailable,
            runtime: None,
            observation_error: Some(error.to_string().chars().take(512).collect()),
        },
    }
}

pub fn effective_presentation(
    project_id: &ProjectId,
    stored: Option<&ProjectPresentationSettings>,
) -> EffectiveProjectPresentation {
    match stored {
        Some(settings) => EffectiveProjectPresentation {
            display_name: settings.display_name.clone(),
            badge_label: settings.badge_label.clone(),
            color: settings.color,
            revision: settings.revision,
            stored: true,
        },
        None => {
            let display_name = project_id.as_str().to_string();
            EffectiveProjectPresentation {
                badge_label: default_badge(&display_name),
                color: default_color(project_id),
                display_name,
                revision: 0,
                stored: false,
            }
        }
    }
}

fn default_badge(name: &str) -> String {
    let words = name
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    let badge = if words.len() >= 2 {
        words
            .iter()
            .take(2)
            .filter_map(|word| word.chars().next())
            .collect::<String>()
    } else {
        words
            .first()
            .copied()
            .unwrap_or("P")
            .chars()
            .take(2)
            .collect::<String>()
    };
    badge.to_ascii_uppercase()
}

fn default_color(project_id: &ProjectId) -> ProjectPaletteColor {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in project_id.as_str().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    ProjectPaletteColor::ALL[(hash % ProjectPaletteColor::ALL.len() as u64) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_management::{
        AgentActionId, AgentActionPhase, AgentManagementPhaseEvent, AgentOperationKind,
        DirectorRotateMode, ManagedAgentSpec, ProjectAuthority,
    };

    fn session(value: &str) -> CutexSessionId {
        CutexSessionId::new(value).unwrap()
    }

    fn project(value: &str) -> ProjectId {
        ProjectId::new(value).unwrap()
    }

    fn timestamp() -> Rfc3339 {
        Rfc3339::new("2026-09-03T00:00:00Z").unwrap()
    }

    fn invocation(value: &str) -> AgentManagementInvocation {
        AgentManagementInvocation {
            caller_cutex_session: session(value),
            caller_runtime_agent_id: format!("runtime-{value}"),
        }
    }

    fn agent(project_id: &ProjectId, value: &str, retired: bool) -> ManagedAgentRecord {
        ManagedAgentRecord {
            project_id: project_id.clone(),
            created_by_director_session: session("cutex.director"),
            created_by_operator_session: None,
            cutex_session_id: session(value),
            native_session_id: format!("native-{value}"),
            spec: ManagedAgentSpec {
                name: value.to_string(),
                cwd: format!("/tmp/{value}"),
                profile: "default".to_string(),
                runtime_backend: "app_server".to_string(),
                model: "gpt-test".to_string(),
                reasoning: "medium".to_string(),
                permissions: "default".to_string(),
                approval_policy: "never".to_string(),
                sandbox_mode: "workspace-write".to_string(),
                groups: vec!["workers".to_string()],
                expose_to_im: false,
                pin: false,
            },
            created_at: timestamp(),
            retired_at: retired.then(timestamp),
        }
    }

    fn provider_with_project() -> (AgentManagementProvider, std::path::PathBuf, ProjectId) {
        let root =
            std::env::temp_dir().join(format!("cutex-project-projection-{}", uuid::Uuid::new_v4()));
        let provider = AgentManagementProvider::open(&root).unwrap();
        let project_id = project("project-alpha");
        provider
            .store()
            .with_state(true, |mut state| {
                state.projects.insert(
                    project_id.clone(),
                    ProjectAuthority {
                        project_id: project_id.clone(),
                        authorized_director_session: session("cutex.director"),
                        authority_epoch: 7,
                        updated_at: timestamp(),
                    },
                );
                for record in [
                    agent(&project_id, "cutex.director", false),
                    agent(&project_id, "cutex.worker-online", false),
                    agent(&project_id, "cutex.worker-retired", true),
                ] {
                    state.agents.insert(record.cutex_session_id.clone(), record);
                }
                Ok((state, (), true))
            })
            .unwrap();
        (provider, root, project_id)
    }

    fn observation(id: &CutexSessionId) -> Result<AgentRuntimeObservation, AgentManagementError> {
        Ok(AgentRuntimeObservation {
            cutex_session_id: id.clone(),
            native_session_id: format!("native-{}", id.as_str()),
            active: true,
            cwd: "/tmp/exact".to_string(),
            profile: "default".to_string(),
            runtime_backend: "app_server".to_string(),
            model: "gpt-test".to_string(),
            reasoning: "medium".to_string(),
            permissions: "default".to_string(),
            approval_policy: "never".to_string(),
            sandbox_mode: "workspace-write".to_string(),
            groups: vec!["project:forged".to_string()],
            runtime_generation: 1,
            runtime_agent_ids: vec![format!("runtime-{}", id.as_str())],
            app_server_runtime: true,
            agent_bus_endpoint_ids: Vec::new(),
        })
    }

    #[test]
    fn defaults_are_stable_and_require_no_store_write() {
        let (provider, root, project_id) = provider_with_project();
        let before = provider.store().snapshot().unwrap().store_revision;
        let first = provider
            .list_cutex_projects(&invocation("cutex.director"))
            .unwrap();
        let second = provider
            .list_cutex_projects(&invocation("cutex.director"))
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(first[0].presentation.display_name, "project-alpha");
        assert_eq!(first[0].presentation.badge_label, "PA");
        assert!(!first[0].presentation.stored);
        assert_eq!(
            effective_presentation(&project("cutex-stack-main"), None).badge_label,
            "CS"
        );
        assert_eq!(provider.store().snapshot().unwrap().store_revision, before);
        std::fs::remove_dir_all(root).unwrap();
        let _ = project_id;
    }

    #[test]
    fn exact_owning_project_identity_controls_reads_not_groups_or_names() {
        let (provider, root, project_id) = provider_with_project();
        assert_eq!(
            provider
                .read_cutex_project(
                    &invocation("cutex.worker-online"),
                    &project_id,
                    &observation
                )
                .unwrap_err(),
            AgentManagementError::ProjectNotAuthorized
        );
        assert_eq!(
            provider
                .read_cutex_project(
                    &invocation("cutex.director"),
                    &project("project-lookalike"),
                    &observation,
                )
                .unwrap_err(),
            AgentManagementError::ProjectNotAuthorized
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn details_keep_director_active_and_retired_members_separate() {
        let (provider, root, project_id) = provider_with_project();
        let workspace = provider
            .read_cutex_project(&invocation("cutex.director"), &project_id, &observation)
            .unwrap();
        assert_eq!(workspace.project_id, project_id);
        assert_eq!(workspace.authority_epoch, 7);
        assert_eq!(
            workspace.director.member.unwrap().agent.cutex_session_id,
            session("cutex.director")
        );
        assert_eq!(workspace.active_agents.len(), 1);
        assert_eq!(workspace.retired_agents.len(), 1);
        assert_eq!(
            workspace.retired_agents[0].agent.cutex_session_id,
            session("cutex.worker-retired")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn presentation_validates_badge_and_authorized_director() {
        let (provider, root, project_id) = provider_with_project();
        let request = ProjectPresentationUpdateRequest {
            project_id: project_id.clone(),
            expected_presentation_revision: 0,
            presentation: ProjectPresentationInput {
                display_name: "Alpha Team".to_string(),
                badge_label: "AT".to_string(),
                color: ProjectPaletteColor::Green,
            },
        };
        assert_eq!(
            provider
                .update_project_presentation(&invocation("cutex.worker-online"), &request)
                .unwrap_err(),
            AgentManagementError::ProjectNotAuthorized
        );
        let saved = provider
            .update_project_presentation(&invocation("cutex.director"), &request)
            .unwrap();
        assert_eq!(saved.revision, 1);

        let mut invalid = request;
        invalid.expected_presentation_revision = 1;
        invalid.presentation.badge_label = "ABC".to_string();
        assert_eq!(
            provider.update_project_presentation(&invocation("cutex.director"), &invalid),
            Err(AgentManagementError::InvalidRequest(
                "invalid_project_badge_label"
            ))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn presentation_validation_enforces_bounded_name_and_terminal_cell_badge() {
        let valid = |display_name: &str, badge_label: &str| ProjectPresentationInput {
            display_name: display_name.to_string(),
            badge_label: badge_label.to_string(),
            color: ProjectPaletteColor::Magenta,
        };
        assert!(valid("Alpha", "A").validate().is_ok());
        assert!(valid("Alpha", "界").validate().is_ok());
        assert!(valid("Alpha", "CX").validate().is_ok());
        for invalid in [
            valid("", "A"),
            valid("bad\nname", "A"),
            valid(&"x".repeat(81), "A"),
            valid("Alpha", ""),
            valid("Alpha", "ABC"),
            valid("Alpha", "A B"),
            valid("Alpha", "界A"),
        ] {
            assert!(matches!(
                invalid.validate(),
                Err(AgentManagementError::InvalidRequest(_))
            ));
        }
    }

    #[test]
    fn management_project_reads_need_no_agent_invocation_or_environment_identity() {
        let (provider, root, project_id) = provider_with_project();
        let principal = HumanManagementPrincipal::authenticated();
        let collection = provider
            .list_cutex_projects_for_management(&principal)
            .unwrap();
        assert_eq!(collection.projects.len(), 1);
        assert_eq!(
            collection.projects[0].access_role,
            ProjectAccessRole::HumanManagement
        );
        let workspace = provider
            .read_cutex_project_for_management(&principal, &project_id, &observation)
            .unwrap();
        assert_eq!(workspace.project_id, project_id);
        assert_eq!(workspace.access_role, ProjectAccessRole::HumanManagement);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn management_presentation_write_fences_authority_and_records_human_actor() {
        let (provider, root, project_id) = provider_with_project();
        let principal = HumanManagementPrincipal::authenticated();
        let mut request = HumanManagementPresentationUpdateRequest {
            schema: crate::management::control_plane::HumanManagementPresentationSchema::V1,
            project_id: project_id.clone(),
            expected_authority_epoch: 6,
            expected_presentation_revision: 0,
            presentation: ProjectPresentationInput {
                display_name: "Control Plane".to_string(),
                badge_label: "CX".to_string(),
                color: ProjectPaletteColor::Magenta,
            },
        };
        assert_eq!(
            provider.update_project_presentation_for_management(&principal, &request),
            Err(AgentManagementError::Conflict("stale_project_authority"))
        );
        request.expected_authority_epoch = 7;
        let saved = provider
            .update_project_presentation_for_management(&principal, &request)
            .unwrap();
        assert!(saved.updated_by_human_management);
        assert!(saved.updated_by_director_session.is_none());
        assert_eq!(saved.badge_label, "CX");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn management_operator_grant_revoke_are_idempotent_cas_and_fully_audited() {
        let (provider, root, project_id) = provider_with_project();
        let principal = HumanManagementPrincipal::authenticated();
        let operator = session("cutex.worker-online");
        let grant = HumanManagementOperatorActionRequest {
            schema: crate::management::control_plane::HumanManagementOperatorSchema::V1,
            action_id: AgentActionId::new("human-grant-1").unwrap(),
            project_id: project_id.clone(),
            expected_authority_epoch: 7,
            expected_grant_revision: 0,
            operation: HumanManagementOperatorKind::Grant,
            operator_cutex_session_id: operator.clone(),
        };
        let receipt = provider
            .execute_operator_action_for_management(&principal, &grant)
            .unwrap();
        assert_eq!(receipt.grant_revision, 1);
        assert!(receipt.audit_event.performed_by_human_management);
        assert!(
            receipt
                .grant
                .as_ref()
                .unwrap()
                .performed_by_human_management
        );
        let after_grant = provider.store().snapshot().unwrap();
        assert_eq!(after_grant.operator_audit_events.len(), 1);
        assert_eq!(after_grant.human_management_operator_actions.len(), 1);
        let revision = after_grant.store_revision;
        assert_eq!(
            provider
                .execute_operator_action_for_management(&principal, &grant)
                .unwrap(),
            receipt
        );
        assert_eq!(
            provider.store().snapshot().unwrap().store_revision,
            revision
        );
        assert_eq!(
            provider.bind_project_authority(&crate::agent_management::ProjectAuthorityRequest {
                schema: crate::agent_management::AgentManagementSchema::V1,
                action_id: AgentActionId::new("human-grant-1").unwrap(),
                project_id: project_id.clone(),
                authorized_director_session: session("cutex.director"),
                expected_authorized_director_session: Some(session("cutex.director")),
                expected_authority_epoch: Some(7),
            }),
            Err(AgentManagementError::Conflict("action_id_domain_conflict"))
        );

        let stale = HumanManagementOperatorActionRequest {
            action_id: AgentActionId::new("human-grant-stale").unwrap(),
            ..grant.clone()
        };
        assert_eq!(
            provider.execute_operator_action_for_management(&principal, &stale),
            Err(AgentManagementError::Conflict(
                "operator_grant_revision_conflict"
            ))
        );
        let revoke = HumanManagementOperatorActionRequest {
            schema: crate::management::control_plane::HumanManagementOperatorSchema::V1,
            action_id: AgentActionId::new("human-revoke-1").unwrap(),
            project_id: project_id.clone(),
            expected_authority_epoch: 7,
            expected_grant_revision: 1,
            operation: HumanManagementOperatorKind::Revoke,
            operator_cutex_session_id: operator,
        };
        let revoked = provider
            .execute_operator_action_for_management(&principal, &revoke)
            .unwrap();
        assert_eq!(revoked.grant_revision, 2);
        assert!(revoked.grant.is_none());
        assert!(provider.store().snapshot().unwrap().operator_grants[&project_id].is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_retained_rotation_is_reviewable_but_never_auto_repaired() {
        let (provider, root, project_id) = provider_with_project();
        provider
            .store()
            .with_state(true, |mut state| {
                let action_id = AgentActionId::new("legacy-r11-rotation").unwrap();
                state.phase_events.insert(
                    "legacy-r11-rotation:complete".to_string(),
                    AgentManagementPhaseEvent {
                        event_id: "legacy-r11-rotation:complete".to_string(),
                        action_id,
                        project_id: project_id.clone(),
                        operation: AgentOperationKind::DirectorRotate,
                        phase: AgentActionPhase::Complete,
                        phase_sequence: 4,
                        committed_at: timestamp(),
                        presentation_owner_cutex_session_id: session("cutex.director"),
                        subject_cutex_session_id: None,
                        subject_agent_name: None,
                        predecessor_cutex_session_id: Some(session("cutex.worker-online")),
                        successor_cutex_session_id: Some(session("cutex.director")),
                        replace_policy: None,
                        rotation_mode: Some(DirectorRotateMode::RetainPredecessorWithMessage),
                        authority_epoch: Some(7),
                    },
                );
                Ok((state, (), true))
            })
            .unwrap();
        let before = provider.store().snapshot().unwrap().store_revision;
        let workspace = provider
            .read_cutex_project_for_management(
                &HumanManagementPrincipal::authenticated(),
                &project_id,
                &observation,
            )
            .unwrap();
        assert_eq!(workspace.legacy_operator_repair_candidates.len(), 1);
        let after = provider.store().snapshot().unwrap();
        assert!(!after.operator_grants.contains_key(&project_id));
        assert_eq!(after.store_revision, before);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn presentation_write_preserves_unknown_additive_fields() {
        let (provider, root, project_id) = provider_with_project();
        provider
            .store()
            .with_state(true, |mut state| {
                state
                    .extra
                    .insert("future_root".to_string(), serde_json::json!({"kept": true}));
                state.project_presentations.insert(
                    project_id.clone(),
                    ProjectPresentationSettings {
                        display_name: "Old".to_string(),
                        badge_label: "O".to_string(),
                        color: ProjectPaletteColor::Cyan,
                        revision: 3,
                        updated_at: timestamp(),
                        updated_by_director_session: Some(session("cutex.director")),
                        updated_by_human_management: false,
                        extra: BTreeMap::from([(
                            "future_setting".to_string(),
                            serde_json::json!([1, 2, 3]),
                        )]),
                    },
                );
                Ok((state, (), true))
            })
            .unwrap();
        serde_json::from_slice::<super::super::AgentManagementSnapshot>(
            &std::fs::read(root.join("agent-management-v1.json")).unwrap(),
        )
        .expect("snapshot with additive fields remains readable");
        provider
            .update_project_presentation(
                &invocation("cutex.director"),
                &ProjectPresentationUpdateRequest {
                    project_id: project_id.clone(),
                    expected_presentation_revision: 3,
                    presentation: ProjectPresentationInput {
                        display_name: "New".to_string(),
                        badge_label: "N".to_string(),
                        color: ProjectPaletteColor::Yellow,
                    },
                },
            )
            .unwrap();
        let snapshot = provider.store().snapshot().unwrap();
        assert_eq!(snapshot.extra["future_root"]["kept"], true);
        assert_eq!(
            snapshot.project_presentations[&project_id].extra["future_setting"],
            serde_json::json!([1, 2, 3])
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_provider_is_an_authorization_error_not_a_native_workspace_fallback() {
        let root =
            std::env::temp_dir().join(format!("cutex-project-empty-{}", uuid::Uuid::new_v4()));
        let provider = AgentManagementProvider::open(&root).unwrap();
        assert_eq!(
            provider.list_cutex_projects(&invocation("cutex.director")),
            Err(AgentManagementError::NotAuthorizedDirector)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operator_projection_is_distinct_and_cannot_mutate_presentation() {
        let (provider, root, project_id) = provider_with_project();
        let operator = session("cutex.worker-online");
        provider
            .store()
            .with_state(true, |mut state| {
                state
                    .operator_grants
                    .entry(project_id.clone())
                    .or_default()
                    .insert(
                        operator.clone(),
                        AgentOperatorGrant {
                            project_id: project_id.clone(),
                            operator_cutex_session_id: operator.clone(),
                            grant_revision: 1,
                            granted_at: timestamp(),
                            granted_by_primary_director_session: session("cutex.director"),
                            performed_by_human_management: false,
                        },
                    );
                state.operator_grant_revisions.insert(project_id.clone(), 1);
                Ok((state, (), true))
            })
            .unwrap();

        let summaries = provider
            .list_cutex_projects(&invocation(operator.as_str()))
            .unwrap();
        assert_eq!(summaries[0].access_role, ProjectAccessRole::AgentOperator);
        assert_eq!(summaries[0].operator_count, 1);
        let workspace = provider
            .read_cutex_project(&invocation(operator.as_str()), &project_id, &observation)
            .unwrap();
        assert_eq!(workspace.access_role, ProjectAccessRole::AgentOperator);
        assert_eq!(
            workspace.director.cutex_session_id,
            session("cutex.director")
        );
        assert_eq!(workspace.agent_operators.len(), 1);
        assert!(workspace.active_agents.is_empty());
        assert!(matches!(
            provider.update_project_presentation(
                &invocation(operator.as_str()),
                &ProjectPresentationUpdateRequest {
                    project_id: project_id.clone(),
                    expected_presentation_revision: 0,
                    presentation: ProjectPresentationInput {
                        display_name: "Forbidden".to_string(),
                        badge_label: "F".to_string(),
                        color: ProjectPaletteColor::Red,
                    },
                }
            ),
            Err(AgentManagementError::ProjectNotAuthorized)
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
}
