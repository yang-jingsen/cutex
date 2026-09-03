use super::*;

#[test]
fn management_serve_command_parses() {
    let cli = Cli::try_parse_from([
        "cutex",
        "management",
        "serve",
        "--port",
        "24270",
        "--bind",
        "100.79.47.97",
        "--token",
        "secret",
    ])
    .expect("management serve should parse");
    assert!(matches!(
        cli.command,
        Some(CommandKind::Management {
            command: ManagementCommand::Serve {
                port: Some(24270),
                bind,
                token: Some(token),
            }
        }) if bind == "100.79.47.97" && token == "secret"
    ));
}

#[test]
fn management_remote_up_command_parses() {
    let cli = Cli::try_parse_from([
        "cutex",
        "management",
        "remote-up",
        "host-b",
        "--local-port",
        "24670",
        "--remote-port",
        "24270",
        "--token",
        "secret",
        "--show-ssh-fallback",
    ])
    .expect("management remote-up should parse");
    assert!(matches!(
        cli.command,
        Some(CommandKind::Management {
            command: ManagementCommand::RemoteUp {
                host,
                service_id: None,
                local_port: Some(24670),
                remote_port: Some(24270),
                token: Some(token),
                show_ssh_fallback: true,
            }
        }) if host == "host-b" && token == "secret"
    ));
}

#[test]
fn every_agent_management_operation_has_a_structured_cli_surface() {
    let cases = [
        ("create", AgentOperationKind::Create),
        ("query-managed", AgentOperationKind::QueryManaged),
        ("online", AgentOperationKind::Online),
        ("offline", AgentOperationKind::Offline),
        ("restart", AgentOperationKind::Restart),
        ("close", AgentOperationKind::Close),
        ("replace", AgentOperationKind::Replace),
        ("director-rotate", AgentOperationKind::DirectorRotate),
    ];
    for (name, expected) in cases {
        let cli = Cli::try_parse_from([
            "cutex",
            "agent",
            "manage",
            name,
            "--request-file",
            "/tmp/request.json",
        ])
        .expect("Agent Management command should parse");
        let actual = match cli.command {
            Some(CommandKind::Agent {
                command: AgentCommand::Manage { command },
            }) => match command {
                AgentManagementCliCommand::Create { .. } => AgentOperationKind::Create,
                AgentManagementCliCommand::QueryManaged { .. } => AgentOperationKind::QueryManaged,
                AgentManagementCliCommand::Online { .. } => AgentOperationKind::Online,
                AgentManagementCliCommand::Offline { .. } => AgentOperationKind::Offline,
                AgentManagementCliCommand::Restart { .. } => AgentOperationKind::Restart,
                AgentManagementCliCommand::Close { .. } => AgentOperationKind::Close,
                AgentManagementCliCommand::Replace { .. } => AgentOperationKind::Replace,
                AgentManagementCliCommand::DirectorRotate { .. } => {
                    AgentOperationKind::DirectorRotate
                }
            },
            other => panic!("unexpected parsed command: {other:?}"),
        };
        assert_eq!(actual, expected);
    }
}

#[test]
fn project_authority_admin_surface_parses_separately() {
    let cli = Cli::try_parse_from([
        "cutex",
        "management",
        "agent-authority",
        "--request-file",
        "/tmp/authority.json",
        "--port",
        "24270",
    ])
    .expect("project authority command should parse");
    assert!(matches!(
        cli.command,
        Some(CommandKind::Management {
            command: ManagementCommand::AgentAuthority {
                request_file,
                port: Some(24270),
                token: None,
            }
        }) if request_file == "/tmp/authority.json"
    ));
}

#[test]
fn legacy_director_ownership_import_is_a_separate_root_management_surface() {
    let cli = Cli::try_parse_from([
        "cutex",
        "management",
        "agent-ownership-import",
        "--request-file",
        "/tmp/legacy-director-import.json",
        "--port",
        "24270",
    ])
    .expect("legacy Director ownership import command should parse");
    assert!(matches!(
        cli.command,
        Some(CommandKind::Management {
            command: ManagementCommand::AgentOwnershipImport {
                request_file,
                port: Some(24270),
                token: None,
            }
        }) if request_file == "/tmp/legacy-director-import.json"
    ));
}
