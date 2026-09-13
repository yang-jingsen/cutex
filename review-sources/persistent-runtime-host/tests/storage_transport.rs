mod common;

use persistent_runtime_host::registry::{RegistryStore, RegistryStoreError};
use persistent_runtime_host::transport::{read_json_frame, write_json_frame, MAX_FRAME_BYTES};
use persistent_runtime_host::*;
use std::collections::BTreeMap;
use std::io::{BufReader, Cursor};

#[test]
fn file_registry_persists_atomic_cas_snapshots_with_private_permissions() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("registry-v1.json");
    let service_id = ServiceId::from("persisted");
    let definitions = BTreeMap::from([(
        service_id.clone(),
        StoredDefinition {
            definition_revision: 7,
            definition: common::definition("persisted", &[]),
        },
    )]);

    let registry = FileRegistry::open(&path).unwrap();
    let persisted = registry.replace(0, definitions.clone()).unwrap();
    assert_eq!(persisted.revision, 1);
    assert_eq!(persisted.definitions, definitions);
    assert!(matches!(
        registry.replace(0, BTreeMap::new()),
        Err(RegistryStoreError::Conflict {
            expected: 0,
            actual: 1
        })
    ));
    drop(registry);

    let reopened = FileRegistry::open(&path).unwrap();
    assert_eq!(reopened.load().unwrap(), persisted);
    assert_eq!(reopened.path(), path);
    assert!(std::fs::read_dir(directory.path())
        .unwrap()
        .all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".tmp-")));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(directory.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn rotating_file_logs_enforce_generation_and_byte_bounds() {
    let directory = tempfile::tempdir().unwrap();
    let logs = RotatingFileLogs::open(
        directory.path(),
        FileLogConfig {
            max_bytes_per_file: 512,
            max_files_per_service: 3,
        },
    )
    .unwrap();
    let service_id = ServiceId::from("chatty");
    let run_id = RunId::from("run-one");
    for sequence in 1..=30 {
        logs.append(&LogEntry {
            service_id: service_id.clone(),
            run_id: run_id.clone(),
            sequence,
            timestamp_ms: sequence,
            stream: LogStream::Stdout,
            encoding: LogEncoding::Utf8,
            data: "x".repeat(900),
            truncated: false,
        })
        .unwrap();
    }

    let retained = std::fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap())
        .collect::<Vec<_>>();
    assert!(retained.len() <= 3);
    for entry in &retained {
        assert!(entry.metadata().unwrap().len() <= 512);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                entry.metadata().unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    let entries = logs.read(&service_id, Some(&run_id), None, 10_000).unwrap();
    assert!(!entries.is_empty());
    assert!(entries.len() <= 3);
    assert!(entries.iter().all(|entry| entry.truncated));
    assert!(entries
        .windows(2)
        .all(|pair| pair[0].sequence < pair[1].sequence));
    assert_eq!(logs.last_sequence(&service_id).unwrap(), 30);
}

#[test]
fn transport_accepts_max_payload_and_rejects_oversize_or_malformed_frames() {
    let value = "a".repeat(MAX_FRAME_BYTES - 2);
    let mut wire = Vec::new();
    write_json_frame(&mut wire, &value).unwrap();
    assert_eq!(wire.len(), MAX_FRAME_BYTES + 1);
    let decoded: String = read_json_frame(&mut BufReader::new(Cursor::new(wire)))
        .unwrap()
        .unwrap();
    assert_eq!(decoded.len(), value.len());

    let oversized = format!("\"{}\"\n", "b".repeat(MAX_FRAME_BYTES - 1));
    let error =
        read_json_frame::<_, String>(&mut BufReader::new(Cursor::new(oversized))).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("exceeds"));

    let error = read_json_frame::<_, serde_json::Value>(&mut BufReader::new(Cursor::new(
        b"{not-json}\n".to_vec(),
    )))
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}

#[cfg(unix)]
#[test]
fn single_instance_and_absent_endpoint_preserve_the_explicit_startup_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let state_dir = directory.path().join("state");
    let paths = RuntimePaths::new(&state_dir).unwrap();
    let first = SingleInstanceGuard::acquire(&paths).unwrap();
    let error = match SingleInstanceGuard::acquire(&paths) {
        Ok(_) => panic!("second host unexpectedly acquired the instance lock"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    drop(first);
    let second = SingleInstanceGuard::acquire(&paths).unwrap();
    drop(second);

    let absent = directory.path().join("absent");
    let absent_paths = RuntimePaths::new(&absent).unwrap();
    let client = LocalClient::new(&absent_paths.socket);
    let error = client
        .call(&RequestEnvelope::v1(
            "absent-host",
            Request::GetHostInfo(GetHostInfoParams {}),
        ))
        .unwrap_err();
    assert!(error.to_string().contains("never starts PRH implicitly"));
    assert!(!absent.exists());

    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&state_dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[cfg(unix)]
#[test]
fn local_state_files_refuse_symlink_substitution() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let target_file = directory.path().join("target-file");
    std::fs::write(&target_file, b"{}\n").unwrap();
    let registry_link = directory.path().join("registry-link");
    symlink(&target_file, &registry_link).unwrap();
    assert!(FileRegistry::open(&registry_link).is_err());

    let real_logs = directory.path().join("real-logs");
    std::fs::create_dir(&real_logs).unwrap();
    let logs_link = directory.path().join("logs-link");
    symlink(&real_logs, &logs_link).unwrap();
    assert!(RotatingFileLogs::open(&logs_link, FileLogConfig::default()).is_err());

    let state_dir = directory.path().join("state-with-link");
    let paths = RuntimePaths::new(&state_dir).unwrap();
    paths.prepare().unwrap();
    symlink(&target_file, &paths.lock).unwrap();
    assert!(SingleInstanceGuard::acquire(&paths).is_err());
}
