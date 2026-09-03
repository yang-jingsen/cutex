//! Agent bus audit log storage and terminal rendering.

use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use anyhow::Context;
use serde_json::Value;

use crate::agent_bus::delivery::legacy_delivery_mode_label;
use crate::config::paths::runtime_dir;

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const CYAN: &str = "\x1b[36m";

pub fn agent_bus_audit_log_path() -> anyhow::Result<PathBuf> {
    Ok(runtime_dir()?.join("agent-bus-audit.jsonl"))
}

pub fn append_agent_bus_audit_record(value: Value) -> anyhow::Result<()> {
    let path = agent_bus_audit_log_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create runtime dir: {}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("Failed to open agent audit log: {}", path.display()))?;
    serde_json::to_writer(&mut file, &value)?;
    writeln!(file)?;
    Ok(())
}

pub fn content_preview(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let mut preview = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        preview.push_str("...");
    }
    preview
}

pub fn agent_audit_record_matches(value: &Value, agent: &str) -> bool {
    [
        "from",
        "to",
        "to_name",
        "agent_id",
        "agent_name",
        "message_id",
    ]
    .iter()
    .any(|key| value.get(*key).and_then(Value::as_str) == Some(agent))
        || value
            .get("message_ids")
            .and_then(Value::as_array)
            .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(agent)))
}

pub fn print_agent_audit_record(value: &Value) {
    let ts = value
        .get("timestamp")
        .and_then(Value::as_str)
        .unwrap_or("-");
    match value.get("event").and_then(Value::as_str).unwrap_or("-") {
        "sent" => {
            let id = value
                .get("message_id")
                .and_then(Value::as_str)
                .unwrap_or("-");
            let from = value.get("from").and_then(Value::as_str).unwrap_or("-");
            let to = value
                .get("to_name")
                .and_then(Value::as_str)
                .or_else(|| value.get("to").and_then(Value::as_str))
                .unwrap_or("-");
            let trigger = value
                .get("trigger_turn")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let delivery_mode = value
                .get("delivery_mode")
                .and_then(Value::as_str)
                .unwrap_or_else(|| legacy_delivery_mode_label(trigger));
            let deduplicated = value
                .get("deduplicated")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let preview = value
                .get("content_preview")
                .and_then(Value::as_str)
                .unwrap_or("");
            println!(
                "{DIM}{ts}{RESET} {GREEN}sent{RESET} {id} {from} -> {to} mode={delivery_mode} trigger_turn={trigger} deduplicated={deduplicated} {preview}"
            );
        }
        "polled" => {
            let agent_id = value.get("agent_id").and_then(Value::as_str).unwrap_or("-");
            let agent_name = value
                .get("agent_name")
                .and_then(Value::as_str)
                .unwrap_or("-");
            let count = value.get("count").and_then(Value::as_u64).unwrap_or(0);
            println!(
                "{DIM}{ts}{RESET} {CYAN}polled{RESET} agent={agent_name} id={agent_id} count={count}"
            );
        }
        "acked" => {
            let agent_id = value.get("agent_id").and_then(Value::as_str).unwrap_or("-");
            let count = value.get("count").and_then(Value::as_u64).unwrap_or(0);
            println!("{DIM}{ts}{RESET} {GREEN}acked{RESET} agent={agent_id} count={count}");
        }
        event => println!("{DIM}{ts}{RESET} {event} {value}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_preview_truncates_on_character_boundaries() {
        assert_eq!(content_preview("abcdef", 3), "abc...");
        assert_eq!(content_preview("ab", 3), "ab");
    }

    #[test]
    fn audit_record_match_checks_agent_fields_and_message_ids() {
        let value = serde_json::json!({
            "from": "agent-a",
            "message_ids": ["msg-1", "msg-2"]
        });

        assert!(agent_audit_record_matches(&value, "agent-a"));
        assert!(agent_audit_record_matches(&value, "msg-2"));
        assert!(!agent_audit_record_matches(&value, "agent-b"));
    }
}
