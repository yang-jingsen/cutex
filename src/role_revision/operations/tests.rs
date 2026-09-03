use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::role_revision::repository::{canonical_request_digest, MutationOutcome, RequestLookup};
use crate::session::model::CutexSessionStore;
use crate::session::store::save_cutex_session_store_to_path;

use super::*;

static CASE: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
struct TestClock {
    calls: AtomicU64,
}

impl TestClock {
    fn calls(&self) -> u64 {
        self.calls.load(Ordering::SeqCst)
    }
}

impl TrustedClock for TestClock {
    fn now(&self) -> Rfc3339 {
        let second = self.calls.fetch_add(1, Ordering::SeqCst);
        Rfc3339::new(format!("2026-08-17T00:00:{second:02}Z")).unwrap()
    }
}

fn private_dir(label: &str) -> PathBuf {
    let root =
        PathBuf::from(std::env::var_os("TMPDIR").expect("TMPDIR is required")).join(format!(
            "role-c72-{}-{label}-{}",
            std::process::id(),
            CASE.fetch_add(1, Ordering::SeqCst)
        ));
    fs::create_dir(&root).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    }
    root
}

fn id<T>(value: &str, constructor: impl FnOnce(String) -> Result<T, ValueError>) -> T {
    constructor(value.to_owned()).unwrap()
}

fn sha(character: char) -> Sha256 {
    Sha256::new(character.to_string().repeat(64)).unwrap()
}

fn known_time() -> EvidenceTime {
    EvidenceTime::Known {
        rfc3339: Rfc3339::new("2026-08-16T00:00:00Z").unwrap(),
    }
}

fn evidence(kind: EvidenceKind, subjects: Vec<IdentityRef>, suffix: &str) -> EvidenceRef {
    EvidenceRef {
        kind,
        receipt_id: ReceiptId::new(format!("receipt-{suffix}")).unwrap(),
        receipt_sha256: sha('a'),
        subjects,
        occurred_at: known_time(),
    }
}

struct Harness {
    root: PathBuf,
    operations: RoleSeatOperations,
    clock: Arc<TestClock>,
    request_number: u64,
}

#[derive(Clone)]
struct Flow {
    context: TransitionContext,
    candidate: DurableSessionRef,
    identity: RuntimeIdentity,
}

impl Harness {
    fn new(label: &str) -> Self {
        let root = private_dir(label);
        let clock = Arc::new(TestClock::default());
        let operations = RoleSeatOperations::with_clock(root.clone(), clock.clone()).unwrap();
        Self {
            root,
            operations,
            clock,
            request_number: 0,
        }
    }

    fn project() -> ProjectId {
        id("project-c", ProjectId::new)
    }

    fn family_id() -> RoleFamilyId {
        id("family-c", RoleFamilyId::new)
    }

    fn initialization() -> InitializationId {
        id("initialization-c", InitializationId::new)
    }

    fn current_store_revision(&self) -> StoreRevision {
        self.operations.repository.load().unwrap().store_revision
    }

    fn envelope_with(
        &mut self,
        request: MutationRequest,
        expected_store_revision: StoreRevision,
    ) -> RequestEnvelope {
        self.request_number += 1;
        let mut envelope = RequestEnvelope {
            schema: RequestSchema::V1,
            request_id: RequestId::new(format!("request-{}", self.request_number)).unwrap(),
            request_digest_sha256: sha('0'),
            expected_store_revision,
            request,
        };
        envelope.request_digest_sha256 = canonical_request_digest(&envelope).unwrap();
        envelope
    }

    fn envelope(&mut self, request: MutationRequest) -> RequestEnvelope {
        let expected = self.current_store_revision();
        self.envelope_with(request, expected)
    }

    fn commit_envelope(&self, request: &RequestEnvelope) -> MutationResponse {
        match self.operations.execute(request) {
            MutationOutcome::Committed(response) => response,
            outcome => panic!("expected committed outcome, got {outcome:?}"),
        }
    }

    fn commit(&mut self, request: MutationRequest) -> (RequestEnvelope, MutationResponse) {
        let request = self.envelope(request);
        let response = self.commit_envelope(&request);
        (request, response)
    }

    fn store_bytes(&self) -> Vec<u8> {
        fs::read(self.root.join("role-seat-core-v1.json")).unwrap()
    }

    fn initialize(&mut self) -> (RequestEnvelope, MutationResponse) {
        let incumbent = DurableSessionRef {
            cutex_session_id: CutexSessionId::new("cutex-root").unwrap(),
            durable_revision: DurableRevision::new(7).unwrap(),
        };
        let approval = evidence(
            EvidenceKind::HumanApproval,
            vec![
                IdentityRef::HumanApproval {
                    id: HumanApprovalId::new("approval-root").unwrap(),
                },
                IdentityRef::Project {
                    id: Self::project(),
                },
                IdentityRef::RoleFamily {
                    id: Self::family_id(),
                },
                IdentityRef::CutexSession {
                    id: incumbent.cutex_session_id.clone(),
                },
            ],
            "root-approval",
        );
        let initialization_evidence = evidence(
            EvidenceKind::RootInitialization,
            vec![
                IdentityRef::Project {
                    id: Self::project(),
                },
                IdentityRef::RoleFamily {
                    id: Self::family_id(),
                },
                IdentityRef::CutexSession {
                    id: incumbent.cutex_session_id.clone(),
                },
            ],
            "root-initialization",
        );
        self.commit(MutationRequest::InitializeFamily(InitializeFamilyRequest {
            project_id: Self::project(),
            role_family_id: Self::family_id(),
            role_key: RoleKey::new("director").unwrap(),
            initialization_id: Self::initialization(),
            chosen_root_revision: RoleRevisionNumber::new(7).unwrap(),
            incumbent,
            human_approval_id: HumanApprovalId::new("approval-root").unwrap(),
            approval_evidence: approval,
            initialization_evidence,
            effective_at: known_time(),
        }))
    }

    fn source_snapshot(&self) -> AuthoritySnapshot {
        let family = self.operations.get_family().unwrap().unwrap();
        current_authority_snapshot(&family).unwrap()
    }

    fn identity(name: &str) -> RuntimeIdentity {
        RuntimeIdentity {
            cutex_session_id: CutexSessionId::new(format!("cutex-{name}")).unwrap(),
            cute_codex_session_id: CuteCodexSessionId::new(format!("codex-{name}")).unwrap(),
            runtime_agent_id: RuntimeAgentId::new(format!("runtime-{name}")).unwrap(),
            runtime_generation: RuntimeGeneration::new(1).unwrap(),
        }
    }

    fn prepare_request(
        &self,
        transition_name: &str,
        candidate_name: &str,
    ) -> PrepareRotationRequest {
        let source = self.source_snapshot();
        let family = self.operations.get_family().unwrap().unwrap();
        let identity = Self::identity(candidate_name);
        let task_id = TaskId::new(format!("task-{transition_name}")).unwrap();
        let mut handoff_subjects = vec![
            IdentityRef::Project {
                id: Self::project(),
            },
            IdentityRef::RoleFamily {
                id: Self::family_id(),
            },
            IdentityRef::Task {
                id: task_id.clone(),
            },
        ];
        handoff_subjects.extend(runtime_subjects(&identity));
        let handoff = HandoffRef {
            task_id: task_id.clone(),
            task_revision: TaskRevision::new(1).unwrap(),
            handoff_sha256: sha('b'),
            recipient: identity,
            acceptance_receipt: evidence(
                EvidenceKind::HandoffAcceptance,
                handoff_subjects,
                &format!("handoff-{transition_name}"),
            ),
        };
        let approval = evidence(
            EvidenceKind::HumanApproval,
            vec![
                IdentityRef::HumanApproval {
                    id: HumanApprovalId::new(format!("approval-{transition_name}")).unwrap(),
                },
                IdentityRef::Project {
                    id: Self::project(),
                },
                IdentityRef::RoleFamily {
                    id: Self::family_id(),
                },
                IdentityRef::CutexSession {
                    id: source.cutex_session_id.clone(),
                },
                IdentityRef::Task { id: task_id },
            ],
            &format!("approval-{transition_name}"),
        );
        PrepareRotationRequest {
            project_id: Self::project(),
            role_family_id: Self::family_id(),
            initialization_id: Self::initialization(),
            transition_id: TransitionId::new(transition_name).unwrap(),
            source_authority: source,
            allocator: FamilyAllocatorObservation {
                project_id: Self::project(),
                role_family_id: Self::family_id(),
                initialization_id: Self::initialization(),
                observed_store_revision: self.current_store_revision(),
                next_role_revision: family.next_role_revision,
            },
            human_approval_id: HumanApprovalId::new(format!("approval-{transition_name}")).unwrap(),
            approval_evidence: approval,
            handoff,
        }
    }

    fn prepare(&mut self, transition_name: &str, candidate_name: &str) -> Flow {
        let input = self.prepare_request(transition_name, candidate_name);
        self.commit(MutationRequest::PrepareRotation(input));
        let family = self.operations.get_family().unwrap().unwrap();
        let transition = family
            .transitions
            .get(&TransitionId::new(transition_name).unwrap())
            .unwrap();
        let context = transition_context_for(&family, transition);
        Flow {
            candidate: DurableSessionRef {
                cutex_session_id: context.handoff.recipient.cutex_session_id.clone(),
                durable_revision: DurableRevision::new(1).unwrap(),
            },
            identity: context.handoff.recipient.clone(),
            context,
        }
    }

    fn candidate_evidence(flow: &Flow) -> CandidateEvidence {
        let mut subjects = transition_subjects(&flow.context);
        subjects.push(IdentityRef::CutexSession {
            id: flow.candidate.cutex_session_id.clone(),
        });
        CandidateEvidence {
            session: flow.candidate.clone(),
            receipt: evidence(
                EvidenceKind::CandidateCreation,
                subjects,
                &format!("candidate-{}", flow.context.transition_id.as_str()),
            ),
        }
    }

    fn candidate(&mut self, flow: &Flow) {
        let candidate = Self::candidate_evidence(flow);
        self.commit(MutationRequest::RecordCandidate(RecordCandidateRequest {
            context: flow.context.clone(),
            successor: flow.candidate.clone(),
            evidence: candidate.receipt,
        }));
    }

    fn adoption_evidence(flow: &Flow) -> AdoptionEvidence {
        let mut subjects = transition_subjects(&flow.context);
        subjects.extend(runtime_subjects(&flow.identity));
        AdoptionEvidence {
            identity: flow.identity.clone(),
            receipt: evidence(
                EvidenceKind::Adoption,
                subjects,
                &format!("adoption-{}", flow.context.transition_id.as_str()),
            ),
        }
    }

    fn adopt(&mut self, flow: &Flow) {
        let adoption = Self::adoption_evidence(flow);
        self.commit(MutationRequest::RecordAdoption(RecordAdoptionRequest {
            context: flow.context.clone(),
            candidate_session: flow.candidate.clone(),
            identity: flow.identity.clone(),
            evidence: adoption.receipt,
        }));
    }

    fn delivery_evidence(flow: &Flow) -> DeliveryEvidence {
        let delivery_id =
            DeliveryId::new(format!("{}/initial", flow.context.transition_id.as_str())).unwrap();
        let mut subjects = transition_subjects(&flow.context);
        subjects.push(IdentityRef::Delivery {
            id: delivery_id.clone(),
        });
        subjects.extend(runtime_subjects(&flow.identity));
        DeliveryEvidence {
            delivery_id,
            recipient: flow.identity.clone(),
            receipt: evidence(
                EvidenceKind::InitialDelivery,
                subjects,
                &format!("delivery-{}", flow.context.transition_id.as_str()),
            ),
        }
    }

    fn deliver(&mut self, flow: &Flow) {
        let delivery = Self::delivery_evidence(flow);
        self.commit(MutationRequest::RecordInitialDelivery(
            RecordInitialDeliveryRequest {
                context: flow.context.clone(),
                delivery_id: delivery.delivery_id,
                recipient: flow.identity.clone(),
                evidence: delivery.receipt,
            },
        ));
    }

    fn acknowledgement_evidence(flow: &Flow) -> AcknowledgementEvidence {
        let mut subjects = transition_subjects(&flow.context);
        subjects.extend(runtime_subjects(&flow.identity));
        AcknowledgementEvidence {
            responder: flow.identity.clone(),
            handoff_sha256: flow.context.handoff.handoff_sha256.clone(),
            receipt: evidence(
                EvidenceKind::Acknowledgement,
                subjects,
                &format!("ack-{}", flow.context.transition_id.as_str()),
            ),
        }
    }

    fn acknowledge(&mut self, flow: &Flow) {
        let acknowledgement = Self::acknowledgement_evidence(flow);
        self.commit(MutationRequest::RecordAcknowledgement(
            RecordAcknowledgementRequest {
                context: flow.context.clone(),
                responder: flow.identity.clone(),
                handoff_sha256: flow.context.handoff.handoff_sha256.clone(),
                evidence: acknowledgement.receipt,
            },
        ));
    }

    fn transfer_evidence(flow: &Flow) -> EvidenceRef {
        let mut subjects = transition_subjects(&flow.context);
        subjects.push(IdentityRef::CutexSession {
            id: flow.context.intended_predecessor.cutex_session_id.clone(),
        });
        subjects.extend(runtime_subjects(&flow.identity));
        evidence(
            EvidenceKind::TransferVerification,
            subjects,
            &format!("transfer-{}", flow.context.transition_id.as_str()),
        )
    }

    fn transfer(&mut self, flow: &Flow) {
        self.commit(MutationRequest::TransferAuthority(
            TransferAuthorityRequest {
                context: flow.context.clone(),
                fresh_incumbent: DurableSessionRef {
                    cutex_session_id: flow.context.intended_predecessor.cutex_session_id.clone(),
                    durable_revision: flow.context.intended_predecessor.source_durable_revision,
                },
                candidate_session: flow.candidate.clone(),
                adopted_identity: flow.identity.clone(),
                expected_authority_epoch: flow.context.intended_predecessor.authority_epoch,
                evidence: Self::transfer_evidence(flow),
            },
        ));
    }

    fn completion_evidence(flow: &Flow) -> EvidenceRef {
        let mut subjects = transition_subjects(&flow.context);
        subjects.extend(runtime_subjects(&flow.identity));
        evidence(
            EvidenceKind::Completion,
            subjects,
            &format!("completion-{}", flow.context.transition_id.as_str()),
        )
    }

    fn complete(&mut self, flow: &Flow) {
        self.commit(MutationRequest::CompleteRotation(CompleteRotationRequest {
            context: flow.context.clone(),
            adopted_identity: flow.identity.clone(),
            evidence: Self::completion_evidence(flow),
        }));
    }

    fn successful_rotation(&mut self, transition_name: &str, candidate_name: &str) -> Flow {
        let flow = self.prepare(transition_name, candidate_name);
        assert!(self
            .operations
            .get_family()
            .unwrap()
            .unwrap()
            .active_rotation
            .is_some());
        self.candidate(&flow);
        self.adopt(&flow);
        self.deliver(&flow);
        self.acknowledge(&flow);
        self.transfer(&flow);
        assert!(self
            .operations
            .get_family()
            .unwrap()
            .unwrap()
            .active_rotation
            .is_some());
        self.complete(&flow);
        assert!(self
            .operations
            .get_family()
            .unwrap()
            .unwrap()
            .active_rotation
            .is_none());
        flow
    }

    fn phase_payload(flow: &Flow, phase: RotationPhase) -> PhasePayload {
        match phase {
            RotationPhase::Candidate => PhasePayload::Candidate {
                candidate: Self::candidate_evidence(flow),
            },
            RotationPhase::Adoption => PhasePayload::Adoption {
                candidate_session: flow.candidate.clone(),
                adoption: Self::adoption_evidence(flow),
            },
            RotationPhase::InitialDelivery => PhasePayload::InitialDelivery {
                delivery: Self::delivery_evidence(flow),
            },
            RotationPhase::Acknowledgement => PhasePayload::Acknowledgement {
                acknowledgement: Self::acknowledgement_evidence(flow),
            },
            RotationPhase::Transfer => PhasePayload::Transfer {
                fresh_incumbent: DurableSessionRef {
                    cutex_session_id: flow.context.intended_predecessor.cutex_session_id.clone(),
                    durable_revision: flow.context.intended_predecessor.source_durable_revision,
                },
                candidate_session: flow.candidate.clone(),
                recipient: flow.identity.clone(),
                evidence: Self::transfer_evidence(flow),
            },
            RotationPhase::Completion => PhasePayload::Completion {
                transition_id: flow.context.transition_id.clone(),
                evidence: Self::completion_evidence(flow),
            },
            RotationPhase::Prepare => panic!("prepare is not an external unknown phase"),
        }
    }

    fn advance_before(&mut self, flow: &Flow, phase: RotationPhase) {
        if matches!(
            phase,
            RotationPhase::Adoption
                | RotationPhase::InitialDelivery
                | RotationPhase::Acknowledgement
                | RotationPhase::Transfer
                | RotationPhase::Completion
        ) {
            self.candidate(flow);
        }
        if matches!(
            phase,
            RotationPhase::InitialDelivery
                | RotationPhase::Acknowledgement
                | RotationPhase::Transfer
                | RotationPhase::Completion
        ) {
            self.adopt(flow);
        }
        if matches!(
            phase,
            RotationPhase::Acknowledgement | RotationPhase::Transfer | RotationPhase::Completion
        ) {
            self.deliver(flow);
        }
        if matches!(phase, RotationPhase::Transfer | RotationPhase::Completion) {
            self.acknowledge(flow);
        }
        if phase == RotationPhase::Completion {
            self.transfer(flow);
        }
    }

    fn unknown_reason(phase: RotationPhase) -> ReasonCode {
        match phase {
            RotationPhase::Candidate => ReasonCode::PersistenceOutcomeUnknown,
            RotationPhase::Adoption => ReasonCode::AdoptionOutcomeUnknown,
            RotationPhase::InitialDelivery => ReasonCode::DeliveryOutcomeUnknown,
            RotationPhase::Acknowledgement => ReasonCode::AcknowledgementOutcomeUnknown,
            RotationPhase::Transfer => ReasonCode::TransferOutcomeUnknown,
            RotationPhase::Completion => ReasonCode::CompletionOutcomeUnknown,
            RotationPhase::Prepare => panic!("prepare is not an external unknown phase"),
        }
    }

    fn observation_evidence(flow: &Flow, phase: RotationPhase) -> EvidenceRef {
        let mut subjects = transition_subjects(&flow.context);
        subjects.extend(runtime_subjects(&flow.identity));
        evidence(
            EvidenceKind::UnknownObservation,
            subjects,
            &format!("unknown-{phase:?}").to_ascii_lowercase(),
        )
    }

    fn resolution_evidence(flow: &Flow, phase: RotationPhase) -> EvidenceRef {
        let mut subjects = transition_subjects(&flow.context);
        subjects.extend(runtime_subjects(&flow.identity));
        evidence(
            EvidenceKind::UnknownResolution,
            subjects,
            &format!("resolution-{phase:?}").to_ascii_lowercase(),
        )
    }

    fn record_unknown(&mut self, flow: &Flow, phase: RotationPhase) -> UnknownOutcome {
        let payload = Self::phase_payload(flow, phase);
        let (_, response) = self.commit(MutationRequest::RecordUnknown(RecordUnknownRequest {
            context: flow.context.clone(),
            adopted_identity: flow.identity.clone(),
            attempt: AttemptNumber::new(1).unwrap(),
            phase,
            attempted_payload: payload,
            reason_code: Self::unknown_reason(phase),
            evidence: Self::observation_evidence(flow, phase),
        }));
        match response.result {
            MutationResult::RecordUnknown { unknown, .. } => unknown,
            _ => unreachable!(),
        }
    }
}

fn assert_no_write(outcome: MutationOutcome, before: &[u8]) {
    assert!(matches!(outcome, MutationOutcome::NoWrite(_)));
    let _ = before;
}

#[test]
fn c_init_human_chosen_root_has_no_fabricated_history() {
    let mut harness = Harness::new("init");
    harness.initialize();
    let family = harness.operations.get_family().unwrap().unwrap();
    assert_eq!(family.revisions.len(), 1);
    assert_eq!(
        family.revisions.keys().copied().collect::<Vec<_>>(),
        vec![RoleRevisionNumber::new(7).unwrap()]
    );
    assert_eq!(
        family.next_role_revision,
        RoleRevisionNumber::new(8).unwrap()
    );
    assert_eq!(family.current_authority.authority_epoch.get(), 1);
    assert_eq!(harness.clock.calls(), 1);
}

#[test]
fn c_two_two_successes_move_authority_and_preserve_first_history() {
    let mut harness = Harness::new("two");
    harness.initialize();
    let first = harness.successful_rotation("transition-one", "candidate-one");
    let first_history = harness
        .operations
        .get_transition(&first.context.transition_id)
        .unwrap()
        .unwrap();
    let second = harness.successful_rotation("transition-two", "candidate-two");
    let family = harness.operations.get_family().unwrap().unwrap();
    assert_eq!(family.current_authority.authority_epoch.get(), 3);
    assert_eq!(family.current_authority.role_revision.get(), 9);
    assert_eq!(
        harness
            .operations
            .get_transition(&first.context.transition_id)
            .unwrap()
            .unwrap(),
        first_history
    );
    assert_eq!(
        harness
            .operations
            .get_transition(&second.context.transition_id)
            .unwrap()
            .unwrap()
            .state,
        TransitionState::Completed
    );
}

#[test]
fn c_gaps_failed_and_cancelled_allocations_remain_consumed() {
    let mut harness = Harness::new("gaps");
    harness.initialize();
    let failed = harness.prepare("transition-failed", "candidate-failed");
    let mut failure_subjects = transition_subjects(&failed.context);
    failure_subjects.extend(runtime_subjects(&failed.identity));
    harness.commit(MutationRequest::FailRotation(TerminalRequest {
        context: failed.context.clone(),
        adopted_identity: failed.identity.clone(),
        attempt: AttemptNumber::new(1).unwrap(),
        phase: RotationPhase::Candidate,
        reason_code: ReasonCode::ExternalFailure,
        evidence: evidence(EvidenceKind::Failure, failure_subjects, "failed-gap"),
    }));
    let cancelled = harness.prepare("transition-cancelled", "candidate-cancelled");
    let mut cancellation_subjects = transition_subjects(&cancelled.context);
    cancellation_subjects.extend(runtime_subjects(&cancelled.identity));
    harness.commit(MutationRequest::CancelRotation(TerminalRequest {
        context: cancelled.context.clone(),
        adopted_identity: cancelled.identity.clone(),
        attempt: AttemptNumber::new(1).unwrap(),
        phase: RotationPhase::Candidate,
        reason_code: ReasonCode::HumanCancelled,
        evidence: evidence(
            EvidenceKind::Cancellation,
            cancellation_subjects,
            "cancelled-gap",
        ),
    }));
    harness.successful_rotation("transition-success", "candidate-success");
    let family = harness.operations.get_family().unwrap().unwrap();
    assert_eq!(
        family
            .revisions
            .get(&RoleRevisionNumber::new(8).unwrap())
            .unwrap()
            .state,
        RoleRevisionState::Failed
    );
    assert_eq!(
        family
            .revisions
            .get(&RoleRevisionNumber::new(9).unwrap())
            .unwrap()
            .state,
        RoleRevisionState::Cancelled
    );
    assert_eq!(family.current_authority.role_revision.get(), 10);
    let ancestry = harness.operations.get_successful_ancestry().unwrap();
    assert_eq!(
        ancestry
            .iter()
            .map(|revision| revision.role_revision.get())
            .collect::<Vec<_>>(),
        vec![10, 7]
    );
}

#[test]
fn c_fence_stale_inputs_and_second_prepare_are_byte_identical_and_clock_free() {
    let mut harness = Harness::new("fence");
    harness.initialize();
    let baseline_calls = harness.clock.calls();
    let baseline = harness.store_bytes();

    let valid_prepare = harness.prepare_request("transition-fence", "candidate-fence");
    let stale = harness.envelope_with(
        MutationRequest::PrepareRotation(valid_prepare.clone()),
        StoreRevision::new(1).unwrap(),
    );
    assert_no_write(harness.operations.execute(&stale), &baseline);
    assert_eq!(harness.store_bytes(), baseline);
    assert_eq!(harness.clock.calls(), baseline_calls);

    for mutate in 0..3 {
        let mut input = valid_prepare.clone();
        match mutate {
            0 => input.source_authority.role_revision = RoleRevisionNumber::new(99).unwrap(),
            1 => input.source_authority.authority_epoch = AuthorityEpoch::new(99).unwrap(),
            _ => input.source_authority.source_durable_revision = DurableRevision::new(99).unwrap(),
        }
        let request = harness.envelope(MutationRequest::PrepareRotation(input));
        assert_no_write(harness.operations.execute(&request), &baseline);
        assert_eq!(harness.store_bytes(), baseline);
        assert_eq!(harness.clock.calls(), baseline_calls);
    }

    let flow = harness.prepare("transition-fence", "candidate-fence");
    let locked_bytes = harness.store_bytes();
    let locked_calls = harness.clock.calls();
    let second = harness.prepare_request("transition-second", "candidate-second");
    let request = harness.envelope(MutationRequest::PrepareRotation(second));
    assert_no_write(harness.operations.execute(&request), &locked_bytes);
    assert_eq!(harness.store_bytes(), locked_bytes);
    assert_eq!(harness.clock.calls(), locked_calls);

    let mut stale_context = flow.context.clone();
    stale_context.candidate_revision = RoleRevisionNumber::new(99).unwrap();
    let candidate = Harness::candidate_evidence(&flow);
    let request = harness.envelope(MutationRequest::RecordCandidate(RecordCandidateRequest {
        context: stale_context,
        successor: flow.candidate,
        evidence: candidate.receipt,
    }));
    assert_no_write(harness.operations.execute(&request), &locked_bytes);
    assert_eq!(harness.store_bytes(), locked_bytes);
    assert_eq!(harness.clock.calls(), locked_calls);
}

#[test]
fn c_replay_freezes_result_and_same_id_different_material_conflicts() {
    let mut harness = Harness::new("replay");
    let (initialize, original) = harness.initialize();
    harness.prepare("transition-later", "candidate-later");
    let before = harness.store_bytes();
    let calls = harness.clock.calls();
    let replay = harness.operations.execute(&initialize);
    assert!(matches!(replay, MutationOutcome::Committed(ref response) if response == &original));
    assert_eq!(harness.store_bytes(), before);
    assert_eq!(harness.clock.calls(), calls);
    assert!(matches!(
        harness.operations.get_request_result(&initialize),
        RequestLookup::Committed(ref response) if response == &original
    ));

    let mut conflicting = initialize.clone();
    if let MutationRequest::InitializeFamily(input) = &mut conflicting.request {
        input.role_key = RoleKey::new("different-role").unwrap();
    }
    conflicting.request_digest_sha256 = canonical_request_digest(&conflicting).unwrap();
    assert!(matches!(
        harness.operations.execute(&conflicting),
        MutationOutcome::NoWrite(RepositoryError::RequestConflict)
    ));
    assert!(matches!(
        harness.operations.get_request_result(&conflicting),
        RequestLookup::RequestConflict
    ));
    assert_eq!(harness.store_bytes(), before);
    assert_eq!(harness.clock.calls(), calls);
}

#[test]
fn c_unknown_all_external_phases_preserve_audit_and_apply_verified_outcomes() {
    for phase in [
        RotationPhase::Candidate,
        RotationPhase::Adoption,
        RotationPhase::InitialDelivery,
        RotationPhase::Acknowledgement,
        RotationPhase::Transfer,
        RotationPhase::Completion,
    ] {
        let mut harness = Harness::new(&format!("unknown-{phase:?}").to_ascii_lowercase());
        harness.initialize();
        let flow = harness.prepare("transition-unknown", "candidate-unknown");
        harness.advance_before(&flow, phase);
        let before_family = harness.operations.get_family().unwrap().unwrap();
        let before_authority = before_family.current_authority.clone();
        let before_revision = before_family
            .revisions
            .get(&flow.context.candidate_revision)
            .unwrap()
            .clone();
        let unknown = harness.record_unknown(&flow, phase);
        let recorded = harness.operations.get_family().unwrap().unwrap();
        assert_eq!(recorded.current_authority, before_authority);
        assert_eq!(
            recorded
                .revisions
                .get(&flow.context.candidate_revision)
                .unwrap(),
            &before_revision
        );
        assert!(recorded.active_rotation.is_some());
        assert_eq!(
            unknown.prior.current_authority,
            unknown.post_state.current_authority
        );

        let request = harness.envelope(MutationRequest::ResolveUnknown(ResolveUnknownRequest {
            context: flow.context.clone(),
            adopted_identity: flow.identity.clone(),
            attempt: unknown.attempt,
            outcome: ResolutionIntent::PhaseSucceeded {
                verified_payload: unknown.attempted_payload.clone(),
            },
            evidence: Harness::resolution_evidence(&flow, phase),
        }));
        let bytes = harness.store_bytes();
        let calls = harness.clock.calls();
        let outcome = harness.operations.execute(&request);
        if phase == RotationPhase::Transfer {
            assert!(matches!(
                outcome,
                MutationOutcome::NoWrite(RepositoryError::PlanRejected)
            ));
            assert_eq!(harness.store_bytes(), bytes);
            assert_eq!(harness.clock.calls(), calls);
            assert_eq!(
                harness
                    .operations
                    .get_transition(&flow.context.transition_id)
                    .unwrap()
                    .unwrap()
                    .state,
                TransitionState::Unknown
            );
        } else {
            let response = match outcome {
                MutationOutcome::Committed(response) => response,
                other => panic!("verified resolution failed: {other:?}"),
            };
            let resolved = match response.result {
                MutationResult::ResolveUnknown { unknown, .. } => unknown,
                _ => unreachable!(),
            };
            assert!(resolved.resolution.is_some());
            let family = harness.operations.get_family().unwrap().unwrap();
            if phase == RotationPhase::Completion {
                assert!(family.active_rotation.is_none());
                assert_eq!(family.current_authority, before_authority);
            } else {
                assert!(family.active_rotation.is_some());
                assert_eq!(family.current_authority, before_authority);
            }
        }
    }

    for outcome in [TerminalOutcome::Failed, TerminalOutcome::Cancelled] {
        let mut harness = Harness::new("unknown-terminal");
        harness.initialize();
        let flow = harness.prepare("transition-terminal", "candidate-terminal");
        let unknown = harness.record_unknown(&flow, RotationPhase::Candidate);
        let mut subjects = transition_subjects(&flow.context);
        subjects.extend(runtime_subjects(&flow.identity));
        let (intent, expected_state) = match outcome {
            TerminalOutcome::Failed => (
                ResolutionIntent::Failed {
                    reason_code: ReasonCode::ExternalFailure,
                    evidence: evidence(EvidenceKind::Failure, subjects, "unknown-failed"),
                },
                TransitionState::Failed,
            ),
            TerminalOutcome::Cancelled => (
                ResolutionIntent::Cancelled {
                    reason_code: ReasonCode::HumanCancelled,
                    evidence: evidence(EvidenceKind::Cancellation, subjects, "unknown-cancelled"),
                },
                TransitionState::Cancelled,
            ),
        };
        harness.commit(MutationRequest::ResolveUnknown(ResolveUnknownRequest {
            context: flow.context.clone(),
            adopted_identity: flow.identity.clone(),
            attempt: unknown.attempt,
            outcome: intent,
            evidence: Harness::resolution_evidence(&flow, RotationPhase::Candidate),
        }));
        let family = harness.operations.get_family().unwrap().unwrap();
        assert!(family.active_rotation.is_none());
        assert_eq!(
            family
                .transitions
                .get(&flow.context.transition_id)
                .unwrap()
                .state,
            expected_state
        );
    }
}

#[test]
fn c_read_getters_and_successful_ancestry_exclude_gaps() {
    let mut harness = Harness::new("read");
    harness.initialize();
    let gap = harness.prepare("transition-gap", "candidate-gap");
    let mut subjects = transition_subjects(&gap.context);
    subjects.extend(runtime_subjects(&gap.identity));
    harness.commit(MutationRequest::FailRotation(TerminalRequest {
        context: gap.context.clone(),
        adopted_identity: gap.identity,
        attempt: AttemptNumber::new(1).unwrap(),
        phase: RotationPhase::Candidate,
        reason_code: ReasonCode::ExternalFailure,
        evidence: evidence(EvidenceKind::Failure, subjects, "read-gap"),
    }));
    let first = harness.successful_rotation("transition-read-one", "candidate-read-one");
    let second = harness.successful_rotation("transition-read-two", "candidate-read-two");
    assert!(harness
        .operations
        .get_current_authority()
        .unwrap()
        .is_some());
    assert!(harness
        .operations
        .get_revision(second.context.candidate_revision)
        .unwrap()
        .is_some());
    assert!(harness
        .operations
        .get_transition(&first.context.transition_id)
        .unwrap()
        .is_some());
    assert_eq!(
        harness
            .operations
            .get_successful_ancestry()
            .unwrap()
            .iter()
            .map(|revision| revision.role_revision.get())
            .collect::<Vec<_>>(),
        vec![10, 9, 7]
    );
}

#[test]
fn c_session_full_stage_c_sequence_does_not_touch_seeded_session_store() {
    let case_root = private_dir("session");
    let session_path = case_root.join("sessions").join("cutex-sessions.json");
    let session_store = CutexSessionStore::default();
    save_cutex_session_store_to_path(&session_path, &session_store).unwrap();
    let before = fs::read(&session_path).unwrap();

    let role_root = case_root.join("role");
    fs::create_dir(&role_root).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&role_root, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let clock = Arc::new(TestClock::default());
    let operations = RoleSeatOperations::with_clock(role_root.clone(), clock.clone()).unwrap();
    let mut harness = Harness {
        root: role_root,
        operations,
        clock,
        request_number: 0,
    };
    harness.initialize();
    harness.successful_rotation("transition-session", "candidate-session");
    assert_eq!(fs::read(&session_path).unwrap(), before);
}
