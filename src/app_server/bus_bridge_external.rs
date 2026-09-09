//! Private U+S6 consumer in the existing Bus worker. No retry permission,
//! secondary queue, or interpretation of model output as business completion.
use super::*;
use crate::agent_bus::model::AgentMessageKind;
use crate::app_server::external_input::{
    Delivery, DeliveryState, Envelope, ExternalInputClient, Message, Source, SourceKind,
};
use anyhow::ensure;

fn task_provider() -> anyhow::Result<crate::task_service::TaskServiceProvider> {
    Ok(crate::task_service::TaskServiceProvider::open(
        crate::task_delivery::provider_adapter::default_task_service_provider_root()?,
    )?)
}
fn watchdog() -> anyhow::Result<crate::task_service::TaskStaleWatchdog> {
    crate::task_service::TaskStaleWatchdog::open(
        crate::task_service::default_task_watchdog_root()?,
        crate::task_service::TaskWatchdogConfig::from_env()?,
    )
}

#[cfg(all(unix, feature = "stock-launch-test-hook"))]
fn before_commit_test_gate(message: &AgentBusMessage, stage: &str) -> anyhow::Result<()> {
    use std::io::{BufRead, Write};
    use std::os::unix::fs::MetadataExt;
    let configured = std::env::var("CUTEX_NATIVE_DELIVERY_TEST_STAGE")
        .unwrap_or_else(|_| "before_business_commit".into());
    if configured != stage {
        return Ok(());
    }
    let Ok(selected) = std::env::var("CUTEX_NATIVE_DELIVERY_TEST_MESSAGE") else {
        return Ok(());
    };
    if message.external_message_id.as_deref() != Some(selected.as_str()) {
        return Ok(());
    }
    let home = std::path::PathBuf::from(std::env::var("CUTEX_TEST_PRIVATE_HOME")?);
    ensure!(
        home.is_absolute() && home.join(".cutex-test-private-home").is_file(),
        "test gate requires private fixture home"
    );
    let path = std::path::PathBuf::from(std::env::var("CUTEX_NATIVE_DELIVERY_TEST_GATE")?);
    ensure!(
        path.starts_with(&home) && path.canonicalize()? == path,
        "test gate outside private home"
    );
    ensure!(
        std::fs::symlink_metadata(&path)?.uid() == unsafe { libc::geteuid() },
        "test gate wrong owner"
    );
    let mut stream = std::os::unix::net::UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    writeln!(stream, "{}", message.id)?;
    let mut response = String::new();
    std::io::BufReader::new(stream).read_line(&mut response)?;
    ensure!(
        response == "continue\n",
        "private fault gate interrupted before business commit"
    );
    Ok(())
}

pub(super) fn validate_target(
    message: &AgentBusMessage,
    owner: &str,
    seats: &crate::seat::SeatOccupancySnapshot,
    roster: &crate::agent_management::AgentManagementSnapshot,
) -> anyhow::Result<()> {
    ensure!(
        message.to_cutex_session_id.as_deref() == Some(owner),
        "external input recipient conflict"
    );
    if let Some(metadata) = task_service_completion_metadata(message)? {
        let snapshot = task_provider()?.query()?;
        let n = snapshot
            .completion_notifications
            .get(&metadata.notification_id)
            .context("completion notification absent")?;
        ensure!(
            n.project_id == metadata.project_id
                && n.assignment_id == metadata.assignment_id
                && n.task_id == metadata.task_id
                && n.task_revision == metadata.task_revision
                && n.kind == metadata.kind
                && n.target_seat_id == metadata.target_seat_id
                && n.transition_action_id == metadata.transition_action_id,
            "completion metadata conflict"
        );
        let target =
            crate::seat::task_seat_occupancy(seats, n.project_id.as_ref(), &n.target_seat_id)
                .context("completion seat missing/fenced")?;
        ensure!(
            target.occupant_cutex_session.as_str() == owner,
            "completion recipient rotated; retain pending for current Director"
        );
        if n.target_seat_id.as_str() == "cutex-director" {
            if let Some(project) = &n.project_id {
                let authority = roster
                    .projects
                    .get(project)
                    .context("project Director authority absent")?;
                ensure!(authority.authorized_director_session.as_str() == owner, "project Director authority/seat mismatch; coupled transfer must resolve before delivery");
            }
        }
    }
    if let Some(metadata) = task_service_metadata(message)? {
        DurableTaskServiceContextRecorder.validate_assignment(&metadata)?;
        let snapshot = task_provider()?.query()?;
        ensure!(
            snapshot
                .assignments
                .get(&metadata.assignment_id)
                .context("assignment absent")?
                .assignee_cutex_session
                .as_str()
                == owner,
            "assignment recipient conflict"
        );
    }
    if let Some(metadata) = task_service_worker_followup_metadata(message)? {
        DurableTaskServiceContextRecorder.validate_worker_followup(&metadata, owner)?;
    }
    if let Some(metadata) = task_service_watchdog_metadata(message)? {
        let n = watchdog()?
            .notification(&metadata.notification_id)?
            .context("watchdog absent")?;
        ensure!(
            crate::task_service::TaskWatchdogMessageMetadata::from(&n) == metadata,
            "watchdog metadata conflict"
        );
        let snapshot = task_provider()?.query()?;
        let assignment = snapshot
            .assignments
            .values()
            .find(|a| a.assignment_id.as_str() == metadata.assignment_id)
            .context("watchdog assignment absent")?;
        ensure!(
            assignment.project_id == metadata.project_id
                && assignment.state != crate::task_service::AssignmentState::Closed
                && assignment
                    .active_attempt
                    .is_some_and(|a| a.get() == metadata.attempt_number),
            "watchdog assignment/attempt no longer current"
        );
        match &n.target {
            crate::task_service::TaskWatchdogTarget::AssigneeSession(id) => ensure!(
                id == owner && assignment.assignee_cutex_session.as_str() == owner,
                "watchdog assignee conflict"
            ),
            crate::task_service::TaskWatchdogTarget::AuthoritySeat(id) => {
                let seat = crate::task_service::SeatId::new(id.clone())
                    .map_err(|_| anyhow::anyhow!("invalid watchdog seat"))?;
                let target = crate::seat::task_seat_occupancy(seats, n.project_id.as_ref(), &seat)
                    .context("watchdog seat missing/fenced")?;
                ensure!(
                    target.occupant_cutex_session.as_str() == owner,
                    "watchdog recipient rotated"
                );
                if id == "cutex-director" {
                    if let Some(project) = &n.project_id {
                        ensure!(
                            roster
                                .projects
                                .get(project)
                                .context("watchdog project missing")?
                                .authorized_director_session
                                .as_str()
                                == owner,
                            "watchdog project authority/seat mismatch"
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

/// Validate the authoritative outbox projection before freezing native bytes.
/// Existing historical envelopes are not synthesized from a new display name.
fn fresh_task_projection(
    message: &AgentBusMessage,
    owner: &str,
    activated_at: &str,
) -> anyhow::Result<()> {
    use crate::agent_bus::delivery::AgentDeliveryMode as Mode;
    ensure!(
        message.from == TASK_SERVICE_SYSTEM_SENDER && message.from_cutex_session_id.is_none(),
        "Task source conflict"
    );
    let snapshot = task_provider()?.query()?;
    let (family, external, created) = if let Some(m) = task_service_completion_metadata(message)? {
        let n = snapshot
            .completion_notifications
            .get(&m.notification_id)
            .context("completion absent")?;
        let mode = match n.delivery_mode {
            crate::task_service::CompletionNotificationDeliveryMode::AfterTurn => Mode::AfterTurn,
            crate::task_service::CompletionNotificationDeliveryMode::Soon => Mode::Soon,
        };
        ensure!(
            !n.is_delivered()
                && message.content == n.human_readable_content
                && message.delivery_mode == mode,
            "completion projection conflict"
        );
        (
            "tsc",
            n.notification_id.as_str().to_string(),
            n.created_at.as_str().to_string(),
        )
    } else if let Some(m) = task_service_metadata(message)? {
        let a = snapshot
            .assignments
            .get(&m.assignment_id)
            .context("assignment absent")?;
        let send = snapshot
            .send_attempts
            .get(&m.send_attempt_id)
            .context("send attempt absent")?;
        ensure!(
            message.delivery_mode == Mode::Soon
                && message.external_message_id.as_deref() == Some(&send.external_message_id),
            "assignment mode/message conflict"
        );
        crate::agent_bus::model::validate_task_service_assignment_summary(
            &message.content,
            m.require_valid_contract()?,
        )?;
        (
            "tsa",
            send.external_message_id.clone(),
            a.created_at.as_str().to_string(),
        )
    } else if let Some(m) = task_service_worker_followup_metadata(message)? {
        let n = snapshot
            .worker_followup_notifications
            .get(&m.notification_id)
            .context("follow-up absent")?;
        ensure!(
            !n.is_delivered()
                && message.content == n.decision_reference
                && message.delivery_mode == Mode::Soon,
            "follow-up projection conflict"
        );
        (
            "tsf",
            n.notification_id.as_str().to_string(),
            n.created_at.as_str().to_string(),
        )
    } else if let Some(m) = task_service_watchdog_metadata(message)? {
        let n = watchdog()?
            .notification(&m.notification_id)?
            .context("watchdog absent")?;
        let mode = match n.delivery_mode {
            crate::task_service::TaskWatchdogDeliveryMode::Soon => Mode::Soon,
            crate::task_service::TaskWatchdogDeliveryMode::AfterTurn => Mode::AfterTurn,
        };
        ensure!(
            !n.is_delivered() && message.content == n.content && message.delivery_mode == mode,
            "watchdog projection conflict"
        );
        let created = n
            .facts
            .first()
            .context("watchdog provenance absent")?
            .recorded_at
            .to_string();
        ("tsw", n.notification_id, created)
    } else {
        anyhow::bail!("unsupported Task canonical projection");
    };
    ensure!(
        chrono::DateTime::parse_from_rfc3339(&created)?
            >= chrono::DateTime::parse_from_rfc3339(activated_at)?,
        "pre-activation Task projection requires explicit review"
    );
    ensure!(
        message.id == crate::agent_bus::queue::native_task_message_id(family, &external, owner),
        "Task outbox message identity conflict"
    );
    Ok(())
}

fn envelope(
    message: &AgentBusMessage,
    binding: &crate::launch::stock::ExternalInputBinding,
) -> anyhow::Result<Envelope> {
    ensure!(
        message.kind == AgentBusEnvelopeKind::Message
            && message.to_cutex_session_id.as_deref() == Some(&binding.owner_id),
        "unsupported native business envelope"
    );
    let delivery = match message.delivery_mode {
        crate::agent_bus::delivery::AgentDeliveryMode::AfterTurn => Delivery::AfterTurn,
        crate::agent_bus::delivery::AgentDeliveryMode::Passive => Delivery::Passive,
        crate::agent_bus::delivery::AgentDeliveryMode::Soon => Delivery::Soon,
        _ => anyhow::bail!(
            "native ingress does not support interrupt; explicit sender decision required"
        ),
    };
    let (source, event_type, text) = match message.sender_kind {
        AgentMessageKind::Agent => {
            ensure!(
                message.control_type.is_none() && message.from != AGENT_MANAGEMENT_SYSTEM_SENDER,
                "management control ingress is not enabled"
            );
            let source = message
                .from_cutex_session_id
                .as_ref()
                .context("authenticated durable sender absent")?;
            crate::role_revision::CutexSessionId::new(source.clone())
                .map_err(|_| anyhow::anyhow!("invalid durable sender"))?;
            (
                Source {
                    kind: SourceKind::Agent,
                    id: source.clone(),
                },
                "message",
                format!("Message Type: MESSAGE\nPayload:\n{}", message.content),
            )
        }
        AgentMessageKind::TaskServiceSystem => {
            ensure!(
                message.from == TASK_SERVICE_SYSTEM_SENDER
                    && message.from_cutex_session_id.is_none(),
                "Task source conflict"
            );
            let text = if let Some(metadata) = task_service_completion_metadata(message)? {
                let snapshot = task_provider()?.query()?;
                snapshot
                    .completion_notifications
                    .get(&metadata.notification_id)
                    .context("completion absent")?
                    .human_readable_content
                    .clone()
            } else if let Some(metadata) = task_service_metadata(message)? {
                format!("Assignment ID: {}\nTask: {} revision {}\nAction: perform the assigned work using Task Service tools.\nContract:\n{}", metadata.assignment_id.as_str(), metadata.task_id.as_str(), metadata.task_revision.get(), metadata.require_valid_contract()?)
            } else if let Some(metadata) = task_service_worker_followup_metadata(message)? {
                format!("Assignment ID: {}\nTask: {} revision {}\nAction: address requested changes.\nDecision:\n{}", metadata.assignment_id.as_str(), metadata.task_id.as_str(), metadata.task_revision.get(), metadata.decision_reference)
            } else if let Some(metadata) = task_service_watchdog_metadata(message)? {
                watchdog()?
                    .notification(&metadata.notification_id)?
                    .context("watchdog absent")?
                    .content
            } else {
                anyhow::bail!("unsupported Task control ingress; retain pending");
            };
            (
                Source {
                    kind: SourceKind::Service,
                    id: TASK_SERVICE_SYSTEM_SENDER.into(),
                },
                "task_notification",
                text,
            )
        }
        AgentMessageKind::JobServiceSystem => {
            // Existing protected projection validates dedicated service provenance.
            job_service_inter_agent_params(&binding.thread_id, &binding.owner_id, None, message)?;
            let m: crate::agent_bus::model::JobServiceCompletionRequest = serde_json::from_value(
                message
                    .control_payload
                    .clone()
                    .context("job metadata absent")?,
            )?;
            let status = serde_json::to_value(&m.terminal_status)?
                .as_str()
                .context("job status shape")?
                .to_string();
            (
                Source {
                    kind: SourceKind::Service,
                    id: "cutex-job-service".into(),
                },
                "job_completion",
                format!(
                    "Job: {}\nResult: {}\nOutput reference: {}\nSummary (external data): {}",
                    m.job_id,
                    status,
                    m.output_reference.as_deref().unwrap_or("unavailable"),
                    m.summary.as_deref().unwrap_or("unavailable")
                ),
            )
        }
        _ => anyhow::bail!("Human/owner ingress not enabled through Agent Bus adapter"),
    };
    let mut e = Envelope {
        version: 1,
        owner_id: binding.owner_id.clone(),
        thread_id: binding.thread_id.clone(),
        runtime_generation: binding.runtime_generation,
        message: Message {
            id: message.id.clone(),
            source,
            event_type: event_type.into(),
            delivery,
            text,
        },
        semantic_sha256: String::new(),
    };
    e.semantic_sha256 = e.digest();
    e.validate()?;
    Ok(e)
}

pub(super) fn deliver(
    bus: &dyn RuntimeAgentBus,
    options: &AppServerAgentBusBridgeOptions,
    generation: u64,
    messages: Vec<AgentBusMessage>,
    status: &Arc<Mutex<AppServerAgentBusBridgeStatus>>,
    connection: &mut Option<ExternalInputClient>,
) -> anyhow::Result<DeliverySweepOutcome> {
    let mut outcome = DeliverySweepOutcome {
        had_messages: !messages.is_empty(),
        ..Default::default()
    };
    if let Some(client) = connection {
        if let Err(error) = client.drain_hints() {
            *connection = None;
            return Err(error);
        }
    }
    if messages.is_empty() {
        return Ok(outcome);
    }
    let path = crate::session::store::cutex_sessions_path()?;
    if connection.is_none() {
        *connection = Some(ExternalInputClient::connect(
            &path,
            &options.cutex_session_id,
            generation,
        )?);
    }
    let client = connection.as_ref().expect("connected");
    let repository = agent_bus_message_repository()?;
    for polled in messages {
        let result = (|| -> anyhow::Result<bool> {
            let before =
                crate::agent_management::AgentManagementStore::open_default()?.snapshot()?;
            let seats = crate::seat::SeatOccupancyStore::open_default()?;
            if repository.snapshot_by_message_id(&polled.id)?.is_none()
                && polled.sender_kind.is_task_service_system()
            {
                // Task notifications already have an authoritative outbox. Only
                // materialize its fresh authenticated projection into the existing
                // Bus repository; old/ambiguous K-era notifications require review.
                seats.with_notification_snapshot(|s| -> anyhow::Result<()> {
                    let store = crate::session::store::load_cutex_session_store_from_path(&path)?;
                    let r = store
                        .sessions
                        .get(&options.cutex_session_id)
                        .context("recipient absent")?;
                    r.app_server_runtime.as_ref().context("recipient offline")?;
                    let activation = store
                        .explicit_launch_receipts
                        .values()
                        .filter_map(|receipt| match receipt {
                            crate::agent_management::ExplicitLaunchActionReceipt::Activation(a)
                                if a.review.subject.cutex_session_id.as_str()
                                    == options.cutex_session_id
                                    && r.explicit_launch.as_ref() == Some(&a.review.contract) =>
                            {
                                Some(a)
                            }
                            _ => None,
                        })
                        .min_by_key(|a| &a.committed_at)
                        .context("native activation provenance absent")?;
                    // The explicit activation, not the latest runtime start,
                    // establishes this private lineage across owned restarts.
                    fresh_task_projection(
                        &polled,
                        &options.cutex_session_id,
                        &activation.committed_at,
                    )?;
                    let mut canonical = polled.clone();
                    canonical.to_cutex_session_id = Some(options.cutex_session_id.clone());
                    validate_target(&canonical, &options.cutex_session_id, s, &before)?;
                    let params = inter_agent_params(
                        &options.thread_id,
                        &options.cutex_session_id,
                        &options.cutex_session_id,
                        &canonical,
                    )?;
                    repository.record_queued(
                        crate::management::v2::agent_bus_state::AgentBusQueuedMessage {
                            owner_cutex_session_id: options.cutex_session_id.clone(),
                            message_id: canonical.id.clone(),
                            from_cutex_session_id: None,
                            to_cutex_session_id: options.cutex_session_id.clone(),
                            from_runtime_agent_id: None,
                            to_runtime_agent_id: Some(options.registration.id.clone()),
                            delivery_mode: canonical.delivery_mode.event_label().into(),
                            content: canonical.content.clone(),
                            queued_at: Utc::now(),
                            semantic_sha256: inter_agent_semantic_sha256(&params),
                            canonical_envelope: canonical,
                        },
                    )?;
                    Ok(())
                })??;
            }
            let message = repository.canonical_message(&polled.id)?;
            let recipient =
                crate::role_revision::CutexSessionId::new(options.cutex_session_id.clone())
                    .map_err(|_| anyhow::anyhow!("invalid durable recipient"))?;
            ensure!(
                before
                    .agents
                    .get(&recipient)
                    .is_none_or(|a| a.retired_at.is_none()),
                "recipient permanently retired"
            );
            seats.with_notification_snapshot(|s| {
                validate_target(&message, &options.cutex_session_id, s, &before)
            })??;
            let frozen =
                repository.freeze_external_input(&options.cutex_session_id, &message.id, |m| {
                    let e = envelope(m, client.binding())?;
                    client.require_delivery(&e.message.delivery)?;
                    Ok(e)
                })?;
            ensure!(
                frozen.owner_id == options.cutex_session_id
                    && frozen.thread_id == options.thread_id,
                "frozen native target changed; no retarget"
            );
            let mut current = frozen.clone();
            current.runtime_generation = generation; // occurrence is not semantic identity
            let mut observed = client.status(&[current.key()])?.statuses.remove(0);
            if observed.delivery_state == DeliveryState::Unknown {
                observed = client.submit(&current)?.statuses.remove(0);
            }
            repository.observe_external_input(&message.id, &frozen, &observed)?;
            let Some(receipt) = observed.receipt else {
                anyhow::bail!(
                    "native pending: {:?}/{:?}/{:?}",
                    observed.delivery_state,
                    observed.processing.state,
                    observed.processing.reason
                );
            };
            ensure!(
                observed.delivery_state == DeliveryState::ContextPersisted,
                "native receipt without A4"
            );
            #[cfg(all(unix, feature = "stock-launch-test-hook"))]
            before_commit_test_gate(&message, "before_business_commit")?;
            // Provider -> seat -> durable occurrence -> Task fact -> Bus CAS.
            // Lifecycle mutations cannot slip between the last fence and CAS.
            let management = crate::agent_management::AgentManagementStore::open_default()?;
            let _mutation = management
                .try_lock_delivery_mutations()?
                .context("lifecycle transition in progress; reconcile current occurrence")?;
            let roster = management.snapshot()?;
            let id = crate::role_revision::CutexSessionId::new(options.cutex_session_id.clone())
                .map_err(|_| anyhow::anyhow!("invalid durable recipient"))?;
            ensure!(
                roster
                    .agents
                    .get(&id)
                    .is_none_or(|a| a.retired_at.is_none()),
                "recipient permanently retired"
            );
            seats.with_notification_snapshot(|s| -> anyhow::Result<()> {
                validate_target(&message, &options.cutex_session_id, s, &roster)?;
                crate::session::store::with_locked_session_store(&path, |_| {
                    client.fence()?;
                    // Existing Task context-fact adapters retain their idempotent
                    // action references; no native-to-legacy receipt fabrication.
                    if let Some(m) = task_service_metadata(&message)? {
                        DurableTaskServiceContextRecorder.record_context_inserted(
                            &m,
                            &message.id,
                            &receipt.receipt_id,
                        )?;
                    }
                    if let Some(m) = task_service_completion_metadata(&message)? {
                        DurableTaskServiceContextRecorder.record_completion_context_inserted(
                            &m,
                            &message.id,
                            &receipt.receipt_id,
                        )?;
                        #[cfg(all(unix, feature = "stock-launch-test-hook"))]
                        before_commit_test_gate(&message, "after_task_fact")?;
                    }
                    if let Some(m) = task_service_worker_followup_metadata(&message)? {
                        DurableTaskServiceContextRecorder.record_worker_followup_context_inserted(
                            &m,
                            &options.cutex_session_id,
                            &message.id,
                            &receipt.receipt_id,
                        )?;
                    }
                    if let Some(m) = task_service_watchdog_metadata(&message)? {
                        DurableTaskServiceContextRecorder.record_watchdog_context_inserted(
                            &m,
                            &message.id,
                            &receipt.receipt_id,
                        )?;
                        DurableTaskServiceContextRecorder.record_watchdog_turn_binding(
                            &m,
                            &options.cutex_session_id,
                            &options.thread_id,
                            &receipt.turn_id,
                        )?;
                    }
                    repository.record_external_input_delivered(
                        &options.cutex_session_id,
                        &message.id,
                        &frozen,
                        &receipt,
                        generation,
                    )
                })
            })??;
            // No in-memory ACK retry shortcut: lost ACK reconciles durable A4
            // with current occurrence and target again in the next sweep.
            let count = bus.ack(&options.registration.id, &[message.id.clone()])?;
            mark_acknowledged(status, count, 0);
            Ok(true)
        })();
        match result {
            Ok(progress) => outcome.made_progress |= progress,
            Err(error) => {
                outcome.retained_pending = true;
                let _ = repository.record_external_input_error(&polled.id, &error.to_string());
                mark_error(
                    status,
                    format!("native delivery {} remains pending: {error:#}", polled.id),
                );
            }
        }
        if let Err(error) = client.drain_hints() {
            *connection = None;
            return Err(error);
        }
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn message() -> AgentBusMessage {
        serde_json::from_value(serde_json::json!({"id":"fixture-message","from":"Formal name", "to":"runtime-fixture", "fromCutexSessionId":"cutex.11111111-1111-4111-8111-111111111111", "toCutexSessionId":"cutex.22222222-2222-4222-8222-222222222222", "content":"Hello 私有", "deliveryMode":"after_turn", "triggerTurn":true, "createdAtEpochSecs":1})).unwrap()
    }
    fn binding() -> crate::launch::stock::ExternalInputBinding {
        crate::launch::stock::ExternalInputBinding {
            version: 1,
            owner_id: "cutex.22222222-2222-4222-8222-222222222222".into(),
            thread_id: "22222222-2222-4222-8222-222222222222".into(),
            runtime_generation: 7,
            canonical_byte_limit: Default::default(),
        }
    }
    #[test]
    fn external_bus_template_uses_authenticated_id_not_name_or_claimed_text() {
        let mut m = message();
        let original = envelope(&m, &binding()).unwrap();
        m.from = "Title changed".into();
        assert_eq!(original, envelope(&m, &binding()).unwrap());
        assert_eq!(original.message.source.id, m.from_cutex_session_id.unwrap());
        assert!(!original.message.text.contains("runtime-fixture"));
        assert!(!original.message.text.contains("fixture-message"));
        assert!(original.message.text.contains("Hello 私有"));
    }
    #[test]
    fn external_bus_unsupported_modes_roles_and_missing_identity_fail_closed() {
        let mut soon = message();
        let after = envelope(&soon, &binding()).unwrap();
        soon.delivery_mode = crate::agent_bus::delivery::AgentDeliveryMode::Soon;
        let immediate = envelope(&soon, &binding()).unwrap();
        assert_eq!(after.message.text, immediate.message.text);
        assert_eq!(immediate.message.delivery, Delivery::Soon);
        assert_ne!(after.semantic_sha256, immediate.semantic_sha256);
        for mode in [crate::agent_bus::delivery::AgentDeliveryMode::Interrupt] {
            let mut m = message();
            m.delivery_mode = mode;
            assert!(envelope(&m, &binding()).is_err());
        }
        for role in [
            AgentMessageKind::User,
            AgentMessageKind::Owner,
            AgentMessageKind::TaskServiceSystem,
            AgentMessageKind::JobServiceSystem,
        ] {
            let mut m = message();
            m.sender_kind = role;
            assert!(envelope(&m, &binding()).is_err());
        }
        let mut m = message();
        m.from_cutex_session_id = None;
        assert!(envelope(&m, &binding()).is_err());
        let mut m = message();
        m.to_cutex_session_id = None;
        assert!(envelope(&m, &binding()).is_err());
        let mut m = message();
        m.control_type = Some("system".into());
        assert!(envelope(&m, &binding()).is_err());
        let mut m = message();
        m.delivery_mode = crate::agent_bus::delivery::AgentDeliveryMode::Passive;
        assert_eq!(
            envelope(&m, &binding()).unwrap().message.delivery,
            Delivery::Passive
        );
    }
    #[test]
    fn external_bus_keeps_utf8_transport_cap_and_generation_out_of_digest() {
        let mut m = message();
        m.content = "界".repeat(22000);
        assert!(envelope(&m, &binding()).is_err());
        let a = envelope(&message(), &binding()).unwrap();
        let mut b = binding();
        b.runtime_generation += 1;
        assert_eq!(
            a.semantic_sha256,
            envelope(&message(), &b).unwrap().semantic_sha256
        );
    }
}
