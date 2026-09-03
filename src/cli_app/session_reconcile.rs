use chrono::Utc;

use cutex::agent_bus::model::AgentBusAgent;
use cutex::im::registry::ImRegistry;
use cutex::management::v2::model::CutexMessage;
use cutex::management::v2::model::EventCorrelation;
use cutex::management::v2::model::EventSource;
use cutex::management::v2::model::PendingEvent;
use cutex::management::v2::repository::management_v2_repository;
use cutex::platform::host::current_host_name;
use cutex::session::model::CutexSessionReconcileOutcome;
use cutex::session::runtime_reconciliation::reconcile_cutex_session_store_for_registration;
use cutex::session::service::{
    reconcile_cutex_session_store_from_agent, reconcile_cutex_session_store_from_im_registration,
};
use cutex::session::store::{load_cutex_session_store, save_cutex_session_store};

pub(crate) fn reconcile_cutex_session_from_agent(agent: &AgentBusAgent) -> anyhow::Result<()> {
    let timestamp = Utc::now().to_rfc3339();
    let host_id = current_host_name();
    let mut store = load_cutex_session_store()?;
    let Some(outcome) =
        reconcile_cutex_session_store_from_agent(&mut store, agent, &host_id, &timestamp)?
    else {
        return Ok(());
    };
    save_cutex_session_store(&store)?;
    append_cutex_session_reconcile_events(&outcome, agent, &timestamp)?;
    Ok(())
}

pub(crate) fn reconcile_cutex_session_registration(agent: &AgentBusAgent) -> anyhow::Result<()> {
    let timestamp = Utc::now().to_rfc3339();
    let host_id = current_host_name();
    let mut store = load_cutex_session_store()?;
    let reconciliation =
        reconcile_cutex_session_store_for_registration(&mut store, agent, &host_id, &timestamp)?;
    if !reconciliation.store_fence_required {
        return Ok(());
    }
    save_cutex_session_store(&store)?;
    if let Some(outcome) = reconciliation.outcome {
        append_cutex_session_reconcile_events(&outcome, agent, &timestamp)?;
    }
    Ok(())
}

pub(crate) fn mirror_im_registry_into_cutex_session_store(
    registry: &ImRegistry,
) -> anyhow::Result<()> {
    if registry.sessions.is_empty() {
        return Ok(());
    }
    let timestamp = Utc::now().to_rfc3339();
    let mut store = load_cutex_session_store()?;
    let mut changed = false;
    for entry in registry.sessions.values() {
        changed |=
            reconcile_cutex_session_store_from_im_registration(&mut store, entry, &timestamp)?;
    }
    if changed {
        save_cutex_session_store(&store)?;
    }
    Ok(())
}

pub(crate) fn append_cutex_session_reconcile_events(
    outcome: &CutexSessionReconcileOutcome,
    _agent: &AgentBusAgent,
    timestamp: &str,
) -> anyhow::Result<()> {
    if outcome.events.is_empty() {
        return Ok(());
    }
    let store = load_cutex_session_store()?;
    let record = store
        .sessions
        .values()
        .find(|record| record.cutex_session_id == outcome.cutex_session_id)
        .ok_or_else(|| anyhow::anyhow!("reconciled cutex session disappeared"))?;
    let app_server_connected = super::app_server_runtime::runtime_manager()
        .status(&record.cutex_session_id)?
        .is_some_and(|status| status.connected);
    management_v2_repository()?.append(PendingEvent {
        cutex_session_id: record.cutex_session_id.clone(),
        host_id: current_host_name(),
        source: EventSource::Cutex,
        schema: None,
        correlation: EventCorrelation {
            thread_id: record.codex_session_id.clone(),
            ..Default::default()
        },
        native: None,
        cutex: Some(CutexMessage {
            method: "cutex/runtime/endpointChanged".to_string(),
            params: serde_json::json!({
                "runtimeGeneration": record.runtime_generation,
                "appServerConnected": app_server_connected,
                "changedAt": timestamp,
            }),
        }),
    })?;
    Ok(())
}
