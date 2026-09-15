//! On-demand Human report history; ordinary task refresh excludes report bodies.
use crate::task_service::AssignmentId;
use crate::task_service::TaskServiceProvider;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Report {
    pub key: String,
    pub attempt: u64,
    pub kind: String,
    pub timestamp: String,
    pub text: String,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReportPage {
    pub reports: Vec<Report>,
    pub next_before: Option<String>,
}
fn report_text(value: &str) -> Option<String> {
    let preview = crate::observability::sanitize_visible_output(value)?;
    if preview == "[redacted sensitive output]" {
        return Some(preview);
    }
    // A report is not the 512-character live-activity preview. Preserve its
    // contents, replacing terminal controls; bound exceptional legacy bodies.
    let mut text = value
        .chars()
        .take(8192)
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                '\u{fffd}'
            } else {
                c
            }
        })
        .collect::<String>();
    if value.chars().count() > 8192 {
        text.push_str("\n[Report display truncated at 8,192 characters]");
    }
    Some(text)
}
fn report_key(time: &str, attempt: u64, kind: &str, action: &str) -> String {
    let time = chrono::DateTime::parse_from_rfc3339(time)
        .map(|t| {
            t.with_timezone(&chrono::Utc)
                .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
        })
        .unwrap_or_else(|_| time.to_owned());
    format!("{time}|{attempt:020}|{kind}|{action}")
}
fn page(mut rows: Vec<Report>, before: &str) -> ReportPage {
    rows.sort_by(|a, b| b.key.cmp(&a.key));
    rows.retain(|r| before.is_empty() || r.key.as_str() < before);
    let more = rows.len() > 25;
    rows.truncate(25);
    let next_before = more.then(|| rows.last().unwrap().key.clone());
    ReportPage {
        reports: rows,
        next_before,
    }
}
pub fn read(
    provider: &TaskServiceProvider,
    assignment: &AssignmentId,
    before: &str,
) -> anyhow::Result<ReportPage> {
    let snapshot = provider
        .query_assignment(assignment)
        .map_err(|e| anyhow::anyhow!("Task reports unavailable: {e}"))?;
    let mut rows = Vec::new();
    for attempt in snapshot
        .attempts
        .get(assignment)
        .into_iter()
        .flat_map(|a| a.values())
    {
        let number = attempt.attempt_number.get();
        for status in &attempt.status_receipts {
            if let Some(text) = report_text(&status.summary) {
                rows.push(Report {
                    key: report_key(
                        status.recorded_at.as_str(),
                        number,
                        "status",
                        status.action_id.as_str(),
                    ),
                    attempt: number,
                    kind: "Status".into(),
                    timestamp: status.recorded_at.as_str().into(),
                    text,
                });
            }
        }
        for result in &attempt.result_receipts {
            if let Some(text) = report_text(&result.result_reference) {
                rows.push(Report {
                    key: report_key(
                        result.submitted_at.as_str(),
                        number,
                        "result",
                        result.action_id.as_str(),
                    ),
                    attempt: number,
                    kind: "Result reference".into(),
                    timestamp: result.submitted_at.as_str().into(),
                    text,
                });
            }
        }
    }
    Ok(page(rows, before))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reports_preserve_long_text_and_normalize_time_zones() {
        let body = "line\n".repeat(300);
        assert_eq!(report_text(&body).unwrap(), body);
        assert!(!report_text("hello\x1b[31m").unwrap().contains('\x1b'));
        assert_eq!(
            report_key("2026-09-15T21:00:00+10:00", 1, "status", "a"),
            report_key("2026-09-15T11:00:00Z", 1, "status", "a")
        );
    }
    #[test]
    fn newer_arrivals_do_not_shift_older_pages() {
        let mut rows = (0..60)
            .map(|i| Report {
                key: format!("{i:04}"),
                attempt: 1,
                kind: "Status".into(),
                timestamp: String::new(),
                text: i.to_string(),
            })
            .collect::<Vec<_>>();
        let first = page(rows.clone(), "");
        assert_eq!(first.reports[0].text, "59");
        rows.push(Report {
            key: "0060".into(),
            attempt: 1,
            kind: "Status".into(),
            timestamp: String::new(),
            text: "new".into(),
        });
        let second = page(rows, first.next_before.as_deref().unwrap());
        assert_eq!(second.reports[0].text, "34");
        assert_eq!(second.reports.len(), 25);
    }
}
