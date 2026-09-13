# Typed 创建遗漏 Job descriptor（2026-09-14）

本次错误发生在修复部署之后：r18 部署验收完成于 2026-09-13 16:48:54 UTC，cesc r4 失败于 16:49:36 UTC。不是旧版本残留报错。

## 原因与修复

`RuntimeExecutionPermit::online` 直接调用底层 `review_stock_runtime_locked`，其默认 `job_mcp=None`。普通 CLI/API 的 `ReviewRuntime` 另有读取历史启动回执或本机已安装 descriptor 的逻辑，typed 路径未使用它。因此 profile 含 `cutex_job` 时，即使本机 descriptor 已正确安装，也在进入 Prepared 前失败。

抽取 `inherited_runtime_job`，让两个入口共用相同逻辑：先读该 native contract 的已完成启动/迁移配置，否则使用 LocalDeployment；为新 review 刷新当前 Job daemon occurrence 并验证 descriptor。保持历史回执不变，不删除 Job MCP，也不放宽凭据、adapter、launcher 或 socket 校验。已存在的 runtime action 仍重放原 review。

## 验证

- Agent Management 库测试：148 通过，1 忽略。
- 隔离 HOME 保留真实 aemeath profile 的 `cutex_job` 配置，并安装现有 descriptor；只连接既有 Job daemon，不提交 Job 或模型请求。
- 旧 r18 重现 `selected profile requires an explicit reviewed coherent Job descriptor`，action 停在 configured，已捕获 native ID 和 durable ID，response 为 null。
- 切换修复候选、保留原 Director owner 和同一创建 action；重试成功，native ID/durable ID 不变。运行回执包含 `requires_job=true` 和非空 reviewed Job descriptor。
- Human 修改下一次 cwd/model/sandbox 后 typed Online 保留当前 owner；Undo、Restart、Offline、Online、Close 均成功，代次为 1→2→3。
- Human Stop 成功但没有 JSON stdout，测试脚本末尾原先错误假定必有 JSON；修正脚本并独立核对所有 action 回执和退休状态。隔离服务端口关闭、凭据副本清理。

上轮隔离测试删去了所有 MCP 配置，遗漏了这个实际使用条件。这次明确保留 Job 配置。回归脚本 `scripts/typed_job_lifecycle_smoke.py` 接受预先准备的隔离 fixture 目录；它需要固定端口 24762/24772、fixture.json、bundle.json、job-descriptor.json、aemeath 账号副本和 HOME 私有测试标记。必须先用旧 r18 捕获失败，再仅停隔离管理服务，以修复候选运行同一目录。该脚本不提供通用安装或凭据复制器，不应直接以生产 HOME 运行。

## 现场续接

`create-pro-review-worker-r4-20260914-v1` 实际仍在 configured，最终 response 为 null；虽然调用结果显示 owner_action_required，原 action 可续接。修复部署后重试同一个 action ID 和原请求，不必改 profile、增加 descriptor 或新建 r5。

r3 的 v1 action 仍有独立的 native_session_captured 预留；以 v2 创建同名同目录被拒是保留原创建身份的幂等约束。已有 r4 后不应同时恢复 r3 来制造重复 worker。本轮不删除或重写这些生产历史，也不自动派发实验。

## 部署结果

代码 `38704a9c07ea16cbeb10c5edb3e220eed4343239` 已部署到 `release-job-r19`。CLI/Bus/Management 已更新，393 个身份及 4 个存活 native owner 保留，PID/出生时间/代次不变、心跳前进。两服务 active、NRestarts=0；Task 数据没有清空，quick_check=ok，current/receipts/events 仍为 2/1/1。生产 r4 原创建 action 未代替 Director 执行；实验仍由原负责人按原合同派发。
