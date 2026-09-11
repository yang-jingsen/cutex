# 展示 v2：新 VM 会话 pv2

新目录 pv2/h1，不是 pv1/pi1/hj1。旧历史、owner、auth、held 输入均不动。
仅 Human 私有测试，不是生产更新。准确组合见
artifacts/display-v2-composition-r1/build-manifest.json，SHA256
d4c56c783fddf74ca2dd2721f40f13099ece9a7fbc77e78e42b4cb79208d1166。
Cutex 编译源码2e7ff4a / native cc4a080d / Job f3bc9c8，默认字节，未重建。
准备已完成，入口可以直接连接。

## 1. 进入已启动的同一线程

```sh
ssh -t -F /mnt/mambo/vmstore/cutex-linux-acceptance/ssh/config cutex-linux-acceptance \
 'python3 -B /home/cutex-linux-test/acceptance-upload/pv2/fixtures/fixed_human_entry.py attach'
```

入口只 attach，不隐藏服务启动。模型 gpt-5.6-terra / low，接收端
read-only / on-request。不要通过 Trust 或修改权限来消除提示。

## 2. 粘贴一次

```text
使用 tool_search/CodeMode 调用 cutex_job.submit 一次：
actionId: human-display-v2-job-1
argv: ["/bin/sh", "-c", "cat probe-readable; printf compact-job-output"]
cwd: /home/cutex-linux-test/acceptance-upload/pv2/h1
不要直接执行 shell，不轮询、不重复提交。
提交后回复 Submitted；收到完成通知后，只调用一次 read_output 读取 stdout。
任何错误立即停止，不重试 held 输入。
```

只批准指定 Job 和 stdout 读取。预期显示实际 actionId + 短 Job ID，
成功条目不再有整墙 JSON；输出有 stdout/字节范围及有界文本预览：
private-read-success 换行接 compact-job-output。原始结果仍在 transcript/raw。
未知/失败回执可能保留详细信息，不等于折叠失效；不要把它当成功。

此会话 `private_job_presentation.version=2`，默认不生成重复 Notice。
仍会有真实模型入站完成通知并触发后续读取；不应另外等一张相同终态 Notice。
独立 summary 是显式可选能力，不是此默认 Job 测试的一部分。
相邻且有明确引用的记录可合卡，但异步 Job 不保证相邻；不重排 Agent 回复
来制造合卡。该功能已有同一原生字节的 p6/p7 专项证明，无需强行重测。

可退出 CLI 后同命令重连查看回放，不再提交 Job。约两分钟仍无结果或报错，
停止发消息并报告短错误/Job ID/步骤；不无限等待或重试。

## 3. 错误、保留与清理

窄范围脱敏观察器将嵌套错误保存在 guest 本地
`/home/cutex-linux-test/acceptance-upload/pv2/h1/human-errors.jsonl`（0600）。
不要贴 auth、请求头、完整配置/对话或未经审核日志。willRetry=true 是中间错误，
不是最终成功。缺少错误文件也不证明测试成功。

退出 CLI 不会清理后台 owner。准备成功后特意保留服务及 guest auth；
测试结束并确定无需现场时，才执行：

```sh
ssh -t -F /mnt/mambo/vmstore/cutex-linux-acceptance/ssh/config cutex-linux-acceptance \
 'python3 -B /home/cutex-linux-test/acceptance-upload/pv2/fixtures/fixed_human_entry.py cleanup'
```

输入 CLEAN，仅对出生 tick/exe/进程组匹配的 pv2 自有进程发停止，超时不强杀；
成功后删除新 guest auth、保留历史。不会清理旧 fixture 或同步认证回宿主机。
准备不产生付费 turn、Job、审批或 held retry。未知 pj07 重启竞态风险继续保留；
本任务不重现它，不宣称 Windows/完整生产沙箱/发布或本次真实 provider闭环通过。

## 实际准备记录

- 源码起点3fb0c1412f36c01b771d107db046c7f8ae378942 / tree e21475c760ce7633dad0fc6b4eb3682e1c02f187；本次仅三个夹具/文档文件，无产品修改或构建。
- DEFAULT Cutex00a7998a933d17132aacbf9a8fe798f80966e3adcf529798e42d4304a8631396；facade4c3936b4eb6a7cda0203fbe31c7c62c4c81a639a979c1da1f07e5d14b3e69c83。
- 新 CLI52b868441c65cf12370ffbd7dea30d972b801f875b7e3d69b41ce2a0501fd2a9。服务器、host、schema、Job从既有固定 guest 文件复制到新路径（不更改源文件），全部接收端字节按组合清单核对。
- durable cutex.01a0904d-cfcd-7be0-bef5-f72d30dc9c78；native同UUID，generation1，Bus24940 / Management24941。实际 root review/activation/run Ready 与原样重放通过。
- 新 localpolicy精确为 version2 + 此durable recipient，无额外 template（default suppress）；不会自动退回v1。
- 真实当前owner配置发现 cutex 7工具及 cutex_job 4工具，connected/schema非空；readiness检查再次确认原生历史零turn、相同ID/generation/运行PID。
- 真实私有PTY只attach：显示gpt-5.6-terra、没有文本或Enter、正常exit0、无强杀、termios恢复。未发送任何付费turn、Job、审批或heldretry；没有额外显示样例/合卡造景。
- 已知pid/启动tick/exe/进程组保存在guest私有 h1/handoff.json；服务、runtime和观察器经出生身份验证，按要求保留供Human。
- auth从已授权pv1 guest私有源以no-follow句柄及属主/0600检查后复制，未读取或修改host auth。staged-auth已移除，新nativehome auth0600暂留；无同步回去。旧pv1/pi1/hj1不清理。
- 一次准备成功，无本轮失败/重试。复用上一轮已修正的schema/24xxx端口夹具；不重新隐藏旧失败证据。脚本语法及diff检查通过。
- Guest总额10,569,756,672 bytes（<12GiB），可用48,410,689,536 bytes（>30GiB）。Mambo任务根24,058,032,128 bytes（<24GiB），空闲>100GiB。未删旧证据/缓存或构建新大文件。

本记录仅证明就绪和无输入attach；新的真实Job/输出/通知显示结果由Human随后测试。
