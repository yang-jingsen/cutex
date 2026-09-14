//! Frozen descriptive Task notification facts; never routing or mutation authority.
use super::*;

pub(super) fn apply(e: &mut Envelope, message: &AgentBusMessage) -> anyhow::Result<()> {
    if e.message.source.kind != SourceKind::Service
        || e.message.source.id != "cutex-task-service"
        || e.message.event_type != "task_notification"
        || message.control_type.as_deref() != Some("cutex.task_service.completion.v1")
    {
        return Ok(());
    }
    let metadata: crate::agent_bus::model::TaskServiceCompletionMetadata = serde_json::from_value(
        message
            .control_payload
            .clone()
            .context("missing Task metadata")?,
    )?;
    let notification = task_provider()?
        .completion_notification(&metadata.notification_id)?
        .context("missing Task notification")?;
    ensure!(
        notification.assignment_id == metadata.assignment_id
            && notification.transition_action_id == metadata.transition_action_id
            && notification.kind == metadata.kind,
        "Task display identity mismatch"
    );
    e.version = 2;
    e.view = Some(crate::app_server::external_input::view::StructuredView {
        schema: "cutex.task-notification.v1".into(),
        data: serde_json::json!({
            "notificationId": notification.notification_id,
            "taskId": notification.task_id,
            "assignmentId": notification.assignment_id,
            "taskRevision": notification.task_revision,
            "attemptNumber": notification.attempt_number,
            "transitionActionId": notification.transition_action_id,
            "kind": notification.kind,
            "occurredAt": notification.created_at,
        }),
    });
    e.semantic_sha256 = e.digest();
    e.validate()
}
