//! Agent bus delivery-mode semantics and legacy compatibility helpers.

use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentDeliveryMode {
    #[default]
    AfterTurn,
    Soon,
    Passive,
    Interrupt,
}

impl AgentDeliveryMode {
    pub fn trigger_turn(&self) -> bool {
        !matches!(self, AgentDeliveryMode::Passive)
    }

    pub fn label(&self) -> &'static str {
        match self {
            AgentDeliveryMode::AfterTurn => "after-turn",
            AgentDeliveryMode::Soon => "soon",
            AgentDeliveryMode::Passive => "passive",
            AgentDeliveryMode::Interrupt => "interrupt",
        }
    }

    pub fn event_label(&self) -> &'static str {
        match self {
            AgentDeliveryMode::AfterTurn => "after_turn",
            AgentDeliveryMode::Soon => "soon",
            AgentDeliveryMode::Passive => "passive",
            AgentDeliveryMode::Interrupt => "interrupt",
        }
    }
}

pub fn agent_delivery_mode_from_flags(
    queue_only: bool,
    soon: bool,
    interrupt: bool,
) -> anyhow::Result<AgentDeliveryMode> {
    let selected = [queue_only, soon, interrupt]
        .into_iter()
        .filter(|selected| *selected)
        .count();
    if selected > 1 {
        anyhow::bail!("Choose only one of --queue-only, --soon, or --interrupt");
    }
    if queue_only {
        Ok(AgentDeliveryMode::Passive)
    } else if soon {
        Ok(AgentDeliveryMode::Soon)
    } else if interrupt {
        Ok(AgentDeliveryMode::Interrupt)
    } else {
        Ok(AgentDeliveryMode::AfterTurn)
    }
}

pub fn legacy_delivery_mode_label(trigger_turn: bool) -> &'static str {
    if trigger_turn {
        "soon"
    } else {
        "passive"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_delivery_mode_flags_default_to_after_turn() {
        assert_eq!(
            agent_delivery_mode_from_flags(false, false, false).expect("default mode should parse"),
            AgentDeliveryMode::AfterTurn
        );
        assert_eq!(
            agent_delivery_mode_from_flags(true, false, false).expect("queue-only should parse"),
            AgentDeliveryMode::Passive
        );
        assert_eq!(
            agent_delivery_mode_from_flags(false, true, false).expect("soon should parse"),
            AgentDeliveryMode::Soon
        );
        assert!(agent_delivery_mode_from_flags(true, true, false).is_err());
    }
}
