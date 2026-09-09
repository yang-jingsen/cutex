use super::*;
use serde_json::json;

#[test]
fn native_status_changed_hint_is_not_an_ack_and_rejects_control_fields() {
    let hint: StatusChanged =
        serde_json::from_value(json!({"threadId":"thread","messageId":"message"})).unwrap();
    assert_eq!(hint.thread_id, "thread");
    for extra in ["receipt", "ownerId", "runtimeGeneration", "delivered"] {
        let mut value = serde_json::to_value(&hint).unwrap();
        value[extra] = json!(true);
        assert!(serde_json::from_value::<StatusChanged>(value).is_err());
    }
}

fn envelope() -> Envelope {
    let mut e = Envelope {
        version: 1,
        owner_id: "owner".into(),
        thread_id: "thread".into(),
        runtime_generation: 7,
        message: Message {
            id: "message".into(),
            source: Source {
                kind: SourceKind::Service,
                id: "service-id".into(),
            },
            event_type: "opaque.v1".into(),
            delivery: Delivery::AfterTurn,
            text: "hello\n世界".into(),
        },
        semantic_sha256: String::new(),
    };
    e.semantic_sha256 = e.digest();
    e
}
fn binding() -> ExternalInputBinding {
    ExternalInputBinding {
        version: 1,
        owner_id: "owner".into(),
        thread_id: "thread".into(),
        runtime_generation: 7,
        canonical_byte_limit: Default::default(),
    }
}
fn response() -> Response {
    let e = envelope();
    let mut receipt = Receipt {
        schema: "codex.external-input-receipt.v1".into(),
        receipt_id: String::new(),
        owner_id: e.owner_id.clone(),
        thread_id: e.thread_id.clone(),
        message_id: e.message.id.clone(),
        semantic_sha256: e.semantic_sha256.clone(),
        response_item_id: e.message.id.clone(),
        turn_id: "turn".into(),
        ordinal: 1,
    };
    receipt.receipt_id = receipt.digest_id();
    Response {
        version: 1,
        owner_id: e.owner_id,
        thread_id: e.thread_id,
        runtime_generation: 7,
        statuses: vec![Status {
            message_id: e.message.id,
            semantic_sha256: e.semantic_sha256,
            delivery_state: DeliveryState::ContextPersisted,
            receipt: Some(receipt),
            processing: ProcessingStatus {
                state: ProcessingState::Pending,
                attempt_id: None,
                reason: None,
            },
        }],
    }
}
#[test]
fn exact_native_digest_and_receipt_vectors_generation_independent() {
    let mut e = envelope();
    assert_eq!(
        e.digest(),
        "5b066f6337afec950bb316090397702df3c239477ac13768d8a1fd6bf86a400e"
    );
    assert_eq!(
        response().statuses[0].receipt.as_ref().unwrap().receipt_id,
        "eir1_3692b2f248c7c1a6e14112bbf3e6d5c498ee0c737d098c3aed89e42ec7dd93fa"
    );
    e.runtime_generation += 1;
    assert_eq!(e.digest(), envelope().digest());
    e.message.text.push(' ');
    assert_ne!(e.digest(), envelope().digest());
}
#[test]
fn envelope_strict_types_no_sender_authority_or_policy() {
    for extra in [
        "canonicalByteLimit",
        "role",
        "permissions",
        "caller",
        "token",
    ] {
        let mut v = serde_json::to_value(envelope()).unwrap();
        v[extra] = json!("off");
        assert!(serde_json::from_value::<Envelope>(v).is_err());
    }
    for role in ["human", "system", "developer", "assistant"] {
        let mut v = serde_json::to_value(envelope()).unwrap();
        v["message"]["source"]["kind"] = json!(role);
        assert!(serde_json::from_value::<Envelope>(v).is_err());
    }
    let mut e = envelope();
    e.message.text = "界".repeat(21846);
    e.semantic_sha256 = e.digest();
    assert!(e.validate().is_err());
    e.message.text = "x".repeat(65536);
    e.semantic_sha256 = e.digest();
    assert!(e.validate().is_ok());
}
#[test]
fn response_rejects_stale_forged_receipt_wrong_count_and_false_a4() {
    let keys = [envelope().key()];
    let valid = response();
    valid.validate(&binding(), &keys).unwrap();
    for change in 0..7 {
        let mut r = valid.clone();
        match change {
            0 => r.runtime_generation += 1,
            1 => r.statuses.clear(),
            2 => r.statuses[0].receipt = None,
            3 => r.statuses[0].delivery_state = DeliveryState::Pending,
            4 => r.statuses[0].receipt.as_mut().unwrap().ordinal += 1,
            5 => r.statuses[0].receipt.as_mut().unwrap().thread_id = "foreign".into(),
            _ => r.statuses[0].semantic_sha256 = "f".repeat(64),
        }
        assert!(r.validate(&binding(), &keys).is_err(), "{change}");
    }
}
#[test]
fn response_processing_held_is_not_business_completion() {
    let mut r = response();
    let keys = [envelope().key()];
    r.statuses[0].processing = ProcessingStatus {
        state: ProcessingState::Held,
        attempt_id: Some(uuid::Uuid::new_v4().to_string()),
        reason: Some(HoldReason::NoOutput),
    };
    r.validate(&binding(), &keys).unwrap();
    r.statuses[0].processing.attempt_id = None;
    assert!(r.validate(&binding(), &keys).is_err());
    r.statuses[0].processing.reason = Some(HoldReason::CanonicalSizePolicy);
    r.validate(&binding(), &keys).unwrap();
    let mut raw = serde_json::to_value(&r).unwrap();
    raw["statuses"][0]["deliveryState"] = json!("delivered");
    assert!(serde_json::from_value::<Response>(raw).is_err());
}
#[test]
fn receiver_policy_exact_native_range_and_null_denial() {
    use crate::launch::stock::CanonicalBytePolicy;
    for value in [json!(1), json!(10000), json!(u32::MAX), json!("off")] {
        assert!(serde_json::from_value::<CanonicalBytePolicy>(value).is_ok());
    }
    for value in [
        json!(null),
        json!(0),
        json!(-1),
        json!(1.5),
        json!(true),
        json!(4294967296u64),
        json!("OFF"),
    ] {
        assert!(serde_json::from_value::<CanonicalBytePolicy>(value).is_err());
    }
    let raw = serde_json::to_value(binding()).unwrap();
    assert!(raw.get("canonicalByteLimit").is_none());
    assert_eq!(
        serde_json::from_value::<ExternalInputBinding>(raw).unwrap(),
        binding()
    );
}
#[test]
#[cfg(unix)]
fn binding_is_private_nonsymlink_create_once_and_exact_occurrence() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let dir = std::env::temp_dir().join(format!("cutex-ingress-binding-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let b = binding();
    let path = b.stage(&dir).unwrap();
    b.verify(&dir).unwrap();
    assert!(b.stage(&dir).is_err());
    let mut wrong = b.clone();
    wrong.runtime_generation += 1;
    assert!(wrong.verify(&dir).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(b.verify(&dir).is_err());
    std::fs::rename(&path, dir.join("retained")).unwrap();
    symlink("retained", &path).unwrap();
    assert!(b.verify(&dir).is_err());
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_file(dir.join("retained")).unwrap();
    std::fs::remove_dir(dir).unwrap();
}
