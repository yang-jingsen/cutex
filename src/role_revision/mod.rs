//! Closed Role-Seat v1 data model, validation, and durable operation facade.

use chrono::{DateTime, SecondsFormat};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub mod operations;
pub mod repository;

pub const MAX_JSON_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ValueError> {
                let value = value.into();
                if value.is_empty()
                    || value.len() > 256
                    || !value.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric()
                            || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'@')
                    })
                {
                    return Err(ValueError::InvalidId);
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

macro_rules! positive_number {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
        pub struct $name(u64);

        impl $name {
            pub fn new(value: u64) -> Result<Self, ValueError> {
                if value == 0 || value > MAX_JSON_SAFE_INTEGER {
                    return Err(ValueError::InvalidPositiveNumber);
                }
                Ok(Self(value))
            }

            pub fn get(self) -> u64 {
                self.0
            }

            pub fn checked_next(self) -> Result<Self, ValueError> {
                self.0
                    .checked_add(1)
                    .filter(|value| *value <= MAX_JSON_SAFE_INTEGER)
                    .map(Self)
                    .ok_or(ValueError::InvalidPositiveNumber)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_u64(self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = u64::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

string_id!(ProjectId);
string_id!(RoleFamilyId);
string_id!(RoleKey);
string_id!(InitializationId);
string_id!(TransitionId);
string_id!(RequestId);
string_id!(HumanApprovalId);
string_id!(TaskId);
string_id!(CutexSessionId);
string_id!(CuteCodexSessionId);
string_id!(RuntimeAgentId);
string_id!(DeliveryId);
string_id!(ReceiptId);

positive_number!(StoreRevision);
positive_number!(RoleRevisionNumber);
positive_number!(AuthorityEpoch);
positive_number!(RuntimeGeneration);
positive_number!(TaskRevision);
positive_number!(DurableRevision);
positive_number!(AttemptNumber);
positive_number!(NumericResult);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueError {
    InvalidId,
    InvalidPositiveNumber,
    InvalidSha256,
    InvalidRfc3339,
}

impl fmt::Display for ValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidId => "invalid typed id",
            Self::InvalidPositiveNumber => "number must be a positive JSON-safe integer",
            Self::InvalidSha256 => "sha256 must contain 64 lowercase hexadecimal characters",
            Self::InvalidRfc3339 => "timestamp must be normalized RFC3339 UTC",
        })
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct Sha256(String);

impl Sha256 {
    pub fn new(value: impl Into<String>) -> Result<Self, ValueError> {
        let value = value.into();
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(ValueError::InvalidSha256);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for Sha256 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Sha256 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct Rfc3339(String);

impl Rfc3339 {
    pub fn new(value: impl Into<String>) -> Result<Self, ValueError> {
        let value = value.into();
        let parsed =
            DateTime::parse_from_rfc3339(&value).map_err(|_| ValueError::InvalidRfc3339)?;
        let normalized = parsed
            .with_timezone(&chrono::Utc)
            .to_rfc3339_opts(SecondsFormat::AutoSi, true);
        if value != normalized {
            return Err(ValueError::InvalidRfc3339);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for Rfc3339 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Rfc3339 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum StoreSchema {
    #[serde(rename = "cutex/role-seat-core/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RequestSchema {
    #[serde(rename = "cutex/role-seat-request/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ResultSchema {
    #[serde(rename = "cutex/role-seat-result/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownTimeReason {
    NotObserved,
    ReceiptOmitsTime,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum EvidenceTime {
    Known { rfc3339: Rfc3339 },
    Unknown { reason: UnknownTimeReason },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    HumanApproval,
    RootInitialization,
    HandoffAcceptance,
    CandidateCreation,
    Adoption,
    InitialDelivery,
    Acknowledgement,
    TransferVerification,
    Completion,
    Failure,
    Cancellation,
    UnknownObservation,
    UnknownResolution,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum IdentityRef {
    HumanApproval {
        id: HumanApprovalId,
    },
    Project {
        id: ProjectId,
    },
    RoleFamily {
        id: RoleFamilyId,
    },
    Task {
        id: TaskId,
    },
    CutexSession {
        id: CutexSessionId,
    },
    CuteCodexSession {
        id: CuteCodexSessionId,
    },
    RuntimeAgent {
        id: RuntimeAgentId,
        generation: RuntimeGeneration,
    },
    Delivery {
        id: DeliveryId,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRef {
    pub kind: EvidenceKind,
    pub receipt_id: ReceiptId,
    pub receipt_sha256: Sha256,
    pub subjects: Vec<IdentityRef>,
    pub occurred_at: EvidenceTime,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DurableSessionRef {
    pub cutex_session_id: CutexSessionId,
    pub durable_revision: DurableRevision,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeIdentity {
    pub cutex_session_id: CutexSessionId,
    pub cute_codex_session_id: CuteCodexSessionId,
    pub runtime_agent_id: RuntimeAgentId,
    pub runtime_generation: RuntimeGeneration,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthoritySnapshot {
    pub role_revision: RoleRevisionNumber,
    pub cutex_session_id: CutexSessionId,
    pub authority_epoch: AuthorityEpoch,
    pub source_durable_revision: DurableRevision,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EstablishedBy {
    RootInitialization { initialization_id: InitializationId },
    Transfer { transition_id: TransitionId },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CurrentAuthority {
    pub role_revision: RoleRevisionNumber,
    pub cutex_session_id: CutexSessionId,
    pub authority_epoch: AuthorityEpoch,
    pub effective_at: EvidenceTime,
    pub established_by: EstablishedBy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RotationLock {
    pub transition_id: TransitionId,
    pub candidate_revision: RoleRevisionNumber,
    pub source_authority_epoch: AuthorityEpoch,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RootInitialization {
    pub initialization_id: InitializationId,
    pub chosen_root_revision: RoleRevisionNumber,
    pub incumbent: DurableSessionRef,
    pub approval_evidence: EvidenceRef,
    pub initialization_evidence: EvidenceRef,
    pub effective_at: EvidenceTime,
    pub recorded_at: Rfc3339,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SuccessfulPredecessor {
    pub role_revision: RoleRevisionNumber,
    pub cutex_session_id: CutexSessionId,
    pub transfer_transition_id: TransitionId,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleRevisionState {
    InitializedCurrent,
    Candidate,
    Current,
    Superseded,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionState {
    Prepared,
    CandidateRecorded,
    Adopted,
    InitialDeliveryRecorded,
    Acknowledged,
    AuthorityTransferred,
    Completed,
    Failed,
    Cancelled,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RotationPhase {
    Prepare,
    Candidate,
    Adoption,
    InitialDelivery,
    Acknowledgement,
    Transfer,
    Completion,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalOutcome {
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    IdentityMismatch,
    EvidenceMismatch,
    ExternalFailure,
    Rejected,
    HumanCancelled,
    SupersededRequest,
    ExternalCancelled,
    PersistenceOutcomeUnknown,
    DeliveryOutcomeUnknown,
    AdoptionOutcomeUnknown,
    AcknowledgementOutcomeUnknown,
    TransferOutcomeUnknown,
    CompletionOutcomeUnknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FailedAttempt {
    pub attempt: AttemptNumber,
    pub outcome: TerminalOutcome,
    pub phase: RotationPhase,
    pub reason_code: ReasonCode,
    pub evidence: EvidenceRef,
    pub recorded_at: Rfc3339,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoleRevision {
    pub role_revision: RoleRevisionNumber,
    pub session: Option<DurableSessionRef>,
    pub state: RoleRevisionState,
    pub intended_predecessor: Option<AuthoritySnapshot>,
    pub successful_predecessor: Option<SuccessfulPredecessor>,
    pub root_revision: Option<RoleRevisionNumber>,
    pub allocated_at: Rfc3339,
    pub terminal_attempt: Option<FailedAttempt>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffRef {
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    pub handoff_sha256: Sha256,
    pub recipient: RuntimeIdentity,
    pub acceptance_receipt: EvidenceRef,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateEvidence {
    pub session: DurableSessionRef,
    pub receipt: EvidenceRef,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdoptionEvidence {
    pub identity: RuntimeIdentity,
    pub receipt: EvidenceRef,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryEvidence {
    pub delivery_id: DeliveryId,
    pub recipient: RuntimeIdentity,
    pub receipt: EvidenceRef,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AcknowledgementEvidence {
    pub responder: RuntimeIdentity,
    pub handoff_sha256: Sha256,
    pub receipt: EvidenceRef,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionEvidenceSnapshot {
    pub candidate_evidence: Option<CandidateEvidence>,
    pub adoption_evidence: Option<AdoptionEvidence>,
    pub delivery_evidence: Option<DeliveryEvidence>,
    pub acknowledgement_evidence: Option<AcknowledgementEvidence>,
    pub completion_evidence: Option<EvidenceRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub enum PhasePayload {
    Prepare {
        source_authority: AuthoritySnapshot,
        handoff: HandoffRef,
    },
    Candidate {
        candidate: CandidateEvidence,
    },
    Adoption {
        candidate_session: DurableSessionRef,
        adoption: AdoptionEvidence,
    },
    InitialDelivery {
        delivery: DeliveryEvidence,
    },
    Acknowledgement {
        acknowledgement: AcknowledgementEvidence,
    },
    Transfer {
        fresh_incumbent: DurableSessionRef,
        candidate_session: DurableSessionRef,
        recipient: RuntimeIdentity,
        evidence: EvidenceRef,
    },
    Completion {
        transition_id: TransitionId,
        evidence: EvidenceRef,
    },
}

impl PhasePayload {
    fn phase(&self) -> RotationPhase {
        match self {
            Self::Prepare { .. } => RotationPhase::Prepare,
            Self::Candidate { .. } => RotationPhase::Candidate,
            Self::Adoption { .. } => RotationPhase::Adoption,
            Self::InitialDelivery { .. } => RotationPhase::InitialDelivery,
            Self::Acknowledgement { .. } => RotationPhase::Acknowledgement,
            Self::Transfer { .. } => RotationPhase::Transfer,
            Self::Completion { .. } => RotationPhase::Completion,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionPriorSnapshot {
    pub transition_state: TransitionState,
    pub revision_state: RoleRevisionState,
    pub intended_predecessor: AuthoritySnapshot,
    pub current_authority: AuthoritySnapshot,
    pub active_rotation: RotationLock,
    pub evidence: TransitionEvidenceSnapshot,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UnknownPostState {
    pub transition_state: TransitionState,
    pub revision_state: RoleRevisionState,
    pub current_authority: AuthoritySnapshot,
    pub active_rotation: RotationLock,
    pub evidence: TransitionEvidenceSnapshot,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PhaseSuccessPostState {
    pub transition_state: TransitionState,
    pub revision_state: RoleRevisionState,
    pub current_authority: AuthoritySnapshot,
    pub active_rotation: Option<RotationLock>,
    pub evidence: TransitionEvidenceSnapshot,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalPostState {
    pub transition_state: TransitionState,
    pub revision_state: RoleRevisionState,
    pub current_authority: AuthoritySnapshot,
    pub active_rotation: Option<RotationLock>,
    pub evidence: TransitionEvidenceSnapshot,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResolutionOutcome {
    PhaseSucceeded {
        verified_payload: PhasePayload,
        post_state: PhaseSuccessPostState,
    },
    Failed {
        attempt: FailedAttempt,
        post_state: TerminalPostState,
    },
    Cancelled {
        attempt: FailedAttempt,
        post_state: TerminalPostState,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UnknownResolution {
    pub outcome: ResolutionOutcome,
    pub evidence: EvidenceRef,
    pub recorded_at: Rfc3339,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UnknownOutcome {
    pub initialization_id: InitializationId,
    pub transition_id: TransitionId,
    pub attempt: AttemptNumber,
    pub phase: RotationPhase,
    pub prior: TransitionPriorSnapshot,
    pub attempted_payload: PhasePayload,
    pub reason_code: ReasonCode,
    pub evidence: EvidenceRef,
    pub recorded_at: Rfc3339,
    pub post_state: UnknownPostState,
    pub resolution: Option<UnknownResolution>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoleTransition {
    pub transition_id: TransitionId,
    pub candidate_revision: RoleRevisionNumber,
    pub intended_predecessor: AuthoritySnapshot,
    pub approval_evidence: EvidenceRef,
    pub handoff: HandoffRef,
    pub state: TransitionState,
    pub candidate_evidence: Option<CandidateEvidence>,
    pub adoption_evidence: Option<AdoptionEvidence>,
    pub delivery_evidence: Option<DeliveryEvidence>,
    pub acknowledgement_evidence: Option<AcknowledgementEvidence>,
    pub completion_evidence: Option<EvidenceRef>,
    pub unknown_outcomes: Vec<UnknownOutcome>,
    pub terminal_attempt: Option<FailedAttempt>,
    pub created_at: Rfc3339,
    pub updated_at: Rfc3339,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoleFamily {
    pub role_family_id: RoleFamilyId,
    pub project_id: ProjectId,
    pub role_key: RoleKey,
    pub root_initialization: RootInitialization,
    pub next_role_revision: RoleRevisionNumber,
    pub current_authority: CurrentAuthority,
    pub active_rotation: Option<RotationLock>,
    pub revisions: BTreeMap<RoleRevisionNumber, RoleRevision>,
    pub transitions: BTreeMap<TransitionId, RoleTransition>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    InitializeFamily,
    PrepareRotation,
    RecordCandidate,
    RecordAdoption,
    RecordInitialDelivery,
    RecordAcknowledgement,
    TransferAuthority,
    CompleteRotation,
    FailRotation,
    CancelRotation,
    RecordUnknown,
    ResolveUnknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionContext {
    pub project_id: ProjectId,
    pub role_family_id: RoleFamilyId,
    pub initialization_id: InitializationId,
    pub transition_id: TransitionId,
    pub candidate_revision: RoleRevisionNumber,
    pub intended_predecessor: AuthoritySnapshot,
    pub handoff: HandoffRef,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FamilyAllocatorObservation {
    pub project_id: ProjectId,
    pub role_family_id: RoleFamilyId,
    pub initialization_id: InitializationId,
    pub observed_store_revision: StoreRevision,
    pub next_role_revision: RoleRevisionNumber,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InitializeFamilyRequest {
    pub project_id: ProjectId,
    pub role_family_id: RoleFamilyId,
    pub role_key: RoleKey,
    pub initialization_id: InitializationId,
    pub chosen_root_revision: RoleRevisionNumber,
    pub incumbent: DurableSessionRef,
    pub human_approval_id: HumanApprovalId,
    pub approval_evidence: EvidenceRef,
    pub initialization_evidence: EvidenceRef,
    pub effective_at: EvidenceTime,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareRotationRequest {
    pub project_id: ProjectId,
    pub role_family_id: RoleFamilyId,
    pub initialization_id: InitializationId,
    pub transition_id: TransitionId,
    pub source_authority: AuthoritySnapshot,
    pub allocator: FamilyAllocatorObservation,
    pub human_approval_id: HumanApprovalId,
    pub approval_evidence: EvidenceRef,
    pub handoff: HandoffRef,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecordCandidateRequest {
    pub context: TransitionContext,
    pub successor: DurableSessionRef,
    pub evidence: EvidenceRef,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecordAdoptionRequest {
    pub context: TransitionContext,
    pub candidate_session: DurableSessionRef,
    pub identity: RuntimeIdentity,
    pub evidence: EvidenceRef,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecordInitialDeliveryRequest {
    pub context: TransitionContext,
    pub delivery_id: DeliveryId,
    pub recipient: RuntimeIdentity,
    pub evidence: EvidenceRef,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecordAcknowledgementRequest {
    pub context: TransitionContext,
    pub responder: RuntimeIdentity,
    pub handoff_sha256: Sha256,
    pub evidence: EvidenceRef,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransferAuthorityRequest {
    pub context: TransitionContext,
    pub fresh_incumbent: DurableSessionRef,
    pub candidate_session: DurableSessionRef,
    pub adopted_identity: RuntimeIdentity,
    pub expected_authority_epoch: AuthorityEpoch,
    pub evidence: EvidenceRef,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompleteRotationRequest {
    pub context: TransitionContext,
    pub adopted_identity: RuntimeIdentity,
    pub evidence: EvidenceRef,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalRequest {
    pub context: TransitionContext,
    pub adopted_identity: RuntimeIdentity,
    pub attempt: AttemptNumber,
    pub phase: RotationPhase,
    pub reason_code: ReasonCode,
    pub evidence: EvidenceRef,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecordUnknownRequest {
    pub context: TransitionContext,
    pub adopted_identity: RuntimeIdentity,
    pub attempt: AttemptNumber,
    pub phase: RotationPhase,
    pub attempted_payload: PhasePayload,
    pub reason_code: ReasonCode,
    pub evidence: EvidenceRef,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResolutionIntent {
    PhaseSucceeded {
        verified_payload: PhasePayload,
    },
    Failed {
        reason_code: ReasonCode,
        evidence: EvidenceRef,
    },
    Cancelled {
        reason_code: ReasonCode,
        evidence: EvidenceRef,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveUnknownRequest {
    pub context: TransitionContext,
    pub adopted_identity: RuntimeIdentity,
    pub attempt: AttemptNumber,
    pub outcome: ResolutionIntent,
    pub evidence: EvidenceRef,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "operation",
    content = "input",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum MutationRequest {
    InitializeFamily(InitializeFamilyRequest),
    PrepareRotation(PrepareRotationRequest),
    RecordCandidate(RecordCandidateRequest),
    RecordAdoption(RecordAdoptionRequest),
    RecordInitialDelivery(RecordInitialDeliveryRequest),
    RecordAcknowledgement(RecordAcknowledgementRequest),
    TransferAuthority(TransferAuthorityRequest),
    CompleteRotation(CompleteRotationRequest),
    FailRotation(TerminalRequest),
    CancelRotation(TerminalRequest),
    RecordUnknown(RecordUnknownRequest),
    ResolveUnknown(ResolveUnknownRequest),
}

impl MutationRequest {
    pub fn operation(&self) -> Operation {
        match self {
            Self::InitializeFamily(_) => Operation::InitializeFamily,
            Self::PrepareRotation(_) => Operation::PrepareRotation,
            Self::RecordCandidate(_) => Operation::RecordCandidate,
            Self::RecordAdoption(_) => Operation::RecordAdoption,
            Self::RecordInitialDelivery(_) => Operation::RecordInitialDelivery,
            Self::RecordAcknowledgement(_) => Operation::RecordAcknowledgement,
            Self::TransferAuthority(_) => Operation::TransferAuthority,
            Self::CompleteRotation(_) => Operation::CompleteRotation,
            Self::FailRotation(_) => Operation::FailRotation,
            Self::CancelRotation(_) => Operation::CancelRotation,
            Self::RecordUnknown(_) => Operation::RecordUnknown,
            Self::ResolveUnknown(_) => Operation::ResolveUnknown,
        }
    }

    fn scope(&self) -> (&ProjectId, &RoleFamilyId, &InitializationId) {
        match self {
            Self::InitializeFamily(input) => (
                &input.project_id,
                &input.role_family_id,
                &input.initialization_id,
            ),
            Self::PrepareRotation(input) => (
                &input.project_id,
                &input.role_family_id,
                &input.initialization_id,
            ),
            Self::RecordCandidate(input) => (
                &input.context.project_id,
                &input.context.role_family_id,
                &input.context.initialization_id,
            ),
            Self::RecordAdoption(input) => (
                &input.context.project_id,
                &input.context.role_family_id,
                &input.context.initialization_id,
            ),
            Self::RecordInitialDelivery(input) => (
                &input.context.project_id,
                &input.context.role_family_id,
                &input.context.initialization_id,
            ),
            Self::RecordAcknowledgement(input) => (
                &input.context.project_id,
                &input.context.role_family_id,
                &input.context.initialization_id,
            ),
            Self::TransferAuthority(input) => (
                &input.context.project_id,
                &input.context.role_family_id,
                &input.context.initialization_id,
            ),
            Self::CompleteRotation(input) => (
                &input.context.project_id,
                &input.context.role_family_id,
                &input.context.initialization_id,
            ),
            Self::FailRotation(input) | Self::CancelRotation(input) => (
                &input.context.project_id,
                &input.context.role_family_id,
                &input.context.initialization_id,
            ),
            Self::RecordUnknown(input) => (
                &input.context.project_id,
                &input.context.role_family_id,
                &input.context.initialization_id,
            ),
            Self::ResolveUnknown(input) => (
                &input.context.project_id,
                &input.context.role_family_id,
                &input.context.initialization_id,
            ),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RequestEnvelope {
    pub schema: RequestSchema,
    pub request_id: RequestId,
    pub request_digest_sha256: Sha256,
    pub expected_store_revision: StoreRevision,
    pub request: MutationRequest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "disposition", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResultDisposition {
    Applied,
    Replay {
        original_request_id: RequestId,
        request_digest_sha256: Sha256,
        original_committed_store_revision: StoreRevision,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum MutationResult {
    InitializeFamily {
        role_family_id: RoleFamilyId,
        root_revision: RoleRevisionNumber,
        authority_epoch: AuthorityEpoch,
    },
    PrepareRotation {
        transition_id: TransitionId,
        candidate_revision: RoleRevisionNumber,
        source_authority_epoch: AuthorityEpoch,
    },
    RecordCandidate {
        transition_id: TransitionId,
        candidate_revision: RoleRevisionNumber,
        session: DurableSessionRef,
    },
    RecordAdoption {
        transition_id: TransitionId,
        identity: RuntimeIdentity,
    },
    RecordInitialDelivery {
        transition_id: TransitionId,
        delivery_id: DeliveryId,
    },
    RecordAcknowledgement {
        transition_id: TransitionId,
        handoff_sha256: Sha256,
    },
    TransferAuthority {
        transition_id: TransitionId,
        role_revision: RoleRevisionNumber,
        cutex_session_id: CutexSessionId,
        authority_epoch: AuthorityEpoch,
    },
    CompleteRotation {
        transition_id: TransitionId,
        role_revision: RoleRevisionNumber,
    },
    FailRotation {
        transition_id: TransitionId,
        attempt: FailedAttempt,
    },
    CancelRotation {
        transition_id: TransitionId,
        attempt: FailedAttempt,
    },
    RecordUnknown {
        transition_id: TransitionId,
        unknown: UnknownOutcome,
    },
    ResolveUnknown {
        transition_id: TransitionId,
        unknown: UnknownOutcome,
    },
}

impl MutationResult {
    pub fn operation(&self) -> Operation {
        match self {
            Self::InitializeFamily { .. } => Operation::InitializeFamily,
            Self::PrepareRotation { .. } => Operation::PrepareRotation,
            Self::RecordCandidate { .. } => Operation::RecordCandidate,
            Self::RecordAdoption { .. } => Operation::RecordAdoption,
            Self::RecordInitialDelivery { .. } => Operation::RecordInitialDelivery,
            Self::RecordAcknowledgement { .. } => Operation::RecordAcknowledgement,
            Self::TransferAuthority { .. } => Operation::TransferAuthority,
            Self::CompleteRotation { .. } => Operation::CompleteRotation,
            Self::FailRotation { .. } => Operation::FailRotation,
            Self::CancelRotation { .. } => Operation::CancelRotation,
            Self::RecordUnknown { .. } => Operation::RecordUnknown,
            Self::ResolveUnknown { .. } => Operation::ResolveUnknown,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MutationResponse {
    pub schema: ResultSchema,
    pub request_id: RequestId,
    pub operation: Operation,
    pub project_id: ProjectId,
    pub role_family_id: RoleFamilyId,
    pub initialization_id: InitializationId,
    pub disposition: ResultDisposition,
    pub committed_store_revision: StoreRevision,
    pub result: MutationResult,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IdempotencyRecord {
    pub operation: Operation,
    pub project_id: ProjectId,
    pub role_family_id: RoleFamilyId,
    pub initialization_id: InitializationId,
    pub request_digest_sha256: Sha256,
    pub committed_store_revision: StoreRevision,
    pub result: MutationResult,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoleSeatStore {
    pub schema: StoreSchema,
    pub store_revision: StoreRevision,
    pub family: Option<RoleFamily>,
    pub idempotency: BTreeMap<RequestId, IdempotencyRecord>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidationCode {
    RequestIdMismatch,
    OperationMismatch,
    ReplayMismatch,
    EvidenceKindMismatch,
    EvidenceSubjectsMismatch,
    ContextMismatch,
    ResultMismatch,
    TerminalMismatch,
    UnknownMismatch,
    NumericOverflow,
    StoreEnvelopeMismatch,
    RootMismatch,
    RevisionMismatch,
    TransitionMismatch,
    LineageMismatch,
    AllocatorMismatch,
    LockMismatch,
    IdempotencyMismatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValidationError {
    pub code: ValidationCode,
}

impl ValidationError {
    fn new(code: ValidationCode) -> Self {
        Self { code }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "role-seat request validation failed: {:?}",
            self.code
        )
    }
}

impl std::error::Error for ValidationError {}

fn evidence(
    actual: &EvidenceRef,
    kind: EvidenceKind,
    subjects: Vec<IdentityRef>,
) -> Result<(), ValidationError> {
    if actual.kind != kind {
        return Err(ValidationError::new(ValidationCode::EvidenceKindMismatch));
    }
    if actual.subjects != subjects {
        return Err(ValidationError::new(
            ValidationCode::EvidenceSubjectsMismatch,
        ));
    }
    Ok(())
}

fn runtime_subjects(identity: &RuntimeIdentity) -> Vec<IdentityRef> {
    vec![
        IdentityRef::CutexSession {
            id: identity.cutex_session_id.clone(),
        },
        IdentityRef::CuteCodexSession {
            id: identity.cute_codex_session_id.clone(),
        },
        IdentityRef::RuntimeAgent {
            id: identity.runtime_agent_id.clone(),
            generation: identity.runtime_generation,
        },
    ]
}

fn transition_subjects(context: &TransitionContext) -> Vec<IdentityRef> {
    vec![
        IdentityRef::Project {
            id: context.project_id.clone(),
        },
        IdentityRef::RoleFamily {
            id: context.role_family_id.clone(),
        },
        IdentityRef::Task {
            id: context.handoff.task_id.clone(),
        },
    ]
}

fn validate_context(context: &TransitionContext) -> Result<(), ValidationError> {
    let mut subjects = transition_subjects(context);
    subjects.extend(runtime_subjects(&context.handoff.recipient));
    evidence(
        &context.handoff.acceptance_receipt,
        EvidenceKind::HandoffAcceptance,
        subjects,
    )
}

fn validate_runtime_context(
    context: &TransitionContext,
    identity: &RuntimeIdentity,
) -> Result<(), ValidationError> {
    validate_context(context)?;
    if identity != &context.handoff.recipient {
        return Err(ValidationError::new(ValidationCode::ContextMismatch));
    }
    Ok(())
}

fn expected_prior_state(phase: RotationPhase) -> TransitionState {
    match phase {
        RotationPhase::Prepare => TransitionState::Prepared,
        RotationPhase::Candidate => TransitionState::Prepared,
        RotationPhase::Adoption => TransitionState::CandidateRecorded,
        RotationPhase::InitialDelivery => TransitionState::Adopted,
        RotationPhase::Acknowledgement => TransitionState::InitialDeliveryRecorded,
        RotationPhase::Transfer => TransitionState::Acknowledged,
        RotationPhase::Completion => TransitionState::AuthorityTransferred,
    }
}

fn expected_success_state(phase: RotationPhase) -> TransitionState {
    match phase {
        RotationPhase::Prepare => TransitionState::Prepared,
        RotationPhase::Candidate => TransitionState::CandidateRecorded,
        RotationPhase::Adoption => TransitionState::Adopted,
        RotationPhase::InitialDelivery => TransitionState::InitialDeliveryRecorded,
        RotationPhase::Acknowledgement => TransitionState::Acknowledged,
        RotationPhase::Transfer => TransitionState::AuthorityTransferred,
        RotationPhase::Completion => TransitionState::Completed,
    }
}

fn reason_matches_terminal(reason: ReasonCode, outcome: TerminalOutcome) -> bool {
    match outcome {
        TerminalOutcome::Failed => matches!(
            reason,
            ReasonCode::IdentityMismatch
                | ReasonCode::EvidenceMismatch
                | ReasonCode::ExternalFailure
                | ReasonCode::Rejected
        ),
        TerminalOutcome::Cancelled => matches!(
            reason,
            ReasonCode::HumanCancelled
                | ReasonCode::SupersededRequest
                | ReasonCode::ExternalCancelled
        ),
    }
}

fn reason_matches_unknown(reason: ReasonCode, phase: RotationPhase) -> bool {
    reason == ReasonCode::PersistenceOutcomeUnknown
        || matches!(
            (reason, phase),
            (ReasonCode::AdoptionOutcomeUnknown, RotationPhase::Adoption)
                | (
                    ReasonCode::DeliveryOutcomeUnknown,
                    RotationPhase::InitialDelivery
                )
                | (
                    ReasonCode::AcknowledgementOutcomeUnknown,
                    RotationPhase::Acknowledgement
                )
                | (ReasonCode::TransferOutcomeUnknown, RotationPhase::Transfer)
                | (
                    ReasonCode::CompletionOutcomeUnknown,
                    RotationPhase::Completion
                )
        )
}

fn validate_candidate_evidence(
    context: &TransitionContext,
    candidate: &CandidateEvidence,
) -> Result<(), ValidationError> {
    let mut subjects = transition_subjects(context);
    subjects.push(IdentityRef::CutexSession {
        id: candidate.session.cutex_session_id.clone(),
    });
    evidence(
        &candidate.receipt,
        EvidenceKind::CandidateCreation,
        subjects,
    )?;
    if candidate.session.cutex_session_id != context.handoff.recipient.cutex_session_id {
        return Err(ValidationError::new(ValidationCode::UnknownMismatch));
    }
    Ok(())
}

fn validate_adoption_evidence(
    context: &TransitionContext,
    identity: &RuntimeIdentity,
    adoption: &AdoptionEvidence,
) -> Result<(), ValidationError> {
    if &adoption.identity != identity {
        return Err(ValidationError::new(ValidationCode::UnknownMismatch));
    }
    let mut subjects = transition_subjects(context);
    subjects.extend(runtime_subjects(identity));
    evidence(&adoption.receipt, EvidenceKind::Adoption, subjects)
}

fn validate_delivery_evidence(
    context: &TransitionContext,
    identity: &RuntimeIdentity,
    delivery: &DeliveryEvidence,
) -> Result<(), ValidationError> {
    let expected = DeliveryId::new(format!("{}/initial", context.transition_id.as_str()))
        .map_err(|_| ValidationError::new(ValidationCode::UnknownMismatch))?;
    if delivery.delivery_id != expected || &delivery.recipient != identity {
        return Err(ValidationError::new(ValidationCode::UnknownMismatch));
    }
    let mut subjects = transition_subjects(context);
    subjects.push(IdentityRef::Delivery {
        id: delivery.delivery_id.clone(),
    });
    subjects.extend(runtime_subjects(identity));
    evidence(&delivery.receipt, EvidenceKind::InitialDelivery, subjects)
}

fn validate_acknowledgement_evidence(
    context: &TransitionContext,
    identity: &RuntimeIdentity,
    acknowledgement: &AcknowledgementEvidence,
) -> Result<(), ValidationError> {
    if &acknowledgement.responder != identity
        || acknowledgement.handoff_sha256 != context.handoff.handoff_sha256
    {
        return Err(ValidationError::new(ValidationCode::UnknownMismatch));
    }
    let mut subjects = transition_subjects(context);
    subjects.extend(runtime_subjects(identity));
    evidence(
        &acknowledgement.receipt,
        EvidenceKind::Acknowledgement,
        subjects,
    )
}

fn validate_completion_evidence(
    context: &TransitionContext,
    identity: &RuntimeIdentity,
    completion: &EvidenceRef,
) -> Result<(), ValidationError> {
    let mut subjects = transition_subjects(context);
    subjects.extend(runtime_subjects(identity));
    evidence(completion, EvidenceKind::Completion, subjects)
}

fn validate_evidence_snapshot(
    context: &TransitionContext,
    identity: &RuntimeIdentity,
    snapshot: &TransitionEvidenceSnapshot,
) -> Result<(), ValidationError> {
    if let Some(candidate) = &snapshot.candidate_evidence {
        validate_candidate_evidence(context, candidate)?;
    }
    if let Some(adoption) = &snapshot.adoption_evidence {
        validate_adoption_evidence(context, identity, adoption)?;
    }
    if let Some(delivery) = &snapshot.delivery_evidence {
        validate_delivery_evidence(context, identity, delivery)?;
    }
    if let Some(acknowledgement) = &snapshot.acknowledgement_evidence {
        validate_acknowledgement_evidence(context, identity, acknowledgement)?;
    }
    if let Some(completion) = &snapshot.completion_evidence {
        validate_completion_evidence(context, identity, completion)?;
    }
    Ok(())
}

fn prior_evidence_shape_matches(
    phase: RotationPhase,
    snapshot: &TransitionEvidenceSnapshot,
) -> bool {
    let present = (
        snapshot.candidate_evidence.is_some(),
        snapshot.adoption_evidence.is_some(),
        snapshot.delivery_evidence.is_some(),
        snapshot.acknowledgement_evidence.is_some(),
        snapshot.completion_evidence.is_some(),
    );
    match phase {
        RotationPhase::Prepare => false,
        RotationPhase::Candidate => present == (false, false, false, false, false),
        RotationPhase::Adoption => present == (true, false, false, false, false),
        RotationPhase::InitialDelivery => present == (true, true, false, false, false),
        RotationPhase::Acknowledgement => present == (true, true, true, false, false),
        RotationPhase::Transfer | RotationPhase::Completion => {
            present == (true, true, true, true, false)
        }
    }
}

fn phase_payload_matches(
    context: &TransitionContext,
    identity: &RuntimeIdentity,
    prior: &TransitionEvidenceSnapshot,
    payload: &PhasePayload,
) -> Result<(), ValidationError> {
    match payload {
        PhasePayload::Prepare {
            source_authority,
            handoff,
        } if source_authority == &context.intended_predecessor && handoff == &context.handoff => {
            Ok(())
        }
        PhasePayload::Prepare { .. } => Err(ValidationError::new(ValidationCode::UnknownMismatch)),
        PhasePayload::Candidate { candidate } => validate_candidate_evidence(context, candidate),
        PhasePayload::Adoption {
            candidate_session,
            adoption,
        } => {
            validate_adoption_evidence(context, identity, adoption)?;
            if candidate_session.cutex_session_id != identity.cutex_session_id
                || prior
                    .candidate_evidence
                    .as_ref()
                    .map(|candidate| &candidate.session)
                    != Some(candidate_session)
            {
                return Err(ValidationError::new(ValidationCode::UnknownMismatch));
            }
            Ok(())
        }
        PhasePayload::InitialDelivery { delivery } => {
            validate_delivery_evidence(context, identity, delivery)
        }
        PhasePayload::Acknowledgement { acknowledgement } => {
            validate_acknowledgement_evidence(context, identity, acknowledgement)
        }
        PhasePayload::Transfer {
            fresh_incumbent,
            candidate_session,
            recipient,
            evidence: transfer_evidence,
        } => {
            let mut subjects = transition_subjects(context);
            subjects.push(IdentityRef::CutexSession {
                id: fresh_incumbent.cutex_session_id.clone(),
            });
            subjects.extend(runtime_subjects(identity));
            evidence(
                transfer_evidence,
                EvidenceKind::TransferVerification,
                subjects,
            )?;
            if recipient != identity
                || candidate_session.cutex_session_id != identity.cutex_session_id
                || fresh_incumbent.cutex_session_id != context.intended_predecessor.cutex_session_id
                || fresh_incumbent.durable_revision
                    != context.intended_predecessor.source_durable_revision
                || prior
                    .candidate_evidence
                    .as_ref()
                    .map(|candidate| &candidate.session)
                    != Some(candidate_session)
            {
                return Err(ValidationError::new(ValidationCode::UnknownMismatch));
            }
            Ok(())
        }
        PhasePayload::Completion {
            transition_id,
            evidence: completion,
        } => {
            validate_completion_evidence(context, identity, completion)?;
            if transition_id != &context.transition_id {
                return Err(ValidationError::new(ValidationCode::UnknownMismatch));
            }
            Ok(())
        }
    }
}

fn expected_success_evidence(
    prior: &TransitionEvidenceSnapshot,
    payload: &PhasePayload,
) -> TransitionEvidenceSnapshot {
    let mut expected = prior.clone();
    match payload {
        PhasePayload::Candidate { candidate } => {
            expected.candidate_evidence = Some(candidate.clone());
        }
        PhasePayload::Adoption { adoption, .. } => {
            expected.adoption_evidence = Some(adoption.clone());
        }
        PhasePayload::InitialDelivery { delivery } => {
            expected.delivery_evidence = Some(delivery.clone());
        }
        PhasePayload::Acknowledgement { acknowledgement } => {
            expected.acknowledgement_evidence = Some(acknowledgement.clone());
        }
        PhasePayload::Completion {
            evidence: completion,
            ..
        } => {
            expected.completion_evidence = Some(completion.clone());
        }
        PhasePayload::Prepare { .. } | PhasePayload::Transfer { .. } => {}
    }
    expected
}

fn transferred_authority_matches(
    context: &TransitionContext,
    identity: &RuntimeIdentity,
    evidence_snapshot: &TransitionEvidenceSnapshot,
    authority: &AuthoritySnapshot,
) -> bool {
    let Ok(next_epoch) = context.intended_predecessor.authority_epoch.checked_next() else {
        return false;
    };
    let Some(candidate) = &evidence_snapshot.candidate_evidence else {
        return false;
    };
    authority.role_revision == context.candidate_revision
        && authority.cutex_session_id == identity.cutex_session_id
        && authority.authority_epoch == next_epoch
        && authority.source_durable_revision == candidate.session.durable_revision
}

fn success_post_matches(
    context: &TransitionContext,
    identity: &RuntimeIdentity,
    unknown: &UnknownOutcome,
    post_state: &PhaseSuccessPostState,
) -> bool {
    if post_state.transition_state != expected_success_state(unknown.phase) {
        return false;
    }
    if post_state.evidence
        != expected_success_evidence(&unknown.prior.evidence, &unknown.attempted_payload)
    {
        return false;
    }
    match unknown.phase {
        RotationPhase::Transfer | RotationPhase::Completion => {
            post_state.revision_state == RoleRevisionState::Current
                && transferred_authority_matches(
                    context,
                    identity,
                    &post_state.evidence,
                    &post_state.current_authority,
                )
                && if unknown.phase == RotationPhase::Completion {
                    post_state.active_rotation.is_none()
                } else {
                    post_state.active_rotation.as_ref() == Some(&unknown.prior.active_rotation)
                }
        }
        _ => {
            post_state.revision_state == unknown.prior.revision_state
                && post_state.current_authority == unknown.prior.current_authority
                && post_state.active_rotation.as_ref() == Some(&unknown.prior.active_rotation)
        }
    }
}

fn validate_unknown(
    context: &TransitionContext,
    identity: &RuntimeIdentity,
    unknown: &UnknownOutcome,
    require_resolution: bool,
) -> Result<(), ValidationError> {
    validate_runtime_context(context, identity)?;
    phase_payload_matches(
        context,
        identity,
        &unknown.prior.evidence,
        &unknown.attempted_payload,
    )?;
    validate_evidence_snapshot(context, identity, &unknown.prior.evidence)?;
    validate_evidence_snapshot(context, identity, &unknown.post_state.evidence)?;
    let prior_authority_matches = if unknown.phase == RotationPhase::Completion {
        transferred_authority_matches(
            context,
            identity,
            &unknown.prior.evidence,
            &unknown.prior.current_authority,
        )
    } else {
        unknown.prior.current_authority == context.intended_predecessor
    };
    if unknown.initialization_id != context.initialization_id
        || unknown.transition_id != context.transition_id
        || unknown.phase != unknown.attempted_payload.phase()
        || !reason_matches_unknown(unknown.reason_code, unknown.phase)
        || !prior_evidence_shape_matches(unknown.phase, &unknown.prior.evidence)
        || unknown.prior.intended_predecessor != context.intended_predecessor
        || !prior_authority_matches
        || unknown.prior.revision_state
            != if unknown.phase == RotationPhase::Completion {
                RoleRevisionState::Current
            } else {
                RoleRevisionState::Candidate
            }
        || unknown.prior.active_rotation.transition_id != context.transition_id
        || unknown.prior.active_rotation.candidate_revision != context.candidate_revision
        || unknown.prior.active_rotation.source_authority_epoch
            != context.intended_predecessor.authority_epoch
        || unknown.prior.transition_state != expected_prior_state(unknown.phase)
        || unknown.post_state.transition_state != TransitionState::Unknown
        || unknown.post_state.revision_state != unknown.prior.revision_state
        || unknown.post_state.current_authority != unknown.prior.current_authority
        || unknown.post_state.active_rotation != unknown.prior.active_rotation
        || unknown.post_state.evidence != unknown.prior.evidence
        || unknown.resolution.is_some() != require_resolution
    {
        return Err(ValidationError::new(ValidationCode::UnknownMismatch));
    }

    let mut subjects = transition_subjects(context);
    subjects.extend(runtime_subjects(identity));
    evidence(
        &unknown.evidence,
        EvidenceKind::UnknownObservation,
        subjects.clone(),
    )?;

    if let Some(resolution) = &unknown.resolution {
        evidence(
            &resolution.evidence,
            EvidenceKind::UnknownResolution,
            subjects.clone(),
        )?;
        match &resolution.outcome {
            ResolutionOutcome::PhaseSucceeded {
                verified_payload,
                post_state,
            } => {
                validate_evidence_snapshot(context, identity, &post_state.evidence)?;
                if verified_payload != &unknown.attempted_payload
                    || verified_payload.phase() != unknown.phase
                    || !success_post_matches(context, identity, unknown, post_state)
                {
                    return Err(ValidationError::new(ValidationCode::UnknownMismatch));
                }
            }
            ResolutionOutcome::Failed {
                attempt,
                post_state,
            } => validate_terminal_resolution(
                unknown,
                attempt,
                post_state,
                TerminalOutcome::Failed,
                &subjects,
            )?,
            ResolutionOutcome::Cancelled {
                attempt,
                post_state,
            } => validate_terminal_resolution(
                unknown,
                attempt,
                post_state,
                TerminalOutcome::Cancelled,
                &subjects,
            )?,
        }
    }
    Ok(())
}

fn validate_terminal_resolution(
    unknown: &UnknownOutcome,
    attempt: &FailedAttempt,
    post_state: &TerminalPostState,
    outcome: TerminalOutcome,
    subjects: &[IdentityRef],
) -> Result<(), ValidationError> {
    let expected_transition = match outcome {
        TerminalOutcome::Failed => TransitionState::Failed,
        TerminalOutcome::Cancelled => TransitionState::Cancelled,
    };
    let expected_revision = match outcome {
        TerminalOutcome::Failed => RoleRevisionState::Failed,
        TerminalOutcome::Cancelled => RoleRevisionState::Cancelled,
    };
    if matches!(
        unknown.phase,
        RotationPhase::Transfer | RotationPhase::Completion
    ) || attempt.attempt != unknown.attempt
        || attempt.outcome != outcome
        || attempt.phase != unknown.phase
        || !reason_matches_terminal(attempt.reason_code, outcome)
        || post_state.transition_state != expected_transition
        || post_state.revision_state != expected_revision
        || post_state.current_authority != unknown.prior.current_authority
        || post_state.active_rotation.is_some()
        || post_state.evidence != unknown.prior.evidence
    {
        return Err(ValidationError::new(ValidationCode::UnknownMismatch));
    }
    evidence(
        &attempt.evidence,
        match outcome {
            TerminalOutcome::Failed => EvidenceKind::Failure,
            TerminalOutcome::Cancelled => EvidenceKind::Cancellation,
        },
        subjects.to_vec(),
    )?;
    Ok(())
}

fn validate_terminal_intent(
    request: &TerminalRequest,
    attempt: &FailedAttempt,
    outcome: TerminalOutcome,
) -> Result<(), ValidationError> {
    validate_runtime_context(&request.context, &request.adopted_identity)?;
    if !reason_matches_terminal(request.reason_code, outcome)
        || matches!(
            request.phase,
            RotationPhase::Transfer | RotationPhase::Completion
        )
    {
        return Err(ValidationError::new(ValidationCode::TerminalMismatch));
    }
    let kind = match outcome {
        TerminalOutcome::Failed => EvidenceKind::Failure,
        TerminalOutcome::Cancelled => EvidenceKind::Cancellation,
    };
    let mut subjects = transition_subjects(&request.context);
    subjects.extend(runtime_subjects(&request.adopted_identity));
    evidence(&request.evidence, kind, subjects)?;
    if attempt.attempt != request.attempt
        || attempt.outcome != outcome
        || attempt.phase != request.phase
        || attempt.reason_code != request.reason_code
        || attempt.evidence != request.evidence
    {
        return Err(ValidationError::new(ValidationCode::TerminalMismatch));
    }
    Ok(())
}

fn validate_record_unknown_intent(
    request: &RecordUnknownRequest,
    unknown: &UnknownOutcome,
) -> Result<(), ValidationError> {
    validate_runtime_context(&request.context, &request.adopted_identity)?;
    let mut subjects = transition_subjects(&request.context);
    subjects.extend(runtime_subjects(&request.adopted_identity));
    evidence(
        &request.evidence,
        EvidenceKind::UnknownObservation,
        subjects,
    )?;
    validate_unknown(&request.context, &request.adopted_identity, unknown, false)?;
    if unknown.attempt != request.attempt
        || unknown.phase != request.phase
        || unknown.attempted_payload != request.attempted_payload
        || unknown.reason_code != request.reason_code
        || unknown.evidence != request.evidence
    {
        return Err(ValidationError::new(ValidationCode::UnknownMismatch));
    }
    Ok(())
}

fn validate_resolve_unknown_intent(
    request: &ResolveUnknownRequest,
    unknown: &UnknownOutcome,
) -> Result<(), ValidationError> {
    validate_runtime_context(&request.context, &request.adopted_identity)?;
    let mut subjects = transition_subjects(&request.context);
    subjects.extend(runtime_subjects(&request.adopted_identity));
    evidence(&request.evidence, EvidenceKind::UnknownResolution, subjects)?;
    validate_unknown(&request.context, &request.adopted_identity, unknown, true)?;
    let resolution = unknown
        .resolution
        .as_ref()
        .ok_or_else(|| ValidationError::new(ValidationCode::UnknownMismatch))?;
    if unknown.attempt != request.attempt || resolution.evidence != request.evidence {
        return Err(ValidationError::new(ValidationCode::UnknownMismatch));
    }
    let matches = match (&request.outcome, &resolution.outcome) {
        (
            ResolutionIntent::PhaseSucceeded { verified_payload },
            ResolutionOutcome::PhaseSucceeded {
                verified_payload: actual,
                ..
            },
        ) => verified_payload == actual,
        (
            ResolutionIntent::Failed {
                reason_code,
                evidence,
            },
            ResolutionOutcome::Failed { attempt, .. },
        ) => attempt.reason_code == *reason_code && attempt.evidence == *evidence,
        (
            ResolutionIntent::Cancelled {
                reason_code,
                evidence,
            },
            ResolutionOutcome::Cancelled { attempt, .. },
        ) => attempt.reason_code == *reason_code && attempt.evidence == *evidence,
        _ => false,
    };
    if !matches {
        return Err(ValidationError::new(ValidationCode::UnknownMismatch));
    }
    Ok(())
}

fn transition_context_for(family: &RoleFamily, transition: &RoleTransition) -> TransitionContext {
    TransitionContext {
        project_id: family.project_id.clone(),
        role_family_id: family.role_family_id.clone(),
        initialization_id: family.root_initialization.initialization_id.clone(),
        transition_id: transition.transition_id.clone(),
        candidate_revision: transition.candidate_revision,
        intended_predecessor: transition.intended_predecessor.clone(),
        handoff: transition.handoff.clone(),
    }
}

fn transition_evidence(transition: &RoleTransition) -> TransitionEvidenceSnapshot {
    TransitionEvidenceSnapshot {
        candidate_evidence: transition.candidate_evidence.clone(),
        adoption_evidence: transition.adoption_evidence.clone(),
        delivery_evidence: transition.delivery_evidence.clone(),
        acknowledgement_evidence: transition.acknowledgement_evidence.clone(),
        completion_evidence: transition.completion_evidence.clone(),
    }
}

fn evidence_shape(snapshot: &TransitionEvidenceSnapshot) -> (bool, bool, bool, bool, bool) {
    (
        snapshot.candidate_evidence.is_some(),
        snapshot.adoption_evidence.is_some(),
        snapshot.delivery_evidence.is_some(),
        snapshot.acknowledgement_evidence.is_some(),
        snapshot.completion_evidence.is_some(),
    )
}

fn expected_transition_evidence_shape(
    state: TransitionState,
    terminal_phase: Option<RotationPhase>,
) -> Option<(bool, bool, bool, bool, bool)> {
    let prior = |phase| match phase {
        RotationPhase::Prepare | RotationPhase::Candidate => {
            Some((false, false, false, false, false))
        }
        RotationPhase::Adoption => Some((true, false, false, false, false)),
        RotationPhase::InitialDelivery => Some((true, true, false, false, false)),
        RotationPhase::Acknowledgement => Some((true, true, true, false, false)),
        RotationPhase::Transfer | RotationPhase::Completion => None,
    };
    match state {
        TransitionState::Prepared => Some((false, false, false, false, false)),
        TransitionState::CandidateRecorded => Some((true, false, false, false, false)),
        TransitionState::Adopted => Some((true, true, false, false, false)),
        TransitionState::InitialDeliveryRecorded => Some((true, true, true, false, false)),
        TransitionState::Acknowledged | TransitionState::AuthorityTransferred => {
            Some((true, true, true, true, false))
        }
        TransitionState::Completed => Some((true, true, true, true, true)),
        TransitionState::Failed | TransitionState::Cancelled => terminal_phase.and_then(prior),
        TransitionState::Unknown => None,
    }
}

fn evidence_is_prefix(
    prefix: &TransitionEvidenceSnapshot,
    current: &TransitionEvidenceSnapshot,
) -> bool {
    fn optional_prefix<T: PartialEq>(prefix: &Option<T>, current: &Option<T>) -> bool {
        prefix
            .as_ref()
            .map_or(true, |value| current.as_ref() == Some(value))
    }
    optional_prefix(&prefix.candidate_evidence, &current.candidate_evidence)
        && optional_prefix(&prefix.adoption_evidence, &current.adoption_evidence)
        && optional_prefix(&prefix.delivery_evidence, &current.delivery_evidence)
        && optional_prefix(
            &prefix.acknowledgement_evidence,
            &current.acknowledgement_evidence,
        )
        && optional_prefix(&prefix.completion_evidence, &current.completion_evidence)
}

fn successful_depth(
    family: &RoleFamily,
    start: RoleRevisionNumber,
) -> Result<u64, ValidationError> {
    let root = family.root_initialization.chosen_root_revision;
    let mut current = start;
    let mut depth = 0_u64;
    let mut seen = BTreeSet::new();
    loop {
        if !seen.insert(current) {
            return Err(ValidationError::new(ValidationCode::LineageMismatch));
        }
        let revision = family
            .revisions
            .get(&current)
            .ok_or_else(|| ValidationError::new(ValidationCode::LineageMismatch))?;
        if current == root {
            if revision.successful_predecessor.is_some() {
                return Err(ValidationError::new(ValidationCode::LineageMismatch));
            }
            return Ok(depth);
        }
        let predecessor = revision
            .successful_predecessor
            .as_ref()
            .ok_or_else(|| ValidationError::new(ValidationCode::LineageMismatch))?;
        if predecessor.role_revision >= current {
            return Err(ValidationError::new(ValidationCode::LineageMismatch));
        }
        let predecessor_revision = family
            .revisions
            .get(&predecessor.role_revision)
            .ok_or_else(|| ValidationError::new(ValidationCode::LineageMismatch))?;
        if predecessor_revision
            .session
            .as_ref()
            .map(|session| &session.cutex_session_id)
            != Some(&predecessor.cutex_session_id)
        {
            return Err(ValidationError::new(ValidationCode::LineageMismatch));
        }
        depth = depth
            .checked_add(1)
            .filter(|value| *value < MAX_JSON_SAFE_INTEGER)
            .ok_or_else(|| ValidationError::new(ValidationCode::NumericOverflow))?;
        current = predecessor.role_revision;
    }
}

fn validate_successful_lineage(family: &RoleFamily) -> Result<(), ValidationError> {
    let root = family.root_initialization.chosen_root_revision;
    let mut reverse_lineage = Vec::new();
    let mut revision_number = family.current_authority.role_revision;
    let mut seen = BTreeSet::new();

    loop {
        if !seen.insert(revision_number) {
            return Err(ValidationError::new(ValidationCode::LineageMismatch));
        }
        let revision = family
            .revisions
            .get(&revision_number)
            .ok_or_else(|| ValidationError::new(ValidationCode::LineageMismatch))?;
        if revision_number == root {
            if revision.successful_predecessor.is_some() {
                return Err(ValidationError::new(ValidationCode::LineageMismatch));
            }
            break;
        }
        let predecessor = revision
            .successful_predecessor
            .as_ref()
            .ok_or_else(|| ValidationError::new(ValidationCode::LineageMismatch))?;
        let transition = family
            .transitions
            .get(&predecessor.transfer_transition_id)
            .ok_or_else(|| ValidationError::new(ValidationCode::LineageMismatch))?;
        if transition.candidate_revision != revision_number {
            return Err(ValidationError::new(ValidationCode::LineageMismatch));
        }
        reverse_lineage.push(revision_number);
        revision_number = predecessor.role_revision;
    }

    let mut successful_revisions = BTreeSet::new();
    successful_revisions.insert(root);
    let mut previous_revision = root;
    let mut previous_epoch = AuthorityEpoch::new(1)
        .map_err(|_| ValidationError::new(ValidationCode::NumericOverflow))?;

    for revision_number in reverse_lineage.into_iter().rev() {
        let revision = family
            .revisions
            .get(&revision_number)
            .ok_or_else(|| ValidationError::new(ValidationCode::LineageMismatch))?;
        let predecessor = revision
            .successful_predecessor
            .as_ref()
            .ok_or_else(|| ValidationError::new(ValidationCode::LineageMismatch))?;
        let predecessor_revision = family
            .revisions
            .get(&previous_revision)
            .ok_or_else(|| ValidationError::new(ValidationCode::LineageMismatch))?;
        let predecessor_session = predecessor_revision
            .session
            .as_ref()
            .ok_or_else(|| ValidationError::new(ValidationCode::LineageMismatch))?;
        let transition = family
            .transitions
            .get(&predecessor.transfer_transition_id)
            .ok_or_else(|| ValidationError::new(ValidationCode::LineageMismatch))?;

        if predecessor.role_revision != previous_revision
            || predecessor.cutex_session_id != predecessor_session.cutex_session_id
            || transition.candidate_revision != revision_number
            || transition.intended_predecessor.role_revision != previous_revision
            || transition.intended_predecessor.cutex_session_id
                != predecessor_session.cutex_session_id
            || transition.intended_predecessor.source_durable_revision
                != predecessor_session.durable_revision
            || transition.intended_predecessor.authority_epoch != previous_epoch
        {
            return Err(ValidationError::new(ValidationCode::LineageMismatch));
        }

        previous_epoch = previous_epoch
            .checked_next()
            .map_err(|_| ValidationError::new(ValidationCode::NumericOverflow))?;
        previous_revision = revision_number;
        successful_revisions.insert(revision_number);
    }

    if family.current_authority.role_revision != previous_revision
        || family.current_authority.authority_epoch != previous_epoch
    {
        return Err(ValidationError::new(ValidationCode::LineageMismatch));
    }

    for revision in family.revisions.values() {
        if revision.role_revision != root
            && revision.successful_predecessor.is_some()
            && !successful_revisions.contains(&revision.role_revision)
        {
            return Err(ValidationError::new(ValidationCode::LineageMismatch));
        }
    }
    for transition in family.transitions.values() {
        if matches!(
            transition.state,
            TransitionState::AuthorityTransferred | TransitionState::Completed
        ) && !successful_revisions.contains(&transition.candidate_revision)
        {
            return Err(ValidationError::new(ValidationCode::LineageMismatch));
        }
    }
    Ok(())
}

fn authority_snapshot_matches(
    family: &RoleFamily,
    snapshot: &AuthoritySnapshot,
) -> Result<(), ValidationError> {
    let revision = family
        .revisions
        .get(&snapshot.role_revision)
        .ok_or_else(|| ValidationError::new(ValidationCode::LineageMismatch))?;
    let session = revision
        .session
        .as_ref()
        .ok_or_else(|| ValidationError::new(ValidationCode::LineageMismatch))?;
    let expected_epoch = AuthorityEpoch::new(
        successful_depth(family, snapshot.role_revision)?
            .checked_add(1)
            .ok_or_else(|| ValidationError::new(ValidationCode::NumericOverflow))?,
    )
    .map_err(|_| ValidationError::new(ValidationCode::NumericOverflow))?;
    if snapshot.cutex_session_id != session.cutex_session_id
        || snapshot.source_durable_revision != session.durable_revision
        || snapshot.authority_epoch != expected_epoch
    {
        return Err(ValidationError::new(ValidationCode::LineageMismatch));
    }
    Ok(())
}

fn current_authority_snapshot(family: &RoleFamily) -> Result<AuthoritySnapshot, ValidationError> {
    let revision = family
        .revisions
        .get(&family.current_authority.role_revision)
        .ok_or_else(|| ValidationError::new(ValidationCode::RevisionMismatch))?;
    let session = revision
        .session
        .as_ref()
        .ok_or_else(|| ValidationError::new(ValidationCode::RevisionMismatch))?;
    Ok(AuthoritySnapshot {
        role_revision: family.current_authority.role_revision,
        cutex_session_id: family.current_authority.cutex_session_id.clone(),
        authority_epoch: family.current_authority.authority_epoch,
        source_durable_revision: session.durable_revision,
    })
}

fn validate_root(family: &RoleFamily) -> Result<(), ValidationError> {
    let root = &family.root_initialization;
    let Some(IdentityRef::HumanApproval { id }) = root.approval_evidence.subjects.first() else {
        return Err(ValidationError::new(ValidationCode::RootMismatch));
    };
    evidence(
        &root.approval_evidence,
        EvidenceKind::HumanApproval,
        vec![
            IdentityRef::HumanApproval { id: id.clone() },
            IdentityRef::Project {
                id: family.project_id.clone(),
            },
            IdentityRef::RoleFamily {
                id: family.role_family_id.clone(),
            },
            IdentityRef::CutexSession {
                id: root.incumbent.cutex_session_id.clone(),
            },
        ],
    )?;
    evidence(
        &root.initialization_evidence,
        EvidenceKind::RootInitialization,
        vec![
            IdentityRef::Project {
                id: family.project_id.clone(),
            },
            IdentityRef::RoleFamily {
                id: family.role_family_id.clone(),
            },
            IdentityRef::CutexSession {
                id: root.incumbent.cutex_session_id.clone(),
            },
        ],
    )?;
    let revision = family
        .revisions
        .get(&root.chosen_root_revision)
        .ok_or_else(|| ValidationError::new(ValidationCode::RootMismatch))?;
    if revision.role_revision != root.chosen_root_revision
        || revision.session.as_ref() != Some(&root.incumbent)
        || revision.intended_predecessor.is_some()
        || revision.successful_predecessor.is_some()
        || revision.root_revision != Some(root.chosen_root_revision)
        || revision.terminal_attempt.is_some()
    {
        return Err(ValidationError::new(ValidationCode::RootMismatch));
    }
    Ok(())
}

fn phase_progress(phase: RotationPhase) -> u8 {
    match phase {
        RotationPhase::Prepare => 0,
        RotationPhase::Candidate => 1,
        RotationPhase::Adoption => 2,
        RotationPhase::InitialDelivery => 3,
        RotationPhase::Acknowledgement => 4,
        RotationPhase::Transfer => 5,
        RotationPhase::Completion => 6,
    }
}

fn transition_progress(state: TransitionState) -> Option<u8> {
    match state {
        TransitionState::Prepared => Some(0),
        TransitionState::CandidateRecorded => Some(1),
        TransitionState::Adopted => Some(2),
        TransitionState::InitialDeliveryRecorded => Some(3),
        TransitionState::Acknowledged => Some(4),
        TransitionState::AuthorityTransferred => Some(5),
        TransitionState::Completed => Some(6),
        TransitionState::Failed | TransitionState::Cancelled | TransitionState::Unknown => None,
    }
}

fn successful_unknown_post_allows_next_prior(
    context: &TransitionContext,
    previous_phase: RotationPhase,
    post_state: &PhaseSuccessPostState,
    next: &UnknownOutcome,
) -> bool {
    next.prior.intended_predecessor == context.intended_predecessor
        && post_state.active_rotation.as_ref() == Some(&next.prior.active_rotation)
        && phase_progress(next.phase) > phase_progress(previous_phase)
        && transition_progress(next.prior.transition_state)
            .zip(transition_progress(post_state.transition_state))
            .is_some_and(|(next, previous)| next >= previous)
        && evidence_is_prefix(&post_state.evidence, &next.prior.evidence)
        && if next.phase == RotationPhase::Completion {
            next.prior.transition_state == TransitionState::AuthorityTransferred
        } else {
            next.prior.revision_state == post_state.revision_state
                && next.prior.current_authority == post_state.current_authority
        }
}

fn validate_transition_unknowns<'a>(
    family: &RoleFamily,
    transition: &'a RoleTransition,
    context: &TransitionContext,
    current_evidence: &TransitionEvidenceSnapshot,
) -> Result<Option<&'a UnknownOutcome>, ValidationError> {
    let identity = &transition.handoff.recipient;
    let mut unresolved = None;
    let mut seen_attempts = BTreeSet::new();
    let mut previous_attempt = None;
    let mut previous_recorded_at: Option<&Rfc3339> = None;
    let mut previous_success: Option<(RotationPhase, &PhaseSuccessPostState, &Rfc3339)> = None;
    for (index, unknown) in transition.unknown_outcomes.iter().enumerate() {
        if let Some((previous_phase, post_state, resolved_at)) = previous_success {
            if unknown.recorded_at.as_str() < resolved_at.as_str()
                || !successful_unknown_post_allows_next_prior(
                    context,
                    previous_phase,
                    post_state,
                    unknown,
                )
            {
                return Err(ValidationError::new(ValidationCode::UnknownMismatch));
            }
        }
        if !seen_attempts.insert(unknown.attempt)
            || previous_attempt.is_some_and(|attempt| unknown.attempt <= attempt)
            || previous_recorded_at
                .is_some_and(|recorded_at| unknown.recorded_at.as_str() < recorded_at.as_str())
            || unknown.resolution.as_ref().is_some_and(|resolution| {
                resolution.recorded_at.as_str() < unknown.recorded_at.as_str()
                    || resolution.recorded_at.as_str() > transition.updated_at.as_str()
            })
            || unknown.recorded_at.as_str() < transition.created_at.as_str()
            || unknown.recorded_at.as_str() > transition.updated_at.as_str()
        {
            return Err(ValidationError::new(ValidationCode::UnknownMismatch));
        }
        previous_attempt = Some(unknown.attempt);
        previous_recorded_at = Some(&unknown.recorded_at);
        let resolved = unknown.resolution.is_some();
        validate_unknown(context, identity, unknown, resolved)?;
        match unknown
            .resolution
            .as_ref()
            .map(|resolution| &resolution.outcome)
        {
            None => {
                if unresolved.is_some() || index + 1 != transition.unknown_outcomes.len() {
                    return Err(ValidationError::new(ValidationCode::UnknownMismatch));
                }
                unresolved = Some(unknown);
                previous_success = None;
            }
            Some(ResolutionOutcome::PhaseSucceeded { post_state, .. }) => {
                if !evidence_is_prefix(&post_state.evidence, current_evidence) {
                    return Err(ValidationError::new(ValidationCode::UnknownMismatch));
                }
                previous_success = Some((
                    unknown.phase,
                    post_state,
                    &unknown.resolution.as_ref().unwrap().recorded_at,
                ));
            }
            Some(ResolutionOutcome::Failed {
                attempt,
                post_state,
            })
            | Some(ResolutionOutcome::Cancelled {
                attempt,
                post_state,
            }) => {
                if index + 1 != transition.unknown_outcomes.len()
                    || transition.terminal_attempt.as_ref() != Some(attempt)
                    || transition.state != post_state.transition_state
                    || current_evidence != &post_state.evidence
                {
                    return Err(ValidationError::new(ValidationCode::UnknownMismatch));
                }
                previous_success = None;
            }
        }
    }

    if let Some(unknown) = unresolved {
        let current_authority = current_authority_snapshot(family)?;
        if transition.state != TransitionState::Unknown
            || current_evidence != &unknown.post_state.evidence
            || family.active_rotation.as_ref() != Some(&unknown.post_state.active_rotation)
            || current_authority != unknown.post_state.current_authority
        {
            return Err(ValidationError::new(ValidationCode::UnknownMismatch));
        }
    } else if transition.state == TransitionState::Unknown {
        return Err(ValidationError::new(ValidationCode::UnknownMismatch));
    }

    if let Some(attempt) = &transition.terminal_attempt {
        let resolved_terminal = transition.unknown_outcomes.last().is_some_and(|unknown| {
            unknown
                .resolution
                .as_ref()
                .is_some_and(|resolution| match &resolution.outcome {
                    ResolutionOutcome::Failed {
                        attempt: resolved, ..
                    }
                    | ResolutionOutcome::Cancelled {
                        attempt: resolved, ..
                    } => resolved == attempt,
                    ResolutionOutcome::PhaseSucceeded { .. } => false,
                })
        });
        if !resolved_terminal
            && previous_attempt.is_some_and(|previous| attempt.attempt <= previous)
        {
            return Err(ValidationError::new(ValidationCode::TerminalMismatch));
        }
    }
    Ok(unresolved)
}

fn validate_failed_attempt(
    context: &TransitionContext,
    identity: &RuntimeIdentity,
    attempt: &FailedAttempt,
    outcome: TerminalOutcome,
) -> Result<(), ValidationError> {
    if attempt.outcome != outcome
        || !reason_matches_terminal(attempt.reason_code, outcome)
        || matches!(
            attempt.phase,
            RotationPhase::Transfer | RotationPhase::Completion
        )
    {
        return Err(ValidationError::new(ValidationCode::TerminalMismatch));
    }
    let mut subjects = transition_subjects(context);
    subjects.extend(runtime_subjects(identity));
    evidence(
        &attempt.evidence,
        match outcome {
            TerminalOutcome::Failed => EvidenceKind::Failure,
            TerminalOutcome::Cancelled => EvidenceKind::Cancellation,
        },
        subjects,
    )
}

fn validate_transition(
    family: &RoleFamily,
    transition: &RoleTransition,
    revision: &RoleRevision,
) -> Result<(), ValidationError> {
    if transition.created_at.as_str() > transition.updated_at.as_str() {
        return Err(ValidationError::new(ValidationCode::TransitionMismatch));
    }
    authority_snapshot_matches(family, &transition.intended_predecessor)?;
    if transition.intended_predecessor.role_revision >= transition.candidate_revision {
        return Err(ValidationError::new(ValidationCode::LineageMismatch));
    }
    let context = transition_context_for(family, transition);
    validate_context(&context)?;
    let identity = &transition.handoff.recipient;

    let Some(IdentityRef::HumanApproval { id }) = transition.approval_evidence.subjects.first()
    else {
        return Err(ValidationError::new(ValidationCode::TransitionMismatch));
    };
    evidence(
        &transition.approval_evidence,
        EvidenceKind::HumanApproval,
        vec![
            IdentityRef::HumanApproval { id: id.clone() },
            IdentityRef::Project {
                id: family.project_id.clone(),
            },
            IdentityRef::RoleFamily {
                id: family.role_family_id.clone(),
            },
            IdentityRef::CutexSession {
                id: transition.intended_predecessor.cutex_session_id.clone(),
            },
            IdentityRef::Task {
                id: transition.handoff.task_id.clone(),
            },
        ],
    )?;

    let current_evidence = transition_evidence(transition);
    validate_evidence_snapshot(&context, identity, &current_evidence)?;
    let unresolved = validate_transition_unknowns(family, transition, &context, &current_evidence)?;

    if revision.role_revision != transition.candidate_revision
        || revision.intended_predecessor.as_ref() != Some(&transition.intended_predecessor)
        || revision.session
            != transition
                .candidate_evidence
                .as_ref()
                .map(|candidate| candidate.session.clone())
    {
        return Err(ValidationError::new(ValidationCode::RevisionMismatch));
    }

    let expected_terminal = match transition.state {
        TransitionState::Failed => Some(TerminalOutcome::Failed),
        TransitionState::Cancelled => Some(TerminalOutcome::Cancelled),
        _ => None,
    };
    if let Some(outcome) = expected_terminal {
        let attempt = transition
            .terminal_attempt
            .as_ref()
            .ok_or_else(|| ValidationError::new(ValidationCode::TerminalMismatch))?;
        validate_failed_attempt(&context, identity, attempt, outcome)?;
        let expected_shape =
            expected_transition_evidence_shape(transition.state, Some(attempt.phase))
                .ok_or_else(|| ValidationError::new(ValidationCode::TransitionMismatch))?;
        let expected_revision = match outcome {
            TerminalOutcome::Failed => RoleRevisionState::Failed,
            TerminalOutcome::Cancelled => RoleRevisionState::Cancelled,
        };
        if evidence_shape(&current_evidence) != expected_shape
            || revision.state != expected_revision
            || revision.terminal_attempt.as_ref() != Some(attempt)
            || revision.successful_predecessor.is_some()
            || revision.root_revision.is_some()
        {
            return Err(ValidationError::new(ValidationCode::TerminalMismatch));
        }
    } else if transition.terminal_attempt.is_some() || revision.terminal_attempt.is_some() {
        return Err(ValidationError::new(ValidationCode::TerminalMismatch));
    }

    if !matches!(
        transition.state,
        TransitionState::Unknown | TransitionState::Failed | TransitionState::Cancelled
    ) {
        let expected_shape = expected_transition_evidence_shape(transition.state, None)
            .ok_or_else(|| ValidationError::new(ValidationCode::TransitionMismatch))?;
        if evidence_shape(&current_evidence) != expected_shape {
            return Err(ValidationError::new(ValidationCode::TransitionMismatch));
        }
    }

    if let Some(candidate) = &transition.candidate_evidence {
        if candidate.session.cutex_session_id != identity.cutex_session_id {
            return Err(ValidationError::new(ValidationCode::TransitionMismatch));
        }
    }

    let root = family.root_initialization.chosen_root_revision;
    if let Some(predecessor) = &revision.successful_predecessor {
        let predecessor_revision = family
            .revisions
            .get(&predecessor.role_revision)
            .ok_or_else(|| ValidationError::new(ValidationCode::LineageMismatch))?;
        let predecessor_session = predecessor_revision
            .session
            .as_ref()
            .ok_or_else(|| ValidationError::new(ValidationCode::LineageMismatch))?;
        let success_state = matches!(
            transition.state,
            TransitionState::AuthorityTransferred | TransitionState::Completed
        ) || unresolved
            .is_some_and(|unknown| unknown.phase == RotationPhase::Completion);
        if !success_state
            || predecessor.transfer_transition_id != transition.transition_id
            || predecessor.role_revision != transition.intended_predecessor.role_revision
            || predecessor.cutex_session_id != transition.intended_predecessor.cutex_session_id
            || predecessor_session.cutex_session_id != predecessor.cutex_session_id
            || predecessor_session.durable_revision
                != transition.intended_predecessor.source_durable_revision
            || revision.root_revision != Some(root)
            || transition.candidate_evidence.is_none()
        {
            return Err(ValidationError::new(ValidationCode::LineageMismatch));
        }
        let expected_state = if family.current_authority.role_revision == revision.role_revision {
            RoleRevisionState::Current
        } else {
            RoleRevisionState::Superseded
        };
        if revision.state != expected_state {
            return Err(ValidationError::new(ValidationCode::RevisionMismatch));
        }
    } else if expected_terminal.is_none() {
        if revision.state != RoleRevisionState::Candidate || revision.root_revision.is_some() {
            return Err(ValidationError::new(ValidationCode::RevisionMismatch));
        }
        if matches!(
            transition.state,
            TransitionState::AuthorityTransferred | TransitionState::Completed
        ) || unresolved.is_some_and(|unknown| unknown.phase == RotationPhase::Completion)
        {
            return Err(ValidationError::new(ValidationCode::LineageMismatch));
        }
    }
    Ok(())
}

fn idempotency_result_matches(family: &RoleFamily, result: &MutationResult) -> bool {
    match result {
        MutationResult::InitializeFamily {
            role_family_id,
            root_revision,
            authority_epoch,
        } => {
            role_family_id == &family.role_family_id
                && root_revision == &family.root_initialization.chosen_root_revision
                && authority_epoch.get() == 1
        }
        MutationResult::PrepareRotation {
            transition_id,
            candidate_revision,
            source_authority_epoch,
        } => family
            .transitions
            .get(transition_id)
            .is_some_and(|transition| {
                candidate_revision == &transition.candidate_revision
                    && source_authority_epoch == &transition.intended_predecessor.authority_epoch
            }),
        MutationResult::RecordCandidate {
            transition_id,
            candidate_revision,
            session,
        } => family
            .transitions
            .get(transition_id)
            .is_some_and(|transition| {
                candidate_revision == &transition.candidate_revision
                    && transition
                        .candidate_evidence
                        .as_ref()
                        .map(|candidate| &candidate.session)
                        == Some(session)
            }),
        MutationResult::RecordAdoption {
            transition_id,
            identity,
        } => family
            .transitions
            .get(transition_id)
            .is_some_and(|transition| {
                transition
                    .adoption_evidence
                    .as_ref()
                    .map(|adoption| &adoption.identity)
                    == Some(identity)
            }),
        MutationResult::RecordInitialDelivery {
            transition_id,
            delivery_id,
        } => family
            .transitions
            .get(transition_id)
            .is_some_and(|transition| {
                transition
                    .delivery_evidence
                    .as_ref()
                    .map(|delivery| &delivery.delivery_id)
                    == Some(delivery_id)
            }),
        MutationResult::RecordAcknowledgement {
            transition_id,
            handoff_sha256,
        } => family
            .transitions
            .get(transition_id)
            .is_some_and(|transition| {
                transition
                    .acknowledgement_evidence
                    .as_ref()
                    .map(|acknowledgement| &acknowledgement.handoff_sha256)
                    == Some(handoff_sha256)
            }),
        MutationResult::TransferAuthority {
            transition_id,
            role_revision,
            cutex_session_id,
            authority_epoch,
        } => family
            .transitions
            .get(transition_id)
            .is_some_and(|transition| {
                let Some(revision) = family.revisions.get(&transition.candidate_revision) else {
                    return false;
                };
                let Ok(expected_epoch) = transition
                    .intended_predecessor
                    .authority_epoch
                    .checked_next()
                else {
                    return false;
                };
                role_revision == &transition.candidate_revision
                    && revision.successful_predecessor.is_some()
                    && revision
                        .session
                        .as_ref()
                        .map(|session| &session.cutex_session_id)
                        == Some(cutex_session_id)
                    && authority_epoch == &expected_epoch
            }),
        MutationResult::CompleteRotation {
            transition_id,
            role_revision,
        } => family
            .transitions
            .get(transition_id)
            .is_some_and(|transition| {
                role_revision == &transition.candidate_revision
                    && transition.state == TransitionState::Completed
                    && transition.completion_evidence.is_some()
            }),
        MutationResult::FailRotation {
            transition_id,
            attempt,
        } => family
            .transitions
            .get(transition_id)
            .is_some_and(|transition| {
                transition.state == TransitionState::Failed
                    && transition.terminal_attempt.as_ref() == Some(attempt)
            }),
        MutationResult::CancelRotation {
            transition_id,
            attempt,
        } => family
            .transitions
            .get(transition_id)
            .is_some_and(|transition| {
                transition.state == TransitionState::Cancelled
                    && transition.terminal_attempt.as_ref() == Some(attempt)
            }),
        MutationResult::RecordUnknown {
            transition_id,
            unknown,
        } => {
            unknown.resolution.is_none()
                && family
                    .transitions
                    .get(transition_id)
                    .is_some_and(|transition| {
                        transition.unknown_outcomes.iter().any(|stored| {
                            let mut original = stored.clone();
                            original.resolution = None;
                            &original == unknown
                        })
                    })
        }
        MutationResult::ResolveUnknown {
            transition_id,
            unknown,
        } => {
            unknown.resolution.is_some()
                && family
                    .transitions
                    .get(transition_id)
                    .is_some_and(|transition| {
                        transition
                            .unknown_outcomes
                            .iter()
                            .any(|stored| stored == unknown)
                    })
        }
    }
}

fn validate_idempotency(store: &RoleSeatStore, family: &RoleFamily) -> Result<(), ValidationError> {
    let mut committed_revisions = BTreeSet::new();
    for record in store.idempotency.values() {
        if record.operation != record.result.operation()
            || record.project_id != family.project_id
            || record.role_family_id != family.role_family_id
            || record.initialization_id != family.root_initialization.initialization_id
            || record.committed_store_revision > store.store_revision
            || !committed_revisions.insert(record.committed_store_revision)
            || !idempotency_result_matches(family, &record.result)
        {
            return Err(ValidationError::new(ValidationCode::IdempotencyMismatch));
        }
    }
    Ok(())
}

fn validate_family(store: &RoleSeatStore, family: &RoleFamily) -> Result<(), ValidationError> {
    validate_root(family)?;
    let root_number = family.root_initialization.chosen_root_revision;
    let mut expected_revision = root_number;
    for (key, revision) in &family.revisions {
        if key != &revision.role_revision || *key != expected_revision {
            return Err(ValidationError::new(ValidationCode::AllocatorMismatch));
        }
        expected_revision = expected_revision
            .checked_next()
            .map_err(|_| ValidationError::new(ValidationCode::NumericOverflow))?;
    }
    if family.revisions.is_empty() || family.next_role_revision != expected_revision {
        return Err(ValidationError::new(ValidationCode::AllocatorMismatch));
    }

    let root = family
        .revisions
        .get(&root_number)
        .ok_or_else(|| ValidationError::new(ValidationCode::RootMismatch))?;
    let current_is_root = family.current_authority.role_revision == root_number;
    if root.state
        != if current_is_root {
            RoleRevisionState::InitializedCurrent
        } else {
            RoleRevisionState::Superseded
        }
    {
        return Err(ValidationError::new(ValidationCode::RootMismatch));
    }

    let mut candidate_revisions = BTreeSet::new();
    let mut unresolved_count = 0_usize;
    let mut live_transitions = Vec::new();
    for (key, transition) in &family.transitions {
        if key != &transition.transition_id
            || !candidate_revisions.insert(transition.candidate_revision)
            || transition.candidate_revision == root_number
        {
            return Err(ValidationError::new(ValidationCode::TransitionMismatch));
        }
        let revision = family
            .revisions
            .get(&transition.candidate_revision)
            .ok_or_else(|| ValidationError::new(ValidationCode::TransitionMismatch))?;
        validate_transition(family, transition, revision)?;
        unresolved_count += transition
            .unknown_outcomes
            .iter()
            .filter(|unknown| unknown.resolution.is_none())
            .count();
        if matches!(
            transition.state,
            TransitionState::Prepared
                | TransitionState::CandidateRecorded
                | TransitionState::Adopted
                | TransitionState::InitialDeliveryRecorded
                | TransitionState::Acknowledged
                | TransitionState::AuthorityTransferred
                | TransitionState::Unknown
        ) {
            live_transitions.push(transition);
        }
    }
    if unresolved_count > 1 {
        return Err(ValidationError::new(ValidationCode::UnknownMismatch));
    }
    if candidate_revisions.len() + 1 != family.revisions.len()
        || family
            .revisions
            .keys()
            .any(|revision| *revision != root_number && !candidate_revisions.contains(revision))
    {
        return Err(ValidationError::new(ValidationCode::RevisionMismatch));
    }

    let current_revision = family
        .revisions
        .get(&family.current_authority.role_revision)
        .ok_or_else(|| ValidationError::new(ValidationCode::RevisionMismatch))?;
    if current_revision
        .session
        .as_ref()
        .map(|session| &session.cutex_session_id)
        != Some(&family.current_authority.cutex_session_id)
        || !matches!(
            current_revision.state,
            RoleRevisionState::InitializedCurrent | RoleRevisionState::Current
        )
        || family
            .revisions
            .values()
            .filter(|revision| {
                matches!(
                    revision.state,
                    RoleRevisionState::InitializedCurrent | RoleRevisionState::Current
                )
            })
            .count()
            != 1
    {
        return Err(ValidationError::new(ValidationCode::RevisionMismatch));
    }
    validate_successful_lineage(family)?;
    let expected_epoch = AuthorityEpoch::new(
        successful_depth(family, family.current_authority.role_revision)?
            .checked_add(1)
            .ok_or_else(|| ValidationError::new(ValidationCode::NumericOverflow))?,
    )
    .map_err(|_| ValidationError::new(ValidationCode::NumericOverflow))?;
    if family.current_authority.authority_epoch != expected_epoch {
        return Err(ValidationError::new(ValidationCode::LineageMismatch));
    }

    match &family.current_authority.established_by {
        EstablishedBy::RootInitialization { initialization_id } => {
            if !current_is_root
                || initialization_id != &family.root_initialization.initialization_id
                || family.current_authority.authority_epoch.get() != 1
                || family.current_authority.effective_at != family.root_initialization.effective_at
            {
                return Err(ValidationError::new(ValidationCode::RootMismatch));
            }
        }
        EstablishedBy::Transfer { transition_id } => {
            let transition = family
                .transitions
                .get(transition_id)
                .ok_or_else(|| ValidationError::new(ValidationCode::LineageMismatch))?;
            if current_is_root
                || transition.candidate_revision != family.current_authority.role_revision
                || current_revision
                    .successful_predecessor
                    .as_ref()
                    .map(|predecessor| &predecessor.transfer_transition_id)
                    != Some(transition_id)
            {
                return Err(ValidationError::new(ValidationCode::LineageMismatch));
            }
        }
    }

    match (family.active_rotation.as_ref(), live_transitions.as_slice()) {
        (None, []) => {}
        (Some(active), [transition]) => {
            if active.transition_id != transition.transition_id
                || active.candidate_revision != transition.candidate_revision
                || active.source_authority_epoch != transition.intended_predecessor.authority_epoch
            {
                return Err(ValidationError::new(ValidationCode::LockMismatch));
            }
            let before_transfer =
                !matches!(transition.state, TransitionState::AuthorityTransferred)
                    && !(transition.state == TransitionState::Unknown
                        && transition
                            .unknown_outcomes
                            .iter()
                            .find(|unknown| unknown.resolution.is_none())
                            .is_some_and(|unknown| unknown.phase == RotationPhase::Completion));
            if before_transfer
                && transition.intended_predecessor != current_authority_snapshot(family)?
            {
                return Err(ValidationError::new(ValidationCode::LockMismatch));
            }
        }
        _ => return Err(ValidationError::new(ValidationCode::LockMismatch)),
    }

    validate_idempotency(store, family)
}

pub fn validate_store(store: &RoleSeatStore) -> Result<(), ValidationError> {
    match &store.family {
        None if store.idempotency.is_empty() => Ok(()),
        None => Err(ValidationError::new(ValidationCode::StoreEnvelopeMismatch)),
        Some(family) => validate_family(store, family),
    }
}

pub fn validate_request(
    request: &RequestEnvelope,
    response: &MutationResponse,
) -> Result<(), ValidationError> {
    let operation = request.request.operation();
    if response.request_id != request.request_id {
        return Err(ValidationError::new(ValidationCode::RequestIdMismatch));
    }
    if response.operation != operation || response.result.operation() != operation {
        return Err(ValidationError::new(ValidationCode::OperationMismatch));
    }
    let (project_id, role_family_id, initialization_id) = request.request.scope();
    if &response.project_id != project_id
        || &response.role_family_id != role_family_id
        || &response.initialization_id != initialization_id
    {
        return Err(ValidationError::new(ValidationCode::ContextMismatch));
    }
    match &response.disposition {
        ResultDisposition::Applied => {
            let expected_committed = request
                .expected_store_revision
                .checked_next()
                .map_err(|_| ValidationError::new(ValidationCode::NumericOverflow))?;
            if response.committed_store_revision != expected_committed {
                return Err(ValidationError::new(ValidationCode::ResultMismatch));
            }
        }
        ResultDisposition::Replay {
            original_request_id,
            request_digest_sha256,
            original_committed_store_revision,
        } => {
            let expected_committed = request
                .expected_store_revision
                .checked_next()
                .map_err(|_| ValidationError::new(ValidationCode::NumericOverflow))?;
            if original_request_id != &request.request_id
                || request_digest_sha256 != &request.request_digest_sha256
                || original_committed_store_revision != &response.committed_store_revision
                || response.committed_store_revision != expected_committed
            {
                return Err(ValidationError::new(ValidationCode::ReplayMismatch));
            }
        }
    }

    match (&request.request, &response.result) {
        (
            MutationRequest::InitializeFamily(input),
            MutationResult::InitializeFamily {
                role_family_id,
                root_revision,
                authority_epoch,
            },
        ) => {
            evidence(
                &input.approval_evidence,
                EvidenceKind::HumanApproval,
                vec![
                    IdentityRef::HumanApproval {
                        id: input.human_approval_id.clone(),
                    },
                    IdentityRef::Project {
                        id: input.project_id.clone(),
                    },
                    IdentityRef::RoleFamily {
                        id: input.role_family_id.clone(),
                    },
                    IdentityRef::CutexSession {
                        id: input.incumbent.cutex_session_id.clone(),
                    },
                ],
            )?;
            evidence(
                &input.initialization_evidence,
                EvidenceKind::RootInitialization,
                vec![
                    IdentityRef::Project {
                        id: input.project_id.clone(),
                    },
                    IdentityRef::RoleFamily {
                        id: input.role_family_id.clone(),
                    },
                    IdentityRef::CutexSession {
                        id: input.incumbent.cutex_session_id.clone(),
                    },
                ],
            )?;
            if role_family_id != &input.role_family_id
                || root_revision != &input.chosen_root_revision
                || authority_epoch.get() != 1
            {
                return Err(ValidationError::new(ValidationCode::ResultMismatch));
            }
        }
        (
            MutationRequest::PrepareRotation(input),
            MutationResult::PrepareRotation {
                transition_id,
                candidate_revision,
                source_authority_epoch,
            },
        ) => {
            let mut handoff_subjects = vec![
                IdentityRef::Project {
                    id: input.project_id.clone(),
                },
                IdentityRef::RoleFamily {
                    id: input.role_family_id.clone(),
                },
                IdentityRef::Task {
                    id: input.handoff.task_id.clone(),
                },
            ];
            handoff_subjects.extend(runtime_subjects(&input.handoff.recipient));
            evidence(
                &input.handoff.acceptance_receipt,
                EvidenceKind::HandoffAcceptance,
                handoff_subjects,
            )?;
            evidence(
                &input.approval_evidence,
                EvidenceKind::HumanApproval,
                vec![
                    IdentityRef::HumanApproval {
                        id: input.human_approval_id.clone(),
                    },
                    IdentityRef::Project {
                        id: input.project_id.clone(),
                    },
                    IdentityRef::RoleFamily {
                        id: input.role_family_id.clone(),
                    },
                    IdentityRef::CutexSession {
                        id: input.source_authority.cutex_session_id.clone(),
                    },
                    IdentityRef::Task {
                        id: input.handoff.task_id.clone(),
                    },
                ],
            )?;
            if input.allocator.project_id != input.project_id
                || input.allocator.role_family_id != input.role_family_id
                || input.allocator.initialization_id != input.initialization_id
                || input.allocator.observed_store_revision != request.expected_store_revision
                || input.allocator.next_role_revision <= input.source_authority.role_revision
                || transition_id != &input.transition_id
                || candidate_revision != &input.allocator.next_role_revision
                || source_authority_epoch != &input.source_authority.authority_epoch
            {
                return Err(ValidationError::new(ValidationCode::ResultMismatch));
            }
        }
        (
            MutationRequest::RecordCandidate(input),
            MutationResult::RecordCandidate {
                transition_id,
                candidate_revision,
                session,
            },
        ) => {
            validate_context(&input.context)?;
            let mut subjects = transition_subjects(&input.context);
            subjects.push(IdentityRef::CutexSession {
                id: input.successor.cutex_session_id.clone(),
            });
            evidence(&input.evidence, EvidenceKind::CandidateCreation, subjects)?;
            if transition_id != &input.context.transition_id
                || candidate_revision != &input.context.candidate_revision
                || session != &input.successor
                || input.successor.cutex_session_id
                    != input.context.handoff.recipient.cutex_session_id
            {
                return Err(ValidationError::new(ValidationCode::ResultMismatch));
            }
        }
        (
            MutationRequest::RecordAdoption(input),
            MutationResult::RecordAdoption {
                transition_id,
                identity,
            },
        ) => {
            validate_runtime_context(&input.context, &input.identity)?;
            let mut subjects = transition_subjects(&input.context);
            subjects.extend(runtime_subjects(&input.identity));
            evidence(&input.evidence, EvidenceKind::Adoption, subjects)?;
            if transition_id != &input.context.transition_id
                || identity != &input.identity
                || input.candidate_session.cutex_session_id != input.identity.cutex_session_id
            {
                return Err(ValidationError::new(ValidationCode::ResultMismatch));
            }
        }
        (
            MutationRequest::RecordInitialDelivery(input),
            MutationResult::RecordInitialDelivery {
                transition_id,
                delivery_id,
            },
        ) => {
            validate_runtime_context(&input.context, &input.recipient)?;
            let expected_delivery =
                DeliveryId::new(format!("{}/initial", input.context.transition_id.as_str()))
                    .map_err(|_| ValidationError::new(ValidationCode::ContextMismatch))?;
            let mut subjects = transition_subjects(&input.context);
            subjects.push(IdentityRef::Delivery {
                id: input.delivery_id.clone(),
            });
            subjects.extend(runtime_subjects(&input.recipient));
            evidence(&input.evidence, EvidenceKind::InitialDelivery, subjects)?;
            if input.delivery_id != expected_delivery
                || transition_id != &input.context.transition_id
                || delivery_id != &input.delivery_id
            {
                return Err(ValidationError::new(ValidationCode::ResultMismatch));
            }
        }
        (
            MutationRequest::RecordAcknowledgement(input),
            MutationResult::RecordAcknowledgement {
                transition_id,
                handoff_sha256,
            },
        ) => {
            validate_runtime_context(&input.context, &input.responder)?;
            let mut subjects = transition_subjects(&input.context);
            subjects.extend(runtime_subjects(&input.responder));
            evidence(&input.evidence, EvidenceKind::Acknowledgement, subjects)?;
            if input.handoff_sha256 != input.context.handoff.handoff_sha256
                || transition_id != &input.context.transition_id
                || handoff_sha256 != &input.handoff_sha256
            {
                return Err(ValidationError::new(ValidationCode::ResultMismatch));
            }
        }
        (
            MutationRequest::TransferAuthority(input),
            MutationResult::TransferAuthority {
                transition_id,
                role_revision,
                cutex_session_id,
                authority_epoch,
            },
        ) => {
            validate_runtime_context(&input.context, &input.adopted_identity)?;
            let mut subjects = transition_subjects(&input.context);
            subjects.push(IdentityRef::CutexSession {
                id: input.fresh_incumbent.cutex_session_id.clone(),
            });
            subjects.extend(runtime_subjects(&input.adopted_identity));
            evidence(
                &input.evidence,
                EvidenceKind::TransferVerification,
                subjects,
            )?;
            let next_epoch = input
                .expected_authority_epoch
                .checked_next()
                .map_err(|_| ValidationError::new(ValidationCode::NumericOverflow))?;
            if input.expected_authority_epoch != input.context.intended_predecessor.authority_epoch
                || input.fresh_incumbent.cutex_session_id
                    != input.context.intended_predecessor.cutex_session_id
                || input.fresh_incumbent.durable_revision
                    != input.context.intended_predecessor.source_durable_revision
                || input.candidate_session.cutex_session_id
                    != input.adopted_identity.cutex_session_id
                || transition_id != &input.context.transition_id
                || role_revision != &input.context.candidate_revision
                || cutex_session_id != &input.candidate_session.cutex_session_id
                || authority_epoch != &next_epoch
            {
                return Err(ValidationError::new(ValidationCode::ResultMismatch));
            }
        }
        (
            MutationRequest::CompleteRotation(input),
            MutationResult::CompleteRotation {
                transition_id,
                role_revision,
            },
        ) => {
            validate_runtime_context(&input.context, &input.adopted_identity)?;
            let mut subjects = transition_subjects(&input.context);
            subjects.extend(runtime_subjects(&input.adopted_identity));
            evidence(&input.evidence, EvidenceKind::Completion, subjects)?;
            if transition_id != &input.context.transition_id
                || role_revision != &input.context.candidate_revision
            {
                return Err(ValidationError::new(ValidationCode::ResultMismatch));
            }
        }
        (
            MutationRequest::FailRotation(input),
            MutationResult::FailRotation {
                transition_id,
                attempt,
            },
        ) => {
            validate_terminal_intent(input, attempt, TerminalOutcome::Failed)?;
            if transition_id != &input.context.transition_id {
                return Err(ValidationError::new(ValidationCode::TerminalMismatch));
            }
        }
        (
            MutationRequest::CancelRotation(input),
            MutationResult::CancelRotation {
                transition_id,
                attempt,
            },
        ) => {
            validate_terminal_intent(input, attempt, TerminalOutcome::Cancelled)?;
            if transition_id != &input.context.transition_id {
                return Err(ValidationError::new(ValidationCode::TerminalMismatch));
            }
        }
        (
            MutationRequest::RecordUnknown(input),
            MutationResult::RecordUnknown {
                transition_id,
                unknown,
            },
        ) => {
            validate_record_unknown_intent(input, unknown)?;
            if transition_id != &input.context.transition_id {
                return Err(ValidationError::new(ValidationCode::UnknownMismatch));
            }
        }
        (
            MutationRequest::ResolveUnknown(input),
            MutationResult::ResolveUnknown {
                transition_id,
                unknown,
            },
        ) => {
            validate_resolve_unknown_intent(input, unknown)?;
            if transition_id != &input.context.transition_id {
                return Err(ValidationError::new(ValidationCode::UnknownMismatch));
            }
        }
        _ => return Err(ValidationError::new(ValidationCode::OperationMismatch)),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use serde_json::{json, Value};

    fn project() -> ProjectId {
        ProjectId::new("project-a").unwrap()
    }

    fn family() -> RoleFamilyId {
        RoleFamilyId::new("family-a").unwrap()
    }

    fn initialization() -> InitializationId {
        InitializationId::new("initialization-a").unwrap()
    }

    fn transition() -> TransitionId {
        TransitionId::new("transition-a").unwrap()
    }

    fn request_id(operation: Operation) -> RequestId {
        RequestId::new(format!("request-{operation:?}").to_ascii_lowercase()).unwrap()
    }

    fn timestamp() -> Rfc3339 {
        Rfc3339::new("2026-08-16T00:00:00Z").unwrap()
    }

    fn sha(character: char) -> Sha256 {
        Sha256::new(character.to_string().repeat(64)).unwrap()
    }

    fn incumbent() -> DurableSessionRef {
        DurableSessionRef {
            cutex_session_id: CutexSessionId::new("cutex-incumbent").unwrap(),
            durable_revision: DurableRevision::new(3).unwrap(),
        }
    }

    fn candidate() -> DurableSessionRef {
        DurableSessionRef {
            cutex_session_id: CutexSessionId::new("cutex-candidate").unwrap(),
            durable_revision: DurableRevision::new(1).unwrap(),
        }
    }

    fn runtime() -> RuntimeIdentity {
        RuntimeIdentity {
            cutex_session_id: candidate().cutex_session_id,
            cute_codex_session_id: CuteCodexSessionId::new("cute-codex-candidate").unwrap(),
            runtime_agent_id: RuntimeAgentId::new("runtime-candidate").unwrap(),
            runtime_generation: RuntimeGeneration::new(4).unwrap(),
        }
    }

    fn authority() -> AuthoritySnapshot {
        AuthoritySnapshot {
            role_revision: RoleRevisionNumber::new(7).unwrap(),
            cutex_session_id: incumbent().cutex_session_id,
            authority_epoch: AuthorityEpoch::new(1).unwrap(),
            source_durable_revision: incumbent().durable_revision,
        }
    }

    fn receipt(kind: EvidenceKind, subjects: Vec<IdentityRef>, suffix: &str) -> EvidenceRef {
        EvidenceRef {
            kind,
            receipt_id: ReceiptId::new(format!("receipt-{suffix}")).unwrap(),
            receipt_sha256: sha('a'),
            subjects,
            occurred_at: EvidenceTime::Known {
                rfc3339: timestamp(),
            },
        }
    }

    fn base_subjects() -> Vec<IdentityRef> {
        vec![
            IdentityRef::Project { id: project() },
            IdentityRef::RoleFamily { id: family() },
            IdentityRef::Task {
                id: TaskId::new("task-a").unwrap(),
            },
        ]
    }

    fn runtime_evidence_subjects(identity: &RuntimeIdentity) -> Vec<IdentityRef> {
        let mut subjects = base_subjects();
        subjects.extend(runtime_subjects(identity));
        subjects
    }

    fn handoff() -> HandoffRef {
        let recipient = runtime();
        HandoffRef {
            task_id: TaskId::new("task-a").unwrap(),
            task_revision: TaskRevision::new(2).unwrap(),
            handoff_sha256: sha('b'),
            recipient: recipient.clone(),
            acceptance_receipt: receipt(
                EvidenceKind::HandoffAcceptance,
                runtime_evidence_subjects(&recipient),
                "handoff",
            ),
        }
    }

    fn context() -> TransitionContext {
        TransitionContext {
            project_id: project(),
            role_family_id: family(),
            initialization_id: initialization(),
            transition_id: transition(),
            candidate_revision: RoleRevisionNumber::new(8).unwrap(),
            intended_predecessor: authority(),
            handoff: handoff(),
        }
    }

    fn lock() -> RotationLock {
        RotationLock {
            transition_id: transition(),
            candidate_revision: RoleRevisionNumber::new(8).unwrap(),
            source_authority_epoch: AuthorityEpoch::new(1).unwrap(),
        }
    }

    fn candidate_evidence() -> CandidateEvidence {
        let session = candidate();
        let mut subjects = base_subjects();
        subjects.push(IdentityRef::CutexSession {
            id: session.cutex_session_id.clone(),
        });
        CandidateEvidence {
            session,
            receipt: receipt(
                EvidenceKind::CandidateCreation,
                subjects,
                "snapshot-candidate",
            ),
        }
    }

    fn adoption_evidence() -> AdoptionEvidence {
        let identity = runtime();
        AdoptionEvidence {
            identity: identity.clone(),
            receipt: receipt(
                EvidenceKind::Adoption,
                runtime_evidence_subjects(&identity),
                "snapshot-adoption",
            ),
        }
    }

    fn delivery_evidence() -> DeliveryEvidence {
        let identity = runtime();
        let delivery_id = DeliveryId::new("transition-a/initial").unwrap();
        let mut subjects = base_subjects();
        subjects.push(IdentityRef::Delivery {
            id: delivery_id.clone(),
        });
        subjects.extend(runtime_subjects(&identity));
        DeliveryEvidence {
            delivery_id,
            recipient: identity,
            receipt: receipt(EvidenceKind::InitialDelivery, subjects, "snapshot-delivery"),
        }
    }

    fn acknowledgement_evidence() -> AcknowledgementEvidence {
        let identity = runtime();
        AcknowledgementEvidence {
            responder: identity.clone(),
            handoff_sha256: sha('b'),
            receipt: receipt(
                EvidenceKind::Acknowledgement,
                runtime_evidence_subjects(&identity),
                "snapshot-acknowledgement",
            ),
        }
    }

    fn completion_evidence() -> EvidenceRef {
        let identity = runtime();
        receipt(
            EvidenceKind::Completion,
            runtime_evidence_subjects(&identity),
            "snapshot-completion",
        )
    }

    fn empty_evidence_snapshot() -> TransitionEvidenceSnapshot {
        TransitionEvidenceSnapshot {
            candidate_evidence: None,
            adoption_evidence: None,
            delivery_evidence: None,
            acknowledgement_evidence: None,
            completion_evidence: None,
        }
    }

    fn evidence_before(phase: RotationPhase) -> TransitionEvidenceSnapshot {
        let mut snapshot = empty_evidence_snapshot();
        if matches!(
            phase,
            RotationPhase::Adoption
                | RotationPhase::InitialDelivery
                | RotationPhase::Acknowledgement
                | RotationPhase::Transfer
                | RotationPhase::Completion
        ) {
            snapshot.candidate_evidence = Some(candidate_evidence());
        }
        if matches!(
            phase,
            RotationPhase::InitialDelivery
                | RotationPhase::Acknowledgement
                | RotationPhase::Transfer
                | RotationPhase::Completion
        ) {
            snapshot.adoption_evidence = Some(adoption_evidence());
        }
        if matches!(
            phase,
            RotationPhase::Acknowledgement | RotationPhase::Transfer | RotationPhase::Completion
        ) {
            snapshot.delivery_evidence = Some(delivery_evidence());
        }
        if matches!(phase, RotationPhase::Transfer | RotationPhase::Completion) {
            snapshot.acknowledgement_evidence = Some(acknowledgement_evidence());
        }
        snapshot
    }

    fn transferred_authority() -> AuthoritySnapshot {
        AuthoritySnapshot {
            role_revision: RoleRevisionNumber::new(8).unwrap(),
            cutex_session_id: candidate().cutex_session_id,
            authority_epoch: AuthorityEpoch::new(2).unwrap(),
            source_durable_revision: candidate().durable_revision,
        }
    }

    fn phase_payload(phase: RotationPhase) -> PhasePayload {
        let identity = runtime();
        match phase {
            RotationPhase::Prepare => PhasePayload::Prepare {
                source_authority: authority(),
                handoff: handoff(),
            },
            RotationPhase::Candidate => PhasePayload::Candidate {
                candidate: candidate_evidence(),
            },
            RotationPhase::Adoption => PhasePayload::Adoption {
                candidate_session: candidate(),
                adoption: adoption_evidence(),
            },
            RotationPhase::InitialDelivery => PhasePayload::InitialDelivery {
                delivery: delivery_evidence(),
            },
            RotationPhase::Acknowledgement => PhasePayload::Acknowledgement {
                acknowledgement: acknowledgement_evidence(),
            },
            RotationPhase::Transfer => {
                let mut subjects = base_subjects();
                subjects.push(IdentityRef::CutexSession {
                    id: incumbent().cutex_session_id,
                });
                subjects.extend(runtime_subjects(&identity));
                PhasePayload::Transfer {
                    fresh_incumbent: incumbent(),
                    candidate_session: candidate(),
                    recipient: identity,
                    evidence: receipt(
                        EvidenceKind::TransferVerification,
                        subjects,
                        "snapshot-transfer",
                    ),
                }
            }
            RotationPhase::Completion => PhasePayload::Completion {
                transition_id: transition(),
                evidence: completion_evidence(),
            },
        }
    }

    fn unknown_reason(phase: RotationPhase) -> ReasonCode {
        match phase {
            RotationPhase::Prepare | RotationPhase::Candidate => {
                ReasonCode::PersistenceOutcomeUnknown
            }
            RotationPhase::Adoption => ReasonCode::AdoptionOutcomeUnknown,
            RotationPhase::InitialDelivery => ReasonCode::DeliveryOutcomeUnknown,
            RotationPhase::Acknowledgement => ReasonCode::AcknowledgementOutcomeUnknown,
            RotationPhase::Transfer => ReasonCode::TransferOutcomeUnknown,
            RotationPhase::Completion => ReasonCode::CompletionOutcomeUnknown,
        }
    }

    fn unknown_observation(phase: RotationPhase) -> UnknownOutcome {
        let identity = runtime();
        let prior_evidence = evidence_before(phase);
        let current_authority = if phase == RotationPhase::Completion {
            transferred_authority()
        } else {
            authority()
        };
        let revision_state = if phase == RotationPhase::Completion {
            RoleRevisionState::Current
        } else {
            RoleRevisionState::Candidate
        };
        UnknownOutcome {
            initialization_id: initialization(),
            transition_id: transition(),
            attempt: AttemptNumber::new(1).unwrap(),
            phase,
            prior: TransitionPriorSnapshot {
                transition_state: expected_prior_state(phase),
                revision_state,
                intended_predecessor: authority(),
                current_authority: current_authority.clone(),
                active_rotation: lock(),
                evidence: prior_evidence.clone(),
            },
            attempted_payload: phase_payload(phase),
            reason_code: unknown_reason(phase),
            evidence: receipt(
                EvidenceKind::UnknownObservation,
                runtime_evidence_subjects(&identity),
                &format!("unknown-{phase:?}").to_ascii_lowercase(),
            ),
            recorded_at: timestamp(),
            post_state: UnknownPostState {
                transition_state: TransitionState::Unknown,
                revision_state,
                current_authority,
                active_rotation: lock(),
                evidence: prior_evidence,
            },
            resolution: None,
        }
    }

    fn successful_resolution(unknown: &UnknownOutcome) -> UnknownResolution {
        let identity = runtime();
        let phase = unknown.phase;
        let current_authority =
            if matches!(phase, RotationPhase::Transfer | RotationPhase::Completion) {
                transferred_authority()
            } else {
                unknown.prior.current_authority.clone()
            };
        let revision_state = if matches!(phase, RotationPhase::Transfer | RotationPhase::Completion)
        {
            RoleRevisionState::Current
        } else {
            unknown.prior.revision_state
        };
        UnknownResolution {
            outcome: ResolutionOutcome::PhaseSucceeded {
                verified_payload: unknown.attempted_payload.clone(),
                post_state: PhaseSuccessPostState {
                    transition_state: expected_success_state(phase),
                    revision_state,
                    current_authority,
                    active_rotation: if phase == RotationPhase::Completion {
                        None
                    } else {
                        Some(lock())
                    },
                    evidence: expected_success_evidence(
                        &unknown.prior.evidence,
                        &unknown.attempted_payload,
                    ),
                },
            },
            evidence: receipt(
                EvidenceKind::UnknownResolution,
                runtime_evidence_subjects(&identity),
                "resolution",
            ),
            recorded_at: timestamp(),
        }
    }

    fn terminal_resolution(
        unknown: &UnknownOutcome,
        outcome: TerminalOutcome,
    ) -> UnknownResolution {
        let identity = runtime();
        let (kind, reason, transition_state, revision_state) = match outcome {
            TerminalOutcome::Failed => (
                EvidenceKind::Failure,
                ReasonCode::ExternalFailure,
                TransitionState::Failed,
                RoleRevisionState::Failed,
            ),
            TerminalOutcome::Cancelled => (
                EvidenceKind::Cancellation,
                ReasonCode::HumanCancelled,
                TransitionState::Cancelled,
                RoleRevisionState::Cancelled,
            ),
        };
        let attempt = FailedAttempt {
            attempt: unknown.attempt,
            outcome,
            phase: unknown.phase,
            reason_code: reason,
            evidence: receipt(
                kind,
                runtime_evidence_subjects(&identity),
                "unknown-terminal",
            ),
            recorded_at: timestamp(),
        };
        let post_state = TerminalPostState {
            transition_state,
            revision_state,
            current_authority: unknown.prior.current_authority.clone(),
            active_rotation: None,
            evidence: unknown.prior.evidence.clone(),
        };
        UnknownResolution {
            outcome: match outcome {
                TerminalOutcome::Failed => ResolutionOutcome::Failed {
                    attempt,
                    post_state,
                },
                TerminalOutcome::Cancelled => ResolutionOutcome::Cancelled {
                    attempt,
                    post_state,
                },
            },
            evidence: receipt(
                EvidenceKind::UnknownResolution,
                runtime_evidence_subjects(&identity),
                "unknown-terminal-resolution",
            ),
            recorded_at: timestamp(),
        }
    }

    fn resolution_intent(resolution: &UnknownResolution) -> ResolutionIntent {
        match &resolution.outcome {
            ResolutionOutcome::PhaseSucceeded {
                verified_payload, ..
            } => ResolutionIntent::PhaseSucceeded {
                verified_payload: verified_payload.clone(),
            },
            ResolutionOutcome::Failed { attempt, .. } => ResolutionIntent::Failed {
                reason_code: attempt.reason_code,
                evidence: attempt.evidence.clone(),
            },
            ResolutionOutcome::Cancelled { attempt, .. } => ResolutionIntent::Cancelled {
                reason_code: attempt.reason_code,
                evidence: attempt.evidence.clone(),
            },
        }
    }

    fn terminal_request(outcome: TerminalOutcome) -> TerminalRequest {
        let identity = runtime();
        let (kind, reason) = match outcome {
            TerminalOutcome::Failed => (EvidenceKind::Failure, ReasonCode::ExternalFailure),
            TerminalOutcome::Cancelled => (EvidenceKind::Cancellation, ReasonCode::HumanCancelled),
        };
        TerminalRequest {
            context: context(),
            adopted_identity: identity.clone(),
            attempt: AttemptNumber::new(1).unwrap(),
            phase: RotationPhase::Adoption,
            reason_code: reason,
            evidence: receipt(kind, runtime_evidence_subjects(&identity), "terminal"),
        }
    }

    fn terminal_attempt(request: &TerminalRequest, outcome: TerminalOutcome) -> FailedAttempt {
        FailedAttempt {
            attempt: request.attempt,
            outcome,
            phase: request.phase,
            reason_code: request.reason_code,
            evidence: request.evidence.clone(),
            recorded_at: timestamp(),
        }
    }

    fn envelope(request: MutationRequest) -> RequestEnvelope {
        RequestEnvelope {
            schema: RequestSchema::V1,
            request_id: request_id(request.operation()),
            request_digest_sha256: sha('d'),
            expected_store_revision: StoreRevision::new(1).unwrap(),
            request,
        }
    }

    fn response(request: &RequestEnvelope, result: MutationResult) -> MutationResponse {
        MutationResponse {
            schema: ResultSchema::V1,
            request_id: request.request_id.clone(),
            operation: request.request.operation(),
            project_id: project(),
            role_family_id: family(),
            initialization_id: initialization(),
            disposition: ResultDisposition::Applied,
            committed_store_revision: StoreRevision::new(2).unwrap(),
            result,
        }
    }

    fn valid_pairs() -> Vec<(RequestEnvelope, MutationResponse)> {
        let incumbent = incumbent();
        let initialize = envelope(MutationRequest::InitializeFamily(InitializeFamilyRequest {
            project_id: project(),
            role_family_id: family(),
            role_key: RoleKey::new("runtime").unwrap(),
            initialization_id: initialization(),
            chosen_root_revision: RoleRevisionNumber::new(7).unwrap(),
            incumbent: incumbent.clone(),
            human_approval_id: HumanApprovalId::new("approval-a").unwrap(),
            approval_evidence: receipt(
                EvidenceKind::HumanApproval,
                vec![
                    IdentityRef::HumanApproval {
                        id: HumanApprovalId::new("approval-a").unwrap(),
                    },
                    IdentityRef::Project { id: project() },
                    IdentityRef::RoleFamily { id: family() },
                    IdentityRef::CutexSession {
                        id: incumbent.cutex_session_id.clone(),
                    },
                ],
                "approval",
            ),
            initialization_evidence: receipt(
                EvidenceKind::RootInitialization,
                vec![
                    IdentityRef::Project { id: project() },
                    IdentityRef::RoleFamily { id: family() },
                    IdentityRef::CutexSession {
                        id: incumbent.cutex_session_id.clone(),
                    },
                ],
                "initialization",
            ),
            effective_at: EvidenceTime::Known {
                rfc3339: timestamp(),
            },
        }));
        let initialize_response = response(
            &initialize,
            MutationResult::InitializeFamily {
                role_family_id: family(),
                root_revision: RoleRevisionNumber::new(7).unwrap(),
                authority_epoch: AuthorityEpoch::new(1).unwrap(),
            },
        );

        let source = authority();
        let handoff = handoff();
        let prepare = envelope(MutationRequest::PrepareRotation(PrepareRotationRequest {
            project_id: project(),
            role_family_id: family(),
            initialization_id: initialization(),
            transition_id: transition(),
            source_authority: source.clone(),
            allocator: FamilyAllocatorObservation {
                project_id: project(),
                role_family_id: family(),
                initialization_id: initialization(),
                observed_store_revision: StoreRevision::new(1).unwrap(),
                next_role_revision: RoleRevisionNumber::new(8).unwrap(),
            },
            human_approval_id: HumanApprovalId::new("approval-a").unwrap(),
            approval_evidence: receipt(
                EvidenceKind::HumanApproval,
                vec![
                    IdentityRef::HumanApproval {
                        id: HumanApprovalId::new("approval-a").unwrap(),
                    },
                    IdentityRef::Project { id: project() },
                    IdentityRef::RoleFamily { id: family() },
                    IdentityRef::CutexSession {
                        id: source.cutex_session_id.clone(),
                    },
                    IdentityRef::Task {
                        id: handoff.task_id.clone(),
                    },
                ],
                "prepare-approval",
            ),
            handoff,
        }));
        let prepare_response = response(
            &prepare,
            MutationResult::PrepareRotation {
                transition_id: transition(),
                candidate_revision: RoleRevisionNumber::new(8).unwrap(),
                source_authority_epoch: AuthorityEpoch::new(1).unwrap(),
            },
        );

        let candidate_session = candidate();
        let mut candidate_subjects = base_subjects();
        candidate_subjects.push(IdentityRef::CutexSession {
            id: candidate_session.cutex_session_id.clone(),
        });
        let record_candidate = envelope(MutationRequest::RecordCandidate(RecordCandidateRequest {
            context: context(),
            successor: candidate_session.clone(),
            evidence: receipt(
                EvidenceKind::CandidateCreation,
                candidate_subjects,
                "candidate",
            ),
        }));
        let record_candidate_response = response(
            &record_candidate,
            MutationResult::RecordCandidate {
                transition_id: transition(),
                candidate_revision: RoleRevisionNumber::new(8).unwrap(),
                session: candidate_session.clone(),
            },
        );

        let identity = runtime();
        let adoption = envelope(MutationRequest::RecordAdoption(RecordAdoptionRequest {
            context: context(),
            candidate_session: candidate_session.clone(),
            identity: identity.clone(),
            evidence: receipt(
                EvidenceKind::Adoption,
                runtime_evidence_subjects(&identity),
                "adoption",
            ),
        }));
        let adoption_response = response(
            &adoption,
            MutationResult::RecordAdoption {
                transition_id: transition(),
                identity: identity.clone(),
            },
        );

        let delivery_id = DeliveryId::new("transition-a/initial").unwrap();
        let mut delivery_subjects = base_subjects();
        delivery_subjects.push(IdentityRef::Delivery {
            id: delivery_id.clone(),
        });
        delivery_subjects.extend(runtime_subjects(&identity));
        let delivery = envelope(MutationRequest::RecordInitialDelivery(
            RecordInitialDeliveryRequest {
                context: context(),
                delivery_id: delivery_id.clone(),
                recipient: identity.clone(),
                evidence: receipt(EvidenceKind::InitialDelivery, delivery_subjects, "delivery"),
            },
        ));
        let delivery_response = response(
            &delivery,
            MutationResult::RecordInitialDelivery {
                transition_id: transition(),
                delivery_id: delivery_id.clone(),
            },
        );

        let acknowledgement = envelope(MutationRequest::RecordAcknowledgement(
            RecordAcknowledgementRequest {
                context: context(),
                responder: identity.clone(),
                handoff_sha256: sha('b'),
                evidence: receipt(
                    EvidenceKind::Acknowledgement,
                    runtime_evidence_subjects(&identity),
                    "acknowledgement",
                ),
            },
        ));
        let acknowledgement_response = response(
            &acknowledgement,
            MutationResult::RecordAcknowledgement {
                transition_id: transition(),
                handoff_sha256: sha('b'),
            },
        );

        let mut transfer_subjects = base_subjects();
        transfer_subjects.push(IdentityRef::CutexSession {
            id: incumbent.cutex_session_id.clone(),
        });
        transfer_subjects.extend(runtime_subjects(&identity));
        let transfer = envelope(MutationRequest::TransferAuthority(
            TransferAuthorityRequest {
                context: context(),
                fresh_incumbent: incumbent.clone(),
                candidate_session: candidate_session.clone(),
                adopted_identity: identity.clone(),
                expected_authority_epoch: AuthorityEpoch::new(1).unwrap(),
                evidence: receipt(
                    EvidenceKind::TransferVerification,
                    transfer_subjects,
                    "transfer",
                ),
            },
        ));
        let transfer_response = response(
            &transfer,
            MutationResult::TransferAuthority {
                transition_id: transition(),
                role_revision: RoleRevisionNumber::new(8).unwrap(),
                cutex_session_id: candidate_session.cutex_session_id.clone(),
                authority_epoch: AuthorityEpoch::new(2).unwrap(),
            },
        );

        let completion = envelope(MutationRequest::CompleteRotation(CompleteRotationRequest {
            context: context(),
            adopted_identity: identity.clone(),
            evidence: receipt(
                EvidenceKind::Completion,
                runtime_evidence_subjects(&identity),
                "completion",
            ),
        }));
        let completion_response = response(
            &completion,
            MutationResult::CompleteRotation {
                transition_id: transition(),
                role_revision: RoleRevisionNumber::new(8).unwrap(),
            },
        );

        let failure_input = terminal_request(TerminalOutcome::Failed);
        let failure_attempt = terminal_attempt(&failure_input, TerminalOutcome::Failed);
        let failure = envelope(MutationRequest::FailRotation(failure_input));
        let failure_response = response(
            &failure,
            MutationResult::FailRotation {
                transition_id: transition(),
                attempt: failure_attempt,
            },
        );

        let cancellation_input = terminal_request(TerminalOutcome::Cancelled);
        let cancellation_attempt =
            terminal_attempt(&cancellation_input, TerminalOutcome::Cancelled);
        let cancellation = envelope(MutationRequest::CancelRotation(cancellation_input));
        let cancellation_response = response(
            &cancellation,
            MutationResult::CancelRotation {
                transition_id: transition(),
                attempt: cancellation_attempt,
            },
        );

        let unknown = unknown_observation(RotationPhase::Adoption);
        let record_unknown = envelope(MutationRequest::RecordUnknown(RecordUnknownRequest {
            context: context(),
            adopted_identity: identity.clone(),
            attempt: unknown.attempt,
            phase: unknown.phase,
            attempted_payload: unknown.attempted_payload.clone(),
            reason_code: unknown.reason_code,
            evidence: unknown.evidence.clone(),
        }));
        let record_unknown_response = response(
            &record_unknown,
            MutationResult::RecordUnknown {
                transition_id: transition(),
                unknown: unknown.clone(),
            },
        );

        let resolution = successful_resolution(&unknown);
        let mut resolved_unknown = unknown.clone();
        resolved_unknown.resolution = Some(resolution.clone());
        let resolve_unknown = envelope(MutationRequest::ResolveUnknown(ResolveUnknownRequest {
            context: context(),
            adopted_identity: identity,
            attempt: unknown.attempt,
            outcome: ResolutionIntent::PhaseSucceeded {
                verified_payload: unknown.attempted_payload,
            },
            evidence: resolution.evidence,
        }));
        let resolve_unknown_response = response(
            &resolve_unknown,
            MutationResult::ResolveUnknown {
                transition_id: transition(),
                unknown: resolved_unknown,
            },
        );

        vec![
            (initialize, initialize_response),
            (prepare, prepare_response),
            (record_candidate, record_candidate_response),
            (adoption, adoption_response),
            (delivery, delivery_response),
            (acknowledgement, acknowledgement_response),
            (transfer, transfer_response),
            (completion, completion_response),
            (failure, failure_response),
            (cancellation, cancellation_response),
            (record_unknown, record_unknown_response),
            (resolve_unknown, resolve_unknown_response),
        ]
    }

    fn full_store() -> RoleSeatStore {
        let incumbent = incumbent();
        let identity = runtime();
        let predecessor = authority();
        let mut root_subjects = vec![
            IdentityRef::Project { id: project() },
            IdentityRef::RoleFamily { id: family() },
        ];
        root_subjects.push(IdentityRef::CutexSession {
            id: incumbent.cutex_session_id.clone(),
        });
        let root = RootInitialization {
            initialization_id: initialization(),
            chosen_root_revision: RoleRevisionNumber::new(7).unwrap(),
            incumbent: incumbent.clone(),
            approval_evidence: receipt(
                EvidenceKind::HumanApproval,
                vec![
                    IdentityRef::HumanApproval {
                        id: HumanApprovalId::new("approval-a").unwrap(),
                    },
                    IdentityRef::Project { id: project() },
                    IdentityRef::RoleFamily { id: family() },
                    IdentityRef::CutexSession {
                        id: incumbent.cutex_session_id.clone(),
                    },
                ],
                "store-approval",
            ),
            initialization_evidence: receipt(
                EvidenceKind::RootInitialization,
                root_subjects,
                "store-initialization",
            ),
            effective_at: EvidenceTime::Known {
                rfc3339: timestamp(),
            },
            recorded_at: timestamp(),
        };

        let mut revisions = BTreeMap::new();
        revisions.insert(
            RoleRevisionNumber::new(7).unwrap(),
            RoleRevision {
                role_revision: RoleRevisionNumber::new(7).unwrap(),
                session: Some(incumbent.clone()),
                state: RoleRevisionState::Superseded,
                intended_predecessor: None,
                successful_predecessor: None,
                root_revision: Some(RoleRevisionNumber::new(7).unwrap()),
                allocated_at: timestamp(),
                terminal_attempt: None,
            },
        );
        revisions.insert(
            RoleRevisionNumber::new(8).unwrap(),
            RoleRevision {
                role_revision: RoleRevisionNumber::new(8).unwrap(),
                session: Some(candidate()),
                state: RoleRevisionState::Current,
                intended_predecessor: Some(predecessor.clone()),
                successful_predecessor: Some(SuccessfulPredecessor {
                    role_revision: RoleRevisionNumber::new(7).unwrap(),
                    cutex_session_id: incumbent.cutex_session_id.clone(),
                    transfer_transition_id: transition(),
                }),
                root_revision: Some(RoleRevisionNumber::new(7).unwrap()),
                allocated_at: timestamp(),
                terminal_attempt: None,
            },
        );

        let mut unknown = unknown_observation(RotationPhase::Adoption);
        unknown.resolution = Some(successful_resolution(&unknown));
        let mut transitions = BTreeMap::new();
        transitions.insert(
            transition(),
            RoleTransition {
                transition_id: transition(),
                candidate_revision: RoleRevisionNumber::new(8).unwrap(),
                intended_predecessor: predecessor,
                approval_evidence: receipt(
                    EvidenceKind::HumanApproval,
                    vec![
                        IdentityRef::HumanApproval {
                            id: HumanApprovalId::new("approval-a").unwrap(),
                        },
                        IdentityRef::Project { id: project() },
                        IdentityRef::RoleFamily { id: family() },
                        IdentityRef::CutexSession {
                            id: incumbent.cutex_session_id.clone(),
                        },
                        IdentityRef::Task {
                            id: TaskId::new("task-a").unwrap(),
                        },
                    ],
                    "store-prepare",
                ),
                handoff: handoff(),
                state: TransitionState::AuthorityTransferred,
                candidate_evidence: Some(CandidateEvidence {
                    session: candidate(),
                    receipt: receipt(
                        EvidenceKind::CandidateCreation,
                        {
                            let mut subjects = base_subjects();
                            subjects.push(IdentityRef::CutexSession {
                                id: candidate().cutex_session_id,
                            });
                            subjects
                        },
                        "store-candidate",
                    ),
                }),
                adoption_evidence: Some(AdoptionEvidence {
                    identity: identity.clone(),
                    receipt: receipt(
                        EvidenceKind::Adoption,
                        runtime_evidence_subjects(&identity),
                        "store-adoption",
                    ),
                }),
                delivery_evidence: Some(DeliveryEvidence {
                    delivery_id: DeliveryId::new("transition-a/initial").unwrap(),
                    recipient: identity.clone(),
                    receipt: receipt(
                        EvidenceKind::InitialDelivery,
                        {
                            let mut subjects = base_subjects();
                            subjects.push(IdentityRef::Delivery {
                                id: DeliveryId::new("transition-a/initial").unwrap(),
                            });
                            subjects.extend(runtime_subjects(&identity));
                            subjects
                        },
                        "store-delivery",
                    ),
                }),
                acknowledgement_evidence: Some(AcknowledgementEvidence {
                    responder: identity.clone(),
                    handoff_sha256: sha('b'),
                    receipt: receipt(
                        EvidenceKind::Acknowledgement,
                        runtime_evidence_subjects(&identity),
                        "store-acknowledgement",
                    ),
                }),
                completion_evidence: None,
                unknown_outcomes: vec![unknown],
                terminal_attempt: None,
                created_at: timestamp(),
                updated_at: timestamp(),
            },
        );

        let transfer_result = MutationResult::TransferAuthority {
            transition_id: transition(),
            role_revision: RoleRevisionNumber::new(8).unwrap(),
            cutex_session_id: candidate().cutex_session_id,
            authority_epoch: AuthorityEpoch::new(2).unwrap(),
        };
        let mut idempotency = BTreeMap::new();
        idempotency.insert(
            RequestId::new("request-transfer").unwrap(),
            IdempotencyRecord {
                operation: Operation::TransferAuthority,
                project_id: project(),
                role_family_id: family(),
                initialization_id: initialization(),
                request_digest_sha256: sha('d'),
                committed_store_revision: StoreRevision::new(2).unwrap(),
                result: transfer_result,
            },
        );

        RoleSeatStore {
            schema: StoreSchema::V1,
            store_revision: StoreRevision::new(2).unwrap(),
            family: Some(RoleFamily {
                role_family_id: family(),
                project_id: project(),
                role_key: RoleKey::new("runtime").unwrap(),
                root_initialization: root,
                next_role_revision: RoleRevisionNumber::new(9).unwrap(),
                current_authority: CurrentAuthority {
                    role_revision: RoleRevisionNumber::new(8).unwrap(),
                    cutex_session_id: candidate().cutex_session_id,
                    authority_epoch: AuthorityEpoch::new(2).unwrap(),
                    effective_at: EvidenceTime::Known {
                        rfc3339: timestamp(),
                    },
                    established_by: EstablishedBy::Transfer {
                        transition_id: transition(),
                    },
                },
                active_rotation: Some(lock()),
                revisions,
                transitions,
            }),
            idempotency,
        }
    }

    fn root_only_store() -> RoleSeatStore {
        let mut store = full_store();
        store.store_revision = StoreRevision::new(1).unwrap();
        store.idempotency.clear();
        let family = store.family.as_mut().unwrap();
        family.active_rotation = None;
        family.transitions.clear();
        family.revisions.retain(|revision, _| revision.get() == 7);
        let root = family
            .revisions
            .get_mut(&RoleRevisionNumber::new(7).unwrap())
            .unwrap();
        root.state = RoleRevisionState::InitializedCurrent;
        family.next_role_revision = RoleRevisionNumber::new(8).unwrap();
        family.current_authority = CurrentAuthority {
            role_revision: RoleRevisionNumber::new(7).unwrap(),
            cutex_session_id: incumbent().cutex_session_id,
            authority_epoch: AuthorityEpoch::new(1).unwrap(),
            effective_at: family.root_initialization.effective_at.clone(),
            established_by: EstablishedBy::RootInitialization {
                initialization_id: initialization(),
            },
        };
        store
    }

    fn runtime_named(name: &str, durable_revision: u64) -> (DurableSessionRef, RuntimeIdentity) {
        let session = DurableSessionRef {
            cutex_session_id: CutexSessionId::new(format!("cutex-{name}")).unwrap(),
            durable_revision: DurableRevision::new(durable_revision).unwrap(),
        };
        let identity = RuntimeIdentity {
            cutex_session_id: session.cutex_session_id.clone(),
            cute_codex_session_id: CuteCodexSessionId::new(format!("cute-{name}")).unwrap(),
            runtime_agent_id: RuntimeAgentId::new(format!("runtime-{name}")).unwrap(),
            runtime_generation: RuntimeGeneration::new(1).unwrap(),
        };
        (session, identity)
    }

    fn handoff_for(task: &str, identity: &RuntimeIdentity) -> HandoffRef {
        let task_id = TaskId::new(task).unwrap();
        let mut subjects = vec![
            IdentityRef::Project { id: project() },
            IdentityRef::RoleFamily { id: family() },
            IdentityRef::Task {
                id: task_id.clone(),
            },
        ];
        subjects.extend(runtime_subjects(identity));
        HandoffRef {
            task_id,
            task_revision: TaskRevision::new(1).unwrap(),
            handoff_sha256: sha('b'),
            recipient: identity.clone(),
            acceptance_receipt: receipt(EvidenceKind::HandoffAcceptance, subjects, task),
        }
    }

    fn context_for(
        transition_id: &str,
        candidate_revision: u64,
        predecessor: AuthoritySnapshot,
        identity: &RuntimeIdentity,
    ) -> TransitionContext {
        TransitionContext {
            project_id: project(),
            role_family_id: family(),
            initialization_id: initialization(),
            transition_id: TransitionId::new(transition_id).unwrap(),
            candidate_revision: RoleRevisionNumber::new(candidate_revision).unwrap(),
            intended_predecessor: predecessor,
            handoff: handoff_for(&format!("task-{transition_id}"), identity),
        }
    }

    fn candidate_for(
        context: &TransitionContext,
        session: &DurableSessionRef,
    ) -> CandidateEvidence {
        let mut subjects = transition_subjects(context);
        subjects.push(IdentityRef::CutexSession {
            id: session.cutex_session_id.clone(),
        });
        CandidateEvidence {
            session: session.clone(),
            receipt: receipt(
                EvidenceKind::CandidateCreation,
                subjects,
                context.transition_id.as_str(),
            ),
        }
    }

    fn adoption_for(context: &TransitionContext, identity: &RuntimeIdentity) -> AdoptionEvidence {
        let mut subjects = transition_subjects(context);
        subjects.extend(runtime_subjects(identity));
        AdoptionEvidence {
            identity: identity.clone(),
            receipt: receipt(
                EvidenceKind::Adoption,
                subjects,
                context.transition_id.as_str(),
            ),
        }
    }

    fn delivery_for(context: &TransitionContext, identity: &RuntimeIdentity) -> DeliveryEvidence {
        let delivery_id =
            DeliveryId::new(format!("{}/initial", context.transition_id.as_str())).unwrap();
        let mut subjects = transition_subjects(context);
        subjects.push(IdentityRef::Delivery {
            id: delivery_id.clone(),
        });
        subjects.extend(runtime_subjects(identity));
        DeliveryEvidence {
            delivery_id,
            recipient: identity.clone(),
            receipt: receipt(
                EvidenceKind::InitialDelivery,
                subjects,
                context.transition_id.as_str(),
            ),
        }
    }

    fn acknowledgement_for(
        context: &TransitionContext,
        identity: &RuntimeIdentity,
    ) -> AcknowledgementEvidence {
        let mut subjects = transition_subjects(context);
        subjects.extend(runtime_subjects(identity));
        AcknowledgementEvidence {
            responder: identity.clone(),
            handoff_sha256: context.handoff.handoff_sha256.clone(),
            receipt: receipt(
                EvidenceKind::Acknowledgement,
                subjects,
                context.transition_id.as_str(),
            ),
        }
    }

    fn completion_for(context: &TransitionContext, identity: &RuntimeIdentity) -> EvidenceRef {
        let mut subjects = transition_subjects(context);
        subjects.extend(runtime_subjects(identity));
        receipt(
            EvidenceKind::Completion,
            subjects,
            context.transition_id.as_str(),
        )
    }

    fn terminal_for(
        context: &TransitionContext,
        identity: &RuntimeIdentity,
        outcome: TerminalOutcome,
        phase: RotationPhase,
    ) -> FailedAttempt {
        let (kind, reason) = match outcome {
            TerminalOutcome::Failed => (EvidenceKind::Failure, ReasonCode::ExternalFailure),
            TerminalOutcome::Cancelled => (EvidenceKind::Cancellation, ReasonCode::HumanCancelled),
        };
        let mut subjects = transition_subjects(context);
        subjects.extend(runtime_subjects(identity));
        FailedAttempt {
            attempt: AttemptNumber::new(1).unwrap(),
            outcome,
            phase,
            reason_code: reason,
            evidence: receipt(kind, subjects, context.transition_id.as_str()),
            recorded_at: timestamp(),
        }
    }

    fn transition_fixture(
        context: &TransitionContext,
        session: &DurableSessionRef,
        identity: &RuntimeIdentity,
        state: TransitionState,
        terminal_phase: Option<RotationPhase>,
    ) -> RoleTransition {
        let candidate = candidate_for(context, session);
        let adoption = adoption_for(context, identity);
        let delivery = delivery_for(context, identity);
        let acknowledgement = acknowledgement_for(context, identity);
        let terminal_outcome = match state {
            TransitionState::Failed => Some(TerminalOutcome::Failed),
            TransitionState::Cancelled => Some(TerminalOutcome::Cancelled),
            _ => None,
        };
        let shape = expected_transition_evidence_shape(state, terminal_phase).unwrap();
        let Some(IdentityRef::HumanApproval { id }) = full_store()
            .family
            .unwrap()
            .transitions
            .get(&transition())
            .unwrap()
            .approval_evidence
            .subjects
            .first()
            .cloned()
        else {
            unreachable!();
        };
        RoleTransition {
            transition_id: context.transition_id.clone(),
            candidate_revision: context.candidate_revision,
            intended_predecessor: context.intended_predecessor.clone(),
            approval_evidence: receipt(
                EvidenceKind::HumanApproval,
                vec![
                    IdentityRef::HumanApproval { id },
                    IdentityRef::Project { id: project() },
                    IdentityRef::RoleFamily { id: family() },
                    IdentityRef::CutexSession {
                        id: context.intended_predecessor.cutex_session_id.clone(),
                    },
                    IdentityRef::Task {
                        id: context.handoff.task_id.clone(),
                    },
                ],
                context.transition_id.as_str(),
            ),
            handoff: context.handoff.clone(),
            state,
            candidate_evidence: shape.0.then_some(candidate),
            adoption_evidence: shape.1.then_some(adoption),
            delivery_evidence: shape.2.then_some(delivery),
            acknowledgement_evidence: shape.3.then_some(acknowledgement),
            completion_evidence: shape.4.then(|| completion_for(context, identity)),
            unknown_outcomes: Vec::new(),
            terminal_attempt: terminal_outcome
                .map(|outcome| terminal_for(context, identity, outcome, terminal_phase.unwrap())),
            created_at: timestamp(),
            updated_at: timestamp(),
        }
    }

    fn append_transition(
        store: &mut RoleSeatStore,
        transition_id: &str,
        candidate_revision: u64,
        predecessor: AuthoritySnapshot,
        state: TransitionState,
        terminal_phase: Option<RotationPhase>,
        name: &str,
    ) -> (TransitionContext, DurableSessionRef, RuntimeIdentity) {
        let (session, identity) = runtime_named(name, 1);
        let context = context_for(
            transition_id,
            candidate_revision,
            predecessor.clone(),
            &identity,
        );
        let transition = transition_fixture(&context, &session, &identity, state, terminal_phase);
        let successful = matches!(
            state,
            TransitionState::AuthorityTransferred | TransitionState::Completed
        );
        let terminal_attempt = transition.terminal_attempt.clone();
        let family = store.family.as_mut().unwrap();
        family.revisions.insert(
            context.candidate_revision,
            RoleRevision {
                role_revision: context.candidate_revision,
                session: transition
                    .candidate_evidence
                    .as_ref()
                    .map(|candidate| candidate.session.clone()),
                state: match state {
                    TransitionState::Failed => RoleRevisionState::Failed,
                    TransitionState::Cancelled => RoleRevisionState::Cancelled,
                    _ if successful => RoleRevisionState::Current,
                    _ => RoleRevisionState::Candidate,
                },
                intended_predecessor: Some(predecessor.clone()),
                successful_predecessor: successful.then(|| SuccessfulPredecessor {
                    role_revision: predecessor.role_revision,
                    cutex_session_id: predecessor.cutex_session_id.clone(),
                    transfer_transition_id: context.transition_id.clone(),
                }),
                root_revision: successful
                    .then_some(family.root_initialization.chosen_root_revision),
                allocated_at: timestamp(),
                terminal_attempt,
            },
        );
        family
            .transitions
            .insert(context.transition_id.clone(), transition);
        family.next_role_revision = context.candidate_revision.checked_next().unwrap();
        (context, session, identity)
    }

    fn active_store(state: TransitionState) -> RoleSeatStore {
        let mut store = root_only_store();
        store.store_revision = StoreRevision::new(2).unwrap();
        let predecessor = authority();
        append_transition(
            &mut store,
            "transition-a",
            8,
            predecessor,
            state,
            None,
            "candidate",
        );
        let family = store.family.as_mut().unwrap();
        family.active_rotation = Some(lock());
        if state == TransitionState::AuthorityTransferred {
            family
                .revisions
                .get_mut(&RoleRevisionNumber::new(7).unwrap())
                .unwrap()
                .state = RoleRevisionState::Superseded;
            family.current_authority = CurrentAuthority {
                role_revision: RoleRevisionNumber::new(8).unwrap(),
                cutex_session_id: candidate().cutex_session_id,
                authority_epoch: AuthorityEpoch::new(2).unwrap(),
                effective_at: EvidenceTime::Known {
                    rfc3339: timestamp(),
                },
                established_by: EstablishedBy::Transfer {
                    transition_id: transition(),
                },
            };
        }
        store
    }

    fn two_completed_successes_store() -> RoleSeatStore {
        let mut store = active_store(TransitionState::AuthorityTransferred);
        {
            let family = store.family.as_mut().unwrap();
            let completion = {
                let first = family.transitions.get(&transition()).unwrap();
                completion_for(
                    &transition_context_for(family, first),
                    &first.handoff.recipient,
                )
            };
            let first = family.transitions.get_mut(&transition()).unwrap();
            first.state = TransitionState::Completed;
            first.completion_evidence = Some(completion);
            family.active_rotation = None;
        }
        let predecessor = AuthoritySnapshot {
            role_revision: RoleRevisionNumber::new(8).unwrap(),
            cutex_session_id: candidate().cutex_session_id,
            authority_epoch: AuthorityEpoch::new(2).unwrap(),
            source_durable_revision: candidate().durable_revision,
        };
        let (context, session, _) = append_transition(
            &mut store,
            "transition-b",
            9,
            predecessor,
            TransitionState::Completed,
            None,
            "candidate-b",
        );
        let family = store.family.as_mut().unwrap();
        family
            .revisions
            .get_mut(&RoleRevisionNumber::new(8).unwrap())
            .unwrap()
            .state = RoleRevisionState::Superseded;
        family.current_authority = CurrentAuthority {
            role_revision: RoleRevisionNumber::new(9).unwrap(),
            cutex_session_id: session.cutex_session_id,
            authority_epoch: AuthorityEpoch::new(3).unwrap(),
            effective_at: EvidenceTime::Known {
                rfc3339: timestamp(),
            },
            established_by: EstablishedBy::Transfer {
                transition_id: context.transition_id,
            },
        };
        family.active_rotation = None;
        store.store_revision = StoreRevision::new(3).unwrap();
        store.idempotency.clear();
        store
    }

    fn consumed_gaps_then_success_store() -> RoleSeatStore {
        let mut store = root_only_store();
        append_transition(
            &mut store,
            "transition-failed",
            8,
            authority(),
            TransitionState::Failed,
            Some(RotationPhase::Adoption),
            "failed",
        );
        append_transition(
            &mut store,
            "transition-cancelled",
            9,
            authority(),
            TransitionState::Cancelled,
            Some(RotationPhase::Candidate),
            "cancelled",
        );
        let (context, session, _) = append_transition(
            &mut store,
            "transition-success",
            10,
            authority(),
            TransitionState::Completed,
            None,
            "success",
        );
        let family = store.family.as_mut().unwrap();
        family
            .revisions
            .get_mut(&RoleRevisionNumber::new(7).unwrap())
            .unwrap()
            .state = RoleRevisionState::Superseded;
        family.current_authority = CurrentAuthority {
            role_revision: RoleRevisionNumber::new(10).unwrap(),
            cutex_session_id: session.cutex_session_id,
            authority_epoch: AuthorityEpoch::new(2).unwrap(),
            effective_at: EvidenceTime::Known {
                rfc3339: timestamp(),
            },
            established_by: EstablishedBy::Transfer {
                transition_id: context.transition_id,
            },
        };
        family.active_rotation = None;
        store.store_revision = StoreRevision::new(4).unwrap();
        store.idempotency.clear();
        store
    }

    fn idempotent_transfer_store() -> RoleSeatStore {
        let mut store = active_store(TransitionState::AuthorityTransferred);
        store.idempotency.insert(
            RequestId::new("request-transfer").unwrap(),
            IdempotencyRecord {
                operation: Operation::TransferAuthority,
                project_id: project(),
                role_family_id: family(),
                initialization_id: initialization(),
                request_digest_sha256: sha('d'),
                committed_store_revision: StoreRevision::new(2).unwrap(),
                result: MutationResult::TransferAuthority {
                    transition_id: transition(),
                    role_revision: RoleRevisionNumber::new(8).unwrap(),
                    cutex_session_id: candidate().cutex_session_id,
                    authority_epoch: AuthorityEpoch::new(2).unwrap(),
                },
            },
        );
        store
    }

    fn unknown_store(
        phase: RotationPhase,
        resolution: Option<Option<TerminalOutcome>>,
    ) -> RoleSeatStore {
        let mut store = root_only_store();
        store.store_revision = StoreRevision::new(2).unwrap();
        let mut unknown = unknown_observation(phase);
        if let Some(outcome) = resolution {
            unknown.resolution = Some(match outcome {
                None => successful_resolution(&unknown),
                Some(outcome) => terminal_resolution(&unknown, outcome),
            });
        }
        let template = full_store()
            .family
            .unwrap()
            .transitions
            .into_values()
            .next()
            .unwrap();
        let mut transition_record = RoleTransition {
            transition_id: transition(),
            candidate_revision: RoleRevisionNumber::new(8).unwrap(),
            intended_predecessor: authority(),
            approval_evidence: template.approval_evidence,
            handoff: handoff(),
            state: TransitionState::Unknown,
            candidate_evidence: unknown.post_state.evidence.candidate_evidence.clone(),
            adoption_evidence: unknown.post_state.evidence.adoption_evidence.clone(),
            delivery_evidence: unknown.post_state.evidence.delivery_evidence.clone(),
            acknowledgement_evidence: unknown.post_state.evidence.acknowledgement_evidence.clone(),
            completion_evidence: unknown.post_state.evidence.completion_evidence.clone(),
            unknown_outcomes: vec![unknown.clone()],
            terminal_attempt: None,
            created_at: timestamp(),
            updated_at: timestamp(),
        };

        let mut revision_state = unknown.post_state.revision_state;
        let mut active_rotation = Some(lock());
        let mut successful = phase == RotationPhase::Completion && resolution.is_none();
        if let Some(resolution) = &unknown.resolution {
            match &resolution.outcome {
                ResolutionOutcome::PhaseSucceeded { post_state, .. } => {
                    transition_record.state = post_state.transition_state;
                    revision_state = post_state.revision_state;
                    active_rotation = post_state.active_rotation.clone();
                    transition_record.candidate_evidence =
                        post_state.evidence.candidate_evidence.clone();
                    transition_record.adoption_evidence =
                        post_state.evidence.adoption_evidence.clone();
                    transition_record.delivery_evidence =
                        post_state.evidence.delivery_evidence.clone();
                    transition_record.acknowledgement_evidence =
                        post_state.evidence.acknowledgement_evidence.clone();
                    transition_record.completion_evidence =
                        post_state.evidence.completion_evidence.clone();
                    successful =
                        matches!(phase, RotationPhase::Transfer | RotationPhase::Completion);
                }
                ResolutionOutcome::Failed {
                    attempt,
                    post_state,
                }
                | ResolutionOutcome::Cancelled {
                    attempt,
                    post_state,
                } => {
                    transition_record.state = post_state.transition_state;
                    revision_state = post_state.revision_state;
                    active_rotation = post_state.active_rotation.clone();
                    transition_record.terminal_attempt = Some(attempt.clone());
                    successful = false;
                }
            }
        }
        let family = store.family.as_mut().unwrap();
        family.revisions.insert(
            RoleRevisionNumber::new(8).unwrap(),
            RoleRevision {
                role_revision: RoleRevisionNumber::new(8).unwrap(),
                session: transition_record
                    .candidate_evidence
                    .as_ref()
                    .map(|candidate| candidate.session.clone()),
                state: revision_state,
                intended_predecessor: Some(authority()),
                successful_predecessor: successful.then(|| SuccessfulPredecessor {
                    role_revision: RoleRevisionNumber::new(7).unwrap(),
                    cutex_session_id: incumbent().cutex_session_id,
                    transfer_transition_id: transition(),
                }),
                root_revision: successful.then_some(RoleRevisionNumber::new(7).unwrap()),
                allocated_at: timestamp(),
                terminal_attempt: transition_record.terminal_attempt.clone(),
            },
        );
        family.transitions.insert(transition(), transition_record);
        family.next_role_revision = RoleRevisionNumber::new(9).unwrap();
        family.active_rotation = active_rotation;
        if successful {
            family
                .revisions
                .get_mut(&RoleRevisionNumber::new(7).unwrap())
                .unwrap()
                .state = RoleRevisionState::Superseded;
            family.current_authority = CurrentAuthority {
                role_revision: RoleRevisionNumber::new(8).unwrap(),
                cutex_session_id: candidate().cutex_session_id,
                authority_epoch: AuthorityEpoch::new(2).unwrap(),
                effective_at: EvidenceTime::Known {
                    rfc3339: timestamp(),
                },
                established_by: EstablishedBy::Transfer {
                    transition_id: transition(),
                },
            };
        }
        store.idempotency.clear();
        store
    }

    fn resolved_unknown_then_transfer_store() -> RoleSeatStore {
        let mut store = unknown_store(RotationPhase::Adoption, Some(None));
        let family = store.family.as_mut().unwrap();
        let transition_record = family.transitions.get_mut(&transition()).unwrap();
        transition_record.state = TransitionState::AuthorityTransferred;
        transition_record.delivery_evidence = Some(delivery_evidence());
        transition_record.acknowledgement_evidence = Some(acknowledgement_evidence());
        let revision = family
            .revisions
            .get_mut(&RoleRevisionNumber::new(8).unwrap())
            .unwrap();
        revision.state = RoleRevisionState::Current;
        revision.successful_predecessor = Some(SuccessfulPredecessor {
            role_revision: RoleRevisionNumber::new(7).unwrap(),
            cutex_session_id: incumbent().cutex_session_id,
            transfer_transition_id: transition(),
        });
        revision.root_revision = Some(RoleRevisionNumber::new(7).unwrap());
        family
            .revisions
            .get_mut(&RoleRevisionNumber::new(7).unwrap())
            .unwrap()
            .state = RoleRevisionState::Superseded;
        family.current_authority = CurrentAuthority {
            role_revision: RoleRevisionNumber::new(8).unwrap(),
            cutex_session_id: candidate().cutex_session_id,
            authority_epoch: AuthorityEpoch::new(2).unwrap(),
            effective_at: EvidenceTime::Known {
                rfc3339: timestamp(),
            },
            established_by: EstablishedBy::Transfer {
                transition_id: transition(),
            },
        };
        store
    }

    fn resolved_unknown_history_store() -> RoleSeatStore {
        let mut store = unknown_store(RotationPhase::Adoption, Some(None));
        let transition_record = store
            .family
            .as_mut()
            .unwrap()
            .transitions
            .get_mut(&transition())
            .unwrap();
        let mut next = unknown_observation(RotationPhase::InitialDelivery);
        next.attempt = AttemptNumber::new(2).unwrap();
        next.resolution = Some(successful_resolution(&next));
        transition_record.unknown_outcomes.push(next);
        transition_record.state = TransitionState::Acknowledged;
        transition_record.delivery_evidence = Some(delivery_evidence());
        transition_record.acknowledgement_evidence = Some(acknowledgement_evidence());
        store
    }

    fn ordinary_progress_between_unknowns_store() -> RoleSeatStore {
        let mut store = unknown_store(RotationPhase::Candidate, Some(None));
        let transition_record = store
            .family
            .as_mut()
            .unwrap()
            .transitions
            .get_mut(&transition())
            .unwrap();
        transition_record.state = TransitionState::Adopted;
        transition_record.adoption_evidence = Some(adoption_evidence());
        let mut next = unknown_observation(RotationPhase::InitialDelivery);
        next.attempt = AttemptNumber::new(2).unwrap();
        next.resolution = Some(successful_resolution(&next));
        transition_record.unknown_outcomes.push(next);
        transition_record.state = TransitionState::Acknowledged;
        transition_record.delivery_evidence = Some(delivery_evidence());
        transition_record.acknowledgement_evidence = Some(acknowledgement_evidence());
        store
    }

    fn parsed_store(store: &RoleSeatStore) -> RoleSeatStore {
        serde_json::from_str(&serde_json::to_string(store).unwrap()).unwrap()
    }

    fn assert_valid_store_json(store: &RoleSeatStore) {
        validate_store(&parsed_store(store)).unwrap();
    }

    fn assert_corrupt_store_json(store: &RoleSeatStore, code: ValidationCode) {
        assert_eq!(validate_store(&parsed_store(store)).unwrap_err().code, code);
    }

    #[test]
    fn store_json_empty_and_root_only_have_paired_corrupt_envelopes() {
        let empty = RoleSeatStore {
            schema: StoreSchema::V1,
            store_revision: StoreRevision::new(1).unwrap(),
            family: None,
            idempotency: BTreeMap::new(),
        };
        assert_valid_store_json(&empty);
        let mut corrupt_empty = empty.clone();
        corrupt_empty.idempotency.insert(
            RequestId::new("orphan-request").unwrap(),
            full_store().idempotency.into_values().next().unwrap(),
        );
        assert_corrupt_store_json(&corrupt_empty, ValidationCode::StoreEnvelopeMismatch);

        let root = root_only_store();
        assert_valid_store_json(&root);
        let mut corrupt_root = root.clone();
        corrupt_root
            .family
            .as_mut()
            .unwrap()
            .root_initialization
            .chosen_root_revision = RoleRevisionNumber::new(6).unwrap();
        assert_corrupt_store_json(&corrupt_root, ValidationCode::RootMismatch);

        let mut corrupt_allocator = root;
        corrupt_allocator
            .family
            .as_mut()
            .unwrap()
            .next_role_revision = RoleRevisionNumber::new(9).unwrap();
        assert_corrupt_store_json(&corrupt_allocator, ValidationCode::AllocatorMismatch);
    }

    #[test]
    fn store_json_two_completed_successes_retain_first_history_and_reject_bad_lineage() {
        let store = two_completed_successes_store();
        assert_valid_store_json(&store);
        assert_eq!(store.family.as_ref().unwrap().transitions.len(), 2);

        let mut missing = store.clone();
        missing
            .family
            .as_mut()
            .unwrap()
            .revisions
            .get_mut(&RoleRevisionNumber::new(9).unwrap())
            .unwrap()
            .successful_predecessor
            .as_mut()
            .unwrap()
            .role_revision = RoleRevisionNumber::new(6).unwrap();
        assert_corrupt_store_json(&missing, ValidationCode::LineageMismatch);

        let mut cyclic = store;
        cyclic
            .family
            .as_mut()
            .unwrap()
            .revisions
            .get_mut(&RoleRevisionNumber::new(9).unwrap())
            .unwrap()
            .successful_predecessor
            .as_mut()
            .unwrap()
            .role_revision = RoleRevisionNumber::new(9).unwrap();
        assert_corrupt_store_json(&cyclic, ValidationCode::LineageMismatch);

        let mut branched = two_completed_successes_store();
        let family = branched.family.as_mut().unwrap();
        let candidate = family
            .revisions
            .get(&RoleRevisionNumber::new(9).unwrap())
            .unwrap()
            .session
            .clone()
            .unwrap();
        let identity = family
            .transitions
            .get(&TransitionId::new("transition-b").unwrap())
            .unwrap()
            .handoff
            .recipient
            .clone();
        let context = context_for("transition-b", 9, authority(), &identity);
        family.transitions.insert(
            context.transition_id.clone(),
            transition_fixture(
                &context,
                &candidate,
                &identity,
                TransitionState::Completed,
                None,
            ),
        );
        let revision = family
            .revisions
            .get_mut(&RoleRevisionNumber::new(9).unwrap())
            .unwrap();
        revision.intended_predecessor = Some(authority());
        revision.successful_predecessor = Some(SuccessfulPredecessor {
            role_revision: RoleRevisionNumber::new(7).unwrap(),
            cutex_session_id: incumbent().cutex_session_id,
            transfer_transition_id: context.transition_id,
        });
        family.current_authority.authority_epoch = AuthorityEpoch::new(2).unwrap();
        assert_corrupt_store_json(&branched, ValidationCode::LineageMismatch);
    }

    #[test]
    fn store_json_failed_cancelled_consumed_gaps_then_success_are_paired() {
        let store = consumed_gaps_then_success_store();
        assert_valid_store_json(&store);
        let family = store.family.as_ref().unwrap();
        let ancestry = family
            .revisions
            .get(&RoleRevisionNumber::new(10).unwrap())
            .unwrap()
            .successful_predecessor
            .as_ref()
            .unwrap();
        assert_eq!(ancestry.role_revision, RoleRevisionNumber::new(7).unwrap());

        let mut corrupt_attempt = store.clone();
        corrupt_attempt
            .family
            .as_mut()
            .unwrap()
            .revisions
            .get_mut(&RoleRevisionNumber::new(8).unwrap())
            .unwrap()
            .terminal_attempt = None;
        assert_corrupt_store_json(&corrupt_attempt, ValidationCode::TerminalMismatch);

        let mut corrupt_allocator = store;
        let family = corrupt_allocator.family.as_mut().unwrap();
        family
            .revisions
            .remove(&RoleRevisionNumber::new(9).unwrap());
        assert_corrupt_store_json(&corrupt_allocator, ValidationCode::AllocatorMismatch);
    }

    #[test]
    fn store_json_active_lock_is_exact_for_every_live_state() {
        let states = [
            TransitionState::Prepared,
            TransitionState::CandidateRecorded,
            TransitionState::Adopted,
            TransitionState::InitialDeliveryRecorded,
            TransitionState::Acknowledged,
            TransitionState::AuthorityTransferred,
        ];
        for state in states {
            let store = active_store(state);
            assert_valid_store_json(&store);
            let mut corrupt = store;
            corrupt
                .family
                .as_mut()
                .unwrap()
                .active_rotation
                .as_mut()
                .unwrap()
                .candidate_revision = RoleRevisionNumber::new(9).unwrap();
            assert_corrupt_store_json(&corrupt, ValidationCode::LockMismatch);
        }
    }

    #[test]
    fn store_json_all_six_unknown_phases_unresolved_and_success_resolved_are_paired() {
        let phases = [
            RotationPhase::Candidate,
            RotationPhase::Adoption,
            RotationPhase::InitialDelivery,
            RotationPhase::Acknowledgement,
            RotationPhase::Transfer,
            RotationPhase::Completion,
        ];
        for phase in phases {
            let unresolved = unknown_store(phase, None);
            assert_valid_store_json(&unresolved);
            let mut corrupt_unresolved = unresolved;
            corrupt_unresolved
                .family
                .as_mut()
                .unwrap()
                .transitions
                .get_mut(&transition())
                .unwrap()
                .unknown_outcomes[0]
                .post_state
                .active_rotation
                .candidate_revision = RoleRevisionNumber::new(9).unwrap();
            assert_corrupt_store_json(&corrupt_unresolved, ValidationCode::UnknownMismatch);

            let resolved = unknown_store(phase, Some(None));
            assert_valid_store_json(&resolved);
            let mut corrupt_resolved = resolved;
            corrupt_resolved
                .family
                .as_mut()
                .unwrap()
                .transitions
                .get_mut(&transition())
                .unwrap()
                .unknown_outcomes[0]
                .resolution
                .as_mut()
                .unwrap()
                .evidence
                .kind = EvidenceKind::Failure;
            assert_corrupt_store_json(&corrupt_resolved, ValidationCode::EvidenceKindMismatch);
        }
    }

    #[test]
    fn store_json_resolved_unknown_history_is_continuous_and_time_ordered() {
        let store = resolved_unknown_history_store();
        assert_valid_store_json(&store);

        let mut duplicate_adoption = store.clone();
        let transition_record = duplicate_adoption
            .family
            .as_mut()
            .unwrap()
            .transitions
            .get_mut(&transition())
            .unwrap();
        let mut duplicate = transition_record.unknown_outcomes[0].clone();
        duplicate.attempt = AttemptNumber::new(2).unwrap();
        transition_record.unknown_outcomes[1] = duplicate;
        assert_corrupt_store_json(&duplicate_adoption, ValidationCode::UnknownMismatch);

        let mut regressive_phase = store.clone();
        let transition_record = regressive_phase
            .family
            .as_mut()
            .unwrap()
            .transitions
            .get_mut(&transition())
            .unwrap();
        let mut regressive = unknown_observation(RotationPhase::Adoption);
        regressive.attempt = AttemptNumber::new(2).unwrap();
        regressive.resolution = Some(successful_resolution(&regressive));
        transition_record.unknown_outcomes[1] = regressive;
        assert_corrupt_store_json(&regressive_phase, ValidationCode::UnknownMismatch);

        let mut broken_prior = store.clone();
        broken_prior
            .family
            .as_mut()
            .unwrap()
            .transitions
            .get_mut(&transition())
            .unwrap()
            .unknown_outcomes[1]
            .prior
            .active_rotation
            .source_authority_epoch = AuthorityEpoch::new(2).unwrap();
        assert_corrupt_store_json(&broken_prior, ValidationCode::UnknownMismatch);

        let mut next_before_resolution = store.clone();
        let transition_record = next_before_resolution
            .family
            .as_mut()
            .unwrap()
            .transitions
            .get_mut(&transition())
            .unwrap();
        transition_record.unknown_outcomes[0]
            .resolution
            .as_mut()
            .unwrap()
            .recorded_at = Rfc3339::new("2026-08-16T00:00:01Z").unwrap();
        transition_record.updated_at = Rfc3339::new("2026-08-16T00:00:02Z").unwrap();
        assert_corrupt_store_json(&next_before_resolution, ValidationCode::UnknownMismatch);

        let mut late_resolution = store;
        late_resolution
            .family
            .as_mut()
            .unwrap()
            .transitions
            .get_mut(&transition())
            .unwrap()
            .unknown_outcomes[0]
            .resolution
            .as_mut()
            .unwrap()
            .recorded_at = Rfc3339::new("2026-08-16T00:00:01Z").unwrap();
        assert_corrupt_store_json(&late_resolution, ValidationCode::UnknownMismatch);
    }

    #[test]
    fn store_json_unknown_history_allows_ordinary_forward_progress() {
        let store = ordinary_progress_between_unknowns_store();
        assert_valid_store_json(&store);

        let mut repeated_phase = store.clone();
        let transition_record = repeated_phase
            .family
            .as_mut()
            .unwrap()
            .transitions
            .get_mut(&transition())
            .unwrap();
        let mut repeated = unknown_observation(RotationPhase::Candidate);
        repeated.attempt = AttemptNumber::new(2).unwrap();
        repeated.resolution = Some(successful_resolution(&repeated));
        transition_record.unknown_outcomes[1] = repeated;
        assert_corrupt_store_json(&repeated_phase, ValidationCode::UnknownMismatch);

        let mut missing_intervening_evidence = store.clone();
        missing_intervening_evidence
            .family
            .as_mut()
            .unwrap()
            .transitions
            .get_mut(&transition())
            .unwrap()
            .unknown_outcomes[1]
            .prior
            .evidence
            .adoption_evidence = None;
        assert_corrupt_store_json(
            &missing_intervening_evidence,
            ValidationCode::UnknownMismatch,
        );

        let mut pre_transfer_drift = store.clone();
        pre_transfer_drift
            .family
            .as_mut()
            .unwrap()
            .transitions
            .get_mut(&transition())
            .unwrap()
            .unknown_outcomes[1]
            .prior
            .active_rotation
            .source_authority_epoch = AuthorityEpoch::new(2).unwrap();
        assert_corrupt_store_json(&pre_transfer_drift, ValidationCode::UnknownMismatch);

        let mut fabricated_transfer = store;
        let transition_record = fabricated_transfer
            .family
            .as_mut()
            .unwrap()
            .transitions
            .get_mut(&transition())
            .unwrap();
        let mut completion = unknown_observation(RotationPhase::Completion);
        completion.attempt = AttemptNumber::new(2).unwrap();
        transition_record.unknown_outcomes[1] = completion;
        assert_corrupt_store_json(&fabricated_transfer, ValidationCode::UnknownMismatch);
    }

    #[test]
    fn store_json_unknown_terminal_resolutions_are_valid_only_before_transfer() {
        let pre_transfer = [
            RotationPhase::Candidate,
            RotationPhase::Adoption,
            RotationPhase::InitialDelivery,
            RotationPhase::Acknowledgement,
        ];
        for phase in pre_transfer {
            assert_valid_store_json(&unknown_store(phase, Some(Some(TerminalOutcome::Failed))));
            assert_valid_store_json(&unknown_store(
                phase,
                Some(Some(TerminalOutcome::Cancelled)),
            ));
        }
        for phase in [RotationPhase::Transfer, RotationPhase::Completion] {
            assert_corrupt_store_json(
                &unknown_store(phase, Some(Some(TerminalOutcome::Failed))),
                ValidationCode::UnknownMismatch,
            );
            assert_corrupt_store_json(
                &unknown_store(phase, Some(Some(TerminalOutcome::Cancelled))),
                ValidationCode::UnknownMismatch,
            );
        }
    }

    #[test]
    fn store_json_current_pointer_session_epoch_revision_and_state_are_paired() {
        let store = two_completed_successes_store();
        assert_valid_store_json(&store);

        let mut bad_session = store.clone();
        bad_session
            .family
            .as_mut()
            .unwrap()
            .current_authority
            .cutex_session_id = incumbent().cutex_session_id;
        assert_corrupt_store_json(&bad_session, ValidationCode::RevisionMismatch);

        let mut bad_pointer = store.clone();
        bad_pointer
            .family
            .as_mut()
            .unwrap()
            .current_authority
            .role_revision = RoleRevisionNumber::new(8).unwrap();
        assert_corrupt_store_json(&bad_pointer, ValidationCode::RevisionMismatch);

        let mut bad_epoch = store.clone();
        bad_epoch
            .family
            .as_mut()
            .unwrap()
            .current_authority
            .authority_epoch = AuthorityEpoch::new(2).unwrap();
        assert_corrupt_store_json(&bad_epoch, ValidationCode::LineageMismatch);

        let mut bad_state = store;
        bad_state
            .family
            .as_mut()
            .unwrap()
            .revisions
            .get_mut(&RoleRevisionNumber::new(9).unwrap())
            .unwrap()
            .state = RoleRevisionState::Candidate;
        assert_corrupt_store_json(&bad_state, ValidationCode::RevisionMismatch);
    }

    #[test]
    fn store_json_orphan_transition_revision_and_idempotency_records_are_paired() {
        let store = root_only_store();
        assert_valid_store_json(&store);

        let mut orphan_revision = store.clone();
        let family = orphan_revision.family.as_mut().unwrap();
        family.revisions.insert(
            RoleRevisionNumber::new(8).unwrap(),
            RoleRevision {
                role_revision: RoleRevisionNumber::new(8).unwrap(),
                session: None,
                state: RoleRevisionState::Candidate,
                intended_predecessor: Some(authority()),
                successful_predecessor: None,
                root_revision: None,
                allocated_at: timestamp(),
                terminal_attempt: None,
            },
        );
        family.next_role_revision = RoleRevisionNumber::new(9).unwrap();
        assert_corrupt_store_json(&orphan_revision, ValidationCode::RevisionMismatch);

        let mut orphan_transition = store.clone();
        let (session, identity) = runtime_named("orphan", 1);
        let context = context_for("transition-orphan", 8, authority(), &identity);
        orphan_transition
            .family
            .as_mut()
            .unwrap()
            .transitions
            .insert(
                context.transition_id.clone(),
                transition_fixture(
                    &context,
                    &session,
                    &identity,
                    TransitionState::Prepared,
                    None,
                ),
            );
        assert_corrupt_store_json(&orphan_transition, ValidationCode::TransitionMismatch);

        let mut orphan_idempotency = store;
        orphan_idempotency.idempotency.insert(
            RequestId::new("orphan-transfer").unwrap(),
            full_store().idempotency.into_values().next().unwrap(),
        );
        assert_corrupt_store_json(&orphan_idempotency, ValidationCode::IdempotencyMismatch);
    }

    #[test]
    fn store_json_resolved_unknown_audit_removal_and_incompatible_payload_are_paired() {
        assert_valid_store_json(&resolved_unknown_then_transfer_store());
        let mut store = unknown_store(RotationPhase::Adoption, Some(None));
        let unknown = store
            .family
            .as_ref()
            .unwrap()
            .transitions
            .get(&transition())
            .unwrap()
            .unknown_outcomes[0]
            .clone();
        store.idempotency.insert(
            RequestId::new("resolve-audit").unwrap(),
            IdempotencyRecord {
                operation: Operation::ResolveUnknown,
                project_id: project(),
                role_family_id: family(),
                initialization_id: initialization(),
                request_digest_sha256: sha('d'),
                committed_store_revision: StoreRevision::new(2).unwrap(),
                result: MutationResult::ResolveUnknown {
                    transition_id: transition(),
                    unknown,
                },
            },
        );
        assert_valid_store_json(&store);

        let mut removed = store.clone();
        removed
            .family
            .as_mut()
            .unwrap()
            .transitions
            .get_mut(&transition())
            .unwrap()
            .unknown_outcomes
            .clear();
        assert_corrupt_store_json(&removed, ValidationCode::IdempotencyMismatch);

        let mut incompatible = store;
        incompatible
            .family
            .as_mut()
            .unwrap()
            .transitions
            .get_mut(&transition())
            .unwrap()
            .unknown_outcomes[0]
            .attempted_payload = phase_payload(RotationPhase::Candidate);
        assert_corrupt_store_json(&incompatible, ValidationCode::UnknownMismatch);
    }

    #[test]
    fn store_json_idempotency_scope_operation_result_and_commit_are_paired() {
        let store = idempotent_transfer_store();
        assert_valid_store_json(&store);

        let mut wrong_scope = store.clone();
        wrong_scope
            .idempotency
            .values_mut()
            .next()
            .unwrap()
            .project_id = ProjectId::new("other-project").unwrap();
        assert_corrupt_store_json(&wrong_scope, ValidationCode::IdempotencyMismatch);

        let mut future_commit = store.clone();
        future_commit
            .idempotency
            .values_mut()
            .next()
            .unwrap()
            .committed_store_revision = StoreRevision::new(3).unwrap();
        assert_corrupt_store_json(&future_commit, ValidationCode::IdempotencyMismatch);

        let mut wrong_result = store;
        let record = wrong_result.idempotency.values_mut().next().unwrap();
        record.operation = Operation::CompleteRotation;
        assert_corrupt_store_json(&wrong_result, ValidationCode::IdempotencyMismatch);
    }

    fn primary_evidence_mut(request: &mut MutationRequest) -> &mut EvidenceRef {
        match request {
            MutationRequest::InitializeFamily(input) => &mut input.approval_evidence,
            MutationRequest::PrepareRotation(input) => &mut input.approval_evidence,
            MutationRequest::RecordCandidate(input) => &mut input.evidence,
            MutationRequest::RecordAdoption(input) => &mut input.evidence,
            MutationRequest::RecordInitialDelivery(input) => &mut input.evidence,
            MutationRequest::RecordAcknowledgement(input) => &mut input.evidence,
            MutationRequest::TransferAuthority(input) => &mut input.evidence,
            MutationRequest::CompleteRotation(input) => &mut input.evidence,
            MutationRequest::FailRotation(input) => &mut input.evidence,
            MutationRequest::CancelRotation(input) => &mut input.evidence,
            MutationRequest::RecordUnknown(input) => &mut input.evidence,
            MutationRequest::ResolveUnknown(input) => &mut input.evidence,
        }
    }

    fn deserialize_rejects<T: DeserializeOwned>(value: Value) {
        assert!(serde_json::from_value::<T>(value).is_err());
    }

    fn json_fixture<T>(value: &T) -> T
    where
        T: serde::Serialize + DeserializeOwned,
    {
        serde_json::from_str(&serde_json::to_string(value).unwrap()).unwrap()
    }

    fn response_with_matching_unknown(
        _request: &RequestEnvelope,
        response: &MutationResponse,
    ) -> MutationResponse {
        response.clone()
    }

    #[derive(Clone, Debug)]
    enum JsonPathPart {
        Key(String),
        Index(usize),
    }

    fn collect_receipt_paths(
        value: &Value,
        current: &mut Vec<JsonPathPart>,
        paths: &mut Vec<Vec<JsonPathPart>>,
    ) {
        match value {
            Value::Object(object) => {
                if object.contains_key("kind")
                    && object.contains_key("receipt_id")
                    && object.contains_key("receipt_sha256")
                    && object.contains_key("subjects")
                    && object.contains_key("occurred_at")
                {
                    paths.push(current.clone());
                }
                for (key, child) in object {
                    current.push(JsonPathPart::Key(key.clone()));
                    collect_receipt_paths(child, current, paths);
                    current.pop();
                }
            }
            Value::Array(array) => {
                for (index, child) in array.iter().enumerate() {
                    current.push(JsonPathPart::Index(index));
                    collect_receipt_paths(child, current, paths);
                    current.pop();
                }
            }
            _ => {}
        }
    }

    fn value_at_path_mut<'a>(value: &'a mut Value, path: &[JsonPathPart]) -> &'a mut Value {
        let mut current = value;
        for part in path {
            current = match part {
                JsonPathPart::Key(key) => current.get_mut(key).unwrap(),
                JsonPathPart::Index(index) => current.get_mut(*index).unwrap(),
            };
        }
        current
    }

    #[test]
    fn every_operation_has_a_valid_json_fixture_and_exact_result() {
        let pairs = valid_pairs();
        assert_eq!(pairs.len(), 12);
        for (request, response) in pairs {
            let request_json = serde_json::to_string(&request).unwrap();
            let response_json = serde_json::to_string(&response).unwrap();
            let request: RequestEnvelope = serde_json::from_str(&request_json).unwrap();
            let response: MutationResponse = serde_json::from_str(&response_json).unwrap();
            validate_request(&request, &response).unwrap();
        }
    }

    #[test]
    fn every_operation_rejects_reordered_or_extra_evidence_subjects() {
        for (mut request, response) in valid_pairs() {
            let evidence = primary_evidence_mut(&mut request.request);
            evidence.subjects.swap(0, 1);
            assert_eq!(
                validate_request(&request, &response).unwrap_err().code,
                ValidationCode::EvidenceSubjectsMismatch
            );
        }

        for (mut request, response) in valid_pairs() {
            primary_evidence_mut(&mut request.request)
                .subjects
                .push(IdentityRef::Project { id: project() });
            assert_eq!(
                validate_request(&request, &response).unwrap_err().code,
                ValidationCode::EvidenceSubjectsMismatch
            );
        }
    }

    #[test]
    fn every_nested_receipt_rejects_wrong_subject_order_extra_and_unknown_field() {
        let mut pairs = valid_pairs();
        for phase in [
            RotationPhase::Candidate,
            RotationPhase::Adoption,
            RotationPhase::InitialDelivery,
            RotationPhase::Acknowledgement,
            RotationPhase::Transfer,
            RotationPhase::Completion,
        ] {
            let identity = runtime();
            let unknown = unknown_observation(phase);
            let record = envelope(MutationRequest::RecordUnknown(RecordUnknownRequest {
                context: context(),
                adopted_identity: identity.clone(),
                attempt: unknown.attempt,
                phase: unknown.phase,
                attempted_payload: unknown.attempted_payload.clone(),
                reason_code: unknown.reason_code,
                evidence: unknown.evidence.clone(),
            }));
            let record_response = response(
                &record,
                MutationResult::RecordUnknown {
                    transition_id: transition(),
                    unknown: unknown.clone(),
                },
            );
            pairs.push((record, record_response));

            let resolution = successful_resolution(&unknown);
            let mut resolved = unknown.clone();
            resolved.resolution = Some(resolution.clone());
            let resolve = envelope(MutationRequest::ResolveUnknown(ResolveUnknownRequest {
                context: context(),
                adopted_identity: identity,
                attempt: unknown.attempt,
                outcome: ResolutionIntent::PhaseSucceeded {
                    verified_payload: unknown.attempted_payload,
                },
                evidence: resolution.evidence,
            }));
            let resolve_response = response(
                &resolve,
                MutationResult::ResolveUnknown {
                    transition_id: transition(),
                    unknown: resolved,
                },
            );
            pairs.push((resolve, resolve_response));
        }
        let mut receipt_count = 0;
        for (request, response) in pairs {
            let valid = serde_json::to_value(&request).unwrap();
            let valid_request: RequestEnvelope = serde_json::from_value(valid.clone()).unwrap();
            let valid_response = json_fixture(&response);
            validate_request(&valid_request, &valid_response).unwrap();
            let mut paths = Vec::new();
            collect_receipt_paths(&valid, &mut Vec::new(), &mut paths);
            assert!(!paths.is_empty());
            receipt_count += paths.len();

            for path in paths {
                let mut wrong_subject = valid.clone();
                value_at_path_mut(&mut wrong_subject, &path)["subjects"]
                    .as_array_mut()
                    .unwrap()
                    .first_mut()
                    .unwrap()["id"] = json!("forged-required-subject");
                let wrong_subject: RequestEnvelope = serde_json::from_value(wrong_subject).unwrap();
                let matching_response =
                    json_fixture(&response_with_matching_unknown(&wrong_subject, &response));
                assert!(validate_request(&wrong_subject, &matching_response).is_err());

                let mut reordered = valid.clone();
                value_at_path_mut(&mut reordered, &path)["subjects"]
                    .as_array_mut()
                    .unwrap()
                    .swap(0, 1);
                let reordered: RequestEnvelope = serde_json::from_value(reordered).unwrap();
                let matching_response = response_with_matching_unknown(&reordered, &response);
                assert!(validate_request(&reordered, &matching_response).is_err());

                let mut extra_subject = valid.clone();
                value_at_path_mut(&mut extra_subject, &path)["subjects"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"kind": "project", "id": "project-forged"}));
                let extra_subject: RequestEnvelope = serde_json::from_value(extra_subject).unwrap();
                let matching_response = response_with_matching_unknown(&extra_subject, &response);
                assert!(validate_request(&extra_subject, &matching_response).is_err());

                let mut unknown_field = valid.clone();
                value_at_path_mut(&mut unknown_field, &path)["credential"] = json!("forbidden");
                deserialize_rejects::<RequestEnvelope>(unknown_field);
            }
        }
        assert!(receipt_count >= 30);
    }

    #[test]
    fn closed_schema_enum_and_unknown_field_json_are_paired() {
        let (request, _) = valid_pairs().remove(0);
        let valid = serde_json::to_value(&request).unwrap();
        assert!(serde_json::from_value::<RequestEnvelope>(valid.clone()).is_ok());

        let mut wrong_schema = valid.clone();
        wrong_schema["schema"] = json!("cutex/role-seat-request/v2");
        deserialize_rejects::<RequestEnvelope>(wrong_schema);

        let mut wrong_operation = valid.clone();
        wrong_operation["request"]["operation"] = json!("delete_family");
        deserialize_rejects::<RequestEnvelope>(wrong_operation);

        let mut unknown_input_field = valid.clone();
        unknown_input_field["request"]["input"]["credential"] = json!("forbidden");
        deserialize_rejects::<RequestEnvelope>(unknown_input_field);

        let store = RoleSeatStore {
            schema: StoreSchema::V1,
            store_revision: StoreRevision::new(1).unwrap(),
            family: None,
            idempotency: BTreeMap::new(),
        };
        let valid_store = serde_json::to_value(store).unwrap();
        assert!(serde_json::from_value::<RoleSeatStore>(valid_store.clone()).is_ok());
        let mut wrong_store_schema = valid_store.clone();
        wrong_store_schema["schema"] = json!("cutex/role-seat-core/v2");
        deserialize_rejects::<RoleSeatStore>(wrong_store_schema);
        let mut extra_store_field = valid_store;
        extra_store_field["body"] = json!("forbidden");
        deserialize_rejects::<RoleSeatStore>(extra_store_field);

        let complete_store = serde_json::to_value(full_store()).unwrap();
        assert!(serde_json::from_value::<RoleSeatStore>(complete_store.clone()).is_ok());
        let mut unknown_revision_state = complete_store.clone();
        unknown_revision_state["family"]["revisions"]["8"]["state"] = json!("orphaned");
        deserialize_rejects::<RoleSeatStore>(unknown_revision_state);
        let mut nested_unknown_field = complete_store;
        nested_unknown_field["family"]["transitions"]["transition-a"]["credential"] =
            json!("forbidden");
        deserialize_rejects::<RoleSeatStore>(nested_unknown_field);
    }

    #[test]
    fn every_numeric_newtype_rejects_zero_and_max_plus_one() {
        macro_rules! bounds {
            ($kind:ty) => {{
                assert!(serde_json::from_str::<$kind>("0").is_err());
                let maximum: $kind =
                    serde_json::from_str(&MAX_JSON_SAFE_INTEGER.to_string()).unwrap();
                assert_eq!(maximum.get(), MAX_JSON_SAFE_INTEGER);
                assert!(maximum.checked_next().is_err());
                assert!(serde_json::from_str::<$kind>("9007199254740992").is_err());
            }};
        }
        bounds!(StoreRevision);
        bounds!(RoleRevisionNumber);
        bounds!(AuthorityEpoch);
        bounds!(RuntimeGeneration);
        bounds!(TaskRevision);
        bounds!(DurableRevision);
        bounds!(AttemptNumber);
        bounds!(NumericResult);
        assert_eq!(
            StoreRevision::new(1).unwrap().checked_next().unwrap().get(),
            2
        );
    }

    #[test]
    fn project_family_session_task_handoff_and_required_fields_are_bound() {
        let pairs = valid_pairs();

        let (mut wrong_project, response) = pairs[0].clone();
        if let MutationRequest::InitializeFamily(input) = &mut wrong_project.request {
            input.project_id = ProjectId::new("project-forged").unwrap();
        }
        assert!(validate_request(&json_fixture(&wrong_project), &json_fixture(&response)).is_err());

        let (mut wrong_family, response) = pairs[2].clone();
        if let MutationRequest::RecordCandidate(input) = &mut wrong_family.request {
            input.context.role_family_id = RoleFamilyId::new("family-forged").unwrap();
        }
        assert!(validate_request(&json_fixture(&wrong_family), &json_fixture(&response)).is_err());

        let (mut wrong_session, response) = pairs[2].clone();
        if let MutationRequest::RecordCandidate(input) = &mut wrong_session.request {
            input.successor.cutex_session_id = CutexSessionId::new("cutex-forged").unwrap();
            if let Some(IdentityRef::CutexSession { id }) = input.evidence.subjects.last_mut() {
                *id = input.successor.cutex_session_id.clone();
            }
        }
        assert!(validate_request(&json_fixture(&wrong_session), &json_fixture(&response)).is_err());

        let (mut wrong_task, response) = pairs[5].clone();
        if let MutationRequest::RecordAcknowledgement(input) = &mut wrong_task.request {
            input.context.handoff.task_id = TaskId::new("task-forged").unwrap();
        }
        assert!(validate_request(&json_fixture(&wrong_task), &json_fixture(&response)).is_err());

        let (mut wrong_handoff, mut response) = pairs[5].clone();
        if let MutationRequest::RecordAcknowledgement(input) = &mut wrong_handoff.request {
            input.handoff_sha256 = sha('c');
        }
        if let MutationResult::RecordAcknowledgement { handoff_sha256, .. } = &mut response.result {
            *handoff_sha256 = sha('c');
        }
        assert!(validate_request(&json_fixture(&wrong_handoff), &json_fixture(&response)).is_err());

        let mut missing_initialization = serde_json::to_value(&pairs[2].0).unwrap();
        missing_initialization["request"]["input"]["context"]
            .as_object_mut()
            .unwrap()
            .remove("initialization_id");
        deserialize_rejects::<RequestEnvelope>(missing_initialization);

        let mut missing_source_durable = serde_json::to_value(&pairs[1].0).unwrap();
        missing_source_durable["request"]["input"]["source_authority"]
            .as_object_mut()
            .unwrap()
            .remove("source_durable_revision");
        deserialize_rejects::<RequestEnvelope>(missing_source_durable);
    }

    #[test]
    fn paired_runtime_generation_forgery_fails_against_handoff_identity() {
        let (mut request, mut response) = valid_pairs()[3].clone();
        if let MutationRequest::RecordAdoption(input) = &mut request.request {
            input.identity.runtime_generation = RuntimeGeneration::new(5).unwrap();
            if let Some(IdentityRef::RuntimeAgent { generation, .. }) =
                input.evidence.subjects.last_mut()
            {
                *generation = RuntimeGeneration::new(5).unwrap();
            }
        }
        if let MutationResult::RecordAdoption { identity, .. } = &mut response.result {
            identity.runtime_generation = RuntimeGeneration::new(5).unwrap();
        }
        assert_eq!(
            validate_request(&json_fixture(&request), &json_fixture(&response))
                .unwrap_err()
                .code,
            ValidationCode::ContextMismatch
        );
    }

    #[test]
    fn cross_operation_result_and_replay_relations_fail_closed() {
        let pairs = valid_pairs();
        let (request, mut response) = pairs[2].clone();
        response.result = pairs[3].1.result.clone();
        assert_eq!(
            validate_request(&json_fixture(&request), &json_fixture(&response))
                .unwrap_err()
                .code,
            ValidationCode::OperationMismatch
        );

        let (request, mut response) = pairs[4].clone();
        response.disposition = ResultDisposition::Replay {
            original_request_id: RequestId::new("request-forged").unwrap(),
            request_digest_sha256: sha('d'),
            original_committed_store_revision: response.committed_store_revision,
        };
        let request = json_fixture(&request);
        let response = json_fixture(&response);
        assert_eq!(
            validate_request(&request, &response).unwrap_err().code,
            ValidationCode::ReplayMismatch
        );

        let (request, mut response) = pairs[4].clone();
        response.disposition = ResultDisposition::Replay {
            original_request_id: request.request_id.clone(),
            request_digest_sha256: sha('d'),
            original_committed_store_revision: StoreRevision::new(1).unwrap(),
        };
        let request = json_fixture(&request);
        let response = json_fixture(&response);
        assert_eq!(
            validate_request(&request, &response).unwrap_err().code,
            ValidationCode::ReplayMismatch
        );

        let mut incompatible = serde_json::to_value(&pairs[2].0).unwrap();
        incompatible["request"]["input"]["delivery_id"] = json!("transition-a/initial");
        deserialize_rejects::<RequestEnvelope>(incompatible);
    }

    #[test]
    fn skipped_allocator_applied_revision_and_replay_digest_are_paired() {
        let pairs = valid_pairs();

        let (mut skipped, mut skipped_response) = pairs[1].clone();
        if let MutationRequest::PrepareRotation(input) = &mut skipped.request {
            input.allocator.next_role_revision = RoleRevisionNumber::new(9).unwrap();
        }
        if let MutationResult::PrepareRotation {
            candidate_revision, ..
        } = &mut skipped_response.result
        {
            *candidate_revision = RoleRevisionNumber::new(9).unwrap();
        }
        let skipped = json_fixture(&skipped);
        let skipped_response = json_fixture(&skipped_response);
        validate_request(&skipped, &skipped_response).unwrap();

        let mut wrong_candidate = skipped_response.clone();
        if let MutationResult::PrepareRotation {
            candidate_revision, ..
        } = &mut wrong_candidate.result
        {
            *candidate_revision = RoleRevisionNumber::new(8).unwrap();
        }
        let wrong_candidate = json_fixture(&wrong_candidate);
        assert_eq!(
            validate_request(&skipped, &wrong_candidate)
                .unwrap_err()
                .code,
            ValidationCode::ResultMismatch
        );

        let mut wrong_allocator_scope = skipped.clone();
        if let MutationRequest::PrepareRotation(input) = &mut wrong_allocator_scope.request {
            input.allocator.observed_store_revision = StoreRevision::new(2).unwrap();
        }
        let wrong_allocator_scope = json_fixture(&wrong_allocator_scope);
        assert_eq!(
            validate_request(&wrong_allocator_scope, &skipped_response)
                .unwrap_err()
                .code,
            ValidationCode::ResultMismatch
        );

        let (request, mut wrong_applied_revision) = pairs[3].clone();
        wrong_applied_revision.committed_store_revision = StoreRevision::new(99).unwrap();
        let wrong_applied_revision = json_fixture(&wrong_applied_revision);
        assert_eq!(
            validate_request(&request, &wrong_applied_revision)
                .unwrap_err()
                .code,
            ValidationCode::ResultMismatch
        );

        let (request, mut replay) = pairs[4].clone();
        replay.disposition = ResultDisposition::Replay {
            original_request_id: request.request_id.clone(),
            request_digest_sha256: request.request_digest_sha256.clone(),
            original_committed_store_revision: StoreRevision::new(2).unwrap(),
        };
        let replay = json_fixture(&replay);
        validate_request(&request, &replay).unwrap();

        let mut wrong_digest = replay;
        if let ResultDisposition::Replay {
            request_digest_sha256,
            ..
        } = &mut wrong_digest.disposition
        {
            *request_digest_sha256 = sha('e');
        }
        let wrong_digest = json_fixture(&wrong_digest);
        assert_eq!(
            validate_request(&request, &wrong_digest).unwrap_err().code,
            ValidationCode::ReplayMismatch
        );

        let mut missing_digest = serde_json::to_value(&request).unwrap();
        missing_digest
            .as_object_mut()
            .unwrap()
            .remove("request_digest_sha256");
        deserialize_rejects::<RequestEnvelope>(missing_digest);
    }

    #[test]
    fn all_six_unknown_phases_have_valid_and_corrupt_json_pairs() {
        let phases = [
            RotationPhase::Candidate,
            RotationPhase::Adoption,
            RotationPhase::InitialDelivery,
            RotationPhase::Acknowledgement,
            RotationPhase::Transfer,
            RotationPhase::Completion,
        ];
        for phase in phases {
            let identity = runtime();
            let unknown = unknown_observation(phase);
            let record = envelope(MutationRequest::RecordUnknown(RecordUnknownRequest {
                context: context(),
                adopted_identity: identity.clone(),
                attempt: unknown.attempt,
                phase: unknown.phase,
                attempted_payload: unknown.attempted_payload.clone(),
                reason_code: unknown.reason_code,
                evidence: unknown.evidence.clone(),
            }));
            let record_response = response(
                &record,
                MutationResult::RecordUnknown {
                    transition_id: transition(),
                    unknown: unknown.clone(),
                },
            );
            let record_json = serde_json::to_string(&record).unwrap();
            let parsed_record: RequestEnvelope = serde_json::from_str(&record_json).unwrap();
            validate_request(&parsed_record, &record_response).unwrap();

            let resolution = successful_resolution(&unknown);
            let mut resolved_unknown = unknown.clone();
            resolved_unknown.resolution = Some(resolution.clone());
            let resolve = envelope(MutationRequest::ResolveUnknown(ResolveUnknownRequest {
                context: context(),
                adopted_identity: identity,
                attempt: unknown.attempt,
                outcome: resolution_intent(&resolution),
                evidence: resolution.evidence.clone(),
            }));
            let resolve_response = response(
                &resolve,
                MutationResult::ResolveUnknown {
                    transition_id: transition(),
                    unknown: resolved_unknown,
                },
            );
            let resolve_json = serde_json::to_string(&resolve).unwrap();
            let parsed_resolve: RequestEnvelope = serde_json::from_str(&resolve_json).unwrap();
            validate_request(&parsed_resolve, &resolve_response).unwrap();

            let mut corrupt_prior = record_response.clone();
            if let MutationResult::RecordUnknown { unknown, .. } = &mut corrupt_prior.result {
                unknown.prior.evidence.completion_evidence = Some(completion_evidence());
            }
            let corrupt_prior = json_fixture(&corrupt_prior);
            assert!(validate_request(&record, &corrupt_prior).is_err());

            let mut corrupt_post = record_response.clone();
            if let MutationResult::RecordUnknown { unknown, .. } = &mut corrupt_post.result {
                unknown.post_state.evidence.completion_evidence = Some(completion_evidence());
            }
            let corrupt_post = json_fixture(&corrupt_post);
            assert!(validate_request(&record, &corrupt_post).is_err());

            let mut corrupt_payload = resolve.clone();
            if let MutationRequest::ResolveUnknown(input) = &mut corrupt_payload.request {
                if let ResolutionIntent::PhaseSucceeded { verified_payload } = &mut input.outcome {
                    *verified_payload = phase_payload(if phase == RotationPhase::Candidate {
                        RotationPhase::Adoption
                    } else {
                        RotationPhase::Candidate
                    });
                }
            }
            let corrupt_payload = json_fixture(&corrupt_payload);
            assert!(validate_request(&corrupt_payload, &resolve_response).is_err());
        }
    }

    #[test]
    fn completion_requires_transferred_authority_and_terminal_post_transfer_is_forbidden() {
        let identity = runtime();
        let completion_unknown = unknown_observation(RotationPhase::Completion);
        let completion_record = envelope(MutationRequest::RecordUnknown(RecordUnknownRequest {
            context: context(),
            adopted_identity: identity.clone(),
            attempt: completion_unknown.attempt,
            phase: completion_unknown.phase,
            attempted_payload: completion_unknown.attempted_payload.clone(),
            reason_code: completion_unknown.reason_code,
            evidence: completion_unknown.evidence.clone(),
        }));
        let completion_response = response(
            &completion_record,
            MutationResult::RecordUnknown {
                transition_id: transition(),
                unknown: completion_unknown.clone(),
            },
        );
        let completion_record = json_fixture(&completion_record);
        let completion_response = json_fixture(&completion_response);
        validate_request(&completion_record, &completion_response).unwrap();

        let mut predecessor_current = completion_response.clone();
        if let MutationResult::RecordUnknown { unknown, .. } = &mut predecessor_current.result {
            unknown.prior.current_authority = authority();
            unknown.post_state.current_authority = authority();
        }
        let predecessor_current = json_fixture(&predecessor_current);
        assert_eq!(
            validate_request(&completion_record, &predecessor_current)
                .unwrap_err()
                .code,
            ValidationCode::UnknownMismatch
        );

        for phase in [
            RotationPhase::Candidate,
            RotationPhase::Adoption,
            RotationPhase::InitialDelivery,
            RotationPhase::Acknowledgement,
        ] {
            for outcome in [TerminalOutcome::Failed, TerminalOutcome::Cancelled] {
                let unknown = unknown_observation(phase);
                let resolution = terminal_resolution(&unknown, outcome);
                let mut resolved = unknown.clone();
                resolved.resolution = Some(resolution.clone());
                let request = envelope(MutationRequest::ResolveUnknown(ResolveUnknownRequest {
                    context: context(),
                    adopted_identity: identity.clone(),
                    attempt: unknown.attempt,
                    outcome: resolution_intent(&resolution),
                    evidence: resolution.evidence.clone(),
                }));
                let response = response(
                    &request,
                    MutationResult::ResolveUnknown {
                        transition_id: transition(),
                        unknown: resolved,
                    },
                );
                let request = json_fixture(&request);
                let response = json_fixture(&response);
                validate_request(&request, &response).unwrap();
            }
        }

        for phase in [RotationPhase::Transfer, RotationPhase::Completion] {
            for outcome in [TerminalOutcome::Failed, TerminalOutcome::Cancelled] {
                let unknown = unknown_observation(phase);
                let resolution = terminal_resolution(&unknown, outcome);
                let mut resolved = unknown.clone();
                resolved.resolution = Some(resolution.clone());
                let request = envelope(MutationRequest::ResolveUnknown(ResolveUnknownRequest {
                    context: context(),
                    adopted_identity: identity.clone(),
                    attempt: unknown.attempt,
                    outcome: resolution_intent(&resolution),
                    evidence: resolution.evidence.clone(),
                }));
                let response = response(
                    &request,
                    MutationResult::ResolveUnknown {
                        transition_id: transition(),
                        unknown: resolved,
                    },
                );
                let request = json_fixture(&request);
                let response = json_fixture(&response);
                assert_eq!(
                    validate_request(&request, &response).unwrap_err().code,
                    ValidationCode::UnknownMismatch
                );
            }
        }
    }

    #[test]
    fn terminal_attempt_and_unknown_shapes_reject_incompatible_pairs() {
        let pairs = valid_pairs();
        let (request, mut response) = pairs[8].clone();
        if let MutationResult::FailRotation { attempt, .. } = &mut response.result {
            attempt.outcome = TerminalOutcome::Cancelled;
        }
        let request = json_fixture(&request);
        let response = json_fixture(&response);
        assert_eq!(
            validate_request(&request, &response).unwrap_err().code,
            ValidationCode::TerminalMismatch
        );

        let (mut phase_mismatch, response) = pairs[10].clone();
        if let MutationRequest::RecordUnknown(input) = &mut phase_mismatch.request {
            input.phase = RotationPhase::Transfer;
        }
        let phase_mismatch = json_fixture(&phase_mismatch);
        let response = json_fixture(&response);
        assert_eq!(
            validate_request(&phase_mismatch, &response)
                .unwrap_err()
                .code,
            ValidationCode::UnknownMismatch
        );

        let (request, mut prior_mismatch) = pairs[10].clone();
        if let MutationResult::RecordUnknown { unknown, .. } = &mut prior_mismatch.result {
            unknown.prior.transition_state = TransitionState::Acknowledged;
        }
        let prior_mismatch = json_fixture(&prior_mismatch);
        let request = json_fixture(&request);
        assert_eq!(
            validate_request(&request, &prior_mismatch)
                .unwrap_err()
                .code,
            ValidationCode::UnknownMismatch
        );

        let (request, mut post_mismatch) = pairs[10].clone();
        if let MutationResult::RecordUnknown { unknown, .. } = &mut post_mismatch.result {
            unknown.post_state.revision_state = RoleRevisionState::Current;
        }
        let post_mismatch = json_fixture(&post_mismatch);
        let request = json_fixture(&request);
        assert_eq!(
            validate_request(&request, &post_mismatch).unwrap_err().code,
            ValidationCode::UnknownMismatch
        );

        let (mut payload_mismatch, response) = pairs[11].clone();
        if let MutationRequest::ResolveUnknown(input) = &mut payload_mismatch.request {
            if let ResolutionIntent::PhaseSucceeded { verified_payload } = &mut input.outcome {
                *verified_payload = PhasePayload::Completion {
                    transition_id: transition(),
                    evidence: completion_evidence(),
                };
            }
        }
        let payload_mismatch = json_fixture(&payload_mismatch);
        let response = json_fixture(&response);
        assert_eq!(
            validate_request(&payload_mismatch, &response)
                .unwrap_err()
                .code,
            ValidationCode::UnknownMismatch
        );
    }

    #[test]
    fn strict_value_json_rejects_noncanonical_time_hash_and_ids() {
        assert!(serde_json::from_str::<Rfc3339>("\"2026-08-16T10:00:00+10:00\"").is_err());
        assert!(serde_json::from_str::<Rfc3339>("\"2026-08-16T00:00:00Z\"").is_ok());
        assert!(serde_json::from_value::<Sha256>(json!("A".repeat(64))).is_err());
        assert!(serde_json::from_value::<Sha256>(json!("a".repeat(64))).is_ok());
        assert!(serde_json::from_value::<ProjectId>(json!("project with spaces")).is_err());
        assert!(serde_json::from_value::<ProjectId>(json!("project-a")).is_ok());
    }

    #[test]
    fn serialized_fixture_closure_rejects_identity_replay_and_response_corruption() {
        for (request, response) in valid_pairs() {
            let request_json = serde_json::to_value(&request).unwrap();
            let response_json = serde_json::to_value(&response).unwrap();
            let parsed_request: RequestEnvelope = serde_json::from_value(request_json).unwrap();
            let parsed_response: MutationResponse = serde_json::from_value(response_json).unwrap();
            validate_request(&parsed_request, &parsed_response).unwrap();
        }

        let (request, response) = valid_pairs()[0].clone();
        let mut response_value = serde_json::to_value(response).unwrap();
        response_value
            .as_object_mut()
            .unwrap()
            .insert("token".into(), json!("forbidden"));
        assert!(serde_json::from_value::<MutationResponse>(response_value).is_err());

        let mut request_value = serde_json::to_value(request).unwrap();
        request_value["request"]["input"]["project_id"] = json!("wrong-project");
        let wrong_project: RequestEnvelope = serde_json::from_value(request_value).unwrap();
        let response = valid_pairs()[0].1.clone();
        assert!(validate_request(&wrong_project, &response).is_err());
    }
}
