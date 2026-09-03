//! Durable logical-seat occupancy for authenticated Task Service authority.
//!
//! Management authentication is the only product boundary allowed to mutate
//! this store. Agent Bus callers can resolve an already-authenticated durable
//! session against it, but cannot claim a seat or epoch in a request body.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::role_revision::Sha256 as TypedSha256;
use crate::role_revision::{CutexSessionId, Rfc3339, MAX_JSON_SAFE_INTEGER};
use crate::rotation::{
    ReleaseRotationBoundary, ReleaseRotationExternalStep, ReleaseRotationRecord,
    ReleaseRotationRecordSchema, ReleaseRotationRequest, ReleaseRotationStatus, ReleaseTemplate,
};

pub(crate) const RELEASE_ROTATION_RESTART_BETWEEN_BOUNDARIES: &str =
    "rotation_interrupted_after_restart";
pub(crate) const RELEASE_ROTATION_RESTART_EXTERNAL_OUTCOME_UNKNOWN: &str =
    "external_outcome_unknown_after_restart";
#[cfg(test)]
use crate::task_service::ProviderError;
use crate::task_service::{ActionId, AuthenticatedPrincipal, SeatId};

const STORE_FILE: &str = "seat-occupancy-v1.json";
const LOCK_FILE: &str = "seat-occupancy-v1.lock";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SeatOccupancyStoreSchema {
    #[serde(rename = "cutex/seat-occupancy-store/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SeatOccupancyCommandSchema {
    #[serde(rename = "cutex/seat-occupancy-command/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SeatOccupancyReceiptSchema {
    #[serde(rename = "cutex/seat-occupancy-receipt/v1")]
    V1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeatOccupancyBindRequest {
    pub schema: SeatOccupancyCommandSchema,
    pub action_id: ActionId,
    pub seat_id: SeatId,
    pub occupant_cutex_session: CutexSessionId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeatOccupancy {
    pub seat_id: SeatId,
    pub occupant_cutex_session: CutexSessionId,
    pub epoch: u64,
    pub bound_at: Rfc3339,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeatOccupancyReceipt {
    pub schema: SeatOccupancyReceiptSchema,
    pub action_id: ActionId,
    pub request_sha256: String,
    pub store_revision: u64,
    pub occupancy: SeatOccupancy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeatRotatingVacancy {
    pub seat_id: SeatId,
    pub action_id: ActionId,
    pub request_sha256: TypedSha256,
    pub predecessor_cutex_session: CutexSessionId,
    pub epoch: u64,
    pub vacated_at: Rfc3339,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeatOccupancySnapshot {
    pub schema: SeatOccupancyStoreSchema,
    pub store_revision: u64,
    pub occupancies: BTreeMap<SeatId, SeatOccupancy>,
    pub receipts: BTreeMap<ActionId, SeatOccupancyReceipt>,
    #[serde(default)]
    pub rotating_vacancies: BTreeMap<SeatId, SeatRotatingVacancy>,
    #[serde(default)]
    pub release_rotations: BTreeMap<ActionId, ReleaseRotationRecord>,
    #[serde(default)]
    pub active_release_rotation: Option<ActionId>,
}

impl SeatOccupancySnapshot {
    fn empty() -> Self {
        Self {
            schema: SeatOccupancyStoreSchema::V1,
            store_revision: 0,
            occupancies: BTreeMap::new(),
            receipts: BTreeMap::new(),
            rotating_vacancies: BTreeMap::new(),
            release_rotations: BTreeMap::new(),
            active_release_rotation: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SeatAuthorityError {
    InvalidRequest(&'static str),
    Unauthorized,
    Conflict(&'static str),
    PersistenceUnavailable,
    InvalidStore,
    Io(io::ErrorKind),
}

impl fmt::Display for SeatAuthorityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "seat authority error: {self:?}")
    }
}

impl std::error::Error for SeatAuthorityError {}

impl From<io::Error> for SeatAuthorityError {
    fn from(value: io::Error) -> Self {
        Self::Io(value.kind())
    }
}

#[derive(Clone)]
pub(crate) struct SeatOccupancyStore {
    root: Arc<PathBuf>,
    process_lock: Arc<Mutex<()>>,
}

impl SeatOccupancyStore {
    pub(crate) fn open(root: impl Into<PathBuf>) -> Result<Self, SeatAuthorityError> {
        let root = root.into();
        prepare_private_root(&root)?;
        Ok(Self {
            root: Arc::new(root),
            process_lock: Arc::new(Mutex::new(())),
        })
    }

    pub(crate) fn open_default() -> anyhow::Result<Self> {
        Self::open(
            crate::config::paths::runtime_dir()?
                .join("task-service")
                .join("seat-authority-v1"),
        )
        .map_err(anyhow::Error::new)
    }

    pub(crate) fn bind(
        &self,
        request: &SeatOccupancyBindRequest,
    ) -> Result<SeatOccupancyReceipt, SeatAuthorityError> {
        if !matches!(request.seat_id.as_str(), "cutex-director" | "cutex-release") {
            return Err(SeatAuthorityError::InvalidRequest("unsupported_seat"));
        }
        let digest = request_digest(request)?;
        self.with_locked_state(true, |mut state| {
            if let Some(receipt) = state.receipts.get(&request.action_id).cloned() {
                return if receipt.request_sha256 == digest {
                    Ok((state, receipt, false))
                } else {
                    Err(SeatAuthorityError::Conflict("action_id_payload_conflict"))
                };
            }
            if state.occupancies.iter().any(|(seat, occupancy)| {
                seat != &request.seat_id
                    && occupancy.occupant_cutex_session == request.occupant_cutex_session
            }) {
                return Err(SeatAuthorityError::Conflict(
                    "session_already_occupies_another_seat",
                ));
            }
            if state.rotating_vacancies.contains_key(&request.seat_id)
                || (request.seat_id.as_str() == "cutex-release"
                    && state.active_release_rotation.is_some())
            {
                return Err(SeatAuthorityError::Conflict("seat_rotation_in_progress"));
            }
            let epoch = state
                .occupancies
                .get(&request.seat_id)
                .map_or(Ok(1), |current| {
                    current
                        .epoch
                        .checked_add(1)
                        .filter(|value| *value <= MAX_JSON_SAFE_INTEGER)
                        .ok_or(SeatAuthorityError::Conflict("seat_epoch_overflow"))
                })?;
            let revision = state
                .store_revision
                .checked_add(1)
                .filter(|value| *value <= MAX_JSON_SAFE_INTEGER)
                .ok_or(SeatAuthorityError::Conflict("store_revision_overflow"))?;
            let occupancy = SeatOccupancy {
                seat_id: request.seat_id.clone(),
                occupant_cutex_session: request.occupant_cutex_session.clone(),
                epoch,
                bound_at: now(),
            };
            let receipt = SeatOccupancyReceipt {
                schema: SeatOccupancyReceiptSchema::V1,
                action_id: request.action_id.clone(),
                request_sha256: digest,
                store_revision: revision,
                occupancy: occupancy.clone(),
            };
            state.store_revision = revision;
            state.occupancies.insert(request.seat_id.clone(), occupancy);
            state
                .receipts
                .insert(request.action_id.clone(), receipt.clone());
            Ok((state, receipt, true))
        })
    }

    pub(crate) fn query(&self) -> Result<SeatOccupancySnapshot, SeatAuthorityError> {
        self.with_locked_state(false, |state| Ok((state.clone(), state, false)))
    }

    /// Atomically commit the Release authority boundary and its durable
    /// rotation record. All potentially rejecting preflight checks happen
    /// before this call; the checks repeated here fence stale races.
    pub(crate) fn begin_release_rotation(
        &self,
        director_cutex_session: &CutexSessionId,
        request: &ReleaseRotationRequest,
        request_sha256: &TypedSha256,
        template: &ReleaseTemplate,
        template_sha256: &TypedSha256,
    ) -> Result<ReleaseRotationRecord, SeatAuthorityError> {
        if request.target_seat.as_str() != "cutex-release" {
            return Err(SeatAuthorityError::InvalidRequest(
                "unsupported_rotation_target",
            ));
        }
        self.with_locked_state(true, |mut state| {
            if let Some(record) = state.release_rotations.get(&request.action_id).cloned() {
                return if &record.request_sha256 == request_sha256 {
                    Ok((state, record, false))
                } else {
                    Err(SeatAuthorityError::Conflict("action_id_payload_conflict"))
                };
            }
            if state.active_release_rotation.is_some() {
                return Err(SeatAuthorityError::Conflict("release_rotation_in_progress"));
            }
            let director = state
                .occupancies
                .get(&SeatId::new("cutex-director").map_err(|_| SeatAuthorityError::InvalidStore)?)
                .ok_or(SeatAuthorityError::Unauthorized)?;
            if &director.occupant_cutex_session != director_cutex_session {
                return Err(SeatAuthorityError::Unauthorized);
            }
            let current = state
                .occupancies
                .get(&request.target_seat)
                .ok_or(SeatAuthorityError::Conflict("release_seat_not_bound"))?;
            if current.occupant_cutex_session != request.expected_predecessor_cutex_session
                || current.epoch != request.expected_seat_epoch
            {
                return Err(SeatAuthorityError::Conflict("stale_release_occupancy"));
            }
            if template.version != request.expected_template_version
                || template_sha256 != &request.expected_template_sha256
            {
                return Err(SeatAuthorityError::Conflict("stale_release_template"));
            }
            let revision = next_revision(state.store_revision)?;
            let timestamp = now();
            let vacancy = SeatRotatingVacancy {
                seat_id: request.target_seat.clone(),
                action_id: request.action_id.clone(),
                request_sha256: request_sha256.clone(),
                predecessor_cutex_session: current.occupant_cutex_session.clone(),
                epoch: current.epoch,
                vacated_at: timestamp.clone(),
            };
            let record = ReleaseRotationRecord {
                schema: ReleaseRotationRecordSchema::V1,
                action_id: request.action_id.clone(),
                request_sha256: request_sha256.clone(),
                director_cutex_session: director_cutex_session.clone(),
                target_seat: request.target_seat.clone(),
                predecessor_cutex_session: request.expected_predecessor_cutex_session.clone(),
                predecessor_seat_epoch: request.expected_seat_epoch,
                template: template.clone(),
                template_sha256: template_sha256.clone(),
                starting_message: request.starting_message.clone(),
                status: ReleaseRotationStatus::Running,
                completed_boundary: ReleaseRotationBoundary::SeatRevoked,
                pending_external_step: None,
                successor_cutex_session: None,
                successor_thread_id: None,
                successor_seat_epoch: None,
                delivered_message_id: None,
                blocked_reason: None,
                created_at: timestamp.clone(),
                updated_at: timestamp,
                completed_at: None,
            };
            state.store_revision = revision;
            state.occupancies.remove(&request.target_seat);
            state
                .rotating_vacancies
                .insert(request.target_seat.clone(), vacancy);
            state.active_release_rotation = Some(request.action_id.clone());
            state
                .release_rotations
                .insert(request.action_id.clone(), record.clone());
            Ok((state, record, true))
        })
    }

    pub(crate) fn mutate_release_rotation<T>(
        &self,
        action_id: &ActionId,
        request_sha256: &TypedSha256,
        operation: impl FnOnce(&mut ReleaseRotationRecord) -> Result<T, SeatAuthorityError>,
    ) -> Result<(ReleaseRotationRecord, T), SeatAuthorityError> {
        self.with_locked_state(true, |mut state| {
            let record = state
                .release_rotations
                .get_mut(action_id)
                .ok_or(SeatAuthorityError::Conflict("rotation_not_found"))?;
            if &record.request_sha256 != request_sha256 {
                return Err(SeatAuthorityError::Conflict("action_id_payload_conflict"));
            }
            let value = operation(record)?;
            record.updated_at = now();
            let record = record.clone();
            state.store_revision = next_revision(state.store_revision)?;
            Ok((state, (record, value), true))
        })
    }

    pub(crate) fn mark_release_rotation_pending(
        &self,
        action_id: &ActionId,
        request_sha256: &TypedSha256,
        expected_status: ReleaseRotationStatus,
        expected_boundary: ReleaseRotationBoundary,
        step: ReleaseRotationExternalStep,
    ) -> Result<ReleaseRotationRecord, SeatAuthorityError> {
        self.mutate_release_rotation(action_id, request_sha256, |record| {
            if record.status == ReleaseRotationStatus::Complete {
                return Err(SeatAuthorityError::Conflict("rotation_already_complete"));
            }
            if record.status != expected_status
                || record.completed_boundary != expected_boundary
                || record.pending_external_step.is_some()
            {
                return Err(SeatAuthorityError::Conflict("external_boundary_changed"));
            }
            record.status = ReleaseRotationStatus::Running;
            record.blocked_reason = None;
            record.pending_external_step = Some(step);
            Ok(())
        })
        .map(|(record, ())| record)
    }

    pub(crate) fn block_release_rotation(
        &self,
        action_id: &ActionId,
        request_sha256: &TypedSha256,
        reason: String,
    ) -> Result<ReleaseRotationRecord, SeatAuthorityError> {
        self.mutate_release_rotation(action_id, request_sha256, |record| {
            if record.status == ReleaseRotationStatus::Complete {
                return Err(SeatAuthorityError::Conflict("rotation_already_complete"));
            }
            record.status = ReleaseRotationStatus::Blocked;
            record.pending_external_step = None;
            record.blocked_reason = Some(reason);
            Ok(())
        })
        .map(|(record, ())| record)
    }

    /// On provider restart, durably classify every nonterminal operation that
    /// depended on the lost in-memory execution frame. The exact completed
    /// boundary and optional pending step are preserved for Management-root
    /// recovery; the active rotation fence is untouched.
    pub(crate) fn block_interrupted_release_rotations(&self) -> Result<usize, SeatAuthorityError> {
        self.with_locked_state(true, |mut state| {
            let timestamp = now();
            let mut changed = 0usize;
            for record in state.release_rotations.values_mut() {
                if record.status == ReleaseRotationStatus::Running {
                    record.status = ReleaseRotationStatus::Blocked;
                    record.blocked_reason = Some(
                        if record.pending_external_step.is_some() {
                            RELEASE_ROTATION_RESTART_EXTERNAL_OUTCOME_UNKNOWN
                        } else {
                            RELEASE_ROTATION_RESTART_BETWEEN_BOUNDARIES
                        }
                        .to_string(),
                    );
                    record.updated_at = timestamp.clone();
                    changed += 1;
                }
            }
            if changed > 0 {
                state.store_revision = next_revision(state.store_revision)?;
            }
            Ok((state, changed, changed > 0))
        })
    }

    /// Explicitly re-arm one restart-classified record. A pending edge is
    /// mechanically repeatable; a record without one resumes only from its
    /// exact known completed boundary.
    pub(crate) fn resume_interrupted_release_rotation(
        &self,
        action_id: &ActionId,
        request_sha256: &TypedSha256,
        expected_boundary: ReleaseRotationBoundary,
        expected_step: Option<ReleaseRotationExternalStep>,
    ) -> Result<ReleaseRotationRecord, SeatAuthorityError> {
        self.mutate_release_rotation(action_id, request_sha256, |record| {
            if record.status != ReleaseRotationStatus::Blocked
                || record.completed_boundary != expected_boundary
                || record.pending_external_step != expected_step
            {
                return Err(SeatAuthorityError::Conflict("recovery_identity_changed"));
            }
            let expected_reason = if expected_step.is_some() {
                RELEASE_ROTATION_RESTART_EXTERNAL_OUTCOME_UNKNOWN
            } else {
                RELEASE_ROTATION_RESTART_BETWEEN_BOUNDARIES
            };
            if record.blocked_reason.as_deref() != Some(expected_reason) {
                return Err(SeatAuthorityError::Conflict(
                    "rotation_is_not_restart_blocked",
                ));
            }
            record.status = ReleaseRotationStatus::Running;
            record.pending_external_step = None;
            record.blocked_reason = None;
            Ok(())
        })
        .map(|(record, ())| record)
    }

    pub(crate) fn bind_release_rotation_successor(
        &self,
        action_id: &ActionId,
        request_sha256: &TypedSha256,
        successor: &CutexSessionId,
    ) -> Result<ReleaseRotationRecord, SeatAuthorityError> {
        self.with_locked_state(true, |mut state| {
            let existing = state
                .release_rotations
                .get(action_id)
                .cloned()
                .ok_or(SeatAuthorityError::Conflict("rotation_not_found"))?;
            if &existing.request_sha256 != request_sha256 {
                return Err(SeatAuthorityError::Conflict("action_id_payload_conflict"));
            }
            if existing.completed_boundary == ReleaseRotationBoundary::SuccessorBound
                || existing.completed_boundary == ReleaseRotationBoundary::DirectorMessageDelivered
            {
                if existing.successor_cutex_session.as_ref() == Some(successor) {
                    return Ok((state, existing, false));
                }
                return Err(SeatAuthorityError::Conflict("successor_payload_conflict"));
            }
            if existing.completed_boundary != ReleaseRotationBoundary::SuccessorRuntimeOnline {
                return Err(SeatAuthorityError::Conflict("successor_runtime_not_online"));
            }
            if existing.status != ReleaseRotationStatus::Running
                || existing.pending_external_step.is_some()
            {
                return Err(SeatAuthorityError::Conflict("external_boundary_changed"));
            }
            if existing.successor_cutex_session.as_ref() != Some(successor) {
                return Err(SeatAuthorityError::Conflict("successor_payload_conflict"));
            }
            let vacancy = state
                .rotating_vacancies
                .get(&existing.target_seat)
                .cloned()
                .ok_or(SeatAuthorityError::Conflict("rotation_vacancy_missing"))?;
            if vacancy.action_id != *action_id
                || vacancy.request_sha256 != *request_sha256
                || vacancy.predecessor_cutex_session != existing.predecessor_cutex_session
                || vacancy.epoch != existing.predecessor_seat_epoch
            {
                return Err(SeatAuthorityError::InvalidStore);
            }
            if state.occupancies.contains_key(&existing.target_seat) {
                return Err(SeatAuthorityError::Conflict(
                    "release_seat_already_occupied",
                ));
            }
            if state.occupancies.iter().any(|(seat, occupancy)| {
                seat != &existing.target_seat && &occupancy.occupant_cutex_session == successor
            }) {
                return Err(SeatAuthorityError::Conflict(
                    "session_already_occupies_another_seat",
                ));
            }
            let epoch = vacancy
                .epoch
                .checked_add(1)
                .filter(|value| *value <= MAX_JSON_SAFE_INTEGER)
                .ok_or(SeatAuthorityError::Conflict("seat_epoch_overflow"))?;
            let timestamp = now();
            let occupancy = SeatOccupancy {
                seat_id: existing.target_seat.clone(),
                occupant_cutex_session: successor.clone(),
                epoch,
                bound_at: timestamp.clone(),
            };
            let record = state
                .release_rotations
                .get_mut(action_id)
                .expect("rotation existence checked");
            record.status = ReleaseRotationStatus::Running;
            record.completed_boundary = ReleaseRotationBoundary::SuccessorBound;
            record.pending_external_step = None;
            record.successor_seat_epoch = Some(epoch);
            record.blocked_reason = None;
            record.updated_at = timestamp;
            let record = record.clone();
            state
                .occupancies
                .insert(existing.target_seat.clone(), occupancy);
            state.rotating_vacancies.remove(&existing.target_seat);
            state.store_revision = next_revision(state.store_revision)?;
            Ok((state, record, true))
        })
    }

    pub(crate) fn complete_release_rotation(
        &self,
        action_id: &ActionId,
        request_sha256: &TypedSha256,
        message_id: String,
    ) -> Result<ReleaseRotationRecord, SeatAuthorityError> {
        self.with_locked_state(true, |mut state| {
            let existing = state
                .release_rotations
                .get(action_id)
                .cloned()
                .ok_or(SeatAuthorityError::Conflict("rotation_not_found"))?;
            if &existing.request_sha256 != request_sha256 {
                return Err(SeatAuthorityError::Conflict("action_id_payload_conflict"));
            }
            if existing.status == ReleaseRotationStatus::Complete {
                return if existing.delivered_message_id.as_deref() == Some(&message_id) {
                    Ok((state, existing, false))
                } else {
                    Err(SeatAuthorityError::Conflict("message_payload_conflict"))
                };
            }
            if existing.completed_boundary != ReleaseRotationBoundary::SuccessorBound {
                return Err(SeatAuthorityError::Conflict("successor_not_bound"));
            }
            if existing.status != ReleaseRotationStatus::Running
                || existing.pending_external_step
                    != Some(ReleaseRotationExternalStep::DeliverDirectorMessage)
            {
                return Err(SeatAuthorityError::Conflict("external_boundary_changed"));
            }
            let timestamp = now();
            let record = state
                .release_rotations
                .get_mut(action_id)
                .expect("rotation existence checked");
            record.status = ReleaseRotationStatus::Complete;
            record.completed_boundary = ReleaseRotationBoundary::DirectorMessageDelivered;
            record.pending_external_step = None;
            record.delivered_message_id = Some(message_id);
            record.blocked_reason = None;
            record.updated_at = timestamp.clone();
            record.completed_at = Some(timestamp);
            let record = record.clone();
            state.active_release_rotation = None;
            state.store_revision = next_revision(state.store_revision)?;
            Ok((state, record, true))
        })
    }

    /// Resolve a stable session against the latest durable occupancy. This is
    /// called after runtime-to-session authentication and for every seated
    /// Task Service request, so a rebind removes its predecessor immediately.
    #[cfg(test)]
    pub(crate) fn resolve_principal(
        &self,
        cutex_session_id: &CutexSessionId,
    ) -> Result<AuthenticatedPrincipal, SeatAuthorityError> {
        let snapshot = self.query()?;
        let occupancy = snapshot
            .occupancies
            .values()
            .find(|occupancy| &occupancy.occupant_cutex_session == cutex_session_id)
            .ok_or(SeatAuthorityError::Unauthorized)?;
        AuthenticatedPrincipal::seated_session(
            cutex_session_id.clone(),
            occupancy.seat_id.clone(),
            occupancy.epoch,
        )
        .map_err(|error| match error {
            ProviderError::InvalidRequest(reason) => SeatAuthorityError::InvalidRequest(reason),
            _ => SeatAuthorityError::InvalidStore,
        })
    }

    /// Execute one seated operation while holding the occupancy read lock.
    /// Management rebind takes the exclusive form of the same lock, giving a
    /// linearizable authority boundary: either the predecessor operation wins
    /// before rebind, or it observes the new occupant and is rejected.
    pub(crate) fn with_current_principal<T>(
        &self,
        cutex_session_id: &CutexSessionId,
        operation: impl FnOnce(&AuthenticatedPrincipal) -> T,
    ) -> Result<T, SeatAuthorityError> {
        self.with_current_principal_snapshot(cutex_session_id, |principal, _| operation(principal))
    }

    /// Execute one seated operation against the same authoritative occupancy
    /// snapshot that authenticated its principal. Callers that need another
    /// seat during the operation must use this snapshot rather than recursively
    /// entering the occupancy store while its shared authority lock is held.
    pub(crate) fn with_current_principal_snapshot<T>(
        &self,
        cutex_session_id: &CutexSessionId,
        operation: impl FnOnce(&AuthenticatedPrincipal, &SeatOccupancySnapshot) -> T,
    ) -> Result<T, SeatAuthorityError> {
        let _process = self
            .process_lock
            .lock()
            .map_err(|_| SeatAuthorityError::PersistenceUnavailable)?;
        let lock_path = self.root.join(LOCK_FILE);
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        set_private_open_options(&mut options);
        let lock = match options.open(lock_path) {
            Ok(lock) => lock,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(SeatAuthorityError::Unauthorized)
            }
            Err(error) => return Err(error.into()),
        };
        lock.lock_shared()?;
        let snapshot = read_snapshot(&self.root)?;
        let occupancy = snapshot
            .occupancies
            .values()
            .find(|occupancy| &occupancy.occupant_cutex_session == cutex_session_id)
            .ok_or(SeatAuthorityError::Unauthorized)?;
        let principal = AuthenticatedPrincipal::seated_session(
            cutex_session_id.clone(),
            occupancy.seat_id.clone(),
            occupancy.epoch,
        )
        .map_err(|_| SeatAuthorityError::InvalidStore)?;
        Ok(operation(&principal, &snapshot))
    }

    fn with_locked_state<T>(
        &self,
        create: bool,
        operation: impl FnOnce(
            SeatOccupancySnapshot,
        ) -> Result<(SeatOccupancySnapshot, T, bool), SeatAuthorityError>,
    ) -> Result<T, SeatAuthorityError> {
        let _process = self
            .process_lock
            .lock()
            .map_err(|_| SeatAuthorityError::PersistenceUnavailable)?;
        let lock_path = self.root.join(LOCK_FILE);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(create);
        set_private_open_options(&mut options);
        let lock = match options.open(&lock_path) {
            Ok(lock) => lock,
            Err(error) if !create && error.kind() == io::ErrorKind::NotFound => {
                let (state, value, write) = operation(SeatOccupancySnapshot::empty())?;
                debug_assert!(!write);
                let _ = state;
                return Ok(value);
            }
            Err(error) => return Err(error.into()),
        };
        lock.lock_exclusive()?;
        let state = read_snapshot(&self.root)?;
        let (state, value, write) = operation(state)?;
        if write {
            write_snapshot(&self.root, &state)?;
        }
        Ok(value)
    }
}

fn prepare_private_root(root: &Path) -> Result<(), SeatAuthorityError> {
    if let Ok(metadata) = fs::symlink_metadata(root) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(SeatAuthorityError::InvalidStore);
        }
    } else {
        fs::create_dir_all(root)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    crate::platform::private_fs::secure_tree(root)
        .map_err(|error| SeatAuthorityError::Io(error.io_kind()))?;
    Ok(())
}

fn read_snapshot(root: &Path) -> Result<SeatOccupancySnapshot, SeatAuthorityError> {
    let path = root.join(STORE_FILE);
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(SeatOccupancySnapshot::empty())
        }
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(|_| SeatAuthorityError::InvalidStore)
}

fn write_snapshot(root: &Path, snapshot: &SeatOccupancySnapshot) -> Result<(), SeatAuthorityError> {
    let bytes = serde_json::to_vec(snapshot).map_err(|_| SeatAuthorityError::InvalidStore)?;
    let temporary = root.join(format!(".{STORE_FILE}.{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    set_private_open_options(&mut options);
    let mut file = options.open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        fs::rename(&temporary, root.join(STORE_FILE))?;
        File::open(root)?.sync_all()?;
    }
    #[cfg(windows)]
    {
        let (directory, identity) = crate::platform::private_fs::open_validated_directory(root)
            .map_err(|error| SeatAuthorityError::Io(error.io_kind()))?;
        let source = temporary
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(SeatAuthorityError::InvalidStore)?;
        crate::platform::private_fs::replace_child(root, identity, source, STORE_FILE)
            .map_err(|error| SeatAuthorityError::Io(error.io_kind()))?;
        crate::platform::private_fs::sync_directory(&directory)
            .map_err(|error| SeatAuthorityError::Io(error.io_kind()))?;
    }
    Ok(())
}

fn request_digest(request: &SeatOccupancyBindRequest) -> Result<String, SeatAuthorityError> {
    let bytes = serde_json::to_vec(request)
        .map_err(|_| SeatAuthorityError::InvalidRequest("invalid_request"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn next_revision(current: u64) -> Result<u64, SeatAuthorityError> {
    current
        .checked_add(1)
        .filter(|value| *value <= MAX_JSON_SAFE_INTEGER)
        .ok_or(SeatAuthorityError::Conflict("store_revision_overflow"))
}

fn now() -> Rfc3339 {
    Rfc3339::new(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .expect("UTC timestamp is RFC3339")
}

#[cfg(unix)]
fn set_private_open_options(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
}

#[cfg(windows)]
fn set_private_open_options(_options: &mut OpenOptions) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(label: &str) -> PathBuf {
        let root = std::env::var_os("CUTEX_TASK_SERVICE_TEST_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("seat-{label}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("create seat test root");
        root
    }

    fn bind(action: &str, seat: &str, session: &str) -> SeatOccupancyBindRequest {
        SeatOccupancyBindRequest {
            schema: SeatOccupancyCommandSchema::V1,
            action_id: ActionId::new(action).expect("action"),
            seat_id: SeatId::new(seat).expect("seat"),
            occupant_cutex_session: CutexSessionId::new(session).expect("session"),
        }
    }

    #[test]
    fn restart_preserves_occupancy_and_exact_replay() {
        let root = root("restart");
        let store = SeatOccupancyStore::open(&root).expect("open");
        let request = bind("bind-1", "cutex-director", "session-director");
        let first = store.bind(&request).expect("bind");
        drop(store);
        let reopened = SeatOccupancyStore::open(&root).expect("reopen");
        assert_eq!(reopened.bind(&request).expect("replay"), first);
        assert_eq!(reopened.query().expect("query").occupancies.len(), 1);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn rebind_increments_epoch_and_revokes_predecessor() {
        let root = root("rebind");
        let store = SeatOccupancyStore::open(&root).expect("open");
        let predecessor = CutexSessionId::new("session-old").expect("session");
        store
            .bind(&bind("bind-old", "cutex-release", predecessor.as_str()))
            .expect("first bind");
        assert!(store.resolve_principal(&predecessor).is_ok());
        let successor = CutexSessionId::new("session-new").expect("session");
        let rebound = store
            .bind(&bind("bind-new", "cutex-release", successor.as_str()))
            .expect("rebind");
        assert_eq!(rebound.occupancy.epoch, 2);
        assert_eq!(
            store.resolve_principal(&predecessor),
            Err(SeatAuthorityError::Unauthorized)
        );
        assert!(store.resolve_principal(&successor).is_ok());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn authoritative_snapshot_keeps_rebind_linearizable_without_recursive_reads() {
        let root = root("snapshot-rebind");
        let store = SeatOccupancyStore::open(&root).expect("open");
        let predecessor = CutexSessionId::new("session-old-snapshot").expect("session");
        store
            .bind(&bind(
                "bind-old-snapshot",
                "cutex-director",
                predecessor.as_str(),
            ))
            .expect("first bind");

        let authenticated_store = store.clone();
        let authenticated_session = predecessor.clone();
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let authenticated = std::thread::spawn(move || {
            authenticated_store
                .with_current_principal_snapshot(&authenticated_session, |principal, snapshot| {
                    assert_eq!(
                        principal.authenticated_session_id().unwrap(),
                        &authenticated_session
                    );
                    assert_eq!(
                        snapshot
                            .occupancies
                            .get(&SeatId::new("cutex-director").unwrap())
                            .unwrap()
                            .occupant_cutex_session,
                        authenticated_session
                    );
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                })
                .expect("authenticated snapshot");
        });
        entered_rx.recv().unwrap();

        let rebinding_store = store.clone();
        let (rebound_tx, rebound_rx) = std::sync::mpsc::sync_channel(1);
        let rebinding = std::thread::spawn(move || {
            let receipt = rebinding_store
                .bind(&bind(
                    "bind-new-snapshot",
                    "cutex-director",
                    "session-new-snapshot",
                ))
                .expect("rebind");
            rebound_tx.send(receipt).unwrap();
        });
        assert!(rebound_rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_err());
        release_tx.send(()).unwrap();
        authenticated.join().unwrap();
        assert_eq!(
            rebound_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap()
                .occupancy
                .epoch,
            2
        );
        rebinding.join().unwrap();
        assert_eq!(
            store.resolve_principal(&predecessor),
            Err(SeatAuthorityError::Unauthorized)
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn strict_bind_body_rejects_forged_runtime_or_epoch_fields() {
        for forbidden in ["runtime_id", "seat_epoch", "attempt_token", "cas_revision"] {
            let mut value =
                serde_json::to_value(bind("bind-strict", "cutex-director", "session-director"))
                    .expect("serialize");
            value[forbidden] = serde_json::json!("forged");
            assert!(serde_json::from_value::<SeatOccupancyBindRequest>(value).is_err());
        }
    }
}
