# Cutex notification 现状与通用出站方案

2026-09-14。本轮完成现状调查；以下新版出站是待实施方案，没有宣称它已经接通。按用户要求不做键盘厂商适配：Linux/OpenRazer 接收端自行决定灯效。

## 结论

**当前旧通知链路没有启用，也不能靠打开一个开关就认定恢复可用。** 旧服务可以保留作可选 Linux 桌面输出，但不适合作为新版出站的唯一基础。建议事件在 Cutex 服务侧产生，先做好通用出站，再由各接收端处理 toast、声音、键盘灯等。

## 实测状态

- `cutex notify desktop status`：enabled=false，port=24250，token 未设置，health=not running，external_forward 未配置。
- `cutex-desktop-notify.service`：inactive。
- notify_service_url/token/events/各 idle timeout 均未配置。
- 未启用服务、未修改配置、未向真实外部地址发送通知。

注意三种“通知”不同：

| 链路 | 当前情况 |
| --- | --- |
| Task Service 给 Agent 的通知 / Agent Bus 消息 | 属于 agent 协作和持久任务投递，不是给人的外部通知 |
| cute-codex 原生终端提醒 | 当前源码有 OSC 9 / BEL 后端，效果依赖终端；不能当作外部服务收到事件的证据 |
| Cutex 旧 desktop/external notify | 24250 HTTP 桥＋notify-send＋可选外部转发；当前关闭，旧私有事件生产端未接通 |

## 旧实现的问题与证据

1. 旧 `src/notify/launch.rs` 生成 CODEX_NOTIFY_SERVICE_URL/TOKEN 等环境变量，旧 profile launch 路径会调用它。当前 stock/native 路径会清理继承环境，未调用这条旧通知装配；当前 light cute-codex Rust 源码也搜索不到上述私有变量的消费者。单纯恢复变量传递不足以补全事件来源。
2. `src/notify/desktop.rs` 接收 `/api/agent-notify/push`，使用 notify-send 显示 Linux 桌面通知。开启桌面输出时，notify-send 失败会提前返回，连后续外部转发也一起跳过。不同输出端不应互相阻塞。
3. 外部转发失败只打印 warning，但入站仍可返回 200；没有独立的投递状态、重试队列或人工重试入口。
4. `src/http/client.rs::http_post_json_expect_success` 只读一次、最多 64 字节；读失败被变成 0 字节，并且 0 字节也当成功。TCP 拆包、超时和无响应的结果都可能不准确。只支持 HTTP，连接建立没有使用显式 connect_timeout。不能复用它作为“可靠成功回执”。
5. 服务串行 accept/处理请求；桌面命令和外部网络请求位于同一路径。慢输出会拖延后续通知。
6. health 只代表 HTTP 服务响应，不证明桌面输出或外部接收端正常。
7. 旧 payload 是 status/project_name/agent_name/时长等字段，没有稳定的业务 event_id 与明确来源。很难可靠去重或判断这是历史重放、实时完成，还是单纯 idle。

相关源文件：`src/notify/{desktop,launch,service}.rs`、`src/http/client.rs`、`src/launch/env.rs`、`src/cli_app/stock_lifecycle.rs`；原生终端后端在 cute-codex `codex-rs/tui/src/notifications/`。旧 payload 单元测试只验证解析/显示，不能视为真实整条链路验证。

## 新版最小方案

### A. 事件来源放在服务端

优先从 Cutex 已有 app-server runtime event worker 接入实时 turn 完成、失败、需要用户输入/审批事件；Task/Job 的通知另在各自**已提交的状态转换**之后接入。不要通过扫描聊天文本或读取整个历史判断。

第一批建议：

| 事件 | 含义 |
| --- | --- |
| agent.turn_completed | 一个 turn 完成，不代表整个研究/Task 完成 |
| agent.turn_failed | turn 失败 |
| agent.attention_required | 确实存在待用户处理的请求，不从 idle 猜测 |
| task.review_ready | 已提交的任务状态进入待审阅 |
| task.closed | 任务关闭，携带实际终态；关闭不等于成功 |
| job.completed / job.failed | 进程结果已确认，携带退出信息 |

先做 Agent 事件闭环，再接 Task/Job，逐类声明覆盖范围。历史 resume/replay 不自动重新推送旧通知。

### B. 通用事件信封

`schema: cutex/notification/v1`；包含稳定 event_id、type、occurred_at（无可靠原时间时明确用 observed_at）、Agent ID＋名称、可选 project/task/job 标识、severity、短 summary。默认不包含 prompt、完整消息、工具输出、凭据或完整日志。

event_id 用于接收端去重；重试同一事件不换 ID。键盘接收端依据 type/severity/agent 自行映射颜色，Cutex 不输出 Razer/OpenRazer 的硬件命令。

### C. 出站与故障可见性

- 一个 HTTP JSON webhook 先跑通；后续需要其他输出再加适配器。协议需明确 HTTP/HTTPS 支持，不能接受配置后默默不发送。
- 复用可理解的配置入口：URL、可选 token、事件订阅范围。无需证明“真人”、Director 许可或额外 review。
- 事件提交与网络发送分离；失败不能阻塞 Agent，也不能反向改变 Task/Job 业务状态。
- 小型有界持久 outbox；记录 pending/delivered/failed、尝试次数和最近错误。限制容量、保留期限及诊断日志大小，避免再次出现巨量日志。
- 有界超时、退避重试；收到完整有效的 HTTP 2xx 才记 delivered。保证至少一次投递，接收端按 event_id 去重；不承诺 exactly-once。
- 旧桌面 toast 作为独立输出，失败不能阻止 webhook。
- 人工入口建议：status、test、list failed、retry <event-id>；测试事件明确标注 synthetic，不伪造一个 Task 已完成。

### D. 验收

先用本机录制型接收端验证 JSON 与顺序，再测试断网/超时/HTTP 拆包/4xx/5xx/服务重启/重复事件。检查 Agent 照常工作、旧历史不刷屏、Task/Job 事件只在实际提交后产生、失败可查询重试、存储有明确上限。

完成这一层之后，Linux 的 OpenRazer 接收器可以独立接入。当前不安装驱动、不控制键盘、不把厂商 API 加进 Cutex 主服务。
