# Job 展示 v3：新会话 pv3

## 进入 CLI

在宿主机终端执行：

```sh
ssh -t -F /mnt/mambo/vmstore/cutex-linux-acceptance/ssh/config cutex-linux-acceptance \
 'python3 -B /home/cutex-linux-test/acceptance-upload/pv3/fixtures/fixed_human_entry.py attach'
```

此命令只连接已准备好的同一个 owner/thread，不启动后台安装器。
这是新测试会话，pv2 和更早会话保持原样。
应显示 gpt-5.6-terra / low；接收端 read-only / on-request。
不要通过 Trust 或改权限消除警告。模型不符或入口报错就停下。

## 粘贴一次

```text
使用 tool_search/CodeMode 调用 cutex_job.submit 一次：
actionId: human-display-v3-job-1
argv: ["/bin/sh", "-c", "sleep 1; printf human-v3-job-output"]
cwd: /home/cutex-linux-test/acceptance-upload/pv3/h1
不要直接执行 shell，不轮询、不重复提交。
提交成功后回复 Submitted。收到完成通知后，只调用一次 read_output 读取 stdout，告诉我实际输出。
任何错误立即停止，不重试 held 输入。
```

只批准这份指定 Job 和 stdout 读取。预期提交/完成/输出条目使用
`human-display-v3-job-1` 和短 Job ID，完成条目显示真实状态、exit code、
观测运行时长（约一秒，不要求精确一秒）。stdout 应为 `human-v3-job-output`。
成功条目不再堆整墙 JSON；原始事实仍可在 transcript 查看。

执行退出、输出读取、模型最终回复是不同事实；仅看到 Submitted 不算完成。
普通 Job 不额外产生重复 Notice，这是默认策略。无需再等一张相同终态通知。
约两分钟仍无结果或出现错误，就停止发送，报告步骤、短错误和 Job ID；
不要自动重新提交或释放旧 hold。

可 Ctrl+C 退出（按 CLI 提示必要时再按一次），用相同命令重连检查回放。
不重新提交 Job；这是同 owner 重连，不是后台进程重启测试。

## 错误与清理

脱敏错误留在 guest 私有文件：
`/home/cutex-linux-test/acceptance-upload/pv3/h1/human-errors.jsonl`。
中间 `willRetry=true` 不等于最终失败或成功；请报告实际终态。
不要粘贴 auth、请求头、完整配置、完整对话或未经审核日志。

服务和新 guest auth 特意保留供你测试。测试结束、退出 CLI 并确定无需现场后：

```sh
ssh -t -F /mnt/mambo/vmstore/cutex-linux-acceptance/ssh/config cutex-linux-acceptance \
 'python3 -B /home/cutex-linux-test/acceptance-upload/pv3/fixtures/fixed_human_entry.py cleanup'
```

输入 CLEAN 才停止出生时间/exe/进程组匹配的 pv3 自有进程，并删除新 guest auth；
不强杀未知进程，不删除历史，不清理 pv2，不同步认证回宿主机。
新 Bus6/Job2/原生历史不能交给旧二进制作为降级回滚；需要旧版本就另建独立会话。

准备过程不发送模型 turn、Job、审批或 held retry。Human 真实测试尚待执行，
不是生产发布、Windows 或完整沙箱验收。既有 pj07 风险仍保留，未追加复现。

## 技术准备记录（无需测试者操作）

- 起点435ec5896d886eb89966c7b6295480265ac657ee / tree80bcd2baf5a97eaa6b8364e3223e4c2189ac2d98。本次只有两个小脚本及本文档；未构建或修改产品。
- 产品b48a2f63a5e60e6b0c6e6d8ca97e4ae4882dd53f / tree9fe31ce69c255c0572ac0919e9cd94cb5e0dff09。准确组合清单 artifacts/job-view-v3-r2/build-manifest.json，SHA e1e0b16b4435d2b522ab0128b9ac2632428ecc4dd118a912645c7560af118d5d，guest副本pv3/build-manifest.json。
- DEFAULT dev Cutex2412fb31/facade0aa3ea1b、native f8c33add CLIb8307571 / b8e9cc server7bc7f3d7 / Uhost3e85d674 / schema c2a54d59、Jobf7bbe3c ELFba1a8d4f；所有完整哈希在清单内，跨SSH传输后逐一比较成功，包括同目录host和CLI/server。
- 新durable cutex.01a0919d-fe8b-79d3-89d6-ed7937f75ef2；native01a0919d-fe8b-79d3-89d6-ed7937f75ef2；generation1；Bus24960 / Management24961；Job socket pv3/h1/h/job.sock。选端口前检查未监听；旧fixture未停止或重写。
- 实际中性创建/adopt/root review/activation/运行Ready和精确请求重放通过。Job daemon使用--completion-v2，private_job_presentation为version2+此recipient、默认suppress。
- 实际当前owner发现cutex7工具及Job4工具（cancel/query/read_output/submit），全部connected/schema非空。CLI前后检查相同ID/generation/PID，线程仍零turn。
- 实际PTY只attach，观察terra标签；没有文本/Enter/审批，正常exit0，无强制终止，termios恢复。已准备服务/runtime/observer继续运行供Human，而不是只交付未执行启动脚本。
- 认证从已授权pv2/h1私有auth源读取一次，非链接/TokenUser属主/0600/句柄前后稳定检查后复制；host认证未访问。新nativehome auth0600按要求暂留，staged-auth已删除；无同步回去。清理命令尚未执行。
- 准备一次成功，无本次失败或自动模型请求；真实模型/Job测试仍由Human执行。这里只证明零轮次准备及同owner attach，不宣称新的真实provider闭环或进程重启恢复。
- guest私有handoff.json保留pid/start_ticks/exe/pgid用于确认清理，cli-smoke.json保留实际终端结果。旧历史保持不变；旧Bus/Job/native writer不得打开新数据。
- Guest acceptance-upload总计12,147,687,424 bytes（<12GiB），可用46,831,857,664 bytes（>30GiB）。Mambo沿用18.31GB任务根，无构建/大缓存增量；未删除旧证据或旧Human文件。
- 脚本语法、完整本次差异及diff空白检查通过；原有独立jv04真实Job/模型显示分离证据复用。此结果仅VM_HUMAN_READY，不是生产发布或Human验收。
