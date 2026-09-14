# 第二轮跨仓库审核入口（2026-09-14）

本轮材料补齐实际部署的 cute-codex light 源码、独立 Job Service 和 PRH/hostctl。请以指定 commit 为审核对象，不能仅查看两仓库的 main/release。最新追加了消息显示、Task 合同交接和 Job cwd / launcher 兼容修复，详见 [修复记录](MESSAGE-TASK-JOB-FIX.md)。分隔线时间恢复详见 [时间显示记录](TIMESTAMP-FIX.md)。legacy 分页遗漏的补修详见 [修复记录](LEGACY-TIMELINE-FIX.md)。Task MCP 参数可用性修复详见 [记录](TASK-MCP-ARGUMENTS-FIX.md)。MCP resubmit 传输修复见 [记录](TASK-MCP-RESUBMIT-TRANSPORT-FIX.md)，通知恢复排序修复见 [调查](TASK-NOTIFICATION-DELAY-FINDINGS.md)，本轮全部自定义显示见 [清单](CUSTOM-EVENT-DISPLAY.md)。旧发布记录保留在 Git 历史。

## 源码与部署对应

| 组件 | 固定源码 / 审核位置 | 说明 |
| --- | --- | --- |
| Cutex CLI、Agent Bus、Management API、Task Service | [`9c4af47`](https://github.com/yang-jingsen/cutex/tree/9c4af47)；分支 `review/runtime-repair-20260914` | release-runtime-r26 优化构建，新增通知恢复排序、发生时间和 Task 展示事实。 |
| cute-codex CLI、app-server、Code Mode host | [`382431657`](https://github.com/yang-jingsen/cute-codex/tree/382431657)；分支 `review/runtime-repair-20260914` | release-native-r7 CLI 新增统一事件样式；app-server 与 Code Mode host 复用 r4 的相同二进制（同包复制）。legacy 缓存保留。 |
| Cutex MCP facade | Cutex `2dbdf1f`，`src/agent_bus/mcp_tasks.rs` / `mcp_http_response.rs` | release-runtime-r25 优化构建，包含逐操作参数说明、完整 HTTP 回执读取和不确定响应阶段诊断；native-r7 继续引用该 facade。 |
| Job Service / Job MCP adapter | [`bbaebdd7a4ec6c05290d633d7ad2a174f53468f5`](https://github.com/yang-jingsen/cutex/tree/bbaebdd7a4ec6c05290d633d7ad2a174f53468f5)；分支 `review/job-service-20260914` | 独立 Git 历史发布在 Cutex 的专用分支。主审核分支中的 `review-sources/job-service/` 是完全相同的 tracked source。 |
| PRH、hostctl、Linux sentinel / Windows host | [`2fcb9c4b09d44e1010cfed2983e094aa53261ebe`](https://github.com/yang-jingsen/cutex/tree/2fcb9c4b09d44e1010cfed2983e094aa53261ebe/persistent-runtime-host)；分支 `review/persistent-runtime-host-20260914` | 对应 `persistent-runtime-host/` 子项目。主审核分支 `review-sources/persistent-runtime-host/` 为相同源码树；当前 PRH host 文件 hash 与该部署清单一致。 |

Job 主 daemon 和新安装的 adapter 为 bbaebdd；旧 native 的 adapter 进程保留。daemon 同时接受旧、新 launcher，旧进程在下次启动时换用新版本。

`review-sources/` 是审核快照，不参与 Cutex 默认构建，不表示两个附属项目已经合入 Cutex 的运行生命周期。Job 原根 tree 为 `e18dbc983e9666b43466cb94ab45caab2289385d`，PRH 子树为 `d5559e72c3ce55696c54f74b08cab236221a55d4`。

## 比较范围

- Cutex 首轮审核快照为 `74734f3`。本轮代码修复截至 `9c4af47`，可以直接比较两者；最新分支另包含说明和配套源码快照。
- cute-codex 实际 light 改造基线为 `3d2ee51ca2d5db578f328aa75e20aa22c0197c9a`，到 4e3e2b1fc 有 66 个提交。最近的恢复优化仅是 `8cde7956..a8677ec` 两个提交，不能把此前 light 改造遗漏掉。
- GitHub 原 cute-codex release `d52dc3d14bb36fa783a5e3c1942d7d13bd86d8c4` 与当前部署历史没有共同祖先（两个仓库历史均非 shallow）。请使用两个快照的直接 diff，或以上 light 基线；不要使用依赖 merge-base 的三点比较。

```bash
# 在 Cutex 仓库
 git diff 74734f3 9c4af47 -- src
# 在 cute-codex 仓库，light 改造及随后恢复修复
 git diff 3d2ee51ca2d5db578f328aa75e20aa22c0197c9a 4e3e2b1fc9b1c8931e7a472ab01b665a089f42d3
# 若要核对上一轮看到的 release：比较两个完整树（不是三点 diff）
 git diff d52dc3d14bb36fa783a5e3c1942d7d13bd86d8c4 4e3e2b1fc9b1c8931e7a472ab01b665a089f42d3
```

## 审核顺序与产品意图

先读 [首轮修复记录](AUDIT-FOLLOWUP.md) 和 [Job 漏接后续修复](JOB-FOLLOWUP.md)，但请独立验证其中结论。

1. **入口一致性**：Human、CLI、typed Create/Online/Restart/Offline/Close、已有 owner reconnect 是否都使用正确实际身份及已安装配置。重点检查 Job descriptor、profile 投影、未完成 action 的同 ID 续接。
2. **停止与退休**：核对 PID 出生标识、pidfd、独立子孙进程捕获、父退出后的重试，以及 Archive/Restore 各自条件。当前方案不是恶意进程强隔离；不要凭未捕获的旧数字 PID 杀进程。
3. **跨仓库恢复与活动 turn**：Cutex `app_server/{commands,manager}.rs`、native `app-server/src/request_processors/thread_processor.rs`、thread-store/history 和 TUI。核对 `excludeTurns` / `initialTurnsPage` 的实际实现、active turn 保留、interrupt 和历史显示。
4. **任务存储与 Job 完成链**：Cutex scoped provider/storage、bridge、Job `src/` 和 frozen completion facts。核对精确重放、并发锁范围、长期读取成本以及消息/任务/运行代次关系。

管理员经常委托同系统用户下的 YOLO agent 执行 `cutex human`。不要求物理人证明、独立登录、Director 在线、额外人工 review 或迁移冻结。必要校验应保护实际身份、事务和清晰的幂等语义，不应把当前配置/历史失败回执变成无法合法恢复的门槛。

请按严重程度报告具体调用链、触发步骤、后果、最小修复和应补测试。区分静态证实、实际复现和条件性风险；不要仅凭命名或旧说明认定当前代码正确。

## 证据边界

当前验证：Cutex 库 886、CLI 572 通过，8 忽略、1 条既有测试跳过；另新增 launcher preflight 线协议测试通过。Job Service 全量测试 23 项通过，真实 MCP 使用本地模拟模型验证子工作目录和第二个 allowlisted launcher。隔离 native / Job 生命周期覆盖新建、复用、重启、离线、上线和关闭，未发送模型请求。

cute-codex 消息显示定向测试 13 项通过。全量 TUI 为 4091 通过、37 失败、1 个 IDE IPC 测试超时、6 跳过；失败位于未改动区域，包括版本号快照差异，未将这些快照改成新结果。请区分定向回归通过与完整套件未全绿。详细边界见修复记录。

尚未进行长期规模压力测试或恶意进程逃逸测试。完整合同读取、原 sandbox 的 cwd 执行、旧/新 launcher 共存和各入口新建配置值得优先反证。源码发布不代表审核通过，也不更改 main 或 release。

材料只包含 tracked source、测试和说明；不包含运行数据库、聊天历史、账号凭据、真实令牌或部署二进制。
