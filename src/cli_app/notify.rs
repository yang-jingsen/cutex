use cutex::cli::args::NotifyCommand;

pub(crate) fn run_command(command: NotifyCommand) -> anyhow::Result<()> {
    match command {
        NotifyCommand::Session { thread_id, level, cycle, json } => {
            use cutex::notify::session::{self, Change};
            let change = match (level, cycle) {
                (Some(level), _) => Change::Set(level),
                (None, true) => Change::Cycle,
                (None, false) => Change::Read,
            };
            let preference = session::session(&thread_id, change)?;
            if json {
                println!("{}", serde_json::to_string(&preference)?);
            } else {
                println!("{} · {}", preference.thread_id, preference.label);
            }
            Ok(())
        },
    }
}

/// Add the notification control to the light frontend without modifying reviewed assets.
pub(super) fn status_line(items: Option<&Vec<String>>) -> Vec<String> {
    let mut items = items.cloned().unwrap_or_else(|| vec!["model-with-reasoning".into(), "current-dir".into()]);
    for item in &mut items {
        *item = cutex::launch::selected_status::canonical_id(item).to_string();
    }
    if !items.is_empty() && !items.iter().any(|item| item == "cutex_notification") {
        items.push("cutex_notification".into());
    }
    items
}
