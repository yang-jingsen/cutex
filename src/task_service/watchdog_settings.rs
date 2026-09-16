//! Persisted defaults for the existing watchdog timing stages.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TaskWatchdogSettings {
    pub poll_secs: u64,
    pub stale_secs: u64,
    pub escalation_secs: u64,
}
impl Default for TaskWatchdogSettings {
    fn default() -> Self {
        Self {
            poll_secs: 60,
            stale_secs: 600,
            escalation_secs: 600,
        }
    }
}
impl TaskWatchdogSettings {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            (5..=3600).contains(&self.poll_secs),
            "Watchdog poll interval must be 5–3600 seconds"
        );
        anyhow::ensure!(
            (60..=86400).contains(&self.stale_secs),
            "Watchdog reminder threshold must be 60–86400 seconds"
        );
        anyhow::ensure!(
            (60..=86400).contains(&self.escalation_secs),
            "Watchdog escalation interval must be 60–86400 seconds"
        );
        Ok(())
    }
}
