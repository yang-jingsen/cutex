//! Durable Stage C operations over the Stage B role-seat repository.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{SecondsFormat, Utc};

use super::repository::{
    MutationOutcome, PlannedMutation, RepositoryError, RequestLookup, RoleSeatRepository,
};
use super::*;

#[cfg(test)]
mod tests;

pub(crate) trait TrustedClock: Send + Sync {
    fn now(&self) -> Rfc3339;
}

struct SystemClock;

impl TrustedClock for SystemClock {
    fn now(&self) -> Rfc3339 {
        Rfc3339::new(Utc::now().to_rfc3339_opts(SecondsFormat::AutoSi, true))
            .expect("UTC clock output is normalized RFC3339")
    }
}

/// The single mutation and read facade for a Role-Seat v1 repository.
pub struct RoleSeatOperations {
    repository: RoleSeatRepository,
    clock: Arc<dyn TrustedClock>,
}

impl RoleSeatOperations {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, RepositoryError> {
        Self::with_clock(root, Arc::new(SystemClock))
    }

    pub(crate) fn with_clock(
        root: impl Into<PathBuf>,
        clock: Arc<dyn TrustedClock>,
    ) -> Result<Self, RepositoryError> {
        Ok(Self {
            repository: RoleSeatRepository::new(root)?,
            clock,
        })
    }

    /// Executes exactly one locked Stage B mutation for the supplied operation.
    pub fn execute(&self, request: &RequestEnvelope) -> MutationOutcome {
        match &request.request {
            MutationRequest::InitializeFamily(input) => self.repository.mutate(request, |store| {
                plan_initialize(store, input, self.clock.as_ref())
            }),
            MutationRequest::PrepareRotation(input) => self.repository.mutate(request, |store| {
                plan_prepare(store, input, self.clock.as_ref())
            }),
            MutationRequest::RecordCandidate(input) => self.repository.mutate(request, |store| {
                plan_candidate(store, input, self.clock.as_ref())
            }),
            MutationRequest::RecordAdoption(input) => self.repository.mutate(request, |store| {
                plan_adoption(store, input, self.clock.as_ref())
            }),
            MutationRequest::RecordInitialDelivery(input) => {
                self.repository.mutate(request, |store| {
                    plan_delivery(store, input, self.clock.as_ref())
                })
            }
            MutationRequest::RecordAcknowledgement(input) => {
                self.repository.mutate(request, |store| {
                    plan_acknowledgement(store, input, self.clock.as_ref())
                })
            }
            MutationRequest::TransferAuthority(input) => self.repository.mutate(request, |store| {
                plan_transfer(store, input, self.clock.as_ref())
            }),
            MutationRequest::CompleteRotation(input) => self.repository.mutate(request, |store| {
                plan_completion(store, input, self.clock.as_ref())
            }),
            MutationRequest::FailRotation(input) => self.repository.mutate(request, |store| {
                plan_terminal(store, input, TerminalOutcome::Failed, self.clock.as_ref())
            }),
            MutationRequest::CancelRotation(input) => self.repository.mutate(request, |store| {
                plan_terminal(
                    store,
                    input,
                    TerminalOutcome::Cancelled,
                    self.clock.as_ref(),
                )
            }),
            MutationRequest::RecordUnknown(input) => self.repository.mutate(request, |store| {
                plan_record_unknown(store, input, self.clock.as_ref())
            }),
            MutationRequest::ResolveUnknown(input) => self.repository.mutate(request, |store| {
                plan_resolve_unknown(store, input, self.clock.as_ref())
            }),
        }
    }

    pub fn get_family(&self) -> Result<Option<RoleFamily>, RepositoryError> {
        Ok(self.repository.load()?.family)
    }

    pub fn get_current_authority(&self) -> Result<Option<CurrentAuthority>, RepositoryError> {
        Ok(self
            .repository
            .load()?
            .family
            .map(|family| family.current_authority))
    }

    pub fn get_revision(
        &self,
        role_revision: RoleRevisionNumber,
    ) -> Result<Option<RoleRevision>, RepositoryError> {
        Ok(self
            .repository
            .load()?
            .family
            .and_then(|family| family.revisions.get(&role_revision).cloned()))
    }

    pub fn get_transition(
        &self,
        transition_id: &TransitionId,
    ) -> Result<Option<RoleTransition>, RepositoryError> {
        Ok(self
            .repository
            .load()?
            .family
            .and_then(|family| family.transitions.get(transition_id).cloned()))
    }

    /// Returns current-to-root successful ancestry, excluding allocated gaps.
    pub fn get_successful_ancestry(&self) -> Result<Vec<RoleRevision>, RepositoryError> {
        let Some(family) = self.repository.load()?.family else {
            return Ok(Vec::new());
        };
        let root = family.root_initialization.chosen_root_revision;
        let mut number = family.current_authority.role_revision;
        let mut ancestry = Vec::new();
        loop {
            let revision = family
                .revisions
                .get(&number)
                .cloned()
                .ok_or(RepositoryError::PlanRejected)?;
            let predecessor = revision.successful_predecessor.clone();
            ancestry.push(revision);
            if number == root {
                break;
            }
            number = predecessor
                .ok_or(RepositoryError::PlanRejected)?
                .role_revision;
        }
        Ok(ancestry)
    }

    pub fn get_request_result(&self, request: &RequestEnvelope) -> RequestLookup {
        self.repository.get_request_result(request)
    }
}

fn rejected<T>() -> Result<T, RepositoryError> {
    Err(RepositoryError::PlanRejected)
}

fn valid(result: Result<(), ValidationError>) -> Result<(), RepositoryError> {
    result.map_err(|_| RepositoryError::PlanRejected)
}

fn family_for_context<'a>(
    store: &'a RoleSeatStore,
    context: &TransitionContext,
) -> Result<&'a RoleFamily, RepositoryError> {
    let family = store.family.as_ref().ok_or(RepositoryError::PlanRejected)?;
    if family.project_id != context.project_id
        || family.role_family_id != context.role_family_id
        || family.root_initialization.initialization_id != context.initialization_id
    {
        return rejected();
    }
    let transition = family
        .transitions
        .get(&context.transition_id)
        .ok_or(RepositoryError::PlanRejected)?;
    if transition_context_for(family, transition) != *context {
        return rejected();
    }
    let lock = family
        .active_rotation
        .as_ref()
        .ok_or(RepositoryError::PlanRejected)?;
    if lock.transition_id != context.transition_id
        || lock.candidate_revision != context.candidate_revision
        || lock.source_authority_epoch != context.intended_predecessor.authority_epoch
    {
        return rejected();
    }
    Ok(family)
}

fn plan_initialize(
    store: &RoleSeatStore,
    input: &InitializeFamilyRequest,
    clock: &dyn TrustedClock,
) -> Result<PlannedMutation, RepositoryError> {
    if store.family.is_some() || !store.idempotency.is_empty() {
        return rejected();
    }
    valid(evidence(
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
    ))?;
    valid(evidence(
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
    ))?;
    let next_role_revision = input
        .chosen_root_revision
        .checked_next()
        .map_err(|_| RepositoryError::PlanRejected)?;
    let authority_epoch = AuthorityEpoch::new(1).map_err(|_| RepositoryError::PlanRejected)?;
    let now = clock.now();
    let root_revision = RoleRevision {
        role_revision: input.chosen_root_revision,
        session: Some(input.incumbent.clone()),
        state: RoleRevisionState::InitializedCurrent,
        intended_predecessor: None,
        successful_predecessor: None,
        root_revision: Some(input.chosen_root_revision),
        allocated_at: now.clone(),
        terminal_attempt: None,
    };
    let family = RoleFamily {
        role_family_id: input.role_family_id.clone(),
        project_id: input.project_id.clone(),
        role_key: input.role_key.clone(),
        root_initialization: RootInitialization {
            initialization_id: input.initialization_id.clone(),
            chosen_root_revision: input.chosen_root_revision,
            incumbent: input.incumbent.clone(),
            approval_evidence: input.approval_evidence.clone(),
            initialization_evidence: input.initialization_evidence.clone(),
            effective_at: input.effective_at.clone(),
            recorded_at: now,
        },
        next_role_revision,
        current_authority: CurrentAuthority {
            role_revision: input.chosen_root_revision,
            cutex_session_id: input.incumbent.cutex_session_id.clone(),
            authority_epoch,
            effective_at: input.effective_at.clone(),
            established_by: EstablishedBy::RootInitialization {
                initialization_id: input.initialization_id.clone(),
            },
        },
        active_rotation: None,
        revisions: BTreeMap::from([(input.chosen_root_revision, root_revision)]),
        transitions: BTreeMap::new(),
    };
    Ok(PlannedMutation {
        family: Some(family),
        result: MutationResult::InitializeFamily {
            role_family_id: input.role_family_id.clone(),
            root_revision: input.chosen_root_revision,
            authority_epoch,
        },
    })
}

fn plan_prepare(
    store: &RoleSeatStore,
    input: &PrepareRotationRequest,
    clock: &dyn TrustedClock,
) -> Result<PlannedMutation, RepositoryError> {
    let existing = store.family.as_ref().ok_or(RepositoryError::PlanRejected)?;
    if existing.project_id != input.project_id
        || existing.role_family_id != input.role_family_id
        || existing.root_initialization.initialization_id != input.initialization_id
        || existing.active_rotation.is_some()
        || existing.current_authority.role_revision != input.source_authority.role_revision
        || existing.current_authority.cutex_session_id != input.source_authority.cutex_session_id
        || existing.current_authority.authority_epoch != input.source_authority.authority_epoch
        || current_authority_snapshot(existing).map_err(|_| RepositoryError::PlanRejected)?
            != input.source_authority
        || input.allocator.project_id != input.project_id
        || input.allocator.role_family_id != input.role_family_id
        || input.allocator.initialization_id != input.initialization_id
        || input.allocator.observed_store_revision != store.store_revision
        || input.allocator.next_role_revision != existing.next_role_revision
        || existing.transitions.contains_key(&input.transition_id)
    {
        return rejected();
    }
    let context = TransitionContext {
        project_id: input.project_id.clone(),
        role_family_id: input.role_family_id.clone(),
        initialization_id: input.initialization_id.clone(),
        transition_id: input.transition_id.clone(),
        candidate_revision: existing.next_role_revision,
        intended_predecessor: input.source_authority.clone(),
        handoff: input.handoff.clone(),
    };
    valid(validate_context(&context))?;
    valid(evidence(
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
    ))?;
    let candidate_revision = existing.next_role_revision;
    let next_role_revision = candidate_revision
        .checked_next()
        .map_err(|_| RepositoryError::PlanRejected)?;
    let now = clock.now();
    let lock = RotationLock {
        transition_id: input.transition_id.clone(),
        candidate_revision,
        source_authority_epoch: input.source_authority.authority_epoch,
    };
    let revision = RoleRevision {
        role_revision: candidate_revision,
        session: None,
        state: RoleRevisionState::Candidate,
        intended_predecessor: Some(input.source_authority.clone()),
        successful_predecessor: None,
        root_revision: None,
        allocated_at: now.clone(),
        terminal_attempt: None,
    };
    let transition = RoleTransition {
        transition_id: input.transition_id.clone(),
        candidate_revision,
        intended_predecessor: input.source_authority.clone(),
        approval_evidence: input.approval_evidence.clone(),
        handoff: input.handoff.clone(),
        state: TransitionState::Prepared,
        candidate_evidence: None,
        adoption_evidence: None,
        delivery_evidence: None,
        acknowledgement_evidence: None,
        completion_evidence: None,
        unknown_outcomes: Vec::new(),
        terminal_attempt: None,
        created_at: now.clone(),
        updated_at: now,
    };
    let mut family = existing.clone();
    family.next_role_revision = next_role_revision;
    family.active_rotation = Some(lock);
    family.revisions.insert(candidate_revision, revision);
    family
        .transitions
        .insert(input.transition_id.clone(), transition);
    Ok(PlannedMutation {
        family: Some(family),
        result: MutationResult::PrepareRotation {
            transition_id: input.transition_id.clone(),
            candidate_revision,
            source_authority_epoch: input.source_authority.authority_epoch,
        },
    })
}

fn plan_candidate(
    store: &RoleSeatStore,
    input: &RecordCandidateRequest,
    clock: &dyn TrustedClock,
) -> Result<PlannedMutation, RepositoryError> {
    let existing = family_for_context(store, &input.context)?;
    let transition = existing
        .transitions
        .get(&input.context.transition_id)
        .ok_or(RepositoryError::PlanRejected)?;
    let revision = existing
        .revisions
        .get(&input.context.candidate_revision)
        .ok_or(RepositoryError::PlanRejected)?;
    let candidate = CandidateEvidence {
        session: input.successor.clone(),
        receipt: input.evidence.clone(),
    };
    if transition.state != TransitionState::Prepared
        || transition.candidate_evidence.is_some()
        || revision.state != RoleRevisionState::Candidate
        || revision.session.is_some()
    {
        return rejected();
    }
    valid(validate_candidate_evidence(&input.context, &candidate))?;
    let now = clock.now();
    let mut family = existing.clone();
    family
        .revisions
        .get_mut(&input.context.candidate_revision)
        .ok_or(RepositoryError::PlanRejected)?
        .session = Some(input.successor.clone());
    let transition = family
        .transitions
        .get_mut(&input.context.transition_id)
        .ok_or(RepositoryError::PlanRejected)?;
    transition.candidate_evidence = Some(candidate);
    transition.state = TransitionState::CandidateRecorded;
    transition.updated_at = now;
    Ok(PlannedMutation {
        family: Some(family),
        result: MutationResult::RecordCandidate {
            transition_id: input.context.transition_id.clone(),
            candidate_revision: input.context.candidate_revision,
            session: input.successor.clone(),
        },
    })
}

fn plan_adoption(
    store: &RoleSeatStore,
    input: &RecordAdoptionRequest,
    clock: &dyn TrustedClock,
) -> Result<PlannedMutation, RepositoryError> {
    let existing = family_for_context(store, &input.context)?;
    let transition = existing
        .transitions
        .get(&input.context.transition_id)
        .ok_or(RepositoryError::PlanRejected)?;
    let adoption = AdoptionEvidence {
        identity: input.identity.clone(),
        receipt: input.evidence.clone(),
    };
    if transition.state != TransitionState::CandidateRecorded
        || transition
            .candidate_evidence
            .as_ref()
            .map(|candidate| &candidate.session)
            != Some(&input.candidate_session)
        || transition.adoption_evidence.is_some()
    {
        return rejected();
    }
    valid(validate_runtime_context(&input.context, &input.identity))?;
    valid(validate_adoption_evidence(
        &input.context,
        &input.identity,
        &adoption,
    ))?;
    let now = clock.now();
    let mut family = existing.clone();
    let transition = family
        .transitions
        .get_mut(&input.context.transition_id)
        .ok_or(RepositoryError::PlanRejected)?;
    transition.adoption_evidence = Some(adoption);
    transition.state = TransitionState::Adopted;
    transition.updated_at = now;
    Ok(PlannedMutation {
        family: Some(family),
        result: MutationResult::RecordAdoption {
            transition_id: input.context.transition_id.clone(),
            identity: input.identity.clone(),
        },
    })
}

fn plan_delivery(
    store: &RoleSeatStore,
    input: &RecordInitialDeliveryRequest,
    clock: &dyn TrustedClock,
) -> Result<PlannedMutation, RepositoryError> {
    let existing = family_for_context(store, &input.context)?;
    let transition = existing
        .transitions
        .get(&input.context.transition_id)
        .ok_or(RepositoryError::PlanRejected)?;
    let delivery = DeliveryEvidence {
        delivery_id: input.delivery_id.clone(),
        recipient: input.recipient.clone(),
        receipt: input.evidence.clone(),
    };
    if transition.state != TransitionState::Adopted
        || transition
            .adoption_evidence
            .as_ref()
            .map(|adoption| &adoption.identity)
            != Some(&input.recipient)
        || transition.delivery_evidence.is_some()
    {
        return rejected();
    }
    valid(validate_runtime_context(&input.context, &input.recipient))?;
    valid(validate_delivery_evidence(
        &input.context,
        &input.recipient,
        &delivery,
    ))?;
    let now = clock.now();
    let mut family = existing.clone();
    let transition = family
        .transitions
        .get_mut(&input.context.transition_id)
        .ok_or(RepositoryError::PlanRejected)?;
    transition.delivery_evidence = Some(delivery);
    transition.state = TransitionState::InitialDeliveryRecorded;
    transition.updated_at = now;
    Ok(PlannedMutation {
        family: Some(family),
        result: MutationResult::RecordInitialDelivery {
            transition_id: input.context.transition_id.clone(),
            delivery_id: input.delivery_id.clone(),
        },
    })
}

fn plan_acknowledgement(
    store: &RoleSeatStore,
    input: &RecordAcknowledgementRequest,
    clock: &dyn TrustedClock,
) -> Result<PlannedMutation, RepositoryError> {
    let existing = family_for_context(store, &input.context)?;
    let transition = existing
        .transitions
        .get(&input.context.transition_id)
        .ok_or(RepositoryError::PlanRejected)?;
    let acknowledgement = AcknowledgementEvidence {
        responder: input.responder.clone(),
        handoff_sha256: input.handoff_sha256.clone(),
        receipt: input.evidence.clone(),
    };
    if transition.state != TransitionState::InitialDeliveryRecorded
        || transition.acknowledgement_evidence.is_some()
    {
        return rejected();
    }
    valid(validate_runtime_context(&input.context, &input.responder))?;
    valid(validate_acknowledgement_evidence(
        &input.context,
        &input.responder,
        &acknowledgement,
    ))?;
    let now = clock.now();
    let mut family = existing.clone();
    let transition = family
        .transitions
        .get_mut(&input.context.transition_id)
        .ok_or(RepositoryError::PlanRejected)?;
    transition.acknowledgement_evidence = Some(acknowledgement);
    transition.state = TransitionState::Acknowledged;
    transition.updated_at = now;
    Ok(PlannedMutation {
        family: Some(family),
        result: MutationResult::RecordAcknowledgement {
            transition_id: input.context.transition_id.clone(),
            handoff_sha256: input.handoff_sha256.clone(),
        },
    })
}

fn transfer_fences(
    family: &RoleFamily,
    input: &TransferAuthorityRequest,
) -> Result<(), RepositoryError> {
    let transition = family
        .transitions
        .get(&input.context.transition_id)
        .ok_or(RepositoryError::PlanRejected)?;
    if transition.state != TransitionState::Acknowledged
        || current_authority_snapshot(family).map_err(|_| RepositoryError::PlanRejected)?
            != input.context.intended_predecessor
        || input.expected_authority_epoch != input.context.intended_predecessor.authority_epoch
        || input.fresh_incumbent.cutex_session_id
            != input.context.intended_predecessor.cutex_session_id
        || input.fresh_incumbent.durable_revision
            != input.context.intended_predecessor.source_durable_revision
        || transition
            .candidate_evidence
            .as_ref()
            .map(|candidate| &candidate.session)
            != Some(&input.candidate_session)
        || transition
            .adoption_evidence
            .as_ref()
            .map(|adoption| &adoption.identity)
            != Some(&input.adopted_identity)
    {
        return rejected();
    }
    valid(validate_runtime_context(
        &input.context,
        &input.adopted_identity,
    ))?;
    let mut subjects = transition_subjects(&input.context);
    subjects.push(IdentityRef::CutexSession {
        id: input.fresh_incumbent.cutex_session_id.clone(),
    });
    subjects.extend(runtime_subjects(&input.adopted_identity));
    valid(evidence(
        &input.evidence,
        EvidenceKind::TransferVerification,
        subjects,
    ))
}

fn apply_transfer(
    family: &mut RoleFamily,
    context: &TransitionContext,
    candidate_session: &DurableSessionRef,
    now: &Rfc3339,
) -> Result<AuthorityEpoch, RepositoryError> {
    let next_epoch = context
        .intended_predecessor
        .authority_epoch
        .checked_next()
        .map_err(|_| RepositoryError::PlanRejected)?;
    let predecessor = family
        .revisions
        .get_mut(&context.intended_predecessor.role_revision)
        .ok_or(RepositoryError::PlanRejected)?;
    predecessor.state = RoleRevisionState::Superseded;
    let revision = family
        .revisions
        .get_mut(&context.candidate_revision)
        .ok_or(RepositoryError::PlanRejected)?;
    revision.state = RoleRevisionState::Current;
    revision.successful_predecessor = Some(SuccessfulPredecessor {
        role_revision: context.intended_predecessor.role_revision,
        cutex_session_id: context.intended_predecessor.cutex_session_id.clone(),
        transfer_transition_id: context.transition_id.clone(),
    });
    revision.root_revision = Some(family.root_initialization.chosen_root_revision);
    family.current_authority = CurrentAuthority {
        role_revision: context.candidate_revision,
        cutex_session_id: candidate_session.cutex_session_id.clone(),
        authority_epoch: next_epoch,
        effective_at: EvidenceTime::Known {
            rfc3339: now.clone(),
        },
        established_by: EstablishedBy::Transfer {
            transition_id: context.transition_id.clone(),
        },
    };
    let transition = family
        .transitions
        .get_mut(&context.transition_id)
        .ok_or(RepositoryError::PlanRejected)?;
    transition.state = TransitionState::AuthorityTransferred;
    transition.updated_at = now.clone();
    Ok(next_epoch)
}

fn plan_transfer(
    store: &RoleSeatStore,
    input: &TransferAuthorityRequest,
    clock: &dyn TrustedClock,
) -> Result<PlannedMutation, RepositoryError> {
    let existing = family_for_context(store, &input.context)?;
    transfer_fences(existing, input)?;
    let now = clock.now();
    let mut family = existing.clone();
    let next_epoch = apply_transfer(&mut family, &input.context, &input.candidate_session, &now)?;
    Ok(PlannedMutation {
        family: Some(family),
        result: MutationResult::TransferAuthority {
            transition_id: input.context.transition_id.clone(),
            role_revision: input.context.candidate_revision,
            cutex_session_id: input.candidate_session.cutex_session_id.clone(),
            authority_epoch: next_epoch,
        },
    })
}

fn completion_fences(
    family: &RoleFamily,
    input: &CompleteRotationRequest,
) -> Result<(), RepositoryError> {
    let transition = family
        .transitions
        .get(&input.context.transition_id)
        .ok_or(RepositoryError::PlanRejected)?;
    let expected_epoch = input
        .context
        .intended_predecessor
        .authority_epoch
        .checked_next()
        .map_err(|_| RepositoryError::PlanRejected)?;
    if transition.state != TransitionState::AuthorityTransferred
        || family.current_authority.role_revision != input.context.candidate_revision
        || family.current_authority.cutex_session_id != input.adopted_identity.cutex_session_id
        || family.current_authority.authority_epoch != expected_epoch
        || family.current_authority.established_by
            != (EstablishedBy::Transfer {
                transition_id: input.context.transition_id.clone(),
            })
        || transition
            .adoption_evidence
            .as_ref()
            .map(|adoption| &adoption.identity)
            != Some(&input.adopted_identity)
    {
        return rejected();
    }
    valid(validate_runtime_context(
        &input.context,
        &input.adopted_identity,
    ))?;
    valid(validate_completion_evidence(
        &input.context,
        &input.adopted_identity,
        &input.evidence,
    ))
}

fn apply_completion(
    family: &mut RoleFamily,
    context: &TransitionContext,
    evidence_ref: &EvidenceRef,
    now: &Rfc3339,
) -> Result<(), RepositoryError> {
    let transition = family
        .transitions
        .get_mut(&context.transition_id)
        .ok_or(RepositoryError::PlanRejected)?;
    transition.completion_evidence = Some(evidence_ref.clone());
    transition.state = TransitionState::Completed;
    transition.updated_at = now.clone();
    if family
        .active_rotation
        .as_ref()
        .is_none_or(|lock| lock.transition_id != context.transition_id)
    {
        return rejected();
    }
    family.active_rotation = None;
    Ok(())
}

fn plan_completion(
    store: &RoleSeatStore,
    input: &CompleteRotationRequest,
    clock: &dyn TrustedClock,
) -> Result<PlannedMutation, RepositoryError> {
    let existing = family_for_context(store, &input.context)?;
    completion_fences(existing, input)?;
    let now = clock.now();
    let mut family = existing.clone();
    apply_completion(&mut family, &input.context, &input.evidence, &now)?;
    Ok(PlannedMutation {
        family: Some(family),
        result: MutationResult::CompleteRotation {
            transition_id: input.context.transition_id.clone(),
            role_revision: input.context.candidate_revision,
        },
    })
}

fn terminal_intent_valid(
    input: &TerminalRequest,
    outcome: TerminalOutcome,
) -> Result<(), RepositoryError> {
    valid(validate_runtime_context(
        &input.context,
        &input.adopted_identity,
    ))?;
    if !reason_matches_terminal(input.reason_code, outcome)
        || matches!(
            input.phase,
            RotationPhase::Transfer | RotationPhase::Completion
        )
    {
        return rejected();
    }
    let mut subjects = transition_subjects(&input.context);
    subjects.extend(runtime_subjects(&input.adopted_identity));
    valid(evidence(
        &input.evidence,
        match outcome {
            TerminalOutcome::Failed => EvidenceKind::Failure,
            TerminalOutcome::Cancelled => EvidenceKind::Cancellation,
        },
        subjects,
    ))
}

fn apply_terminal(
    family: &mut RoleFamily,
    context: &TransitionContext,
    attempt: &FailedAttempt,
) -> Result<(), RepositoryError> {
    let transition_state = match attempt.outcome {
        TerminalOutcome::Failed => TransitionState::Failed,
        TerminalOutcome::Cancelled => TransitionState::Cancelled,
    };
    let revision_state = match attempt.outcome {
        TerminalOutcome::Failed => RoleRevisionState::Failed,
        TerminalOutcome::Cancelled => RoleRevisionState::Cancelled,
    };
    let transition = family
        .transitions
        .get_mut(&context.transition_id)
        .ok_or(RepositoryError::PlanRejected)?;
    transition.state = transition_state;
    transition.terminal_attempt = Some(attempt.clone());
    transition.updated_at = attempt.recorded_at.clone();
    let revision = family
        .revisions
        .get_mut(&context.candidate_revision)
        .ok_or(RepositoryError::PlanRejected)?;
    revision.state = revision_state;
    revision.terminal_attempt = Some(attempt.clone());
    family.active_rotation = None;
    Ok(())
}

fn plan_terminal(
    store: &RoleSeatStore,
    input: &TerminalRequest,
    outcome: TerminalOutcome,
    clock: &dyn TrustedClock,
) -> Result<PlannedMutation, RepositoryError> {
    let existing = family_for_context(store, &input.context)?;
    let transition = existing
        .transitions
        .get(&input.context.transition_id)
        .ok_or(RepositoryError::PlanRejected)?;
    if transition.state != expected_prior_state(input.phase)
        || transition.terminal_attempt.is_some()
        || transition
            .unknown_outcomes
            .iter()
            .any(|unknown| unknown.resolution.is_none())
        || transition
            .unknown_outcomes
            .last()
            .is_some_and(|unknown| input.attempt <= unknown.attempt)
    {
        return rejected();
    }
    terminal_intent_valid(input, outcome)?;
    let now = clock.now();
    let attempt = FailedAttempt {
        attempt: input.attempt,
        outcome,
        phase: input.phase,
        reason_code: input.reason_code,
        evidence: input.evidence.clone(),
        recorded_at: now,
    };
    let mut family = existing.clone();
    apply_terminal(&mut family, &input.context, &attempt)?;
    let result = match outcome {
        TerminalOutcome::Failed => MutationResult::FailRotation {
            transition_id: input.context.transition_id.clone(),
            attempt,
        },
        TerminalOutcome::Cancelled => MutationResult::CancelRotation {
            transition_id: input.context.transition_id.clone(),
            attempt,
        },
    };
    Ok(PlannedMutation {
        family: Some(family),
        result,
    })
}

fn plan_record_unknown(
    store: &RoleSeatStore,
    input: &RecordUnknownRequest,
    clock: &dyn TrustedClock,
) -> Result<PlannedMutation, RepositoryError> {
    let existing = family_for_context(store, &input.context)?;
    let transition = existing
        .transitions
        .get(&input.context.transition_id)
        .ok_or(RepositoryError::PlanRejected)?;
    let revision = existing
        .revisions
        .get(&input.context.candidate_revision)
        .ok_or(RepositoryError::PlanRejected)?;
    if input.phase == RotationPhase::Prepare
        || input.attempted_payload.phase() != input.phase
        || transition.state != expected_prior_state(input.phase)
        || !reason_matches_unknown(input.reason_code, input.phase)
        || transition
            .unknown_outcomes
            .iter()
            .any(|unknown| unknown.resolution.is_none())
        || transition
            .unknown_outcomes
            .last()
            .is_some_and(|unknown| input.attempt <= unknown.attempt)
    {
        return rejected();
    }
    valid(validate_runtime_context(
        &input.context,
        &input.adopted_identity,
    ))?;
    let current_evidence = transition_evidence(transition);
    valid(phase_payload_matches(
        &input.context,
        &input.adopted_identity,
        &current_evidence,
        &input.attempted_payload,
    ))?;
    let mut subjects = transition_subjects(&input.context);
    subjects.extend(runtime_subjects(&input.adopted_identity));
    valid(evidence(
        &input.evidence,
        EvidenceKind::UnknownObservation,
        subjects,
    ))?;
    let current_authority =
        current_authority_snapshot(existing).map_err(|_| RepositoryError::PlanRejected)?;
    let lock = existing
        .active_rotation
        .clone()
        .ok_or(RepositoryError::PlanRejected)?;
    let prior = TransitionPriorSnapshot {
        transition_state: transition.state,
        revision_state: revision.state,
        intended_predecessor: transition.intended_predecessor.clone(),
        current_authority: current_authority.clone(),
        active_rotation: lock.clone(),
        evidence: current_evidence.clone(),
    };
    let post_state = UnknownPostState {
        transition_state: TransitionState::Unknown,
        revision_state: revision.state,
        current_authority,
        active_rotation: lock,
        evidence: current_evidence,
    };
    let now = clock.now();
    let unknown = UnknownOutcome {
        initialization_id: input.context.initialization_id.clone(),
        transition_id: input.context.transition_id.clone(),
        attempt: input.attempt,
        phase: input.phase,
        prior,
        attempted_payload: input.attempted_payload.clone(),
        reason_code: input.reason_code,
        evidence: input.evidence.clone(),
        recorded_at: now.clone(),
        post_state,
        resolution: None,
    };
    let mut family = existing.clone();
    let transition = family
        .transitions
        .get_mut(&input.context.transition_id)
        .ok_or(RepositoryError::PlanRejected)?;
    transition.state = TransitionState::Unknown;
    transition.updated_at = now;
    transition.unknown_outcomes.push(unknown.clone());
    Ok(PlannedMutation {
        family: Some(family),
        result: MutationResult::RecordUnknown {
            transition_id: input.context.transition_id.clone(),
            unknown,
        },
    })
}

fn unresolved_for<'a>(
    family: &'a RoleFamily,
    input: &ResolveUnknownRequest,
) -> Result<&'a UnknownOutcome, RepositoryError> {
    let transition = family
        .transitions
        .get(&input.context.transition_id)
        .ok_or(RepositoryError::PlanRejected)?;
    if transition.state != TransitionState::Unknown {
        return rejected();
    }
    let unknown = transition
        .unknown_outcomes
        .iter()
        .find(|unknown| unknown.resolution.is_none())
        .ok_or(RepositoryError::PlanRejected)?;
    if unknown.attempt != input.attempt
        || unknown.post_state.transition_state != transition.state
        || unknown.post_state.evidence != transition_evidence(transition)
        || unknown.post_state.active_rotation
            != family
                .active_rotation
                .clone()
                .ok_or(RepositoryError::PlanRejected)?
        || unknown.post_state.current_authority
            != current_authority_snapshot(family).map_err(|_| RepositoryError::PlanRejected)?
    {
        return rejected();
    }
    Ok(unknown)
}

fn success_post_state(
    family: &RoleFamily,
    context: &TransitionContext,
) -> Result<PhaseSuccessPostState, RepositoryError> {
    let transition = family
        .transitions
        .get(&context.transition_id)
        .ok_or(RepositoryError::PlanRejected)?;
    let revision = family
        .revisions
        .get(&context.candidate_revision)
        .ok_or(RepositoryError::PlanRejected)?;
    Ok(PhaseSuccessPostState {
        transition_state: transition.state,
        revision_state: revision.state,
        current_authority: current_authority_snapshot(family)
            .map_err(|_| RepositoryError::PlanRejected)?,
        active_rotation: family.active_rotation.clone(),
        evidence: transition_evidence(transition),
    })
}

fn apply_verified_success(
    family: &mut RoleFamily,
    context: &TransitionContext,
    phase: RotationPhase,
    payload: &PhasePayload,
    now: &Rfc3339,
) -> Result<(), RepositoryError> {
    match (phase, payload) {
        (RotationPhase::Candidate, PhasePayload::Candidate { candidate }) => {
            family
                .revisions
                .get_mut(&context.candidate_revision)
                .ok_or(RepositoryError::PlanRejected)?
                .session = Some(candidate.session.clone());
            let transition = family
                .transitions
                .get_mut(&context.transition_id)
                .ok_or(RepositoryError::PlanRejected)?;
            transition.candidate_evidence = Some(candidate.clone());
            transition.state = TransitionState::CandidateRecorded;
            transition.updated_at = now.clone();
        }
        (RotationPhase::Adoption, PhasePayload::Adoption { adoption, .. }) => {
            let transition = family
                .transitions
                .get_mut(&context.transition_id)
                .ok_or(RepositoryError::PlanRejected)?;
            transition.adoption_evidence = Some(adoption.clone());
            transition.state = TransitionState::Adopted;
            transition.updated_at = now.clone();
        }
        (RotationPhase::InitialDelivery, PhasePayload::InitialDelivery { delivery }) => {
            let transition = family
                .transitions
                .get_mut(&context.transition_id)
                .ok_or(RepositoryError::PlanRejected)?;
            transition.delivery_evidence = Some(delivery.clone());
            transition.state = TransitionState::InitialDeliveryRecorded;
            transition.updated_at = now.clone();
        }
        (RotationPhase::Acknowledgement, PhasePayload::Acknowledgement { acknowledgement }) => {
            let transition = family
                .transitions
                .get_mut(&context.transition_id)
                .ok_or(RepositoryError::PlanRejected)?;
            transition.acknowledgement_evidence = Some(acknowledgement.clone());
            transition.state = TransitionState::Acknowledged;
            transition.updated_at = now.clone();
        }
        // A transfer is the authority-store mutation itself. An unresolved
        // pre-transfer record cannot be used to manufacture the pointer move.
        (RotationPhase::Transfer, PhasePayload::Transfer { .. }) => return rejected(),
        (RotationPhase::Completion, PhasePayload::Completion { evidence, .. }) => {
            apply_completion(family, context, evidence, now)?
        }
        _ => return rejected(),
    }
    Ok(())
}

fn terminal_post_state(
    family: &RoleFamily,
    context: &TransitionContext,
) -> Result<TerminalPostState, RepositoryError> {
    let transition = family
        .transitions
        .get(&context.transition_id)
        .ok_or(RepositoryError::PlanRejected)?;
    let revision = family
        .revisions
        .get(&context.candidate_revision)
        .ok_or(RepositoryError::PlanRejected)?;
    Ok(TerminalPostState {
        transition_state: transition.state,
        revision_state: revision.state,
        current_authority: current_authority_snapshot(family)
            .map_err(|_| RepositoryError::PlanRejected)?,
        active_rotation: family.active_rotation.clone(),
        evidence: transition_evidence(transition),
    })
}

fn plan_resolve_unknown(
    store: &RoleSeatStore,
    input: &ResolveUnknownRequest,
    clock: &dyn TrustedClock,
) -> Result<PlannedMutation, RepositoryError> {
    let existing = family_for_context(store, &input.context)?;
    let unknown = unresolved_for(existing, input)?.clone();
    valid(validate_runtime_context(
        &input.context,
        &input.adopted_identity,
    ))?;
    let mut resolution_subjects = transition_subjects(&input.context);
    resolution_subjects.extend(runtime_subjects(&input.adopted_identity));
    valid(evidence(
        &input.evidence,
        EvidenceKind::UnknownResolution,
        resolution_subjects.clone(),
    ))?;

    match &input.outcome {
        ResolutionIntent::PhaseSucceeded { verified_payload } => {
            if verified_payload != &unknown.attempted_payload
                || unknown.phase == RotationPhase::Transfer
            {
                return rejected();
            }
        }
        ResolutionIntent::Failed {
            reason_code,
            evidence: terminal_evidence,
        }
        | ResolutionIntent::Cancelled {
            reason_code,
            evidence: terminal_evidence,
        } => {
            let outcome = if matches!(input.outcome, ResolutionIntent::Failed { .. }) {
                TerminalOutcome::Failed
            } else {
                TerminalOutcome::Cancelled
            };
            if matches!(
                unknown.phase,
                RotationPhase::Transfer | RotationPhase::Completion
            ) || !reason_matches_terminal(*reason_code, outcome)
            {
                return rejected();
            }
            valid(evidence(
                terminal_evidence,
                match outcome {
                    TerminalOutcome::Failed => EvidenceKind::Failure,
                    TerminalOutcome::Cancelled => EvidenceKind::Cancellation,
                },
                resolution_subjects.clone(),
            ))?;
        }
    }

    let now = clock.now();
    let mut family = existing.clone();
    let resolution_outcome = match &input.outcome {
        ResolutionIntent::PhaseSucceeded { verified_payload } => {
            apply_verified_success(
                &mut family,
                &input.context,
                unknown.phase,
                verified_payload,
                &now,
            )?;
            ResolutionOutcome::PhaseSucceeded {
                verified_payload: verified_payload.clone(),
                post_state: success_post_state(&family, &input.context)?,
            }
        }
        ResolutionIntent::Failed {
            reason_code,
            evidence,
        } => {
            let attempt = FailedAttempt {
                attempt: unknown.attempt,
                outcome: TerminalOutcome::Failed,
                phase: unknown.phase,
                reason_code: *reason_code,
                evidence: evidence.clone(),
                recorded_at: now.clone(),
            };
            apply_terminal(&mut family, &input.context, &attempt)?;
            ResolutionOutcome::Failed {
                attempt,
                post_state: terminal_post_state(&family, &input.context)?,
            }
        }
        ResolutionIntent::Cancelled {
            reason_code,
            evidence,
        } => {
            let attempt = FailedAttempt {
                attempt: unknown.attempt,
                outcome: TerminalOutcome::Cancelled,
                phase: unknown.phase,
                reason_code: *reason_code,
                evidence: evidence.clone(),
                recorded_at: now.clone(),
            };
            apply_terminal(&mut family, &input.context, &attempt)?;
            ResolutionOutcome::Cancelled {
                attempt,
                post_state: terminal_post_state(&family, &input.context)?,
            }
        }
    };
    let resolution = UnknownResolution {
        outcome: resolution_outcome,
        evidence: input.evidence.clone(),
        recorded_at: now.clone(),
    };
    let transition = family
        .transitions
        .get_mut(&input.context.transition_id)
        .ok_or(RepositoryError::PlanRejected)?;
    let stored = transition
        .unknown_outcomes
        .iter_mut()
        .find(|stored| stored.attempt == input.attempt && stored.resolution.is_none())
        .ok_or(RepositoryError::PlanRejected)?;
    stored.resolution = Some(resolution);
    transition.updated_at = now;
    let resolved = stored.clone();
    Ok(PlannedMutation {
        family: Some(family),
        result: MutationResult::ResolveUnknown {
            transition_id: input.context.transition_id.clone(),
            unknown: resolved,
        },
    })
}
