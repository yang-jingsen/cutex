# 2026-09-14 审核后修复记录

用户批准修复独立审核中的 M1–M4、L1–L4、S1–S3，以及 cesc 报告的 typed 管理失败。基线为 `74734f3`。本轮没有清空 Task Service、迁移聊天历史或改变 CODEX_HOME，也没有增加管理员登录、人工批准或 Director 在线要求。

## 修复行为

- **Stop / Offline / Restart / Retire**：native 共用实际 owner 停止实现。核对 PID 出生时间，以 pidfd 发信号；停止前捕获后代和独立 session，持久化到该 occurrence 的 `stop-scope.json`，父进程退出后重试仍检查这些进程。普通 Stop 不会在已捕获子进程仍存活时报告成功；force 会继续终止它们。复用 PID 不被信号击中。旧 owner 已不存在且原 scope 无残留时允许清理。
- **退休和恢复**：native 使用实际 binding、停止记录及归档证明，不要求不存在的专用 cgroup；旧 backend 保持原适配。活动任务只阻止 Archive，不阻止 Restore。已停止或已退休的同代 agent 不需要重新启动来获得退休资格。
- **当前运行配置**：启动回执保存实际 `launch_cwd`。Ready 重连、Attach 和登记使用该 occurrence 的 cwd；老回执仅在出生时间验证后读取 `/proc/PID/cwd`。合法更新下次 cwd 不改变当前 owner，登记通过身份检查后投影当前组。
- **统一重连**：CLI 对已有 Ready owner 请求 Management 当前重连，不仅返回历史回执。Management 不可达或 Bus 无法连接时报告失败；成功重连保持 PID、native ID 和代次。v2 响应显式保留 `runtimeAgentId`，契约 schema 和指纹同步更新。
- **Human 配置和 host**：sandbox 与 permission alias 同步更新并一起 Undo；v2 查询和生命周期使用同一个本机 alias 判定。
- **typed 管理**：迁移后的明确 native 记录优先使用实际 Host backend；当前配置来自 durable record，创建 action 的历史 spec 不被重写。typed callback 复用已持有的执行许可，避免锁重入。新建采用与 Human New 相同的空 native thread 创建及 selected-profile 配置，不发送试探模型请求。中断后同一个 action 继续已捕获的 native ID。组规范化后的合法 Ready 结果不再误报不匹配。
- **原生 TUI 配置**：支持 `resume_cwd`；保留 TUI 展示选项和缺省值，避免 `animations` 等合法原生设置阻止 session adoption。认证、provider、权限及 custom status 命令来源校验不由此放宽。
- **单任务存储**：worker prepare/execute、精确回执重放、任务消息热路径、Human Cancel/Reassign 改用 scoped 查询和事务更新。启动不展开所有历史聚合。事务内的版本比较、action digest 和不可变回执保留。
- **prepared 容量**：失效准备记录保留幂等信息，但不继续占用可执行 preparation 的 4096 容量。
- **Task 锁**：任务条件检查和 Prepared 提交使用短临界区；外部 Stop/Spawn/Connect 不持有全局 Task 锁。Prepared 的 runtime transition 使新的 worker delivery 等待正确 owner。Management/席位自身执行锁保持。
- **有界恢复响应**：Cutex 请求 `excludeTurns=true` 和最新一页一条 turn（`itemsView=notLoaded`）；从该页读取活动 turn ID，兼容旧响应。已核对本机实际部署的 native 实现会将活动 turn 放入该页，不需要修改 cute-codex 二进制。

## 实际验收

以下测试均使用隔离 HOME、凭据副本和自建进程；未向生产 agent 发送实验任务或模型请求。全部实际测试服务和凭据副本在结束后清理。

- native New → Start → Retire → Restore；Start → Stop → Retire → Restore；localhost alias Stop；sandbox 切换与 alias 一致。
- 实际 Human Stop `--force` 对指向无关测试进程但出生时间不符的陈旧 binding：清理旧状态，无关进程仍存活。
- 独立子进程测试：child 和 grandchild 各自 setsid 并忽略 TERM，普通 Stop 返回未停止；父退出后 forced 重试清理二者。持久 scope 发生 native ID / generation 不匹配时拒绝使用。
- 在线修改 cwd/组后，实际 PID、代次和当前 cwd 保持，心跳继续；真实 Restart 约 4.7 秒期间四次任务查询约 241/3/3/3 毫秒，均在 Restart 完成前返回。
- 重启隔离 Management/Bus 后 CLI 重接同一 owner；故意停 Bus 时 CLI 返回失败，恢复 Bus 后重接成功。
- typed 实际创建中断后同 action 重试保留已捕获 native ID；Human 修改下一次 sandbox/model/cwd 后 typed Online 保持原 owner；Undo、Restart、Offline、Online、Close 成功。代次按重启/重新上线从 1 到 2 到 3，Close 退休并清 binding。发送模型请求数为零。
- 存储回归将无关历史 attempt 内容设为不可解码，单任务查询、精确重放、正常 worker 状态动作、Human Cancel/Reassign 仍通过；4096 个 preparation 所属 attempt 失效后，新任务仍可 prepare。
- 有界/旧式 thread resume 响应的活动 turn 解析及 WebSocket interrupt 路径回归通过。

最终源码回归：库 882 通过 / 4 忽略；CLI 572 通过 / 4 忽略，跳过既有 `archive_view_is_secret_free_and_offline`。全部单线程执行。中间检查发现的 v2 schema 指纹和响应 fixture 已同步修正；一次并行重编译删除正在运行的测试二进制导致的子进程 ENOENT，在停止并行编译后完整重跑通过。最终候选和部署验收已完成，见下文。此前失败的中间候选从未部署到生产。

## 保留的边界

- 本轮没有宣称恶意进程的强隔离：已在捕获前完全脱离且无任何可追溯父子关系的历史孤儿，不能仅凭旧数字 PID/SID 安全识别；管理员可使用已有手动恢复入口。新 Stop 会在发信号前捕获并保存可追溯进程。没有强制新增 cgroup 门槛。
- scoped 路径解决本次全历史回执展开问题；全局 dashboard/coordinator/maintenance 查询仍有全局投影。active-assignment 仍扫描轻量 assignment 行；prepared 容量仍枚举轻量历史准备信息。不能把这些路径称为恒定时间或无限规模保证。
- maintenance 的 Task 条件在 Prepared admission 时核验，不把整个外部启动过程冻结。admission 前已经进入处理的消息可能与该边界正常竞争；没有新增数据修改冻结。
- auth/provider/工具配置仍使用已有显式投影；TUI 展示选项透传不等于任意共享配置都支持。
- 实际 native 源码核对基于本机 `a8677ec` 工作副本；本仓库不包含整个 cute-codex 源码。长期模型/Job 全业务负载未在本轮运行。

## cesc 既有 action 的续接方式

- `create-pro-review-worker-r3-20260914-v1` 仍在 `native_session_captured`，没有最终 response；修复部署后重试原 action 和原请求，继续已捕获的 native session，不另建同名 worker。
- `online-gpu-worker-pro-review-20260914-v1` 已持久化最终 `owner_action_required` 回执。精确重放仍应返回这条历史结果；修复部署后以新的 action ID 请求同一个既有 worker Online。不要为恢复该 worker 新建 Agent，也不要篡改历史失败回执。

## 最终部署

本机已部署代码 `792f4983941287b032ce3b2115d0fe3f846aa486`，发布目录 `release-review-r18`，二进制 SHA-256 `8e2341bcaf3d4e0390ec47971ac465304f16eb1519ea366c77b75c69a9c089ba`。最终二进制实际通过 New/Start、原生 TUI `animations=false`、运行中退休、停止后退休、Restore、localhost Stop、sandbox alias 和陈旧 PID 误杀防护；隔离服务与凭据清理完成。陈旧 PID fixture 原先错误复用了另一个 occurrence 的停止日志，新版正确拒绝；为陈旧 binding 分配独立测试目录后预期清理行为通过。

CLI、Agent Bus、Management 均更新。部署后 392 个既有身份保留，4 个存活 native owner 的 PID/出生时间/代次不变、心跳前进；两服务 active、NRestarts=0。任务库 quick_check=ok，current/receipts/events 为 2/1/1，部署前后相同；本轮没有清空数据。`human doctor` 和 `human tasks list` 返回成功。cute-codex bundle 未替换。

这些结果证明本轮对应修复及服务更新通过验收，不代表长期模型/Job 业务链已全量验证。已有 TUI 进程需退出列表后重新运行 `cutex` 才加载新版前台。

后续发现并修复了启用 Job MCP 时 typed 创建遗漏 descriptor 的分支，见 [Job 配置后续修复](JOB-FOLLOWUP.md)。首轮无 MCP 的隔离验收未覆盖该条件。
