use persistent_runtime_host::*;
use std::collections::BTreeMap;

fn golden_register_request() -> RequestEnvelope {
    RequestEnvelope {
        protocol: ProtocolVersion::V1,
        capabilities: vec!["future.client.capability".to_owned()],
        request_id: "request-register-web-1".to_owned(),
        request: Request::RegisterService(RegisterServiceParams {
            definition: ServiceDefinition {
                id: ServiceId::from("web"),
                display_name: "Example Web".to_owned(),
                description: Some("Golden fixture service".to_owned()),
                executable: "/opt/prh/bin/example-web".to_owned(),
                arguments: vec!["--listen".to_owned(), "127.0.0.1:4310".to_owned()],
                working_directory: "/var/lib/prh/example-web".to_owned(),
                environment: BTreeMap::from([("RUST_LOG".to_owned(), "info".to_owned())]),
                start_policy: StartPolicy::Manual,
                restart_policy: RestartPolicy::BoundedOnFailure {
                    max_restarts: 3,
                    window_ms: 60_000,
                    backoff_ms: 250,
                },
                dependencies: vec![ServiceId::from("database")],
                readiness_probe: Some(ReadinessProbe::Tcp {
                    host: "127.0.0.1".to_owned(),
                    port: 4310,
                    interval_ms: 500,
                    timeout_ms: 100,
                }),
                shutdown_policy: ShutdownPolicy {
                    graceful_timeout_ms: 5_000,
                    force_kill_timeout_ms: 2_000,
                },
                metadata: BTreeMap::from([("owner".to_owned(), "example".to_owned())]),
            },
            mutation: MutationOptions {
                idempotency_key: "register-web-20260907".to_owned(),
                expected_revision: Some(7),
            },
        }),
    }
}

fn golden_error_response() -> ResponseEnvelope {
    protocol_error("request-new-major", ProtocolVersion { major: 2, minor: 0 })
}

fn golden_event_envelope() -> EventEnvelope {
    EventEnvelope {
        protocol: ProtocolVersion::V1,
        subscription_id: SubscriptionId::from("subscription-0001"),
        event: HostEvent {
            sequence: 42,
            timestamp_ms: 1_789_000_000_123,
            event: HostEventKind::ServiceStateChanged {
                service_id: ServiceId::from("web"),
                run_id: Some(RunId::from("run-0007")),
                desired_state: DesiredState::Running,
                observed_state: ObservedState::Running,
                health: HealthState::Healthy,
            },
        },
    }
}

fn assert_fixture<T: serde::Serialize>(value: &T, fixture: &str) {
    let actual = serde_json::to_value(value).unwrap();
    let expected: serde_json::Value = serde_json::from_str(fixture).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn v1_wire_shapes_match_golden_fixtures() {
    assert_fixture(
        &golden_register_request(),
        include_str!("fixtures/register_service_request_v1.json"),
    );
    assert_fixture(
        &golden_error_response(),
        include_str!("fixtures/unsupported_protocol_response_v1.json"),
    );
    assert_fixture(
        &golden_event_envelope(),
        include_str!("fixtures/service_state_event_v1.json"),
    );

    let request: RequestEnvelope =
        serde_json::from_str(include_str!("fixtures/register_service_request_v1.json")).unwrap();
    assert_eq!(request, golden_register_request());
}

#[test]
fn additive_unknown_fields_are_tolerated_at_every_envelope_level() {
    let mut value = serde_json::to_value(golden_register_request()).unwrap();
    let object = value.as_object_mut().unwrap();
    object.insert("future_envelope_field".to_owned(), serde_json::json!(true));
    object["protocol"].as_object_mut().unwrap().insert(
        "future_version_field".to_owned(),
        serde_json::json!("value"),
    );
    let request = object["request"].as_object_mut().unwrap();
    request.insert("future_request_field".to_owned(), serde_json::json!([1, 2]));
    let params = request["params"].as_object_mut().unwrap();
    params.insert("future_params_field".to_owned(), serde_json::json!({}));
    params["definition"]
        .as_object_mut()
        .unwrap()
        .insert("future_definition_field".to_owned(), serde_json::json!(17));

    let decoded: RequestEnvelope = serde_json::from_value(value).unwrap();
    assert_eq!(decoded, golden_register_request());
}

#[test]
fn incompatible_major_returns_typed_v1_error_with_request_identity() {
    let host = FakeHost::new(FakeHostConfig::default());
    let response = host.handle(RequestEnvelope {
        protocol: ProtocolVersion { major: 9, minor: 4 },
        capabilities: vec!["unknown.capability".to_owned()],
        request_id: "request-new-major".to_owned(),
        request: Request::GetHostInfo(GetHostInfoParams {}),
    });
    assert_eq!(response.request_id, "request-new-major");
    assert_eq!(response.protocol, ProtocolVersion::V1);
    let ResponseOutcome::Error { error } = response.outcome else {
        panic!("new protocol major must be rejected");
    };
    assert_eq!(error.code, ErrorCode::UnsupportedProtocol);
}
