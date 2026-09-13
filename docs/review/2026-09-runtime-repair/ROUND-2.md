# 第二轮跨仓库审核入口（2026-09-14）

本轮材料补齐实际部署的 cute-codex light 源码、独立 Job Service 和 PRH/hostctl。请以指定 commit 为审核对象，不能仅查看两仓库的 main/release。此次整理没有更换任何运行二进制。

## 源码与部署对应

| 组件 | 固定源码 / 审核位置 | 说明 |
| --- | --- | --- |
| Cutex CLI、Agent Bus、Management API、Task Service | [`38704a9`](https://github.com/yang-jingsen/cutex/tree/38704a9c07ea16cbeb10c5edb3e220eed4343239)；分支 `review/runtime-repair-20260914` | 当前运行代码。该分支后续提交仅补充审核材料。 |
| cute-codex CLI、app-server、Code Mode host | [`a8677ecdfbf2ae7a74557ac5263340bb9d8d9422`](https://github.com/yang-jingsen/cute-codex/tree/a8677ecdfbf2ae7a74557ac5263340bb9d8d9422)；分支 `review/runtime-repair-20260914` | 对应 release-native-r1；现场三个文件 SHA-256 与部署记录一致。 |
| Cutex MCP facade | Cutex [`d9e4e29178b3a36f6e6befecdc6b6fffff7ed318`](https://github.com/yang-jingsen/cutex/tree/d9e4e29178b3a36f6e6befecdc6b6fffff7ed318)，`src/bin/cutex-mcp.rs` | 沿用该次构建的 facade，不应误称它由最新 Cutex 提交重新构建。源码和历史已经在 Cutex 仓库。 |
| Job Service / Job MCP adapter | [`f7bbe3c42fbf30bfb5fe379c6f6cd5af951991b9`](https://github.com/yang-jingsen/cutex/tree/f7bbe3c42fbf30bfb5fe379c6f6cd5af951991b9)；分支 `review/job-service-20260914` | 独立 Git 历史发布在 Cutex 的专用分支。主审核分支中的 `review-sources/job-service/` 是完全相同的 tracked source。 |
| PRH、hostctl、Linux sentinel / Windows host | [`2fcb9c4b09d44e1010cfed2983e094aa53261ebe`](https://github.com/yang-jingsen/cutex/tree/2fcb9c4b09d44e1010cfed2983e094aa53261ebe/persistent-runtime-host)；分支 `review/persistent-runtime-host-20260914` | 对应 `persistent-runtime-host/` 子项目。主审核分支 `review-sources/persistent-runtime-host/` 为相同源码树；当前 PRH host 文件 hash 与该部署清单一致。 |

Job 主 daemon 和当前安装的 adapter 为 f7bbe3c；现场另有旧 native 使用 36f8b577 的 adapter，属于同一 Job 分支历史，未在本次发布源码时替换。历史代码可直接按 commit 查看。

`review-sources/` 是审核快照，不参与 Cutex 默认构建，不表示两个附属项目已经合入 Cutex 的运行生命周期。Job 原根 tree 为 `7c722b7613fb2c07c59c9efcdacfbbd2b37aad0a`，PRH 子树为 `d5559e72c3ce55696c54f74b08cab236221a55d4`。

## 比较范围

- Cutex 首轮审核快照为 `74734f3`。本轮代码修复截至 `38704a9`，可以直接比较两者；最新分支另包含说明和配套源码快照。
- cute-codex 实际 light 改造基线为 `3d2ee51ca2d5db578f328aa75e20aa22c0197c9a`，到 a8677ec 有 63 个提交。最近的恢复优化仅是 `8cde7956..a8677ec` 两个提交，不能把此前 light 改造遗漏掉。
- GitHub 原 cute-codex release `d52dc3d14bb36fa783a5e3c1942d7d13bd86d8c4` 与当前部署历史没有共同祖先（两个仓库历史均非 shallow）。请使用两个快照的直接 diff，或以上 light 基线；不要使用依赖 merge-base 的三点比较。

```bash
# 在 Cutex 仓库
 git diff 74734f3 38704a9 -- src
# 在 cute-codex 仓库，light 改造及随后恢复修复
 git diff 3d2ee51ca2d5db578f328aa75e20aa22c0197c9a a8677ecdfbf2ae7a74557ac5263340bb9d8d9422
# 若要核对上一轮看到的 release：比较两个完整树（不是三点 diff）
 git diff d52dc3d14bb36fa783a5e3c1942d7d13bd86d8c4 a8677ecdfbf2ae7a74557ac5263340bb9d8d9422
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

Cutex 审核后完整回归为库 882、CLI 572 通过，8 忽略、1 条既有测试跳过；Job 后续改动定向管理测试 148 通过、1 忽略。真实隔离测试覆盖同 action 恢复、当前/下次配置分离、重连、Stop/退休/Restore、typed 生命周期、保留 Job 配置及并发任务读取。cute-codex 部署前的历史记录为 thread-store 242、TUI 51 通过；这次源码发布没有重跑或扩大该验证结论。

未在本轮运行完整模型/Job 业务负载、恶意进程逃逸或长期规模压力测试。全局 Task 投影、轻量 active/prepared 扫描仍存在。模型侧行为和多入口边界仍值得反证。源码发布不代表审核通过，也不更改 main 或 release。

材料只包含 tracked source、测试和说明；不包含运行数据库、聊天历史、账号凭据、真实令牌或部署二进制。
