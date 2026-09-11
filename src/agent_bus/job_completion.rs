//! Frozen Job v2 facts and deterministic projections. No provider queries.
use super::model::JobServiceTerminalStatus;
use crate::app_server::external_input::view::StructuredView;
use anyhow::ensure;
use serde::{Deserialize, Serialize};

pub const SCHEMA_V2: &str = "cutex.job_service.completion.v2";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IncomingCompletion {
    V1(super::model::JobServiceCompletionRequest),
    V2(CompletionV2),
}

/// Frozen once at canonical acceptance, before any native occurrence is bound.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrozenProjection {
    pub version: u32,
    pub native_version: u32,
    pub request: CompletionV2,
    pub model_text: String,
    pub view: StructuredView,
}
impl FrozenProjection {
    pub fn new(request: CompletionV2) -> anyhow::Result<Self> {
        request.validate()?;
        Ok(Self {
            version: 1,
            native_version: 2,
            model_text: request.model_text(),
            view: request.view()?,
            request,
        })
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            self.version == 1 && self.native_version == 2,
            "unsupported frozen Job projection"
        );
        self.request.validate()?;
        // Version1 is immutable; a future formatter must use a new version.
        ensure!(
            self.model_text == self.request.model_text() && self.view == self.request.view()?,
            "frozen Job projection conflict"
        );
        Ok(())
    }
    pub fn from_message(message: &super::model::AgentBusMessage) -> anyhow::Result<Self> {
        ensure!(
            message.sender_kind == super::model::AgentMessageKind::JobServiceSystem
                && message.from == "cutex-job-service"
                && message.from_cutex_session_id.is_none()
                && message.control_type.as_deref() == Some(SCHEMA_V2)
                && message.delivery_mode == super::delivery::AgentDeliveryMode::AfterTurn,
            "invalid protected Job v2 provenance"
        );
        let frozen: Self = serde_json::from_value(
            message
                .control_payload
                .clone()
                .ok_or_else(|| anyhow::anyhow!("missing frozen Job v2"))?,
        )?;
        frozen.validate()?;
        ensure!(
            message.to_cutex_session_id.as_deref()
                == Some(frozen.request.target_cutex_session_id.as_str())
                && message.external_message_id.as_deref() == Some(frozen.request.event_id.as_str())
                && message.external_action_id.as_deref() == Some(frozen.request.job_id.as_str())
                && message.content == serde_json::to_string(&frozen)?,
            "Job v2 canonical identity/content conflict"
        );
        Ok(frozen)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompletionV2 {
    pub schema: String,
    pub event_id: String,
    pub job_id: String,
    pub job_revision: u64,
    pub terminal_status: JobServiceTerminalStatus,
    pub result_sha256: String,
    pub target_cutex_session_id: String,
    pub facts: Facts,
    pub output_reference: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Facts {
    pub facts_version: u32,
    pub action_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<Execution>,
    pub stdout: Stream,
    pub stderr: Stream,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Execution {
    pub basis: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_observed_at_epoch_millis: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_observed_at_epoch_millis: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_run_duration_millis: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Stream {
    pub retained_bytes: u64,
    pub observed_bytes: u64,
    pub truncated: bool,
}

impl CompletionV2 {
    pub fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            self.schema == SCHEMA_V2 && self.facts.facts_version == 1,
            "unsupported Job facts version"
        );
        for id in [&self.event_id, &self.job_id] {
            ensure!(
                !id.is_empty()
                    && id.len() <= 128
                    && id.bytes().all(
                        |b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-')
                    ),
                "invalid Job completion identifier"
            );
        }
        ensure!(self.job_revision > 0, "Job revision must be positive");
        crate::role_revision::Sha256::new(self.result_sha256.clone())
            .map_err(|_| anyhow::anyhow!("invalid Job result digest"))?;
        ensure!(
            !self.facts.action_id.trim().is_empty() && self.facts.action_id.len() <= 256,
            "invalid Job action label"
        );
        ensure!(
            self.output_reference.len() <= 2048,
            "Job output reference exceeds limit"
        );
        ensure!(
            self.facts
                .terminal_reason
                .as_ref()
                .is_none_or(|v| v.len() <= 2048),
            "Job reason exceeds limit"
        );
        ensure!(
            self.facts.exit_code.is_none_or(|code| code >= 0),
            "Job facts exit code must not encode a signal"
        );
        if let Some(execution) = &self.facts.execution {
            ensure!(
                execution.basis == "runner_release_to_wait_v1",
                "unsupported Job execution basis"
            );
        }
        for stream in [&self.facts.stdout, &self.facts.stderr] {
            ensure!(
                stream.retained_bytes <= stream.observed_bytes,
                "Job retained bytes exceed observed bytes"
            );
        }
        self.view()?;
        Ok(())
    }
    pub fn model_text(&self) -> String {
        let label = match (&self.terminal_status, self.facts.exit_code) {
            (JobServiceTerminalStatus::Exited, Some(0)) => "completed",
            (JobServiceTerminalStatus::Exited, _) => "exited",
            (JobServiceTerminalStatus::Failed, _) => "failed",
            (JobServiceTerminalStatus::Cancelled, _) => "cancelled",
            (JobServiceTerminalStatus::Interrupted, _) => "interrupted",
            (JobServiceTerminalStatus::LaunchUnknown, _) => "launch outcome unknown",
        };
        let mut text = format!("Job {label}. jobId: {}", self.job_id);
        if !self.facts.action_id.is_empty() {
            text.push_str(&format!("\nAction: {}", self.facts.action_id));
        }
        if let Some(code) = self.facts.exit_code {
            text.push_str(&format!("\nExit code: {code}"));
        }
        if let Some(duration) = self
            .facts
            .execution
            .as_ref()
            .and_then(|e| e.observed_run_duration_millis)
        {
            text.push_str(&format!("\nObserved run: {duration} ms"));
        }
        if let Some(reason) = self
            .facts
            .terminal_reason
            .as_ref()
            .filter(|s| !s.is_empty())
        {
            text.push_str(&format!("\nReason (external data): {reason}"));
        }
        text
    }

    pub fn view(&self) -> anyhow::Result<StructuredView> {
        ensure!(
            self.schema == SCHEMA_V2 && self.facts.facts_version == 1,
            "unsupported Job facts version"
        );
        if let Some(execution) = &self.facts.execution {
            ensure!(
                execution.basis == "runner_release_to_wait_v1",
                "unsupported Job execution basis"
            );
        }
        let mut data = serde_json::to_value(&self.facts)?;
        let object = data.as_object_mut().expect("typed facts object");
        object.remove("factsVersion");
        object.insert("jobId".into(), self.job_id.clone().into());
        object.insert("jobRevision".into(), self.job_revision.into());
        object.insert(
            "terminalStatus".into(),
            serde_json::to_value(&self.terminal_status)?,
        );
        object.insert(
            "outputReference".into(),
            self.output_reference.clone().into(),
        );
        let view = StructuredView {
            schema: "cutex.job-completion.v1".into(),
            data,
        };
        view.canonical_json()?;
        Ok(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> CompletionV2 {
        serde_json::from_value(serde_json::json!({
            "schema":SCHEMA_V2,"eventId":"job-terminal:job_123:3", "jobId":"job_123",
            "jobRevision":3,"terminalStatus":"exited","resultSha256":"a".repeat(64),
            "targetCutexSessionId":"cutex.11111111-1111-4111-8111-111111111111",
            "facts":{"factsVersion":1,"actionId":"human-job-1",
                "stdout":{"observedBytes":39,"retainedBytes":39,"truncated":false},
                "stderr":{"observedBytes":0,"retainedBytes":0,"truncated":false}},
            "outputReference":"job-output:job_123"
        }))
        .unwrap()
    }
    #[test]
    fn unknown_exit_is_not_success_and_default_id_once() {
        let request = fixture();
        let text = request.model_text();
        assert_eq!(text, "Job exited. jobId: job_123\nAction: human-job-1");
        assert_eq!(text.matches("job_123").count(), 1);
        let view = request.view().unwrap();
        assert!(view.data.get("exitCode").is_none());
        assert!(view.data.get("execution").is_none());
        assert_eq!(view.data["outputReference"], "job-output:job_123");
        assert!(!text.contains("outputReference"));
    }
    #[test]
    fn replay_and_real_zero_duration_preserved() {
        let mut request = fixture();
        request.facts.exit_code = Some(0);
        request.facts.execution = Some(Execution {
            basis: "runner_release_to_wait_v1".into(),
            start_observed_at_epoch_millis: None,
            exit_observed_at_epoch_millis: None,
            observed_run_duration_millis: Some(0),
        });
        let restored: CompletionV2 =
            serde_json::from_slice(&serde_json::to_vec(&request).unwrap()).unwrap();
        assert_eq!(request.model_text(), restored.model_text());
        assert_eq!(request.view().unwrap(), restored.view().unwrap());
        assert!(restored.model_text().contains("Job completed."));
        assert!(restored.model_text().contains("Observed run: 0 ms"));
        request.terminal_status = JobServiceTerminalStatus::Cancelled;
        assert!(request.model_text().starts_with("Job cancelled."));
    }

    #[test]
    fn exact_producer_bounds_and_unknown_fields_fail_closed() {
        let mut request = fixture();
        request.validate().unwrap();
        request.facts.action_id = "中".repeat(86);
        assert!(request.validate().is_err());
        request = fixture();
        request.facts.exit_code = Some(-9);
        assert!(request.validate().is_err());
        request = fixture();
        request.facts.stdout.retained_bytes = 40;
        assert!(request.validate().is_err());
        let mut value = serde_json::to_value(fixture()).unwrap();
        value["summary"] = "not a v2 field".into();
        assert!(serde_json::from_value::<CompletionV2>(value).is_err());
    }
}
