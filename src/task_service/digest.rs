use serde::Serialize;
use sha2::{Digest, Sha256 as Sha256Hasher};

use super::model::{
    JournalEvent, JournalRecord, JournalSchema, Sha256, StoreRevision, TaskServiceError,
    TransitionEnvelope,
};

const COMMAND_DOMAIN: &[u8] = b"cutex/task-service-transition/v1\0";
const EVENT_DOMAIN: &[u8] = b"cutex/task-service-event/v1\0";

#[derive(Serialize)]
struct CanonicalEnvelope<'a> {
    schema: super::model::EnvelopeSchema,
    receipt_id: &'a super::model::ReceiptId,
    expected_store_revision: StoreRevision,
    fence: &'a Option<super::model::AttemptFence>,
    command: &'a super::model::TaskCommand,
}

#[derive(Serialize)]
struct CanonicalEvent<'a> {
    schema: JournalSchema,
    sequence: u64,
    previous_event_sha256: &'a Sha256,
    store_revision: StoreRevision,
    event: &'a JournalEvent,
}

pub fn canonical_command_digest(envelope: &TransitionEnvelope) -> Result<Sha256, TaskServiceError> {
    let material = CanonicalEnvelope {
        schema: envelope.schema,
        receipt_id: &envelope.receipt_id,
        expected_store_revision: envelope.expected_store_revision,
        fence: &envelope.fence,
        command: &envelope.command,
    };
    digest_serialized(COMMAND_DOMAIN, &material)
}

pub(super) fn event_hash(
    sequence: u64,
    previous_event_sha256: &Sha256,
    store_revision: StoreRevision,
    event: &JournalEvent,
) -> Result<Sha256, TaskServiceError> {
    let material = CanonicalEvent {
        schema: JournalSchema::V1,
        sequence,
        previous_event_sha256,
        store_revision,
        event,
    };
    digest_serialized(EVENT_DOMAIN, &material)
}

pub(super) fn make_record(
    sequence: u64,
    previous_event_sha256: Sha256,
    store_revision: StoreRevision,
    event: JournalEvent,
) -> Result<JournalRecord, TaskServiceError> {
    let event_sha256 = event_hash(sequence, &previous_event_sha256, store_revision, &event)?;
    Ok(JournalRecord {
        schema: JournalSchema::V1,
        sequence,
        previous_event_sha256,
        event_sha256,
        store_revision,
        event,
    })
}

pub(super) fn compact_record_line(record: &JournalRecord) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(record)
        .expect("JournalRecord contains no fallible serde representation");
    bytes.push(b'\n');
    bytes
}

fn digest_serialized<T: Serialize>(
    domain: &[u8],
    material: &T,
) -> Result<Sha256, TaskServiceError> {
    let bytes = serde_json::to_vec(material).map_err(|_| TaskServiceError::Serialization)?;
    let mut hasher = Sha256Hasher::new();
    hasher.update(domain);
    hasher.update(bytes);
    let bytes = hasher.finalize();
    Ok(sha256_bytes_from_digest(&bytes))
}

fn sha256_bytes_from_digest(bytes: &[u8]) -> Sha256 {
    // sha256_bytes is used here only as a stable hexadecimal encoder by hashing
    // an already-domain-separated digest would be wrong, so encode directly.
    let mut encoded = String::with_capacity(64);
    for byte in bytes {
        use std::fmt::Write;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    Sha256::new(encoded).expect("a SHA-256 digest is valid")
}
