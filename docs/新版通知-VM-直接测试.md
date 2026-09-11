# 新版通知 VM 测试（pv1）

仅私有测试：DEFAULT Cutex 5414db97 / native 3d8a73a7 / Job f3bc9c8。
不是旧 pi1/hj1，也不是带故障注入的 feature 二进制。旧会话和历史保留。
准备完成，已 Ready；下面的命令可以直接连接。

## 进入

```sh
ssh -t -F /mnt/mambo/vmstore/cutex-linux-acceptance/ssh/config cutex-linux-acceptance \
 'python3 -B /home/cutex-linux-test/acceptance-upload/pv1/fixtures/fixed_human_entry.py attach'
```

入口只连接已准备的同一线程，不启动服务。模型应为 gpt-5.6-terra / low，
接收端 read-only / on-request。不要为消除警告改变 Trust 或提升权限。

## 一次 Job 测试

```text
使用 tool_search/CodeMode 查找并调用 tools.mcp__cutex_job__submit 一次：
actionId: human-presentation-job-1
argv: ["/bin/sh", "-c", "cat probe-readable; printf visible-job-output"]
cwd: /home/cutex-linux-test/acceptance-upload/pv1/h1
不要直接调用 shell/exec_command，不轮询、不重复提交、不读取其他文件。
提交后简短回复 Submitted。正常完成通知到达后，只调用一次该 Job 的
read_output 读取 stdout，报告结果。出错停止，不释放或重试 held 输入。
```

只批准指定 Job 和 stdout 读取。预期输出 private-read-success 换行接
visible-job-output。另观察完成通知的来源、标题和正文；显示补充与模型输入
有明确关联但不是合并成一张卡。中性 Notice 不等于成功；exited 不等于 exit0；
输入 A4、Job 执行、stdout 读取、显示回执是不同事实。

完成后退出 CLI，再用同一命令连接，检查通知是否回放，不重复提交 Job。
可见条目本身不应启动模型；本次 Job 通知另有模型输入，因此会触发续行。
约两分钟无结果或报错就停止发消息，告诉 Worker 当前步骤及简短错误。

## 错误与结束

脱敏错误仅保存在 guest 本地 pv1/h1/human-errors.jsonl；不要贴完整配置、
auth、请求头或未审核对话。willRetry=true 是中间错误，不是终态成功。
退出 CLI 不停止后台 owner。确认测试结束后再运行精确清理：

```sh
ssh -t -F /mnt/mambo/vmstore/cutex-linux-acceptance/ssh/config cutex-linux-acceptance \
 'python3 -B /home/cutex-linux-test/acceptance-upload/pv1/fixtures/fixed_human_entry.py cleanup'
```

输入 CLEAN 只清理出生身份匹配的 pv1 进程和临时认证，保留历史；失败不强杀。
不会清理旧 pi1/hj1 或宿主机认证。此前一次重启竞态超时仍未解释，长历史
O(history)、生产发布及 Windows 不在此次验收范围。

## 准备证据与范围

固定默认字节：Cutex 03025fcd450cf1edb911cbfc937e23154f86523fa19f0f2171a4909a03bdc969；
facade 54f705eace59ab51dd75566561544e6b499c058103559b56a23ca7082c30856b。
native CLI d97f8d4f32377b22066d25dd545b79b32bb9cacb8b501f9db9a21c290504ad60；
server df95936f3f0d1ff62efb978fee32b85e45da0f7f3166de7606cce3b49fb12018。
host 3e85d67471825f73d02ff5f7e047ca1f6ca8caa3f59e4c6e8d9ca6ca7302cb45；
schema 77e75b7fc47c9b8a9caacf5a7d31c040c6520679a9833f1c2b0e55937ed39c27。
Job d98d5e33322c7200eb9b149bc39d99da3bb169c994fe14c7f401d26c06d7b1f2。
所有传输字节在 guest 校验；没有重新构建或使用 feature 二进制。

保留的准备失败：首次脚本仍校验旧 schema，在任何原生创建前停止，目录保存在
pv1/preflight-schema-failure-h1。修正后创建了一个中性线程，但测试端口25000超出
Bridgeboard 24xxx 范围，Bus 拒绝启动；日志保存在 h1/bus-port-preflight-failure.log。
修正端口后从原有 generation0、无 claim/marker/owner 的精确记录继续，不重复 thread/start。
这些是夹具错误，不是此次重启竞态的复现，也未绕过任何产品校验。

认证从已有授权 pi1 guest 私有 auth 以 no-follow 打开并核验属主/0600后，复制到
新的 pv1 私有位置；原文件和宿主机认证未修改。临时 staging 由准备流程移除；
成功交接后保留新 native home 的 auth 供 Human 测试，不自动清理或同步回去。
Director 已允许 guest 总额9GiB（原8GiB），保留全部旧证据；要求可用空间至少30GiB。

实际交接：durable cutex.01a08f57-2390-7ad0-be56-34e6e120d5e9，native 同 UUID，
generation1；Bus24920 / Management24921。出生身份核验 runtime、三服务、观察器存活。
实际默认字节 root review/activation/run Ready，原请求重放一致；真实原生 MCP
connected，submit/query/read_output/cancel，submit/read_output schema 非空。
原生历史零 turn，没有自动模型请求/Job/审批。真实私有 PTY 同 owner attach 显示
gpt-5.6-terra；未发送文本或 Enter，正常退出0、无强杀、termios恢复。
未额外注入 display-only 通知；零唤醒的独立显示已有 pj03 证据，本次 Human 实测
实际 Job 输入和显示补充以及 resume 回放。尚不声称本次付费 provider/Job闭环通过。

guest 总额8,975,036,416 bytes（<9GiB），可用50,005,549,056 bytes（>30GiB）。
新 auth0600按要求保留，staged-auth已删除；没有清理旧fixture、历史或运行时。
脚本/文档修改不改变产品二进制；本页和 scripts/presentation_human_prepare.py 为复现入口。
