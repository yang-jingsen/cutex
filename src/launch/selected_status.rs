//! Bounded static display projection, separate from authentication and server arguments.
use super::stock::VerifiedFile;
use anyhow::{ensure, Context};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub catalog: VerifiedFile,
    pub path: PathBuf,
    pub payload: Payload,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Payload {
    pub version: u8,
    pub items: Vec<Item>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub id: String,
    pub text: String,
    pub style: Style,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Style {
    pub fg: Option<String>,
    pub bg: Option<String>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underlined: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    items: Vec<Entry>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    id: String,
    title: String,
    description: Option<String>,
    source: Source,
    #[serde(default)]
    render: Render,
    #[serde(default)]
    style: Style,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Source {
    Static { value: String },
    LaunchProfile {},
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Render {
    Value {},
}
impl Default for Render {
    fn default() -> Self {
        Self::Value {}
    }
}

/// Canonical display IDs; old spellings remain accepted in saved configuration.
pub fn canonical_id(id: &str) -> &str {
    match id {
        "custom:bon-voyage" | "cutex_bon_voyage" => "cutex_welcome",
        "custom:profile" => "cutex_profile",
        "notification" | "custom:notification" => "cutex_notification",
        _ => id,
    }
}

pub fn is_static_id(id: &str) -> bool {
    matches!(canonical_id(id), "cutex_welcome" | "cutex_profile")
}

fn requires_catalog(id: &str) -> bool {
    is_static_id(id) || (id.starts_with("custom:") && canonical_id(id) != "cutex_notification")
}

fn digest(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}
fn resolve(bytes: &[u8], order: &[String], label: &str) -> anyhow::Result<Payload> {
    ensure!(bytes.len() <= 8192, "status catalog exceeds reviewed bound");
    let catalog: Catalog = serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("unsupported status catalog/source/render"))?;
    ensure!(catalog.items.len() <= 2, "unsupported status catalog items");
    let mut seen = std::collections::BTreeSet::new();
    for entry in &catalog.items {
        ensure!(
            is_static_id(&entry.id)
                && seen.insert(canonical_id(&entry.id)),
            "unknown/duplicate status item"
        );
        let _ = (&entry.title, &entry.description, &entry.render);
    }
    let mut items = Vec::new();
    for id in order.iter().filter(|id| requires_catalog(id)) {
        ensure!(
            !items.iter().any(|i: &Item| canonical_id(&i.id) == canonical_id(id)),
            "duplicate selected status item"
        );
        let entry = catalog
            .items
            .iter()
            .find(|e| canonical_id(&e.id) == canonical_id(id))
            .context("selected status item missing from catalog")?;
        let text = match &entry.source {
            Source::Static { value } => value.clone(),
            Source::LaunchProfile {} => label.into(),
        };
        items.push(Item {
            id: id.clone(),
            text,
            style: entry.style.clone(),
        });
    }
    let payload = Payload { version: 1, items };
    payload.validate()?;
    Ok(payload)
}
impl Payload {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            self.version == 1 && self.items.len() <= 2,
            "unsupported static status version/size"
        );
        let mut seen = std::collections::BTreeSet::new();
        for i in &self.items {
            ensure!(
                is_static_id(&i.id)
                    && seen.insert(canonical_id(&i.id)),
                "invalid static status ID"
            );
            ensure!(!i.text.trim().is_empty() && i.text.len() <= 256 && !i.text.chars().any(|c| c.is_control() || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')), "invalid static status text");
            for color in [&i.style.fg, &i.style.bg].into_iter().flatten() {
                ensure!(
                    color.len() == 7
                        && color.starts_with('#')
                        && color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit),
                    "invalid status color"
                );
            }
        }
        ensure!(
            serde_json::to_vec(self)?.len() <= 8192,
            "static status file too large"
        );
        Ok(())
    }
}
impl Status {
    pub fn review(
        catalog_path: &Path,
        order: &[String],
        label: &str,
    ) -> anyhow::Result<Option<Self>> {
        if !order.iter().any(|s| requires_catalog(s)) {
            return Ok(None);
        }
        super::selected_profile::validate_asset(catalog_path)?;
        let bytes = super::selected_profile::bounded_asset(catalog_path)?;
        let payload = resolve(&bytes, order, label)?;
        let path = catalog_path
            .parent()
            .context("status catalog parent missing")?
            .join(format!(
                "reviewed-status-{}.json",
                digest(&serde_json::to_vec(&payload)?)
            ));
        Ok(Some(Self {
            catalog: VerifiedFile {
                path: catalog_path.into(),
                sha256: crate::role_revision::Sha256::new(digest(&bytes))
                    .map_err(anyhow::Error::msg)?,
            },
            path,
            payload,
        }))
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        self.payload.validate()?;
        super::selected_profile::validate_asset(&self.catalog.path)?;
        let bytes = super::selected_profile::bounded_asset(&self.catalog.path)?;
        ensure!(
            digest(&bytes) == self.catalog.sha256.as_str(),
            "status catalog changed since review"
        );
        self.validate_frozen()
    }
    /// Ready occurrences use their frozen display payload, not the mutable catalog.
    pub fn validate_frozen(&self) -> anyhow::Result<()> {
        self.payload.validate()?;
        let expected = self
            .catalog
            .path
            .parent()
            .context("status parent missing")?
            .join(format!(
                "reviewed-status-{}.json",
                digest(&serde_json::to_vec(&self.payload)?)
            ));
        ensure!(self.path == expected, "reviewed status path changed");
        Ok(())
    }
    pub fn validate_projection(&self, order: &[String], label: &str) -> anyhow::Result<()> {
        self.validate()?;
        let bytes = super::selected_profile::bounded_asset(&self.catalog.path)?;
        ensure!(
            resolve(&bytes, order, label)? == self.payload,
            "status text/style/selection differs from reviewed profile"
        );
        Ok(())
    }
    /// Create only a new content-addressed nonsecret display file. Existing
    /// content is verified, never overwritten or repaired during attachment.
    pub fn materialize(&self) -> anyhow::Result<&Path> {
        self.validate()?;
        self.materialize_frozen()
    }
    pub fn materialize_frozen(&self) -> anyhow::Result<&Path> {
        self.validate_frozen()?;
        let bytes = serde_json::to_vec(&self.payload)?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        match options.open(&self.path) {
            Ok(mut file) => {
                use std::io::Write;
                file.write_all(&bytes)?;
                file.sync_all()?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        super::selected_profile::validate_asset(&self.path)?;
        ensure!(
            super::selected_profile::bounded_asset(&self.path)? == bytes,
            "reviewed status file changed; no overwrite"
        );
        self.validate_frozen()?;
        Ok(&self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn catalog() -> serde_json::Value {
        serde_json::json!({"items":[
            {"id":"custom:bon-voyage","title":"Bon voyage","source":{"kind":"static","value":"Bon voyage !"},"style":{"fg":"#F6A3C8","bold":true}},
            {"id":"custom:profile","title":"Profile","source":{"kind":"launch_profile"},"style":{"fg":"#FFFFFF","bold":true}}
        ]})
    }
    #[test]
    fn frozen_payload_survives_catalog_removal_but_rejects_changed_materialization() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("frozen-status-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("catalog.json");
        std::fs::write(&path, serde_json::to_vec(&catalog()).unwrap()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let status = Status::review(&path, &["cutex_profile".into()], "profile").unwrap().unwrap();
        status.materialize().unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(status.validate().is_err());
        status.materialize_frozen().unwrap();
        std::fs::write(&status.path, b"changed").unwrap();
        assert!(status.materialize_frozen().is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn canonical_order_accepts_legacy_catalog_and_keeps_dynamic_item_separate() {
        let bytes = serde_json::to_vec(&catalog()).unwrap();
        let payload = resolve(&bytes, &["cutex_welcome".into(), "cutex_profile".into(), "cutex_notification".into()], "chosen").unwrap();
        assert_eq!(payload.items.len(), 2);
        assert_eq!((&payload.items[0].id, &payload.items[0].text), (&"cutex_welcome".to_string(), &"Bon voyage !".to_string()));
        assert_eq!(payload.items[1].text, "chosen");
        assert!(resolve(&bytes, &["cutex_profile".into(), "custom:profile".into()], "chosen").is_err());
    }
    #[test]
    fn selected_label_order_and_style_are_not_account_inferred() {
        let bytes = serde_json::to_vec(&catalog()).unwrap();
        let order = vec![
            "custom:profile".into(),
            "model-with-reasoning".into(),
            "custom:bon-voyage".into(),
        ];
        let a = resolve(&bytes, &order, "aemeath").unwrap();
        let b = resolve(&bytes, &order, "octobre").unwrap();
        assert_eq!(a.items[0].text, "aemeath");
        assert_eq!(b.items[0].text, "octobre");
        assert_eq!(a.items[1].text, "Bon voyage !");
        assert_eq!(a.items[1].style.fg.as_deref(), Some("#F6A3C8"));
        assert!(a.items[1].style.bold);
        assert!(resolve(&bytes, &[], "ignored").unwrap().items.is_empty());
    }
    #[test]
    fn dynamic_unknown_duplicate_and_control_sources_reject() {
        let order = vec!["custom:profile".into()];
        for source in [
            serde_json::json!({"kind":"env","key":"SECRET"}),
            serde_json::json!({"kind":"launch_profile","unknown":true}),
        ] {
            let mut c = catalog();
            c["items"][1]["source"] = source;
            assert!(resolve(&serde_json::to_vec(&c).unwrap(), &order, "label").is_err());
        }
        assert!(resolve(
            &serde_json::to_vec(&catalog()).unwrap(),
            &["custom:profile".into(), "custom:profile".into()],
            "label"
        )
        .is_err());
        assert!(resolve(
            &serde_json::to_vec(&catalog()).unwrap(),
            &order,
            "bad\x1b[31m"
        )
        .is_err());
    }
    #[test]
    fn reviewed_file_reuse_tamper_and_catalog_change_fail_closed() {
        let dir = std::env::temp_dir().join(format!("selected-status-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let source = dir.join("catalog.json");
        std::fs::write(&source, serde_json::to_vec(&catalog()).unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let status = Status::review(&source, &["custom:profile".into()], "selected")
            .unwrap()
            .unwrap();
        let first = status.materialize().unwrap().to_owned();
        assert_eq!(status.materialize().unwrap(), first);
        std::fs::write(&first, b"{}").unwrap();
        assert!(status.materialize().is_err());
        std::fs::write(&source, b"{}").unwrap();
        assert!(status.validate().is_err());
        let mut unknown = status.payload.clone();
        unknown.version = 2;
        assert!(unknown.validate().is_err());
    }
}
