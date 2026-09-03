//! Provider-authoritative Cutex Project read model and presentation settings.
//!
//! A Cutex Project is an Agent Management ownership boundary. Its identity is
//! only the canonical [`ProjectId`] stored by the provider. Display metadata is
//! deliberately non-authoritative and can never select or grant authority.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthStr;

use crate::role_revision::{CutexSessionId, Rfc3339};

use super::{
    now, AgentManagementError, AgentManagementInvocation, AgentManagementProvider,
    AgentRuntimeObservation, ManagedAgentRecord, ProjectAuthority, ProjectId,
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
    pub updated_by_director_session: CutexSessionId,
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
pub struct CutexProjectSummary {
    pub project_id: ProjectId,
    pub authority_epoch: u64,
    pub director_cutex_session_id: CutexSessionId,
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
    pub presentation: EffectiveProjectPresentation,
    pub active_agents: Vec<ProjectMemberProjection>,
    pub retired_agents: Vec<ProjectMemberProjection>,
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
            .filter(|authority| {
                authority.authorized_director_session == invocation.caller_cutex_session
            })
            .map(|authority| summary(&snapshot, authority))
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
        let authority = authorized_authority(&snapshot, invocation, project_id)?;
        let mut active_agents = Vec::new();
        let mut retired_agents = Vec::new();
        let mut director_member = None;
        for agent in snapshot
            .agents
            .values()
            .filter(|agent| &agent.project_id == project_id)
        {
            let member = project_member(agent.clone(), observer);
            if agent.cutex_session_id == authority.authorized_director_session {
                director_member = Some(member);
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
        Ok(CutexProjectWorkspace {
            project_id: project_id.clone(),
            authority_epoch: authority.authority_epoch,
            director: ProjectDirectorProjection {
                cutex_session_id: authority.authorized_director_session.clone(),
                member: director_member,
            },
            presentation: effective_presentation(
                project_id,
                snapshot.project_presentations.get(project_id),
            ),
            active_agents,
            retired_agents,
        })
    }

    pub fn update_project_presentation(
        &self,
        invocation: &AgentManagementInvocation,
        request: &ProjectPresentationUpdateRequest,
    ) -> Result<ProjectPresentationSettings, AgentManagementError> {
        request.presentation.validate()?;
        self.store().with_state(true, |mut state| {
            authorized_authority(&state, invocation, &request.project_id)?;
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
                updated_by_director_session: invocation.caller_cutex_session.clone(),
                extra,
            };
            state
                .project_presentations
                .insert(request.project_id.clone(), settings.clone());
            Ok((state, settings, true))
        })
    }
}

fn authorized_authority<'a>(
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

fn summary(
    snapshot: &super::AgentManagementSnapshot,
    authority: &ProjectAuthority,
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
    use crate::agent_management::{ManagedAgentSpec, ProjectAuthority};

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
        for invalid in [
            valid("", "A"),
            valid("bad\nname", "A"),
            valid(&"x".repeat(81), "A"),
            valid("Alpha", ""),
            valid("Alpha", "ABC"),
            valid("Alpha", "A B"),
        ] {
            assert!(matches!(
                invalid.validate(),
                Err(AgentManagementError::InvalidRequest(_))
            ));
        }
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
                        updated_by_director_session: session("cutex.director"),
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
}
