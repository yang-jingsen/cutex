//! Registration projection from committed reviewed ownership, never a caller flag.
use crate::agent_bus::groups::normalize_registered_agent_groups;
use crate::agent_bus::model::{AgentBusAgent, AgentRegistrationClass};
use crate::agent_management::{ExplicitLaunchActionReceipt, StockRuntimeStage};
use crate::session::model::{CutexSessionRecord, CutexSessionStore};

/// Called only behind the authenticated service registration route, before its
/// store CAS and roster publication. The caller must additionally verify the
/// returned exact native process/bundle before committing. Ordinary registration
/// remains unchanged. A pending launch pins its reviewed revision. A Ready
/// owner can re-register after durable configuration edits, provided its exact
/// process, binding, runtime identity, and current group projection still match.
pub fn preserve_reviewed_groups(
    store: &CutexSessionStore,
    agent: &mut AgentBusAgent,
    host: &str,
) -> anyhow::Result<Option<CutexSessionRecord>> {
    let targets: Vec<_> = store
        .sessions
        .values()
        .filter(|r| {
            r.explicit_launch.is_some()
                && (r.codex_session_id.as_deref() == agent.session_id.as_deref()
                    || r.current_runtime_agent_id.as_deref() == Some(agent.id.as_str())
                    || store.explicit_launch_receipts.values().any(|v| {
                        matches!(v,
                ExplicitLaunchActionReceipt::Runtime(receipt)
                if receipt.review.subject.cutex_session_id.as_str() == r.cutex_session_id
                    && receipt.runtime_agent_id == agent.id)
                    }))
        })
        .collect();
    if targets.is_empty() {
        return Ok(None);
    }
    anyhow::ensure!(
        targets.len() == 1,
        "reviewed registration mapping ambiguous"
    );
    let record = targets[0];
    anyhow::ensure!(
        store.sessions.get(&record.cutex_session_id) == Some(record)
            && store
                .sessions
                .values()
                .filter(|r| r.codex_session_id == record.codex_session_id)
                .count()
                == 1,
        "reviewed registration native mapping ambiguous"
    );
    let receipts: Vec<_> = store
        .explicit_launch_receipts
        .values()
        .filter_map(|v| match v {
            ExplicitLaunchActionReceipt::Runtime(r)
                if r.runtime_agent_id == agent.id
                    && r.review.subject.cutex_session_id.as_str() == record.cutex_session_id =>
            {
                Some(r)
            }
            _ => None,
        })
        .collect();
    anyhow::ensure!(
        receipts.len() == 1,
        "reviewed registration receipt missing or ambiguous"
    );
    let receipt = receipts[0];
    let binding = receipt
        .binding
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("reviewed registration binding missing"))?;
    anyhow::ensure!(
        !record.is_retired()
            && record.agent_enabled
            && record.registration_class == AgentRegistrationClass::Persistent
            && agent.registration_class == AgentRegistrationClass::Persistent
            && record.host_id == host
            && agent.host_id.as_deref() == Some(host)
            && record.codex_session_id.as_deref()
                == Some(receipt.review.contract.native_id.as_str())
            && agent.session_id == record.codex_session_id
            && record.explicit_launch.as_ref() == Some(&receipt.review.contract)
            && (receipt.stage == StockRuntimeStage::Ready
                || record.revision == receipt.review.subject.revision)
            && record.app_server_runtime.as_ref() == Some(binding)
            && record.runtime_pid == Some(binding.pid)
            && agent.pid == binding.pid
            && agent.cwd == crate::session::service::cutex_session_launch_cwd(record)
            && agent.profile == binding.launched_profile.as_deref().unwrap_or("-"),
        "reviewed registration owner/configuration mismatch"
    );
    let same_occurrence = record.runtime_generation == receipt.expected_generation
        && record.current_runtime_agent_id.as_deref() == Some(agent.id.as_str());
    let first_registration = record.runtime_generation == receipt.review.subject.runtime_generation
        && receipt.expected_generation == record.runtime_generation.checked_add(1).unwrap_or(0)
        && record.current_runtime_agent_id.is_none();
    anyhow::ensure!(
        match receipt.stage {
            StockRuntimeStage::Spawned =>
                record.app_server_launch_claim_id.as_deref() == Some(receipt.claim_id.as_str())
                    && (first_registration || same_occurrence),
            StockRuntimeStage::Ready =>
                record.app_server_launch_claim_id.is_none() && same_occurrence,
            _ => false,
        },
        "reviewed registration claim/generation mismatch"
    );
    // Both existing producer and receiver normalize convenience groups. Accept
    // only that exact compatibility projection, not arbitrary submitted groups.
    let expected = normalize_registered_agent_groups(
        record.agent_groups.clone(),
        agent.path_key.as_deref(),
        &agent.cwd,
    );
    anyhow::ensure!(
        agent.groups == expected,
        "reviewed registration requested groups changed"
    );
    agent.groups = record.agent_groups.clone();
    Ok(Some(record.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_management::StockRuntimeReceipt;
    use serde_json::json;

    fn fixture() -> (CutexSessionStore, AgentBusAgent) {
        let native = "019f4b34-82e6-7f72-9027-34df7bdcb82e";
        let mut record = CutexSessionRecord::new(
            "cutex.test".into(),
            Some(native.into()),
            "private".into(),
            "/private".into(),
            None,
        )
        .unwrap();
        record.agent_enabled = true;
        record.registration_class = AgentRegistrationClass::Persistent;
        record.agent_groups = vec!["original-z".into(), "original-a".into()];
        record.display_name_hint = Some("formal".into());
        let receipt: StockRuntimeReceipt = serde_json::from_value(json!({
            "action_id":"reviewed-register", "stage":"spawned", "claim_id":"claim", "runtime_agent_id":"stock.test", "expected_generation":1,
            "publication":null,"error":null,"updated_at":"2026-01-01T00:00:00Z",
            "binding":{"transport":"unix_socket","endpoint":"unix:///private/sock","pid":1234,"runtime_dir":"/private","launched_profile":"alpha","diagnostic_journal_path":"/private/journal","schema_version":"test","schema_sha256":"e".repeat(64),"started_at":"2026-01-01T00:00:00Z"},
            "review":{
                "subject":{"cutex_session_id":record.cutex_session_id,"formal_name":"formal","durable_sha256":"a".repeat(64),"authority_sha256":"a".repeat(64),"current_project_id":null,"revision":record.revision,"runtime_generation":0},
                "contract":{"version":2,"native_id":native,"native_home":"/private","bundle_manifest":"/private/manifest","bundle_sha256":"b".repeat(64)},
                "configuration":{"profile_name":"alpha","profile_id":"private-profile","inherited":false,"profile_sha256":"c".repeat(64),"account_sha256":"d".repeat(64),"model":"private-model","reasoning":null,"model_provider":"private","provider":{"name":"private","base_url":"http://127.0.0.1:1/v1","wire_api":"responses","requires_openai_auth":false,"supports_websockets":false},"sandbox":"read-only","approval":"on-request"},
                "restart":false
            }
        })).unwrap();
        record.explicit_launch = Some(receipt.review.contract.clone());
        record.app_server_launch_claim_id = Some(receipt.claim_id.clone());
        record.runtime_pid = Some(1234);
        record.app_server_runtime = receipt.binding.clone();
        let agent = AgentBusAgent {
            id: "stock.test".into(),
            name: "formal".into(),
            base_name: Some("formal".into()),
            thread_name: None,
            path_key: None,
            session_id: Some(native.into()),
            cutex_session_id: None,
            profile: "alpha".into(),
            cwd: "/private".into(),
            pid: 1234,
            host_id: Some("private".into()),
            groups: normalize_registered_agent_groups(
                record.agent_groups.clone(),
                None,
                "/private",
            ),
            registration_class: AgentRegistrationClass::Persistent,
            last_seen_epoch_secs: 1,
        };
        let mut store = CutexSessionStore::default();
        store
            .sessions
            .insert(record.cutex_session_id.clone(), record);
        store.explicit_launch_receipts.insert(
            receipt.action_id.to_string(),
            ExplicitLaunchActionReceipt::Runtime(receipt),
        );
        (store, agent)
    }

    #[test]
    fn reviewed_registration_preserves_order_and_revision_and_ready_replay() {
        let (mut store, mut agent) = fixture();
        let before = store.sessions["cutex.test"].clone();
        preserve_reviewed_groups(&store, &mut agent, "private")
            .unwrap()
            .unwrap();
        assert_eq!(agent.groups, before.agent_groups);
        crate::session::runtime_reconciliation::reconcile_cutex_session_store_for_registration(
            &mut store,
            &agent,
            "private",
            "2026-01-02T00:00:00Z",
        )
        .unwrap();
        assert_eq!(store.sessions["cutex.test"].revision, before.revision);
        assert_eq!(store.sessions["cutex.test"].runtime_generation, 1);
        // Same normalized request, after the authoritative Ready transition.
        let r = store.sessions.get_mut("cutex.test").unwrap();
        r.app_server_launch_claim_id = None;
        if let ExplicitLaunchActionReceipt::Runtime(receipt) = store
            .explicit_launch_receipts
            .get_mut("reviewed-register")
            .unwrap()
        {
            receipt.stage = StockRuntimeStage::Ready;
        }
        agent.groups = normalize_registered_agent_groups(agent.groups, None, "/private");
        preserve_reviewed_groups(&store, &mut agent, "private")
            .unwrap()
            .unwrap();
        assert_eq!(agent.groups, before.agent_groups);
    }

    #[test]
    fn ready_owner_reconnect_survives_configuration_revision_but_not_owner_change() {
        let (mut store, mut agent) = fixture();
        let record = store.sessions.get_mut("cutex.test").unwrap();
        record.app_server_launch_claim_id = None;
        record.runtime_generation = 1;
        record.current_runtime_agent_id = Some(agent.id.clone());
        record.revision += 2;
        if let ExplicitLaunchActionReceipt::Runtime(receipt) = store
            .explicit_launch_receipts
            .get_mut("reviewed-register")
            .unwrap()
        {
            receipt.stage = StockRuntimeStage::Ready;
        }
        preserve_reviewed_groups(&store, &mut agent, "private").unwrap();
        agent.groups = normalize_registered_agent_groups(agent.groups, None, "/private");
        store
            .sessions
            .get_mut("cutex.test")
            .unwrap()
            .runtime_generation += 1;
        assert!(preserve_reviewed_groups(&store, &mut agent, "private").is_err());
    }

    #[test]
    fn reviewed_registration_rejects_forged_owner_group_and_stale_state() {
        for case in 0..10 {
            let (mut store, mut agent) = fixture();
            let record = store.sessions.get_mut("cutex.test").unwrap();
            match case {
                0 => agent.groups.push("privileged".into()),
                1 => agent.pid += 1,
                2 => agent.id = "foreign".into(),
                3 => agent.session_id = Some(uuid::Uuid::new_v4().to_string()),
                4 => record.runtime_generation = 9,
                5 => record.app_server_launch_claim_id = Some("foreign".into()),
                6 => record.revision += 1,
                7 => agent.host_id = Some("foreign".into()),
                8 => record.agent_groups.push("intentional-change".into()),
                9 => agent.cwd = "/foreign".into(),
                _ => unreachable!(),
            }
            assert!(
                preserve_reviewed_groups(&store, &mut agent, "private").is_err(),
                "case {case}"
            );
        }
    }

    #[test]
    fn reviewed_registration_does_not_change_unmarked_defaults() {
        let (mut store, mut agent) = fixture();
        store
            .sessions
            .get_mut("cutex.test")
            .unwrap()
            .explicit_launch = None;
        let before = agent.groups.clone();
        assert!(preserve_reviewed_groups(&store, &mut agent, "private")
            .unwrap()
            .is_none());
        assert_eq!(agent.groups, before);
        assert!(before.len() > 2);
    }
}
