//! Read-only defaults used only when creating a new human session or agent.
use anyhow::Context;
use crate::profiles::model::{AccountsStore, NewSessionDefaults};

pub fn inherited(profile: &str) -> anyhow::Result<NewSessionDefaults> {
    let accounts: AccountsStore = serde_json::from_slice(&std::fs::read(super::paths::accounts_path()?)?)?;
    let account = accounts.accounts.iter().find(|a| a.name == profile).context("Default profile unavailable")?;
    let files = crate::profiles::materialize::materialized_account_files(account)?;
    let profile: toml::Value = toml::from_str(&std::fs::read_to_string(files.config_path)?)?;
    let deployment = crate::launch::local_deployment::LocalDeployment::selected()?.context("No local runtime installed")?;
    let shared: toml::Value = toml::from_str(&std::fs::read_to_string(deployment.native_home.join("config.toml"))?)?;
    Ok(from_tables(&profile, &shared))
}

fn from_tables(profile: &toml::Value, shared: &toml::Value) -> NewSessionDefaults {
    let value = |key| profile.get(key).or_else(|| shared.get(key)).and_then(toml::Value::as_str).map(str::to_owned);
    NewSessionDefaults { model: value("model"), reasoning: value("model_reasoning_effort") }
}

#[cfg(test)]
mod tests {
    #[test]
    fn profile_values_win_and_missing_effort_inherits_shared() {
        let profile = toml::from_str("model = 'glm'").unwrap();
        let shared = toml::from_str("model = 'gpt'\nmodel_reasoning_effort = 'high'").unwrap();
        let actual = super::from_tables(&profile, &shared);
        assert_eq!(actual.model.as_deref(), Some("glm"));
        assert_eq!(actual.reasoning.as_deref(), Some("high"));
    }
}
