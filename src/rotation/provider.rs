//! Release rotation state machine.

use std::fmt;
use std::sync::OnceLock;

use crate::role_revision::{CutexSessionId, MAX_JSON_SAFE_INTEGER};
use crate::seat::{
    SeatAuthorityError, SeatOccupancyStore, RELEASE_ROTATION_RESTART_BETWEEN_BOUNDARIES,
};
use crate::task_service::ActionId;

use super::template_store::digest;
use super::{
    ReleaseRotationBoundary, ReleaseRotationExternalStep, ReleaseRotationReceipt,
    ReleaseRotationRecord, ReleaseRotationRequest, ReleaseRotationStatus, ReleaseTemplate,
    ReleaseTemplateError, ReleaseTemplateStore, RetryReleaseRotationRequest,
};

pub const RELEASE_ROTATION_MAX_MESSAGE_BYTES: usize = 256 * 1024;

static DEFAULT_RESTART_CLASSIFICATION: OnceLock<Result<(), ReleaseRotationError>> = OnceLock::new();
const RELEASE_ROTATION_EXTERNAL_STEP_FAILED: &str = "external_step_failed";

/// Typed external lifecycle edges. Implementations must target exactly the
/// supplied durable session and must not restore a predecessor on failure.
pub trait ReleaseRotationLifecycle: Send + Sync {
    fn preflight_successor(&self, template: &ReleaseTemplate) -> anyhow::Result<()>;

    fn predecessor_has_active_turn(&self, predecessor: &CutexSessionId) -> anyhow::Result<bool>;

    fn predecessor_thread_id(&self, predecessor: &CutexSessionId)
        -> anyhow::Result<Option<String>>;

    fn offline_predecessor(&self, predecessor: &CutexSessionId) -> anyhow::Result<()>;

    fn retire_predecessor(&self, predecessor: &CutexSessionId) -> anyhow::Result<()>;

    fn create_successor_session(
        &self,
        template: &ReleaseTemplate,
    ) -> anyhow::Result<CutexSessionId>;

    fn verify_successor_session(
        &self,
        successor: &CutexSessionId,
        template: &ReleaseTemplate,
    ) -> anyhow::Result<bool>;

    fn start_successor_thread(
        &self,
        successor: &CutexSessionId,
        template: &ReleaseTemplate,
    ) -> anyhow::Result<String>;

    fn verify_successor_thread(
        &self,
        successor: &CutexSessionId,
        thread_id: &str,
    ) -> anyhow::Result<bool>;

    fn launch_successor_runtime(&self, successor: &CutexSessionId) -> anyhow::Result<()>;

    fn deliver_director_message(
        &self,
        director: &CutexSessionId,
        successor: &CutexSessionId,
        action_id: &ActionId,
        exact_message: &str,
    ) -> anyhow::Result<String>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReleaseRotationError {
    InvalidRequest(&'static str),
    Unauthorized,
    Conflict(&'static str),
    Blocked(ReleaseRotationReceipt),
    PersistenceUnavailable,
    InvalidStore,
}

impl ReleaseRotationError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "invalid_request",
            Self::Unauthorized => "unauthorized",
            Self::Conflict(_) => "conflict",
            Self::Blocked(_) => "blocked",
            Self::PersistenceUnavailable => "persistence_unavailable",
            Self::InvalidStore => "invalid_store",
        }
    }
}

impl fmt::Display for ReleaseRotationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest(reason) | Self::Conflict(reason) => formatter.write_str(reason),
            Self::Unauthorized => formatter.write_str("unauthorized"),
            Self::Blocked(receipt) => write!(
                formatter,
                "rotation blocked after {:?}: {}",
                receipt.completed_boundary,
                receipt
                    .blocked_reason
                    .as_deref()
                    .unwrap_or("external_failure")
            ),
            Self::PersistenceUnavailable => formatter.write_str("persistence_unavailable"),
            Self::InvalidStore => formatter.write_str("invalid_store"),
        }
    }
}

impl std::error::Error for ReleaseRotationError {}

impl From<SeatAuthorityError> for ReleaseRotationError {
    fn from(value: SeatAuthorityError) -> Self {
        match value {
            SeatAuthorityError::InvalidRequest(reason) => Self::InvalidRequest(reason),
            SeatAuthorityError::Unauthorized => Self::Unauthorized,
            SeatAuthorityError::Conflict(reason) => Self::Conflict(reason),
            SeatAuthorityError::PersistenceUnavailable | SeatAuthorityError::Io(_) => {
                Self::PersistenceUnavailable
            }
            SeatAuthorityError::InvalidStore => Self::InvalidStore,
        }
    }
}

impl From<ReleaseTemplateError> for ReleaseRotationError {
    fn from(value: ReleaseTemplateError) -> Self {
        match value {
            ReleaseTemplateError::InvalidRequest(reason) => Self::InvalidRequest(reason),
            ReleaseTemplateError::Conflict(reason) => Self::Conflict(reason),
            ReleaseTemplateError::PersistenceUnavailable | ReleaseTemplateError::Io(_) => {
                Self::PersistenceUnavailable
            }
            ReleaseTemplateError::InvalidStore => Self::InvalidStore,
        }
    }
}

#[derive(Clone)]
pub struct ReleaseRotationProvider {
    seats: SeatOccupancyStore,
    templates: ReleaseTemplateStore,
}

impl ReleaseRotationProvider {
    pub(crate) fn new(seats: SeatOccupancyStore, templates: ReleaseTemplateStore) -> Self {
        Self { seats, templates }
    }

    #[cfg(test)]
    pub(crate) fn reopen(
        seats: SeatOccupancyStore,
        templates: ReleaseTemplateStore,
    ) -> Result<Self, ReleaseRotationError> {
        let provider = Self::new(seats, templates);
        provider.seats.block_interrupted_release_rotations()?;
        Ok(provider)
    }

    pub fn open_default() -> anyhow::Result<Self> {
        let provider = Self::new(
            SeatOccupancyStore::open_default()?,
            ReleaseTemplateStore::open_default()?,
        );
        DEFAULT_RESTART_CLASSIFICATION
            .get_or_init(|| {
                provider
                    .seats
                    .block_interrupted_release_rotations()
                    .map(|_| ())
                    .map_err(Into::into)
            })
            .clone()
            .map_err(anyhow::Error::new)?;
        Ok(provider)
    }

    /// Ordinary authenticated Director request. `has_nonterminal_assignment`
    /// is derived by the co-located Task Service host while holding its
    /// execution mutex; it is never supplied by the request body.
    pub fn request_director(
        &self,
        director_cutex_session: &CutexSessionId,
        request: &ReleaseRotationRequest,
        has_nonterminal_assignment: bool,
        lifecycle: &dyn ReleaseRotationLifecycle,
    ) -> Result<ReleaseRotationReceipt, ReleaseRotationError> {
        validate_request(request)?;
        let request_sha256 = digest(request)?;
        let seat_snapshot = self.seats.query()?;
        if let Some(existing) = seat_snapshot.release_rotations.get(&request.action_id) {
            if existing.request_sha256 != request_sha256 {
                return Err(ReleaseRotationError::Conflict("action_id_payload_conflict"));
            }
            return Ok(ReleaseRotationReceipt::from(existing));
        }
        let director = seat_snapshot
            .occupancies
            .values()
            .find(|occupancy| occupancy.seat_id.as_str() == "cutex-director")
            .ok_or(ReleaseRotationError::Unauthorized)?;
        if &director.occupant_cutex_session != director_cutex_session {
            return Err(ReleaseRotationError::Unauthorized);
        }
        if seat_snapshot.active_release_rotation.is_some() {
            return Err(ReleaseRotationError::Conflict(
                "release_rotation_in_progress",
            ));
        }
        let release = seat_snapshot
            .occupancies
            .get(&request.target_seat)
            .ok_or(ReleaseRotationError::Conflict("release_seat_not_bound"))?;
        if release.occupant_cutex_session != request.expected_predecessor_cutex_session
            || release.epoch != request.expected_seat_epoch
        {
            return Err(ReleaseRotationError::Conflict("stale_release_occupancy"));
        }
        let template_snapshot = self.templates.query()?;
        let template = template_snapshot
            .current_template
            .ok_or(ReleaseRotationError::Conflict(
                "release_template_not_configured",
            ))?;
        let template_sha256 = template_snapshot
            .current_template_sha256
            .ok_or(ReleaseRotationError::InvalidStore)?;
        if template.version != request.expected_template_version
            || template_sha256 != request.expected_template_sha256
        {
            return Err(ReleaseRotationError::Conflict("stale_release_template"));
        }
        if has_nonterminal_assignment {
            return Err(ReleaseRotationError::Conflict(
                "predecessor_has_nonterminal_assignment",
            ));
        }
        if lifecycle
            .predecessor_has_active_turn(&request.expected_predecessor_cutex_session)
            .map_err(|_| ReleaseRotationError::Conflict("active_turn_status_unavailable"))?
        {
            return Err(ReleaseRotationError::Conflict(
                "predecessor_has_active_turn",
            ));
        }
        lifecycle
            .preflight_successor(&template)
            .map_err(|_| ReleaseRotationError::Conflict("successor_preflight_failed"))?;
        let record = self
            .templates
            .with_current_template(|current, current_sha256| {
                if current.version != request.expected_template_version
                    || current_sha256 != &request.expected_template_sha256
                {
                    return Err(ReleaseRotationError::Conflict("stale_release_template"));
                }
                self.seats
                    .begin_release_rotation(
                        director_cutex_session,
                        request,
                        &request_sha256,
                        current,
                        current_sha256,
                    )
                    .map_err(Into::into)
            })??;
        self.execute(record, lifecycle)
    }

    /// Management-root explicit continuation. It never changes the original
    /// request or restores the predecessor; only a blocked exact record can
    /// continue.
    pub fn retry_root(
        &self,
        request: &RetryReleaseRotationRequest,
        lifecycle: &dyn ReleaseRotationLifecycle,
    ) -> Result<ReleaseRotationReceipt, ReleaseRotationError> {
        let snapshot = self.seats.query()?;
        let record = snapshot
            .release_rotations
            .get(&request.action_id)
            .cloned()
            .ok_or(ReleaseRotationError::Conflict("rotation_not_found"))?;
        if record.request_sha256 != request.expected_request_sha256 {
            return Err(ReleaseRotationError::Conflict("action_id_payload_conflict"));
        }
        if record.completed_boundary != request.expected_completed_boundary {
            return Err(ReleaseRotationError::Conflict("completed_boundary_changed"));
        }
        if record.pending_external_step != request.expected_pending_external_step {
            return Err(ReleaseRotationError::Conflict(
                "pending_external_identity_changed",
            ));
        }
        match record.status {
            ReleaseRotationStatus::Complete => Ok(ReleaseRotationReceipt::from(&record)),
            ReleaseRotationStatus::Running => Err(ReleaseRotationError::Conflict(
                "release_rotation_in_progress",
            )),
            ReleaseRotationStatus::Blocked => {
                let corrected = if let Some(step) = record.pending_external_step {
                    if pending_step_boundary(step) != record.completed_boundary {
                        return Err(ReleaseRotationError::InvalidStore);
                    }
                    match step {
                        ReleaseRotationExternalStep::CreateSuccessorSession
                        | ReleaseRotationExternalStep::StartSuccessorThread => {
                            self.apply_explicit_correction(record, request, lifecycle)?
                        }
                        _ => {
                            if request.corrected_successor_cutex_session.is_some()
                                || request.corrected_successor_thread_id.is_some()
                            {
                                return Err(ReleaseRotationError::InvalidRequest(
                                    "unexpected_rotation_correction",
                                ));
                            }
                            self.seats.resume_interrupted_release_rotation(
                                &record.action_id,
                                &record.request_sha256,
                                request.expected_completed_boundary,
                                Some(step),
                            )?
                        }
                    }
                } else if record.blocked_reason.as_deref()
                    == Some(RELEASE_ROTATION_RESTART_BETWEEN_BOUNDARIES)
                {
                    if request.corrected_successor_cutex_session.is_some()
                        || request.corrected_successor_thread_id.is_some()
                    {
                        return Err(ReleaseRotationError::InvalidRequest(
                            "unexpected_rotation_correction",
                        ));
                    }
                    self.seats.resume_interrupted_release_rotation(
                        &record.action_id,
                        &record.request_sha256,
                        request.expected_completed_boundary,
                        None,
                    )?
                } else if record.blocked_reason.as_deref()
                    == Some(RELEASE_ROTATION_EXTERNAL_STEP_FAILED)
                    && record.completed_boundary == ReleaseRotationBoundary::SuccessorSessionCreated
                    && request.corrected_successor_cutex_session.is_none()
                    && request.corrected_successor_thread_id.is_none()
                {
                    self.resume_clean_synchronous_thread_start(&record, lifecycle)?
                } else {
                    self.apply_explicit_correction(record, request, lifecycle)?
                };
                self.execute(corrected, lifecycle)
            }
        }
    }

    fn resume_clean_synchronous_thread_start(
        &self,
        record: &ReleaseRotationRecord,
        lifecycle: &dyn ReleaseRotationLifecycle,
    ) -> Result<ReleaseRotationRecord, ReleaseRotationError> {
        let successor = record
            .successor_cutex_session
            .as_ref()
            .ok_or(ReleaseRotationError::InvalidStore)?;
        if successor == &record.predecessor_cutex_session
            || !lifecycle
                .verify_successor_session(successor, &record.template)
                .map_err(|_| {
                    ReleaseRotationError::Conflict("clean_successor_session_status_unavailable")
                })?
        {
            return Err(ReleaseRotationError::Conflict(
                "successor_session_is_not_clean_for_retry",
            ));
        }
        self.seats
            .mutate_release_rotation(&record.action_id, &record.request_sha256, |current| {
                if current.status != ReleaseRotationStatus::Blocked
                    || current.completed_boundary
                        != ReleaseRotationBoundary::SuccessorSessionCreated
                    || current.pending_external_step.is_some()
                    || current.blocked_reason.as_deref()
                        != Some(RELEASE_ROTATION_EXTERNAL_STEP_FAILED)
                    || current.successor_cutex_session.as_ref() != Some(successor)
                {
                    return Err(SeatAuthorityError::Conflict(
                        "clean_thread_start_retry_identity_changed",
                    ));
                }
                current.status = ReleaseRotationStatus::Running;
                current.blocked_reason = None;
                Ok(())
            })
            .map(|(record, ())| record)
            .map_err(Into::into)
    }

    fn apply_explicit_correction(
        &self,
        record: ReleaseRotationRecord,
        request: &RetryReleaseRotationRequest,
        lifecycle: &dyn ReleaseRotationLifecycle,
    ) -> Result<ReleaseRotationRecord, ReleaseRotationError> {
        match record.completed_boundary {
            ReleaseRotationBoundary::PredecessorRetired => {
                let successor = self.require_verified_successor_session(
                    &record,
                    request.corrected_successor_cutex_session.as_ref(),
                    request.corrected_successor_thread_id.as_deref(),
                    lifecycle,
                )?;
                self.finish_pending_correction(
                    &record,
                    request.expected_completed_boundary,
                    request.expected_pending_external_step,
                    ReleaseRotationBoundary::SuccessorSessionCreated,
                    |record| record.successor_cutex_session = Some(successor),
                )
            }
            ReleaseRotationBoundary::SuccessorSessionCreated => {
                let thread_id = self.require_verified_successor_thread(
                    &record,
                    request.corrected_successor_cutex_session.as_ref(),
                    request.corrected_successor_thread_id.as_deref(),
                    lifecycle,
                )?;
                self.finish_pending_correction(
                    &record,
                    request.expected_completed_boundary,
                    request.expected_pending_external_step,
                    ReleaseRotationBoundary::SuccessorThreadStarted,
                    |record| record.successor_thread_id = Some(thread_id),
                )
            }
            _ => {
                if request.corrected_successor_cutex_session.is_some()
                    || request.corrected_successor_thread_id.is_some()
                {
                    return Err(ReleaseRotationError::InvalidRequest(
                        "unexpected_rotation_correction",
                    ));
                }
                Ok(record)
            }
        }
    }

    fn require_verified_successor_session(
        &self,
        record: &ReleaseRotationRecord,
        corrected_successor: Option<&CutexSessionId>,
        corrected_thread: Option<&str>,
        lifecycle: &dyn ReleaseRotationLifecycle,
    ) -> Result<CutexSessionId, ReleaseRotationError> {
        let successor = corrected_successor.ok_or(ReleaseRotationError::Conflict(
            "explicit_successor_session_correction_required",
        ))?;
        if corrected_thread.is_some()
            || successor == &record.predecessor_cutex_session
            || !lifecycle
                .verify_successor_session(successor, &record.template)
                .map_err(|_| {
                    ReleaseRotationError::Conflict("successor_session_correction_unavailable")
                })?
        {
            return Err(ReleaseRotationError::Conflict(
                "invalid_successor_session_correction",
            ));
        }
        Ok(successor.clone())
    }

    fn require_verified_successor_thread(
        &self,
        record: &ReleaseRotationRecord,
        corrected_successor: Option<&CutexSessionId>,
        corrected_thread: Option<&str>,
        lifecycle: &dyn ReleaseRotationLifecycle,
    ) -> Result<String, ReleaseRotationError> {
        if corrected_successor.is_some() {
            return Err(ReleaseRotationError::Conflict(
                "invalid_successor_thread_correction",
            ));
        }
        let successor = record
            .successor_cutex_session
            .as_ref()
            .ok_or(ReleaseRotationError::InvalidStore)?;
        let thread_id = corrected_thread
            .filter(|value| !value.trim().is_empty())
            .ok_or(ReleaseRotationError::Conflict(
                "explicit_successor_thread_correction_required",
            ))?;
        let predecessor_thread = lifecycle
            .predecessor_thread_id(&record.predecessor_cutex_session)
            .map_err(|_| ReleaseRotationError::Conflict("predecessor_thread_status_unavailable"))?;
        if predecessor_thread.as_deref() == Some(thread_id)
            || !lifecycle
                .verify_successor_thread(successor, thread_id)
                .map_err(|_| {
                    ReleaseRotationError::Conflict("successor_thread_correction_unavailable")
                })?
        {
            return Err(ReleaseRotationError::Conflict(
                "invalid_successor_thread_correction",
            ));
        }
        Ok(thread_id.to_string())
    }

    fn finish_pending_correction(
        &self,
        record: &ReleaseRotationRecord,
        expected_completed_boundary: ReleaseRotationBoundary,
        expected_pending_external_step: Option<ReleaseRotationExternalStep>,
        boundary: ReleaseRotationBoundary,
        update: impl FnOnce(&mut ReleaseRotationRecord),
    ) -> Result<ReleaseRotationRecord, ReleaseRotationError> {
        self.seats
            .mutate_release_rotation(&record.action_id, &record.request_sha256, |record| {
                if record.status != ReleaseRotationStatus::Blocked {
                    return Err(SeatAuthorityError::Conflict("rotation_is_not_blocked"));
                }
                if record.completed_boundary != expected_completed_boundary {
                    return Err(SeatAuthorityError::Conflict("completed_boundary_changed"));
                }
                if record.pending_external_step != expected_pending_external_step {
                    return Err(SeatAuthorityError::Conflict(
                        "pending_external_identity_changed",
                    ));
                }
                update(record);
                record.completed_boundary = boundary;
                record.pending_external_step = None;
                record.status = ReleaseRotationStatus::Running;
                record.blocked_reason = None;
                Ok(())
            })
            .map(|(record, ())| record)
            .map_err(Into::into)
    }

    pub fn query(
        &self,
        action_id: Option<&ActionId>,
    ) -> Result<Vec<ReleaseRotationReceipt>, ReleaseRotationError> {
        let snapshot = self.seats.query()?;
        if let Some(action_id) = action_id {
            return Ok(snapshot
                .release_rotations
                .get(action_id)
                .map(ReleaseRotationReceipt::from)
                .into_iter()
                .collect());
        }
        Ok(snapshot
            .release_rotations
            .values()
            .map(ReleaseRotationReceipt::from)
            .collect())
    }

    fn execute(
        &self,
        mut record: ReleaseRotationRecord,
        lifecycle: &dyn ReleaseRotationLifecycle,
    ) -> Result<ReleaseRotationReceipt, ReleaseRotationError> {
        loop {
            record = match record.completed_boundary {
                ReleaseRotationBoundary::SeatRevoked => self.external_unit_step(
                    &record,
                    ReleaseRotationExternalStep::OfflinePredecessor,
                    ReleaseRotationBoundary::PredecessorOfflined,
                    || lifecycle.offline_predecessor(&record.predecessor_cutex_session),
                )?,
                ReleaseRotationBoundary::PredecessorOfflined => self.external_unit_step(
                    &record,
                    ReleaseRotationExternalStep::RetirePredecessor,
                    ReleaseRotationBoundary::PredecessorRetired,
                    || lifecycle.retire_predecessor(&record.predecessor_cutex_session),
                )?,
                ReleaseRotationBoundary::PredecessorRetired => {
                    let pending = self.mark_pending(
                        &record,
                        ReleaseRotationExternalStep::CreateSuccessorSession,
                    )?;
                    let successor = match lifecycle.create_successor_session(&pending.template) {
                        Ok(successor) if successor != pending.predecessor_cutex_session => {
                            successor
                        }
                        Ok(_) => {
                            return self.blocked(&pending, "successor_reused_predecessor_session")
                        }
                        Err(error) => return self.blocked_external(&pending, &error),
                    };
                    self.finish_pending(
                        &pending,
                        ReleaseRotationExternalStep::CreateSuccessorSession,
                        ReleaseRotationBoundary::SuccessorSessionCreated,
                        |record| record.successor_cutex_session = Some(successor),
                    )?
                }
                ReleaseRotationBoundary::SuccessorSessionCreated => {
                    let successor = match record.successor_cutex_session.clone() {
                        Some(successor) => successor,
                        None => return self.blocked(&record, "successor_session_missing"),
                    };
                    let predecessor_thread =
                        match lifecycle.predecessor_thread_id(&record.predecessor_cutex_session) {
                            Ok(thread) => thread,
                            Err(error) => return self.blocked_external(&record, &error),
                        };
                    let pending = self
                        .mark_pending(&record, ReleaseRotationExternalStep::StartSuccessorThread)?;
                    let thread_id = match lifecycle
                        .start_successor_thread(&successor, &pending.template)
                    {
                        Ok(thread_id)
                            if !thread_id.trim().is_empty()
                                && predecessor_thread.as_deref() != Some(thread_id.as_str()) =>
                        {
                            thread_id
                        }
                        Ok(_) => {
                            return self
                                .blocked(&pending, "thread_start_did_not_return_distinct_thread")
                        }
                        Err(error) => return self.blocked_external(&pending, &error),
                    };
                    self.finish_pending(
                        &pending,
                        ReleaseRotationExternalStep::StartSuccessorThread,
                        ReleaseRotationBoundary::SuccessorThreadStarted,
                        |record| record.successor_thread_id = Some(thread_id),
                    )?
                }
                ReleaseRotationBoundary::SuccessorThreadStarted => {
                    let successor = match record.successor_cutex_session.clone() {
                        Some(successor) => successor,
                        None => return self.blocked(&record, "successor_session_missing"),
                    };
                    self.external_unit_step(
                        &record,
                        ReleaseRotationExternalStep::LaunchSuccessorRuntime,
                        ReleaseRotationBoundary::SuccessorRuntimeOnline,
                        || lifecycle.launch_successor_runtime(&successor),
                    )?
                }
                ReleaseRotationBoundary::SuccessorRuntimeOnline => {
                    let successor = match record.successor_cutex_session.clone() {
                        Some(successor) => successor,
                        None => return self.blocked(&record, "successor_session_missing"),
                    };
                    self.seats.bind_release_rotation_successor(
                        &record.action_id,
                        &record.request_sha256,
                        &successor,
                    )?
                }
                ReleaseRotationBoundary::SuccessorBound => {
                    let successor = match record.successor_cutex_session.clone() {
                        Some(successor) => successor,
                        None => return self.blocked(&record, "successor_session_missing"),
                    };
                    let pending = self.mark_pending(
                        &record,
                        ReleaseRotationExternalStep::DeliverDirectorMessage,
                    )?;
                    let message_id = match lifecycle.deliver_director_message(
                        &pending.director_cutex_session,
                        &successor,
                        &pending.action_id,
                        &pending.starting_message,
                    ) {
                        Ok(message_id) if !message_id.trim().is_empty() => message_id,
                        Ok(_) => {
                            return self.blocked(&pending, "message_delivery_omitted_message_id")
                        }
                        Err(error) => return self.blocked_external(&pending, &error),
                    };
                    self.seats.complete_release_rotation(
                        &pending.action_id,
                        &pending.request_sha256,
                        message_id,
                    )?
                }
                ReleaseRotationBoundary::DirectorMessageDelivered => {
                    return Ok(ReleaseRotationReceipt::from(&record));
                }
            };
            if record.status == ReleaseRotationStatus::Complete {
                return Ok(ReleaseRotationReceipt::from(&record));
            }
        }
    }

    fn external_unit_step(
        &self,
        record: &ReleaseRotationRecord,
        step: ReleaseRotationExternalStep,
        boundary: ReleaseRotationBoundary,
        operation: impl FnOnce() -> anyhow::Result<()>,
    ) -> Result<ReleaseRotationRecord, ReleaseRotationError> {
        let pending = self.mark_pending(record, step)?;
        if let Err(error) = operation() {
            return self.blocked_external(&pending, &error);
        }
        self.finish_pending(&pending, step, boundary, |_| {})
    }

    fn mark_pending(
        &self,
        record: &ReleaseRotationRecord,
        step: ReleaseRotationExternalStep,
    ) -> Result<ReleaseRotationRecord, ReleaseRotationError> {
        self.seats
            .mark_release_rotation_pending(
                &record.action_id,
                &record.request_sha256,
                record.status,
                record.completed_boundary,
                step,
            )
            .map_err(Into::into)
    }

    fn finish_pending(
        &self,
        record: &ReleaseRotationRecord,
        step: ReleaseRotationExternalStep,
        boundary: ReleaseRotationBoundary,
        update: impl FnOnce(&mut ReleaseRotationRecord),
    ) -> Result<ReleaseRotationRecord, ReleaseRotationError> {
        let expected_completed_boundary = record.completed_boundary;
        self.seats
            .mutate_release_rotation(&record.action_id, &record.request_sha256, |record| {
                if record.status != ReleaseRotationStatus::Running
                    || record.completed_boundary != expected_completed_boundary
                    || record.pending_external_step != Some(step)
                {
                    return Err(SeatAuthorityError::Conflict("external_boundary_changed"));
                }
                update(record);
                record.completed_boundary = boundary;
                record.pending_external_step = None;
                record.status = ReleaseRotationStatus::Running;
                record.blocked_reason = None;
                Ok(())
            })
            .map(|(record, ())| record)
            .map_err(Into::into)
    }

    fn blocked<T>(
        &self,
        record: &ReleaseRotationRecord,
        reason: &str,
    ) -> Result<T, ReleaseRotationError> {
        let blocked = self.seats.block_release_rotation(
            &record.action_id,
            &record.request_sha256,
            sanitize_reason(reason),
        )?;
        Err(ReleaseRotationError::Blocked(ReleaseRotationReceipt::from(
            &blocked,
        )))
    }

    fn blocked_external<T>(
        &self,
        record: &ReleaseRotationRecord,
        _error: &anyhow::Error,
    ) -> Result<T, ReleaseRotationError> {
        // External lifecycle errors can contain commands, paths, profile
        // diagnostics or transport credentials. The completed boundary and
        // pending step already identify the causal location; persist only a
        // stable credential-safe reason.
        self.blocked(record, RELEASE_ROTATION_EXTERNAL_STEP_FAILED)
    }
}

fn pending_step_boundary(step: ReleaseRotationExternalStep) -> ReleaseRotationBoundary {
    match step {
        ReleaseRotationExternalStep::OfflinePredecessor => ReleaseRotationBoundary::SeatRevoked,
        ReleaseRotationExternalStep::RetirePredecessor => {
            ReleaseRotationBoundary::PredecessorOfflined
        }
        ReleaseRotationExternalStep::CreateSuccessorSession => {
            ReleaseRotationBoundary::PredecessorRetired
        }
        ReleaseRotationExternalStep::StartSuccessorThread => {
            ReleaseRotationBoundary::SuccessorSessionCreated
        }
        ReleaseRotationExternalStep::LaunchSuccessorRuntime => {
            ReleaseRotationBoundary::SuccessorThreadStarted
        }
        ReleaseRotationExternalStep::DeliverDirectorMessage => {
            ReleaseRotationBoundary::SuccessorBound
        }
    }
}

fn validate_request(request: &ReleaseRotationRequest) -> Result<(), ReleaseRotationError> {
    if request.target_seat.as_str() != "cutex-release" {
        return Err(ReleaseRotationError::InvalidRequest(
            "unsupported_rotation_target",
        ));
    }
    if request.expected_seat_epoch == 0 || request.expected_seat_epoch > MAX_JSON_SAFE_INTEGER {
        return Err(ReleaseRotationError::InvalidRequest(
            "invalid_expected_seat_epoch",
        ));
    }
    if request.expected_template_version == 0
        || request.expected_template_version > MAX_JSON_SAFE_INTEGER
    {
        return Err(ReleaseRotationError::InvalidRequest(
            "invalid_expected_template_version",
        ));
    }
    if request.starting_message.trim() != request.starting_message
        || request.starting_message.is_empty()
        || request.starting_message.len() > RELEASE_ROTATION_MAX_MESSAGE_BYTES
        || request.starting_message.contains('\0')
    {
        return Err(ReleaseRotationError::InvalidRequest(
            "invalid_starting_message",
        ));
    }
    Ok(())
}

fn sanitize_reason(reason: &str) -> String {
    let mut safe = reason
        .chars()
        .filter(|character| !character.is_control())
        .take(512)
        .collect::<String>();
    for marker in ["Bearer ", "token=", "authorization="] {
        if let Some(index) = safe.to_ascii_lowercase().find(&marker.to_ascii_lowercase()) {
            safe.truncate(index);
            safe.push_str("[redacted]");
            break;
        }
    }
    if safe.is_empty() {
        "external_step_failed".to_string()
    } else {
        safe
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::path::PathBuf;
    use std::sync::Mutex;

    use super::*;
    use crate::role_revision::Sha256;
    use crate::rotation::template_store::tests::template;
    use crate::rotation::{
        ConfigureReleaseTemplateRequest, ReleaseRotationCommandSchema, ReleaseTemplateCommandSchema,
    };
    use crate::seat::{SeatOccupancyBindRequest, SeatOccupancyCommandSchema};
    use crate::task_service::SeatId;

    fn session(value: &str) -> CutexSessionId {
        CutexSessionId::new(value).expect("session")
    }

    fn root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "cutex-release-rotation-{label}-{}",
            uuid::Uuid::new_v4()
        ))
    }

    fn bind(action: &str, seat: &str, occupant: &str) -> SeatOccupancyBindRequest {
        SeatOccupancyBindRequest {
            schema: SeatOccupancyCommandSchema::V1,
            action_id: ActionId::new(action).expect("action"),
            seat_id: SeatId::new(seat).expect("seat"),
            occupant_cutex_session: session(occupant),
        }
    }

    fn fixture(label: &str) -> (ReleaseRotationProvider, PathBuf, ReleaseRotationRequest) {
        let root = root(label);
        let seats = SeatOccupancyStore::open(root.join("seats")).expect("seats");
        seats
            .bind(&bind("director-bind", "cutex-director", "cutex.director"))
            .expect("director");
        seats
            .bind(&bind("release-bind", "cutex-release", "cutex.release-old"))
            .expect("release");
        let templates = ReleaseTemplateStore::open(root.join("templates")).expect("templates");
        let template_request = ConfigureReleaseTemplateRequest {
            schema: ReleaseTemplateCommandSchema::V1,
            action_id: ActionId::new("template-config").expect("action"),
            expected_current_version: None,
            template: template(1),
        };
        let template_receipt = templates.configure(&template_request).expect("template");
        let request = ReleaseRotationRequest {
            schema: ReleaseRotationCommandSchema::V1,
            action_id: ActionId::new("rotate-release").expect("action"),
            target_seat: SeatId::new("cutex-release").expect("seat"),
            expected_predecessor_cutex_session: session("cutex.release-old"),
            expected_seat_epoch: 1,
            expected_template_version: 1,
            expected_template_sha256: template_receipt.template_sha256,
            starting_message: "Review the frozen candidate.".to_string(),
        };
        (
            ReleaseRotationProvider::new(seats, templates),
            root,
            request,
        )
    }

    fn reopen(root: &std::path::Path) -> ReleaseRotationProvider {
        ReleaseRotationProvider::reopen(
            SeatOccupancyStore::open(root.join("seats")).expect("reopen seats"),
            ReleaseTemplateStore::open(root.join("templates")).expect("reopen templates"),
        )
        .expect("reopen provider")
    }

    fn begin_without_execute(
        provider: &ReleaseRotationProvider,
        request: &ReleaseRotationRequest,
    ) -> ReleaseRotationRecord {
        let request_sha256 = digest(request).expect("request digest");
        provider
            .templates
            .with_current_template(|current, current_sha256| {
                provider.seats.begin_release_rotation(
                    &session("cutex.director"),
                    request,
                    &request_sha256,
                    current,
                    current_sha256,
                )
            })
            .expect("template store")
            .expect("begin rotation")
    }

    fn commit_through_boundary(
        provider: &ReleaseRotationProvider,
        request: &ReleaseRotationRequest,
        target: ReleaseRotationBoundary,
    ) -> ReleaseRotationRecord {
        let mut record = begin_without_execute(provider, request);
        while record.completed_boundary != target {
            record = match record.completed_boundary {
                ReleaseRotationBoundary::SeatRevoked => {
                    let pending = provider
                        .mark_pending(&record, ReleaseRotationExternalStep::OfflinePredecessor)
                        .expect("offline pending");
                    provider
                        .finish_pending(
                            &pending,
                            ReleaseRotationExternalStep::OfflinePredecessor,
                            ReleaseRotationBoundary::PredecessorOfflined,
                            |_| {},
                        )
                        .expect("offline complete")
                }
                ReleaseRotationBoundary::PredecessorOfflined => {
                    let pending = provider
                        .mark_pending(&record, ReleaseRotationExternalStep::RetirePredecessor)
                        .expect("retire pending");
                    provider
                        .finish_pending(
                            &pending,
                            ReleaseRotationExternalStep::RetirePredecessor,
                            ReleaseRotationBoundary::PredecessorRetired,
                            |_| {},
                        )
                        .expect("retire complete")
                }
                ReleaseRotationBoundary::PredecessorRetired => {
                    let pending = provider
                        .mark_pending(&record, ReleaseRotationExternalStep::CreateSuccessorSession)
                        .expect("session pending");
                    provider
                        .finish_pending(
                            &pending,
                            ReleaseRotationExternalStep::CreateSuccessorSession,
                            ReleaseRotationBoundary::SuccessorSessionCreated,
                            |record| {
                                record.successor_cutex_session = Some(session("cutex.release-new"))
                            },
                        )
                        .expect("session complete")
                }
                ReleaseRotationBoundary::SuccessorSessionCreated => {
                    let pending = provider
                        .mark_pending(&record, ReleaseRotationExternalStep::StartSuccessorThread)
                        .expect("thread pending");
                    provider
                        .finish_pending(
                            &pending,
                            ReleaseRotationExternalStep::StartSuccessorThread,
                            ReleaseRotationBoundary::SuccessorThreadStarted,
                            |record| record.successor_thread_id = Some("thread-new".to_string()),
                        )
                        .expect("thread complete")
                }
                ReleaseRotationBoundary::SuccessorThreadStarted => {
                    let pending = provider
                        .mark_pending(&record, ReleaseRotationExternalStep::LaunchSuccessorRuntime)
                        .expect("runtime pending");
                    provider
                        .finish_pending(
                            &pending,
                            ReleaseRotationExternalStep::LaunchSuccessorRuntime,
                            ReleaseRotationBoundary::SuccessorRuntimeOnline,
                            |_| {},
                        )
                        .expect("runtime complete")
                }
                ReleaseRotationBoundary::SuccessorRuntimeOnline => provider
                    .seats
                    .bind_release_rotation_successor(
                        &record.action_id,
                        &record.request_sha256,
                        record
                            .successor_cutex_session
                            .as_ref()
                            .expect("successor session"),
                    )
                    .expect("bind successor"),
                ReleaseRotationBoundary::SuccessorBound
                | ReleaseRotationBoundary::DirectorMessageDelivered => {
                    panic!("target boundary is not reachable as nonterminal")
                }
            };
        }
        record
    }

    #[derive(Default)]
    struct FakeLifecycle {
        calls: Mutex<Vec<&'static str>>,
        fail: Mutex<Option<&'static str>>,
        panic_after: Mutex<Option<&'static str>>,
        active_turn: bool,
        predecessor_thread: Option<String>,
        delivered: Mutex<BTreeSet<String>>,
        verified_successor_sessions: Mutex<Vec<CutexSessionId>>,
        thread_start_successors: Mutex<Vec<CutexSessionId>>,
    }

    impl FakeLifecycle {
        fn record(&self, call: &'static str) -> anyhow::Result<()> {
            self.calls.lock().expect("calls").push(call);
            if self.fail.lock().expect("fail").as_ref() == Some(&call) {
                anyhow::bail!("{call}")
            }
            Ok(())
        }

        fn crash_after(&self, call: &'static str) {
            let should_crash =
                self.panic_after.lock().expect("panic_after").as_ref() == Some(&call);
            if should_crash {
                panic!("simulated process loss after {call}");
            }
        }
    }

    impl ReleaseRotationLifecycle for FakeLifecycle {
        fn predecessor_has_active_turn(&self, _: &CutexSessionId) -> anyhow::Result<bool> {
            Ok(self.active_turn)
        }

        fn preflight_successor(&self, _: &ReleaseTemplate) -> anyhow::Result<()> {
            self.record("preflight")
        }

        fn predecessor_thread_id(&self, _: &CutexSessionId) -> anyhow::Result<Option<String>> {
            self.record("predecessor_thread")?;
            Ok(self.predecessor_thread.clone())
        }

        fn offline_predecessor(&self, _: &CutexSessionId) -> anyhow::Result<()> {
            self.record("offline")?;
            self.crash_after("offline");
            Ok(())
        }

        fn retire_predecessor(&self, _: &CutexSessionId) -> anyhow::Result<()> {
            self.record("retire")?;
            self.crash_after("retire");
            Ok(())
        }

        fn create_successor_session(&self, _: &ReleaseTemplate) -> anyhow::Result<CutexSessionId> {
            self.record("create_session")?;
            self.crash_after("create_session");
            Ok(session("cutex.release-new"))
        }

        fn verify_successor_session(
            &self,
            successor: &CutexSessionId,
            _: &ReleaseTemplate,
        ) -> anyhow::Result<bool> {
            self.record("verify_successor_session")?;
            self.verified_successor_sessions
                .lock()
                .expect("verified successors")
                .push(successor.clone());
            Ok(successor.as_str() == "cutex.release-new")
        }

        fn start_successor_thread(
            &self,
            successor: &CutexSessionId,
            _: &ReleaseTemplate,
        ) -> anyhow::Result<String> {
            self.thread_start_successors
                .lock()
                .expect("thread start successors")
                .push(successor.clone());
            self.record("thread_start")?;
            self.crash_after("thread_start");
            Ok("thread-new".to_string())
        }

        fn verify_successor_thread(
            &self,
            successor: &CutexSessionId,
            thread_id: &str,
        ) -> anyhow::Result<bool> {
            Ok(successor.as_str() == "cutex.release-new" && thread_id == "thread-new")
        }

        fn launch_successor_runtime(&self, _: &CutexSessionId) -> anyhow::Result<()> {
            self.record("runtime_online")?;
            self.crash_after("runtime_online");
            Ok(())
        }

        fn deliver_director_message(
            &self,
            _: &CutexSessionId,
            _: &CutexSessionId,
            action_id: &ActionId,
            _: &str,
        ) -> anyhow::Result<String> {
            self.record("deliver")?;
            let id = format!("rotation:{}", action_id.as_str());
            self.delivered.lock().expect("delivery").insert(id.clone());
            self.crash_after("deliver");
            Ok(id)
        }
    }

    #[test]
    fn complete_rotation_revokes_then_binds_once_and_uses_new_thread() {
        let (provider, root, request) = fixture("complete");
        let lifecycle = FakeLifecycle {
            predecessor_thread: Some("thread-old".to_string()),
            ..Default::default()
        };
        let receipt = provider
            .request_director(&session("cutex.director"), &request, false, &lifecycle)
            .expect("rotate");
        assert_eq!(receipt.status, ReleaseRotationStatus::Complete);
        assert_eq!(receipt.predecessor_seat_epoch, 1);
        assert_eq!(receipt.successor_seat_epoch, Some(2));
        assert_eq!(receipt.successor_thread_id.as_deref(), Some("thread-new"));
        assert_eq!(
            lifecycle.calls.lock().expect("calls").as_slice(),
            [
                "preflight",
                "offline",
                "retire",
                "create_session",
                "predecessor_thread",
                "thread_start",
                "runtime_online",
                "deliver"
            ]
        );
        let replay = provider
            .request_director(&session("cutex.director"), &request, false, &lifecycle)
            .expect("replay");
        assert_eq!(replay, receipt);
        assert_eq!(lifecycle.delivered.lock().expect("delivery").len(), 1);
        let mut changed = request.clone();
        changed.starting_message = "changed".to_string();
        assert_eq!(
            provider.request_director(&session("cutex.director"), &changed, false, &lifecycle,),
            Err(ReleaseRotationError::Conflict("action_id_payload_conflict"))
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn reopen_blocks_and_root_recovers_every_between_boundary_commit() {
        for completed_boundary in [
            ReleaseRotationBoundary::SeatRevoked,
            ReleaseRotationBoundary::PredecessorOfflined,
            ReleaseRotationBoundary::PredecessorRetired,
            ReleaseRotationBoundary::SuccessorSessionCreated,
            ReleaseRotationBoundary::SuccessorThreadStarted,
            ReleaseRotationBoundary::SuccessorRuntimeOnline,
            ReleaseRotationBoundary::SuccessorBound,
        ] {
            let (provider, root, request) = fixture("between-boundary");
            let committed = commit_through_boundary(&provider, &request, completed_boundary);
            assert_eq!(committed.status, ReleaseRotationStatus::Running);
            assert_eq!(committed.pending_external_step, None);

            let restarted = reopen(&root);
            let snapshot = restarted.seats.query().expect("restart snapshot");
            let blocked = snapshot
                .release_rotations
                .get(&request.action_id)
                .expect("blocked rotation");
            assert_eq!(blocked.status, ReleaseRotationStatus::Blocked);
            assert_eq!(blocked.completed_boundary, completed_boundary);
            assert_eq!(blocked.pending_external_step, None);
            assert_eq!(
                blocked.blocked_reason.as_deref(),
                Some(RELEASE_ROTATION_RESTART_BETWEEN_BOUNDARIES)
            );
            assert_eq!(
                snapshot.active_release_rotation.as_ref(),
                Some(&request.action_id)
            );
            assert!(snapshot.occupancies.values().all(|occupancy| {
                occupancy.seat_id.as_str() != "cutex-release"
                    || occupancy.occupant_cutex_session.as_str() != "cutex.release-old"
            }));

            let lifecycle = FakeLifecycle {
                predecessor_thread: Some("thread-old".to_string()),
                ..Default::default()
            };
            let complete = restarted
                .retry_root(
                    &RetryReleaseRotationRequest {
                        schema: ReleaseRotationCommandSchema::V1,
                        action_id: request.action_id.clone(),
                        expected_request_sha256: blocked.request_sha256.clone(),
                        expected_completed_boundary: completed_boundary,
                        expected_pending_external_step: None,
                        corrected_successor_cutex_session: None,
                        corrected_successor_thread_id: None,
                    },
                    &lifecycle,
                )
                .expect("recover committed boundary");
            assert_eq!(complete.status, ReleaseRotationStatus::Complete);
            assert_eq!(lifecycle.delivered.lock().expect("delivery").len(), 1);
            assert!(
                lifecycle
                    .calls
                    .lock()
                    .expect("calls")
                    .iter()
                    .filter(|call| **call == "create_session")
                    .count()
                    <= 1
            );
            assert!(
                lifecycle
                    .calls
                    .lock()
                    .expect("calls")
                    .iter()
                    .filter(|call| **call == "thread_start")
                    .count()
                    <= 1
            );
            let final_snapshot = restarted.seats.query().expect("final snapshot");
            assert!(final_snapshot.occupancies.values().all(|occupancy| {
                occupancy.seat_id.as_str() != "cutex-release"
                    || occupancy.occupant_cutex_session.as_str() != "cutex.release-old"
            }));
            fs::remove_dir_all(root).expect("cleanup");
        }
    }

    #[test]
    fn reopen_recovers_process_loss_after_pending_rearm() {
        let (provider, root, request) = fixture("pending-rearm");
        let lifecycle = FakeLifecycle {
            panic_after: Mutex::new(Some("offline")),
            predecessor_thread: Some("thread-old".to_string()),
            ..Default::default()
        };
        assert!(catch_unwind(AssertUnwindSafe(|| {
            let _ =
                provider.request_director(&session("cutex.director"), &request, false, &lifecycle);
        }))
        .is_err());
        let restarted = reopen(&root);
        let blocked = restarted
            .seats
            .query()
            .expect("blocked pending")
            .release_rotations
            .get(&request.action_id)
            .cloned()
            .expect("rotation");
        let rearmed = restarted
            .seats
            .resume_interrupted_release_rotation(
                &blocked.action_id,
                &blocked.request_sha256,
                blocked.completed_boundary,
                blocked.pending_external_step,
            )
            .expect("rearm pending step");
        assert_eq!(rearmed.status, ReleaseRotationStatus::Running);
        assert_eq!(rearmed.pending_external_step, None);

        let restarted_again = reopen(&root);
        let blocked_again = restarted_again
            .seats
            .query()
            .expect("blocked rearm")
            .release_rotations
            .get(&request.action_id)
            .cloned()
            .expect("rotation");
        assert_eq!(blocked_again.status, ReleaseRotationStatus::Blocked);
        assert_eq!(blocked_again.pending_external_step, None);
        assert_eq!(
            blocked_again.blocked_reason.as_deref(),
            Some(RELEASE_ROTATION_RESTART_BETWEEN_BOUNDARIES)
        );
        *lifecycle.panic_after.lock().expect("panic_after") = None;
        let complete = restarted_again
            .retry_root(
                &RetryReleaseRotationRequest {
                    schema: ReleaseRotationCommandSchema::V1,
                    action_id: request.action_id,
                    expected_request_sha256: blocked_again.request_sha256,
                    expected_completed_boundary: blocked_again.completed_boundary,
                    expected_pending_external_step: None,
                    corrected_successor_cutex_session: None,
                    corrected_successor_thread_id: None,
                },
                &lifecycle,
            )
            .expect("recover after lost rearm frame");
        assert_eq!(complete.status, ReleaseRotationStatus::Complete);
        assert_eq!(
            lifecycle
                .calls
                .lock()
                .expect("calls")
                .iter()
                .filter(|call| **call == "offline")
                .count(),
            2
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn reopen_recovers_process_loss_after_verified_correction_advance() {
        for (crash_after_step, pending_step, corrected_boundary) in [
            (
                "create_session",
                ReleaseRotationExternalStep::CreateSuccessorSession,
                ReleaseRotationBoundary::SuccessorSessionCreated,
            ),
            (
                "thread_start",
                ReleaseRotationExternalStep::StartSuccessorThread,
                ReleaseRotationBoundary::SuccessorThreadStarted,
            ),
        ] {
            let (provider, root, request) = fixture("correction-advance");
            let lifecycle = FakeLifecycle {
                panic_after: Mutex::new(Some(crash_after_step)),
                predecessor_thread: Some("thread-old".to_string()),
                ..Default::default()
            };
            assert!(catch_unwind(AssertUnwindSafe(|| {
                let _ = provider.request_director(
                    &session("cutex.director"),
                    &request,
                    false,
                    &lifecycle,
                );
            }))
            .is_err());
            let restarted = reopen(&root);
            let blocked = restarted
                .seats
                .query()
                .expect("blocked correction")
                .release_rotations
                .get(&request.action_id)
                .cloned()
                .expect("rotation");
            *lifecycle.panic_after.lock().expect("panic_after") = None;
            let corrected = restarted
                .apply_explicit_correction(
                    blocked.clone(),
                    &RetryReleaseRotationRequest {
                        schema: ReleaseRotationCommandSchema::V1,
                        action_id: request.action_id.clone(),
                        expected_request_sha256: blocked.request_sha256.clone(),
                        expected_completed_boundary: blocked.completed_boundary,
                        expected_pending_external_step: Some(pending_step),
                        corrected_successor_cutex_session: (pending_step
                            == ReleaseRotationExternalStep::CreateSuccessorSession)
                            .then(|| session("cutex.release-new")),
                        corrected_successor_thread_id: (pending_step
                            == ReleaseRotationExternalStep::StartSuccessorThread)
                            .then(|| "thread-new".to_string()),
                    },
                    &lifecycle,
                )
                .expect("verified correction");
            assert_eq!(corrected.status, ReleaseRotationStatus::Running);
            assert_eq!(corrected.completed_boundary, corrected_boundary);
            assert_eq!(corrected.pending_external_step, None);

            let restarted_again = reopen(&root);
            let blocked_again = restarted_again
                .seats
                .query()
                .expect("blocked corrected boundary")
                .release_rotations
                .get(&request.action_id)
                .cloned()
                .expect("rotation");
            assert_eq!(blocked_again.status, ReleaseRotationStatus::Blocked);
            assert_eq!(blocked_again.completed_boundary, corrected_boundary);
            assert_eq!(blocked_again.pending_external_step, None);
            let complete = restarted_again
                .retry_root(
                    &RetryReleaseRotationRequest {
                        schema: ReleaseRotationCommandSchema::V1,
                        action_id: request.action_id.clone(),
                        expected_request_sha256: blocked_again.request_sha256,
                        expected_completed_boundary: corrected_boundary,
                        expected_pending_external_step: None,
                        corrected_successor_cutex_session: None,
                        corrected_successor_thread_id: None,
                    },
                    &lifecycle,
                )
                .expect("recover corrected boundary");
            assert_eq!(complete.status, ReleaseRotationStatus::Complete);
            assert_eq!(lifecycle.delivered.lock().expect("delivery").len(), 1);
            assert_eq!(
                lifecycle
                    .calls
                    .lock()
                    .expect("calls")
                    .iter()
                    .filter(|call| **call == "create_session")
                    .count(),
                1
            );
            assert_eq!(
                lifecycle
                    .calls
                    .lock()
                    .expect("calls")
                    .iter()
                    .filter(|call| **call == "thread_start")
                    .count(),
                1
            );
            fs::remove_dir_all(root).expect("cleanup");
        }
    }

    #[test]
    fn reopen_blocks_and_root_recovers_every_pending_external_boundary() {
        for (crash_after_step, pending_step, completed_boundary) in [
            (
                "offline",
                ReleaseRotationExternalStep::OfflinePredecessor,
                ReleaseRotationBoundary::SeatRevoked,
            ),
            (
                "retire",
                ReleaseRotationExternalStep::RetirePredecessor,
                ReleaseRotationBoundary::PredecessorOfflined,
            ),
            (
                "create_session",
                ReleaseRotationExternalStep::CreateSuccessorSession,
                ReleaseRotationBoundary::PredecessorRetired,
            ),
            (
                "thread_start",
                ReleaseRotationExternalStep::StartSuccessorThread,
                ReleaseRotationBoundary::SuccessorSessionCreated,
            ),
            (
                "runtime_online",
                ReleaseRotationExternalStep::LaunchSuccessorRuntime,
                ReleaseRotationBoundary::SuccessorThreadStarted,
            ),
            (
                "deliver",
                ReleaseRotationExternalStep::DeliverDirectorMessage,
                ReleaseRotationBoundary::SuccessorBound,
            ),
        ] {
            let (provider, root, request) = fixture(crash_after_step);
            let lifecycle = FakeLifecycle {
                panic_after: Mutex::new(Some(crash_after_step)),
                predecessor_thread: Some("thread-old".to_string()),
                ..Default::default()
            };
            assert!(catch_unwind(AssertUnwindSafe(|| {
                let _ = provider.request_director(
                    &session("cutex.director"),
                    &request,
                    false,
                    &lifecycle,
                );
            }))
            .is_err());
            let interrupted = provider.seats.query().expect("interrupted seats");
            let running = interrupted
                .release_rotations
                .get(&request.action_id)
                .expect("running rotation");
            assert_eq!(running.status, ReleaseRotationStatus::Running);
            assert_eq!(running.completed_boundary, completed_boundary);
            assert_eq!(running.pending_external_step, Some(pending_step));

            let restarted = reopen(&root);
            let snapshot = restarted.seats.query().expect("restarted seats");
            let blocked = snapshot
                .release_rotations
                .get(&request.action_id)
                .expect("blocked rotation");
            assert_eq!(blocked.status, ReleaseRotationStatus::Blocked);
            assert_eq!(blocked.completed_boundary, completed_boundary);
            assert_eq!(blocked.pending_external_step, Some(pending_step));
            assert_eq!(
                blocked.blocked_reason.as_deref(),
                Some("external_outcome_unknown_after_restart")
            );
            assert_eq!(
                snapshot.active_release_rotation.as_ref(),
                Some(&request.action_id)
            );
            assert!(snapshot.occupancies.values().all(|occupancy| {
                occupancy.seat_id.as_str() != "cutex-release"
                    || occupancy.occupant_cutex_session.as_str() != "cutex.release-old"
            }));
            let reopened_again = reopen(&root);
            assert_eq!(
                reopened_again.seats.query().expect("second reopen"),
                snapshot,
                "reclassifying an already blocked restart record must be no-write"
            );

            let calls_before_replay = lifecycle.calls.lock().expect("calls").len();
            let replay = restarted
                .request_director(&session("cutex.director"), &request, false, &lifecycle)
                .expect("ordinary exact replay is read-only");
            assert_eq!(replay.status, ReleaseRotationStatus::Blocked);
            assert_eq!(replay.pending_external_step, Some(pending_step));
            assert_eq!(
                lifecycle.calls.lock().expect("calls").len(),
                calls_before_replay
            );
            let mut changed = request.clone();
            changed.starting_message.push_str(" changed");
            assert_eq!(
                restarted
                    .request_director(&session("cutex.director"), &changed, false, &lifecycle,),
                Err(ReleaseRotationError::Conflict("action_id_payload_conflict"))
            );

            *lifecycle.panic_after.lock().expect("panic_after") = None;
            let complete = restarted
                .retry_root(
                    &RetryReleaseRotationRequest {
                        schema: ReleaseRotationCommandSchema::V1,
                        action_id: request.action_id.clone(),
                        expected_request_sha256: blocked.request_sha256.clone(),
                        expected_completed_boundary: completed_boundary,
                        expected_pending_external_step: Some(pending_step),
                        corrected_successor_cutex_session: (pending_step
                            == ReleaseRotationExternalStep::CreateSuccessorSession)
                            .then(|| session("cutex.release-new")),
                        corrected_successor_thread_id: (pending_step
                            == ReleaseRotationExternalStep::StartSuccessorThread)
                            .then(|| "thread-new".to_string()),
                    },
                    &lifecycle,
                )
                .expect("recover exact pending edge");
            assert_eq!(complete.status, ReleaseRotationStatus::Complete);
            assert_eq!(complete.pending_external_step, None);
            assert_eq!(complete.successor_seat_epoch, Some(2));
            assert_eq!(lifecycle.delivered.lock().expect("delivery").len(), 1);
            assert_eq!(
                lifecycle
                    .calls
                    .lock()
                    .expect("calls")
                    .iter()
                    .filter(|call| **call == "create_session")
                    .count(),
                1,
                "restart recovery must never create a second successor"
            );
            assert_eq!(
                lifecycle
                    .calls
                    .lock()
                    .expect("calls")
                    .iter()
                    .filter(|call| **call == "thread_start")
                    .count(),
                1,
                "restart recovery must never start a second thread"
            );
            fs::remove_dir_all(root).expect("cleanup");
        }
    }

    #[test]
    fn restart_correction_mismatch_is_no_write_for_ambiguous_edges() {
        for (crash_after_step, pending_step) in [
            (
                "create_session",
                ReleaseRotationExternalStep::CreateSuccessorSession,
            ),
            (
                "thread_start",
                ReleaseRotationExternalStep::StartSuccessorThread,
            ),
        ] {
            let (provider, root, request) = fixture(crash_after_step);
            let lifecycle = FakeLifecycle {
                panic_after: Mutex::new(Some(crash_after_step)),
                predecessor_thread: Some("thread-old".to_string()),
                ..Default::default()
            };
            assert!(catch_unwind(AssertUnwindSafe(|| {
                let _ = provider.request_director(
                    &session("cutex.director"),
                    &request,
                    false,
                    &lifecycle,
                );
            }))
            .is_err());
            let restarted = reopen(&root);
            *lifecycle.panic_after.lock().expect("panic_after") = None;
            let before = restarted.seats.query().expect("before mismatch");
            let record = before
                .release_rotations
                .get(&request.action_id)
                .expect("record");
            let mismatch = RetryReleaseRotationRequest {
                schema: ReleaseRotationCommandSchema::V1,
                action_id: request.action_id.clone(),
                expected_request_sha256: record.request_sha256.clone(),
                expected_completed_boundary: record.completed_boundary,
                expected_pending_external_step: Some(pending_step),
                corrected_successor_cutex_session: (pending_step
                    == ReleaseRotationExternalStep::CreateSuccessorSession)
                    .then(|| session("cutex.release-wrong")),
                corrected_successor_thread_id: (pending_step
                    == ReleaseRotationExternalStep::StartSuccessorThread)
                    .then(|| "thread-old".to_string()),
            };
            assert!(matches!(
                restarted.retry_root(&mismatch, &lifecycle),
                Err(ReleaseRotationError::Conflict(_))
            ));
            assert_eq!(
                restarted.seats.query().expect("after mismatch"),
                before,
                "correction mismatch must be no-write"
            );
            let mut wrong_boundary = mismatch.clone();
            wrong_boundary.expected_completed_boundary = ReleaseRotationBoundary::SeatRevoked;
            assert_eq!(
                restarted.retry_root(&wrong_boundary, &lifecycle),
                Err(ReleaseRotationError::Conflict("completed_boundary_changed"))
            );
            assert_eq!(
                restarted.seats.query().expect("after boundary mismatch"),
                before,
                "completed boundary mismatch must be no-write"
            );
            let mut wrong_pending = mismatch;
            wrong_pending.expected_pending_external_step = None;
            assert_eq!(
                restarted.retry_root(&wrong_pending, &lifecycle),
                Err(ReleaseRotationError::Conflict(
                    "pending_external_identity_changed"
                ))
            );
            fs::remove_dir_all(root).expect("cleanup");
        }
    }

    #[test]
    fn repeatable_restart_step_failure_blocks_at_same_recoverable_boundary() {
        let (provider, root, request) = fixture("restart-repeat-failure");
        let lifecycle = FakeLifecycle {
            panic_after: Mutex::new(Some("offline")),
            predecessor_thread: Some("thread-old".to_string()),
            ..Default::default()
        };
        assert!(catch_unwind(AssertUnwindSafe(|| {
            let _ =
                provider.request_director(&session("cutex.director"), &request, false, &lifecycle);
        }))
        .is_err());
        let restarted = reopen(&root);
        let record = restarted
            .seats
            .query()
            .expect("blocked restart")
            .release_rotations
            .get(&request.action_id)
            .cloned()
            .expect("rotation");
        *lifecycle.panic_after.lock().expect("panic_after") = None;
        *lifecycle.fail.lock().expect("fail") = Some("offline");
        let error = restarted
            .retry_root(
                &RetryReleaseRotationRequest {
                    schema: ReleaseRotationCommandSchema::V1,
                    action_id: request.action_id.clone(),
                    expected_request_sha256: record.request_sha256.clone(),
                    expected_completed_boundary: record.completed_boundary,
                    expected_pending_external_step: Some(
                        ReleaseRotationExternalStep::OfflinePredecessor,
                    ),
                    corrected_successor_cutex_session: None,
                    corrected_successor_thread_id: None,
                },
                &lifecycle,
            )
            .expect_err("repeated external step fails");
        let ReleaseRotationError::Blocked(blocked) = error else {
            panic!("expected blocked receipt");
        };
        assert_eq!(
            blocked.completed_boundary,
            ReleaseRotationBoundary::SeatRevoked
        );
        assert_eq!(blocked.pending_external_step, None);
        assert_eq!(
            blocked.blocked_reason.as_deref(),
            Some("external_step_failed")
        );
        *lifecycle.fail.lock().expect("fail") = None;
        let complete = restarted
            .retry_root(
                &RetryReleaseRotationRequest {
                    schema: ReleaseRotationCommandSchema::V1,
                    action_id: request.action_id,
                    expected_request_sha256: record.request_sha256,
                    expected_completed_boundary: blocked.completed_boundary,
                    expected_pending_external_step: None,
                    corrected_successor_cutex_session: None,
                    corrected_successor_thread_id: None,
                },
                &lifecycle,
            )
            .expect("ordinary blocked retry remains recoverable");
        assert_eq!(complete.status, ReleaseRotationStatus::Complete);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn blocked_rotation_requires_explicit_retry_and_fences_concurrent_action() {
        let (provider, root, request) = fixture("retry");
        let lifecycle = FakeLifecycle {
            fail: Mutex::new(Some("deliver")),
            predecessor_thread: Some("thread-old".to_string()),
            ..Default::default()
        };
        let blocked = provider
            .request_director(&session("cutex.director"), &request, false, &lifecycle)
            .expect_err("blocked");
        let ReleaseRotationError::Blocked(receipt) = blocked else {
            panic!("expected blocked receipt");
        };
        assert_eq!(
            receipt.completed_boundary,
            ReleaseRotationBoundary::SuccessorBound
        );
        let mut concurrent = request.clone();
        concurrent.action_id = ActionId::new("concurrent-rotation").expect("action");
        assert_eq!(
            provider.request_director(&session("cutex.director"), &concurrent, false, &lifecycle,),
            Err(ReleaseRotationError::Conflict(
                "release_rotation_in_progress"
            ))
        );
        *lifecycle.fail.lock().expect("fail") = None;
        let retried = provider
            .retry_root(
                &RetryReleaseRotationRequest {
                    schema: ReleaseRotationCommandSchema::V1,
                    action_id: request.action_id.clone(),
                    expected_request_sha256: receipt.request_sha256,
                    expected_completed_boundary: receipt.completed_boundary,
                    expected_pending_external_step: None,
                    corrected_successor_cutex_session: None,
                    corrected_successor_thread_id: None,
                },
                &lifecycle,
            )
            .expect("explicit retry");
        assert_eq!(retried.status, ReleaseRotationStatus::Complete);
        assert_eq!(retried.successor_seat_epoch, Some(2));
        assert_eq!(lifecycle.delivered.lock().expect("delivery").len(), 1);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn clean_synchronous_thread_start_failure_retries_same_successor_without_correction() {
        let (provider, root, request) = fixture("clean-thread-start-retry");
        let lifecycle = FakeLifecycle {
            fail: Mutex::new(Some("thread_start")),
            predecessor_thread: Some("thread-old".to_string()),
            ..Default::default()
        };
        let error = provider
            .request_director(&session("cutex.director"), &request, false, &lifecycle)
            .expect_err("synchronous thread start failure blocks");
        let ReleaseRotationError::Blocked(blocked) = error else {
            panic!("expected blocked receipt");
        };
        assert_eq!(
            blocked.completed_boundary,
            ReleaseRotationBoundary::SuccessorSessionCreated
        );
        assert_eq!(blocked.pending_external_step, None);
        assert_eq!(
            blocked.blocked_reason.as_deref(),
            Some(RELEASE_ROTATION_EXTERNAL_STEP_FAILED)
        );
        assert_eq!(
            blocked.successor_cutex_session.as_ref(),
            Some(&session("cutex.release-new"))
        );

        *lifecycle.fail.lock().expect("fail") = None;
        let complete = provider
            .retry_root(
                &RetryReleaseRotationRequest {
                    schema: ReleaseRotationCommandSchema::V1,
                    action_id: request.action_id.clone(),
                    expected_request_sha256: blocked.request_sha256,
                    expected_completed_boundary: blocked.completed_boundary,
                    expected_pending_external_step: None,
                    corrected_successor_cutex_session: None,
                    corrected_successor_thread_id: None,
                },
                &lifecycle,
            )
            .expect("retry clean successor");
        assert_eq!(complete.status, ReleaseRotationStatus::Complete);
        assert_eq!(
            lifecycle
                .calls
                .lock()
                .expect("calls")
                .iter()
                .filter(|call| **call == "create_session")
                .count(),
            1,
            "retry must not create another durable successor"
        );
        assert_eq!(
            lifecycle
                .calls
                .lock()
                .expect("calls")
                .iter()
                .filter(|call| **call == "verify_successor_session")
                .count(),
            1
        );
        assert_eq!(
            lifecycle
                .verified_successor_sessions
                .lock()
                .expect("verified successors")
                .as_slice(),
            [session("cutex.release-new")]
        );
        assert_eq!(
            lifecycle
                .calls
                .lock()
                .expect("calls")
                .iter()
                .filter(|call| **call == "thread_start")
                .count(),
            2,
            "one failed attempt and one supported retry"
        );
        assert_eq!(
            lifecycle
                .thread_start_successors
                .lock()
                .expect("thread start successors")
                .as_slice(),
            [session("cutex.release-new"), session("cutex.release-new")]
        );
        let seats = provider.seats.query().expect("seats");
        let release = seats
            .occupancies
            .values()
            .find(|occupancy| occupancy.seat_id.as_str() == "cutex-release")
            .expect("successor bound");
        assert_eq!(release.occupant_cutex_session.as_str(), "cutex.release-new");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn outcome_unknown_session_creation_requires_exact_correction_not_second_candidate() {
        let (provider, root, request) = fixture("session-correction");
        let lifecycle = FakeLifecycle {
            fail: Mutex::new(Some("create_session")),
            predecessor_thread: Some("thread-old".to_string()),
            ..Default::default()
        };
        let error = provider
            .request_director(&session("cutex.director"), &request, false, &lifecycle)
            .expect_err("blocked");
        let ReleaseRotationError::Blocked(receipt) = error else {
            panic!("expected blocked receipt");
        };
        assert_eq!(
            receipt.completed_boundary,
            ReleaseRotationBoundary::PredecessorRetired
        );
        *lifecycle.fail.lock().expect("fail") = None;
        assert_eq!(
            provider.retry_root(
                &RetryReleaseRotationRequest {
                    schema: ReleaseRotationCommandSchema::V1,
                    action_id: request.action_id.clone(),
                    expected_request_sha256: receipt.request_sha256.clone(),
                    expected_completed_boundary: receipt.completed_boundary,
                    expected_pending_external_step: None,
                    corrected_successor_cutex_session: None,
                    corrected_successor_thread_id: None,
                },
                &lifecycle,
            ),
            Err(ReleaseRotationError::Conflict(
                "explicit_successor_session_correction_required"
            ))
        );
        let complete = provider
            .retry_root(
                &RetryReleaseRotationRequest {
                    schema: ReleaseRotationCommandSchema::V1,
                    action_id: request.action_id,
                    expected_request_sha256: receipt.request_sha256,
                    expected_completed_boundary: receipt.completed_boundary,
                    expected_pending_external_step: None,
                    corrected_successor_cutex_session: Some(session("cutex.release-new")),
                    corrected_successor_thread_id: None,
                },
                &lifecycle,
            )
            .expect("correct exact successor");
        assert_eq!(complete.status, ReleaseRotationStatus::Complete);
        assert_eq!(
            lifecycle
                .calls
                .lock()
                .expect("calls")
                .iter()
                .filter(|call| **call == "create_session")
                .count(),
            1,
            "correction must not create a second durable candidate"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn all_preflight_conflicts_are_no_write() {
        for (label, active_turn, assignment) in
            [("active-turn", true, false), ("assignment", false, true)]
        {
            let (provider, root, request) = fixture(label);
            let lifecycle = FakeLifecycle {
                active_turn,
                ..Default::default()
            };
            assert!(provider
                .request_director(&session("cutex.director"), &request, assignment, &lifecycle,)
                .is_err());
            assert!(provider.query(None).expect("query").is_empty());
            fs::remove_dir_all(root).expect("cleanup");
        }
    }

    #[test]
    fn invalid_successor_preflight_is_no_write_before_seat_or_lifecycle_effects() {
        let (provider, root, request) = fixture("invalid-successor-preflight");
        let lifecycle = FakeLifecycle {
            fail: Mutex::new(Some("preflight")),
            ..Default::default()
        };
        assert_eq!(
            provider.request_director(&session("cutex.director"), &request, false, &lifecycle),
            Err(ReleaseRotationError::Conflict("successor_preflight_failed"))
        );
        assert!(provider.query(None).expect("query").is_empty());
        let seats = provider.seats.query().expect("seats");
        let release = seats
            .occupancies
            .values()
            .find(|occupancy| occupancy.seat_id.as_str() == "cutex-release")
            .expect("predecessor remains bound");
        assert_eq!(release.occupant_cutex_session.as_str(), "cutex.release-old");
        assert_eq!(
            lifecycle.calls.lock().expect("calls").as_slice(),
            ["preflight"]
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn external_failures_are_blocked_at_exact_boundary_and_never_restore_predecessor() {
        for (failure, boundary) in [
            ("offline", ReleaseRotationBoundary::SeatRevoked),
            ("retire", ReleaseRotationBoundary::PredecessorOfflined),
            (
                "create_session",
                ReleaseRotationBoundary::PredecessorRetired,
            ),
            (
                "predecessor_thread",
                ReleaseRotationBoundary::SuccessorSessionCreated,
            ),
            (
                "thread_start",
                ReleaseRotationBoundary::SuccessorSessionCreated,
            ),
            (
                "runtime_online",
                ReleaseRotationBoundary::SuccessorThreadStarted,
            ),
            ("deliver", ReleaseRotationBoundary::SuccessorBound),
        ] {
            let (provider, root, request) = fixture(failure);
            let lifecycle = FakeLifecycle {
                fail: Mutex::new(Some(failure)),
                predecessor_thread: Some("thread-old".to_string()),
                ..Default::default()
            };
            let error = provider
                .request_director(&session("cutex.director"), &request, false, &lifecycle)
                .expect_err("blocked");
            let ReleaseRotationError::Blocked(receipt) = error else {
                panic!("expected blocked receipt");
            };
            assert_eq!(receipt.completed_boundary, boundary);
            assert_eq!(receipt.status, ReleaseRotationStatus::Blocked);
            let query = provider
                .query(Some(&request.action_id))
                .expect("query failed rotation");
            assert_eq!(query.len(), 1);
            assert_eq!(query[0].status, ReleaseRotationStatus::Blocked);
            assert!(provider
                .query(None)
                .expect("query all")
                .iter()
                .all(|receipt| receipt.status != ReleaseRotationStatus::Running));
            let seat = provider.seats.query().expect("seats");
            let release = seat
                .occupancies
                .values()
                .find(|occupancy| occupancy.seat_id.as_str() == "cutex-release");
            if boundary == ReleaseRotationBoundary::SuccessorBound {
                assert_eq!(
                    release.map(|occupancy| occupancy.occupant_cutex_session.as_str()),
                    Some("cutex.release-new")
                );
            } else {
                assert!(release.is_none());
            }
            assert!(seat.occupancies.values().all(|occupancy| {
                occupancy.occupant_cutex_session.as_str() != "cutex.release-old"
                    || occupancy.seat_id.as_str() != "cutex-release"
            }));
            fs::remove_dir_all(root).expect("cleanup");
        }
    }

    #[test]
    fn stale_authority_occupancy_template_and_changed_replay_do_not_write() {
        let (provider, root, request) = fixture("stale");
        let lifecycle = FakeLifecycle::default();
        let mut wrong_target = request.clone();
        wrong_target.target_seat = SeatId::new("cutex-director").expect("seat");
        assert_eq!(
            provider
                .request_director(&session("cutex.director"), &wrong_target, false, &lifecycle,),
            Err(ReleaseRotationError::InvalidRequest(
                "unsupported_rotation_target"
            ))
        );
        assert_eq!(
            provider.request_director(
                &session("cutex.stale-director"),
                &request,
                false,
                &lifecycle
            ),
            Err(ReleaseRotationError::Unauthorized)
        );
        let mut stale_epoch = request.clone();
        stale_epoch.expected_seat_epoch = 2;
        assert_eq!(
            provider.request_director(&session("cutex.director"), &stale_epoch, false, &lifecycle),
            Err(ReleaseRotationError::Conflict("stale_release_occupancy"))
        );
        let mut stale_template = request.clone();
        stale_template.expected_template_sha256 = Sha256::new("2".repeat(64)).expect("hash");
        assert_eq!(
            provider.request_director(
                &session("cutex.director"),
                &stale_template,
                false,
                &lifecycle
            ),
            Err(ReleaseRotationError::Conflict("stale_release_template"))
        );
        assert!(provider.query(None).expect("query").is_empty());
        fs::remove_dir_all(root).expect("cleanup");
    }
}
