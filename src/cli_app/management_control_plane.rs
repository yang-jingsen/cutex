//! Client for the authenticated local Human/Management control plane.
//!
//! This module deliberately has no Agent Bus identity dependency. In
//! particular it never reads `CUTEX_AGENT_ID` and never submits an Agent
//! occurrence as a Human TUI caller.

use cutex::agent_management::{CutexProjectWorkspace, ProjectId, ProjectPresentationSettings};
use cutex::management::control_plane::{
    HumanManagementOperatorActionRequest, HumanManagementOperatorReceipt,
    HumanManagementPresentationUpdateRequest, HumanManagementProjectCollection,
    HumanManagementTaskQueryRequest, HumanManagementTaskQueryResponse,
};
use cutex::management::remote::management_http_json;
use cutex::management::service::{
    management_base_url, management_root_credential, DEFAULT_MANAGEMENT_PORT,
};
use cutex::profiles::model::CodezConfig;

#[derive(Clone, Debug)]
pub(super) struct ManagementControlClient {
    base_url: String,
    root_bearer: String,
}

impl ManagementControlClient {
    pub(super) fn connect() -> anyhow::Result<Self> {
        let config = cutex::config::store::load_codez_config();
        Self::connect_with_config(&config)
    }

    fn connect_with_config(config: &CodezConfig) -> anyhow::Result<Self> {
        let root_bearer = management_root_credential(config, None)?.to_string();
        cutex::management::launch::ensure_management_api_running(config, DEFAULT_MANAGEMENT_PORT)?;
        Ok(Self {
            base_url: management_base_url(DEFAULT_MANAGEMENT_PORT),
            root_bearer,
        })
    }

    pub(super) fn projects(&self) -> anyhow::Result<HumanManagementProjectCollection> {
        self.request("GET", "/v2/agent-management/projects", None)
    }

    pub(super) fn project(&self, project_id: &ProjectId) -> anyhow::Result<CutexProjectWorkspace> {
        self.request(
            "GET",
            &format!("/v2/agent-management/projects/{project_id}"),
            None,
        )
    }

    pub(super) fn update_presentation(
        &self,
        request: &HumanManagementPresentationUpdateRequest,
    ) -> anyhow::Result<ProjectPresentationSettings> {
        self.request(
            "POST",
            "/v2/agent-management/project-presentation",
            Some(&serde_json::to_vec(request)?),
        )
    }

    pub(super) fn operator_action(
        &self,
        request: &HumanManagementOperatorActionRequest,
    ) -> anyhow::Result<HumanManagementOperatorReceipt> {
        self.request(
            "POST",
            "/v2/agent-management/operator-actions",
            Some(&serde_json::to_vec(request)?),
        )
    }

    pub(super) fn tasks(
        &self,
        request: &HumanManagementTaskQueryRequest,
    ) -> anyhow::Result<HumanManagementTaskQueryResponse> {
        self.request(
            "POST",
            "/v2/task-service/management-query",
            Some(&serde_json::to_vec(request)?),
        )
    }

    fn request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> anyhow::Result<T> {
        let value =
            management_http_json(&self.base_url, method, path, Some(&self.root_bearer), body)?;
        serde_json::from_value(value)
            .map_err(|error| anyhow::anyhow!("invalid Management control response: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cutex::profiles::model::ManagementApiToken;

    #[test]
    fn configured_client_uses_only_dedicated_management_root() {
        let config = CodezConfig {
            management_api_token: Some(ManagementApiToken::new("management-root")),
            agent_bus_token: Some("agent-bus-root".to_string()),
            ..Default::default()
        };
        // Construction of the authenticated context is pure and has no Agent
        // environment lookup. Avoid starting a server in this focused test by
        // asserting the credential resolver used by the constructor.
        assert_eq!(
            management_root_credential(&config, None).unwrap(),
            "management-root"
        );
        assert_ne!(
            management_root_credential(&config, None).unwrap(),
            config.agent_bus_token.as_deref().unwrap()
        );
    }
}
