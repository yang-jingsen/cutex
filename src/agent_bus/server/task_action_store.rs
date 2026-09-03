//! Descriptor-bound durable evidence and uncertainty store for worker actions.

use std::collections::{BTreeMap, BTreeSet};
#[cfg(unix)]
use std::ffi::CString;
use std::fs::File;
#[cfg(unix)]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::io;
use std::io::{Read, Write};
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use chrono::{SecondsFormat, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};

use crate::agent_bus::model::{
    TaskWorkerActionKind, TaskWorkerReceiptAbsence, TaskWorkerResolution,
    TaskWorkerResolutionEvidence, TaskWorkerResult,
};
use crate::role_revision::{ReceiptId, Rfc3339, Sha256, StoreRevision};
use crate::task_delivery::{TaskWorkerAuthorizedAction, TaskWorkerAuthorizedOwner};
use crate::task_service::sha256_bytes;

const SNAPSHOT_FILE: &str = "task-worker-action-evidence-v1.json";
const LOCK_FILE: &str = "task-worker-action-evidence-v1.lock";
const TEMP_PREFIX: &str = ".task-worker-action-evidence-v1.tmp-";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum TaskWorkerActionEvidenceStoreSchema {
    #[serde(rename = "cutex/agent-bus-task-worker-store/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum TaskWorkerTransportRecordSchema {
    #[serde(rename = "cutex/agent-bus-task-worker-record/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TaskWorkerTransportCreationState {
    DurablePrepared,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TaskWorkerTransportRecordV1 {
    pub schema: TaskWorkerTransportRecordSchema,
    pub record_id: ReceiptId,
    pub action_key_sha256: Sha256,
    pub canonical_request_sha256: Sha256,
    pub creation_state: TaskWorkerTransportCreationState,
    pub created_at: Rfc3339,
    pub owner: TaskWorkerAuthorizedOwner,
    pub action: TaskWorkerActionKind,
    pub task_id: crate::role_revision::TaskId,
    pub task_revision: crate::role_revision::TaskRevision,
    pub attempt_fence: crate::task_delivery::PilotAttemptFence,
    pub expected_store_revision: StoreRevision,
    pub action_id: ReceiptId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<TaskWorkerResult>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum TaskWorkerUncertaintySchema {
    #[serde(rename = "cutex/agent-bus-task-worker-uncertainty/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TaskWorkerUncertaintyState {
    Pending,
    Resolved,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TaskWorkerUncertaintyFenceV1 {
    pub schema: TaskWorkerUncertaintySchema,
    pub uncertainty_id: ReceiptId,
    pub state: TaskWorkerUncertaintyState,
    pub transport_record_id: ReceiptId,
    pub canonical_request_sha256: Sha256,
    pub action_id: ReceiptId,
    pub action: TaskWorkerActionKind,
    pub task_id: crate::role_revision::TaskId,
    pub task_revision: crate::role_revision::TaskRevision,
    pub attempt_fence: crate::task_delivery::PilotAttemptFence,
    pub expected_store_revision: StoreRevision,
    pub owner: TaskWorkerAuthorizedOwner,
    pub began_at: Rfc3339,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<TaskWorkerResolution>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct TaskWorkerActionEvidenceStoreV1 {
    schema: TaskWorkerActionEvidenceStoreSchema,
    store_revision: StoreRevision,
    records_by_action_key: BTreeMap<Sha256, TaskWorkerTransportRecordV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    uncertainty: Option<TaskWorkerUncertaintyFenceV1>,
}

impl Default for TaskWorkerActionEvidenceStoreV1 {
    fn default() -> Self {
        Self {
            schema: TaskWorkerActionEvidenceStoreSchema::V1,
            store_revision: StoreRevision::new(1).expect("revision one is valid"),
            records_by_action_key: BTreeMap::new(),
            uncertainty: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ActionProbe {
    New,
    Existing(TaskWorkerTransportRecordV1),
    ExactBlocked {
        uncertainty_id: ReceiptId,
        action_id: ReceiptId,
    },
    Blocked,
    Conflict,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreparedTaskWorkerAction {
    pub record: TaskWorkerTransportRecordV1,
    pub uncertainty: TaskWorkerUncertaintyFenceV1,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum EvidenceStoreError {
    InvalidRoot,
    InvalidPrivateFile,
    InvalidSnapshot,
    RootBindingChanged,
    LockUnavailable,
    Serialization,
    RevisionOverflow,
    DefiniteNoWrite,
    PersistenceUnknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PersistFailure {
    Definite,
    Unknown,
}

struct RootHandle {
    path: PathBuf,
    directory: File,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    identity: crate::platform::private_fs::FileIdentity,
}

impl RootHandle {
    fn validate_binding(&self) -> Result<(), EvidenceStoreError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;

            let rebound = open_validated_root(&self.path)
                .map_err(|_| EvidenceStoreError::RootBindingChanged)?;
            let metadata = rebound
                .metadata()
                .map_err(|_| EvidenceStoreError::RootBindingChanged)?;
            if metadata.dev() != self.device || metadata.ino() != self.inode {
                return Err(EvidenceStoreError::RootBindingChanged);
            }
            Ok(())
        }
        #[cfg(windows)]
        {
            crate::platform::private_fs::validate_binding(&self.path, self.identity)
                .map_err(|_| EvidenceStoreError::RootBindingChanged)
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StoreFaultPoint {
    BeforeWrite,
    AfterTempSync,
    AfterRename,
    AfterParentSync,
}

#[derive(Default)]
struct FaultController {
    #[cfg(test)]
    point: Mutex<Option<StoreFaultPoint>>,
}

impl FaultController {
    #[cfg(test)]
    fn new(point: StoreFaultPoint) -> Self {
        Self {
            point: Mutex::new(Some(point)),
        }
    }

    #[cfg(test)]
    fn hit(&self, point: StoreFaultPoint) -> bool {
        let mut configured = self.point.lock().expect("fault mutex");
        if configured.as_ref() == Some(&point) {
            *configured = None;
            true
        } else {
            false
        }
    }

    #[cfg(not(test))]
    fn before_write(&self) -> bool {
        false
    }

    #[cfg(test)]
    fn before_write(&self) -> bool {
        self.hit(StoreFaultPoint::BeforeWrite)
    }

    #[cfg(not(test))]
    fn after_temp_sync(&self) -> bool {
        false
    }

    #[cfg(test)]
    fn after_temp_sync(&self) -> bool {
        self.hit(StoreFaultPoint::AfterTempSync)
    }

    #[cfg(not(test))]
    fn after_rename(&self) -> bool {
        false
    }

    #[cfg(test)]
    fn after_rename(&self) -> bool {
        self.hit(StoreFaultPoint::AfterRename)
    }

    #[cfg(not(test))]
    fn after_parent_sync(&self) -> bool {
        false
    }

    #[cfg(test)]
    fn after_parent_sync(&self) -> bool {
        self.hit(StoreFaultPoint::AfterParentSync)
    }
}

pub(crate) struct TaskWorkerActionEvidenceStore {
    root: Arc<RootHandle>,
    local_boundary: Mutex<()>,
    faults: FaultController,
}

impl TaskWorkerActionEvidenceStore {
    pub(crate) fn open(root: impl Into<PathBuf>) -> Result<Self, EvidenceStoreError> {
        Self::open_inner(root.into(), FaultController::default())
    }

    #[cfg(test)]
    pub(crate) fn open_with_fault(
        root: impl Into<PathBuf>,
        point: StoreFaultPoint,
    ) -> Result<Self, EvidenceStoreError> {
        Self::open_inner(root.into(), FaultController::new(point))
    }

    fn open_inner(root: PathBuf, faults: FaultController) -> Result<Self, EvidenceStoreError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;

            let directory = open_validated_root(&root)?;
            let metadata = directory
                .metadata()
                .map_err(|_| EvidenceStoreError::InvalidRoot)?;
            let store = Self {
                root: Arc::new(RootHandle {
                    path: root,
                    directory,
                    device: metadata.dev(),
                    inode: metadata.ino(),
                }),
                local_boundary: Mutex::new(()),
                faults,
            };
            cleanup_stale_temps(&store.root)?;
            store.snapshot()?;
            Ok(store)
        }
        #[cfg(windows)]
        {
            let (directory, identity) = crate::platform::private_fs::secure_directory(&root)
                .map_err(|_| EvidenceStoreError::InvalidRoot)?;
            let store = Self {
                root: Arc::new(RootHandle {
                    path: root,
                    directory,
                    identity,
                }),
                local_boundary: Mutex::new(()),
                faults,
            };
            cleanup_stale_temps(&store.root)?;
            store.snapshot()?;
            Ok(store)
        }
    }

    pub(crate) fn probe(
        &self,
        action: &TaskWorkerAuthorizedAction,
    ) -> Result<ActionProbe, EvidenceStoreError> {
        let snapshot = self.snapshot()?;
        let key = action_key(&action.request.action_id);
        let canonical = canonical_request_sha256(action)?;
        let record = snapshot.records_by_action_key.get(&key);
        if record.is_some_and(|record| record.canonical_request_sha256 != canonical) {
            return Ok(ActionProbe::Conflict);
        }
        if let Some(uncertainty) = snapshot.uncertainty {
            let exact = record.is_some_and(|record| {
                uncertainty.transport_record_id == record.record_id
                    && uncertainty.action_id == action.request.action_id
                    && uncertainty.canonical_request_sha256 == canonical
                    && uncertainty.owner == action.owner
            });
            return Ok(if exact {
                ActionProbe::ExactBlocked {
                    uncertainty_id: uncertainty.uncertainty_id,
                    action_id: uncertainty.action_id,
                }
            } else {
                ActionProbe::Blocked
            });
        }
        Ok(match record {
            Some(record) => ActionProbe::Existing(record.clone()),
            None => ActionProbe::New,
        })
    }

    pub(crate) fn contains_action_id(&self, action_id: &ReceiptId) -> bool {
        let Ok(snapshot) = self.snapshot() else {
            return false;
        };
        snapshot
            .records_by_action_key
            .contains_key(&action_key(action_id))
    }

    pub(crate) fn prepare(
        &self,
        action: &TaskWorkerAuthorizedAction,
    ) -> Result<PreparedTaskWorkerAction, EvidenceStoreError> {
        self.mutate(|current| {
            if current.uncertainty.is_some() {
                return Err(EvidenceStoreError::DefiniteNoWrite);
            }
            let key = action_key(&action.request.action_id);
            let canonical_request_sha256 = canonical_request_sha256(action)?;
            let record = match current.records_by_action_key.get(&key) {
                Some(record) if record.canonical_request_sha256 == canonical_request_sha256 => {
                    record.clone()
                }
                Some(_) => return Err(EvidenceStoreError::DefiniteNoWrite),
                None => {
                    let record = TaskWorkerTransportRecordV1 {
                        schema: TaskWorkerTransportRecordSchema::V1,
                        record_id: server_uuid(),
                        action_key_sha256: key.clone(),
                        canonical_request_sha256: canonical_request_sha256.clone(),
                        creation_state: TaskWorkerTransportCreationState::DurablePrepared,
                        created_at: now(),
                        owner: action.owner.clone(),
                        action: action.request.action,
                        task_id: action.request.task_id.clone(),
                        task_revision: action.request.task_revision,
                        attempt_fence: action.request.attempt_fence.clone(),
                        expected_store_revision: action.request.expected_store_revision,
                        action_id: action.request.action_id.clone(),
                        result: action.request.result.clone(),
                    };
                    current.records_by_action_key.insert(key, record.clone());
                    record
                }
            };
            let uncertainty = TaskWorkerUncertaintyFenceV1 {
                schema: TaskWorkerUncertaintySchema::V1,
                uncertainty_id: server_uuid(),
                state: TaskWorkerUncertaintyState::Pending,
                transport_record_id: record.record_id.clone(),
                canonical_request_sha256: record.canonical_request_sha256.clone(),
                action_id: record.action_id.clone(),
                action: record.action,
                task_id: record.task_id.clone(),
                task_revision: record.task_revision,
                attempt_fence: record.attempt_fence.clone(),
                expected_store_revision: record.expected_store_revision,
                owner: record.owner.clone(),
                began_at: now(),
                resolution: None,
            };
            current.uncertainty = Some(uncertainty.clone());
            Ok(PreparedTaskWorkerAction {
                record,
                uncertainty,
            })
        })
    }

    pub(crate) fn clear_known(
        &self,
        prepared: &PreparedTaskWorkerAction,
    ) -> Result<(), EvidenceStoreError> {
        self.mutate(|current| {
            let Some(uncertainty) = current.uncertainty.as_ref() else {
                return Err(EvidenceStoreError::DefiniteNoWrite);
            };
            if uncertainty.uncertainty_id != prepared.uncertainty.uncertainty_id
                || uncertainty.state != TaskWorkerUncertaintyState::Pending
                || uncertainty.transport_record_id != prepared.record.record_id
            {
                return Err(EvidenceStoreError::DefiniteNoWrite);
            }
            current.uncertainty = None;
            Ok(())
        })
    }

    pub(crate) fn uncertainty(
        &self,
    ) -> Result<Option<TaskWorkerUncertaintyFenceV1>, EvidenceStoreError> {
        Ok(self.snapshot()?.uncertainty)
    }

    pub(crate) fn record_for_uncertainty(
        &self,
        uncertainty: &TaskWorkerUncertaintyFenceV1,
    ) -> Result<TaskWorkerTransportRecordV1, EvidenceStoreError> {
        let snapshot = self.snapshot()?;
        snapshot
            .records_by_action_key
            .get(&action_key(&uncertainty.action_id))
            .filter(|record| record.record_id == uncertainty.transport_record_id)
            .cloned()
            .ok_or(EvidenceStoreError::InvalidSnapshot)
    }

    pub(crate) fn resolve(
        &self,
        uncertainty_id: &ReceiptId,
        action_id: &ReceiptId,
        evidence: TaskWorkerResolutionEvidence,
    ) -> Result<TaskWorkerResolution, EvidenceStoreError> {
        self.mutate(|current| {
            let uncertainty = current
                .uncertainty
                .as_mut()
                .ok_or(EvidenceStoreError::DefiniteNoWrite)?;
            if &uncertainty.uncertainty_id != uncertainty_id || &uncertainty.action_id != action_id
            {
                return Err(EvidenceStoreError::DefiniteNoWrite);
            }
            if let Some(resolution) = uncertainty.resolution.as_ref() {
                return Ok(resolution.clone());
            }
            if uncertainty.state != TaskWorkerUncertaintyState::Pending {
                return Err(EvidenceStoreError::InvalidSnapshot);
            }
            let mut resolution = TaskWorkerResolution {
                resolution_id: server_uuid(),
                resolution_sha256: crate::task_service::zero_sha256(),
                resolved_at: now(),
                evidence,
            };
            resolution.resolution_sha256 = resolution_sha256(uncertainty, &resolution)?;
            uncertainty.state = TaskWorkerUncertaintyState::Resolved;
            uncertainty.resolution = Some(resolution.clone());
            Ok(resolution)
        })
    }

    pub(crate) fn ack(
        &self,
        uncertainty_id: &ReceiptId,
        action_id: &ReceiptId,
        resolution_id: &ReceiptId,
        resolution_sha256: &Sha256,
    ) -> Result<(), EvidenceStoreError> {
        self.mutate(|current| {
            let uncertainty = current
                .uncertainty
                .as_ref()
                .ok_or(EvidenceStoreError::DefiniteNoWrite)?;
            let resolution = uncertainty
                .resolution
                .as_ref()
                .ok_or(EvidenceStoreError::DefiniteNoWrite)?;
            if uncertainty.state != TaskWorkerUncertaintyState::Resolved
                || &uncertainty.uncertainty_id != uncertainty_id
                || &uncertainty.action_id != action_id
                || &resolution.resolution_id != resolution_id
                || &resolution.resolution_sha256 != resolution_sha256
            {
                return Err(EvidenceStoreError::DefiniteNoWrite);
            }
            current.uncertainty = None;
            Ok(())
        })
    }

    #[allow(dead_code)]
    pub(crate) fn record_by_id(
        &self,
        record_id: &ReceiptId,
    ) -> Result<Option<TaskWorkerTransportRecordV1>, EvidenceStoreError> {
        Ok(self
            .snapshot()?
            .records_by_action_key
            .into_values()
            .find(|record| &record.record_id == record_id))
    }

    fn snapshot(&self) -> Result<TaskWorkerActionEvidenceStoreV1, EvidenceStoreError> {
        self.with_locked_snapshot(|snapshot| Ok(snapshot))
    }

    fn mutate<T>(
        &self,
        mutator: impl FnOnce(&mut TaskWorkerActionEvidenceStoreV1) -> Result<T, EvidenceStoreError>,
    ) -> Result<T, EvidenceStoreError> {
        let _boundary = self
            .local_boundary
            .lock()
            .map_err(|_| EvidenceStoreError::LockUnavailable)?;
        self.root.validate_binding()?;
        let lock = open_lock(&self.root)?;
        FileExt::lock_exclusive(&lock).map_err(|_| EvidenceStoreError::LockUnavailable)?;
        let mut current = load_snapshot(&self.root)?.unwrap_or_default();
        validate_snapshot(&current)?;
        let result = mutator(&mut current)?;
        current.store_revision = current
            .store_revision
            .checked_next()
            .map_err(|_| EvidenceStoreError::RevisionOverflow)?;
        validate_snapshot(&current)?;
        let bytes = serde_json::to_vec(&current).map_err(|_| EvidenceStoreError::Serialization)?;
        match persist_snapshot(&self.root, &bytes, &self.faults) {
            Ok(()) => Ok(result),
            Err(PersistFailure::Definite) => Err(EvidenceStoreError::DefiniteNoWrite),
            Err(PersistFailure::Unknown) => Err(EvidenceStoreError::PersistenceUnknown),
        }
    }

    fn with_locked_snapshot<T>(
        &self,
        observer: impl FnOnce(TaskWorkerActionEvidenceStoreV1) -> Result<T, EvidenceStoreError>,
    ) -> Result<T, EvidenceStoreError> {
        let _boundary = self
            .local_boundary
            .lock()
            .map_err(|_| EvidenceStoreError::LockUnavailable)?;
        self.root.validate_binding()?;
        let lock = open_lock(&self.root)?;
        FileExt::lock_shared(&lock).map_err(|_| EvidenceStoreError::LockUnavailable)?;
        let snapshot = load_snapshot(&self.root)?.unwrap_or_default();
        validate_snapshot(&snapshot)?;
        observer(snapshot)
    }
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct CanonicalRequestMaterial<'a> {
    schema: &'static str,
    owner: &'a TaskWorkerAuthorizedOwner,
    wire_schema: crate::agent_bus::model::TaskWorkerActionSchema,
    action: TaskWorkerActionKind,
    task_id: &'a crate::role_revision::TaskId,
    task_revision: crate::role_revision::TaskRevision,
    attempt_fence: &'a crate::task_delivery::PilotAttemptFence,
    expected_store_revision: StoreRevision,
    action_id: &'a ReceiptId,
    result: &'a Option<TaskWorkerResult>,
}

fn canonical_request_sha256(
    action: &TaskWorkerAuthorizedAction,
) -> Result<Sha256, EvidenceStoreError> {
    let bytes = serde_json::to_vec(&CanonicalRequestMaterial {
        schema: "cutex/agent-bus-task-worker-canonical-request/v1",
        owner: &action.owner,
        wire_schema: action.request.schema,
        action: action.request.action,
        task_id: &action.request.task_id,
        task_revision: action.request.task_revision,
        attempt_fence: &action.request.attempt_fence,
        expected_store_revision: action.request.expected_store_revision,
        action_id: &action.request.action_id,
        result: &action.request.result,
    })
    .map_err(|_| EvidenceStoreError::Serialization)?;
    Ok(domain_sha256(
        b"cutex/agent-bus-task-worker-canonical-request/v1\0",
        &bytes,
    ))
}

fn action_key(action_id: &ReceiptId) -> Sha256 {
    domain_sha256(
        b"cutex/agent-bus-task-worker-action-id/v1\0",
        action_id.as_str().as_bytes(),
    )
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct ResolutionDigestMaterial<'a> {
    schema: &'static str,
    uncertainty_id: &'a ReceiptId,
    transport_record_id: &'a ReceiptId,
    canonical_request_sha256: &'a Sha256,
    action_id: &'a ReceiptId,
    action: TaskWorkerActionKind,
    task_id: &'a crate::role_revision::TaskId,
    task_revision: crate::role_revision::TaskRevision,
    attempt_fence: &'a crate::task_delivery::PilotAttemptFence,
    expected_store_revision: StoreRevision,
    owner: &'a TaskWorkerAuthorizedOwner,
    resolution_id: &'a ReceiptId,
    resolved_at: &'a Rfc3339,
    evidence: &'a TaskWorkerResolutionEvidence,
}

fn resolution_sha256(
    uncertainty: &TaskWorkerUncertaintyFenceV1,
    resolution: &TaskWorkerResolution,
) -> Result<Sha256, EvidenceStoreError> {
    let bytes = serde_json::to_vec(&ResolutionDigestMaterial {
        schema: "cutex/agent-bus-task-worker-resolution/v1",
        uncertainty_id: &uncertainty.uncertainty_id,
        transport_record_id: &uncertainty.transport_record_id,
        canonical_request_sha256: &uncertainty.canonical_request_sha256,
        action_id: &uncertainty.action_id,
        action: uncertainty.action,
        task_id: &uncertainty.task_id,
        task_revision: uncertainty.task_revision,
        attempt_fence: &uncertainty.attempt_fence,
        expected_store_revision: uncertainty.expected_store_revision,
        owner: &uncertainty.owner,
        resolution_id: &resolution.resolution_id,
        resolved_at: &resolution.resolved_at,
        evidence: &resolution.evidence,
    })
    .map_err(|_| EvidenceStoreError::Serialization)?;
    Ok(domain_sha256(
        b"cutex/agent-bus-task-worker-resolution/v1\0",
        &bytes,
    ))
}

fn domain_sha256(domain: &[u8], bytes: &[u8]) -> Sha256 {
    let mut material = Vec::with_capacity(domain.len() + bytes.len());
    material.extend_from_slice(domain);
    material.extend_from_slice(bytes);
    sha256_bytes(&material)
}

fn validate_snapshot(store: &TaskWorkerActionEvidenceStoreV1) -> Result<(), EvidenceStoreError> {
    if store.schema != TaskWorkerActionEvidenceStoreSchema::V1 {
        return Err(EvidenceStoreError::InvalidSnapshot);
    }
    let mut record_ids = BTreeSet::new();
    for (key, record) in &store.records_by_action_key {
        if record.schema != TaskWorkerTransportRecordSchema::V1
            || record.creation_state != TaskWorkerTransportCreationState::DurablePrepared
            || key != &record.action_key_sha256
            || key != &action_key(&record.action_id)
            || !is_server_uuid(&record.record_id)
            || !record_ids.insert(record.record_id.clone())
            || record.task_id != record.attempt_fence.task_id
            || record.task_revision != record.attempt_fence.task_revision
            || record.owner.sender_runtime_agent_id != record.attempt_fence.owner.runtime_agent_id
            || record.owner.sender_cutex_session_id != record.attempt_fence.owner.cutex_session_id
            || record.owner.sender_durable_revision != record.attempt_fence.owner.durable_revision
            || record.owner.sender_runtime_generation
                != record.attempt_fence.owner.runtime_generation
        {
            return Err(EvidenceStoreError::InvalidSnapshot);
        }
        validate_result_shape(record.action, record.result.as_ref())?;
        let authorized = authorized_from_record(record)?;
        if canonical_request_sha256(&authorized)? != record.canonical_request_sha256 {
            return Err(EvidenceStoreError::InvalidSnapshot);
        }
    }
    if let Some(uncertainty) = &store.uncertainty {
        if uncertainty.schema != TaskWorkerUncertaintySchema::V1
            || !is_server_uuid(&uncertainty.uncertainty_id)
            || record_ids.contains(&uncertainty.uncertainty_id)
        {
            return Err(EvidenceStoreError::InvalidSnapshot);
        }
        let record = store
            .records_by_action_key
            .get(&action_key(&uncertainty.action_id))
            .ok_or(EvidenceStoreError::InvalidSnapshot)?;
        if uncertainty.transport_record_id != record.record_id
            || uncertainty.canonical_request_sha256 != record.canonical_request_sha256
            || uncertainty.action != record.action
            || uncertainty.task_id != record.task_id
            || uncertainty.task_revision != record.task_revision
            || uncertainty.attempt_fence != record.attempt_fence
            || uncertainty.expected_store_revision != record.expected_store_revision
            || uncertainty.owner != record.owner
        {
            return Err(EvidenceStoreError::InvalidSnapshot);
        }
        match (uncertainty.state, uncertainty.resolution.as_ref()) {
            (TaskWorkerUncertaintyState::Pending, None) => {}
            (TaskWorkerUncertaintyState::Resolved, Some(resolution)) => {
                if !is_server_uuid(&resolution.resolution_id)
                    || resolution.resolution_id == uncertainty.uncertainty_id
                    || record_ids.contains(&resolution.resolution_id)
                    || resolution_sha256(uncertainty, resolution)? != resolution.resolution_sha256
                {
                    return Err(EvidenceStoreError::InvalidSnapshot);
                }
                match &resolution.evidence {
                    TaskWorkerResolutionEvidence::Committed(evidence)
                        if evidence.receipt.action_id == record.action_id
                            && evidence.receipt.task_id == record.task_id
                            && evidence.receipt.task_revision == record.task_revision
                            && evidence.receipt.transport_record_id == record.record_id
                            && evidence.event_cursor.sequence
                                <= evidence.observed_journal_cursor.sequence
                            && evidence.receipt.committed_store_revision
                                <= evidence.observed_store_revision => {}
                    TaskWorkerResolutionEvidence::Absent(TaskWorkerReceiptAbsence { .. }) => {}
                    _ => return Err(EvidenceStoreError::InvalidSnapshot),
                }
            }
            _ => return Err(EvidenceStoreError::InvalidSnapshot),
        }
    }
    Ok(())
}

fn authorized_from_record(
    record: &TaskWorkerTransportRecordV1,
) -> Result<TaskWorkerAuthorizedAction, EvidenceStoreError> {
    let result_bytes = match record.result.as_ref() {
        None => None,
        Some(TaskWorkerResult::Utf8 { text, sha256 }) => {
            let bytes = text.as_bytes().to_vec();
            if sha256_bytes(&bytes) != *sha256 {
                return Err(EvidenceStoreError::InvalidSnapshot);
            }
            Some(bytes)
        }
        Some(TaskWorkerResult::Base64 { data, sha256 }) => {
            let bytes = BASE64
                .decode(data.as_bytes())
                .map_err(|_| EvidenceStoreError::InvalidSnapshot)?;
            if BASE64.encode(&bytes) != *data || sha256_bytes(&bytes) != *sha256 {
                return Err(EvidenceStoreError::InvalidSnapshot);
            }
            Some(bytes)
        }
    };
    Ok(TaskWorkerAuthorizedAction {
        request: crate::agent_bus::model::TaskWorkerActionRequest {
            schema: crate::agent_bus::model::TaskWorkerActionSchema::V1,
            action: record.action,
            task_id: record.task_id.clone(),
            task_revision: record.task_revision,
            attempt_fence: record.attempt_fence.clone(),
            expected_store_revision: record.expected_store_revision,
            action_id: record.action_id.clone(),
            result: record.result.clone(),
        },
        result_bytes,
        owner: record.owner.clone(),
    })
}

fn validate_result_shape(
    action: TaskWorkerActionKind,
    result: Option<&TaskWorkerResult>,
) -> Result<(), EvidenceStoreError> {
    match (action, result) {
        (TaskWorkerActionKind::Accept | TaskWorkerActionKind::Start, None) => Ok(()),
        (TaskWorkerActionKind::Complete, Some(_)) => Ok(()),
        _ => Err(EvidenceStoreError::InvalidSnapshot),
    }
}

fn now() -> Rfc3339 {
    Rfc3339::new(Utc::now().to_rfc3339_opts(SecondsFormat::AutoSi, true))
        .expect("UTC timestamp is normalized")
}

fn server_uuid() -> ReceiptId {
    ReceiptId::new(uuid::Uuid::new_v4().to_string()).expect("UUID is a valid receipt ID")
}

fn is_server_uuid(id: &ReceiptId) -> bool {
    uuid::Uuid::parse_str(id.as_str())
        .map(|parsed| parsed.to_string() == id.as_str())
        .unwrap_or(false)
}

#[cfg(unix)]
fn open_validated_root(root: &Path) -> Result<File, EvidenceStoreError> {
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let directory = options
            .open(root)
            .map_err(|_| EvidenceStoreError::InvalidRoot)?;
        let metadata = directory
            .metadata()
            .map_err(|_| EvidenceStoreError::InvalidRoot)?;
        if !metadata.file_type().is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o7777 != 0o700
        {
            return Err(EvidenceStoreError::InvalidRoot);
        }
        Ok(directory)
    }
}

fn open_lock(root: &RootHandle) -> Result<File, EvidenceStoreError> {
    root.validate_binding()?;
    let file = open_child(root, LOCK_FILE, libc::O_RDWR | libc::O_CREAT, 0o600)?;
    validate_private_file(&file)?;
    sync_root(root)?;
    Ok(file)
}

fn cleanup_stale_temps(root: &RootHandle) -> Result<(), EvidenceStoreError> {
    root.validate_binding()?;
    let lock = open_lock(root)?;
    FileExt::lock_exclusive(&lock).map_err(|_| EvidenceStoreError::LockUnavailable)?;
    let mut removed = false;
    for entry in std::fs::read_dir(&root.path).map_err(|_| EvidenceStoreError::InvalidRoot)? {
        let entry = entry.map_err(|_| EvidenceStoreError::InvalidRoot)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(TEMP_PREFIX) {
            continue;
        }
        let file = open_child(root, &name, libc::O_RDONLY, 0)?;
        validate_private_file(&file)?;
        drop(file);
        unlink_child(root, &name)?;
        removed = true;
    }
    if removed {
        sync_root(root)?;
    }
    Ok(())
}

fn load_snapshot(
    root: &RootHandle,
) -> Result<Option<TaskWorkerActionEvidenceStoreV1>, EvidenceStoreError> {
    root.validate_binding()?;
    let mut file = match open_child(root, SNAPSHOT_FILE, libc::O_RDONLY, 0) {
        Ok(file) => file,
        Err(EvidenceStoreError::DefiniteNoWrite) => return Ok(None),
        Err(error) => return Err(error),
    };
    validate_private_file(&file)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| EvidenceStoreError::InvalidSnapshot)?;
    let snapshot: TaskWorkerActionEvidenceStoreV1 =
        serde_json::from_slice(&bytes).map_err(|_| EvidenceStoreError::InvalidSnapshot)?;
    validate_snapshot(&snapshot)?;
    if serde_json::to_vec(&snapshot).map_err(|_| EvidenceStoreError::Serialization)? != bytes {
        return Err(EvidenceStoreError::InvalidSnapshot);
    }
    Ok(Some(snapshot))
}

fn persist_snapshot(
    root: &RootHandle,
    bytes: &[u8],
    faults: &FaultController,
) -> Result<(), PersistFailure> {
    if faults.before_write() {
        return Err(PersistFailure::Definite);
    }
    let temp = format!(
        "{TEMP_PREFIX}{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    );
    let mut renamed = false;
    let result = (|| {
        root.validate_binding()
            .map_err(|_| PersistFailure::Definite)?;
        let mut file = open_child(
            root,
            &temp,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            0o600,
        )
        .map_err(|_| PersistFailure::Definite)?;
        validate_private_file(&file).map_err(|_| PersistFailure::Definite)?;
        file.write_all(bytes)
            .map_err(|_| PersistFailure::Definite)?;
        file.sync_all().map_err(|_| PersistFailure::Definite)?;
        if faults.after_temp_sync() {
            return Err(PersistFailure::Definite);
        }
        drop(file);
        root.validate_binding()
            .map_err(|_| PersistFailure::Definite)?;
        rename_child(root, &temp, SNAPSHOT_FILE).map_err(|_| PersistFailure::Definite)?;
        renamed = true;
        if faults.after_rename() {
            return Err(PersistFailure::Unknown);
        }
        sync_root(root).map_err(|_| PersistFailure::Unknown)?;
        if faults.after_parent_sync() {
            return Err(PersistFailure::Unknown);
        }
        Ok(())
    })();
    if !renamed {
        let _ = unlink_child(root, &temp);
    }
    result
}

fn validate_private_file(file: &File) -> Result<(), EvidenceStoreError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let metadata = file
            .metadata()
            .map_err(|_| EvidenceStoreError::InvalidPrivateFile)?;
        if !metadata.file_type().is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o7777 != 0o600
        {
            return Err(EvidenceStoreError::InvalidPrivateFile);
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        crate::platform::private_fs::validate_private_file(file)
            .map_err(|_| EvidenceStoreError::InvalidPrivateFile)
    }
}

#[cfg(unix)]
type PlatformMode = libc::mode_t;
#[cfg(not(unix))]
type PlatformMode = u32;

fn open_child(
    root: &RootHandle,
    name: &str,
    flags: libc::c_int,
    mode: PlatformMode,
) -> Result<File, EvidenceStoreError> {
    #[cfg(unix)]
    {
        let name = CString::new(name).map_err(|_| EvidenceStoreError::DefiniteNoWrite)?;
        let descriptor = unsafe {
            libc::openat(
                root.directory.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                mode,
            )
        };
        if descriptor < 0 {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::NotFound {
                Err(EvidenceStoreError::DefiniteNoWrite)
            } else {
                Err(EvidenceStoreError::InvalidPrivateFile)
            };
        }
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }
    #[cfg(windows)]
    {
        let _ = mode;
        crate::platform::private_fs::open_child(&root.path, root.identity, name, flags, true)
            .map_err(|error| {
                if error.io_kind() == std::io::ErrorKind::NotFound {
                    EvidenceStoreError::DefiniteNoWrite
                } else {
                    EvidenceStoreError::InvalidPrivateFile
                }
            })
    }
}

fn rename_child(root: &RootHandle, source: &str, target: &str) -> Result<(), EvidenceStoreError> {
    #[cfg(unix)]
    {
        let source = CString::new(source).map_err(|_| EvidenceStoreError::DefiniteNoWrite)?;
        let target = CString::new(target).map_err(|_| EvidenceStoreError::DefiniteNoWrite)?;
        let result = unsafe {
            libc::renameat(
                root.directory.as_raw_fd(),
                source.as_ptr(),
                root.directory.as_raw_fd(),
                target.as_ptr(),
            )
        };
        if result != 0 {
            return Err(EvidenceStoreError::InvalidPrivateFile);
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        crate::platform::private_fs::replace_child(&root.path, root.identity, source, target)
            .map_err(|_| EvidenceStoreError::InvalidPrivateFile)
    }
}

fn unlink_child(root: &RootHandle, name: &str) -> Result<(), EvidenceStoreError> {
    #[cfg(unix)]
    {
        let name = CString::new(name).map_err(|_| EvidenceStoreError::DefiniteNoWrite)?;
        let result = unsafe { libc::unlinkat(root.directory.as_raw_fd(), name.as_ptr(), 0) };
        if result != 0 && io::Error::last_os_error().kind() != io::ErrorKind::NotFound {
            return Err(EvidenceStoreError::InvalidPrivateFile);
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        crate::platform::private_fs::unlink_child(&root.path, root.identity, name)
            .map_err(|_| EvidenceStoreError::InvalidPrivateFile)
    }
}

fn sync_root(root: &RootHandle) -> Result<(), EvidenceStoreError> {
    root.validate_binding()?;
    #[cfg(unix)]
    {
        root.directory
            .sync_all()
            .map_err(|_| EvidenceStoreError::PersistenceUnknown)
    }
    #[cfg(windows)]
    {
        crate::platform::private_fs::sync_directory(&root.directory)
            .map_err(|_| EvidenceStoreError::PersistenceUnknown)
    }
}

pub(crate) fn authorized_action_from_record(
    record: &TaskWorkerTransportRecordV1,
) -> Result<TaskWorkerAuthorizedAction, EvidenceStoreError> {
    authorized_from_record(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_bus::model::{TaskWorkerActionRequest, TaskWorkerActionSchema};
    use crate::role_revision::{
        AttemptNumber, CutexSessionId, DurableRevision, RuntimeAgentId, RuntimeGeneration, TaskId,
        TaskRevision,
    };
    use crate::task_delivery::{AttemptToken, PilotAttemptFence, PilotOwnerSnapshot};

    fn private_root(label: &str) -> PathBuf {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        #[cfg(windows)]
        crate::platform::private_fs::secure_directory(&root).unwrap();
        root
    }

    fn action() -> TaskWorkerAuthorizedAction {
        let task_id = TaskId::new("store-task").unwrap();
        let task_revision = TaskRevision::new(1).unwrap();
        let runtime = RuntimeAgentId::new("store-runtime").unwrap();
        let owner = PilotOwnerSnapshot {
            cutex_session_id: CutexSessionId::new("cutex.store").unwrap(),
            durable_revision: DurableRevision::new(7).unwrap(),
            runtime_agent_id: runtime.clone(),
            runtime_generation: RuntimeGeneration::new(3).unwrap(),
        };
        TaskWorkerAuthorizedAction {
            request: TaskWorkerActionRequest {
                schema: TaskWorkerActionSchema::V1,
                action: TaskWorkerActionKind::Accept,
                task_id: task_id.clone(),
                task_revision,
                attempt_fence: PilotAttemptFence {
                    task_id,
                    task_revision,
                    attempt_number: AttemptNumber::new(1).unwrap(),
                    attempt_token: AttemptToken::new("store-attempt").unwrap(),
                    owner: owner.clone(),
                },
                expected_store_revision: StoreRevision::new(4).unwrap(),
                action_id: ReceiptId::new("store-action").unwrap(),
                result: None,
            },
            result_bytes: None,
            owner: TaskWorkerAuthorizedOwner {
                sender_runtime_agent_id: runtime,
                sender_roster_session_id: "store-roster".to_string(),
                sender_cutex_session_id: owner.cutex_session_id,
                sender_durable_revision: owner.durable_revision,
                sender_runtime_generation: owner.runtime_generation,
            },
        }
    }

    #[test]
    fn task_action_store_t_exec_evidence_idempotency_and_restart() {
        let root = private_root("T-EXEC-EVIDENCE-IDEMPOTENCY");
        let store = TaskWorkerActionEvidenceStore::open(&root).unwrap();
        let action = action();
        let prepared = store.prepare(&action).unwrap();
        let record_id = prepared.record.record_id.clone();
        drop(store);
        let reopened = TaskWorkerActionEvidenceStore::open(&root).unwrap();
        assert_eq!(
            reopened.record_by_id(&record_id).unwrap().unwrap(),
            prepared.record
        );
        assert!(matches!(
            reopened.probe(&action).unwrap(),
            ActionProbe::ExactBlocked { .. }
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn task_action_store_t_exec_store_prepare_crash_matrix() {
        for point in [
            StoreFaultPoint::BeforeWrite,
            StoreFaultPoint::AfterTempSync,
            StoreFaultPoint::AfterRename,
            StoreFaultPoint::AfterParentSync,
        ] {
            let root = private_root("T-EXEC-STORE-PREPARE-CRASH");
            let store = TaskWorkerActionEvidenceStore::open_with_fault(&root, point).unwrap();
            let result = store.prepare(&action());
            assert!(result.is_err());
            drop(store);
            let reopened = TaskWorkerActionEvidenceStore::open(&root).unwrap();
            if let Some(fence) = reopened.uncertainty().unwrap() {
                assert_eq!(fence.state, TaskWorkerUncertaintyState::Pending);
                assert!(reopened
                    .record_by_id(&fence.transport_record_id)
                    .unwrap()
                    .is_some());
            }
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn task_action_store_t_exec_evidence_nonterminal_records_are_distinct() {
        let root = private_root("T-EXEC-EVIDENCE-NONTERMINAL");
        let store = TaskWorkerActionEvidenceStore::open(&root).unwrap();
        let accept = action();
        let accepted = store.prepare(&accept).unwrap();
        assert!(accepted.record.result.is_none());
        store.clear_known(&accepted).unwrap();
        let mut start = accept.clone();
        start.request.action = TaskWorkerActionKind::Start;
        start.request.action_id = ReceiptId::new("store-start-action").unwrap();
        start.request.expected_store_revision = StoreRevision::new(5).unwrap();
        let started = store.prepare(&start).unwrap();
        assert!(started.record.result.is_none());
        assert_ne!(accepted.record.record_id, started.record.record_id);
        assert_ne!(
            accepted.record.canonical_request_sha256,
            started.record.canonical_request_sha256
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn task_action_store_t_exec_evidence_complete_recovers_exact_tagged_bytes() {
        let root = private_root("T-EXEC-EVIDENCE-COMPLETE");
        let store = TaskWorkerActionEvidenceStore::open(&root).unwrap();
        let mut utf8 = action();
        let utf8_bytes = "opaque result ∎".as_bytes().to_vec();
        utf8.request.action = TaskWorkerActionKind::Complete;
        utf8.request.action_id = ReceiptId::new("store-complete-utf8").unwrap();
        utf8.request.result = Some(TaskWorkerResult::Utf8 {
            text: String::from_utf8(utf8_bytes.clone()).unwrap(),
            sha256: sha256_bytes(&utf8_bytes),
        });
        utf8.result_bytes = Some(utf8_bytes.clone());
        let utf8_prepared = store.prepare(&utf8).unwrap();
        let utf8_id = utf8_prepared.record.record_id.clone();
        store.clear_known(&utf8_prepared).unwrap();

        let mut binary = action();
        let binary_bytes = vec![0, 255, 17, 42, 128];
        binary.request.action = TaskWorkerActionKind::Complete;
        binary.request.action_id = ReceiptId::new("store-complete-base64").unwrap();
        binary.request.result = Some(TaskWorkerResult::Base64 {
            data: base64::engine::general_purpose::STANDARD.encode(&binary_bytes),
            sha256: sha256_bytes(&binary_bytes),
        });
        binary.result_bytes = Some(binary_bytes.clone());
        let binary_prepared = store.prepare(&binary).unwrap();
        let binary_id = binary_prepared.record.record_id.clone();
        drop(store);

        let reopened = TaskWorkerActionEvidenceStore::open(&root).unwrap();
        let utf8_record = reopened.record_by_id(&utf8_id).unwrap().unwrap();
        let binary_record = reopened.record_by_id(&binary_id).unwrap().unwrap();
        assert_eq!(
            authorized_action_from_record(&utf8_record)
                .unwrap()
                .result_bytes,
            Some(utf8_bytes)
        );
        assert_eq!(
            authorized_action_from_record(&binary_record)
                .unwrap()
                .result_bytes,
            Some(binary_bytes)
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
