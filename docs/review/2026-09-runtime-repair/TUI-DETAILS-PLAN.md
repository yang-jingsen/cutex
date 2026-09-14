# Cutex 列表与 Details 统一方案

状态：2026-09-14 用户已批准执行；Settings 明确保留当前三列，不加筛选。布局已实现并通过终端验证，本阶段为 r32 CLI 更新。

## 1. 已对齐的结构

采用你指定的顺序：**先左右分，左侧再分上筛选、下列表；右侧是选中对象的 Details。**

```text
CUTEX  Agents  Sessions  Projects  Tasks  Jobs  Settings
Cutex Agents  …
┌ 左侧 Filter ──────────┐ ┌ 对应 Details ─────────────────┐
│ …                     │ │ 对象名称、状态、更新时间       │
└───────────────────────┘ │                               │
┌ 左侧 List ────────────┐ │ 概览                         │
│ > 当前对象            │ │ 近期活动 / 下一步             │
│   …                   │ │ 相关对象                     │
│                       │ │ 运行配置 / 技术标识            │
└───────────────────────┘ └───────────────────────────────┘
Ready / 当前状态
两行上下文快捷键（F2 只出现一次）
```

- 筛选只控制左侧列表，右侧始终说明当前选中项；选中项被筛掉时切到新选中项，没有结果时显示空状态。
- 两栏从同一高度开始，到同一高度结束，筛选框不横跨 Details。
- 先沿用目前 ≥115 列拆分的阈值；列表最多 130 列，超过之后新增宽度全部给 Details。不同实体可有更窄的列表目标宽度，不强求都占到 130 列。
- 小于阈值时只显示左侧；Alt+I 打开整页 Details，Esc 回列表，并保留列表选中项和滚动位置。已有实现可复用。
- Settings 按用户最新要求保留当前三列，不增加筛选，不纳入本轮布局重排。
- Jobs 目前没有数据接入，本轮仅在方案中预留；不把占位页改成看似“查询成功、没有 Job”的假空列表。

## 2. 现状与每页改动

| 页面 | 当前实现 | 建议 |
| --- | --- | --- |
| Agents | 已是左侧筛选＋列表、右侧 Inspector；详情混合 Overview/Actions/Settings | 保留几何结构，重排 Details 内容与操作入口 |
| Sessions | 列表；Alt+I 已能打开整页详情，且复用 Agent 的部分字段与标题 | 增加常驻右侧 Session Details；原生会话与 Agent 字段分开 |
| Projects | 列表；Enter 后进入 Members/Overview/Operators/Appearance 页面；成员也有独立详情 | 项目列表右侧增加项目摘要；Enter 仍进入完整项目工作区，不将完整成员表塞进小栏 |
| Tasks | 顶部筛选横跨左右，下方才拆列表/Inspector | 改成统一结构；区分任务、assignment、attempt，先显示结果与阻塞原因 |
| Jobs | 未接入列表 | 接入之后复用统一结构；第一版不同时施工 Job 管理操作 |
| Settings | 分类/选项/当前值三列 | 用户明确保留，不加筛选；本轮不改布局 |

## 3. Details 第一屏放什么

**共同规则：先回答“这是谁、现在怎样、下一步是什么”，再显示配置和长 ID。**

顶部使用对象名称和一行状态；右侧栏不重复展示多层 Inspector/Overview/Agent 标题。信息由短分组构成，每组少量字段；仅在聚焦时强调边框。错误和缺失数据要区分，未加载不能写成“没有”。

### Agents

1. 名称、项目/角色、Online/Offline、最近活动及实际时间。
2. 正在执行什么；存在明确等待项时展示等待原因。没有来源就不推测。
3. 当前运行 profile/model 与下次启动配置分开；不同才突出差异。
4. cwd、host、最近运行实例；长 ID 放到 Technical 区域，可展开或复制。

Actions 和 Edit 不作为与 Overview 平级的大段“信息内容”混排。保留 Alt+A、Alt+E 打开对应操作/编辑页；Details 首屏给一个简短操作提示即可。

### Sessions

1. 原生标题、最近更新时间、cwd。
2. 是否关联 Cutex Agent；已关联时显示 Agent 名称和项目，未关联时明确标注原生会话。
3. provider/model 等已有元数据；会话 ID 放到技术信息。
4. 若已有摘要则展示短预览；**不为选中行预览读取整份历史，不触发历史分页或 resume**。

当前复用 Agent Inspector 的方式容易出现对原生会话无意义的 profile/runtime 字段，应该拆分展示模型。

### Projects

1. 项目名称、badge、Director 名称、成员数。
2. 有可靠现成投影时再显示在线成员/活动 Task 汇总；没有就不临时扫描所有会话。
3. 进入 Members、管理项目等动作入口。
4. project_id、authority epoch 等排到技术信息；Operators/Appearance 留在完整工作区。

项目摘要不要每移动一行就同步调用 API。优先使用列表已有投影，详细数据异步加载并缓存，旧选中项的迟到响应不能覆盖新项。

### Tasks

1. 可读任务名、状态、最近一次真实更新时间。
2. 当前负责人和 attempt；阻塞、等待审阅、失败时优先展示原因。
3. 结果摘要和结果位置；其次才是 activity、last output/tool。
4. assignment/revision/attempt IDs 放到技术信息。

Agent 的“最近全局活动”不一定属于这项 Task，必须保留“Agent 最近观测活动”的明确标签；不能当作 Task 进度。当前 Task 页面实际上按 assignment 投影，展示和选择身份需要保留这一点。

### Jobs（以后接入）

名称、状态/退出码、开始/完成时间、所属 Agent/Task、简短命令和 cwd、stdout/stderr 路径。日志预览按需有界读取；不要把完整日志放入每次刷新或默认 Details。

## 4. 交互

- ↑/↓：左侧选中项；Details 随选择更新。
- Alt+I：聚焦 Details；窄窗口切整页。Esc：回列表。
- Details 聚焦后 ↑/↓、PgUp/PgDn、Home/End：只滚动详情，不偷偷改变列表选中项。
- Alt+A：对象动作；Alt+E：编辑；F2：状态/失败/确认的详细说明，**不代替对象 Details**。
- ←/→：列表焦点下切顶部页面；详情的内容切换不用同一对键再次嵌套。
- 选中背景 #26344F；profile 等字段前景色不因行选中改变。当前页名粗体白字，非当前页灰色。
- 页面内有编辑草稿时保留现有保存/放弃行为；普通查询、聚焦和切栏不增加授权步骤。

### Enter：现状与建议

当前没有“双击”检测。筛选焦点下第一次 Enter 只退出筛选，第二次 Enter 才执行列表主动作；在线 Agent 通常 attach，离线或要求确认的动作会进入确认页。Details 中 Enter 当前不启动 Agent。

建议下一轮让 **筛选态 Enter 一次完成筛选并执行当前选中项的主动作**；Esc/Tab 只结束筛选。在线项直接进入，已有离线启动确认单独保留。本建议尚未实施，这一轮只补全提示，不悄悄改变 Enter 的行为。

## 5. 实施顺序与验收

1. 抽取共享左右布局与 Details 外框；先迁移 Tasks 和 Sessions，再加 Projects 摘要。
2. 统一焦点、Alt+I、Esc 回退及选中项保留；Settings 保持现状。
3. 重排 Agents/Tasks 内容，最后处理按需加载与跨对象跳转。Jobs 在真实数据接入时跟上。
4. 用 80/120/180/280 列实际终端检查：无额外切换 alternate screen；两栏边界一致；筛选只占左栏；快速移动/刷新/resize 不显示旧对象；窄屏能进出 Details；首屏可看懂，无满屏 ID；不因渲染触发模型、历史解析或管理写入。

本轮先修的独立缺陷：Tasks 重建终端导致的闪屏来源、Sessions 重复绘制导致的标题残色、各页标题前缀着色、Details 快捷键提示、profile 选中变色。大范围布局按本方案审阅后再做。

## 6. 状态与时间补充（后续消息合并）

- RECENCY 按运行 Cutex 的主机本地时区显示 `MM-DD HH:MM`；底层 epoch 不变。SSH 客户端的时区不会自动传入服务器。
- PROJECT 查询不到显示 `N/A`；确认没有项目仍用 `-`。技术详情保留具体原因。
- 所有底部说明文字统一白色，快捷键主题蓝。
- Agent 生命周期只有 Online / Stale / Offline：分别用 #E08EB2 / #74BAC3 / 灰色；未知或不可用用 #D9B45F。选中行不改变语义色。
- Sessions 还显示 managed、unmanaged（中性文字）、retired（灰），cwd unavailable、ambiguous mapping（金）。这些是关联状态，不是运行状态，不改名伪装成 Online。
- Task 状态还有 queued、assigned（蓝）、running（当前保留绿）、review（粉）、blocked（金）、closed（灰）。本轮未把它们一概套成 Agent 的三状态；颜色引用也集中在主题模块，后续可以单独调整。

## 7. Cancel / Discard / Save 确认流程

已找到共用的 LeaveReview：原本清空整屏，只显示 Cancel / Discard and leave / Save 几行。创建 Project 的离开确认与设置页离开确认都走它；本轮先将它改为居中小弹窗，背景页面保留。

- 标题 Unsaved changes；提示是否带着未保存修改离开。
- Keep editing：取消本次离开，保留草稿；Esc 同义，避免单独 Cancel 容易被理解成“取消创建”。
- Discard and leave：丢弃本地草稿再执行原来的离开动作。
- Save：只有调用方确实支持保存才出现；沿用当前保存后处理，失败仍留在编辑页。**不承诺 Save 会自动完成原离开动作**；“保存成功后离开”需要单独梳理异步保存完成事件，再决定是否统一。
- 默认继续编辑；可用左右/Tab 选择，Enter 确认。不要一按 Enter 就丢弃草稿。

后续排查清单：Project Create/Appearance、Global/Agent settings、Profile 编辑/重命名/移除、Adopt/启动等业务确认。前四类统一草稿离开提示；涉及实际业务写入的确认保留明确对象、动作和失败结果，不把它们都叫 Cancel。已有独立居中确认框先保持，避免同时改变其保存语义。

## 8. 列表边框偶发错位

用户观察：Agents 和 Projects 的 List 内第三行右侧边框经常比其他行靠左，偶尔正常。此项独立于布局方案，按渲染缺陷调查。

当前证据：180 列 PTY 捕获中，Agents 左列表所有数据行右边框均在第 110 列，Projects 在第 180 列；这只能说明该次捕获正常，不能排除真实终端的间歇问题。后续覆盖窗口缩放、列表更新及含宽字符内容，区分实际光标位置错误与字体/终端显示宽度差异。未定位前不通过随意补空格处理。

补充观察：用户明确对应 Agents 的 scpolya-2、Projects 的 IFM（均第三条数据）；往往刚切入出现，移动选中项到该行即恢复。优先排查切入帧和增量绘制，暂不归因于名称长度。

追加验证：`capture-tui-borders.py` 在同一 PTY 依次切换两页，窗口宽度 180→100→140→80→180，每个尺寸捕获切入和三次向下移动；保存 32 份最终尺寸/状态组合，数据行边框坐标均一致。该模拟终端结果未复现用户终端现象；终端种类及“切入”具体路径待确认。当前没有为此加入整屏 clear 或强制全量重绘，避免未经验证重新引入闪屏。

环境已确认：PyCharm Terminal，切换顶部 Panel（不是从 cute-codex 返回）。Agents 第三条 scpolya-2 的 Online 比其他行靠前约 3 个字符；Projects 第三条 IFM 从 Director 第二列起左移约 5 个字符，边框随之后移位置错误。应检查第二列之前的输出定位及切页差分，不能仅修右边框。两个名称均为 ASCII，当前没有支持“名称含宽字符”归因的证据。

JetBrains 解析验证：从 Cutex 捕获首次进入 Agents/Projects 和往返切页的原始 ANSI 输出，用本机 IntelliJ 安装自带的 `intellij.libraries.jediterm.core.jar` 在无 GUI 模式回放。四次结果均对齐：180 列时 Agents 各行 Online 为零基列 49、左列表右边框 109；Projects IFM 的 Director 为 115、右边框 179。此验证覆盖该版本 JediTerm 的逻辑缓冲区，不覆盖用户 PyCharm 的版本、终端引擎及实际字体绘制；问题仍未修复。回放程序与原始捕获仅保留本地，不上传运行内容。

用户截图（2026-09-14 12:49:56 / 12:50:12）确认：Agents scpolya-2 名称左端对齐，从 Online 起后续列和边框一起左移约 3 格；Projects IFM 名称左端对齐，从 Director 起后续列和边框一起左移约 5 格。截图保留本地 Downloads。环境为 PyCharm 2026.2.2 / Reworked 2025；旧 JediTerm 回放不能替代该引擎的现场验证。已提供 `record-cutex-tui.py`，输出及 timing/尺寸/必要元数据，本地 32 MiB 限额、无 stdin 记录；启动、两页切换、退出 smoke test 通过。等待现场 ANSI 记录与截图对照，不把旧模拟器未复现当作问题不存在。

根因已由用户现场记录复现（`tui-diagnostics/20260914-125152-666920`）：96×30，约 5.007 秒显示 Sessions，第三行标题含中文；5.487 秒切回 Agents，JediTerm 缓冲区在 scpolya-2 右边留下 3 个 U+E000 宽字符尾格标记；11.299 秒第一次下移仍保留；12.368 秒第二次下移选中该行后消失。原先只检查边框逻辑坐标漏掉了这些显示宽度为零的残留标记。

修复方案：当前 Ratatui 0.30/ratatui-core 0.1.2 对默认背景宽字符变窄时可省略尾格；Cutex 共享 Terminal 在渲染后仅为上一帧的宽字符尾格、且当前不是新宽字符延续的位置标记 AlwaysUpdate。所有 Panel 共用，不依赖 PyCharm 环境变量，不改名称列宽，不清屏、不每帧全量输出。新增中文→ASCII 空格尾格回归，以及中文不变/移动不破坏新宽字符回归。

验证完成：旧 r29 与修复版在 96 列实际 PTY 中执行 Sessions→Agents，旧版第三行留下 3 个占位标记，修复版 0 个。中文标题→IFM 的合成差分回归确认旧版漏 5 个尾格、修复版全部输出；实际 Projects 对照这一次未复现旧版偏移，不扩大这一项的现场证据。完整 TUI 272 通过 / 原有 3 个基线失败 / 2 忽略；最终两项新回归通过。当前准备 r30 优化构建，待用户原终端确认视觉恢复。

## 本轮实现边界

四个列表页共用布局和 Details 外框；Sessions、Projects 摘要只用已有列表数据，不因选中行重新读完整历史或请求管理写操作。80/120/180/280 列均做实际 PTY 检查。Settings 保留三列，Jobs 保留明确占位。筛选态 Enter 的行为没有改变；更丰富的 Agent 业务摘要和 Jobs 列表仍属后续工作。
