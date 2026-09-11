# 项目内近14天迁移候选：只读选择快照

固定参考时刻 **2026-09-11 18:48:15 UTC = 2026-09-12 04:48:15 Australia/Sydney**。
下界包含 **2026-08-28 18:48:15 UTC = 2026-08-29 04:48:15 AEST**。
在线状态/项目归属是本次实际读取时的状态，不伪称参考时刻的历史状态。
未执行迁移、停机、复制历史/认证、备份或运行候选历史。

## 结论与选择规则

- **34 个候选：18 在线¹、16 离线**。不是把先前21个在线全选。
- **46 个近期有实际记录但已退休的历史项目成员**单列在JSON，不自动复活。
- **20 个项目关联记录无窗口内可确认实际活动**，排除；**5 个历史解析异常**，未知。
- 管理存储105个成员（含已退休历史归属）均已检查；392个durable中另外287个
  不在此正式成员集合，未按名字/cwd/分组推断项目、未扫描其历史。
- 原生目录按精确native ID读取，105个绑定都能定位；没有标题匹配、mtime、
  last_seen_at、目录更新时间或心跳日期推断。固定上界后的记录不改变选择。
- 只取外层timestamp的response_item实际user/assistant消息或工具调用/结果。
  UI event_msg（包括状态/watchdog通知）、compacted、reasoning和纯通信metadata
  不独立使成员符合条件。外部通信metadata另存锚点，不从正文猜真实Human。
  不宣称仅凭metadata已重新认证它的来源；没有导出对话正文/工具参数/输出。
- 历史读取固定文件前缀、忽略尚未完成的末行；完整行解析失败即未知，不像旧
  loader那样静默跳过后宣布兼容。JSON解析仅访问结构/时间/角色字段；正文不保留。

机器可读身份、精确rollout路径/行号/ordinal/字节偏移、项目和活动/未知锚点：
[project-recent14-candidates.json](project-recent14-candidates.json)。
重现脚本：[project_recent14_inventory.py](../scripts/project_recent14_inventory.py)；
只读数据库mode=ro/query_only，原文件不变。固定时间重跑时当前成员状态可能变化，
所以本次冻结JSON是此选择结果，不是持续生命周期台账。

## 候选（日期均AEST，UTC+10）

| 正式名称 | 项目显示名 | 当前状态 | 最后实际记录本地时间 | 停机协调 |
| --- | --- | --- | --- | --- |
| cesc-attribution-integration-r2 | CellScript | 在线¹ | 2026-09-09 15:23:00 |  |
| cesc-attribution-r2 | CellScript | 在线¹ | 2026-09-09 15:01:29 |  |
| cesc-deep-direction-r1 | CellScript | 离线 | 2026-09-07 08:30:44 |  |
| cesc-director-r2 | CellScript | 在线¹ | 2026-09-11 20:32:27 | 项目 Director； |
| cesc-gpu-training-r2 | CellScript | 在线¹ | 2026-09-11 07:15:30 |  |
| cesc-literature-brief-r1 | CellScript | 离线 | 2026-09-01 15:27:43 |  |
| cesc-shared-projection-r2 | CellScript | 在线¹ | 2026-09-09 15:38:19 |  |
| cesc-signal-audit-r1 | CellScript | 离线 | 2026-09-01 19:18:38 |  |
| cesc-status-review-r2 | CellScript | 在线¹ | 2026-09-09 23:13:11 |  |
| cesc-target-first-r2 | CellScript | 在线¹ | 2026-09-09 15:13:21 |  |
| cesc-tutor-r1 | CellScript | 离线 | 2026-09-05 22:06:39 |  |
| cute-codex-0153-upgrade-r1 | Cutex | 离线 | 2026-09-04 20:43:46 |  |
| cute-codex-cutex-source-r1 | Cutex | 离线 | 2026-09-08 04:48:37 |  |
| cute-codex-kernel-reconcile-r1 | Cutex | 在线¹ | 2026-09-08 07:52:15 |  |
| cute-codex-light-core-r1 | Cutex | 在线¹ | 2026-09-12 04:47:36 | 未关闭 Task； |
| cute-codex-log-wal-fix-r2 | Cutex | 离线 | 2026-09-06 14:36:35 |  |
| cute-codex-state-compat-r1 | Cutex | 离线 | 2026-09-07 20:16:47 |  |
| cutex-agent-source-consumer-r1 | Cutex | 离线 | 2026-09-08 04:57:39 |  |
| cutex-director-r12 | Cutex | 在线¹ | 2026-09-09 08:12:16 |  |
| cutex-director-r13 | Cutex | 在线¹ | 2026-09-12 04:48:14 | 项目 Director； |
| cutex-durable-import-r1 | Cutex | 在线¹ | 2026-09-12 04:46:02 | 未关闭 Task；当前执行者； |
| cutex-durable-import-review-r1 | Cutex | 在线¹ | 2026-09-12 02:03:03 | 未关闭 Task； |
| cutex-mcp-android-research-r1 | Cutex | 在线¹ | 2026-09-09 18:43:35 |  |
| cutex-r37-release-deploy-r1 | Cutex | 离线 | 2026-09-06 23:52:40 |  |
| cutex-r40-source-release-r1 | Cutex | 离线 | 2026-09-08 05:00:37 |  |
| cutex-r40-tethys-deploy-r1 | Cutex | 离线 | 2026-09-08 05:46:34 |  |
| cutex-r41-release-deploy-r1 | Cutex | 在线¹ | 2026-09-08 08:46:34 |  |
| ifm-director-r4 | IFM | 离线 | 2026-09-04 17:13:49 | 项目 Director； |
| ifm-director-r5 | IFM | 在线¹ | 2026-09-07 14:47:01 |  |
| ifm-ema-figures | IFM | 在线¹ | 2026-09-02 15:40:10 |  |
| scpolya-2 | ScPolyA | 在线¹ | 2026-09-09 16:01:01 | 项目 Director； |
| tethys-classifier-eva-r1 | TethysUNE | 离线 | 2026-08-30 13:21:47 |  |
| tethys-director-r2 | TethysUNE | 离线 | 2026-08-30 13:21:25 | 项目 Director； |
| vec-submission-exporter-r1 | VEC-2026 | 离线 | 2026-09-01 16:26:12 |  |

¹ PID/exe存在的只读观察，不是发起online或健康探测。Director标记来自正式项目
authority指针，**不是名称带director就猜角色**（例如ifm-director-r4与r5）。
未关闭Task标记来自当前Task provider assignment状态，包含历史blocked/awaiting_ack，
不是宣称它们正在执行模型turn。cutex-director-r13和本执行者最后安排停机；
各项目Director必须通过有权限的协调方安排，不对其他项目发送生命周期请求。
16个离线候选迁移后仍应离线，不能为验证而自动拉起。

## 未知与退休边界

| 正式名称 | 项目 | 生命周期 | 原因 |
| --- | --- | --- | --- |
| cesc-current-direction-r1 | CellScript | 未退休 | 19 条无法解析的完整 JSONL 记录 |
| cesc-director-r1 | CellScript | 未退休 | 24 条无法解析的完整 JSONL 记录 |
| cutex-director-r11 | Cutex | 已退休 | 1 条无法解析的完整 JSONL 记录 |
| vcc-director-r1 | VCC-2026 | 未退休 | 1 条无法解析的完整 JSONL 记录 |
| vce-director-r1 | VEC-2026 | 未退休 | 1 条无法解析的完整 JSONL 记录 |

异常行号/偏移见JSON（例如不是纯末尾并发半行）。不修复原历史、不根据其余
有效行把未知悄悄加入候选。46个qualifying_retired_or_archived记录此次实际均为
retired；独立分类保留正式名称/历史项目/最后实际日期，迁移计划不得恢复它们。
归属逻辑复用Management current_project_id：显式membership override优先，
未退休默认managed.project_id；退休仅保留历史来源，不能当当前可操作成员。

## 保留历史转换：已知与未证明

候选结构实际分布：30 paginated、4 legacy；32 cli、2 exec，所选34个没有
parent_thread_id/forked_from或session_id≠thread ID。所有绑定共享现有权威
codex-home目录数据库。不能凭这点另造每profile一份home/历史。

已观察记录族：session_meta、response_item、event_msg、world_state、
turn_context、token_usage_record、inter_agent_communication_metadata、compacted。
其中所选完整文件累计301个compacted、480个world_state记录；这些不是可以
随意扔掉的显示日志。保留main/root身份、压缩replacement history/rollback、
工具结果与memory mode/相关数据库映射，需要实际新解析器的副本验证。

精确K ef53716 history/src/rollout_payload.rs与新server b8e9cc同文件均声明上述
顶层记录族；新协议也有ThreadSettingsApplied、SubAgentActivity、ItemCompleted、
AgentMessage、ThreadRolledBack。**共同枚举名称只是源码相容线索，不是同历史resume证明**：
ItemCompleted内层封闭类型、工具名/参数、压缩内容、记忆和目录schema可能影响重建；
本次没有执行history解析器、thread/resume或模型，不虚构它们已通过。已接受jv04
证明的是新建中性owner后的Job显示/重连，不证明这34份旧K历史转换。

明确配置阻碍（不是推测）：

- 27个显式aemeath、4个继承当前默认aemeath；2个octobre
  （cute-codex-log-wal-fix-r2、cutex-r37-release-deploy-r1）和1个GLM
  （tethys-director-r2）。
- 当前产品src/launch/stock.rs:430起只支持Fake/AemeathChatgptV1 reviewed模式；
  没有octobre/GLM真实认证路线，不能改成aemeath或fake来算迁移成功。
- 三个当前profile都未声明新reviewed mode，且有allowlist外字段。
  aemeath有mcp_servers/skills/tui/projects/service_tier等；GLM另有memories、
  plugins/model_catalog_json等；octobre也有skills/tui等。完整配置原样套用会拒绝。
  必须明确投影哪些设置、保留哪些原始设置，不能静默丢弃记忆/工具/权限或改全局profile。
- stable durable/native/project/profile/permissions必须保留；既有活跃Task保护、
  offline/no-claim和root review/CAS仍适用。当前host没有explicit marker，
  不表示可以绕过逐记录激活。激活后的marker换bundle仍无通用替换API。
- 最终灰色CLI5fc4719/042ba655、SHA1a51163f已接受为包输入；server7bc7f3d7/
  Uhost3e85/schema c2a54d59不变。当前Cutex productb48a还需另任务更新精确CLI pin，
  本次未编译或尝试混搭。

## 最小下一步（需另行授权，不在本次执行）

1. 冻结最终精确包后，做一个**私有复制历史、无认证/无模型**兼容性验证任务：
   从这34个精确目录锚点复制必要目录DB/rollout及其引用，先对全部34个执行新
   typed decoder和history重建，检查parse-error=0、原native ID、最后记录及
   compaction/rollback引用一致；异常5个另外诊断，不原地修改。
   按实际结构选最小paginated+compacted/legacy/IAC/记忆代表，隔离新owner
   thread/read/resume无turn验证。不得从新的中性线程测试外推旧历史。
2. 同任务核对profile/权限投影；aemeath仅在显式接受投影后走已有review模式。
   octobre/GLM应明确列阻碍并由Director选择受控适配或延期，不能暗自换配置。
3. 发布窗口重查候选/Task/角色，执行**既定唯一一次备份**（约定root目前不存在，
   本任务未捕获）。停止并核验选中owner身份，保留未选中对象；记录每个原在线/
   离线状态与身份。按已验证的保留历史路径转换/激活，离线保持离线；
   普通worker先、项目协调者后、当前Director/执行者最后交接。
4. 中央Cutex/PRH Job更新仍按上份发布盘点的CAS/精确daemon-adapter配对办理。
   不创建第二备份，不用旧writer打开Bus6/Job2/新native历史回滚。
   停机跨项目权限由Human/相应正式协调方授权，清单本身不授予生命周期权限。

本次无需新功能大审计或pj07复现。结果是选择快照+确定的配置缺口，
不是迁移成功、支持所有旧历史或部署许可。pv3/VM/认证/服务均未触碰。

