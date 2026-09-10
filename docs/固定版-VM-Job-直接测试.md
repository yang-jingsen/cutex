# 修复版 VM Job：直接进入 CLI

这是新的 `pi1/h1` 私有会话，**不是旧 hj1**。旧 SSH 窗口、旧历史和 held 项未动。
准备由 Worker 完成；下面的命令只 attach，不会暗中启动服务、重审阅或创建会话。
固定组合：Cutex d89a1585 / native ca580a783 / Job f3bc9c8，默认字节见
`artifacts/provider-item-pin-r1/build-manifest.json`，SHA256
`0153e3e1a1625b6e08c3e710ba17dac675ac2ecbd71559a6535721e2c8f8d8cb`。

## 1. 在宿主机终端直接连接

```sh
ssh -t -F /mnt/mambo/vmstore/cutex-linux-acceptance/ssh/config cutex-linux-acceptance \
 'python3 -B /home/cutex-linux-test/acceptance-upload/pi1/fixtures/fixed_human_entry.py attach'
```

预期进入原生 CLI，模型 `gpt-5.6-terra` / low，接收端 read-only / on-request。
若出现项目未信任提示，不必为了消除提示点 Trust。入口拒绝/进程已退出时停止，
不要运行旧脚本、改 marker、重试旧 Job 或另起原生 writer。

## 2. 只提交一次测试

可选先问“只回复 READY，不调用工具”；然后粘贴下面内容一次：

```text
使用 tool_search/CodeMode 查找并调用 tools.mcp__cutex_job__submit 一次：
actionId: human-fixed-job-1
argv: ["/bin/sh", "-c", "cat probe-readable; printf real-job-output"]
cwd: /home/cutex-linux-test/acceptance-upload/pi1/h1
不要直接调用 shell/exec_command，不轮询、不重试提交、不读取其他文件。
提交后简短回复 Submitted。正常完成通知到达后，只调用一次该 Job 的 read_output
读取 stdout，并报告输出。任何步骤报错就停止，不释放或重试 held 输入。
```

只批准这个 Job 及其 stdout read，不批准其他命令或权限升级。预期 stdout 包含
`private-read-success` 和 `real-job-output`。约两分钟仍无结果，或任何错误出现，
停止发消息并报告；不要再提交一次。submit、exit0、stdout、read_output/最终回复、
完成通知 A4 是不同事实；不能互相替代。

## 3. 错误与退出

窄范围错误观察器已经单独运行，不需要额外终端。嵌套 error.message 先脱敏再写入
guest 本地0600文件：
`/home/cutex-linux-test/acceptance-upload/pi1/h1/human-errors.jsonl`。
willRetry=true 是中间错误，false/failed completion 是终止证据。
只分享审核后的短错误、Job ID 和成功到哪一步；不贴 auth、HTTP headers、配置、
完整对话或未审核日志。该文件不存在不等于成功。模式脱敏不是任意文本绝无秘密的保证。

退出 CLI 不会停止接收端，方便保留故障现场。确认不再需要运行时后，可在宿主机执行：

```sh
ssh -t -F /mnt/mambo/vmstore/cutex-linux-acceptance/ssh/config cutex-linux-acceptance \
 'python3 -B /home/cutex-linux-test/acceptance-upload/pi1/fixtures/fixed_human_entry.py cleanup'
```

输入 CLEAN 才清理；它核对记录的 PID、启动 tick、exe 和进程组，仅停止此新夹具，
不强杀身份不明或超时进程；成功后删除本次 guest auth，保留历史。
失败则停止并联系 owner，不 pkill/rm目录。它不清理旧 hj1 或宿主机认证。
新服务是临时私有进程，无 systemd/autostart/PRH 新 owner。

## 准备边界

最小 aemeath 配置只包含已批准 provider mode、terra/low；无宿主机其他 MCP/settings
迁移。宿主机 auth 仅经 SSH 复制，0600存于此新 native home；不打印、不同步刷新回去。
为 Human 测试暂时保留认证和进程，测试结束需执行上面的精确清理。
准备不发送 turn/start、Job submit、审批或 held retry；真实 provider 完整成功仍待 Human。

## 本次实际准备记录

- 运行身份：`cutex.01a08d51-be7c-71c3-b968-9dee81e85356`；native ID 为同 UUID。
- 当前 generation1，私有 Bus24900 / Management24901；旧 hj1 使用的24800/24801未动。
- 实际 root review/activation、真实 runtime readiness 成功，原样启动回执重放一致。
- 实际原生 MCP discovery：cutex_job connected，submit/query/read_output/cancel；
  submit/read_output schema 非空。不是手工伪造 Core metadata 或 grant。
- 准备完成时原生历史零 turn，未创建 Job，脱敏错误文件尚未出现。
- 原始 staged-auth 已移除；新 native home 的临时 auth0600为本次 Human 测试保留。
  只复制必要 auth，host 配置/认证未写入。没有自动模型调用或审批。
- 精确进程 PID、启动 tick、exe/进程组、endpoint 身份在 guest 私有
  `pi1/h1/handoff.json`；不要把文件直接贴到协调消息。清理命令以此验证，不猜 PID。
- 新脚本仅夹具/文档，未修改产品、native或Job源。3项AST、8项既有脱敏检查和diff检查通过。
- 实际私有 PTY 同 owner attach 已显示 gpt-5.6-terra，未发送文字/Enter/审批；
  Ctrl+C 正常退出0，未强制杀 CLI，termios 恢复。接收端及三服务/观察器仍由出生身份核验存活。
  这不是 Job执行/模型最终回复验证；完整真实provider结果仍未证明。

## Human 测试后的只读收尾

Human 随后实际完成一次测试，报告 Submitted → 完成通知触发续行 →
cutex_job.read_output → 正确最终 stdout。read_output 显示36字节、gap=false、
truncated=false，内容为 private-read-success 换行接 real-job-output。

只读持久化核对：仅1个 Job，actionId=human-fixed-job-1，
job_9763f63c55844a3ca8cd294ed7824d3a，exited/exitCode0；completionDelivery=delivered，
outbox acknowledged=true/delivered，包含 a4Receipt 字段，attemptCount9。
通知尝试次数不等于 Job 执行次数；没有观测到重复 Job 或重复 external_event item。
原生历史恰有1个 external_event，持久化 ID 仍为68字符（符合只修改请求副本的修复）。

脱敏错误观察器有1条中间断流：willRetry=true，responseStreamDisconnected，
“Reconnecting... 1/5”；因此不宣称零重连。Human 的后续正确输出证明本次恢复后完成，
未再报告旧 string_above_max_length 终止错误。未捕获完整请求或推断所有FCO兼容性。

可见性仍有限：ca580a783 的 tui/chatwidget/protocol.rs 将
ThreadExternalInputStatusChanged 归入空处理分支；目前没有可见入站通知卡片，
不是 Human 漏开信任开关。通知上下文/触发成功与可见条目是不同边界。

本收尾仅只读：无新模型请求、提交、retry/ACK、进程停止或认证删除。运行时和
guest临时认证仍保留，待 Human/Director 明确结束后按本页清理；旧hj1同样未动。
此结果证明这一真实terra-low Job闭环，不代表全量迁移、Windows或发布验收。
