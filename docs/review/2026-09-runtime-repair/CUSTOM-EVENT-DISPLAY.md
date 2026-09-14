# Cutex / cute-codex 自定义事件显示清单

本轮决定：时间放在标题行末尾，消息正文从第二行开始；窄窗口允许自然换行。下面是实际显示范围与时间来源，不把普通 MCP 工具调用都改成 Cutex 样式。

## 统一规则

- 正常完成事件的圆点：`#E08EB2`。运行中／结果不确定保留弱化提示，失败保留红色。
- 标题中的 Agent 名称：`#F7B3CD`；Task 名称：`#74BAC3`；Job 名称：`#D9B45F`。没有友好名称时，现有 ID / 短 ID 使用同类颜色。
- 时间使用本地时区：当天 `HH:MM:SS`；跨天 `MM-DD HH:MM:SS`；跨年才显示年份。时间弱化显示，不加星期。
- 有可靠持久时间才显示。不会把恢复／渲染时间当事件时间。标题中的时间表示发生时间，不是到达顺序。
- 消息预览最多两行，多余省略；Ctrl+T / 原始记录保留完整正文和结构化事实。
- 普通未知外部事件仍用通用显示，避免把未识别的数据冒充可信服务事件。

## 消息

| 类别 | 标题 | 第二行起 | 时间来源 |
| --- | --- | --- | --- |
| 发送中 | `Sending message to <agent> · <mode>` | 尚无回执 | 无；不编造发生时间 |
| 已入队发送 | `Sent message to <agent> · <mode> · <time>` | 消息预览，最多两行 | Bus 原始消息入队时间，重试保持同一时间 |
| 收到消息 | `Received message from <agent> · <mode> · <time>` | 消息预览，最多两行 | 冻结在展示事实中的原始消息入队时间 |

`mode` 为 `soon`、`passive` 或 `after-turn`。旧消息若没有时间事实，标题不显示时间。Sent 表示发送回执确认入队，不声称对方已阅读或处理。

```text
• Sent message to cesc-pro-review-r4 · after-turn · 10:12:03
  请核对报告中的两项结果，并给出摘要……
• Received message from cesc-pro-review-r4 · after-turn · 10:13:28
  已完成核对，结果如下……
```

## Task 服务入站通知

| 服务迁移 | 标题 |
| --- | --- |
| ReviewReady | `Task ready for review · <task> · <time>` |
| TerminalClosure | `Task closed · <task> · <time>` |
| Blocked | `Task blocked · <task> · <time>` |
| Declined | `Task declined · <task> · <time>` |
| AttemptAborted | `Task attempt aborted · <task> · <time>` |
| RetriesExhausted | `Task retries exhausted · <task> · <time>` |
| OwnerActionRequired | `Task needs owner action · <task> · <time>` |

第二行列出 assignment、revision、attempt。通知明确是历史状态迁移；当前状态应以查询为准。新通知携带发生时间、notification ID、transition action ID；这些事实在完整记录中可见。

TerminalClosure 不一定是成功，因此不笼统改名为 completed。旧格式 ReviewReady / TerminalClosure 等标准通知也能专用显示；缺少时间时不补造。旧的自由格式通知无法可靠解析时保留通用显示。

```text
• Task ready for review · cesc2-cellscript-pro-review-experiments-r2 · 03:50:56
  Assignment …r4-a1 · Revision 1 · Attempt 1
  Historical transition; query the task for its current state.
• Task closed · cesc2-cellscript-pro-review-experiments-r2 · 10:02:07
  Assignment …r4-a1 · Revision 1 · Attempt 1
  Historical transition; query the task for its current state.
```

示例中的 `…r4-a1` 只是文档缩写；实际详情保留完整 assignment ID。没有额外任务名称字段时，标题使用 task ID，不猜测合同标题。

## Job 完成通知

| 结果 | 标题 |
| --- | --- |
| 正常退出且 exit=0 | `Job completed · <action/name> · <job-short-id> · <time>` |
| 其他退出 | `Job exited · …` |
| 失败 | `Job failed · …` |
| 取消 | `Job cancelled · …` |
| 中断、结果未知 | `Job interrupted — result unknown · …` |
| 启动状态未知 | `Job launch unknown · …` |

可用的 action 名称和短 ID 保留；时间来自既有 exitObservedAtEpochMillis，不用持续时间推算。没有退出观测时间时不显示时间。第二行保留 exit code、运行时长、stdout/stderr 大小和截断标记，随后显示原因。

## 自定义工具调用

这些条目已有专用回执呈现，本轮统一其圆点和标题实体颜色。除发送消息新增可靠入队时间外，现有回执缺少通用持久时间字段的操作暂不显示时刻。

| 组 | 全部已识别操作 | 展示方式 |
| --- | --- | --- |
| Job | submit、query、read_output、cancel | 动词 + Job 名称／短 ID；详情保留状态、输出预览等 |
| Agent 查询 | cutex_agent_list、query_managed | 列表／查询摘要，无目标名称时不强行添加 |
| Agent 管理 | create、query_managed、online、offline、restart、close、replace、director_rotate | 动词 + Agent 名称或身份；回执失败／需要管理员操作明确显示 |
| Task Worker | start、report_status、block、resume、submit、decline、abort_attempt | 动词 + Task／assignment 身份；保留结果状态 |
| Task Director | create_revision、assign、create_and_assign、query、accept_result、request_changes、fail_result、cancel | 动词 + Task／assignment 身份 |
| Task Terminal | accept_result、request_changes、fail_result | 同类决策呈现 |

尚未列入专用注册表的工具（例如合同全文读取）保留通用 MCP 显示，避免把全文阅读回执压缩得难以查看。未来增加时间时应补持久事件字段，而不是在 TUI new() 中写当前时间。

## 与通知排序修复的关系

恢复批次按服务事件发生时间排列，不再按 SHA-256 通知 ID 字典序排列。它改善积压通知恢复顺序，不承诺跨目标、跨投递模式的全局顺序；旧事件也不会因此修改当前任务状态。历史记录和已冻结消息不重写。
