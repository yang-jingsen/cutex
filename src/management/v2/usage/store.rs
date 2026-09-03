use std::collections::HashSet;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::sync::OnceLock;

use anyhow::Context;
use chrono::Datelike;
use fs2::FileExt;

use crate::config::atomic::write_private_pretty_json_atomic;

use super::model::parse_timestamp;
use super::model::UsageLedger;
use super::model::UsageLedgerEntry;
use super::model::UsageStateFile;
use super::model::UsageStateSnapshot;

const USAGE_STATE_FILE: &str = "session-usage-state.json";
const USAGE_LOCK_FILE: &str = "session-usage-state.lock";
const USAGE_LEDGER_DIR: &str = "usage-ledger";

static USAGE_STORE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Default)]
pub(super) struct UsageMutation {
    pub changed: bool,
    pub entries: Vec<UsageLedgerEntry>,
}

pub(super) fn update_usage_state_at(
    root: &Path,
    received_at: &str,
    mutate: impl FnOnce(&mut UsageStateFile) -> anyhow::Result<UsageMutation>,
) -> anyhow::Result<()> {
    with_usage_store_lock(root, |state_path, ledger_dir| {
        let mut state = load_usage_state(state_path)?;
        flush_pending_entries(state_path, ledger_dir, &mut state)?;

        let mutation = mutate(&mut state)?;
        if !mutation.changed {
            return Ok(());
        }
        state.mark_changed(received_at)?;
        state.pending_entries = mutation.entries;
        state.validate()?;
        write_usage_state(state_path, &state)?;
        flush_pending_entries(state_path, ledger_dir, &mut state)
    })
}

pub(super) fn load_usage_state_snapshot_at(root: &Path) -> anyhow::Result<UsageStateSnapshot> {
    with_usage_store_lock(root, |state_path, ledger_dir| {
        let mut state = load_usage_state(state_path)?;
        flush_pending_entries(state_path, ledger_dir, &mut state)?;
        Ok(state.snapshot())
    })
}

pub(super) fn load_usage_ledger_at(root: &Path) -> anyhow::Result<UsageLedger> {
    with_usage_store_lock(root, |state_path, ledger_dir| {
        let mut state = load_usage_state(state_path)?;
        flush_pending_entries(state_path, ledger_dir, &mut state)?;
        read_usage_ledger(ledger_dir)
    })
}

pub(super) fn load_usage_data_at(root: &Path) -> anyhow::Result<(UsageStateSnapshot, UsageLedger)> {
    with_usage_store_lock(root, |state_path, ledger_dir| {
        let mut state = load_usage_state(state_path)?;
        flush_pending_entries(state_path, ledger_dir, &mut state)?;
        Ok((state.snapshot(), read_usage_ledger(ledger_dir)?))
    })
}

fn flush_pending_entries(
    state_path: &Path,
    ledger_dir: &Path,
    state: &mut UsageStateFile,
) -> anyhow::Result<()> {
    if state.pending_entries.is_empty() {
        return Ok(());
    }
    for entry in &state.pending_entries {
        append_ledger_entry(ledger_dir, entry)?;
    }
    state.pending_entries.clear();
    state.validate()?;
    write_usage_state(state_path, state)
}

fn append_ledger_entry(ledger_dir: &Path, entry: &UsageLedgerEntry) -> anyhow::Result<()> {
    entry.validate()?;
    fs::create_dir_all(ledger_dir).with_context(|| {
        format!(
            "Failed to create management v2 usage ledger: {}",
            ledger_dir.display()
        )
    })?;
    secure_directory(ledger_dir)?;
    let observed_at = parse_timestamp(entry.observed_at())?;
    let path = ledger_dir.join(format!(
        "{:04}-{:02}.jsonl",
        observed_at.year(),
        observed_at.month()
    ));
    let mut options = OpenOptions::new();
    options.write(true).append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .with_context(|| format!("Failed to open usage ledger: {}", path.display()))?;
    secure_file(&path)?;
    let encoded = serde_json::to_vec(entry).context("Failed to serialize usage ledger entry")?;
    // Prefixing with a newline isolates a torn final write from the next retry.
    file.write_all(b"\n")?;
    file.write_all(&encoded)?;
    file.sync_data()
        .with_context(|| format!("Failed to sync usage ledger: {}", path.display()))?;
    Ok(())
}

fn read_usage_ledger(ledger_dir: &Path) -> anyhow::Result<UsageLedger> {
    let mut paths = match fs::read_dir(ledger_dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|value| value == "jsonl"))
            .collect::<Vec<_>>(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(UsageLedger::default())
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to read usage ledger: {}", ledger_dir.display()));
        }
    };
    paths.sort();
    let mut ledger = UsageLedger::default();
    let mut seen = HashSet::new();
    for path in paths {
        read_ledger_file(&path, &mut seen, &mut ledger)?;
    }
    ledger.entries.sort_by(|left, right| {
        left.observed_at()
            .cmp(right.observed_at())
            .then_with(|| left.entry_id().cmp(right.entry_id()))
    });
    Ok(ledger)
}

fn read_ledger_file(
    path: &Path,
    seen: &mut HashSet<String>,
    ledger: &mut UsageLedger,
) -> anyhow::Result<()> {
    let file = File::open(path)
        .with_context(|| format!("Failed to open usage ledger: {}", path.display()))?;
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line_number = index + 1;
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                ledger.warnings.push(format!(
                    "{}:{line_number}: failed to read usage ledger line: {error}",
                    path.display()
                ));
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let entry = match serde_json::from_str::<UsageLedgerEntry>(&line) {
            Ok(entry) => entry,
            Err(error) => {
                ledger.warnings.push(format!(
                    "{}:{line_number}: ignored invalid usage ledger JSON: {error}",
                    path.display()
                ));
                continue;
            }
        };
        if let Err(error) = entry.validate() {
            ledger.warnings.push(format!(
                "{}:{line_number}: ignored invalid usage ledger entry: {error:#}",
                path.display()
            ));
            continue;
        }
        if seen.insert(entry.entry_id().to_string()) {
            ledger.entries.push(entry);
        }
    }
    Ok(())
}

fn with_usage_store_lock<T>(
    root: &Path,
    action: impl FnOnce(&Path, &Path) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let _process_guard = USAGE_STORE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("management v2 usage lock was poisoned"))?;
    fs::create_dir_all(root)?;
    secure_directory(root)?;
    let lock_file = open_private_lock(&root.join(USAGE_LOCK_FILE))?;
    lock_file.lock_exclusive()?;
    let result = action(&root.join(USAGE_STATE_FILE), &root.join(USAGE_LEDGER_DIR));
    let unlock = lock_file.unlock();
    if result.is_ok() {
        unlock?;
    }
    result
}

pub(super) fn load_usage_state(path: &Path) -> anyhow::Result<UsageStateFile> {
    let state = match fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<UsageStateFile>(&bytes)
            .with_context(|| format!("Failed to parse management v2 usage: {}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => UsageStateFile::default(),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("Failed to read management v2 usage: {}", path.display())
            });
        }
    };
    state.validate()?;
    Ok(state)
}

pub(super) fn write_usage_state(path: &Path, state: &UsageStateFile) -> anyhow::Result<()> {
    write_private_pretty_json_atomic(path, state, "management v2 session usage")
}

fn open_private_lock(path: &Path) -> anyhow::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    let file = options.open(path)?;
    secure_file(path)?;
    Ok(file)
}

#[cfg(unix)]
fn secure_directory(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn secure_directory(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn secure_file(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn secure_file(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
pub(super) fn state_path(root: &Path) -> std::path::PathBuf {
    root.join(USAGE_STATE_FILE)
}
