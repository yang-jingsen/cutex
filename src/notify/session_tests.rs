use super::*;

#[test]
fn preferences_persist_without_crossing_sessions_and_reload_labels() {
    let root =
        std::env::temp_dir().join(format!("cutex-notification-test-{}", uuid::Uuid::new_v4()));
    let id = uuid::Uuid::new_v4().to_string();
    let other = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        session_at(&root, &id, Change::Read).unwrap().level,
        Level::Off
    );
    assert!(!root.exists());
    assert_eq!(
        session_at(&root, &id, Change::Cycle).unwrap().level,
        Level::Important
    );
    assert_eq!(session_at(&root, &id, Change::Read).unwrap().label, "CIAO!");
    assert_eq!(
        session_at(&root, &other, Change::Read).unwrap().level,
        Level::Off
    );
    fs::write(
        root.join("config.json"),
        r#"{"important":"关注","normal":"普通","off":"关闭"}"#,
    )
    .unwrap();
    assert_eq!(session_at(&root, &id, Change::Read).unwrap().label, "关注");
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let root = root.clone();
            let id = id.clone();
            std::thread::spawn(move || session_at(&root, &id, Change::Cycle).unwrap())
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(
        session_at(&root, &id, Change::Read).unwrap().level,
        Level::Off
    );
    fs::write(root.join("config.json"), r#"{"important":"\u001b[31m"}"#).unwrap();
    assert!(session_at(&root, &id, Change::Set(Level::Normal)).is_err());
    fs::remove_dir_all(root).unwrap();
}
