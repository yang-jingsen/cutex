//! Narrow outbound MCP protocol adapter. Authority and receipts remain in Cutex.
use crate::role_revision::RuntimeAgentId;
use anyhow::{bail, Context};
use serde::Deserialize;
use serde_json::{json, Value};

#[path = "mcp_list.rs"]
pub(crate) mod list;
#[path = "mcp_management.rs"]
mod management;
#[path = "mcp_tasks.rs"]
mod tasks;

pub const THREAD_HEADER: &str = "x-cutex-mcp-thread-id";
pub const GENERATION_HEADER: &str = "x-cutex-mcp-generation";

#[derive(Clone, Debug)]
pub struct CallerFence {
    pub thread_id: String,
    pub generation: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryArgs {
    action_id: String,
    project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendArgs {
    to: String,
    message: String,
    external_message_id: String,
    delivery_mode: crate::agent_bus::delivery::AgentDeliveryMode,
}

pub fn tools() -> Value {
    let mut result = json!({"tools":[
        {"name":"query_managed","description":"Query managed Agents in an authorized Cutex Project. Caller comes from the current runtime, never arguments.","inputSchema":{"type":"object","properties":{"action_id":{"type":"string"},"project_id":{"type":"string"}},"required":["action_id"],"additionalProperties":false}},
        {"name":"send","description":"Enqueue to an exact durable Cutex Agent with a current registered endpoint; offline targets are rejected. Pending is not inbound delivery or A4. Exact payload replay deduplicates within the existing Bus dedupe window; changed payload may create a new message even with the same external_message_id.","inputSchema":{"type":"object","properties":{"to":{"type":"string"},"message":{"type":"string"},"external_message_id":{"type":"string"},"delivery_mode":{"type":"string","enum":["after_turn","soon","passive","interrupt"]}},"required":["to","message","external_message_id","delivery_mode"],"additionalProperties":false}}
    ]});
    result["tools"]
        .as_array_mut()
        .unwrap()
        .extend(tasks::tools());
    result["tools"]
        .as_array_mut()
        .unwrap()
        .extend([management::tool(), list::tool()]);
    result
}

struct TaskTransport<'a> {
    port: u16,
    token: &'a str,
    runtime: &'a RuntimeAgentId,
    fence: &'a CallerFence,
}
impl tasks::Transport for TaskTransport<'_> {
    fn post(&mut self, path: &str, body: &Value) -> anyhow::Result<Value> {
        // Same bounded retry policy as the native wrapper. Never return raw
        // transport diagnostics or provider HTTP error bodies to the model.
        for attempt in 0..2 {
            match crate::agent_bus::client::submit_mcp_control(
                self.port,
                self.token,
                self.runtime,
                self.fence,
                path,
                body,
            ) {
                Ok(value)
                    if value["http_status"]
                        .as_u64()
                        .is_some_and(|s| s >= 500 || s == 408) =>
                {
                    if attempt == 1 {
                        anyhow::bail!("response uncertain");
                    }
                }
                Ok(value) => return Ok(value),
                Err(_) if attempt == 0 => {}
                Err(_) => anyhow::bail!("response uncertain"),
            }
        }
        unreachable!()
    }
}

pub fn request(
    name: &str,
    args: Value,
    runtime: &RuntimeAgentId,
) -> anyhow::Result<(&'static str, Value)> {
    match name {
        "query_managed" => {
            let args: QueryArgs = serde_json::from_value(args)?;
            let value = json!({"schema":"cutex/agent-management/v1","action_id":args.action_id,"project_id":args.project_id,"operation":"query_managed"});
            let _: crate::agent_management::AgentManagementRequest =
                serde_json::from_value(value.clone())?;
            Ok(("/api/agent-management/v1/actions", value))
        }
        "send" => {
            let args: SendArgs = serde_json::from_value(args)?;
            let _: crate::role_revision::CutexSessionId =
                crate::role_revision::CutexSessionId::new(args.to.clone())
                    .map_err(|_| anyhow::anyhow!("invalid durable target"))?;
            if !crate::agent_bus::routing::is_full_durable_cutex_session_id(&args.to)
                || args.external_message_id.trim().is_empty()
                || args.message.trim().is_empty()
            {
                bail!(
                    "exact durable recipient, message and stable external_message_id are required"
                );
            }
            Ok((
                "/api/messages/send",
                json!({"to":args.to,"content":args.message,"external_message_id":args.external_message_id,"delivery_mode":args.delivery_mode,"kind":"message","from_agent_id":runtime.as_str(),"all_groups":false,"all_hosts":false}),
            ))
        }
        _ => bail!("unsupported MCP operation"),
    }
}

pub fn run() -> anyhow::Result<()> {
    use std::io::{BufRead, Read, Write};
    let runtime = RuntimeAgentId::new(std::env::var("CUTEX_AGENT_ID")?)
        .map_err(|_| anyhow::anyhow!("invalid runtime ID"))?;
    let generation: u64 = std::env::var("CUTEX_RUNTIME_GENERATION")?.parse()?;
    if generation == 0 {
        bail!("current runtime generation required");
    }
    let token = std::env::var("CUTEX_AGENT_BUS_TOKEN")?;
    let url = url::Url::parse(&std::env::var("CUTEX_AGENT_BUS_URL")?)?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.path() != "/"
        || url.query().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("MCP requires the configured local Cutex Bus endpoint");
    }
    let port = url.port().context("explicit local Bus port required")?;
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    loop {
        let mut bytes = Vec::new();
        let count = input.by_ref().take(262145).read_until(b'\n', &mut bytes)?;
        if count == 0 {
            break;
        }
        if count > 262144 || bytes.last() != Some(&b'\n') {
            bail!("MCP frame exceeds limit");
        }
        let message: Value = serde_json::from_slice(&bytes)?;
        let Some(id) = message.get("id") else {
            continue;
        };
        let result = match message["method"].as_str() {
            Some("initialize") => Ok(
                json!({"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"cutex-outbound-prototype","version":"1"}}),
            ),
            Some("ping") => Ok(json!({})),
            Some("tools/list") => Ok(tools()),
            Some("tools/call") => (|| -> anyhow::Result<Value> {
                let thread = message["params"]["_meta"]["threadId"]
                    .as_str()
                    .context("Core threadId missing")?;
                let thread = crate::session::identity::normalize_codex_session_id(thread)?;
                let name = message["params"]["name"]
                    .as_str()
                    .context("tool name missing")?;
                let args = message["params"]["arguments"].clone();
                let fence = CallerFence {
                    thread_id: thread,
                    generation,
                };
                let result = if matches!(
                    name,
                    tasks::WORKER | tasks::DIRECTOR | tasks::TERMINAL | tasks::READ
                ) {
                    tasks::invoke(
                        name,
                        args,
                        &mut TaskTransport {
                            port,
                            token: &token,
                            runtime: &runtime,
                            fence: &fence,
                        },
                    )
                } else if name == "cutex_agent_management" {
                    management::invoke(args, |body| {
                        crate::agent_bus::client::submit_mcp_control(
                            port,
                            &token,
                            &runtime,
                            &fence,
                            "/api/agent-management/v1/actions",
                            body,
                        )
                    })
                } else if name == "cutex_agent_list" {
                    if list::validate_args(args).is_err() {
                        json!({"ok":false,"code":"unsupported_scope_or_arguments","detail":"Only optional all_groups=false and all_hosts=false are supported; caller identity is not an argument."})
                    } else {
                        let value = crate::agent_bus::client::submit_mcp_control(
                            port,
                            &token,
                            &runtime,
                            &fence,
                            "/api/agents?all_groups=false&all_hosts=false",
                            &json!({}),
                        )?;
                        if value["ok"] == true
                            && value["scope"] == "local_group_visible"
                            && value["agents"].is_array()
                        {
                            value
                        } else {
                            json!({"ok":false,"code":"provider_observation_unavailable","detail":"Authenticated local Agent list unavailable; no empty or successful observation inferred."})
                        }
                    }
                } else {
                    let (path, body) = request(name, args, &runtime)?;
                    crate::agent_bus::client::submit_mcp_control(
                        port, &token, &runtime, &fence, path, &body,
                    )?
                };
                let failed = result.get("http_status").is_some()
                    || result["outcome"]["status"] == "no_write"
                    || result["outcome"]["status"] == "owner_action_required"
                    || matches!(
                        result["status"].as_str(),
                        Some("no_write" | "conflict" | "response_uncertain")
                    )
                    || result["ok"] == false;
                Ok(
                    json!({"isError":failed,"content":[{"type":"text","text":serde_json::to_string(&result)?}]}),
                )
            })(),
            _ => Err(anyhow::anyhow!("unsupported MCP method")),
        };
        // Never echo transport errors/credentials into model-visible output.
        let response = match result {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err(_) => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"Cutex rejected operation, caller metadata, or authenticated service request"}})
            }
        };
        serde_json::to_writer(&mut output, &response)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn runtime() -> RuntimeAgentId {
        RuntimeAgentId::new("runtime-mcp-test").unwrap()
    }

    #[test]
    fn mcp_schema_and_parser_do_not_accept_caller_authority() {
        for tool in tools()["tools"].as_array().unwrap() {
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
            for key in [
                "caller",
                "caller_cutex_session_id",
                "role",
                "runtime_generation",
                "token",
                "threadId",
            ] {
                assert!(tool["inputSchema"]["properties"].get(key).is_none());
                let mut args = if tool["name"] == "query_managed" {
                    json!({"action_id":"q"})
                } else {
                    json!({"to":"cutex.00000000-0000-0000-0000-000000000001","message":"hello","external_message_id":"m","delivery_mode":"passive"})
                };
                args[key] = json!("spoof");
                assert!(request(tool["name"].as_str().unwrap(), args, &runtime()).is_err());
            }
        }
    }

    #[test]
    fn mcp_query_is_typed_and_never_routes_to_human_management() {
        let (path, value) = request(
            "query_managed",
            json!({"action_id":"q","project_id":"project-a"}),
            &runtime(),
        )
        .unwrap();
        assert_eq!(path, "/api/agent-management/v1/actions");
        let parsed: crate::agent_management::AgentManagementRequest =
            serde_json::from_value(value).unwrap();
        assert!(matches!(
            parsed.operation,
            crate::agent_management::AgentOperation::QueryManaged
        ));
        assert!(request("close", json!({}), &runtime()).is_err());
    }

    #[test]
    fn mcp_send_preserves_modes_and_exact_action_without_sender_override() {
        for mode in ["after_turn", "soon", "passive", "interrupt"] {
            let (path, value) = request("send", json!({"to":"cutex.00000000-0000-0000-0000-000000000001","message":"hello","external_message_id":"same-action","delivery_mode":mode}), &runtime()).unwrap();
            assert_eq!(path, "/api/messages/send");
            let parsed: crate::agent_bus::model::AgentBusSendRequest =
                serde_json::from_value(value).unwrap();
            assert_eq!(parsed.resolved_delivery_mode().event_label(), mode);
            assert_eq!(parsed.external_message_id.as_deref(), Some("same-action"));
            assert_eq!(parsed.from_agent_id.as_deref(), Some("runtime-mcp-test"));
            assert!(parsed.from.is_none() && parsed.from_session_id.is_none());
        }
        assert!(request("send", json!({"to":"guessed-name","message":"hello","external_message_id":"m","delivery_mode":"passive"}), &runtime()).is_err());
    }
}
