//! Client for the authenticated local Human/Management control plane.
//!
//! This module deliberately has no Agent Bus identity dependency. In
//! particular it never reads `CUTEX_AGENT_ID` and never submits an Agent
//! occurrence as a Human TUI caller.

use cutex::agent_management::{CutexProjectWorkspace, ProjectId, ProjectPresentationSettings};
use cutex::management::control_plane::{
    HumanManagementOperatorActionRequest, HumanManagementOperatorReceipt,
    HumanManagementPresentationUpdateRequest, HumanManagementProjectCollection,
    HumanManagementProjectMutationReceipt, HumanManagementProjectMutationRequest,
    HumanManagementTaskQueryRequest, HumanManagementTaskQueryResponse,
};
use cutex::management::remote::management_http_json_with_timeout;
use cutex::management::service::{
    management_base_url, management_root_credential, DEFAULT_MANAGEMENT_PORT,
};
use cutex::profiles::model::CodezConfig;
use std::time::Duration;

#[derive(Clone, Debug)]
pub(super) struct ManagementControlClient {
    base_url: String,
    root_bearer: String,
}

impl ManagementControlClient {
    pub(super) fn review_stock_runtime(
        &self,
        cutex_session_id: cutex::role_revision::CutexSessionId,
        restart: bool,
    ) -> anyhow::Result<cutex::agent_management::StockRuntimeReview> {
        self.request_with_timeout(
            "POST",
            "/v2/agent-management/explicit-launch",
            Some(&serde_json::to_vec(
                &cutex::agent_management::ExplicitLaunchRequest::ReviewRuntime {
                    cutex_session_id,
                    restart,
                    receiver_canonical_byte_limit: Default::default(),
                    job_mcp: None,
                },
            )?),
            Duration::from_secs(120),
        )
    }

    pub(super) fn run_stock_runtime(
        &self,
        action_id: cutex::agent_management::AgentActionId,
        review: cutex::agent_management::StockRuntimeReview,
    ) -> anyhow::Result<cutex::agent_management::StockRuntimeReceipt> {
        self.request_with_timeout(
            "POST",
            "/v2/agent-management/explicit-launch",
            Some(&serde_json::to_vec(
                &cutex::agent_management::ExplicitLaunchRequest::Run { action_id, review },
            )?),
            Duration::from_secs(120),
        )
    }

    pub(super) fn adopt_saved_native(
        &self,
        request: &cutex::agent_management::HumanAdoptRequest,
    ) -> anyhow::Result<cutex::agent_management::HumanAdoptResult> {
        self.request(
            "POST",
            "/v2/agent-management/adopt-saved-native",
            Some(&serde_json::to_vec(request)?),
        )
    }
    pub(super) fn review_agent_archive(
        &self,
        request: &cutex::agent_management::AgentArchiveReviewRequest,
    ) -> anyhow::Result<cutex::agent_management::AgentArchiveReview> {
        self.request(
            "POST",
            "/v2/agent-management/archive-review",
            Some(&serde_json::to_vec(request)?),
        )
    }

    pub(super) fn execute_agent_archive(
        &self,
        request: &cutex::agent_management::AgentArchiveRequest,
    ) -> anyhow::Result<cutex::agent_management::AgentArchiveReceipt> {
        self.request(
            "POST",
            "/v2/agent-management/archive-actions",
            Some(&serde_json::to_vec(request)?),
        )
    }
    #[cfg(test)]
    pub(super) fn test_endpoint(base_url: String, root_bearer: String) -> Self {
        Self {
            base_url,
            root_bearer,
        }
    }
    pub(super) fn durable_candidates(
        &self,
    ) -> anyhow::Result<Vec<cutex::agent_management::DurableAgentCandidate>> {
        self.request("GET", "/v2/agent-management/durable-candidates", None)
    }

    pub(super) fn import_durable_agent(
        &self,
        request: &cutex::agent_management::DurableImportRequest,
    ) -> anyhow::Result<cutex::agent_management::DurableImportReceipt> {
        self.request(
            "POST",
            "/v2/agent-management/durable-import",
            Some(&serde_json::to_vec(request)?),
        )
    }
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

    pub(super) fn project_mutation(
        &self,
        request: &HumanManagementProjectMutationRequest,
    ) -> anyhow::Result<HumanManagementProjectMutationReceipt> {
        self.request(
            "POST",
            "/v2/agent-management/project-mutations",
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
        self.request_with_timeout(method, path, body, Duration::from_secs(5))
    }

    fn request_with_timeout<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
        timeout: Duration,
    ) -> anyhow::Result<T> {
        let value = management_http_json_with_timeout(
            &self.base_url,
            method,
            path,
            Some(&self.root_bearer),
            body,
            timeout,
        )?;
        serde_json::from_value(value)
            .map_err(|error| anyhow::anyhow!("invalid Management control response: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cutex::profiles::model::ManagementApiToken;

    #[test]
    fn runtime_request_waits_for_response_beyond_old_five_second_limit() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let client = ManagementControlClient::test_endpoint(
            format!("http://{}", listener.local_addr().unwrap()),
            "test-root".into(),
        );
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            socket.read(&mut request).unwrap();
            std::thread::sleep(Duration::from_millis(5500));
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 17\r\nConnection: close\r\n\r\n{\"stage\":\"ready\"}").unwrap();
        });
        let result: serde_json::Value = client
            .request_with_timeout(
                "POST",
                "/v2/agent-management/explicit-launch",
                None,
                Duration::from_secs(120),
            )
            .unwrap();
        assert_eq!(result["stage"], "ready");
        server.join().unwrap();
    }

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
