//! Human history editing. Native APIs own the mutation; Cutex retains identity.
use super::session_tui_layout as theme;
use anyhow::{ensure, Context};
use cutex::app_server::client::{AppServerClient, AppServerClientOptions};
#[cfg(all(test, target_os = "linux"))]
use cutex::app_server::client::AppServerEndpoint;
use cutex::catalog::{CatalogEndpoint, OwnedStdioEndpoint};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug)]
pub(super) struct Turn {
    pub id: String,
    pub text: String,
    pub time: String,
    pub active: bool,
    pub legacy_input_turn: bool,
}
#[derive(Clone, Debug)]
pub(super) struct History {
    pub thread: String,
    pub turns: Vec<Turn>, // newest first, ordered by provider (not timestamps)
    pub paginated: bool,
    pub path: PathBuf,
}
enum Connection {
    Live(AppServerClient),
    Isolated(OwnedStdioEndpoint),
}
impl Connection {
    fn request(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        match self {
            Self::Live(client) => Ok(client.handle().request(method, params)?),
            Self::Isolated(endpoint) => Ok(endpoint.request(method, params)?),
        }
    }
}
pub(super) struct Editor {
    connection: Connection,
    thread: String,
    home: PathBuf,
}
impl Editor {
    #[cfg(not(any(target_os = "linux", windows)))]
    pub(super) fn open(_id: &str) -> anyhow::Result<Self> {
        anyhow::bail!("History revert currently requires a local Linux runtime")
    }

    #[cfg(any(target_os = "linux", windows))]
    pub(super) fn open(id: &str) -> anyhow::Result<Self> {
        let store = cutex::session::store::load_cutex_session_store()?;
        let record = store.sessions.get(id).or_else(|| {
            store
                .sessions
                .values()
                .find(|r| r.codex_session_id.as_deref() == Some(id))
        });
        let thread = record
            .and_then(|r| r.codex_session_id.clone())
            .unwrap_or_else(|| id.into());
        ensure!(
            uuid::Uuid::parse_str(&thread)?.to_string() == thread,
            "exact native thread ID required"
        );
        let home = match record.and_then(|r| r.explicit_launch.as_ref()) {
            Some(contract) => contract.native_home.clone(),
            None => cutex::launch::local_deployment::LocalDeployment::source_home()?,
        };
        if let Some(record) = record {
            ensure!(!record.is_retired(), "retired history is read-only");
        }
        let connection = if let Some(binding) = record.and_then(|r| r.app_server_runtime.as_ref()) {
            let endpoint = cutex::app_server::runtime::endpoint_from_runtime_binding(binding)?;
            let mut options = AppServerClientOptions::new(endpoint);
            options.request_timeout = Duration::from_secs(120);
            Connection::Live(AppServerClient::connect(options)?)
        } else if let Some((record, contract)) =
            record.and_then(|r| r.explicit_launch.as_ref().map(|c| (r, c)))
        {
            let bundle = cutex::launch::stock::StockBundle::load_running(contract)?;
            let profile = cutex::launch::stock::current_configuration(record)?;
            let launch = super::stock_lifecycle::configured(
                super::stock_lifecycle::clean_launch(&bundle.executable.path, &home)?,
                &profile,
                true,
                matches!(contract.version, 3 | 4).then_some(home.as_path()),
            )?
            .args(["--listen", "stdio://"]);
            let mut command = launch.to_command();
            command.current_dir(&record.cwd);
            let options =
                cutex::catalog::StdioAppServerOptions::new(&bundle.executable.path, home.clone());
            let mut endpoint = OwnedStdioEndpoint::spawn_command(options, command)?;
            let initialized = endpoint.request("initialize", json!({"clientInfo":{"name":"cutex_history","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}))?;
            ensure!(
                initialized["codexHome"].as_str() == home.to_str(),
                "native history home mismatch"
            );
            endpoint.notify("initialized", None)?;
            Connection::Isolated(endpoint)
        } else {
            let cwd = record
                .map(|r| PathBuf::from(&r.cwd))
                .unwrap_or(std::env::current_dir()?);
            let launch = super::session_native_workflow::NativeLaunch {
                cwd,
                native_home: home.clone(),
                profile: record.and_then(|r| r.profile.clone()),
                model: None,
            };
            Connection::Isolated(launch.endpoint()?)
        };
        Ok(Self {
            connection,
            thread,
            home,
        })
    }
    pub(super) fn read(&mut self) -> anyhow::Result<History> {
        let metadata = self.connection.request(
            "thread/read",
            json!({"threadId":self.thread,"includeTurns":false}),
        )?;
        let thread = metadata.get("thread").context("thread metadata missing")?;
        ensure!(
            thread["id"].as_str() == Some(&self.thread),
            "thread identity changed"
        );
        let paginated = thread["historyMode"].as_str() == Some("paginated");
        let path = thread["path"]
            .as_str()
            .context("native history path is unavailable")?
            .into();
        let mut turns = Vec::new();
        // 0.154 supports bounded turn summaries for legacy history as well.
        // Never hydrate command output for a history-selection screen.
        let mut cursor: Option<String> = None;
        let mut seen = std::collections::BTreeSet::new();
        loop {
            let page = self.connection.request("thread/turns/list", json!({"threadId":self.thread,"sortDirection":"desc","limit":100,"itemsView":"summary","cursor":cursor}))?;
            for turn in page["data"].as_array().context("turn page missing")? {
                turns.push(parse_turn(turn)?);
            }
            cursor = page["nextCursor"].as_str().map(str::to_owned);
            let Some(next) = &cursor else {
                break;
            };
            ensure!(
                seen.insert(next.clone()),
                "native history pagination did not advance"
            );
        }
        let mut unique = std::collections::BTreeSet::new();
        ensure!(
            turns.iter().all(|t| unique.insert(t.id.clone())),
            "duplicate native turn IDs"
        );
        Ok(History {
            thread: self.thread.clone(),
            turns,
            paginated,
            path,
        })
    }
    pub(super) fn revert(&mut self, reviewed: &History, keep: &str) -> anyhow::Result<String> {
        let mut current = self.read()?;
        ensure!(
            current.thread == reviewed.thread && ids(&current) == ids(reviewed),
            "history changed; reload and choose the retained turn again"
        );
        let remove = current
            .turns
            .iter()
            .position(|t| t.id == keep)
            .context("selected turn disappeared")?;
        ensure!(
            remove > 0,
            "this is already the newest turn; nothing to remove"
        );
        ensure!(
            !current.turns[remove].active,
            "cannot retain an unfinished turn"
        );
        ensure!(current.paginated || current.turns[..remove].iter().all(|t| t.legacy_input_turn),
            "this legacy range includes non-user turns; native rollback counts user turns and cannot safely express this boundary. No history was changed");
        if let Connection::Isolated(endpoint) = &self.connection {
            ensure!(!has_writer(&current.path, endpoint.child_id())?, "this session is open in another native runtime; disconnect/close it before reverting");
            self.connection.request(
                "thread/resume",
                json!({"threadId":self.thread,"deferGoalContinuation":true}),
            )?;
        }
        if let Some(active) = current.turns.iter().find(|t| t.active) {
            self.connection.request(
                "turn/interrupt",
                json!({"threadId":self.thread,"turnId":active.id}),
            )?;
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            loop {
                let latest = self.read()?;
                ensure!(
                    ids(&latest) == ids(&current),
                    "history changed while stopping; reload before reverting"
                );
                if !latest.turns.iter().any(|t| t.active) {
                    current = latest;
                    break;
                }
                ensure!(
                    std::time::Instant::now() < deadline,
                    "current turn has not stopped; history was not reverted"
                );
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        // Re-read after stopping and validate the exact source before backing up.
        let metadata = self.connection.request(
            "thread/read",
            json!({"threadId":self.thread,"includeTurns":false}),
        )?;
        let source = PathBuf::from(
            metadata["thread"]["path"]
                .as_str()
                .context("native history path missing")?,
        )
        .canonicalize()?;
        ensure!(
            source.starts_with(self.home.canonicalize()?),
            "history path is outside this session's home"
        );
        let action = format!("revert-{}", uuid::Uuid::new_v4());
        let directory = cutex::config::paths::runtime_dir()?
            .join("history-reverts")
            .join(&action);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&directory)?;
        }
        #[cfg(windows)]
        {
            std::fs::create_dir_all(&directory)?;
            cutex::platform::private_fs::secure_directory(&directory)?;
        }
        #[cfg(not(any(unix, windows)))]
        std::fs::create_dir_all(&directory)?;
        let backup = directory.join("before.jsonl");
        let sources = cutex::launch::native_history::recovery_sources(&self.home, &source)?;
        let mut copies = Vec::new();
        for (index, path) in sources.iter().enumerate() {
            let target = if index == 0 {
                backup.clone()
            } else {
                directory.join(format!("ancestor-{index}.jsonl"))
            };
            let size = std::fs::metadata(path)?.len();
            let copied = std::fs::copy(path, &target)
                .context("could not save recovery copy; history unchanged")?;
            ensure!(
                copied == size && std::fs::metadata(path)?.len() == size,
                "history changed during backup; reload before reverting"
            );
            std::fs::File::open(&target)?.sync_all()?;
            copies.push(json!({"source":path,"backup":target}));
        }
        let latest = self.read()?;
        ensure!(
            ids(&latest) == ids(&current) && !latest.turns.iter().any(|t| t.active),
            "history changed while preparing the recovery copy; reload before reverting"
        );
        let receipt = directory.join("receipt.json");
        let mut facts = json!({"actionId":action,"threadId":self.thread,"keepTurnId":keep,"beforeTurnId":current.turns[remove-1].id,"removedTurns":remove,"source":source,"backup":backup,"sources":copies,"status":"prepared"});
        cutex::config::atomic::write_private_pretty_json_atomic(
            &receipt,
            &facts,
            "history revert",
        )?;
        let result = if current.paginated {
            self.connection.request(
                "thread/revert",
                json!({"threadId":self.thread,"beforeTurnId":current.turns[remove-1].id}),
            )
        } else {
            self.connection.request(
                "thread/rollback",
                json!({"threadId":self.thread,"numTurns":remove}),
            )
        };
        if let Err(error) = result {
            facts["status"] = json!("outcome_unknown");
            facts["error"] = json!(error.to_string());
            cutex::config::atomic::write_private_pretty_json_atomic(
                &receipt,
                &facts,
                "history revert",
            )?;
            anyhow::bail!("Revert not confirmed: {error}; recovery record: {}. Reload history before retrying.", receipt.display());
        }
        let after = self.read()?;
        ensure!(
            ids(&after) == ids(&current)[remove..],
            "revert response received but retained history differs; inspect {}",
            receipt.display()
        );
        facts["status"] = json!("complete");
        facts["resultPath"] = json!(after.path);
        cutex::config::atomic::write_private_pretty_json_atomic(
            &receipt,
            &facts,
            "history revert",
        )?;
        Ok(format!(
            "Removed {remove} newer turn(s); same session identity. Recovery copy: {}",
            backup.display()
        ))
    }
}
fn filtered_turns(history: &History, filter: &str) -> Vec<usize> {
    let filter = filter.to_lowercase();
    history
        .turns
        .iter()
        .enumerate()
        .filter(|(_, t)| {
            filter.is_empty()
                || format!("{} {} {}", t.time, t.text, t.id)
                    .to_lowercase()
                    .contains(&filter)
        })
        .map(|(index, _)| index)
        .collect()
}

fn ids(history: &History) -> Vec<String> {
    history.turns.iter().map(|t| t.id.clone()).collect()
}
fn parse_turn(value: &Value) -> anyhow::Result<Turn> {
    let mut text = Vec::new();
    for item in value["items"].as_array().into_iter().flatten() {
        match item["type"].as_str() {
            Some("userMessage") => {
                let parts: Vec<_> = item["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c["text"].as_str())
                    .collect();
                text.push(format!("You: {}", parts.join(" ")));
            }
            Some("agentMessage") => {
                if let Some(s) = item["text"].as_str() {
                    text.push(format!("Agent: {s}"));
                }
            }
            _ => {}
        }
    }
    let time = value["startedAt"]
        .as_i64()
        .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "Time unavailable".into());
    Ok(Turn {
        id: value["id"].as_str().context("turn ID missing")?.into(),
        text: if text.is_empty() {
            "(No text summary)".into()
        } else {
            text.join("\n").chars().take(4000).collect()
        },
        time,
        active: value["status"].as_str() == Some("inProgress"),
        legacy_input_turn: value["items"]
            .as_array()
            .is_some_and(|items| items.iter().any(|i| i["type"] == "userMessage")),
    })
}
#[cfg(target_os = "linux")]
fn has_writer(path: &Path, own_pid: u32) -> anyhow::Result<bool> {
    let expected = path.canonicalize()?;
    for p in std::fs::read_dir("/proc")? {
        let p = p?;
        if p.file_name().to_string_lossy() == own_pid.to_string() {
            continue;
        }
        if !p
            .file_name()
            .to_string_lossy()
            .bytes()
            .all(|b| b.is_ascii_digit())
        {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(p.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            if std::fs::read_link(fd.path()).ok().as_ref() != Some(&expected) {
                continue;
            }
            let info = std::fs::read_to_string(p.path().join("fdinfo").join(fd.file_name()))?;
            if let Some(flags) = info.lines().find_map(|l| l.strip_prefix("flags:\t")) {
                if u32::from_str_radix(flags.trim(), 8)? & 3 != 0 {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}
#[cfg(not(target_os = "linux"))]
fn has_writer(_: &Path, _: u32) -> anyhow::Result<bool> {
    anyhow::bail!("Bring this session online before reverting history on this platform; isolated writer detection is unavailable")
}

pub(super) fn run(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    events: &mut super::session_tui::ShellEvents,
    id: String,
) -> anyhow::Result<Option<String>> {
    use crossterm::event::{Event, KeyCode, KeyEventKind};
    use ratatui::{
        layout::{Constraint, Layout},
        style::Style,
        text::{Line, Span},
        widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    };
    enum Command {
        Reload,
        Revert(History, String),
    }
    enum Reply {
        Loaded(History),
        Done(String),
        Failed(String),
    }
    let (send, receive) = std::sync::mpsc::channel::<Command>();
    let (reply, results) = std::sync::mpsc::channel::<Reply>();
    let title = id.clone();
    std::thread::spawn(move || {
        let mut editor = match Editor::open(&id) {
            Ok(e) => e,
            Err(e) => {
                let _ = reply.send(Reply::Failed(format!("{e:#}")));
                return;
            }
        };
        let load = |editor: &mut Editor| match editor.read() {
            Ok(h) => Reply::Loaded(h),
            Err(e) => Reply::Failed(format!("{e:#}")),
        };
        if reply.send(load(&mut editor)).is_err() {
            return;
        }
        while let Ok(command) = receive.recv() {
            let value = match command {
                Command::Reload => load(&mut editor),
                Command::Revert(h, keep) => match editor.revert(&h, &keep) {
                    Ok(s) => {
                        // Release the temporary owner before the user can resume.
                        drop(editor);
                        let _ = reply.send(Reply::Done(s));
                        return;
                    }
                    Err(e) => Reply::Failed(format!("{e:#}")),
                },
            };
            if reply.send(value).is_err() {
                return;
            }
        }
    });
    let mut history: Option<History> = None;
    let mut filter = String::new();
    let mut editing = false;
    let mut selected = 0usize;
    let mut confirmation: Option<usize> = None;
    let mut busy = true;
    let mut applying = false;
    let mut message = String::from("Loading history…");
    let mut list_state = ListState::default();
    loop {
        while let Ok(result) = results.try_recv() {
            busy = false;
            applying = false;
            match result {
                Reply::Loaded(h) => {
                    history = Some(h);
                    selected = 0;
                    confirmation = None;
                    message = "Choose the last turn to keep. Newest first.".into();
                }
                Reply::Done(s) => return Ok(Some(s)),
                Reply::Failed(s) => {
                    message = s;
                    confirmation = None;
                }
            }
        }
        let visible = history
            .as_ref()
            .map(|h| filtered_turns(h, &filter))
            .unwrap_or_default();
        selected = selected.min(visible.len().saturating_sub(1));
        list_state.select((!visible.is_empty()).then_some(selected));
        terminal.draw(|frame|{
            let area=frame.area();
            let regions=Layout::vertical([Constraint::Length(2),Constraint::Min(4),Constraint::Length(3),Constraint::Length(1)]).split(area);
            let columns=Layout::horizontal([Constraint::Percentage(52),Constraint::Percentage(48)]).split(regions[1]);
            frame.render_widget(Paragraph::new(format!("Revert history · {title}\nFilter{}: {filter}",if editing{" [typing]"}else{" [/ ]"})).style(Style::default().fg(theme::focus())),regions[0]);
            let items:Vec<ListItem>=visible.iter().map(|i|{
                let h=history.as_ref().unwrap();let t=&h.turns[*i];
                let summary=t.text.lines().next().unwrap_or("");
                ListItem::new(vec![Line::from(vec![Span::styled(format!("{}  ",t.time),Style::default().fg(theme::focus())),Span::raw(format!("Turn {}{}",h.turns.len()-i,if t.active{" · running"}else{""}))]),Line::raw(summary.chars().take(180).collect::<String>())])
            }).collect();
            frame.render_stateful_widget(List::new(items).block(Block::default().borders(Borders::ALL).title(" History · newest → oldest ")).highlight_style(Style::default().bg(theme::selection())).highlight_symbol("› "),columns[0],&mut list_state);
            let details=if let Some(i)=confirmation {
                let h=history.as_ref().unwrap();let t=&h.turns[i];
                format!("Keep through {}\n{}\n\nRemove {i} newer turns.\n{}\n\nThe session ID and Cutex Agent authority stay the same.\nFiles, sent messages, Tasks and Jobs will not be undone.\nA recovery copy is saved before editing.\n\nEnter: Confirm revert\nEsc: Back",t.time,t.text.lines().take(3).collect::<Vec<_>>().join("\n").chars().take(180).collect::<String>(),if h.turns.iter().any(|t|t.active){"The current turn will be stopped."}else{""})
            }else if let Some(i)=visible.get(selected){history.as_ref().unwrap().turns[*i].text.chars().take(4000).collect()}else{"No matching turns".into()};
            frame.render_widget(Paragraph::new(details).wrap(Wrap{trim:false}).block(Block::default().borders(Borders::ALL).title(if confirmation.is_some(){" Confirm revert "}else{" Details "})),columns[1]);
            frame.render_widget(Paragraph::new(&*message).wrap(Wrap{trim:true}).style(Style::default().fg(if busy{theme::accent()}else{theme::text()})),regions[2]);
            frame.render_widget(Paragraph::new(if busy{"Working…"}else if confirmation.is_some(){"Enter Confirm revert · Esc Cancel"}else{"↑/↓ Select · / Filter · Enter Keep through this turn · r Reload · Esc Back"}),regions[3]);
        })?;
        let Some(event) = events.next()? else {
            continue;
        };
        let Event::Key(key) = event else {
            continue;
        };
        if key.kind == KeyEventKind::Release
            || (key.code == KeyCode::Enter && key.kind != KeyEventKind::Press)
        {
            continue;
        }
        if busy {
            if key.code == KeyCode::Esc && !applying {
                return Ok(None);
            }
            continue;
        }
        if editing {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => editing = false,
                KeyCode::Backspace => {
                    filter.pop();
                    selected = 0;
                }
                KeyCode::Char(c) => {
                    filter.push(c);
                    selected = 0;
                }
                _ => {}
            }
            continue;
        }
        if let Some(i) = confirmation {
            match key.code {
                KeyCode::Esc => confirmation = None,
                KeyCode::Enter => {
                    let h = history.as_ref().unwrap();
                    send.send(Command::Revert(h.clone(), h.turns[i].id.clone()))?;
                    busy = true;
                    applying = true;
                    message="Stopping the current turn if needed, saving recovery copy, then reverting…".into();
                }
                _ => {}
            }
            continue;
        }
        match key.code {
            KeyCode::Esc => return Ok(None),
            KeyCode::Char('/') => editing = true,
            KeyCode::Char('r') => {
                send.send(Command::Reload)?;
                busy = true;
                message = "Reloading…".into();
            }
            KeyCode::Up => selected = selected.saturating_sub(1),
            KeyCode::Down => selected = (selected + 1).min(visible.len().saturating_sub(1)),
            KeyCode::PageDown => selected = (selected + 10).min(visible.len().saturating_sub(1)),
            KeyCode::PageUp => selected = selected.saturating_sub(10),
            KeyCode::Enter => {
                if let Some(i) = visible.get(selected) {
                    if *i == 0 {
                        message = "This is the newest turn; nothing to remove.".into();
                    } else if history.as_ref().unwrap().turns[*i].active {
                        message = "Choose a completed turn to retain.".into();
                    } else {
                        confirmation = Some(*i);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn summaries_keep_turn_ids_and_local_time_without_counting_messages_as_turns() {
        let turn=parse_turn(&json!({"id":"turn-one","status":"completed","startedAt":null,"items":[{"type":"userMessage","content":[{"type":"text","text":"问题 中文"}]},{"type":"commandExecution","command":"echo no"},{"type":"agentMessage","text":"Answer"}]})).unwrap();
        assert_eq!(turn.id, "turn-one");
        assert_eq!(turn.time, "Time unavailable");
        assert!(turn.text.contains("问题 中文") && turn.text.contains("Answer"));
        assert!(!turn.text.contains("echo no"));
    }
    #[test]
    fn filtering_preserves_newest_first_and_real_removal_boundary() {
        let h = History {
            thread: "t".into(),
            path: "/tmp/history".into(),
            paginated: false,
            turns: (0..5)
                .map(|i| Turn {
                    id: i.to_string(),
                    text: if i % 2 == 0 {
                        "中文 MATCH".into()
                    } else {
                        "hidden".into()
                    },
                    time: "Time unavailable".into(),
                    active: false,
                    legacy_input_turn: true,
                })
                .collect(),
        };
        assert_eq!(filtered_turns(&h, "match"), vec![0, 2, 4]);
        assert_eq!(filtered_turns(&h, "中文"), vec![0, 2, 4]);
        assert_eq!(filtered_turns(&h, "absent"), Vec::<usize>::new());
        assert_eq!(filtered_turns(&h, "match")[1], 2); // remove two turns, including the hidden one
    }
    #[test]
    #[ignore = "isolated native mock-model fixture required"]
    #[cfg(target_os = "linux")]
    fn actual_native_revert_preserves_identity_and_recovery_copy() {
        let fixture = PathBuf::from(std::env::var("CUTEX_HISTORY_FIXTURE").unwrap());
        assert!(fixture.join(".cutex-test-private-home").is_file());
        let data: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("fixture.json")).unwrap()).unwrap();
        let mut options = AppServerClientOptions::new(AppServerEndpoint::UnixSocket {
            socket_path: data["socket"].as_str().unwrap().into(),
        });
        options.request_timeout = Duration::from_secs(120);
        let mut editor = if std::env::var_os("CUTEX_HISTORY_OFFLINE").is_some() {
            Editor::open(data["thread"].as_str().unwrap()).unwrap()
        } else {
            Editor {
                connection: Connection::Live(AppServerClient::connect(options).unwrap()),
                thread: data["thread"].as_str().unwrap().into(),
                home: fixture.join(".cutex/codex-home"),
            }
        };
        let history = editor.read().unwrap();
        assert_eq!(history.turns.len(), 3);
        assert!(history.turns[0].text.contains("Third"));
        assert!(history.turns[2].text.contains("First"));
        let middle = history.turns[1].id.clone();
        editor.revert(&history, &middle).unwrap();
        assert!(editor.revert(&history, &middle).is_err());
        let history = editor.read().unwrap();
        let keep = history.turns[1].id.clone();
        let outcome = editor.revert(&history, &keep).unwrap();
        let after = editor.read().unwrap();
        assert_eq!(
            cutex::launch::native_history::current(&editor.home, &editor.thread).unwrap(),
            after.path.canonicalize().unwrap()
        );
        assert_eq!(after.thread, history.thread);
        assert_eq!(after.turns.len(), 1);
        assert_eq!(after.turns[0].id, keep);
        assert!(outcome.contains("Removed 1 newer turn(s)"));
        let receipts: Vec<_> = std::fs::read_dir(fixture.join(".cutex/runtime/history-reverts"))
            .unwrap()
            .collect();
        assert_eq!(receipts.len(), 2);
        let root = receipts[0].as_ref().unwrap().path();
        assert!(std::fs::metadata(root.join("before.jsonl")).unwrap().len() > 0);
        let receipt: Value =
            serde_json::from_slice(&std::fs::read(root.join("receipt.json")).unwrap()).unwrap();
        assert_eq!(receipt["status"], "complete");
        assert!(editor.revert(&history, &keep).is_err());
        let mut source_counts = receipts
            .iter()
            .map(|entry| {
                let receipt: Value = serde_json::from_slice(
                    &std::fs::read(entry.as_ref().unwrap().path().join("receipt.json")).unwrap(),
                )
                .unwrap();
                receipt["sources"].as_array().unwrap().len()
            })
            .collect::<Vec<_>>();
        source_counts.sort();
        assert_eq!(
            source_counts,
            if history.paginated {
                vec![1, 2]
            } else {
                vec![1, 1]
            }
        );
        println!("{}", outcome);
    }
}
