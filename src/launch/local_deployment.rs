//! Locally selected default runtime for saved-thread adoption.
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context};
use serde::{Deserialize, Serialize};

use crate::agent_management::{file_sha256, ExplicitLaunchContract};
use crate::session::model::{CutexSessionRecord, CutexSessionRuntimeBackend, CutexSessionStore};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalDeployment {
    pub native_home: PathBuf,
    pub bundle_manifest: PathBuf,
    #[serde(default)]
    pub job_mcp: Option<super::job_mcp::JobMcpDescriptor>,
}

impl LocalDeployment {
    /// Select an installed artifact template; existing agent bindings are not rewritten.
    pub fn install(&self) -> anyhow::Result<()> {
        ensure!(
            cfg!(any(target_os = "linux", windows)),
            "local runtime deployment is unsupported on this platform"
        );
        ensure!(self.native_home.is_dir(), "saved native home missing");
        let bundle: super::stock::StockBundle =
            serde_json::from_slice(&std::fs::read(&self.bundle_manifest)?)?;
        super::stock::StockBundle::load_references(
            bundle
                .shared_config
                .path
                .parent()
                .context("bundle config parent missing")?,
            &self.bundle_manifest,
            &file_sha256(&self.bundle_manifest)?,
        )?;
        ensure!(
            bundle.soon_ingress(),
            "local deployment requires compatible light runtime"
        );
        crate::config::atomic::write_private_pretty_json_atomic(
            &crate::config::paths::runtime_dir()?.join("light/deployment.json"),
            self,
            "local runtime selection",
        )
    }

    pub fn source_home() -> anyhow::Result<PathBuf> {
        match Self::selected()? {
            Some(deployment) => Ok(deployment.native_home),
            None => crate::config::paths::host_codex_home_dir(),
        }
    }

    pub fn selected() -> anyhow::Result<Option<Self>> {
        let path = crate::config::paths::runtime_dir()?.join("light/deployment.json");
        Self::read(&path)
    }

    pub fn read(path: &Path) -> anyhow::Result<Option<Self>> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(
                serde_json::from_slice(&bytes).context("invalid local runtime deployment")?,
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Bind a stopped saved identity to an isolated copy of its history.
    /// No runtime is started; validate before changing the caller's record.
    pub fn adopt(
        &self,
        record: &mut CutexSessionRecord,
        sessions: &CutexSessionStore,
    ) -> anyhow::Result<()> {
        ensure!(
            record.runtime_pid.is_none()
                && record.current_runtime_agent_id.is_none()
                && record.app_server_runtime.is_none()
                && record.app_server_launch_claim_id.is_none(),
            "saved thread has a runtime owner; stop it before adoption"
        );
        let native_id = record
            .codex_session_id
            .clone()
            .context("saved native identity required")?;
        ensure!(
            uuid::Uuid::parse_str(&native_id)?.to_string() == native_id,
            "exact native UUID required"
        );
        let source_home = self.native_home.canonicalize()?;
        let source_history = super::native_history::current(&source_home, &native_id)?;
        let sources = super::native_history::recovery_sources(&source_home, &source_history)?;
        ensure!(
            !sources.iter().map(|path| has_history_writer(path)).collect::<anyhow::Result<Vec<_>>>()?.into_iter().any(|open| open),
            "saved thread is open for writing; stop its native session before Adopt"
        );
        let destination = crate::config::paths::runtime_dir()?
            .join("light/agents")
            .join(&native_id);
        let mut bundle: super::stock::StockBundle =
            serde_json::from_slice(&std::fs::read(&self.bundle_manifest)?)?;
        ensure!(
            bundle.soon_ingress(),
            "local default runtime requires supported light protocol"
        );
        for file in [
            &bundle.executable,
            &bundle.code_mode_host,
            &bundle.facade,
            &bundle.schema,
        ] {
            file.validate()?;
        }
        bundle
            .cli
            .as_ref()
            .context("local CLI missing")?
            .validate()?;
        let mut config: toml::Value =
            toml::from_str(&std::fs::read_to_string(source_home.join("config.toml"))?)?;
        config
            .as_table_mut()
            .context("native config table required")?
            .insert("cutex_projection_version".into(), toml::Value::Integer(2));
        let config = toml::to_string(&config)?;
        super::stock::validate_shared_config(&config)?;
        let mut candidate = record.clone();
        candidate.runtime_backend = CutexSessionRuntimeBackend::Host;
        if candidate.sandbox_mode.is_none() {
            candidate.sandbox_mode = Some("danger-full-access".into());
        }
        if candidate.permission_defaults.is_none() {
            candidate.permission_defaults = candidate.sandbox_mode.clone();
        }
        if candidate.approval_policy.is_none() {
            candidate.approval_policy = Some("never".into());
        }
        super::stock::migration_configuration(&candidate)?.validate_auth_home(&source_home)?;
        let history_hash = file_sha256(&source_history)?;
        let mut provenance = serde_json::json!({"native_id": native_id, "source_history": source_history, "sha256": history_hash});
        if sources.len() > 1 {
            provenance["dependencies"] = serde_json::Value::Array(sources.iter().skip(1)
                .map(|path| Ok(serde_json::json!({"path":path,"sha256":file_sha256(path)?})))
                .collect::<anyhow::Result<Vec<_>>>()?);
        }
        let materialization = if destination.exists() {
            destination.clone()
        } else {
            destination.with_file_name(format!(".adopt-{}-{}", native_id, uuid::Uuid::new_v4()))
        };
        if destination.exists() {
            ensure!(
                serde_json::from_slice::<serde_json::Value>(&std::fs::read(
                    destination.join("adoption.json")
                )?)? == provenance,
                "existing adoption home has different source history; use human recovery"
            );
        } else {
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(materialization.join("sessions"))?;
            copy_adoption_history(&source_home, &source_history, &materialization)?;
            ensure!(file_sha256(&source_history)? == history_hash,
                "history changed during adoption; source retained");
            crate::config::atomic::write_private_pretty_json_atomic(
                &materialization.join("adoption.json"),
                &provenance,
                "adoption source",
            )?;
            for asset in ["skills", "memories", "plugins"] {
                let source = source_home.join(asset);
                if source.exists() {
                    crate::platform::shared_assets::link_directory(&source, &materialization.join(asset))?;
                }
            }
        }
        bundle.launch_config_sha256 = None; // User configuration is not an immutable artifact.
        std::fs::write(materialization.join("config.toml"), config)?;
        bundle.version = 4;
        bundle.shared_config = super::stock::VerifiedFile {
            path: destination.join("config.toml"),
            sha256: file_sha256(&materialization.join("config.toml"))?,
        };
        crate::config::atomic::write_private_pretty_json_atomic(
            &materialization.join("bundle.json"),
            &bundle,
            "local runtime bundle",
        )?;
        if materialization != destination {
            std::fs::rename(&materialization, &destination)?;
        }
        let contract = ExplicitLaunchContract {
            version: 4,
            migration_action_id: None,
            native_id,
            native_home: destination.canonicalize()?,
            bundle_manifest: destination.join("bundle.json").canonicalize()?,
            bundle_sha256: file_sha256(&destination.join("bundle.json"))?,
        };
        super::stock::StockBundle::load(&contract)?;
        candidate.explicit_launch = Some(contract.clone());
        super::stock::validate_native(&candidate, sessions, &contract)?;
        *record = candidate;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adoption_preserves_reverted_history_chain_without_resuming_ancestor() {
        let root = std::env::temp_dir().join(format!("adopt-chain-{}", uuid::Uuid::new_v4()));
        let home = root.join("source");
        let target = root.join("target");
        std::fs::create_dir_all(home.join("sessions")).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let revision = uuid::Uuid::new_v4();
        let old = home.join("sessions").join(format!("rollout-2026-09-13T00-00-00-{id}.jsonl"));
        let current = home.join("sessions").join(format!("rollout-2026-09-15T00-00-00-{id}_{revision}.jsonl"));
        for (path, base) in [(&old, serde_json::Value::Null), (&current, serde_json::json!({"thread_id":id,"end_ordinal_exclusive":1,"end_byte_offset":1}))] {
            std::fs::write(path, serde_json::json!({"type":"session_meta","payload":{"id":id,"history_base":base}}).to_string()).unwrap();
        }
        copy_adoption_history(&home, &current, &target).unwrap();
        let selected = super::super::native_history::current(&target, &id.to_string()).unwrap();
        assert_eq!(selected, target.join("sessions").join(current.file_name().unwrap()));
        let chain = super::super::native_history::recovery_sources(&target, &selected).unwrap();
        assert_eq!(chain.len(), 2);
        assert_eq!(std::fs::read(&chain[0]).unwrap(), std::fs::read(&current).unwrap());
        assert_eq!(std::fs::read(&chain[1]).unwrap(), std::fs::read(&old).unwrap());
        std::fs::remove_file(&old).unwrap();
        assert!(copy_adoption_history(&home, &current, &root.join("missing")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(any(target_os = "linux", windows))]
    #[test]
    fn open_history_writer_blocks_adoption_until_closed() {
        let path =
            std::env::temp_dir().join(format!("cutex-history-writer-{}", uuid::Uuid::new_v4()));
        let file = std::fs::File::create(&path).unwrap();
        assert!(has_history_writer(&path).unwrap());
        drop(file);
        assert!(!has_history_writer(&path).unwrap());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn absent_deployment_is_optional_but_invalid_selection_is_visible() {
        let root = std::env::temp_dir().join(format!("cutex-deployment-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("deployment.json");
        assert!(LocalDeployment::read(&path).unwrap().is_none());
        std::fs::write(&path, b"invalid").unwrap();
        assert!(LocalDeployment::read(&path).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}

/// Keep only the selected rollout active. Immutable ancestors remain available
/// to native rollout-ID lookup in archived_sessions, never as resume candidates.
fn copy_adoption_history(home: &Path, current: &Path, destination: &Path) -> anyhow::Result<()> {
    let sources = super::native_history::recovery_sources(home, current)?;
    let hashes = sources.iter().map(|p| file_sha256(p)).collect::<anyhow::Result<Vec<_>>>()?;
    for (index, (source, hash)) in sources.iter().zip(&hashes).enumerate() {
        ensure!(!has_history_writer(source)?, "saved history is open for writing; stop its native session before Adopt");
        let directory = destination.join(if index == 0 { "sessions" } else { "archived_sessions" });
        std::fs::create_dir_all(&directory)?;
        let target = directory.join(source.file_name().context("history filename required")?);
        std::fs::copy(source, &target)?;
        ensure!(file_sha256(&target)? == *hash, "history copy mismatch");
    }
    for (source, hash) in sources.iter().zip(hashes) {
        ensure!(file_sha256(source)? == hash, "history changed during adoption; source retained");
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn has_history_writer(history: &Path) -> anyhow::Result<bool> {
    for process in std::fs::read_dir("/proc")? {
        let process = process?;
        if process
            .file_name()
            .to_string_lossy()
            .parse::<u32>()
            .is_err()
        {
            continue;
        }
        let Ok(descriptors) = std::fs::read_dir(process.path().join("fd")) else {
            continue;
        };
        for descriptor in descriptors.flatten() {
            if std::fs::read_link(descriptor.path()).ok().as_deref() != Some(history) {
                continue;
            }
            let info = match std::fs::read_to_string(
                process.path().join("fdinfo").join(descriptor.file_name()),
            ) {
                Ok(info) => info,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            if let Some(flags) = info.lines().find_map(|line| line.strip_prefix("flags:")) {
                if u32::from_str_radix(flags.trim(), 8)? & libc::O_ACCMODE as u32
                    != libc::O_RDONLY as u32
                {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}
#[cfg(windows)]
fn has_history_writer(history: &Path) -> anyhow::Result<bool> {
    use std::os::windows::fs::OpenOptionsExt;
    // Denying write sharing conflicts with an already open rollout writer.
    match std::fs::OpenOptions::new().read(true).share_mode(1).open(history) {
        Ok(_) => Ok(false),
        Err(error) if error.raw_os_error() == Some(32) => Ok(true),
        Err(error) => Err(error.into()),
    }
}
#[cfg(not(any(target_os = "linux", windows)))]
fn has_history_writer(_: &Path) -> anyhow::Result<bool> {
    anyhow::bail!("local adoption requires Linux")
}
