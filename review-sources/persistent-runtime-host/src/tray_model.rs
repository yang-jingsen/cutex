//! Pure projection of v1 host state into the compact Windows tray surface.

use crate::model::{HealthState, HostStatus, ObservedState, OperationPhase, StopOutcome};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayServiceProjection {
    pub service_id: String,
    pub label: String,
    pub status: String,
    pub can_start: bool,
    pub can_stop: bool,
    pub can_restart: bool,
    pub needs_attention: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayProjection {
    pub host_label: String,
    pub tooltip: String,
    pub services: Vec<TrayServiceProjection>,
    /// Stable fingerprint of currently visible degraded/error conditions. A
    /// tray client may dismiss this exact fingerprint without mutating host
    /// truth; a changed condition produces a new fingerprint and reappears.
    pub alert_fingerprint: Option<String>,
}

pub fn project_tray_status(status: &HostStatus) -> TrayProjection {
    let mutations_enabled = status.phase == crate::model::HostPhase::Running;
    let mut services = status
        .services
        .iter()
        .map(|service| {
            let runtime = &service.runtime;
            let needs_attention = runtime.observed_state == ObservedState::Failed
                || runtime.health == HealthState::Unhealthy
                || runtime.operation_phase == Some(OperationPhase::Failed)
                || runtime.last_stop_outcome == Some(StopOutcome::TimedOut);
            let status = match (runtime.observed_state, runtime.health) {
                (ObservedState::Running, HealthState::Healthy) => "running · healthy".to_owned(),
                (ObservedState::Running, HealthState::Unhealthy) => {
                    "running · unhealthy".to_owned()
                }
                (ObservedState::Running, _) => "running".to_owned(),
                (ObservedState::Starting, _) => "starting".to_owned(),
                (ObservedState::Stopping, _) => "stopping".to_owned(),
                (ObservedState::Failed, _) => "failed".to_owned(),
                (ObservedState::Stopped, _) => "stopped".to_owned(),
            };
            TrayServiceProjection {
                service_id: service.definition.id.as_str().to_owned(),
                label: service.definition.display_name.clone(),
                status,
                can_start: mutations_enabled
                    && matches!(
                        runtime.observed_state,
                        ObservedState::Stopped | ObservedState::Failed
                    ),
                can_stop: mutations_enabled
                    && !matches!(runtime.observed_state, ObservedState::Stopped),
                can_restart: mutations_enabled
                    && matches!(
                        runtime.observed_state,
                        ObservedState::Running | ObservedState::Failed
                    ),
                needs_attention,
            }
        })
        .collect::<Vec<_>>();
    services.sort_by(|left, right| {
        left.label
            .to_lowercase()
            .cmp(&right.label.to_lowercase())
            .then_with(|| left.service_id.cmp(&right.service_id))
    });
    let running = status
        .services
        .iter()
        .filter(|service| service.runtime.observed_state == ObservedState::Running)
        .count();
    let attention = services
        .iter()
        .filter(|service| service.needs_attention)
        .count();
    let alert_fingerprint = (attention > 0).then(|| {
        services
            .iter()
            .filter(|service| service.needs_attention)
            .map(|service| format!("{}={}", service.service_id, service.status))
            .collect::<Vec<_>>()
            .join("|")
    });
    TrayProjection {
        host_label: format!("PRH {:?}", status.phase).to_lowercase(),
        tooltip: if attention == 0 {
            format!("PRH: {running}/{} services running", services.len())
        } else {
            format!("PRH: {attention} service(s) need attention")
        },
        services,
        alert_fingerprint,
    }
}

impl TrayProjection {
    pub fn alerts_visible(&self, dismissed_fingerprint: Option<&str>) -> bool {
        self.alert_fingerprint
            .as_deref()
            .is_some_and(|fingerprint| Some(fingerprint) != dismissed_fingerprint)
    }
}
