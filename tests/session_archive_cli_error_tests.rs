use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(feature = "archive-conflict-test-hook")]
use cutex::agent_bus::model::AgentRegistrationClass;
#[cfg(feature = "archive-conflict-test-hook")]
use cutex::session::model::{CutexSessionArchiveState, CutexSessionRecord, CutexSessionStore};
#[cfg(feature = "archive-conflict-test-hook")]
use cutex::session::store::{load_cutex_session_store, save_cutex_session_store};
#[cfg(feature = "archive-conflict-test-hook")]
use std::ffi::OsString;
#[cfg(feature = "archive-conflict-test-hook")]
use std::sync::{Mutex, OnceLock};
#[cfg(feature = "archive-conflict-test-hook")]
use std::thread;
#[cfg(feature = "archive-conflict-test-hook")]
use std::time::Duration;

#[cfg(feature = "archive-conflict-test-hook")]
const FIXTURE_SESSION_ID: &str = "cutex.archive-fixture";

struct DisposableHome(PathBuf);

impl DisposableHome {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "cutex-session-archive-cli-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("disposable home should be created");
        Self(path)
    }
}

impl Drop for DisposableHome {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(self.0.join(".cutex"), fs::Permissions::from_mode(0o700));
        }
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn archive_command(home: &Path, command: &str, id: &str, json: bool) -> Command {
    let mut process = Command::new(env!("CARGO_BIN_EXE_cutex"));
    process
        .args(["session", command, id])
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_HOME")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if json {
        process.arg("--json");
    }
    #[cfg(feature = "archive-agent-bus-roster-test-fixture")]
    {
        let marker = home
            .join(".cutex")
            .join("test-fixtures")
            .join("empty-roster.marker");
        if marker.is_file() {
            process.env("CUTEX_ARCHIVE_TEST_EMPTY_ROSTER_MARKER", marker);
        }
    }
    process
}

fn invoke_missing_archive_command(command: &str, json: bool) -> Output {
    let home = DisposableHome::new();
    archive_command(&home.0, command, "missing-session", json)
        .output()
        .expect("archive command should execute")
}

fn json_error(output: Output) -> serde_json::Value {
    assert!(!output.status.success(), "archive command should fail");
    assert!(output.stdout.is_empty(), "JSON failure must not use stdout");
    assert!(
        !output.stderr.contains(&0x1b),
        "JSON failure must not include ANSI: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    let error: serde_json::Value = serde_json::from_str(stderr.trim()).expect("stderr JSON object");
    assert!(error.is_object(), "stderr must contain one JSON object");
    error
}

fn assert_route_lookup_error(error: serde_json::Value) {
    assert_eq!(error["stage"], "route");
    assert_eq!(error["code"], "session_not_found");
    assert_eq!(
        error["message"],
        "cutex session is not known: missing-session"
    );
    assert_eq!(error["retryable"], false);
    assert_eq!(error["details"]["cutexSessionId"], "missing-session");
    assert_eq!(error["outcomeUnknown"], false);
}

#[test]
fn retire_and_restore_json_lookup_failures_are_typed_process_envelopes() {
    assert_route_lookup_error(json_error(invoke_missing_archive_command("retire", true)));
    assert_route_lookup_error(json_error(invoke_missing_archive_command("restore", true)));
}

#[test]
fn retire_human_lookup_failure_keeps_human_presentation() {
    let output = invoke_missing_archive_command("retire", false);

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("error:"));
    assert!(stderr.contains("cutex session is not known: missing-session"));
}

#[cfg(feature = "archive-conflict-test-hook")]
static FIXTURE_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[cfg(feature = "archive-conflict-test-hook")]
struct HomeEnvironment {
    previous_home: Option<OsString>,
}

#[cfg(feature = "archive-conflict-test-hook")]
impl HomeEnvironment {
    fn set(home: &Path) -> Self {
        let previous_home = std::env::var_os("HOME");
        unsafe { std::env::set_var("HOME", home) };
        Self { previous_home }
    }
}

#[cfg(feature = "archive-conflict-test-hook")]
impl Drop for HomeEnvironment {
    fn drop(&mut self) {
        match &self.previous_home {
            Some(home) => unsafe { std::env::set_var("HOME", home) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }
}

#[cfg(feature = "archive-conflict-test-hook")]
fn managed_fixture_record(lifecycle: CutexSessionArchiveState) -> CutexSessionRecord {
    let mut record = CutexSessionRecord::new_at(
        FIXTURE_SESSION_ID.to_string(),
        Some("019e-archive-fixture".to_string()),
        "fixture-host".to_string(),
        "/tmp/cutex-archive-fixture".to_string(),
        Some("fixture".to_string()),
        "2026-08-14T00:00:00Z".to_string(),
    )
    .expect("fixture record should be valid");
    record.registration_class = AgentRegistrationClass::Persistent;
    record.archive_state = lifecycle;
    if lifecycle == CutexSessionArchiveState::Retired {
        record.retired_at = Some("2026-08-14T00:01:00Z".to_string());
    }
    record
}

#[cfg(feature = "archive-conflict-test-hook")]
fn with_fixture_home<T>(home: &Path, operation: impl FnOnce() -> T) -> T {
    let _lock = FIXTURE_ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("fixture environment lock should not be poisoned");
    let _environment = HomeEnvironment::set(home);
    operation()
}

#[cfg(feature = "archive-conflict-test-hook")]
fn seed_fixture(home: &Path, record: CutexSessionRecord) {
    with_fixture_home(home, || {
        let mut store = CutexSessionStore::default();
        store
            .sessions
            .insert(record.cutex_session_id.clone(), record);
        save_cutex_session_store(&store).expect("fixture store should save");
    });
    let marker = home
        .join(".cutex")
        .join("test-fixtures")
        .join("empty-roster.marker");
    fs::create_dir_all(marker.parent().expect("marker parent"))
        .expect("roster fixture parent should be created");
    fs::write(&marker, b"cutex-archive-empty-roster-v1\n")
        .expect("empty roster fixture marker should be written");
}

#[cfg(feature = "archive-conflict-test-hook")]
fn run_fixture_command(home: &Path, command: &str) -> serde_json::Value {
    json_error(
        archive_command(home, command, FIXTURE_SESSION_ID, true)
            .output()
            .expect("fixture archive command should execute"),
    )
}

#[cfg(feature = "archive-conflict-test-hook")]
fn assert_provider_error(
    error: serde_json::Value,
    stage: &str,
    code: &str,
    message: &str,
    retryable: bool,
    outcome_unknown: bool,
) {
    assert_eq!(error["stage"], stage);
    assert_eq!(error["code"], code);
    assert_eq!(error["message"], message);
    assert_eq!(error["retryable"], retryable);
    assert_eq!(error["outcomeUnknown"], outcome_unknown);
}

#[cfg(feature = "archive-conflict-test-hook")]
#[test]
fn repeated_state_provider_failures_cross_the_binary_boundary() {
    let retired = DisposableHome::new();
    seed_fixture(
        &retired.0,
        managed_fixture_record(CutexSessionArchiveState::Retired),
    );
    let already_retired = run_fixture_command(&retired.0, "retire");
    assert_provider_error(
        already_retired.clone(),
        "route",
        "already_retired",
        "the cutex session is already retired",
        false,
        false,
    );
    assert_eq!(already_retired["details"], serde_json::json!({}));

    let active = DisposableHome::new();
    seed_fixture(
        &active.0,
        managed_fixture_record(CutexSessionArchiveState::Active),
    );
    let already_active = run_fixture_command(&active.0, "restore");
    assert_provider_error(
        already_active.clone(),
        "route",
        "already_active",
        "the cutex session is already active",
        false,
        false,
    );
    assert_eq!(already_active["details"], serde_json::json!({}));
}

#[cfg(feature = "archive-conflict-test-hook")]
fn wait_for_marker(path: &Path) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !path.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for hook marker {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(feature = "archive-conflict-test-hook")]
#[test]
fn revision_conflict_is_generated_by_the_provider_after_the_gated_barrier() {
    const TOKEN: &str = "revision-conflict-fixture";
    let fixture = DisposableHome::new();
    seed_fixture(
        &fixture.0,
        managed_fixture_record(CutexSessionArchiveState::Active),
    );

    let marker_dir = fixture.0.join(".cutex").join("archive-conflict-test-hook");
    let ready = marker_dir.join(format!("{TOKEN}.ready"));
    let release = marker_dir.join(format!("{TOKEN}.release"));
    let child = archive_command(&fixture.0, "retire", FIXTURE_SESSION_ID, true)
        .env("CUTEX_ARCHIVE_CONFLICT_TEST_TOKEN", TOKEN)
        .spawn()
        .expect("gated archive command should start");
    wait_for_marker(&ready);

    with_fixture_home(&fixture.0, || {
        let mut store = load_cutex_session_store().expect("fixture store should reload");
        let record = store
            .sessions
            .get_mut(FIXTURE_SESSION_ID)
            .expect("fixture record should exist");
        record
            .bump_durable_revision()
            .expect("fixture revision should advance");
        save_cutex_session_store(&store).expect("external fixture mutation should save");
    });
    fs::write(&release, TOKEN).expect("release marker should write");

    let error = json_error(
        child
            .wait_with_output()
            .expect("gated command should finish"),
    );
    assert_provider_error(
        error.clone(),
        "route",
        "revision_conflict",
        "durable session revision conflict: expected 1, current 2",
        true,
        false,
    );
    assert_eq!(error["details"]["expectedRevision"], 1);
    assert_eq!(error["details"]["currentRevision"], 2);
    assert_eq!(error["details"]["resyncRequired"], true);
}

#[cfg(all(feature = "archive-conflict-test-hook", unix))]
#[test]
fn outcome_unknown_provider_failure_uses_an_unwritable_disposable_store() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = DisposableHome::new();
    seed_fixture(
        &fixture.0,
        managed_fixture_record(CutexSessionArchiveState::Active),
    );
    let cutex_dir = fixture.0.join(".cutex");
    fs::set_permissions(&cutex_dir, fs::Permissions::from_mode(0o500))
        .expect("fixture store directory should become read-only");

    let error = run_fixture_command(&fixture.0, "retire");
    assert_provider_error(
        error.clone(),
        "persistence",
        "persistence_uncertain",
        "the durable session outcome is uncertain; resync before retrying",
        false,
        true,
    );
    assert_eq!(error["details"]["resyncRequired"], true);
    assert!(error["details"]["diagnostic"].is_string());
}
