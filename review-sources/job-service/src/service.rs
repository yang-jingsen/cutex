use crate::grant::{semantic_request_sha256, verify_caller_grant, verify_grant};
use crate::model::*;
use crate::process::{self, LiveProcess};
use crate::store::{Store, StoredState, insert_outbox, project_outbox, read_bounded};
use sha2::Sha256;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct ServiceConfig {
    pub state_root: PathBuf,
    pub grant_key: Vec<u8>,
    pub api_token: Vec<u8>,
    pub runner_executable: PathBuf,
    pub per_stream_output_bytes: u64,
    pub max_read_bytes: usize,
    pub cancel_grace: Duration,
    pub max_jobs: usize,
    pub max_active_jobs: usize,
    pub allowed_launchers: std::collections::BTreeMap<String, String>,
    pub completion_wire_version: CompletionWireVersion,
}

impl ServiceConfig {
    pub fn validate(&self) -> Result<(), JobError> {
        if self.grant_key.len() < 32 {
            return Err(JobError::Invalid(
                "grant key must contain at least 32 bytes".into(),
            ));
        }
        if self.api_token.len() < 32 {
            return Err(JobError::Invalid(
                "API token must contain at least 32 bytes".into(),
            ));
        }
        if !self.runner_executable.is_absolute() || !self.runner_executable.is_file() {
            return Err(JobError::Invalid(
                "runner executable must be an existing absolute file".into(),
            ));
        }
        if !(4096..=64 * 1024 * 1024).contains(&self.per_stream_output_bytes) {
            return Err(JobError::Invalid(
                "per-stream output limit must be 4 KiB..=64 MiB".into(),
            ));
        }
        if !(1..=1024 * 1024).contains(&self.max_read_bytes) {
            return Err(JobError::Invalid("read limit must be 1..=1 MiB".into()));
        }
        if !(Duration::from_millis(100)..=Duration::from_secs(30)).contains(&self.cancel_grace) {
            return Err(JobError::Invalid(
                "cancel grace must be 100 ms..=30 s".into(),
            ));
        }
        if self.max_jobs == 0
            || self.max_jobs > 100_000
            || self.max_active_jobs == 0
            || self.max_active_jobs > 256
            || self.max_active_jobs > self.max_jobs
        {
            return Err(JobError::Invalid("job count limits are invalid".into()));
        }
        if self.allowed_launchers.is_empty() {
            return Err(JobError::Invalid(
                "at least one sandbox launcher is required".into(),
            ));
        }
        for (path, digest) in &self.allowed_launchers {
            let canonical = std::fs::canonicalize(path)?;
            if canonical.to_string_lossy() != path.as_str()
                || crate::grant::file_sha256(&canonical)? != *digest
            {
                return Err(JobError::Invalid(
                    "sandbox launcher allowlist identity mismatch".into(),
                ));
            }
        }
        Ok(())
    }
}

struct Inner {
    config: ServiceConfig,
    store: Store,
    state: Mutex<StoredState>,
    live: Mutex<HashMap<String, LiveProcess>>,
    executable: PathBuf,
}

#[derive(Clone)]
pub struct JobService {
    inner: Arc<Inner>,
}

#[derive(Debug, Clone)]
pub(crate) struct CompletionAttempt {
    pub record: OutboxRecord,
    pub query_only: bool,
}

pub(crate) struct CompletionAttemptResult {
    pub delivery_state: CompletionDeliveryState,
    pub receipt: Option<CompletionReceipt>,
    pub last_error: Option<String>,
    pub next_attempt_at_epoch_millis: u64,
    pub final_for_retention: bool,
}

impl JobService {
    pub fn open(config: ServiceConfig) -> Result<Self, JobError> {
        config.validate()?;
        let (store, state) =
            Store::open(config.state_root.clone(), config.completion_wire_version)?;
        store.ensure_output_dir()?;
        let executable = config.runner_executable.clone();
        Ok(Self {
            inner: Arc::new(Inner {
                config,
                store,
                state: Mutex::new(state),
                live: Mutex::new(HashMap::new()),
                executable,
            }),
        })
    }

    pub fn submit(
        &self,
        api_token: &[u8],
        request: JobRequest,
        grant: ExecutionGrant,
    ) -> Result<SubmitReceipt, JobError> {
        self.authenticate(api_token)?;
        validate_request(&request)?;
        if !request.environment.is_empty() {
            return Err(JobError::Unauthorized(
                "JS1 does not permit request environment injection".into(),
            ));
        }
        let now = now_secs();
        verify_grant(
            &self.inner.config.grant_key,
            &self.inner.config.allowed_launchers,
            &request,
            &grant,
            now,
        )?;
        let digest = semantic_request_sha256(&request)?;
        {
            let state = self.inner.state.lock().expect("state mutex poisoned");
            if let Some(job_id) = state.action_index.get(&request.action_id) {
                let existing = state.jobs.get(job_id).expect("action index is consistent");
                if existing.request_sha256 != digest {
                    return Err(JobError::Conflict(
                        "action ID was reused with different semantics".into(),
                    ));
                }
                return Ok(SubmitReceipt {
                    status: "committed".into(),
                    job: existing.clone(),
                    deduplicated: true,
                });
            }
        }
        let job_id = format!("job_{}", uuid::Uuid::new_v4().simple());
        let output_reference = format!("job-output:{job_id}");
        let record = JobRecord {
            schema: CONTRACT.into(),
            job_id: job_id.clone(),
            revision: 1,
            request_sha256: digest,
            request: PersistedJobRequest::from(&request),
            state: JobState::LaunchPending,
            created_at_epoch_secs: now,
            updated_at_epoch_secs: now,
            process_id: None,
            process_start_ticks: None,
            exit_code: None,
            terminal_reason: None,
            execution: None,
            stdout: StreamSummary {
                retained_bytes: 0,
                observed_bytes: 0,
                truncated: false,
            },
            stderr: StreamSummary {
                retained_bytes: 0,
                observed_bytes: 0,
                truncated: false,
            },
            output_reference,
            completion_delivery: CompletionDeliverySummary::default(),
        };
        {
            let mut state = self.inner.state.lock().expect("state mutex poisoned");
            // CAS after the earlier read covers concurrent submissions in this process.
            if let Some(job_id) = state.action_index.get(&request.action_id) {
                let existing = state.jobs.get(job_id).expect("action index is consistent");
                if existing.request_sha256 != record.request_sha256 {
                    return Err(JobError::Conflict(
                        "action ID was reused with different semantics".into(),
                    ));
                }
                return Ok(SubmitReceipt {
                    status: "committed".into(),
                    job: existing.clone(),
                    deduplicated: true,
                });
            }
            if state.jobs.len() >= self.inner.config.max_jobs {
                return Err(JobError::Conflict("service job-record capacity reached; acknowledged terminal cleanup is required".into()));
            }
            if self.inner.live.lock().expect("live mutex poisoned").len()
                >= self.inner.config.max_active_jobs
            {
                return Err(JobError::Conflict("active job capacity reached".into()));
            }
            state
                .action_index
                .insert(request.action_id.clone(), job_id.clone());
            state.jobs.insert(job_id.clone(), record.clone());
            self.inner.store.save(&mut state)?;
        }

        let sandbox_json = serde_json::to_string(&grant.payload.sandbox_state)?;
        let spawned = match process::spawn_contained(
            &self.inner.executable,
            &grant.payload.launcher_path,
            &sandbox_json,
            &request.argv,
            &request.cwd,
            &request.environment,
            &self.inner.store.output_path(&job_id, "stdout"),
            &self.inner.store.output_path(&job_id, "stderr"),
            self.inner.config.per_stream_output_bytes,
        ) {
            Ok(spawned) => spawned,
            Err(error) => {
                let mut state = self.inner.state.lock().expect("state mutex poisoned");
                let job = state.jobs.get_mut(&job_id).expect("committed job exists");
                job.state = JobState::Failed;
                job.revision += 1;
                job.updated_at_epoch_secs = now_secs();
                job.terminal_reason = Some(format!("launch failed: {error}"));
                let snapshot = job.clone();
                insert_outbox(
                    &mut state.outbox,
                    &snapshot,
                    self.inner.config.completion_wire_version,
                )?;
                if let Some(outbox) = state
                    .outbox
                    .values()
                    .find(|item| item.job_id == job_id)
                    .cloned()
                    && let Some(job) = state.jobs.get_mut(&job_id)
                {
                    project_outbox(job, &outbox);
                }
                self.inner.store.save(&mut state)?;
                return Ok(SubmitReceipt {
                    status: "committed".into(),
                    job: snapshot,
                    deduplicated: false,
                });
            }
        };
        let pid = spawned.live.pid;
        let start_ticks = spawned.live.start_ticks;
        let cancelled = Arc::clone(&spawned.live.cancelled);
        self.inner
            .live
            .lock()
            .expect("live mutex poisoned")
            .insert(job_id.clone(), spawned.live);
        {
            let mut state = self.inner.state.lock().expect("state mutex poisoned");
            let job = state.jobs.get_mut(&job_id).expect("committed job exists");
            job.state = JobState::Running;
            job.revision += 1;
            job.updated_at_epoch_secs = now_secs();
            job.process_id = Some(pid);
            job.process_start_ticks = Some(start_ticks);
            job.execution = Some(ExecutionObservation {
                basis: process::EXECUTION_OBSERVATION_BASIS.into(),
                start_observed_at_epoch_millis: spawned.execution_start.wall_epoch_millis,
                exit_observed_at_epoch_millis: None,
                observed_run_duration_millis: None,
            });
            self.inner.store.save(&mut state)?;
        }
        let inner = Arc::clone(&self.inner);
        let watcher_job_id = job_id.clone();
        std::thread::spawn(move || {
            let mut child = spawned.child;
            let status = child.wait();
            let wait_elapsed = spawned.execution_start.monotonic.elapsed();
            let wait_wall = SystemTime::now();
            // Closing the liveness descriptor asks the sentinel to sweep descendants.
            let live = inner
                .live
                .lock()
                .expect("live mutex poisoned")
                .remove(&watcher_job_id);
            drop(live);
            let mut sentinel = spawned.sentinel;
            let _ = sentinel.wait();
            let _ = spawned.stdout_drain.join();
            let _ = spawned.stderr_drain.join();
            let mut state = inner.state.lock().expect("state mutex poisoned");
            let Some(job) = state.jobs.get_mut(&watcher_job_id) else {
                return;
            };
            let observed_out = spawned
                .stdout_observed
                .load(std::sync::atomic::Ordering::Relaxed);
            let observed_err = spawned
                .stderr_observed
                .load(std::sync::atomic::Ordering::Relaxed);
            job.stdout = summary(
                observed_out,
                &inner.store.output_path(&watcher_job_id, "stdout"),
            );
            job.stderr = summary(
                observed_err,
                &inner.store.output_path(&watcher_job_id, "stderr"),
            );
            job.process_id = None;
            job.process_start_ticks = None;
            job.revision += 1;
            job.updated_at_epoch_secs = now_secs();
            match status {
                Ok(status) => {
                    if let Some(observation) = job.execution.as_mut() {
                        process::complete_observation(observation, wait_wall, wait_elapsed);
                    }
                    job.exit_code = process::exit_code(status);
                    if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                        job.state = JobState::Cancelled;
                        job.terminal_reason = Some("cancelled".into());
                    } else if status.success() {
                        job.state = JobState::Exited;
                    } else {
                        job.state = JobState::Failed;
                        job.terminal_reason = Some("command exited unsuccessfully".into());
                    }
                }
                Err(error) => {
                    job.state = JobState::LaunchUnknown;
                    job.terminal_reason = Some(format!("wait failed: {error}"));
                }
            }
            let snapshot = job.clone();
            let persisted = insert_outbox(
                &mut state.outbox,
                &snapshot,
                inner.config.completion_wire_version,
            )
            .and_then(|_| {
                if let Some(outbox) = state
                    .outbox
                    .values()
                    .find(|item| item.job_id == watcher_job_id)
                    .cloned()
                    && let Some(job) = state.jobs.get_mut(&watcher_job_id)
                {
                    project_outbox(job, &outbox);
                }
                inner.store.save(&mut state)
            });
            let _ = persisted;
        });
        Ok(SubmitReceipt {
            status: "committed".into(),
            job: self.query_inner(&job_id)?,
            deduplicated: false,
        })
    }

    pub fn query(&self, api_token: &[u8], job_id: &str) -> Result<JobRecord, JobError> {
        self.authenticate(api_token)?;
        self.query_inner(job_id)
    }

    pub fn query_for(
        &self,
        api_token: &[u8],
        grant: &CallerGrant,
        job_id: &str,
    ) -> Result<JobRecord, JobError> {
        self.authenticate(api_token)?;
        let job = self.query_inner(job_id)?;
        self.authorize_job_caller(grant, CallerOperation::Query, &job)?;
        Ok(job)
    }

    fn query_inner(&self, job_id: &str) -> Result<JobRecord, JobError> {
        self.inner
            .state
            .lock()
            .expect("state mutex poisoned")
            .jobs
            .get(job_id)
            .cloned()
            .ok_or_else(|| JobError::NotFound(job_id.into()))
    }

    pub fn cancel(
        &self,
        api_token: &[u8],
        job_id: &str,
        expected_revision: u64,
    ) -> Result<JobRecord, JobError> {
        self.authenticate(api_token)?;
        let current = self.query_inner(job_id)?;
        if current.revision != expected_revision {
            return Err(JobError::Conflict("job revision mismatch".into()));
        }
        if current.state.terminal() {
            return Ok(current);
        }
        let live = self.inner.live.lock().expect("live mutex poisoned");
        let process = live.get(job_id).ok_or_else(|| {
            JobError::Conflict("running record has no locally owned process".into())
        })?;
        process::cancel(process, self.inner.config.cancel_grace)?;
        self.query_inner(job_id)
    }

    pub fn cancel_for(
        &self,
        api_token: &[u8],
        grant: &CallerGrant,
        job_id: &str,
        expected_revision: u64,
    ) -> Result<JobRecord, JobError> {
        self.authenticate(api_token)?;
        let job = self.query_inner(job_id)?;
        self.authorize_job_caller(grant, CallerOperation::Cancel, &job)?;
        self.cancel(api_token, job_id, expected_revision)
    }

    pub fn read_output(
        &self,
        api_token: &[u8],
        job_id: &str,
        stream: &str,
        offset: u64,
        max_bytes: usize,
    ) -> Result<OutputPage, JobError> {
        self.authenticate(api_token)?;
        let job = self.query_inner(job_id)?;
        if stream != "stdout" && stream != "stderr" {
            return Err(JobError::Invalid("stream must be stdout or stderr".into()));
        }
        let max = max_bytes.min(self.inner.config.max_read_bytes);
        let path = self.inner.store.output_path(job_id, stream);
        let (bytes, from) = if path.exists() {
            read_bounded(&path, offset, max)?
        } else {
            (Vec::new(), 0)
        };
        let summary = if stream == "stdout" {
            &job.stdout
        } else {
            &job.stderr
        };
        Ok(OutputPage {
            job_id: job_id.into(),
            stream: stream.into(),
            from_offset: from,
            next_offset: from + bytes.len() as u64,
            bytes_hex: hex::encode(bytes),
            gap: offset > summary.retained_bytes,
            truncated: summary.truncated,
        })
    }

    pub fn read_output_for(
        &self,
        api_token: &[u8],
        grant: &CallerGrant,
        job_id: &str,
        stream: &str,
        offset: u64,
        max_bytes: usize,
    ) -> Result<OutputPage, JobError> {
        self.authenticate(api_token)?;
        let job = self.query_inner(job_id)?;
        self.authorize_job_caller(grant, CallerOperation::ReadOutput, &job)?;
        self.read_output(api_token, job_id, stream, offset, max_bytes)
    }

    pub fn pending_outbox(&self, api_token: &[u8]) -> Result<Vec<OutboxRecord>, JobError> {
        self.authenticate(api_token)?;
        Ok(self
            .inner
            .state
            .lock()
            .expect("state mutex poisoned")
            .outbox
            .values()
            .filter(|item| !item.acknowledged)
            .cloned()
            .collect())
    }

    pub fn acknowledge_outbox(
        &self,
        api_token: &[u8],
        event_id: &str,
        result_sha256: &str,
    ) -> Result<(), JobError> {
        self.authenticate(api_token)?;
        let mut state = self.inner.state.lock().expect("state mutex poisoned");
        let item = state
            .outbox
            .get_mut(event_id)
            .ok_or_else(|| JobError::NotFound(event_id.into()))?;
        if item.result_sha256 != result_sha256 {
            return Err(JobError::Conflict("outbox digest mismatch".into()));
        }
        if !matches!(
            item.delivery_state,
            CompletionDeliveryState::Delivered | CompletionDeliveryState::Orphaned
        ) {
            return Err(JobError::Conflict(
                "outbox is not in a terminal delivery state".into(),
            ));
        }
        item.acknowledged = true;
        self.inner.store.save(&mut state)
    }

    pub(crate) fn claim_due_completion(
        &self,
        now_millis: u64,
    ) -> Result<Option<CompletionAttempt>, JobError> {
        let mut state = self.inner.state.lock().expect("state mutex poisoned");
        let Some(event_id) = state
            .outbox
            .iter()
            .find(|(_, item)| {
                !item.acknowledged
                    && item.next_attempt_at_epoch_millis <= now_millis
                    && matches!(
                        item.delivery_state,
                        CompletionDeliveryState::Disabled
                            | CompletionDeliveryState::Ready
                            | CompletionDeliveryState::RetryPending
                            | CompletionDeliveryState::AcceptedPending
                            | CompletionDeliveryState::Archived
                            | CompletionDeliveryState::Unavailable
                    )
            })
            .map(|(event_id, _)| event_id.clone())
        else {
            return Ok(None);
        };
        let item = state
            .outbox
            .get_mut(&event_id)
            .expect("selected outbox exists");
        let query_only = matches!(
            item.delivery_state,
            CompletionDeliveryState::AcceptedPending | CompletionDeliveryState::Archived
        );
        item.delivery_state = CompletionDeliveryState::Sending;
        item.attempt_count = item.attempt_count.saturating_add(1);
        item.last_attempt_at_epoch_millis = Some(now_millis);
        item.last_error = None;
        let record = item.clone();
        if let Some(job) = state.jobs.get_mut(&record.job_id) {
            project_outbox(job, &record);
        }
        self.inner.store.save(&mut state)?;
        Ok(Some(CompletionAttempt { record, query_only }))
    }

    pub(crate) fn finish_completion_attempt(
        &self,
        event_id: &str,
        expected_attempt: u32,
        result: CompletionAttemptResult,
    ) -> Result<(), JobError> {
        let mut state = self.inner.state.lock().expect("state mutex poisoned");
        let item = state
            .outbox
            .get_mut(event_id)
            .ok_or_else(|| JobError::NotFound(event_id.into()))?;
        if item.attempt_count != expected_attempt
            || item.delivery_state != CompletionDeliveryState::Sending
        {
            return Err(JobError::Conflict(
                "completion delivery attempt changed before commit".into(),
            ));
        }
        item.delivery_state = result.delivery_state;
        item.receipt = result.receipt;
        item.last_error = result.last_error;
        item.next_attempt_at_epoch_millis = result.next_attempt_at_epoch_millis;
        item.acknowledged = result.final_for_retention;
        let projection = item.clone();
        if let Some(job) = state.jobs.get_mut(&projection.job_id) {
            project_outbox(job, &projection);
        }
        self.inner.store.save(&mut state)
    }

    fn authenticate(&self, supplied: &[u8]) -> Result<(), JobError> {
        use hmac::{Hmac, Mac};
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.inner.config.api_token).expect("validated token");
        mac.update(b"cutex-job-service-api-v1");
        let expected = mac.finalize().into_bytes();
        let mut candidate = Hmac::<Sha256>::new_from_slice(supplied)
            .map_err(|_| JobError::Unauthorized("invalid API credential".into()))?;
        candidate.update(b"cutex-job-service-api-v1");
        candidate
            .verify_slice(&expected)
            .map_err(|_| JobError::Unauthorized("invalid API credential".into()))
    }

    fn authorize_job_caller(
        &self,
        grant: &CallerGrant,
        operation: CallerOperation,
        job: &JobRecord,
    ) -> Result<(), JobError> {
        verify_caller_grant(
            &self.inner.config.grant_key,
            grant,
            operation,
            &job.job_id,
            &job.request.subscriber_cutex_session_id,
            now_secs(),
        )
    }
}

fn validate_request(request: &JobRequest) -> Result<(), JobError> {
    if request.action_id.trim().is_empty() || request.action_id.len() > 256 {
        return Err(JobError::Invalid("action ID is blank or too long".into()));
    }
    if request.argv.is_empty()
        || request.argv.len() > 1024
        || request.argv.iter().map(String::len).sum::<usize>() > 128 * 1024
        || request.argv.iter().any(|v| v.contains('\0'))
    {
        return Err(JobError::Invalid(
            "argv is empty, too large, or contains NUL".into(),
        ));
    }
    let cwd = std::path::Path::new(&request.cwd);
    if !cwd.is_absolute() || !cwd.is_dir() {
        return Err(JobError::Invalid(
            "cwd must be an existing absolute directory".into(),
        ));
    }
    if request.subscriber_cutex_session_id.trim().is_empty() {
        return Err(JobError::Invalid(
            "subscriber durable session is required".into(),
        ));
    }
    if request.origin.runtime_agent_id.trim().is_empty()
        || request.origin.native_thread_id.trim().is_empty()
        || !matches!(
            request.origin.permission_profile_type.as_str(),
            "managed" | "disabled"
        )
    {
        return Err(JobError::Invalid(
            "trusted execution origin is missing or unsupported".into(),
        ));
    }
    if request.environment.len() > 64
        || request.environment.iter().any(|(k, v)| {
            k.is_empty()
                || k.contains('=')
                || k.contains('\0')
                || v.contains('\0')
                || k.len() > 128
                || v.len() > 8192
        })
    {
        return Err(JobError::Invalid(
            "environment is outside explicit bounds".into(),
        ));
    }
    Ok(())
}

fn summary(observed: u64, path: &std::path::Path) -> StreamSummary {
    let retained = std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    StreamSummary {
        retained_bytes: retained.min(observed),
        observed_bytes: observed,
        truncated: retained < observed,
    }
}

pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
