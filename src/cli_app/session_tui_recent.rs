//! Native recent-thread catalog workspace for the session TUI.
//!
//! The catalog worker owns its app-server connection for the lifetime of one
//! TUI cycle.  It deliberately never inspects Codex provider storage.

use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;

use cutex::catalog::{
    CatalogClient, CatalogError, CatalogThread, SortDirection, ThreadListParams, ThreadPage,
    ThreadSortKey,
};
use cutex::session::model::CutexSessionStore;
use cutex::session::service::cutex_session_is_managed;

const PAGE_SIZE: u32 = 50;
const MAX_DISPLAY_TEXT: usize = 160;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RecentThreadState {
    Unmanaged,
    Managed,
    Retired,
    MissingCwd,
}

impl RecentThreadState {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Unmanaged => "unmanaged",
            Self::Managed => "managed",
            Self::Retired => "retired",
            Self::MissingCwd => "cwd unavailable",
        }
    }

    pub(super) fn can_adopt(self) -> bool {
        matches!(self, Self::Unmanaged)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RecentThreadRow {
    /// The native `thread/list` id. This is the only identity used for
    /// adoption; `session_id` is intentionally not retained as an identity.
    pub(super) thread_id: String,
    pub(super) title: String,
    pub(super) cwd: Option<String>,
    pub(super) provider: String,
    pub(super) source: String,
    pub(super) project_id: Option<String>,
    pub(super) recency_at: Option<i64>,
    pub(super) state: RecentThreadState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RecentLoadState {
    Loading,
    Ready,
    Empty,
    ProviderIncompatible(String),
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RecentCommand {
    LoadMore,
    Retry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RecentAdoptionRequest {
    pub(super) thread_id: String,
    pub(super) title: String,
    pub(super) cwd: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AdoptionReview {
    thread_id: String,
    confirmed: bool,
}

#[derive(Debug)]
enum CatalogCommand {
    Load { cursor: Option<String>, retry: bool },
}

#[derive(Debug)]
pub(super) enum CatalogReply {
    Page {
        cursor: Option<String>,
        result: Result<ThreadPage, CatalogError>,
    },
}

/// One asynchronous, serial catalog connection. `Drop` closes the command
/// sender, allowing the worker and its owned app-server process to exit.
#[derive(Debug)]
pub(super) struct RecentCatalog {
    commands: Sender<CatalogCommand>,
    replies: Receiver<CatalogReply>,
}

impl RecentCatalog {
    pub(super) fn spawn() -> anyhow::Result<Self> {
        let (commands, command_receiver) = mpsc::channel();
        let (reply_sender, replies) = mpsc::channel();
        thread::Builder::new()
            .name("cutex-tui-recent-catalog".to_string())
            .spawn(move || catalog_worker(command_receiver, reply_sender))
            .map_err(|error| anyhow::anyhow!("failed to start recent catalog worker: {error}"))?;
        commands
            .send(CatalogCommand::Load {
                cursor: None,
                retry: false,
            })
            .map_err(|_| anyhow::anyhow!("recent catalog worker stopped before loading"))?;
        Ok(Self { commands, replies })
    }

    pub(super) fn request(&self, command: RecentCommand, cursor: Option<String>) -> bool {
        self.commands
            .send(CatalogCommand::Load {
                cursor,
                retry: command == RecentCommand::Retry,
            })
            .is_ok()
    }

    pub(super) fn poll(&self) -> Option<CatalogReply> {
        match self.replies.try_recv() {
            Ok(reply) => Some(reply),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }
}

fn catalog_worker(commands: Receiver<CatalogCommand>, replies: Sender<CatalogReply>) {
    let mut client = CatalogClient::spawn_local();
    while let Ok(CatalogCommand::Load { cursor, retry }) = commands.recv() {
        if retry && client.is_err() {
            client = CatalogClient::spawn_local();
        }
        let result = match &mut client {
            Ok(client) => client.thread_list(thread_list_params(cursor.clone())),
            Err(error) => Err(error.clone()),
        };
        // A connected app-server can still lose its transport or time out
        // while serving a page. Both leave the owned endpoint unusable, so an
        // explicit retry must reconnect it.
        if should_recreate_catalog_client_after(&result) {
            let error = result.as_ref().expect_err("transport result is an error");
            client = Err(error.clone());
        }
        if replies.send(CatalogReply::Page { cursor, result }).is_err() {
            break;
        }
    }
}

fn should_recreate_catalog_client_after(result: &Result<ThreadPage, CatalogError>) -> bool {
    matches!(
        result,
        Err(CatalogError::Transport(_)) | Err(CatalogError::Timeout { .. })
    )
}

fn thread_list_params(cursor: Option<String>) -> ThreadListParams {
    ThreadListParams {
        cursor,
        limit: Some(PAGE_SIZE),
        sort_key: Some(ThreadSortKey::RecencyAt),
        sort_direction: Some(SortDirection::Desc),
        ..ThreadListParams::default()
    }
}

#[derive(Debug, Clone)]
pub(super) struct RecentSessionsWorkspace {
    rows: Vec<RecentThreadRow>,
    selected: usize,
    next_cursor: Option<String>,
    failed_cursor: Option<String>,
    loading: bool,
    load_state: RecentLoadState,
    review: Option<AdoptionReview>,
}

impl Default for RecentSessionsWorkspace {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            selected: 0,
            next_cursor: None,
            failed_cursor: None,
            loading: true,
            load_state: RecentLoadState::Loading,
            review: None,
        }
    }
}

impl RecentSessionsWorkspace {
    pub(super) fn rows(&self) -> &[RecentThreadRow] {
        &self.rows
    }
    pub(super) fn selected(&self) -> usize {
        self.selected
    }
    pub(super) fn loading(&self) -> bool {
        self.loading
    }
    pub(super) fn load_state(&self) -> &RecentLoadState {
        &self.load_state
    }
    pub(super) fn next_cursor(&self) -> Option<String> {
        self.next_cursor.clone()
    }
    pub(super) fn cursor_for(&self, command: RecentCommand) -> Option<String> {
        match command {
            RecentCommand::LoadMore => self.next_cursor.clone(),
            RecentCommand::Retry => self.failed_cursor.clone(),
        }
    }
    pub(super) fn mark_loading(&mut self) {
        self.loading = true;
        self.load_state = RecentLoadState::Loading;
    }
    pub(super) fn review(&self) -> Option<&RecentThreadRow> {
        let review = self.review.as_ref()?;
        self.rows
            .iter()
            .find(|row| row.thread_id == review.thread_id)
    }
    pub(super) fn review_confirmed(&self) -> bool {
        self.review.as_ref().is_some_and(|review| review.confirmed)
    }

    pub(super) fn receive(&mut self, reply: CatalogReply, store: &CutexSessionStore) {
        let CatalogReply::Page { cursor, result } = reply;
        self.loading = false;
        match result {
            Ok(page) => {
                self.failed_cursor = None;
                let append = cursor.is_some();
                let mut incoming = page
                    .data
                    .into_iter()
                    .map(|thread| recent_row(thread, store))
                    .collect::<Vec<_>>();
                incoming.sort_by(|left, right| {
                    right
                        .recency_at
                        .cmp(&left.recency_at)
                        .then_with(|| left.thread_id.cmp(&right.thread_id))
                });
                if append {
                    let known = self
                        .rows
                        .iter()
                        .map(|row| row.thread_id.clone())
                        .collect::<HashSet<_>>();
                    self.rows.extend(
                        incoming
                            .into_iter()
                            .filter(|row| !known.contains(&row.thread_id)),
                    );
                } else {
                    let selected_id = self
                        .rows
                        .get(self.selected)
                        .map(|row| row.thread_id.clone());
                    self.rows = incoming;
                    self.selected = selected_id
                        .and_then(|id| self.rows.iter().position(|row| row.thread_id == id))
                        .unwrap_or(0);
                }
                self.rows.sort_by(|left, right| {
                    right
                        .recency_at
                        .cmp(&left.recency_at)
                        .then_with(|| left.thread_id.cmp(&right.thread_id))
                });
                self.next_cursor = page.next_cursor;
                self.load_state = if self.rows.is_empty() {
                    RecentLoadState::Empty
                } else {
                    RecentLoadState::Ready
                };
                self.selected = self.selected.min(self.rows.len().saturating_sub(1));
            }
            Err(CatalogError::ProviderIncompatible(message)) => {
                self.failed_cursor = cursor;
                self.load_state = RecentLoadState::ProviderIncompatible(bound(&message));
            }
            Err(error) => {
                self.failed_cursor = cursor;
                self.load_state = RecentLoadState::Failed(bound(&error.to_string()));
            }
        }
    }

    /// Catalog data is never applied without a read-only durable-store
    /// reconciliation. A store read failure must leave the UI retryable rather
    /// than indefinitely showing its previous loading state.
    pub(super) fn reconciliation_failed(&mut self, reply: CatalogReply, message: String) {
        let CatalogReply::Page { cursor, .. } = reply;
        self.loading = false;
        self.failed_cursor = cursor;
        self.load_state = RecentLoadState::Failed(bound(&message));
    }

    pub(super) fn reproject(&mut self, store: &CutexSessionStore) {
        for row in &mut self.rows {
            row.state = thread_state(&row.thread_id, row.cwd.is_some(), store);
        }
    }

    pub(super) fn move_selection(&mut self, direction: isize) {
        if self.rows.is_empty() {
            return;
        }
        self.selected = if direction < 0 {
            if self.selected == 0 {
                self.rows.len() - 1
            } else {
                self.selected - 1
            }
        } else {
            (self.selected + 1) % self.rows.len()
        };
    }

    pub(super) fn select_edge(&mut self, last: bool) {
        if !self.rows.is_empty() {
            self.selected = if last { self.rows.len() - 1 } else { 0 };
        }
    }

    pub(super) fn begin_review(&mut self) -> bool {
        let Some(row) = self.rows.get(self.selected) else {
            return false;
        };
        if !row.state.can_adopt() {
            return false;
        }
        self.review = Some(AdoptionReview {
            thread_id: row.thread_id.clone(),
            confirmed: false,
        });
        true
    }

    pub(super) fn set_review_confirmed(&mut self, confirmed: bool) {
        if let Some(review) = &mut self.review {
            review.confirmed = confirmed;
        }
    }

    pub(super) fn cancel_review(&mut self) {
        self.review = None;
    }

    pub(super) fn adoption_request(&self) -> Option<RecentAdoptionRequest> {
        let review = self.review.as_ref()?;
        if !review.confirmed {
            return None;
        }
        let row = self
            .rows
            .iter()
            .find(|row| row.thread_id == review.thread_id)?;
        (row.state == RecentThreadState::Unmanaged).then(|| RecentAdoptionRequest {
            thread_id: row.thread_id.clone(),
            title: row.title.clone(),
            cwd: row.cwd.clone().expect("unmanaged native thread has cwd"),
        })
    }

    pub(super) fn adoption_succeeded(&mut self, store: &CutexSessionStore) {
        self.reproject(store);
        self.review = None;
    }
}

fn recent_row(thread: CatalogThread, store: &CutexSessionStore) -> RecentThreadRow {
    let cwd = thread
        .cwd
        .map(|path| path.display().to_string())
        .filter(|cwd| !cwd.trim().is_empty());
    let title = thread
        .name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(thread.preview);
    let provider = if thread.model_provider.trim().is_empty() {
        "native".to_string()
    } else {
        bound(&thread.model_provider)
    };
    let source = thread
        .source
        .as_str()
        .map(bound)
        .unwrap_or_else(|| "native".to_string());
    let state = thread_state(&thread.id, cwd.is_some(), store);
    RecentThreadRow {
        thread_id: thread.id,
        title: bound(&title),
        cwd,
        provider,
        source,
        project_id: thread.project_id.map(|project| bound(&project)),
        recency_at: thread
            .recency_at
            .or(thread.updated_at)
            .or(thread.created_at),
        state,
    }
}

fn thread_state(thread_id: &str, has_cwd: bool, store: &CutexSessionStore) -> RecentThreadState {
    if store
        .sessions
        .values()
        .any(|record| record.codex_session_id.as_deref() == Some(thread_id) && record.is_retired())
    {
        RecentThreadState::Retired
    } else if store.sessions.values().any(|record| {
        record.codex_session_id.as_deref() == Some(thread_id) && cutex_session_is_managed(record)
    }) {
        RecentThreadState::Managed
    } else if !has_cwd {
        RecentThreadState::MissingCwd
    } else {
        RecentThreadState::Unmanaged
    }
}

fn bound(value: &str) -> String {
    let mut text = value
        .trim()
        .chars()
        .take(MAX_DISPLAY_TEXT)
        .collect::<String>();
    if value.trim().chars().count() > MAX_DISPLAY_TEXT {
        text.push('…');
    }
    if text.is_empty() {
        "(unnamed thread)".to_string()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cutex::agent_bus::model::AgentRegistrationClass;
    use cutex::session::model::CutexSessionRecord;
    use serde_json::json;

    fn thread(id: &str, session_id: &str, recency: i64) -> CatalogThread {
        CatalogThread {
            id: id.to_string(),
            session_id: session_id.to_string(),
            project_id: Some("project-a".to_string()),
            parent_thread_id: None,
            preview: "a bounded preview".to_string(),
            model_provider: "openai".to_string(),
            created_at: Some(recency),
            updated_at: Some(recency),
            recency_at: Some(recency),
            cwd: Some("/work".into()),
            name: None,
            status: json!({}),
            source: json!("cli"),
            additional_fields: Default::default(),
        }
    }

    fn store_with(id: &str, managed: bool, retired: bool) -> CutexSessionStore {
        let mut store = CutexSessionStore::default();
        let mut record = CutexSessionRecord::new(
            "cutex-test".to_string(),
            Some(id.to_string()),
            "host".to_string(),
            "/work".to_string(),
            None,
        )
        .unwrap();
        record.registration_class = if managed {
            AgentRegistrationClass::Persistent
        } else {
            AgentRegistrationClass::LocalOnly
        };
        if retired {
            record.archive_state = cutex::session::model::CutexSessionArchiveState::Retired;
        }
        store.sessions.insert("cutex-test".to_string(), record);
        store
    }

    #[test]
    fn native_thread_id_not_tree_session_id_controls_duplicate_detection() {
        let store = store_with("thread-1", true, false);
        assert_eq!(
            thread_state("thread-1", true, &store),
            RecentThreadState::Managed
        );
        assert_eq!(
            thread_state("tree-session-1", true, &store),
            RecentThreadState::Unmanaged
        );
    }

    #[test]
    fn retired_native_thread_cannot_be_adopted() {
        let store = store_with("thread-1", true, true);
        assert_eq!(
            thread_state("thread-1", true, &store),
            RecentThreadState::Retired
        );
    }

    #[test]
    fn page_is_sorted_by_native_recency_and_deduplicated_on_load_more() {
        let mut workspace = RecentSessionsWorkspace::default();
        workspace.receive(
            CatalogReply::Page {
                cursor: None,
                result: Ok(ThreadPage {
                    data: vec![thread("older", "tree-a", 1), thread("newer", "tree-b", 2)],
                    next_cursor: Some("next".to_string()),
                    backwards_cursor: None,
                }),
            },
            &CutexSessionStore::default(),
        );
        assert_eq!(workspace.rows[0].thread_id, "newer");
        workspace.receive(
            CatalogReply::Page {
                cursor: Some("next".to_string()),
                result: Ok(ThreadPage {
                    data: vec![
                        thread("newer", "other-tree", 2),
                        thread("oldest", "tree-c", 0),
                    ],
                    next_cursor: None,
                    backwards_cursor: None,
                }),
            },
            &CutexSessionStore::default(),
        );
        assert_eq!(workspace.rows.len(), 3);
    }

    #[test]
    fn adoption_requires_an_explicit_confirmation_and_escape_is_safe() {
        let mut workspace = RecentSessionsWorkspace::default();
        workspace.receive(
            CatalogReply::Page {
                cursor: None,
                result: Ok(ThreadPage {
                    data: vec![thread("thread-1", "tree", 1)],
                    next_cursor: None,
                    backwards_cursor: None,
                }),
            },
            &CutexSessionStore::default(),
        );
        assert!(workspace.begin_review());
        assert!(workspace.adoption_request().is_none());
        workspace.cancel_review();
        assert!(workspace.adoption_request().is_none());
        assert!(workspace.begin_review());
        workspace.set_review_confirmed(true);
        assert_eq!(workspace.adoption_request().unwrap().thread_id, "thread-1");
    }

    #[test]
    fn thread_list_requests_native_recency_descending_in_bounded_pages() {
        let params = thread_list_params(None);
        assert_eq!(params.limit, Some(PAGE_SIZE));
        assert_eq!(params.sort_key, Some(ThreadSortKey::RecencyAt));
        assert_eq!(params.sort_direction, Some(SortDirection::Desc));
    }

    #[test]
    fn native_cwd_is_retained_exactly_while_empty_cwd_is_not_adoptable() {
        let long_cwd = format!("/work/{}", "segment".repeat(80));
        let mut long_thread = thread("thread-1", "tree", 1);
        long_thread.cwd = Some(long_cwd.clone().into());
        let row = recent_row(long_thread, &CutexSessionStore::default());
        assert_eq!(row.cwd.as_deref(), Some(long_cwd.as_str()));

        let mut workspace = RecentSessionsWorkspace::default();
        workspace.rows = vec![row];
        assert!(workspace.begin_review());
        workspace.set_review_confirmed(true);
        assert_eq!(workspace.adoption_request().unwrap().cwd, long_cwd);

        let mut empty_thread = thread("thread-2", "tree", 1);
        empty_thread.cwd = Some("".into());
        let empty = recent_row(empty_thread, &CutexSessionStore::default());
        assert_eq!(empty.cwd, None);
        assert_eq!(empty.state, RecentThreadState::MissingCwd);
    }

    #[test]
    fn transport_failure_from_a_connected_catalog_requires_client_recreation() {
        let result = Err(CatalogError::Transport("connection closed".to_string()));
        assert!(should_recreate_catalog_client_after(&result));
        let timeout = Err(CatalogError::Timeout {
            method: "thread/list".to_string(),
        });
        assert!(should_recreate_catalog_client_after(&timeout));
        let other = Err(CatalogError::Protocol("bad response".to_string()));
        assert!(!should_recreate_catalog_client_after(&other));
    }

    #[test]
    fn store_reconciliation_failure_leaves_a_non_loading_retryable_state() {
        let mut workspace = RecentSessionsWorkspace::default();
        workspace.reconciliation_failed(
            CatalogReply::Page {
                cursor: Some("next-page".to_string()),
                result: Ok(ThreadPage {
                    data: vec![thread("thread-1", "tree", 1)],
                    next_cursor: None,
                    backwards_cursor: None,
                }),
            },
            "durable store unavailable".to_string(),
        );
        assert!(!workspace.loading());
        assert_eq!(
            workspace.cursor_for(RecentCommand::Retry).as_deref(),
            Some("next-page")
        );
        assert!(
            matches!(workspace.load_state(), RecentLoadState::Failed(message) if message.contains("durable store unavailable"))
        );
    }
}
