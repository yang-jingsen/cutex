//! Frozen Job v2 facts and deterministic projections. No provider queries.
use super::model::JobServiceTerminalStatus;
use crate::app_server::external_input::view::StructuredView;
use anyhow::ensure;
use serde::{Deserialize, Serialize};

pub const SCHEMA_V2: &str = "cutex.job_service.completion.v2";

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
    pub observed_bytes: u64,
    pub retained_bytes: u64,
    pub truncated: bool,
}

impl CompletionV2 {
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
}
