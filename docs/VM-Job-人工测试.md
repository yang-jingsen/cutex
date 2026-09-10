# VM Job 人工诊断：连接、启动、测试

本页替代旧的 `human_job_terminal.py` 一键入口。**不要再运行旧入口。**
这是人工操作说明，不是已经通过的端到端测试。以下启动、认证复制和模型测试均未在本任务执行。
固定产品 c69241c / native 0c425 / Job f3bc9c8；模型 `gpt-5.6-terra`、reasoning `low`，不是 luna。权限保持 readOnly / on-request，不切换 fullaccess。

## 1. 连接与实际残留

在 Linux 宿主机终端运行：

```sh
ssh -F /mnt/mambo/vmstore/cutex-linux-acceptance/ssh/config cutex-linux-acceptance
```

本次只读检查：`hj1/h203236` 停在 `configured-job-launch: prepared`；generation=0，无运行绑定、无 launch claim。Bus 24800 和 Management 24801 无监听，记录的 Job PID 27014 已退出，但 `h/job.sock` 残留。两个临时 aemeath auth 文件已删除（不是推测关闭脚本必然清理）；三个私有 Job 凭证文件仍在，权限0600。没有 Job state、没有 `human-errors.jsonl`，原生历史只有 session_meta，没有模型 turn 或 held 项。

因此目前不能直接 attach；需要人工恢复下面三个私有服务并**重新审阅**。保留旧 prepared 回执，不能重放旧启动审阅（Job PID/auth custody 已变化）。不创建新 thread，不触碰 r3/r4 的 held 项。

固定身份：`cutex.01a08d05-bbb5-7f32-a849-02cceb9b2564`；原生 ID 是同一 UUID（无 `cutex.` 前缀）。若现在状态已不同，先停止本步骤，不能照抄“已退出”结论。

## 2. 一次性准备与逐步启动

先在宿主机复制**本次辅助脚本，不含认证**：

```sh
scp -F /mnt/mambo/vmstore/cutex-linux-acceptance/ssh/config \
 /mnt/mambo/PersonaProjects/cutex-mcp-facade-r1/source/scripts/hj1_manual.py \
 cutex-linux-acceptance:/home/cutex-linux-test/acceptance-upload/hj1/fixtures/hj1_manual.py
```

辅助脚本每次只执行一个明确操作：status / review / run / observe / offline；不会开服务、复制认证、创建线程、发送 prompt 或自动重试。已有脱敏模块和固定 RPC 夹具已在 guest。

### 2a. 每个 VM 终端的公共设置

打开多个 SSH 终端，每个先执行：

```sh
R=/home/cutex-linux-test/acceptance-upload/hj1/h203236
B=/home/cutex-linux-test/acceptance-upload/hj1
N=/home/cutex-linux-test/acceptance-upload/vm-r1
umask 077
private() {
 env -i PATH=/usr/local/bin:/usr/bin:/bin LANG=C.UTF-8 TERM=xterm-256color \
 HOME="$R/h" CODEX_HOME="$R/h/.cutex/codex-home" CUTEX_TEST_PRIVATE_HOME="$R/h" \
 TMPDIR="$B/tmp" LD_PRELOAD="$N/connect-guard.so" S4_TEST_ALLOWED_PORTS=24800,24801 "$@"
}
python3 -B "$B/fixtures/hj1_manual.py" status
```

若出现已 ready 的当前运行，不运行 review/run，不另开原生 app-server；转到 2e 使用同一 owner。若出现 claim、非零 generation、未知阶段或与上文不同，保留状态并报告，不自动恢复。

### 2b. 认证：只有决定开始测试时，才由 Human 在宿主机执行

不打印、粘贴或把认证值放到命令参数。确认 guest 目标仍不存在后，用 SSH 加密 stdin 写入0600新文件；不改宿主机原文件，不同步刷新值回宿主机：

```sh
ssh -F /mnt/mambo/vmstore/cutex-linux-acceptance/ssh/config cutex-linux-acceptance \
 'umask 077; set -C; cat > /home/cutex-linux-test/acceptance-upload/hj1/h203236/h/.cutex/codex-home/auth.json' \
 < /home/senxiu/.cutex/profiles/cd6a39eb-3997-45c6-9824-5113fe36a4b8/auth.json
```

此命令传输秘密但不显示内容；禁止 `set -x`。文件已存在就停止，不能覆盖未知认证。这里只恢复先前批准的最小 aemeath guest 配置；没有迁移宿主机其他 MCP/settings。

### 2c. 三个服务，三个可见终端

终端 A（保持打开）：

```sh
private "$B/bin-v2/cutex" agent serve --port 24800
```

终端 B（保持打开）：

```sh
private "$B/bin-v2/cutex" management serve --port 24801
```

终端 C：先检查旧 socket 没有监听（下面第一条若有输出，停止，不删除）：

```sh
ss -xlpn | grep -F -- "$R/h/job.sock"
test ! -e /proc/27014/exe
```

确认仍与只读检查一致、没有其他人启动新 Job 后，仅删除这个已证实的陈旧 socket（不是目录/历史/凭证）：

```sh
test -S "$R/h/job.sock" && rm -- "$R/h/job.sock"
private "$B/bin/cutex-job-service" serve "$R/job-state" "$R/h/job.sock" \
 "$R/h/job-api" "$R/h/job-grant" "$N/native/bin/codex" \
 --completion http://127.0.0.1:24800 \
 "$R/h/.cutex/runtime/task-service/job-service-completion.token"
```

预期几个秒内服务显示监听信息，前台不返回。第四个终端检查 `ss -ltnp 'sport = :24800'`、`ss -ltnp 'sport = :24801'` 和上述 socket 检查。有地址占用/退出/权限错误立即停止，不改端口、不杀未知进程。30秒仍无监听视为失败，而不是继续静默等。

### 2d. 新审阅，然后单独确认启动

第四个 VM 终端（公共设置后）：

```sh
python3 -B "$B/fixtures/hj1_manual.py" review
```

它从原审阅取固定 Job 文件描述，读取当前 socket 的真实 peer PID/启动时间，再交由真实 Management 重新校验；不签 grant、不伪造 Core metadata。结果是本地0600 `manual-review.json`。仅在 VM 本地审阅，确认同一 durable/native ID、terra/low、readOnly/on-request、原 bundle 和本页 Job 路径。不要把整个审阅文件发给别人。

```sh
python3 -B "$B/fixtures/hj1_manual.py" run
```

输入 `RUN` 才提交一次。每15秒显示仍在等同一个请求，最大240秒；必须返回 `stage: ready` 才连接 CLI。超时/中断是结果未知，不意味着未启动，**不要再点 run、删标记、重建 Agent 或换 action ID**。查看 status 并报告阶段。`manual-run-started` 是防误点标记，不是产品事务凭证；真实阶段以 durable store/服务回执为准。

### 2e. 先开错误观察，再连接 CLI

单独 VM 终端：

```sh
python3 -B "$B/fixtures/hj1_manual.py" observe
```

看到 `Observing errors only` 后，在另一个有公共设置的终端：

```sh
private "$B/bin-v2/cutex" session stock-attach \
 cutex.01a08d05-bbb5-7f32-a849-02cceb9b2564
```

这是受支持的 exact-runtime → 同 owner `resume --remote` 路由，不是第二个 writer。不要改成裸 `codex resume`、新会话或 `session online`。核对 CLI 模型/权限；若 attach 拒绝或30秒没有可用界面，记录可见错误并停止，不使用备用入口。观察器只订阅同线程错误，无 turn/审批/retry/ACK；没有错误时不输出是正常的。

## 3. 测什么、如何判断及反馈

可选先问“只回复 READY”；已有正常回复无需重复花费。然后只粘贴一次：

> 使用 tool_search/CodeMode 查找并调用 tools.mcp__cutex_job__submit 一次：actionId 为 human-hj1-job-1，argv 为 ["/bin/sh","-c","cat probe-readable; printf real-job-output"]，cwd 为 /home/cutex-linux-test/acceptance-upload/hj1/h203236。不要直接调用 shell/exec_command，不查询轮询、不重试提交、不读取其他文件。提交后简短回复 Submitted。正常完成通知到达后，只调用一次该 Job 的 read_output 读取 stdout，并简短报告输出。如果任何步骤报错就停止，不释放或重试 held 输入。

只批准这一个 Job 和它的 stdout read；拒绝其他命令、文件、联网、权限升级。预期输出包含 `private-read-success` 和 `real-job-output`。出现错误立即保留诊断，不再次提交；正常完成没有到达时约2分钟后记录“未收到”，不要为了等 Job 不断发模型消息。

分别记录：①submit 返回 Job ID；②进程 exit0；③stdout 可读；④模型确实调用 read_output 并给最终回复；⑤完成消息原生 A4/业务 delivery。①不等于②，②③不等于④⑤；A4 是上下文持久化，不是模型成功回答。最后一项可由后续只读检查确认，不能凭 CLI 看起来完成就宣称。

本地诊断（只有出错才创建）：
`/home/cutex-linux-test/acceptance-upload/hj1/h203236/human-errors.jsonl`。
`willRetry=true` 是中间错误；`willRetry=false` 或 failed completion 是终止证据。未知 `code=other` 的描述不会被故意丢掉，但脱敏会移除 token/URL/headers/dump。模式脱敏不是任意文本绝不含秘密的保证：先本地检查，只报告模型/阶段、Job ID、exit code、以上五项和审核后的短错误，不粘贴 auth、完整审阅、HTTP headers/config/env/对话。文件不存在表示“没捕获错误”，不等于成功。

## 4. 退出与精确清理（由 Human 执行）

先退出 CLI，Ctrl+C 结束观察器；这两步**不会自动停止 owner**。保持三个服务还运行时：

```sh
python3 -B "$B/fixtures/hj1_manual.py" offline
python3 -B "$B/fixtures/hj1_manual.py" status
```

offline 只请求私有24801、当前 generation 的受支持 offline 操作，不 force。不是默认端口的 `session offline`。若服务拒绝/仍有当前运行/结果不明，停止清理并报告，不 pkill、不猜 PID。确认已离线后，在各服务前台终端 Ctrl+C。不要删除历史/回执/Job state；保留供诊断。

不论测试是否成功，结束认证使用后，仅清理本次指定 guest 认证文件（正常 refresh 也在这个私有文件内）：

```sh
rm -f -- /home/cutex-linux-test/acceptance-upload/hj1/h203236/h/.cutex/codex-home/auth.json
test ! -e /home/cutex-linux-test/acceptance-upload/hj1/h203236/h/.cutex/codex-home/auth.json
```

不删除宿主机 auth，不整个目录清理，不自动 retry/ACK/release r3/r4/hj1。私有 Job dummy 凭证可保留用于诊断，不要公开。

## 验证边界

本页路径、现存文件结构、服务源码路由和残留状态已只读核对；辅助脚本 AST 检查通过，8项既有纯脱敏检查通过，实际 guest `status` 只读执行通过（generation0/prepared），`git diff --check` 通过。新手动服务组合、review/run/observer/offline 和实际 CLI/付费交互**未执行**；因此没有新的 provider 成功或故障原因结论。准备任务没有复制认证、启动/停止进程、删文件、重试旧项或调用模型。先前只证明 submit/exit0/stdout/A4，终止错误原因仍未知。非部署/发布验收。
