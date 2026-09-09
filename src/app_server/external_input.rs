//! U+S6 generic trusted-controller adapter. No Bus ACK, business routing or tools.
//! Transport uncertainty never means absence; retry uses the same envelope/key.
use anyhow::{ensure, Context};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::client::{AppServerClient, AppServerClientOptions, AppServerEndpoint, AppServerEvent};
use crate::agent_management::{ExplicitLaunchActionReceipt, StockRuntimeStage};
use crate::launch::stock::{ExternalInputBinding, StockBundle};
use crate::session::model::CutexAppServerRuntimeBinding;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Agent,
    Service,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub kind: SourceKind,
    pub id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    AfterTurn,
    Passive,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub id: String,
    pub source: Source,
    #[serde(rename = "type")]
    pub event_type: String,
    pub delivery: Delivery,
    pub text: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Envelope {
    pub version: u32,
    pub owner_id: String,
    pub thread_id: String,
    pub runtime_generation: u64,
    pub message: Message,
    pub semantic_sha256: String,
}
fn bounded(value: &str, max: usize) -> anyhow::Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= max,
        "invalid ExternalInput byte length"
    );
    Ok(())
}
fn digest_hex(value: &str) -> anyhow::Result<()> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid ExternalInput digest"
    );
    Ok(())
}
fn valid_attempt(value: &Option<String>) -> bool {
    value.as_ref().is_none_or(|s| {
        uuid::Uuid::parse_str(s).is_ok_and(|id| !id.is_nil() && id.to_string() == *s)
    })
}
fn framed(domain: &[u8], fields: &[&str]) -> Sha256 {
    let mut hash = Sha256::new();
    hash.update(domain);
    for field in fields {
        hash.update((field.len() as u64).to_be_bytes());
        hash.update(field.as_bytes());
    }
    hash
}
impl Envelope {
    pub fn digest(&self) -> String {
        format!(
            "{:x}",
            framed(
                b"codex:external-input:v1\0",
                &[
                    &self.owner_id,
                    &self.thread_id,
                    &self.message.id,
                    match self.message.source.kind {
                        SourceKind::Agent => "agent",
                        SourceKind::Service => "service",
                    },
                    &self.message.source.id,
                    &self.message.event_type,
                    match self.message.delivery {
                        Delivery::AfterTurn => "after_turn",
                        Delivery::Passive => "passive",
                    },
                    &self.message.text
                ]
            )
            .finalize()
        )
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        ensure!(self.version == 1, "unsupported ExternalInput version");
        for field in [
            &self.owner_id,
            &self.thread_id,
            &self.message.id,
            &self.message.source.id,
            &self.message.event_type,
        ] {
            bounded(field, 256)?;
        }
        bounded(&self.message.text, 65536)?;
        ensure!(
            self.semantic_sha256 == self.digest(),
            "ExternalInput semantic conflict"
        );
        Ok(())
    }
    pub fn key(&self) -> MessageKey {
        MessageKey {
            message_id: self.message.id.clone(),
            semantic_sha256: self.semantic_sha256.clone(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageKey {
    pub message_id: String,
    pub semantic_sha256: String,
}
impl MessageKey {
    fn validate(&self) -> anyhow::Result<()> {
        bounded(&self.message_id, 256)?;
        digest_hex(&self.semantic_sha256)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub schema: String,
    pub receipt_id: String,
    pub owner_id: String,
    pub thread_id: String,
    pub message_id: String,
    pub semantic_sha256: String,
    pub response_item_id: String,
    pub turn_id: String,
    pub ordinal: u64,
}
impl Receipt {
    pub fn digest_id(&self) -> String {
        let mut hash = framed(
            b"codex:external-input-receipt:v1\0",
            &[
                &self.owner_id,
                &self.thread_id,
                &self.message_id,
                &self.semantic_sha256,
                &self.response_item_id,
                &self.turn_id,
            ],
        );
        hash.update(self.ordinal.to_be_bytes());
        format!("eir1_{:x}", hash.finalize())
    }
    pub(crate) fn validate(
        &self,
        binding: &ExternalInputBinding,
        key: &MessageKey,
    ) -> anyhow::Result<()> {
        ensure!(
            self.schema == "codex.external-input-receipt.v1"
                && self.ordinal > 0
                && self.owner_id == binding.owner_id
                && self.thread_id == binding.thread_id
                && self.message_id == key.message_id
                && self.response_item_id == key.message_id
                && self.semantic_sha256 == key.semantic_sha256
                && self.receipt_id == self.digest_id(),
            "ExternalInput receipt conflict"
        );
        bounded(&self.turn_id, 256)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    Unknown,
    Pending,
    ContextPersisted,
    Conflict,
    RetryableError,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingState {
    None,
    Pending,
    Claimed,
    OutputObserved,
    Held,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldReason {
    CanonicalSizePolicy,
    PlanMode,
    Interrupted,
    RequestUncertain,
    NoOutput,
    ContextMissing,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessingStatus {
    pub state: ProcessingState,
    pub attempt_id: Option<String>,
    pub reason: Option<HoldReason>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Status {
    pub message_id: String,
    pub semantic_sha256: String,
    pub delivery_state: DeliveryState,
    pub receipt: Option<Receipt>,
    pub processing: ProcessingStatus,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Response {
    pub version: u32,
    pub owner_id: String,
    pub thread_id: String,
    pub runtime_generation: u64,
    pub statuses: Vec<Status>,
}
impl Response {
    pub fn validate(
        &self,
        binding: &ExternalInputBinding,
        keys: &[MessageKey],
    ) -> anyhow::Result<()> {
        ensure!(
            self.version == 1
                && self.owner_id == binding.owner_id
                && self.thread_id == binding.thread_id
                && self.runtime_generation == binding.runtime_generation
                && self.statuses.len() == keys.len(),
            "ExternalInput response occurrence/count mismatch"
        );
        for (status, key) in self.statuses.iter().zip(keys) {
            ensure!(
                status.message_id == key.message_id
                    && status.semantic_sha256 == key.semantic_sha256,
                "ExternalInput response key/order mismatch"
            );
            match (&status.delivery_state, &status.receipt) {
                (DeliveryState::ContextPersisted, Some(r)) => r.validate(binding, key)?,
                (DeliveryState::ContextPersisted, None) => anyhow::bail!("A4 missing receipt"),
                (_, Some(_)) => anyhow::bail!("non-A4 has receipt"),
                (_, None) => {}
            }
            let p = &status.processing;
            ensure!(valid_attempt(&p.attempt_id), "invalid processing attempt");
            ensure!(
                match p.state {
                    ProcessingState::None => p.attempt_id.is_none() && p.reason.is_none(),
                    ProcessingState::Pending => p.reason.is_none(),
                    ProcessingState::Claimed | ProcessingState::OutputObserved =>
                        p.attempt_id.is_some() && p.reason.is_none(),
                    ProcessingState::Held =>
                        p.reason.is_some()
                            && (!matches!(
                                p.reason,
                                Some(HoldReason::RequestUncertain | HoldReason::NoOutput)
                            ) || p.attempt_id.is_some()),
                },
                "invalid ExternalInput processing state"
            );
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Retry {
    pub message_id: String,
    pub semantic_sha256: String,
    pub expected_attempt_id: Option<String>,
    pub retry_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryDisposition {
    Released,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetryResponse {
    pub version: u32,
    pub owner_id: String,
    pub thread_id: String,
    pub runtime_generation: u64,
    pub message_id: String,
    pub semantic_sha256: String,
    pub expected_attempt_id: Option<String>,
    pub retry_id: String,
    pub disposition: RetryDisposition,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StatusChanged {
    pub thread_id: String,
    pub message_id: String,
}

/// No credentials or caller-chosen endpoint. Obtained only from a current Ready
/// receipt in the caller's explicitly selected store. A socket is not a writer.
pub struct ExternalInputClient {
    path: PathBuf,
    binding: ExternalInputBinding,
    runtime: CutexAppServerRuntimeBinding,
    client: AppServerClient,
    artifacts: PinnedArtifacts,
}

/// Scoped to one owned connection, not a persistent authority/cache. Hash once
/// between matching file-identity snapshots; every occurrence fence checks them
/// again. Replacement/write/chmod, including same-length writes, invalidates it.
struct PinnedArtifacts {
    contract: crate::agent_management::ExplicitLaunchContract,
    bundle: StockBundle,
    stamps: Vec<(PathBuf, Vec<i128>)>,
}
impl PinnedArtifacts {
    fn stamps(
        contract: &crate::agent_management::ExplicitLaunchContract,
        bundle: &StockBundle,
    ) -> anyhow::Result<Vec<(PathBuf, Vec<i128>)>> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            [
                &contract.bundle_manifest,
                &bundle.executable.path,
                &bundle.code_mode_host.path,
                &bundle.facade.path,
                &bundle.schema.path,
                &bundle.shared_config.path,
            ]
            .into_iter()
            .map(|path| {
                ensure!(
                    path.canonicalize()? == *path,
                    "ingress artifact path changed/symlinked"
                );
                let m = std::fs::symlink_metadata(path)?;
                ensure!(m.is_file(), "ingress artifact is not regular");
                Ok((
                    path.clone(),
                    vec![
                        m.dev() as i128,
                        m.ino() as i128,
                        m.len() as i128,
                        m.mtime() as i128,
                        m.mtime_nsec() as i128,
                        m.ctime() as i128,
                        m.ctime_nsec() as i128,
                        m.mode() as i128,
                        m.uid() as i128,
                        m.gid() as i128,
                    ],
                ))
            })
            .collect()
        }
        #[cfg(not(unix))]
        anyhow::bail!("native artifact fence requires Unix")
    }
    fn load(contract: crate::agent_management::ExplicitLaunchContract) -> anyhow::Result<Self> {
        let proposed: StockBundle =
            serde_json::from_slice(&std::fs::read(&contract.bundle_manifest)?)?;
        let before = Self::stamps(&contract, &proposed)?;
        let bundle = StockBundle::load(&contract)?;
        ensure!(
            bundle == proposed && before == Self::stamps(&contract, &bundle)?,
            "ingress artifacts changed during verification"
        );
        Ok(Self {
            contract,
            bundle,
            stamps: before,
        })
    }
    fn check(&self) -> anyhow::Result<()> {
        ensure!(
            self.stamps == Self::stamps(&self.contract, &self.bundle)?,
            "ingress verified artifact changed; reconnect/review required"
        );
        Ok(())
    }
}
fn occurrence(
    path: &Path,
    owner: &str,
    generation: u64,
    artifacts: &PinnedArtifacts,
) -> anyhow::Result<(ExternalInputBinding, CutexAppServerRuntimeBinding)> {
    let store = crate::session::store::load_cutex_session_store_from_path(path)?;
    let record = store.sessions.get(owner).context("ingress owner absent")?;
    ensure!(
        record.cutex_session_id == owner
            && !record.is_retired()
            && record.runtime_generation == generation
            && record.app_server_launch_claim_id.is_none(),
        "ingress owner stale/archived/not ready"
    );
    let contract = record
        .explicit_launch
        .as_ref()
        .context("ingress explicit launch missing")?;
    ensure!(
        *contract == artifacts.contract,
        "ingress launch contract changed"
    );
    artifacts.check()?;
    let bundle = &artifacts.bundle;
    ensure!(
        bundle.common_ingress(),
        "unchanged stock is registration-only; common ingress unavailable"
    );
    ensure!(
        record.codex_session_id.as_deref() == Some(&contract.native_id),
        "ingress native identity changed"
    );
    ensure!(
        store
            .sessions
            .values()
            .filter(|r| r.codex_session_id.as_deref() == Some(&contract.native_id))
            .count()
            == 1,
        "ingress native identity ambiguous"
    );
    let runtime = record
        .app_server_runtime
        .as_ref()
        .context("ingress runtime offline")?;
    ensure!(
        runtime.schema_sha256 == bundle.schema.sha256.as_str(),
        "ingress runtime schema mismatch"
    );
    #[cfg(target_os = "linux")]
    {
        // Exact recorded owned PID only; no discovery or process-argument scan.
        ensure!(
            runtime.pid > 1
                && std::fs::read_link(format!("/proc/{}/exe", runtime.pid))?
                    == bundle.executable.path,
            "ingress native owner absent/changed"
        );
        let actual = crate::platform::process::process_started_at(runtime.pid)?;
        let expected = chrono::DateTime::parse_from_rfc3339(&runtime.started_at)?;
        ensure!(
            actual.timestamp() == expected.timestamp(),
            "ingress native process occurrence changed"
        );
    }
    let mut receipts = store
        .explicit_launch_receipts
        .values()
        .filter_map(|value| match value {
            ExplicitLaunchActionReceipt::Runtime(r)
                if r.stage == StockRuntimeStage::Ready
                    && r.review.subject.cutex_session_id.as_str() == owner
                    && r.review.contract == *contract
                    && r.expected_generation == generation
                    && r.binding.as_ref() == Some(runtime)
                    && record.current_runtime_agent_id.as_deref() == Some(&r.runtime_agent_id) =>
            {
                Some(r)
            }
            _ => None,
        });
    let ready = receipts
        .next()
        .context("ingress authoritative Ready receipt missing")?;
    ensure!(
        receipts.next().is_none(),
        "ingress Ready occurrence ambiguous"
    );
    let binding = ExternalInputBinding::from_review(ready);
    binding.verify(Path::new(&runtime.runtime_dir))?;
    Ok((binding, runtime.clone()))
}
impl ExternalInputClient {
    pub fn connect(path: &Path, owner: &str, generation: u64) -> anyhow::Result<Self> {
        let store = crate::session::store::load_cutex_session_store_from_path(path)?;
        let contract = store
            .sessions
            .get(owner)
            .and_then(|r| r.explicit_launch.clone())
            .context("ingress explicit launch absent")?;
        let artifacts = PinnedArtifacts::load(contract)?;
        let (binding, runtime) = occurrence(path, owner, generation, &artifacts)?;
        let endpoint = super::runtime::endpoint_from_runtime_binding(&runtime)?;
        match &endpoint {
            #[cfg(unix)]
            AppServerEndpoint::UnixSocket { .. } => {}
            _ => anyhow::bail!("ingress requires private Unix endpoint"),
        }
        let client = AppServerClient::connect(AppServerClientOptions::new(endpoint))?;
        ensure!(
            client
                .initialize_response()
                .get("externalInputVersion")
                .and_then(serde_json::Value::as_u64)
                == Some(1),
            "ingress capability unavailable/mismatched"
        );
        let result = Self {
            path: path.into(),
            binding,
            runtime,
            client,
            artifacts,
        };
        result.fence()?;
        Ok(result)
    }
    pub fn binding(&self) -> &ExternalInputBinding {
        &self.binding
    }
    pub(crate) fn fence(&self) -> anyhow::Result<()> {
        ensure!(
            occurrence(
                &self.path,
                &self.binding.owner_id,
                self.binding.runtime_generation,
                &self.artifacts
            )? == (self.binding.clone(), self.runtime.clone()),
            "ingress occurrence changed; reconcile exact key on current owner"
        );
        Ok(())
    }
    fn params(&self) -> serde_json::Value {
        serde_json::json!({"version":1,"ownerId":self.binding.owner_id,"threadId":self.binding.thread_id,
            "runtimeGeneration":self.binding.runtime_generation})
    }
    fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        self.fence()?;
        let result = self.client.handle().request(method, params);
        // Even an error/lost reply is fenced; never commit a stale response.
        self.fence()?;
        result.context("native ingress RPC outcome: reconcile the same key; never infer absence from a transport error")
    }
    pub fn submit(&self, envelope: &Envelope) -> anyhow::Result<Response> {
        envelope.validate()?;
        ensure!(
            envelope.owner_id == self.binding.owner_id
                && envelope.thread_id == self.binding.thread_id
                && envelope.runtime_generation == self.binding.runtime_generation,
            "ingress envelope occurrence mismatch"
        );
        let response: Response = serde_json::from_value(self.request(
            "thread/externalInput/submit",
            serde_json::to_value(envelope)?,
        )?)?;
        response.validate(&self.binding, &[envelope.key()])?;
        Ok(response)
    }
    pub fn status(&self, keys: &[MessageKey]) -> anyhow::Result<Response> {
        ensure!(
            !keys.is_empty() && keys.len() <= 100,
            "ingress status requires 1..100 keys"
        );
        for key in keys {
            key.validate()?;
        }
        let mut params = self.params();
        params["messages"] = serde_json::to_value(keys)?;
        let response: Response =
            serde_json::from_value(self.request("thread/externalInput/status", params)?)?;
        response.validate(&self.binding, keys)?;
        Ok(response)
    }
    /// Calling controller must have an explicit retry decision. Never automatic,
    /// never exposed to model tools; native validates historical attempt CAS.
    pub fn retry(&self, retry: &Retry) -> anyhow::Result<RetryResponse> {
        bounded(&retry.message_id, 256)?;
        bounded(&retry.retry_id, 256)?;
        digest_hex(&retry.semantic_sha256)?;
        ensure!(
            valid_attempt(&retry.expected_attempt_id),
            "invalid retry attempt"
        );
        let mut params = self.params();
        params
            .as_object_mut()
            .unwrap()
            .extend(serde_json::to_value(retry)?.as_object().unwrap().clone());
        let r: RetryResponse =
            serde_json::from_value(self.request("thread/externalInput/retry", params)?)?;
        ensure!(
            r.version == 1
                && r.owner_id == self.binding.owner_id
                && r.thread_id == self.binding.thread_id
                && r.runtime_generation == self.binding.runtime_generation
                && r.message_id == retry.message_id
                && r.semantic_sha256 == retry.semantic_sha256
                && r.expected_attempt_id == retry.expected_attempt_id
                && r.retry_id == retry.retry_id,
            "ingress retry receipt mismatch"
        );
        Ok(r)
    }
    /// Hints only. No hidden status polling, ACK or wake. Other native events are
    /// left to the normal owner; this control connection never answers approvals.
    pub fn next_hint(&self, timeout: Duration) -> anyhow::Result<Option<StatusChanged>> {
        self.fence()?;
        let event = self.client.recv_event_timeout(timeout)?;
        self.fence()?;
        match event {
            Some(AppServerEvent::Notification(n))
                if n.method == "thread/externalInput/statusChanged" =>
            {
                let hint: StatusChanged =
                    serde_json::from_value(n.params.context("missing statusChanged params")?)?;
                ensure!(
                    hint.thread_id == self.binding.thread_id,
                    "foreign ingress hint"
                );
                bounded(&hint.message_id, 256)?;
                Ok(Some(hint))
            }
            Some(
                AppServerEvent::Disconnected { .. } | AppServerEvent::ProtocolViolation { .. },
            ) => anyhow::bail!("ingress observation unavailable"),
            _ => Ok(None),
        }
    }

    /// Existing bridge owner drains its own bounded control subscription. Hints
    /// are not ACKs. Exhausting the bound closes/reconciles instead of spinning.
    pub(crate) fn drain_hints(&self) -> anyhow::Result<()> {
        self.fence()?;
        for _ in 0..256 {
            match self.client.recv_event_timeout(Duration::ZERO)? {
                None => {
                    self.fence()?;
                    return Ok(());
                }
                Some(AppServerEvent::Notification(n))
                    if n.method == "thread/externalInput/statusChanged" =>
                {
                    let hint: StatusChanged =
                        serde_json::from_value(n.params.context("missing ingress hint")?)?;
                    ensure!(
                        hint.thread_id == self.binding.thread_id,
                        "foreign ingress hint"
                    );
                    bounded(&hint.message_id, 256)?;
                }
                Some(
                    AppServerEvent::Disconnected { .. } | AppServerEvent::ProtocolViolation { .. },
                ) => anyhow::bail!("ingress observation disconnected; reconcile durable keys"),
                _ => {}
            }
        }
        anyhow::bail!("ingress event drain bound reached; reconnect and reconcile")
    }
}

#[cfg(test)]
#[path = "external_input_tests.rs"]
mod tests;
