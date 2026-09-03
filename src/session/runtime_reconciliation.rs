//! Live runtime-agent reconciliation for durable `cutex_session` records.

use crate::agent_bus::model::AgentBusAgent;
use crate::agent_bus::model::AgentRegistrationClass;
use crate::session::identity::normalize_codex_session_id;
use crate::session::metadata::cutex_session_key_for_codex_session;
use crate::session::metadata::cutex_session_key_for_codex_session_including_retired;
use crate::session::model::CutexSessionReconcileEvent;
use crate::session::model::CutexSessionReconcileOutcome;
use crate::session::model::CutexSessionRecord;
use crate::session::model::CutexSessionRuntimeBackend;
use crate::session::model::CutexSessionStore;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutexSessionRegistrationReconcile {
    pub outcome: Option<CutexSessionReconcileOutcome>,
    pub store_fence_required: bool,
}

pub fn reconcile_cutex_session_store_for_registration(
    store: &mut CutexSessionStore,
    agent: &AgentBusAgent,
    host_id: &str,
    timestamp: &str,
) -> anyhow::Result<CutexSessionRegistrationReconcile> {
    let target = registration_store_target(store, agent)?;
    let store_fence_required = !matches!(target, RegistrationStoreTarget::Unmanaged);
    let outcome = match target {
        RegistrationStoreTarget::NativeSession => {
            reconcile_cutex_session_store_from_agent(store, agent, host_id, timestamp)?
        }
        RegistrationStoreTarget::RuntimeIdentity(key) => {
            let record = store.sessions.get_mut(&key).ok_or_else(|| {
                anyhow::anyhow!("registration session disappeared during reconciliation: {key}")
            })?;
            if record.current_runtime_agent_id.as_deref() != Some(agent.id.as_str()) {
                if let Some(previous) = record.current_runtime_agent_id.replace(agent.id.clone()) {
                    record.last_runtime_agent_id = Some(previous);
                }
                record.runtime_generation = record.runtime_generation.saturating_add(1);
            }
            record.last_seen_at = Some(timestamp.to_string());
            record.updated_at = timestamp.to_string();
            None
        }
        RegistrationStoreTarget::Unmanaged => None,
    };
    debug_assert!(store_fence_required || outcome.is_none());
    Ok(CutexSessionRegistrationReconcile {
        outcome,
        store_fence_required,
    })
}

enum RegistrationStoreTarget {
    NativeSession,
    RuntimeIdentity(String),
    Unmanaged,
}

fn registration_store_target(
    store: &CutexSessionStore,
    agent: &AgentBusAgent,
) -> anyhow::Result<RegistrationStoreTarget> {
    let codex_session_id = agent
        .session_id
        .as_deref()
        .map(normalize_codex_session_id)
        .transpose()?;

    if let Some(codex_session_id) = codex_session_id.as_deref() {
        if let Some(record) = store.sessions.values().find(|record| {
            record.is_retired() && record.codex_session_id.as_deref() == Some(codex_session_id)
        }) {
            anyhow::bail!(
                "agent registration targets retired cutex session: {}",
                record.cutex_session_id
            );
        }
        return Ok(RegistrationStoreTarget::NativeSession);
    }

    let mut active_runtime_match = None;
    for (key, record) in store.sessions.iter().filter(|(_, record)| {
        record.current_runtime_agent_id.as_deref() == Some(agent.id.as_str())
            || record.last_runtime_agent_id.as_deref() == Some(agent.id.as_str())
    }) {
        if record.is_retired() {
            anyhow::bail!(
                "agent registration targets retired cutex session: {}",
                record.cutex_session_id
            );
        }
        if active_runtime_match.replace(key.clone()).is_some() {
            anyhow::bail!(
                "agent registration runtime identity is ambiguous across cutex sessions: {}",
                agent.id
            );
        }
    }
    Ok(active_runtime_match
        .map(RegistrationStoreTarget::RuntimeIdentity)
        .unwrap_or(RegistrationStoreTarget::Unmanaged))
}

pub fn reconcile_cutex_session_store_from_agent(
    store: &mut CutexSessionStore,
    agent: &AgentBusAgent,
    host_id: &str,
    timestamp: &str,
) -> anyhow::Result<Option<CutexSessionReconcileOutcome>> {
    let Some(codex_session_id) = agent.session_id.as_deref() else {
        return Ok(None);
    };
    let codex_session_id = normalize_codex_session_id(codex_session_id)?;
    if let Some(key) =
        cutex_session_key_for_codex_session_including_retired(store, &codex_session_id)
    {
        if store
            .sessions
            .get(&key)
            .is_some_and(|record| record.is_retired())
        {
            return Ok(None);
        }
    }
    let session_key = cutex_session_key_for_codex_session(store, &codex_session_id);
    let mut events = Vec::new();

    let stale_keys = store
        .sessions
        .iter()
        .filter_map(|(key, record)| {
            (key != &session_key
                && record.is_active()
                && record.current_runtime_agent_id.as_deref() == Some(agent.id.as_str()))
            .then(|| key.clone())
        })
        .collect::<Vec<_>>();

    for stale_key in stale_keys {
        if let Some(record) = store.sessions.get_mut(&stale_key) {
            let previous_runtime_agent_id = record.current_runtime_agent_id.take();
            record.last_runtime_agent_id = previous_runtime_agent_id.clone();
            record.runtime_generation = record.runtime_generation.saturating_add(1);
            record.updated_at = timestamp.to_string();
            events.push(CutexSessionReconcileEvent {
                event_type: "cutex_session_rebound",
                summary: format!(
                    "Runtime endpoint {} moved from {} to {}",
                    agent.id,
                    record
                        .codex_session_id
                        .as_deref()
                        .unwrap_or(stale_key.as_str()),
                    codex_session_id
                ),
                previous_runtime_agent_id,
                runtime_agent_id: Some(agent.id.clone()),
                previous_cutex_session_id: Some(stale_key),
            });
        }
    }

    let created = !store.sessions.contains_key(&session_key);
    if created {
        let mut record = CutexSessionRecord::new_at(
            session_key.clone(),
            Some(codex_session_id.clone()),
            host_id.to_string(),
            agent.cwd.clone(),
            None,
            timestamp.to_string(),
        )?;
        record.runtime_generation = 1;
        store.sessions.insert(session_key.clone(), record);
    }

    let record = store.sessions.get_mut(&session_key).ok_or_else(|| {
        anyhow::anyhow!("cutex session disappeared during reconciliation: {session_key}")
    })?;
    let previous_runtime_agent_id = record.current_runtime_agent_id.clone();
    let endpoint_changed = previous_runtime_agent_id.as_deref() != Some(agent.id.as_str());
    if endpoint_changed {
        record.last_runtime_agent_id = previous_runtime_agent_id.clone();
        record.current_runtime_agent_id = Some(agent.id.clone());
        if !created {
            record.runtime_generation = record.runtime_generation.saturating_add(1);
        }
        let first_runtime_endpoint = previous_runtime_agent_id.is_none();
        events.push(CutexSessionReconcileEvent {
            event_type: if created || first_runtime_endpoint {
                "runtime_endpoint_registered"
            } else {
                "runtime_endpoint_changed"
            },
            summary: if created || first_runtime_endpoint {
                format!(
                    "Runtime endpoint {} registered for session {}",
                    agent.id, codex_session_id
                )
            } else {
                format!(
                    "Runtime endpoint for session {} changed to {}",
                    codex_session_id, agent.id
                )
            },
            previous_runtime_agent_id,
            runtime_agent_id: Some(agent.id.clone()),
            previous_cutex_session_id: None,
        });
    }

    let mut durable_changed = false;
    durable_changed |=
        replace_if_changed(&mut record.codex_session_id, Some(codex_session_id.clone()));
    if let Some(thread_name) = agent.thread_name.clone() {
        durable_changed |= replace_if_changed(&mut record.thread_name, Some(thread_name));
    }
    // A persistent session's display-name hint is its stable managed identity.
    // The runtime thread name is a mutable conversation title (often generated
    // from the first user message), so it must not replace an existing managed
    // name. Unmanaged sessions still follow their runtime title, and legacy
    // persistent records without a display hint may be initialized here.
    if record.registration_class != AgentRegistrationClass::Persistent
        || record.display_name_hint.is_none()
    {
        durable_changed |= replace_if_changed(
            &mut record.display_name_hint,
            agent
                .thread_name
                .clone()
                .or_else(|| agent.base_name.clone())
                .or_else(|| Some(agent.name.clone())),
        );
    }
    durable_changed |= replace_if_changed(&mut record.host_id, host_id.to_string());
    if record.managed_cwd.is_none() {
        durable_changed |= replace_if_changed(&mut record.cwd, agent.cwd.clone());
    }
    durable_changed |= replace_if_changed(&mut record.agent_enabled, true);
    durable_changed |= replace_if_changed(&mut record.agent_groups, agent.groups.clone());
    let registration_class =
        reconcile_registration_class(record.registration_class, agent.registration_class);
    durable_changed |= replace_if_changed(&mut record.registration_class, registration_class);
    if durable_changed && !created {
        record.bump_durable_revision()?;
    }
    record.last_seen_at = Some(timestamp.to_string());
    record.updated_at = timestamp.to_string();

    Ok(Some(CutexSessionReconcileOutcome {
        cutex_session_id: session_key,
        codex_session_id,
        events,
    }))
}

fn replace_if_changed<T: PartialEq>(slot: &mut T, value: T) -> bool {
    if *slot == value {
        false
    } else {
        *slot = value;
        true
    }
}

pub fn apply_session_online_runtime_observation(
    store: &mut CutexSessionStore,
    key: &str,
    live_agent: Option<&AgentBusAgent>,
    alden_session_name: Option<&str>,
    backend: CutexSessionRuntimeBackend,
    pid: u32,
    host_id: &str,
    timestamp: &str,
) -> anyhow::Result<Option<CutexSessionReconcileOutcome>> {
    let reconcile_outcome = if let Some(agent) = live_agent {
        reconcile_cutex_session_store_from_agent(store, agent, host_id, timestamp)?
    } else {
        None
    };
    let record = store.sessions.get_mut(key).ok_or_else(|| {
        anyhow::anyhow!("cutex session disappeared while recording runtime: {key}")
    })?;
    if record.is_retired() {
        anyhow::bail!("cutex session is retired: {key}");
    }
    record.runtime_backend = backend;
    record.alden_session_name = alden_session_name.map(str::to_string);
    record.alden_pid = (backend == CutexSessionRuntimeBackend::CuteAlden).then_some(pid);
    record.runtime_pid = live_agent
        .map(|agent| agent.pid)
        .or((backend == CutexSessionRuntimeBackend::Host).then_some(pid))
        .or(record.runtime_pid);
    record.updated_at = timestamp.to_string();
    Ok(reconcile_outcome)
}

pub fn clear_cutex_session_runtime_record(
    store: &mut CutexSessionStore,
    key: &str,
    preserve_alden_name: bool,
) -> anyhow::Result<()> {
    let Some(record) = store.sessions.get_mut(key) else {
        return Ok(());
    };
    if !preserve_alden_name {
        record.alden_session_name = None;
    }
    record.pending_launch_id = None;
    record.app_server_launch_claim_id = None;
    record.alden_pid = None;
    record.runtime_pid = None;
    record.app_server_runtime = None;
    record.current_runtime_agent_id = None;
    record.updated_at = chrono::Utc::now().to_rfc3339();
    Ok(())
}

fn reconcile_registration_class(
    existing: AgentRegistrationClass,
    observed: AgentRegistrationClass,
) -> AgentRegistrationClass {
    match (existing, observed) {
        (AgentRegistrationClass::Persistent, _) => AgentRegistrationClass::Persistent,
        (_, observed) => observed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live_agent(session_id: &str, thread_name: &str) -> AgentBusAgent {
        AgentBusAgent {
            id: format!("runtime-{session_id}"),
            name: format!("runtime-{session_id}"),
            base_name: Some("runtime".to_string()),
            thread_name: Some(thread_name.to_string()),
            path_key: None,
            session_id: Some(session_id.to_string()),
            cutex_session_id: None,
            profile: "default".to_string(),
            cwd: "/tmp".to_string(),
            pid: 42,
            host_id: Some("host".to_string()),
            groups: Vec::new(),
            registration_class: AgentRegistrationClass::Persistent,
            last_seen_epoch_secs: 1,
        }
    }

    #[test]
    fn persistent_session_keeps_managed_display_name_when_runtime_title_changes() {
        let mut store = CutexSessionStore::default();
        let mut record = CutexSessionRecord::new_at(
            "cutex.thread-managed-name".to_string(),
            Some("thread-managed-name".to_string()),
            "host".to_string(),
            "/tmp".to_string(),
            None,
            "2026-08-10T00:00:00Z".to_string(),
        )
        .expect("record");
        record.thread_name = Some("managed-worker-r2".to_string());
        record.display_name_hint = Some("managed-worker-r2".to_string());
        record.registration_class = AgentRegistrationClass::Persistent;
        store
            .sessions
            .insert("cutex.thread-managed-name".to_string(), record);

        reconcile_cutex_session_store_from_agent(
            &mut store,
            &live_agent("thread-managed-name", "generated task title"),
            "host",
            "2026-08-10T00:01:00Z",
        )
        .expect("runtime reconciliation");

        let record = store
            .sessions
            .get("cutex.thread-managed-name")
            .expect("managed record");
        assert_eq!(record.thread_name.as_deref(), Some("generated task title"));
        assert_eq!(
            record.display_name_hint.as_deref(),
            Some("managed-worker-r2")
        );
    }

    #[test]
    fn persistent_session_without_display_name_is_initialized_from_runtime() {
        let mut store = CutexSessionStore::default();
        let mut record = CutexSessionRecord::new_at(
            "cutex.thread-missing-name".to_string(),
            Some("thread-missing-name".to_string()),
            "host".to_string(),
            "/tmp".to_string(),
            None,
            "2026-08-10T00:00:00Z".to_string(),
        )
        .expect("record");
        record.registration_class = AgentRegistrationClass::Persistent;
        store
            .sessions
            .insert("cutex.thread-missing-name".to_string(), record);

        reconcile_cutex_session_store_from_agent(
            &mut store,
            &live_agent("thread-missing-name", "runtime title"),
            "host",
            "2026-08-10T00:01:00Z",
        )
        .expect("runtime reconciliation");

        let record = store
            .sessions
            .get("cutex.thread-missing-name")
            .expect("persistent record");
        assert_eq!(record.display_name_hint.as_deref(), Some("runtime title"));
    }

    #[test]
    fn registration_rejects_retired_identity_while_observation_remains_noop() {
        let mut store = CutexSessionStore::default();
        let mut record = CutexSessionRecord::new_at(
            "cutex.retired-registration".to_string(),
            Some("thread-retired-registration".to_string()),
            "host".to_string(),
            "/tmp".to_string(),
            None,
            "2026-08-10T00:00:00Z".to_string(),
        )
        .expect("record");
        record.archive_state = crate::session::model::CutexSessionArchiveState::Retired;
        record.retired_at = Some("2026-08-10T00:01:00Z".to_string());
        store
            .sessions
            .insert("cutex.retired-registration".to_string(), record);
        let agent = AgentBusAgent {
            id: "runtime-retired-registration".to_string(),
            name: "runtime-retired-registration".to_string(),
            base_name: None,
            thread_name: None,
            path_key: None,
            session_id: Some("thread-retired-registration".to_string()),
            cutex_session_id: None,
            profile: "default".to_string(),
            cwd: "/tmp".to_string(),
            pid: 42,
            host_id: Some("host".to_string()),
            groups: Vec::new(),
            registration_class: AgentRegistrationClass::Persistent,
            last_seen_epoch_secs: 1,
        };

        assert!(reconcile_cutex_session_store_from_agent(
            &mut store,
            &agent,
            "host",
            "2026-08-10T00:02:00Z"
        )
        .expect("refresh reconciliation")
        .is_none());
        let error = reconcile_cutex_session_store_for_registration(
            &mut store,
            &agent,
            "host",
            "2026-08-10T00:02:00Z",
        )
        .expect_err("registration must reject retired identity");
        assert!(error.to_string().contains("retired cutex session"));
    }

    #[test]
    fn sessionless_registration_fences_only_matching_durable_runtime_identity() {
        let mut store = CutexSessionStore::default();
        let mut record = CutexSessionRecord::new_at(
            "cutex.sessionless-registration".to_string(),
            Some("thread-sessionless-registration".to_string()),
            "host".to_string(),
            "/tmp".to_string(),
            None,
            "2026-08-10T00:00:00Z".to_string(),
        )
        .expect("record");
        record.last_runtime_agent_id = Some("runtime-sessionless-registration".to_string());
        store
            .sessions
            .insert("cutex.sessionless-registration".to_string(), record);
        let mut agent = AgentBusAgent {
            id: "runtime-sessionless-registration".to_string(),
            name: "runtime-sessionless-registration".to_string(),
            base_name: None,
            thread_name: None,
            path_key: None,
            session_id: None,
            cutex_session_id: None,
            profile: "default".to_string(),
            cwd: "/tmp".to_string(),
            pid: 42,
            host_id: Some("host".to_string()),
            groups: Vec::new(),
            registration_class: AgentRegistrationClass::Persistent,
            last_seen_epoch_secs: 1,
        };

        let matched = reconcile_cutex_session_store_for_registration(
            &mut store,
            &agent,
            "host",
            "2026-08-10T00:02:00Z",
        )
        .expect("matching sessionless registration");
        assert!(matched.store_fence_required);
        assert!(matched.outcome.is_none());
        let claimed = store
            .sessions
            .get("cutex.sessionless-registration")
            .expect("claimed sessionless record");
        assert_eq!(
            claimed.current_runtime_agent_id.as_deref(),
            Some("runtime-sessionless-registration")
        );
        assert_eq!(claimed.runtime_generation, 1);

        agent.id = "unmanaged-sessionless-registration".to_string();
        let unmatched = reconcile_cutex_session_store_for_registration(
            &mut store,
            &agent,
            "host",
            "2026-08-10T00:03:00Z",
        )
        .expect("unmatched sessionless registration");
        assert!(!unmatched.store_fence_required);
        assert!(unmatched.outcome.is_none());
    }
}
