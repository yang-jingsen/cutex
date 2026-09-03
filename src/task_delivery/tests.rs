use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::json;

use super::agent_bus_adapter::{AgentBusAdapter, TaskDeliveryEnvelopeV1};
use super::agent_bus_adapter::{
    AgentBusBoundary, AgentBusBoundaryError, SessionSnapshotBoundary, SessionSnapshotError,
};
use super::*;
use crate::agent_bus::model::{
    AgentBusSendRequest, TaskWorkerActionKind, TaskWorkerActionRequest, TaskWorkerActionSchema,
    TaskWorkerResult,
};
use crate::session::model::{CutexSessionRecord, CutexSessionStore};

const SESSION_ID: &str = "cutex:pilot:worker";
const RUNTIME_ID: &str = "cutex:runtime:worker:1";
const MESSAGE_ID: &str = "00000000-0000-4000-8000-000000000104";

fn typed_id<T>(
    value: &str,
    constructor: impl FnOnce(String) -> Result<T, crate::role_revision::ValueError>,
) -> T {
    constructor(value.to_string()).expect("typed fixture ID")
}

fn task_id(value: &str) -> TaskId {
    typed_id(value, TaskId::new)
}

fn task_revision(value: u64) -> TaskRevision {
    TaskRevision::new(value).expect("task revision")
}

fn receipt_id(value: &str) -> ReceiptId {
    typed_id(value, ReceiptId::new)
}

fn delivery_id(value: &str) -> DeliveryId {
    typed_id(value, DeliveryId::new)
}

fn store_revision(value: u64) -> PilotStoreRevision {
    PilotStoreRevision::new(value).expect("store revision")
}

fn service_root(label: &str) -> PathBuf {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    let root = std::env::temp_dir().join(format!("{label}-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).expect("create private pilot root");
    #[cfg(unix)]
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
        .expect("set private pilot root mode");
    #[cfg(windows)]
    crate::platform::private_fs::secure_directory(&root).expect("set private pilot root DACL");
    root
}

fn owner() -> PilotOwnerSnapshot {
    PilotOwnerSnapshot {
        cutex_session_id: typed_id(SESSION_ID, CutexSessionId::new),
        durable_revision: DurableRevision::new(7).expect("durable revision"),
        runtime_agent_id: typed_id(RUNTIME_ID, RuntimeAgentId::new),
        runtime_generation: RuntimeGeneration::new(3).expect("runtime generation"),
    }
}

fn publish_request(task: &str, expected_store_revision: u64) -> PilotPublishRequest {
    let opaque_contract =
        format!("{{\"task\":\"{task}\",\"instruction\":\"deliver exactly once\"}}");
    PilotPublishRequest {
        specification: PilotTaskSpecification {
            task_id: task_id(task),
            task_revision: task_revision(1),
            contract_sha256: crate::task_service::sha256_bytes(opaque_contract.as_bytes()),
            opaque_contract,
        },
        create_receipt_id: receipt_id(&format!("{task}:create")),
        publish_receipt_id: receipt_id(&format!("{task}:publish")),
        expected_store_revision: store_revision(expected_store_revision),
        attempt_token: AttemptToken::new(format!("{task}:attempt:1")).expect("attempt token"),
        owner: owner(),
    }
}

fn open_pilot(root: &Path) -> TaskDeliveryPilot {
    let pilot = TaskDeliveryPilot::open(root).expect("open pilot");
    pilot.recover().expect("recover pilot");
    pilot
}

fn delivery_request(published: PublishedTask, task: &str) -> PilotDeliveryRequest {
    PilotDeliveryRequest::new(
        published,
        delivery_id(&format!("{task}:delivery:action:1")),
        receipt_id(&format!("{task}:delivery:transition:1")),
    )
}

fn session_record() -> CutexSessionRecord {
    let mut record = CutexSessionRecord::new_at(
        SESSION_ID.to_string(),
        Some("codex-session-pilot".to_string()),
        "tethys".to_string(),
        "/tmp/pilot-worker".to_string(),
        Some("aemeath".to_string()),
        "2026-08-22T00:00:00Z".to_string(),
    )
    .expect("session record");
    record.revision = 7;
    record.current_runtime_agent_id = Some(RUNTIME_ID.to_string());
    record.runtime_generation = 3;
    record.agent_enabled = true;
    record
}

fn serialized_session_boundary(record: CutexSessionRecord) -> Arc<SerializedSessionBoundary> {
    let mut store = CutexSessionStore::default();
    store.sessions.insert(SESSION_ID.to_string(), record);
    Arc::new(SerializedSessionBoundary {
        bytes: Mutex::new(serde_json::to_vec(&store).expect("serialize session store")),
        loads: AtomicUsize::new(0),
    })
}

struct SerializedSessionBoundary {
    bytes: Mutex<Vec<u8>>,
    loads: AtomicUsize,
}

impl SerializedSessionBoundary {
    fn loads(&self) -> usize {
        self.loads.load(Ordering::SeqCst)
    }
}

impl SessionSnapshotBoundary for SerializedSessionBoundary {
    fn load(
        &self,
        cutex_session_id: &CutexSessionId,
    ) -> Result<CutexSessionRecord, SessionSnapshotError> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        let bytes = self.bytes.lock().expect("session bytes mutex").clone();
        let store: CutexSessionStore =
            serde_json::from_slice(&bytes).map_err(|_| SessionSnapshotError::Unavailable)?;
        store
            .sessions
            .get(cutex_session_id.as_str())
            .cloned()
            .ok_or(SessionSnapshotError::NotFound)
    }
}

#[derive(Clone, Copy)]
enum BusBehavior {
    Accept,
    Reject,
    Uncertain,
}

struct SerializedAgentBusBoundary {
    behavior: BusBehavior,
    calls: AtomicUsize,
    queue_insertions: AtomicUsize,
    requests: Mutex<Vec<Vec<u8>>>,
    messages_by_body: Mutex<BTreeMap<Vec<u8>, String>>,
}

impl SerializedAgentBusBoundary {
    fn new(behavior: BusBehavior) -> Arc<Self> {
        Arc::new(Self {
            behavior,
            calls: AtomicUsize::new(0),
            queue_insertions: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            messages_by_body: Mutex::new(BTreeMap::new()),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn queue_insertions(&self) -> usize {
        self.queue_insertions.load(Ordering::SeqCst)
    }

    fn request(&self, index: usize) -> AgentBusSendRequest {
        let body = self.requests.lock().expect("request mutex")[index].clone();
        serde_json::from_slice(&body).expect("observed serialized Agent Bus request")
    }
}

impl AgentBusBoundary for SerializedAgentBusBoundary {
    fn send_once(&self, request_body: &[u8]) -> Result<Vec<u8>, AgentBusBoundaryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests
            .lock()
            .expect("request mutex")
            .push(request_body.to_vec());
        match self.behavior {
            BusBehavior::Reject => return Err(AgentBusBoundaryError::Rejected),
            BusBehavior::Uncertain => return Err(AgentBusBoundaryError::Uncertain),
            BusBehavior::Accept => {}
        }
        let observed: AgentBusSendRequest =
            serde_json::from_slice(request_body).expect("real Agent Bus request body");
        let mut messages = self.messages_by_body.lock().expect("dedupe mutex");
        let (message_id, deduplicated) = match messages.get(request_body) {
            Some(message_id) => (message_id.clone(), true),
            None => {
                self.queue_insertions.fetch_add(1, Ordering::SeqCst);
                messages.insert(request_body.to_vec(), MESSAGE_ID.to_string());
                (MESSAGE_ID.to_string(), false)
            }
        };
        serde_json::to_vec(&json!({
            "id": message_id,
            "to": observed.to,
            "to_session_id": observed.to_session_id,
            "delivery_mode": observed.delivery_mode,
            "trigger_turn": observed.trigger_turn,
            "queued": true,
            "deduplicated": deduplicated,
            "external_message_id": observed.external_message_id,
        }))
        .map_err(|_| AgentBusBoundaryError::Uncertain)
    }
}

fn adapter(
    session: Arc<SerializedSessionBoundary>,
    bus: Arc<SerializedAgentBusBoundary>,
) -> AgentBusAdapter {
    AgentBusAdapter::with_boundaries(session, bus)
}

fn root_image(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut image = fs::read_dir(root)
        .expect("read durable root")
        .map(|entry| {
            let entry = entry.expect("root entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            let bytes = if entry.file_type().expect("entry type").is_file() {
                fs::read(entry.path()).expect("read durable file")
            } else {
                Vec::new()
            };
            (name, bytes)
        })
        .collect::<Vec<_>>();
    image.sort_by(|left, right| left.0.cmp(&right.0));
    image
}

fn send_and_deliver(
    pilot: &TaskDeliveryPilot,
    request: &PilotDeliveryRequest,
    session: Arc<SerializedSessionBoundary>,
    bus: Arc<SerializedAgentBusBoundary>,
) -> (AgentBusDeliveryReceiptV1, DeliveredTask) {
    let receipt = adapter(session, bus)
        .send(request)
        .expect("Agent Bus delivery receipt");
    let delivered = pilot
        .deliver(request.clone(), receipt.clone())
        .expect("record durable delivery");
    (receipt, delivered)
}

#[test]
fn t_pilot_publish() {
    const FIXTURE: &str = "T-PILOT-PUBLISH";
    let root = service_root(FIXTURE);
    let pilot = open_pilot(&root);
    let request = publish_request("pilot-publish", 1);

    let published = pilot.publish(request.clone()).expect("publish task");

    assert_eq!(published.specification, request.specification);
    assert_eq!(published.publication_receipt_id, request.publish_receipt_id);
    assert_eq!(published.committed_store_revision, store_revision(3));
    assert_eq!(published.fence.owner, request.owner);
    let observed = pilot
        .task(task_id("pilot-publish"), task_revision(1))
        .expect("task lookup")
        .expect("published task");
    assert_eq!(observed.phase, PilotTaskPhase::Published);
    assert_eq!(observed.fence.as_ref(), Some(&published.fence));
    assert!(observed.agent_bus_message_id.is_none());
    assert_eq!(
        pilot
            .receipt(receipt_id("pilot-publish:publish"))
            .expect("receipt lookup")
            .expect("publish receipt")
            .resulting_phase,
        PilotTaskPhase::Published
    );
}

#[test]
fn t_pilot_stale_runtime() {
    const FIXTURE: &str = "T-PILOT-STALE-RUNTIME";
    let root = service_root(FIXTURE);
    let pilot = open_pilot(&root);
    let published = pilot
        .publish(publish_request("pilot-stale", 1))
        .expect("publish task");
    let request = delivery_request(published, "pilot-stale");
    let before = root_image(&root);
    let mut stale = session_record();
    stale.runtime_generation = 4;
    stale.current_runtime_agent_id = Some("cutex:runtime:worker:2".to_string());
    let sessions = serialized_session_boundary(stale);
    let bus = SerializedAgentBusBoundary::new(BusBehavior::Accept);

    let error = adapter(sessions.clone(), bus.clone())
        .send(&request)
        .expect_err("stale runtime must be a typed no-send");

    assert!(matches!(
        error,
        AgentBusDeliveryError::Precondition(DeliveryPreconditionError::RuntimeAgentMismatch)
    ));
    assert_eq!(sessions.loads(), 1);
    assert_eq!(bus.calls(), 0);
    assert_eq!(root_image(&root), before);
    assert_eq!(
        pilot
            .task(task_id("pilot-stale"), task_revision(1))
            .expect("task lookup")
            .expect("published task")
            .phase,
        PilotTaskPhase::Published
    );
}

#[test]
fn t_pilot_transport_reject() {
    const FIXTURE: &str = "T-PILOT-TRANSPORT-REJECT";
    let root = service_root(FIXTURE);
    let pilot = open_pilot(&root);
    let published = pilot
        .publish(publish_request("pilot-reject", 1))
        .expect("publish task");
    let request = delivery_request(published, "pilot-reject");
    let sessions = serialized_session_boundary(session_record());
    let bus = SerializedAgentBusBoundary::new(BusBehavior::Reject);
    let before = root_image(&root);

    let error = adapter(sessions, bus.clone())
        .send(&request)
        .expect_err("transport rejection");

    assert_eq!(error, AgentBusDeliveryError::TransportRejected);
    assert_eq!(bus.calls(), 1);
    assert_eq!(bus.queue_insertions(), 0);
    assert_eq!(root_image(&root), before);
}

#[test]
fn t_pilot_delivery_receipt() {
    const FIXTURE: &str = "T-PILOT-DELIVERY-RECEIPT";
    let root = service_root(FIXTURE);
    let pilot = open_pilot(&root);
    let published = pilot
        .publish(publish_request("pilot-receipt", 1))
        .expect("publish task");
    let request = delivery_request(published, "pilot-receipt");
    let sessions = serialized_session_boundary(session_record());
    let bus = SerializedAgentBusBoundary::new(BusBehavior::Accept);

    let receipt = adapter(sessions, bus.clone())
        .send(&request)
        .expect("Agent Bus delivery receipt");
    let before_rejection = root_image(&root);
    let mut forged = receipt.clone();
    forged.target_runtime_agent_id = typed_id("cutex:runtime:wrong", RuntimeAgentId::new);
    assert_eq!(
        pilot
            .deliver(request.clone(), forged)
            .expect_err("wrong-target receipt cannot advance task state"),
        PilotError::InvalidRequest(PilotValidationError::DeliveryReceiptMismatch)
    );
    assert_eq!(root_image(&root), before_rejection);
    let delivered = pilot
        .deliver(request.clone(), receipt.clone())
        .expect("record durable delivery");

    assert_eq!(bus.calls(), 1);
    assert_eq!(bus.queue_insertions(), 1);
    assert_eq!(receipt.agent_bus_message_id, MESSAGE_ID);
    assert!(!receipt.deduplicated);
    assert_eq!(delivered.delivery_receipt, receipt);
    assert_eq!(delivered.committed_store_revision, store_revision(4));
    let outer = bus.request(0);
    assert_eq!(
        outer.external_message_id.as_deref(),
        Some(request.delivery_action_id.as_str())
    );
    assert!(outer.external_action_id.is_none());
    let envelope: TaskDeliveryEnvelopeV1 =
        serde_json::from_str(&outer.content).expect("strict delivery envelope");
    assert_eq!(envelope.task_id, task_id("pilot-receipt"));
    assert_eq!(envelope.delivery_action_id, request.delivery_action_id);
    let observed = pilot
        .task(task_id("pilot-receipt"), task_revision(1))
        .expect("task lookup")
        .expect("delivered task");
    assert_eq!(observed.phase, PilotTaskPhase::Delivered);
    assert_eq!(observed.agent_bus_message_id, Some(receipt_id(MESSAGE_ID)));
    assert_eq!(
        pilot
            .receipt(request.transition_receipt_id)
            .expect("transition receipt lookup")
            .expect("transition receipt")
            .resulting_phase,
        PilotTaskPhase::Delivered
    );
}

#[test]
fn t_pilot_duplicate_delivery() {
    const FIXTURE: &str = "T-PILOT-DUPLICATE-DELIVERY";
    let root = service_root(FIXTURE);
    let pilot = open_pilot(&root);
    let published = pilot
        .publish(publish_request("pilot-duplicate", 1))
        .expect("publish task");
    let request = delivery_request(published, "pilot-duplicate");
    let sessions = serialized_session_boundary(session_record());
    let bus = SerializedAgentBusBoundary::new(BusBehavior::Accept);
    let transport = adapter(sessions, bus.clone());

    let first_receipt = transport.send(&request).expect("first enqueue");
    let second_receipt = transport.send(&request).expect("deduplicated enqueue");
    assert!(!first_receipt.deduplicated);
    assert!(second_receipt.deduplicated);
    assert_eq!(
        first_receipt.agent_bus_message_id,
        second_receipt.agent_bus_message_id
    );
    assert_eq!(bus.calls(), 2);
    assert_eq!(bus.queue_insertions(), 1);

    let first = pilot
        .deliver(request.clone(), first_receipt)
        .expect("first durable delivery");
    let after_first = root_image(&root);
    let replay = pilot
        .deliver(request, second_receipt)
        .expect("exact durable receipt replay");
    assert_eq!(
        replay.committed_store_revision,
        first.committed_store_revision
    );
    assert_eq!(root_image(&root), after_first);
}

#[test]
fn t_pilot_restart_replay() {
    const FIXTURE: &str = "T-PILOT-RESTART-REPLAY";
    let root = service_root(FIXTURE);
    let pilot = open_pilot(&root);
    let published = pilot
        .publish(publish_request("pilot-restart", 1))
        .expect("publish task");
    let request = delivery_request(published, "pilot-restart");
    let sessions = serialized_session_boundary(session_record());
    let bus = SerializedAgentBusBoundary::new(BusBehavior::Accept);
    let receipt = adapter(sessions, bus.clone())
        .send(&request)
        .expect("single Agent Bus enqueue");
    let first = pilot
        .deliver(request.clone(), receipt.clone())
        .expect("first delivery");
    let committed_image = root_image(&root);
    drop(pilot);

    let restarted = open_pilot(&root);
    let replay = restarted
        .deliver(request, receipt)
        .expect("restart receipt replay");

    assert_eq!(
        replay.committed_store_revision,
        first.committed_store_revision
    );
    assert_eq!(root_image(&root), committed_image);
    assert_eq!(bus.calls(), 1);
    assert_eq!(bus.queue_insertions(), 1);
}

#[test]
fn t_pilot_uncertain_send() {
    const FIXTURE: &str = "T-PILOT-UNCERTAIN-SEND";
    let root = service_root(FIXTURE);
    let pilot = open_pilot(&root);
    let published = pilot
        .publish(publish_request("pilot-uncertain", 1))
        .expect("publish task");
    let request = delivery_request(published, "pilot-uncertain");
    let before = root_image(&root);
    let sessions = serialized_session_boundary(session_record());
    let bus = SerializedAgentBusBoundary::new(BusBehavior::Uncertain);

    let error = adapter(sessions, bus.clone())
        .send(&request)
        .expect_err("uncertain send requires reconciliation");

    assert_eq!(error, AgentBusDeliveryError::ReconciliationRequired);
    assert_eq!(bus.calls(), 1);
    assert_eq!(root_image(&root), before);
    assert_eq!(
        pilot
            .task(task_id("pilot-uncertain"), task_revision(1))
            .expect("task lookup")
            .expect("published task")
            .phase,
        PilotTaskPhase::Published
    );
}

#[test]
fn t_pilot_facade_surface() {
    const FIXTURE: &str = "T-PILOT-FACADE-SURFACE";
    let root = service_root(FIXTURE);
    let unopened = TaskDeliveryPilot::open(&root).expect("open facade");
    assert_eq!(
        unopened
            .task(task_id("facade-surface"), task_revision(1))
            .expect_err("explicit recovery is mandatory"),
        PilotError::RecoveryRequired
    );

    let lib_source = include_str!("../lib.rs");
    assert!(lib_source.contains("pub mod task_delivery;"));
    assert!(lib_source.contains("pub mod task_service;"));
    assert_eq!(
        crate::task_service::TASK_SERVICE_PROVIDER_CONTRACT,
        "cutex/task-service-provider/v2"
    );
    assert!(!crate::task_service::TASK_SERVICE_PROVIDER_CONTRACT_JSON
        .contains("expected_store_revision"));

    let value = serde_json::to_value(TaskDeliveryEnvelopeV1 {
        schema: TaskDeliveryEnvelopeSchema::V1,
        task_id: task_id("facade-surface"),
        task_revision: task_revision(1),
        opaque_contract: "opaque".to_string(),
        contract_sha256: crate::task_service::sha256_bytes(b"opaque"),
        attempt_fence: PilotAttemptFence {
            task_id: task_id("facade-surface"),
            task_revision: task_revision(1),
            attempt_number: AttemptNumber::new(1).expect("attempt number"),
            attempt_token: AttemptToken::new("facade:attempt").expect("attempt token"),
            owner: owner(),
        },
        delivery_action_id: delivery_id("facade:delivery"),
    })
    .expect("serialize envelope");
    let mut object = value.as_object().expect("envelope object").clone();
    object.insert("inferred_from_chat".to_string(), json!(true));
    assert!(serde_json::from_value::<TaskDeliveryEnvelopeV1>(json!(object)).is_err());
}

#[test]
fn t_pilot_noninterference() {
    const FIXTURE: &str = "T-PILOT-NONINTERFERENCE";
    let root = service_root(FIXTURE);
    let pilot = open_pilot(&root);
    let first = pilot
        .publish(publish_request("pilot-first", 1))
        .expect("publish first task");
    let request = delivery_request(first, "pilot-first");
    let session = session_record();
    let serialized_session = serde_json::to_vec(&session).expect("session bytes");
    let sessions = serialized_session_boundary(session);
    let bus = SerializedAgentBusBoundary::new(BusBehavior::Accept);
    send_and_deliver(&pilot, &request, sessions, bus.clone());

    let second = pilot
        .publish(publish_request("pilot-second", 4))
        .expect("publish unrelated second task");

    assert_eq!(second.committed_store_revision, store_revision(6));
    assert_eq!(
        pilot
            .task(task_id("pilot-first"), task_revision(1))
            .expect("first task lookup")
            .expect("first task")
            .phase,
        PilotTaskPhase::Delivered
    );
    assert_eq!(
        pilot
            .task(task_id("pilot-second"), task_revision(1))
            .expect("second task lookup")
            .expect("second task")
            .phase,
        PilotTaskPhase::Published
    );
    assert_eq!(
        serde_json::to_vec(&session_record()).expect("session bytes after"),
        serialized_session
    );
    let observed: TaskDeliveryEnvelopeV1 =
        serde_json::from_str(&bus.request(0).content).expect("observed delivery envelope");
    assert_eq!(observed.task_id, task_id("pilot-first"));
    assert!(!observed.opaque_contract.contains("pilot-second"));
}

#[test]
fn t_exec_worker_action_wire_has_no_client_transport_referent() {
    const FIXTURE: &str = "T-EXEC-WIRE-SERVER-REFERENT";
    let task = task_id("worker-wire");
    let revision = task_revision(1);
    let request = TaskWorkerActionRequest {
        schema: TaskWorkerActionSchema::V1,
        action: TaskWorkerActionKind::Accept,
        task_id: task.clone(),
        task_revision: revision,
        attempt_fence: PilotAttemptFence {
            task_id: task,
            task_revision: revision,
            attempt_number: AttemptNumber::new(1).unwrap(),
            attempt_token: AttemptToken::new("worker-wire-attempt").unwrap(),
            owner: owner(),
        },
        expected_store_revision: store_revision(4),
        action_id: receipt_id("worker-wire-accept"),
        result: None,
    };
    let serialized = serde_json::to_vec(&request).unwrap();
    let decoded: TaskWorkerActionRequest = serde_json::from_slice(&serialized).unwrap();
    validate_task_worker_action_request(decoded).expect(FIXTURE);
    let mut forged = serde_json::to_value(&request).unwrap();
    forged.as_object_mut().unwrap().insert(
        "transport_reference".to_string(),
        json!({
            "reference_id": "caller-selected",
            "binding_sha256": "0".repeat(64)
        }),
    );
    assert!(serde_json::from_value::<TaskWorkerActionRequest>(forged).is_err());

    let bytes = b"opaque completion bytes".to_vec();
    let mut complete = request;
    complete.action = TaskWorkerActionKind::Complete;
    complete.action_id = receipt_id("worker-wire-complete");
    complete.result = Some(TaskWorkerResult::Utf8 {
        text: String::from_utf8(bytes.clone()).unwrap(),
        sha256: crate::task_service::sha256_bytes(&bytes),
    });
    validate_task_worker_action_request(complete).expect("exact complete hash");
}
