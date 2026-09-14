# Task MCP 参数说明与校验（2026-09-14）

报告：/tmp/cesc2_task_service_mcp_accept_result_schema_20260914_ZH.md。当前 light 的 facade 复用了早期 native 语义 handler：多种 operation 共享平铺参数对象，额外字段被拒绝却只有 invalid_semantic_payload。accept_result 的 summary/project_id/task_id/task_revision 都不合法，报告判断正确。

修复提交 7532f2a：保留工具名、flat inputSchema 和严格语义/权限校验。统一操作字段表生成 Director/Worker 的逐操作 required/optional/forbidden 帮助及每个字段的使用范围；Director 文档包含最小 acceptance 示例。MCP 预检对跨操作已知字段返回 no_write / field_not_allowed、operation、fields、allowed_fields、required_fields，不发送 HTTP 请求、不回显参数值。兼容旧调用者的 contract_sha256 仍仅在创建操作接受，不新增模型必须计算的字段。

accept_result 的 decision_reference 可选。request_changes 的底层 provider 已要求 bounded、非空的 decision_reference；本次同步工具说明与缺失字段诊断。不能从旧 facade 的 Option 类型推断后端完全不要求它。

验证：22 项 MCP 单元测试通过，覆盖最小 acceptance（带/不带 reference）、四种 Director 决策的非法字段、无网络写入与参数值不泄漏、Worker submit 字段限制及既有协议/权限测试。release facade 实际 stdio tools/list 可见新说明；对隔离身份/不可用测试端口执行非法 acceptance，仍在本地返回明确 no_write，验证未走 transport。没有在已关闭的真实 assignment 上执行验收/重提测试。

部署：release-runtime-r24/cutex-mcp 为 optimized release facade；native-r5 只更新 facade 引用，继续使用 native-r4 的 CLI/app-server 和同一个 Job launcher。不需要更新或重启 Job daemon，也不改变 Task 数据。在线适用 native agent 重启刷新 MCP 工具定义，其他 agent 下次启动使用新定义。Cutex CLI/Bus/Management 继续运行 release-runtime-r23。

独立 resubmit 调查：找到 Worker 原始 rollout，三次相同 payload 的 submit 在 2026-09-13 17:53:15 / 17:53:33 / 17:53:51 UTC 发起，即悉尼 2026-09-14 03:53；返回分别约 10.6 / 10.5 / 10.5 秒，均 response_uncertain。它们早于 10:02 验收关闭，不能归因于“关闭后重提”。当前 receipts 无该 action ID，assignment 为 closed。该时长符合 TaskTransport 两次 5 秒读取超时；没有原始底层错误或服务端逐请求时序，不能确定是 prepare、action 还是连接/服务阻塞，也不宣称本次说明修复解决了它。仍保持“不确定响应重用完全相同 action_id/payload”的规则。
