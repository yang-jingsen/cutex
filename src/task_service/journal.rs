use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

use super::digest::{canonical_command_digest, compact_record_line, event_hash, make_record};
use super::model::{
    sha256_bytes, validate_cursor_shape, EventPage, EventPageRequest, JournalCursor, JournalEvent,
    JournalRecord, JournalTailRecovered, PageSchema, RecoveryIntent, RecoverySchema, StoreRevision,
    TaskServiceError, ValidationCode, MAX_JSON_SAFE_INTEGER,
};
use super::persist::{self, FaultController, RootHandle};

pub(super) struct ParsedJournal {
    pub(super) records: Vec<JournalRecord>,
    pub(super) complete_prefix_length: usize,
    pub(super) suffix: Vec<u8>,
}

pub(super) struct RecoveredJournal {
    pub(super) records: Vec<JournalRecord>,
    pub(super) recovery_applied: bool,
    pub(super) cleanup_intent: bool,
}

pub(super) fn parse_journal(bytes: &[u8]) -> Result<ParsedJournal, TaskServiceError> {
    let complete_prefix_length = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |position| position + 1);
    let mut records = Vec::new();
    let mut previous = super::model::zero_sha256();
    for terminated_line in bytes[..complete_prefix_length].split_inclusive(|byte| *byte == b'\n') {
        let line = &terminated_line[..terminated_line.len() - 1];
        if line.is_empty() {
            return Err(TaskServiceError::InvalidJournal {
                code: ValidationCode::InvalidJson,
            });
        }
        let record: JournalRecord =
            serde_json::from_slice(line).map_err(|_| TaskServiceError::InvalidJournal {
                code: ValidationCode::InvalidJson,
            })?;
        let expected_sequence = records.len() as u64 + 1;
        if record.sequence != expected_sequence || record.sequence > MAX_JSON_SAFE_INTEGER {
            return Err(TaskServiceError::InvalidJournal {
                code: ValidationCode::InvalidSequence,
            });
        }
        if record.previous_event_sha256 != previous {
            return Err(TaskServiceError::InvalidJournal {
                code: ValidationCode::InvalidPreviousHash,
            });
        }
        let expected_hash = event_hash(
            record.sequence,
            &record.previous_event_sha256,
            record.store_revision,
            &record.event,
        )?;
        if record.event_sha256 != expected_hash {
            return Err(TaskServiceError::InvalidJournal {
                code: ValidationCode::InvalidEventHash,
            });
        }
        validate_record_body(&record)?;
        previous = record.event_sha256.clone();
        records.push(record);
    }
    Ok(ParsedJournal {
        records,
        complete_prefix_length,
        suffix: bytes[complete_prefix_length..].to_vec(),
    })
}

pub(super) fn recover_journal(
    root: &RootHandle,
    bytes: &[u8],
    recovery: Option<RecoveryIntent>,
    faults: &FaultController,
) -> Result<RecoveredJournal, TaskServiceError> {
    let parsed = parse_journal(bytes)?;
    if parsed.suffix.is_empty() && recovery.is_none() {
        return Ok(RecoveredJournal {
            records: parsed.records,
            recovery_applied: false,
            cleanup_intent: false,
        });
    }

    let intent = match recovery {
        Some(intent) => {
            validate_recovery_intent(&intent)?;
            intent
        }
        None => {
            if parsed.suffix.is_empty() {
                return Err(invalid_recovery());
            }
            let intent = intent_from_suffix(&parsed)?;
            persist::persist_recovery_intent(root, &intent, faults)?;
            intent
        }
    };

    if recovery_record_is_complete(bytes, &parsed, &intent)? {
        return Ok(RecoveredJournal {
            records: parsed.records,
            recovery_applied: true,
            cleanup_intent: true,
        });
    }

    if parsed.complete_prefix_length as u64 != intent.complete_prefix_length {
        return Err(invalid_recovery());
    }
    let previous = parsed
        .records
        .last()
        .map(|record| record.event_sha256.clone())
        .unwrap_or_else(super::model::zero_sha256);
    if previous != intent.previous_event_sha256
        || intent.target_sequence != parsed.records.len() as u64 + 1
    {
        return Err(invalid_recovery());
    }
    let store_revision = parsed
        .records
        .last()
        .map(|record| record.store_revision)
        .unwrap_or_else(|| StoreRevision::new(1).expect("revision one is valid"));
    let recovery_record = recovery_record(&intent, store_revision)?;
    let recovery_line = compact_record_line(&recovery_record);
    let remainder = &bytes[parsed.complete_prefix_length..];
    let original_suffix = decode_intent_suffix(&intent)?;
    let state_is_valid = remainder == original_suffix.as_slice()
        || remainder.is_empty()
        || (remainder.len() < recovery_line.len() && recovery_line.starts_with(remainder));
    if !state_is_valid {
        return Err(invalid_recovery());
    }

    if !remainder.is_empty() {
        persist::truncate_for_recovery(root, intent.complete_prefix_length, faults)?;
    }
    persist::append_recovery_record(root, &recovery_line, faults)?;
    let durable_bytes = persist::read_journal(root)?.ok_or(TaskServiceError::InvalidJournal {
        code: ValidationCode::InvalidSequence,
    })?;
    let durable = parse_journal(&durable_bytes)?;
    if !durable.suffix.is_empty()
        || durable.records.last() != Some(&recovery_record)
        || durable.complete_prefix_length != durable_bytes.len()
    {
        return Err(invalid_recovery());
    }
    Ok(RecoveredJournal {
        records: durable.records,
        recovery_applied: true,
        cleanup_intent: true,
    })
}

pub(super) fn page_records(
    records: &[JournalRecord],
    request: &EventPageRequest,
) -> Result<EventPage, TaskServiceError> {
    validate_cursor(records, &request.cursor)?;
    let start = request.cursor.sequence as usize;
    let mut delivered = Vec::new();
    let mut scan_index = start;
    while scan_index < records.len() && delivered.len() < request.limit as usize {
        let record = &records[scan_index];
        if deliverable(record, request.task_id.as_ref()) {
            delivered.push(record.clone());
        }
        scan_index += 1;
    }
    let reached_head = !records[scan_index..]
        .iter()
        .any(|record| deliverable(record, request.task_id.as_ref()));
    let continuation = delivered
        .last()
        .map(JournalRecord::cursor)
        .unwrap_or_else(|| request.cursor.clone());
    Ok(EventPage {
        schema: PageSchema::V1,
        records: delivered,
        continuation,
        reached_head,
    })
}

pub(super) fn validate_cursor(
    records: &[JournalRecord],
    cursor: &JournalCursor,
) -> Result<(), TaskServiceError> {
    validate_cursor_shape(cursor).map_err(|_| TaskServiceError::InvalidCursor)?;
    if cursor.sequence == 0 {
        return Ok(());
    }
    let index =
        usize::try_from(cursor.sequence - 1).map_err(|_| TaskServiceError::InvalidCursor)?;
    let Some(record) = records.get(index) else {
        return Err(TaskServiceError::InvalidCursor);
    };
    if record.event_sha256 != cursor.event_sha256 {
        return Err(TaskServiceError::InvalidCursor);
    }
    Ok(())
}

pub(super) fn deliverable(record: &JournalRecord, task_id: Option<&super::model::TaskId>) -> bool {
    match (&record.event, task_id) {
        (JournalEvent::SystemJournalTailRecovered(_), _) => true,
        (JournalEvent::Transition(_), None) => true,
        (JournalEvent::Transition(event), Some(task_id)) => &event.response.task_id == task_id,
    }
}

fn validate_record_body(record: &JournalRecord) -> Result<(), TaskServiceError> {
    match &record.event {
        JournalEvent::Transition(event) => {
            let envelope = &event.envelope;
            let response = &event.response;
            let digest = canonical_command_digest(envelope)?;
            if envelope.request_digest_sha256 != digest
                || envelope.receipt_id != response.receipt_id
                || response.committed_store_revision != record.store_revision
                || response.resulting_phase != command_resulting_phase(&envelope.command)
            {
                return Err(TaskServiceError::InvalidJournal {
                    code: ValidationCode::InvalidTransitionEvent,
                });
            }
        }
        JournalEvent::SystemJournalTailRecovered(recovery) => {
            if recovery.discarded_byte_count == 0
                || recovery.discarded_byte_count > MAX_JSON_SAFE_INTEGER
            {
                return Err(TaskServiceError::InvalidJournal {
                    code: ValidationCode::InvalidTransitionEvent,
                });
            }
        }
    }
    Ok(())
}

pub(super) fn command_resulting_phase(
    command: &super::model::TaskCommand,
) -> super::model::TaskPhase {
    use super::model::TaskCommand::*;
    use super::model::TaskPhase;
    match command {
        CreateDraft(_) => TaskPhase::Draft,
        Publish(_) => TaskPhase::Published,
        CancelDraft(_) | CancelPublished(_) | CancelDelivered(_) | CancelAccepted(_)
        | CancelRunning(_) | CancelWaiting(_) | CancelBlocked(_) | CancelReview(_) => {
            TaskPhase::Cancelled
        }
        RecordDelivery(_) => TaskPhase::Delivered,
        Accept(_) => TaskPhase::Accepted,
        Reject(_) => TaskPhase::Rejected,
        Start(_) | ResumeWaiting(_) | ResumeBlocked(_) => TaskPhase::Running,
        EnterWaiting(_) => TaskPhase::Waiting,
        EnterBlocked(_) | BlockWaiting(_) => TaskPhase::Blocked,
        MarkReviewReady(_) => TaskPhase::ReviewReady,
        CompleteRunning(_) | CompleteReview(_) => TaskPhase::Completed,
        FailRunning(_) | FailReview(_) => TaskPhase::Failed,
    }
}

fn intent_from_suffix(parsed: &ParsedJournal) -> Result<RecoveryIntent, TaskServiceError> {
    let previous_event_sha256 = parsed
        .records
        .last()
        .map(|record| record.event_sha256.clone())
        .unwrap_or_else(super::model::zero_sha256);
    let complete_prefix_length =
        u64::try_from(parsed.complete_prefix_length).map_err(|_| invalid_recovery())?;
    let suffix_byte_count = u64::try_from(parsed.suffix.len()).map_err(|_| invalid_recovery())?;
    let target_sequence = parsed.records.len() as u64 + 1;
    if suffix_byte_count == 0
        || suffix_byte_count > MAX_JSON_SAFE_INTEGER
        || complete_prefix_length > MAX_JSON_SAFE_INTEGER
        || target_sequence > MAX_JSON_SAFE_INTEGER
    {
        return Err(invalid_recovery());
    }
    Ok(RecoveryIntent {
        schema: RecoverySchema::V1,
        complete_prefix_length,
        suffix_byte_count,
        suffix_sha256: sha256_bytes(&parsed.suffix),
        suffix_base64: BASE64.encode(&parsed.suffix),
        previous_event_sha256,
        target_sequence,
    })
}

fn validate_recovery_intent(intent: &RecoveryIntent) -> Result<(), TaskServiceError> {
    let suffix = decode_intent_suffix(intent)?;
    if intent.suffix_byte_count == 0
        || intent.suffix_byte_count > MAX_JSON_SAFE_INTEGER
        || intent.complete_prefix_length > MAX_JSON_SAFE_INTEGER
        || intent.target_sequence == 0
        || intent.target_sequence > MAX_JSON_SAFE_INTEGER
        || intent.suffix_byte_count != suffix.len() as u64
        || intent.suffix_sha256 != sha256_bytes(&suffix)
    {
        return Err(invalid_recovery());
    }
    Ok(())
}

fn decode_intent_suffix(intent: &RecoveryIntent) -> Result<Vec<u8>, TaskServiceError> {
    BASE64
        .decode(&intent.suffix_base64)
        .map_err(|_| invalid_recovery())
}

fn recovery_record(
    intent: &RecoveryIntent,
    store_revision: StoreRevision,
) -> Result<JournalRecord, TaskServiceError> {
    make_record(
        intent.target_sequence,
        intent.previous_event_sha256.clone(),
        store_revision,
        JournalEvent::SystemJournalTailRecovered(JournalTailRecovered {
            discarded_byte_count: intent.suffix_byte_count,
            discarded_suffix_sha256: intent.suffix_sha256.clone(),
        }),
    )
}

fn recovery_record_is_complete(
    bytes: &[u8],
    parsed: &ParsedJournal,
    intent: &RecoveryIntent,
) -> Result<bool, TaskServiceError> {
    let Some(last) = parsed.records.last() else {
        return Ok(false);
    };
    if last.sequence != intent.target_sequence {
        return Ok(false);
    }
    let JournalEvent::SystemJournalTailRecovered(body) = &last.event else {
        return Err(invalid_recovery());
    };
    if last.previous_event_sha256 != intent.previous_event_sha256
        || body.discarded_byte_count != intent.suffix_byte_count
        || body.discarded_suffix_sha256 != intent.suffix_sha256
        || !parsed.suffix.is_empty()
    {
        return Err(invalid_recovery());
    }
    let expected = compact_record_line(last);
    let prefix = usize::try_from(intent.complete_prefix_length).map_err(|_| invalid_recovery())?;
    if bytes.get(prefix..) != Some(expected.as_slice()) {
        return Err(invalid_recovery());
    }
    Ok(true)
}

fn invalid_recovery() -> TaskServiceError {
    TaskServiceError::InvalidRecoveryIntent {
        code: ValidationCode::InvalidRecoveryIntent,
    }
}
