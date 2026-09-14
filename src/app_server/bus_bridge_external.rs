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
    // Presentation generation-race probes must allow the real reviewed launch
    // path to finish artifact validation before reaching its claim. Test-only;
    // production RPC/backoff timeouts are unchanged.
    let timeout = if stage == "after_presentation_append" {
        240
    } else {
        30
    };
    stream.set_read_timeout(Some(Duration::from_secs(timeout)))?;
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
    if message.from == AGENT_MANAGEMENT_SYSTEM_SENDER {
        let metadata = agent_management_metadata(message)?;
        let mut actions = roster
            .actions
            .values()
            .filter(|a| a.external_message_id.as_ref() == message.external_message_id.as_ref());
        let action = actions.next().context("Management start action absent")?;
        ensure!(
            actions.next().is_none(),
            "ambiguous Management start action"
        );
        let intent = roster
            .bootstrap_intents
            .get(&action.action_id)
            .context("reviewed Management start intent absent")?;
        ensure!(
            action
                .known_successor_cutex_session
                .as_ref()
                .map(|s| s.as_str())
                == Some(owner)
                && metadata.requested_by_director == intent.director
                && metadata.requested_by_operator.is_none()
                && action.caller_cutex_session == intent.director,
            "Management start recipient/source conflict"
        );
    }
    if let Some(metadata) = task_service_completion_metadata(message)? {
        let n = task_provider()?
            .completion_notification(&metadata.notification_id)?
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
        ensure!(
            task_provider()?
                .assignment_record(&metadata.assignment_id)?
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
        let assignment = task_provider()?
            .assignment_record(&crate::task_service::AssignmentId::new(
                metadata.assignment_id.clone(),
            )?)?
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

fn native_task_lineage<'a>(
    store: &'a crate::session::model::CutexSessionStore,
    r: &'a crate::session::model::CutexSessionRecord,
    bootstrap_intents: &std::collections::BTreeMap<
        crate::agent_management::AgentActionId,
        crate::agent_management::BootstrapIntentReview,
    >,
    owner: &str,
) -> anyhow::Result<&'a str> {
    store
        .explicit_launch_receipts
        .values()
        .filter_map(|receipt| match receipt {
            crate::agent_management::ExplicitLaunchActionReceipt::Activation(a)
                if a.review.subject.cutex_session_id.as_str() == owner
                    && r.explicit_launch.as_ref() == Some(&a.review.contract) =>
            {
                Some(a.committed_at.as_str())
            }
            crate::agent_management::ExplicitLaunchActionReceipt::Bootstrap(a)
                if a.cutex_session_id.as_str() == owner
                    && r.codex_session_id.as_deref() == Some(a.native_id.as_str())
                    && bootstrap_intents.values().any(|intent| {
                        use sha2::{Digest, Sha256};
                        serde_json::to_vec(intent).is_ok_and(|bytes| {
                            format!("{:x}", Sha256::digest(bytes)) == a.intent_sha256.as_str()
                                && r.explicit_launch.as_ref().is_some_and(|c| {
                                    c.native_id == a.native_id
                                        && c.native_home == intent.native_home
                                        && c.bundle_manifest == intent.bundle_manifest
                                        && c.bundle_sha256 == intent.bundle_sha256
                                })
                        })
                    }) =>
            {
                // S8a creates the new durable record, marker and
                // receipt in ONE save. Its immutable creation
                // timestamp is that lineage, not a later launch.
                Some(r.created_at.as_str())
            }
            crate::agent_management::ExplicitLaunchActionReceipt::Runtime(a)
                if a.stage == crate::agent_management::StockRuntimeStage::Ready
                    && a.review.subject.cutex_session_id.as_str() == owner
                    && r.explicit_launch.as_ref().is_some_and(|contract| {
                        contract.native_id == a.review.contract.native_id
                            && contract.native_home == a.review.contract.native_home
                    })
                    && r.codex_session_id.as_deref()
                        == Some(a.review.contract.native_id.as_str()) =>
            {
                // A normal typed/human launch establishes native ownership
                // without an Activation or Bootstrap migration receipt.
                // Use creation, not restart time: queued assignments survive
                // a later runtime restart.
                Some(r.created_at.as_str())
            }
            _ => None,
        })
        .min()
        .context("native activation provenance absent")
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
    let provider = task_provider()?;
    let (family, external, created) = if let Some(m) = task_service_completion_metadata(message)? {
        let n = provider
            .completion_notification(&m.notification_id)?
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
        let a = provider
            .assignment_record(&m.assignment_id)?
            .context("assignment absent")?;
        let send = provider
            .send_attempt(&m.send_attempt_id)?
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
        let n = provider
            .worker_followup_notification(&m.notification_id)?
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

pub(super) fn envelope(
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
        AgentMessageKind::Agent if message.from == AGENT_MANAGEMENT_SYSTEM_SENDER => {
            // Reuse the reserved in-process Management provenance validator;
            // ordinary send cannot manufacture this control record.
            let metadata = agent_management_metadata(message)?;
            (Source { kind: SourceKind::Service, id: AGENT_MANAGEMENT_SYSTEM_SENDER.into() },
             "management_start",
             format!("Requested by Director: {}\nAction: follow the explicit start instructions.\nInstructions:\n{}", metadata.requested_by_director.as_str(), message.content))
        }
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
                task_provider()?
                    .completion_notification(&metadata.notification_id)?
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
            if message.control_type.as_deref() == Some(crate::agent_bus::job_completion::SCHEMA_V2)
            {
                let frozen =
                    crate::agent_bus::job_completion::FrozenProjection::from_message(message)?;
                let mut e = Envelope {
                    version: frozen.native_version,
                    owner_id: binding.owner_id.clone(),
                    thread_id: binding.thread_id.clone(),
                    runtime_generation: binding.runtime_generation,
                    message: Message {
                        id: message.id.clone(),
                        source: Source {
                            kind: SourceKind::Service,
                            id: "cutex-job-service".into(),
                        },
                        event_type: "job_completion".into(),
                        delivery,
                        text: frozen.model_text,
                    },
                    view: Some(frozen.view),
                    semantic_sha256: String::new(),
                };
                e.semantic_sha256 = e.digest();
                e.validate()?;
                return Ok(e);
            }
            let m: crate::agent_bus::model::JobServiceCompletionRequest = serde_json::from_value(
                message
                    .control_payload
                    .clone()
                    .context("job metadata absent")?,
            )?;
            (
                Source {
                    kind: SourceKind::Service,
                    id: "cutex-job-service".into(),
                },
                "job_completion",
                legacy_job_model_text(&m)?,
            )
        }
        _ => anyhow::bail!("Human/owner ingress not enabled through Agent Bus adapter"),
    };
    let mut e = Envelope {
        version: 1,
        view: None,
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

// Display facts are frozen with the envelope; routing and model input keep the
// authenticated durable sender. A missing local name never blocks delivery.
fn agent_message_view(e: &mut Envelope, name: Option<&str>, mode: &str) -> anyhow::Result<()> {
    if e.message.source.kind != SourceKind::Agent || e.message.event_type != "message" {
        return Ok(());
    }
    e.version = 2;
    e.view = Some(crate::app_server::external_input::view::StructuredView {
        schema: "cutex.agent-message.v1".into(),
        data: serde_json::json!({"senderId":e.message.source.id,
            "senderName":name.map(|name| name.chars().take(160).collect::<String>()),
            "deliveryMode":mode}),
    });
    e.semantic_sha256 = e.digest();
    e.validate()
}

// Compatibility projection for v1 canonical messages, including pending ones.
// New completion versions must never change these bytes on recovery.
fn legacy_job_model_text(
    m: &crate::agent_bus::model::JobServiceCompletionRequest,
) -> anyhow::Result<String> {
    let status = serde_json::to_value(&m.terminal_status)?;
    Ok(format!(
        "Job: {}\nResult: {}\nOutput reference: {}\nSummary (external data): {}",
        m.job_id,
        status.as_str().context("job status shape")?,
        m.output_reference.as_deref().unwrap_or("unavailable"),
        m.summary.as_deref().unwrap_or("unavailable")
    ))
}

#[cfg(test)]
mod job_projection_compatibility_tests {
    use super::*;

    #[test]
    fn legacy_pending_projection_preserves_exact_text_and_optional_values() {
        let mut request: crate::agent_bus::model::JobServiceCompletionRequest =
            serde_json::from_value(serde_json::json!({
                "schema": "cutex.job_service.completion.v1",
                "eventId": "event-1", "jobId": "job-1", "jobRevision": 1,
                "terminalStatus": "exited", "resultSha256": "a".repeat(64),
                "targetCutexSessionId": "cutex.11111111-1111-4111-8111-111111111111"
            }))
            .unwrap();
        assert_eq!(legacy_job_model_text(&request).unwrap(),
            "Job: job-1\nResult: exited\nOutput reference: unavailable\nSummary (external data): unavailable");
        request.output_reference = Some("job-output:job-1".into());
        request.summary = Some("Job job-1 reached terminal state exited".into());
        let persisted = serde_json::to_vec(&request).unwrap();
        let reloaded = serde_json::from_slice(&persisted).unwrap();
        assert_eq!(legacy_job_model_text(&reloaded).unwrap(),
            "Job: job-1\nResult: exited\nOutput reference: job-output:job-1\nSummary (external data): Job job-1 reached terminal state exited");
        request.summary = Some(String::new());
        assert!(legacy_job_model_text(&request)
            .unwrap()
            .ends_with("Summary (external data): "));
    }
}

/// Independent display recovery runs after input ACK, including on empty polls.
/// It never submits input, retries a held turn, or changes business delivery.
pub(super) fn deliver_presentations(
    options: &AppServerAgentBusBridgeOptions,
    generation: u64,
) -> anyhow::Result<()> {
    use crate::app_server::presentation::PresentationClient;
    let repository = agent_bus_message_repository()?;
    let now = Utc::now().timestamp();
    let ids = repository.due_presentations(&options.cutex_session_id, now)?;
    if ids.is_empty() {
        return Ok(());
    }
    let path = crate::session::store::cutex_sessions_path()?;
    let mut client = None;
    for id in ids {
        if repository
            .begin_presentation_attempt(&options.cutex_session_id, &id, now)
            .is_err()
        {
            continue;
        }
        let result = (|| -> anyhow::Result<()> {
            let message = repository.canonical_message(&id)?;
            let management = crate::agent_management::AgentManagementStore::open_default()?;
            let seats = crate::seat::SeatOccupancyStore::open_default()?;
            let roster = management.snapshot()?;
            let target =
                crate::role_revision::CutexSessionId::new(options.cutex_session_id.clone())
                    .map_err(|_| anyhow::anyhow!("invalid presentation recipient"))?;
            ensure!(
                roster
                    .agents
                    .get(&target)
                    .is_none_or(|a| a.retired_at.is_none()),
                "presentation recipient permanently retired"
            );
            seats.with_notification_snapshot(|s| {
                validate_target(&message, &options.cutex_session_id, s, &roster)
            })??;
            if client.is_none() {
                client = Some(PresentationClient::connect(
                    &path,
                    &options.cutex_session_id,
                    generation,
                )?);
            }
            let c = client.as_ref().expect("connected");
            let frozen = repository.freeze_presentation(
                &options.cutex_session_id,
                &id,
                &c.binding().thread_id,
            )?;
            let receipt = match c.status(&frozen)? {
                Some(r) => r,
                None => {
                    #[cfg(all(unix, feature = "stock-launch-test-hook"))]
                    before_commit_test_gate(&message, "before_presentation_append")?;
                    c.append(&frozen)?
                }
            };
            #[cfg(all(unix, feature = "stock-launch-test-hook"))]
            before_commit_test_gate(&message, "after_presentation_append")?;
            let _mutation = management
                .try_lock_delivery_mutations()?
                .context("lifecycle transition in progress")?;
            let roster = management.snapshot()?;
            ensure!(
                roster
                    .agents
                    .get(&target)
                    .is_none_or(|a| a.retired_at.is_none()),
                "presentation recipient permanently retired"
            );
            seats.with_notification_snapshot(|s| -> anyhow::Result<()> {
                validate_target(&message, &options.cutex_session_id, s, &roster)?;
                crate::session::store::with_locked_session_store(&path, |_| {
                    c.fence()?;
                    repository.commit_presentation(
                        &options.cutex_session_id,
                        &id,
                        &frozen,
                        &receipt,
                        generation,
                    )
                })
            })??;
            Ok(())
        })();
        if let Err(error) = result {
            repository.presentation_error(&id, &error.to_string())?;
            // Recorded reason + per-record 30s backoff; unrelated work continues.
        }
    }
    Ok(())
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
                    let activated_at = native_task_lineage(
                        &store,
                        r,
                        &before.bootstrap_intents,
                        &options.cutex_session_id,
                    )?;
                    // The explicit activation, not the latest runtime start,
                    // establishes this private lineage across owned restarts.
                    fresh_task_projection(&polled, &options.cutex_session_id, activated_at)?;
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
                    let mut e = envelope(m, client.binding())?;
                    let sender_name = m.from_cutex_session_id.as_ref().and_then(|id| {
                        crate::session::store::load_cutex_session_store_from_path(&path)
                            .ok()
                            .and_then(|store| {
                                store
                                    .sessions
                                    .get(id)
                                    .and_then(|r| r.formal_agent_name.clone())
                            })
                    });
                    agent_message_view(
                        &mut e,
                        sender_name.as_deref(),
                        m.delivery_mode.event_label(),
                    )?;
                    if let Some(view) = e.view.as_mut().filter(|view| view.schema == "cutex.agent-message.v1") {
                        if m.created_at_epoch_secs > 0 {
                            view.data["occurredAtEpochSeconds"] = serde_json::json!(m.created_at_epoch_secs);
                        }
                        e.semantic_sha256 = e.digest();
                    }
                    task_display::apply(&mut e, m)?;
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
    fn management_start_is_reserved_service_data_not_human_or_agent_authority() {
        let mut m = message();
        m.from = AGENT_MANAGEMENT_SYSTEM_SENDER.into();
        m.control_type = Some(AGENT_MANAGEMENT_START_CONTROL_TYPE.into());
        m.external_message_id = Some("management-action".into());
        m.control_payload = Some(
            serde_json::json!({"schema":"cutex/agent-management/v1","requested_by_director":"cutex.director","requested_by_operator":null}),
        );
        let e = envelope(&m, &binding()).unwrap();
        assert_eq!(e.message.source.kind, SourceKind::Service);
        assert_eq!(e.message.source.id, AGENT_MANAGEMENT_SYSTEM_SENDER);
        assert_eq!(e.message.event_type, "management_start");
        assert!(e
            .message
            .text
            .contains("Requested by Director: cutex.director"));
        m.from = "forged ordinary sender".into();
        assert!(envelope(&m, &binding()).is_err());
    }

    #[test]
    fn ready_launch_establishes_task_lineage_without_migration_receipts() {
        use crate::agent_management::{ExplicitLaunchActionReceipt, StockRuntimeReceipt};
        let mut record = crate::session::model::CutexSessionRecord::new(
            "cutex.worker".into(),
            Some("22222222-2222-4222-8222-222222222222".into()),
            "private".into(),
            "/private".into(),
            Some("alpha".into()),
        )
        .unwrap();
        record.created_at = "2026-09-13T00:00:00Z".into();
        let mut receipt: StockRuntimeReceipt = serde_json::from_value(serde_json::json!({
            "action_id":"normal-create", "stage":"ready", "claim_id":"claim", "runtime_agent_id":"stock.worker",
            "expected_generation":1,"binding":null,"publication":null,"error":null,"updated_at":"2026-09-14T00:00:00Z",
            "review":{
                "subject":{"cutex_session_id":record.cutex_session_id,"formal_name":"worker","durable_sha256":"a".repeat(64),"authority_sha256":"a".repeat(64),"current_project_id":null,"revision":1,"runtime_generation":0},
                "contract":{"version":4,"native_id":record.codex_session_id,"native_home":"/private","bundle_manifest":"/private/manifest","bundle_sha256":"b".repeat(64)},
                "configuration":{"profile_name":"alpha","profile_id":"private","inherited":false,"profile_sha256":"c".repeat(64),"account_sha256":"d".repeat(64),"model":"fixture","reasoning":null,"model_provider":"private","provider":{"name":"private","base_url":"http://127.0.0.1:1/v1","wire_api":"responses","requires_openai_auth":false,"supports_websockets":false},"sandbox":"danger-full-access","approval":"never"},
                "restart":false
            }
        })).unwrap();
        record.explicit_launch = Some(receipt.review.contract.clone());
        let mut store = crate::session::model::CutexSessionStore::default();
        let intents = Default::default();
        store.explicit_launch_receipts.insert(
            receipt.action_id.to_string(),
            ExplicitLaunchActionReceipt::Runtime(receipt.clone()),
        );
        assert_eq!(
            native_task_lineage(&store, &record, &intents, &record.cutex_session_id).unwrap(),
            record.created_at
        );
        // A later restart must not make an already queued assignment too old.
        receipt.review.restart = true;
        receipt.updated_at = "2026-09-15T00:00:00Z".into();
        store.explicit_launch_receipts.insert(
            receipt.action_id.to_string(),
            ExplicitLaunchActionReceipt::Runtime(receipt),
        );
        assert_eq!(
            native_task_lineage(&store, &record, &intents, &record.cutex_session_id).unwrap(),
            record.created_at
        );
        record.explicit_launch.as_mut().unwrap().bundle_manifest = "/private/new-bundle".into();
        assert_eq!(
            native_task_lineage(&store, &record, &intents, &record.cutex_session_id).unwrap(),
            record.created_at
        );
        assert!(native_task_lineage(&store, &record, &intents, "cutex.other").is_err());
        record.codex_session_id = Some("33333333-3333-4333-8333-333333333333".into());
        assert!(native_task_lineage(&store, &record, &intents, &record.cutex_session_id).is_err());
    }

    #[test]
    fn agent_display_facts_do_not_change_model_text_or_durable_sender() {
        let mut e = envelope(&message(), &binding()).unwrap();
        let original = e.message.clone();
        agent_message_view(&mut e, Some("worker-name"), "after_turn").unwrap();
        assert_eq!(e.message, original);
        assert_eq!(e.version, 2);
        assert_eq!(e.view.as_ref().unwrap().data["senderName"], "worker-name");
        assert_eq!(e.semantic_sha256, e.digest());
        e.validate().unwrap();
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

#[path = "bus_bridge_task_display.rs"]
mod task_display;
