# Cutex 修复版独立代码审核入口

本分支供独立审核，不代表审核已通过。首轮审核基线为 `be6374a`（材料快照 `74734f3`）；后续修复、实际验收和剩余边界见 [审核修复记录](AUDIT-FOLLOWUP.md)。以下旧版数量和现场状态仅描述首轮材料，不代表最新部署。

## 比较范围

- GitHub main 基线：`e7d01585661d350bffeea468d1db202628047490`。
- 本机已部署源码：`be6374a`，相对基线 157 个提交、288 个文件变化。这包含前期迁移开发及后续救援，不能全部归为救援作者的修改。
- 本仓库是 Cutex 管理层；cute-codex 的分页恢复补丁在另一个仓库，未包含其源码。不要把该补丁的性能验收当作此仓库的测试。
- 本目录不包含真实凭据、聊天历史、运行数据库或本机二进制。

## 产品意图（用于判断过度设计）

用户希望常规 agent 操作避免常见误操作，同时管理员具备实用的手动恢复能力。管理员经常委托同系统用户下的 YOLO agent 执行 `cutex human`，因此不要求证明操作者是物理人类、不增加独立登录/授权令牌，也不以 Director 在线为人工修复前提。Human 命令名称表达用途，不隔离同用户进程权限。

review 是内部启动准备和可恢复 action 记录，不是另一个 agent 的人工批准。应校验当前实际进程的身份，不能因下一次启动配置改变而拒绝当前进程继续运行或重连。审核时请区分必要的所有权校验与无证据的防御限制。

## 建议分三轮审核

1. **管理 API 与权限**：`src/management/{control_plane.rs,v2/server.rs,v2/session.rs,v2/human_config.rs,v2/human_tasks.rs}`、`src/cli_app/{management_context.rs,management_control_plane.rs,human.rs}`。跟踪路由认证、owner 身份传递、隐藏 agent 定位、归档/跨主机限制，以及错误后的恢复入口。
2. **生命周期与重连**：`src/agent_management/stock_runtime.rs`、`src/cli_app/{stock_lifecycle.rs,app_server_runtime.rs,session_runtime.rs,session_reconcile.rs}`、`src/session/reviewed_registration.rs`、`src/app_server/{manager.rs,bus_bridge.rs}`。跟踪重复启动、响应丢失、进程退出、服务重启、在线修改配置/包，以及 Stop/Attach 是否使用实际运行回执。
3. **任务存储**：`src/task_service/provider/`、`src/task_delivery/provider_adapter.rs` 和调用方。检查原子性、幂等回执、并发冲突、崩溃恢复、查询内存成本、watch 分页、节点共享及长期增长。旧实现每条事件保存全库和累计回执，形成约 81.87 GB 数据；本机经用户授权清空旧任务子树，没有实现通用旧格式迁移。

稳定接口说明：`docs/management/human-cli.md`、`docs/management/task-storage.md`。

## 已知边界与值得反证的地方

- Raw cute-codex、quick/profile 直启仍走旧兼容路径；新建/Adopt/托管原生生命周期已统一。不要假设全部入口实现相同。
- 19 条旧离线记录仍有不完整绑定提示，未自动重写或复活。
- Ready 进程允许 durable revision/下一次包改变；pending launch 仍绑定 review。重点检查此区分是否覆盖所有注册和恢复调用链。
- 人工配置是下一次启动意图；请特别检查其他字段（cwd、组等）是否仍混入当前运行状态。
- 不提供旧任务数据回滚。旧数据删除是一次明确授权的运维操作，不是正常启动行为。
- 长期生产负载、完整模型任务/Job 业务链路尚未全面验收。
- 部分集成测试依赖本机外部 native bundle/fixture，不能宣称克隆后可直接跑完所有测试。

## 已执行验证及其限度

- 新存储/入口集成阶段：库 867 pass / 4 ignored；CLI 565 pass / 4 ignored，单线程，跳过既有 `archive_view_is_secret_free_and_offline`。不是最终提交重新跑过的全量结果。
- 随后定向测试：管理员隐藏 agent/API 28 项，runtime 恢复 18 项；最终登记回归 6 项。
- 存储包括真实子进程退出、事务恢复、冲突、损坏、重复请求；128 次累计状态更新验证共享存储，不能代替长期压力测试。
- 真实隔离生命周期：New、owner API online、非 owner 401、前台进入且未提交模型请求、stop。
- 真实在线换包：旧进程 PID/代次不变；重启 Bus/Management 后心跳前进；停止再启动执行新路径的包。测试凭据和进程清理完成。
- 现场 CLI/Bus/Management 均部署 be6374a，两个既有 native owner 的 PID/出生时间/运行代次不变、心跳更新；392 个身份和席位/Job 凭据保留。

基础构建：`cargo build --locked --release --bin cutex`。测试环境应使用隔离 HOME，按 `src/cli_app/test_home.rs` 的约定设置；不要以真实账号 home 跑变更状态的集成脚本。

## 仓库外运维修复

systemd Bus 的 Type=simple 曾在开始监听前允许 Management 启动，Management 自动启动第二个 Bus。已给 Bus 加 `ExecStartPost`，等待认证后的 HTTP 成功；Management 使用 `After=` 和 `Wants=` 依赖 Bus。两服务 `KillMode=process`，避免停止服务时杀死持久 native owner。

`wait-agent-bus.py` 为实际部署脚本副本，不含令牌值。部署配置示意（路径需按安装位置调整，不要直接覆盖现有服务）：

```ini
# cutex-agent-bus.service.d/20-ready-before-management.conf
[Service]
ExecStartPost=/usr/bin/python3 /PATH/TO/wait-agent-bus.py

# 两服务各自的 drop-in
[Service]
KillMode=process

# Management unit 的依赖
[Unit]
After=cutex-agent-bus.service
Wants=cutex-agent-bus.service
```

Management 还设置了服务私有 TMPDIR；脚本从当前用户的配置中取凭据，固定本机端口 24260，这是本机部署适配而非通用安装器。

## 给审核模型的请求

请独立核对代码，别把本说明当正确性证明。优先报告可触发的行为错误、数据丢失、权限混淆、阻止合法恢复的限制和明显写入/读取放大。每项给出文件/函数、触发步骤、实际后果和最小修复建议；区分已证实问题与待测推断。避免无证据地增加人工批准、角色链、迁移冻结或新的配置门槛。
