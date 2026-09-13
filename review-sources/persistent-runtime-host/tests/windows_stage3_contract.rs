use persistent_runtime_host::model::{
    DesiredState, HealthState, HostPhase, HostStatus, ObservedState, RestartPolicy, RuntimeState,
    ServiceDefinition, ServiceId, ServiceSnapshot, ShutdownPolicy, StartPolicy, StopOutcome,
};
use persistent_runtime_host::project_tray_status;
use persistent_runtime_host::windows_support::{
    build_windows_command_line, encode_windows_environment, merge_windows_environment,
    quote_windows_argument, windows_pipe_name_for_state, WINDOWS_INHERITED_HANDLE_ROLES,
};
use std::collections::BTreeMap;

#[test]
fn windows_argument_quoting_preserves_empty_spaces_quotes_and_trailing_slashes() {
    assert_eq!(quote_windows_argument("plain"), "plain");
    assert_eq!(quote_windows_argument(""), "\"\"");
    assert_eq!(quote_windows_argument("two words"), "\"two words\"");
    assert_eq!(quote_windows_argument("say\"hi"), "\"say\\\"hi\"");
    assert_eq!(
        quote_windows_argument("C:\\path with space\\"),
        "\"C:\\path with space\\\\\""
    );
}

#[test]
fn windows_command_line_starts_with_the_exact_image_and_adds_no_shell_wrapper() {
    let arguments = vec![
        "--name".to_owned(),
        "alpha beta".to_owned(),
        "quote\"here".to_owned(),
    ];
    let line = build_windows_command_line("C:\\Program Files\\svc.exe", &arguments);
    assert_eq!(
        line,
        "\"C:\\Program Files\\svc.exe\" --name \"alpha beta\" \"quote\\\"here\""
    );
    assert!(!line.to_lowercase().contains("cmd.exe /c"));
    assert!(!line.to_lowercase().contains("powershell"));
}

#[test]
fn windows_environment_overrides_case_insensitively_and_is_double_nul_terminated() {
    let base = vec![
        ("Path".to_owned(), "base-path".to_owned()),
        ("SystemRoot".to_owned(), "C:\\Windows".to_owned()),
    ];
    let additions = BTreeMap::from([
        ("PATH".to_owned(), "service-path".to_owned()),
        ("ZETA".to_owned(), "last".to_owned()),
    ]);
    let merged = merge_windows_environment(base, &additions);
    assert_eq!(
        merged,
        vec![
            ("PATH".to_owned(), "service-path".to_owned()),
            ("SystemRoot".to_owned(), "C:\\Windows".to_owned()),
            ("ZETA".to_owned(), "last".to_owned()),
        ]
    );
    let block = encode_windows_environment(&merged);
    assert!(block.ends_with(&[0, 0]));
    assert_eq!(
        String::from_utf16_lossy(&block[..block.len() - 2]),
        "PATH=service-path\0SystemRoot=C:\\Windows\0ZETA=last"
    );
}

#[test]
fn windows_pipe_identity_is_stable_across_case_separator_and_trailing_slash_variants() {
    let first = windows_pipe_name_for_state("C:/Users/Alice/AppData/Local/PRH/");
    let second = windows_pipe_name_for_state("c:\\users\\alice\\appdata\\local\\prh");
    let other = windows_pipe_name_for_state("C:\\Users\\Alice\\AppData\\Local\\Other");
    assert_eq!(first, second);
    assert_ne!(first, other);
    assert!(first.starts_with(r"\\.\pipe\persistent-runtime-host-v1-"));
}

#[test]
fn windows_launch_handle_contract_contains_only_the_three_standard_streams() {
    assert_eq!(
        WINDOWS_INHERITED_HANDLE_ROLES,
        ["stdin", "stdout", "stderr"]
    );
}

#[test]
fn tray_projection_exposes_service_status_controls_and_dismissible_alert_fingerprint() {
    let mut stopped = RuntimeState::stopped(1);
    stopped.desired_state = DesiredState::Stopped;
    let mut unhealthy = RuntimeState::stopped(1);
    unhealthy.desired_state = DesiredState::Running;
    unhealthy.observed_state = ObservedState::Running;
    unhealthy.health = HealthState::Unhealthy;
    unhealthy.last_stop_outcome = Some(StopOutcome::TimedOut);
    let status = HostStatus {
        phase: HostPhase::Running,
        registry_revision: 2,
        services: vec![
            service("zeta", "Zeta", unhealthy),
            service("alpha", "Alpha", stopped),
        ],
    };
    let projection = project_tray_status(&status);
    assert_eq!(projection.services[0].service_id, "alpha");
    assert!(projection.services[0].can_start);
    assert!(!projection.services[0].can_stop);
    assert!(projection.services[1].can_stop);
    assert!(projection.services[1].can_restart);
    assert!(projection.services[1].needs_attention);
    let fingerprint = projection.alert_fingerprint.as_deref().unwrap();
    assert!(projection.alerts_visible(None));
    assert!(!projection.alerts_visible(Some(fingerprint)));
    assert!(projection.tooltip.contains("need attention"));

    let mut shutting_down = status;
    shutting_down.phase = HostPhase::ShuttingDown;
    let projection = project_tray_status(&shutting_down);
    assert!(projection
        .services
        .iter()
        .all(|service| { !service.can_start && !service.can_stop && !service.can_restart }));
}

fn service(id: &str, display_name: &str, runtime: RuntimeState) -> ServiceSnapshot {
    ServiceSnapshot {
        definition: ServiceDefinition {
            id: ServiceId::new(id),
            display_name: display_name.to_owned(),
            description: None,
            executable: "C:\\fixture.exe".to_owned(),
            arguments: Vec::new(),
            working_directory: "C:\\".to_owned(),
            environment: BTreeMap::new(),
            start_policy: StartPolicy::Manual,
            restart_policy: RestartPolicy::Never,
            dependencies: Vec::new(),
            readiness_probe: None,
            shutdown_policy: ShutdownPolicy {
                graceful_timeout_ms: 100,
                force_kill_timeout_ms: 100,
            },
            metadata: BTreeMap::new(),
        },
        runtime,
    }
}
