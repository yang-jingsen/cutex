//! Private durable transaction provider for the Role-Seat v1 store.

use std::fmt;
use std::fs::File;
use std::io;
use std::path::PathBuf;

use fs2::FileExt;

use super::{
    validate_request, validate_store, IdempotencyRecord, InitializationId, MutationRequest,
    MutationResponse, MutationResult, ProjectId, RequestEnvelope, RequestId, ResultDisposition,
    ResultSchema, RoleFamily, RoleFamilyId, RoleSeatStore, Sha256, StoreRevision, ValidationCode,
};

mod digest;
mod persist;

#[cfg(test)]
mod tests;

const STORE_FILE: &str = "role-seat-core-v1.json";
const LOCK_FILE: &str = "role-seat-core-v1.lock";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IoStage {
    InspectRoot,
    OpenLock,
    Lock,
    OpenStore,
    ReadStore,
    CreateTemp,
    WriteTemp,
    SyncTemp,
    Replace,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RepositoryError {
    UnsupportedPlatform,
    RootNotDirectory,
    RootOwnerMismatch,
    RootModeMismatch,
    PrivateFileNotRegular,
    PrivateFileOwnerMismatch,
    PrivateFileModeMismatch,
    Io {
        stage: IoStage,
        kind: io::ErrorKind,
    },
    InvalidJson,
    InvalidStore {
        code: ValidationCode,
    },
    RequestDigestMismatch,
    RequestConflict,
    StoreRevisionConflict {
        expected: StoreRevision,
        actual: StoreRevision,
    },
    StoreRevisionOverflow,
    PlanRejected,
    PlannedOperationMismatch,
    InvalidPlannedRequest {
        code: ValidationCode,
    },
    InvalidNextStore {
        code: ValidationCode,
    },
    Serialization,
    InjectedPreReplaceFailure,
}

impl fmt::Display for RepositoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => formatter
                .write_str("private role-seat repository requires Unix ownership and mode checks"),
            Self::RootNotDirectory => {
                formatter.write_str("role-seat repository root is not a directory")
            }
            Self::RootOwnerMismatch => formatter
                .write_str("role-seat repository root is not owned by the current Unix user"),
            Self::RootModeMismatch => {
                formatter.write_str("role-seat repository root mode is not exactly 0700")
            }
            Self::PrivateFileNotRegular => {
                formatter.write_str("role-seat repository child is not a regular file")
            }
            Self::PrivateFileOwnerMismatch => formatter
                .write_str("role-seat repository child is not owned by the current Unix user"),
            Self::PrivateFileModeMismatch => {
                formatter.write_str("role-seat repository child mode is not exactly 0600")
            }
            Self::Io { stage, kind } => write!(
                formatter,
                "role-seat repository I/O failure at {stage:?}: {kind:?}"
            ),
            Self::InvalidJson => formatter.write_str("role-seat store is not valid v1 JSON"),
            Self::InvalidStore { code } => {
                write!(formatter, "role-seat store validation failed: {code:?}")
            }
            Self::RequestDigestMismatch => formatter
                .write_str("claimed request digest does not match canonical request material"),
            Self::RequestConflict => formatter
                .write_str("request ID was already committed with different canonical material"),
            Self::StoreRevisionConflict { expected, actual } => write!(
                formatter,
                "role-seat store revision conflict: expected {}, actual {}",
                expected.get(),
                actual.get()
            ),
            Self::StoreRevisionOverflow => {
                formatter.write_str("role-seat store revision cannot be incremented")
            }
            Self::PlanRejected => {
                formatter.write_str("role-seat mutation plan rejected the request")
            }
            Self::PlannedOperationMismatch => {
                formatter.write_str("planned result does not match the request operation")
            }
            Self::InvalidPlannedRequest { code } => write!(
                formatter,
                "planned request/result validation failed: {code:?}"
            ),
            Self::InvalidNextStore { code } => {
                write!(formatter, "planned next store validation failed: {code:?}")
            }
            Self::Serialization => {
                formatter.write_str("role-seat next store could not be serialized")
            }
            Self::InjectedPreReplaceFailure => {
                formatter.write_str("role-seat persistence failed before atomic replacement")
            }
        }
    }
}

impl std::error::Error for RepositoryError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PersistencePhase {
    AfterReplace,
    ParentDirectorySync,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MutationOutcome {
    Committed(MutationResponse),
    NoWrite(RepositoryError),
    PersistenceUnknown {
        request_id: RequestId,
        phase: PersistencePhase,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequestLookup {
    NotFound,
    Committed(MutationResponse),
    RequestConflict,
    Unavailable(RepositoryError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedMutation {
    pub family: Option<RoleFamily>,
    pub result: MutationResult,
}

pub struct RoleSeatRepository {
    root: PathBuf,
    faults: persist::FaultController,
}

impl RoleSeatRepository {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, RepositoryError> {
        let repository = Self {
            root: root.into(),
            faults: persist::FaultController::default(),
        };
        persist::validate_root(&repository.root)?;
        Ok(repository)
    }

    pub fn load(&self) -> Result<RoleSeatStore, RepositoryError> {
        persist::validate_root(&self.root)?;
        persist::load_store(&self.store_path())
    }

    pub fn mutate<F>(&self, request: &RequestEnvelope, plan: F) -> MutationOutcome
    where
        F: FnOnce(&RoleSeatStore) -> Result<PlannedMutation, RepositoryError>,
    {
        let digest = match canonical_request_digest(request) {
            Ok(digest) if digest == request.request_digest_sha256 => digest,
            Ok(_) => return MutationOutcome::NoWrite(RepositoryError::RequestDigestMismatch),
            Err(error) => return MutationOutcome::NoWrite(error),
        };
        if let Err(error) = persist::validate_root(&self.root) {
            return MutationOutcome::NoWrite(error);
        }
        let lock = match persist::open_lock(&self.lock_path(), true) {
            Ok(Some(lock)) => lock,
            Ok(None) => unreachable!("create=true always returns a lock"),
            Err(error) => return MutationOutcome::NoWrite(error),
        };
        if let Err(error) = FileExt::lock_exclusive(&lock) {
            return MutationOutcome::NoWrite(RepositoryError::Io {
                stage: IoStage::Lock,
                kind: error.kind(),
            });
        }

        let current = match persist::load_store(&self.store_path()) {
            Ok(store) => store,
            Err(error) => return MutationOutcome::NoWrite(error),
        };
        if let Some(record) = current.idempotency.get(&request.request_id) {
            if record.request_digest_sha256 != digest
                || record.operation != request.request.operation()
            {
                return MutationOutcome::NoWrite(RepositoryError::RequestConflict);
            }
            return MutationOutcome::Committed(response_from_record(&request.request_id, record));
        }
        if current.store_revision != request.expected_store_revision {
            return MutationOutcome::NoWrite(RepositoryError::StoreRevisionConflict {
                expected: request.expected_store_revision,
                actual: current.store_revision,
            });
        }
        let next_revision = match current.store_revision.checked_next() {
            Ok(revision) => revision,
            Err(_) => return MutationOutcome::NoWrite(RepositoryError::StoreRevisionOverflow),
        };
        let planned = match plan(&current) {
            Ok(planned) => planned,
            Err(error) => return MutationOutcome::NoWrite(error),
        };
        if planned.result.operation() != request.request.operation() {
            return MutationOutcome::NoWrite(RepositoryError::PlannedOperationMismatch);
        }
        let (project_id, role_family_id, initialization_id) = request_scope(&request.request);
        let response = MutationResponse {
            schema: ResultSchema::V1,
            request_id: request.request_id.clone(),
            operation: request.request.operation(),
            project_id: project_id.clone(),
            role_family_id: role_family_id.clone(),
            initialization_id: initialization_id.clone(),
            disposition: ResultDisposition::Applied,
            committed_store_revision: next_revision,
            result: planned.result.clone(),
        };
        if let Err(error) = validate_request(request, &response) {
            return MutationOutcome::NoWrite(RepositoryError::InvalidPlannedRequest {
                code: error.code,
            });
        }

        let mut next = current.clone();
        next.store_revision = next_revision;
        next.family = planned.family;
        next.idempotency.insert(
            request.request_id.clone(),
            IdempotencyRecord {
                operation: response.operation,
                project_id: response.project_id.clone(),
                role_family_id: response.role_family_id.clone(),
                initialization_id: response.initialization_id.clone(),
                request_digest_sha256: digest,
                committed_store_revision: next_revision,
                result: response.result.clone(),
            },
        );
        if let Err(error) = validate_store(&next) {
            return MutationOutcome::NoWrite(RepositoryError::InvalidNextStore {
                code: error.code,
            });
        }
        let bytes = match serde_json::to_vec_pretty(&next) {
            Ok(bytes) => bytes,
            Err(_) => return MutationOutcome::NoWrite(RepositoryError::Serialization),
        };
        match persist::replace_store(&self.root, &self.store_path(), &bytes, &self.faults) {
            Ok(()) => MutationOutcome::Committed(response),
            Err(persist::PersistFailure::Definite(error)) => MutationOutcome::NoWrite(error),
            Err(persist::PersistFailure::Unknown(phase)) => MutationOutcome::PersistenceUnknown {
                request_id: request.request_id.clone(),
                phase,
            },
        }
    }

    pub fn get_request_result(&self, request: &RequestEnvelope) -> RequestLookup {
        let digest = match canonical_request_digest(request) {
            Ok(digest) if digest == request.request_digest_sha256 => digest,
            Ok(_) => return RequestLookup::RequestConflict,
            Err(error) => return RequestLookup::Unavailable(error),
        };
        if let Err(error) = persist::validate_root(&self.root) {
            return RequestLookup::Unavailable(error);
        }
        let lock = match persist::open_lock(&self.lock_path(), false) {
            Ok(Some(lock)) => lock,
            Ok(None) => {
                return RequestLookup::Unavailable(RepositoryError::Io {
                    stage: IoStage::OpenLock,
                    kind: io::ErrorKind::NotFound,
                })
            }
            Err(error) => return RequestLookup::Unavailable(error),
        };
        if let Err(error) = FileExt::lock_exclusive(&lock) {
            return RequestLookup::Unavailable(RepositoryError::Io {
                stage: IoStage::Lock,
                kind: error.kind(),
            });
        }
        let store = match persist::load_store(&self.store_path()) {
            Ok(store) => store,
            Err(error) => return RequestLookup::Unavailable(error),
        };
        match store.idempotency.get(&request.request_id) {
            None => RequestLookup::NotFound,
            Some(record)
                if record.request_digest_sha256 != digest
                    || record.operation != request.request.operation() =>
            {
                RequestLookup::RequestConflict
            }
            Some(record) => {
                RequestLookup::Committed(response_from_record(&request.request_id, record))
            }
        }
    }

    fn store_path(&self) -> PathBuf {
        self.root.join(STORE_FILE)
    }

    fn lock_path(&self) -> PathBuf {
        self.root.join(LOCK_FILE)
    }

    #[cfg(test)]
    fn with_test_fault(
        root: impl Into<PathBuf>,
        point: persist::FaultPoint,
    ) -> Result<Self, RepositoryError> {
        let repository = Self {
            root: root.into(),
            faults: persist::FaultController::new(point),
        };
        persist::validate_root(&repository.root)?;
        Ok(repository)
    }
}

pub fn canonical_request_digest(request: &RequestEnvelope) -> Result<Sha256, RepositoryError> {
    digest::canonical_request_digest(request)
}

fn response_from_record(request_id: &RequestId, record: &IdempotencyRecord) -> MutationResponse {
    MutationResponse {
        schema: ResultSchema::V1,
        request_id: request_id.clone(),
        operation: record.operation,
        project_id: record.project_id.clone(),
        role_family_id: record.role_family_id.clone(),
        initialization_id: record.initialization_id.clone(),
        disposition: ResultDisposition::Applied,
        committed_store_revision: record.committed_store_revision,
        result: record.result.clone(),
    }
}

fn request_scope(request: &MutationRequest) -> (&ProjectId, &RoleFamilyId, &InitializationId) {
    match request {
        MutationRequest::InitializeFamily(input) => (
            &input.project_id,
            &input.role_family_id,
            &input.initialization_id,
        ),
        MutationRequest::PrepareRotation(input) => (
            &input.project_id,
            &input.role_family_id,
            &input.initialization_id,
        ),
        MutationRequest::RecordCandidate(input) => (
            &input.context.project_id,
            &input.context.role_family_id,
            &input.context.initialization_id,
        ),
        MutationRequest::RecordAdoption(input) => (
            &input.context.project_id,
            &input.context.role_family_id,
            &input.context.initialization_id,
        ),
        MutationRequest::RecordInitialDelivery(input) => (
            &input.context.project_id,
            &input.context.role_family_id,
            &input.context.initialization_id,
        ),
        MutationRequest::RecordAcknowledgement(input) => (
            &input.context.project_id,
            &input.context.role_family_id,
            &input.context.initialization_id,
        ),
        MutationRequest::TransferAuthority(input) => (
            &input.context.project_id,
            &input.context.role_family_id,
            &input.context.initialization_id,
        ),
        MutationRequest::CompleteRotation(input) => (
            &input.context.project_id,
            &input.context.role_family_id,
            &input.context.initialization_id,
        ),
        MutationRequest::FailRotation(input) | MutationRequest::CancelRotation(input) => (
            &input.context.project_id,
            &input.context.role_family_id,
            &input.context.initialization_id,
        ),
        MutationRequest::RecordUnknown(input) => (
            &input.context.project_id,
            &input.context.role_family_id,
            &input.context.initialization_id,
        ),
        MutationRequest::ResolveUnknown(input) => (
            &input.context.project_id,
            &input.context.role_family_id,
            &input.context.initialization_id,
        ),
    }
}
