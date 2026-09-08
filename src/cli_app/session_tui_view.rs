//! Ephemeral, read-only list projection. Never a persisted identity or authority.
use cutex::agent_management::{CutexProjectWorkspace, ProjectMemberProjection};
use ratatui::{
    layout::{Constraint, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Cell, Paragraph, Row, Table, TableState, Wrap},
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ui_contract_c_v04_columns_fit_inner_width_without_zero_placeholders() {
        for width in 1..=240 {
            for kind in [ListKind::Managed, ListKind::Recent, ListKind::Members] {
                let columns = visible_columns(width, kind);
                assert_eq!(columns[0].0, Column::Name);
                assert!(columns.iter().all(|(_, w)| *w > 0));
                assert!(
                    columns.iter().map(|(_, w)| *w).sum::<u16>() + columns.len() as u16 - 1
                        <= width
                );
                if width >= 36 && kind == ListKind::Members {
                    assert!(columns.iter().any(|(c, _)| *c == Column::Role));
                }
            }
        }
    }
    #[test]
    fn ui_contract_c_v04_unicode_and_windows_tail_identity() {
        for width in 1..40 {
            let value = tail_path(r"C:\長い名前\shared\unique-final", width);
            assert!(value.width() <= width);
            assert!(clipped("界界 emoji 🦀 end", width).width() <= width);
        }
        assert!(tail_path(r"C:\same\different\unique-final", 18).ends_with("unique-final"));
        assert_ne!(
            SubjectRef::Managed("one".into()),
            SubjectRef::Managed("two".into())
        );
        assert_ne!(
            SubjectRef::Native {
                catalog: "a".into(),
                thread: "same".into()
            },
            SubjectRef::Native {
                catalog: "b".into(),
                thread: "same".into()
            }
        );
        assert_ne!(
            Observation::Known("Offline".into()),
            Observation::<String>::Unavailable("missing".into())
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum SubjectRef {
    Managed(String),
    Native { catalog: String, thread: String },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Observation<T> {
    Known(T),
    Unavailable(String),
    Stale(T, String),
}
impl<T> Observation<T> {
    pub fn known(&self) -> Option<&T> {
        match self {
            Self::Known(v) | Self::Stale(v, _) => Some(v),
            _ => None,
        }
    }
}
impl Observation<String> {
    pub fn label(&self) -> String {
        match self {
            Self::Known(v) => v.clone(),
            Self::Unavailable(_) => "Unavailable".into(),
            Self::Stale(v, _) => format!("{v} (stale)"),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AgentSessionView {
    pub subject: SubjectRef,
    pub name: String,
    pub native_title: Option<String>,
    pub native_thread: Option<String>,
    pub native_workspace: Option<String>,
    pub runtime: Observation<String>,
    pub project: Observation<String>,
    pub configured_profile: Option<String>,
    pub effective_profile: Observation<String>,
    pub role: String,
    pub activity: String,
    pub updated: String,
    pub cwd: String,
    pub retirement_note: Option<String>,
}

pub(super) fn member_view(
    member: &ProjectMemberProjection,
    project: &str,
    role: &str,
) -> AgentSessionView {
    let runtime = match (&member.runtime, &member.observation_error) {
        (_, Some(error)) => Observation::Unavailable(error.clone()),
        (Some(runtime), None)
            if runtime.cutex_session_id == member.agent.cutex_session_id
                && runtime.native_session_id == member.agent.native_session_id =>
        {
            Observation::Known(if runtime.active { "Online" } else { "Offline" }.into())
        }
        _ => Observation::Unavailable("runtime observation missing or identity mismatch".into()),
    };
    AgentSessionView {
        subject: SubjectRef::Managed(member.agent.cutex_session_id.as_str().to_owned()),
        name: member.agent.spec.name.clone(), // current provider formal-name projection
        native_title: None, native_thread: Some(member.agent.native_session_id.clone()), native_workspace: None,
        runtime,
        project: Observation::Known(project.into()),
        configured_profile: member.agent.spec.profile.clone(),
        effective_profile: member.runtime.as_ref().filter(|r| r.cutex_session_id == member.agent.cutex_session_id && r.native_session_id == member.agent.native_session_id && member.observation_error.is_none()).map(|r| Observation::Known(r.profile.clone())).unwrap_or_else(|| Observation::Unavailable("effective launch profile not observed".into())),
        role: role.into(), activity: "—".into(), updated: "—".into(), cwd: member.agent.spec.cwd.clone(),
        retirement_note: member.agent.retired_at.as_ref().map(|_| "Provider marks this identity retired; durable retirement not independently reconciled".into()),
    }
}

pub(super) fn project_members(project: &CutexProjectWorkspace) -> Vec<AgentSessionView> {
    let mut members = std::collections::BTreeMap::<SubjectRef, AgentSessionView>::new();
    for (member, role) in project
        .director
        .member
        .iter()
        .map(|m| (m, "Director"))
        .chain(
            project
                .agent_operators
                .iter()
                .map(|m| (&m.member, "Operator")),
        )
        .chain(project.active_agents.iter().map(|m| (m, "Member")))
    {
        let view = member_view(member, project.project_id.as_str(), role);
        members
            .entry(view.subject.clone())
            .and_modify(|existing| {
                if !existing.role.split('/').any(|r| r == role) {
                    existing.role.push('/');
                    existing.role.push_str(role);
                }
            })
            .or_insert(view);
    }
    if project.director.member.is_none()
        && !project
            .archived_agents
            .iter()
            .any(|m| m.agent.cutex_session_id == project.director.cutex_session_id)
    {
        let id = project.director.cutex_session_id.as_str().to_owned();
        members
            .entry(SubjectRef::Managed(id.clone()))
            .and_modify(|view| {
                view.role = format!("Director/{}", view.role);
            })
            .or_insert(AgentSessionView {
                subject: SubjectRef::Managed(id.clone()),
                name: format!("Unavailable ({id})"),
                native_title: None,
                native_thread: None,
                native_workspace: None,
                runtime: Observation::Unavailable("Director member projection missing".into()),
                project: Observation::Known(project.project_id.to_string()),
                configured_profile: None,
                effective_profile: Observation::Unavailable(
                    "Director member projection missing".into(),
                ),
                role: "Director".into(),
                activity: "—".into(),
                updated: "—".into(),
                cwd: "Unavailable".into(),
                retirement_note: None,
            });
    }
    let mut rows: Vec<_> = members.into_values().collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name).then(a.subject.cmp(&b.subject)));
    rows
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ListKind {
    Managed,
    Recent,
    Members,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Column {
    Name,
    Status,
    Role,
    Activity,
    Project,
    Profile,
    Updated,
}
impl Column {
    fn label(self) -> &'static str {
        match self {
            Self::Name => "NAME",
            Self::Status => "STATUS",
            Self::Role => "ROLE",
            Self::Activity => "ACTIVITY",
            Self::Project => "PROJECT",
            Self::Profile => "PROFILE",
            Self::Updated => "RECENCY",
        }
    }
    fn value(self, row: &AgentSessionView) -> String {
        match self {
            Self::Name => row.name.clone(),
            Self::Status => row.runtime.label(),
            Self::Role => row.role.clone(),
            Self::Activity => row.activity.clone(),
            Self::Project => row.project.label(),
            Self::Profile => row
                .configured_profile
                .clone()
                .unwrap_or_else(|| "inherit (config)".into()),
            Self::Updated => row.updated.clone(),
        }
    }
}
pub(super) fn visible_columns(width: u16, kind: ListKind) -> Vec<(Column, u16)> {
    let mut columns = vec![(Column::Status, 11)];
    if kind == ListKind::Members && width >= 36 {
        columns.push((Column::Role, 16));
    }
    if kind == ListKind::Recent && width >= 44 {
        columns.push((Column::Updated, 16));
    }
    if width >= 66 {
        columns.push((Column::Project, 18));
    }
    if width >= 86 {
        columns.push((Column::Profile, 16));
    }
    if kind != ListKind::Recent && width >= 104 {
        columns.push((Column::Activity, 12));
    }
    let used: u16 = columns.iter().map(|(_, w)| w + 1).sum();
    let name = width.saturating_sub(used).max(1);
    if width < 18 {
        return vec![(Column::Name, width.max(1))];
    }
    columns.insert(0, (Column::Name, name));
    columns
}

pub(super) fn clipped(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.into();
    }
    if width == 0 {
        return String::new();
    }
    let mut result = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > width - 1 {
            break;
        }
        result.push(ch);
        used += w;
    }
    result.push('…');
    result
}
pub(super) fn tail_path(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.into();
    }
    if width < 2 {
        return clipped(text, width);
    }
    let mut result = Vec::new();
    let mut used = 1;
    for ch in text.chars().rev() {
        let w = ch.width().unwrap_or(0);
        if used + w > width {
            break;
        }
        result.push(ch);
        used += w;
    }
    format!("…{}", result.into_iter().rev().collect::<String>())
}
pub(super) fn render_table(
    frame: &mut Frame<'_>,
    area: Rect,
    rows: &[AgentSessionView],
    kind: ListKind,
    state: &mut TableState,
) {
    let block = Block::bordered();
    let inner = block.inner(area);
    let columns = visible_columns(inner.width.saturating_sub(2), kind);
    let table = Table::new(
        rows.iter().map(|row| {
            Row::new(
                columns
                    .iter()
                    .map(|(c, w)| Cell::from(clipped(&c.value(row), usize::from(*w)))),
            )
        }),
        columns.iter().map(|(_, w)| Constraint::Length(*w)),
    )
    .header(
        Row::new(columns.iter().map(|(c, _)| c.label()))
            .style(Style::new().add_modifier(Modifier::BOLD)),
    )
    .column_spacing(1)
    .block(block)
    .highlight_symbol("> ")
    .row_highlight_style(
        Style::new()
            .fg(super::session_tui_layout::TEXT)
            .bg(super::session_tui_layout::SELECTION),
    );
    frame.render_stateful_widget(table, area, state);
    if rows.is_empty() && inner.height > 1 {
        frame.render_widget(
            Paragraph::new("No rows in this scope / filter"),
            Rect {
                y: inner.y + 1,
                height: inner.height - 1,
                ..inner
            },
        );
    }
}
pub(super) fn render_inspector(frame: &mut Frame<'_>, area: Rect, row: &AgentSessionView) {
    let mut lines = vec![
        format!("Name: {}", row.name),
        format!("Identity: {:?}", row.subject),
        format!("Status: {}", row.runtime.label()),
        format!("Cutex Project: {}", row.project.label()),
        format!("Role: {}", row.role),
        format!(
            "Configured profile: {}",
            row.configured_profile.as_deref().unwrap_or("inherit")
        ),
        format!("Effective profile: {}", row.effective_profile.label()),
        format!(
            "Native workspace: {}",
            row.native_workspace.as_deref().unwrap_or("not observed")
        ),
        format!(
            "Native title: {}",
            row.native_title.as_deref().unwrap_or("not observed")
        ),
        format!(
            "Path: {}",
            tail_path(&row.cwd, usize::from(area.width.saturating_sub(8)))
        ),
        format!("Full path: {}", row.cwd),
    ];
    for observation in [&row.runtime, &row.project, &row.effective_profile] {
        match observation {
            Observation::Unavailable(reason) | Observation::Stale(_, reason) => {
                lines.push(reason.clone())
            }
            _ => {}
        }
    }
    if let Some(note) = &row.retirement_note {
        lines.push(note.clone());
    }
    lines.push("Read-only inspection · Esc returns · lifecycle actions deferred".into());
    frame.render_widget(
        Paragraph::new(lines.into_iter().map(Line::from).collect::<Vec<_>>())
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(" Inspector ")),
        area,
    );
}
