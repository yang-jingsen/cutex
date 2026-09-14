# Cutex 主页面快捷键顺序

更新：2026-09-14。本轮实施已批准的布局重构；Settings 保留三列。

固定顺序：**选择/打开 → 详情与对象操作 → 切换 Panel → 筛选/刷新 → 状态详情/帮助 → 返回/退出**。页面没有实现的功能不添加假快捷键。

| 页面 | 主页面提示顺序 |
| --- | --- |
| Agents / Sessions | ↑/↓ select · Enter open · Alt+I inspect · Alt+A actions · Alt+E edit · Alt+M new agent · Alt+N new session · ←/→ panels · / filter · F5 refresh · F2 details · F1 commands · Esc back |
| Projects | ↑/↓ select · Enter open · Alt+I inspect · Alt+N create · ←/→ panels · / filter · F5 refresh · F2 details · F1 commands · Esc agents |
| Tasks | ↑/↓ select · Enter/Alt+I inspect · ←/→ panels · / filter · Ctrl+A history · F5 refresh · Esc back |
| Jobs（占位） | ←/→ panels · Alt+6 settings · Esc agents · Ctrl+C exit |
| Settings | ↑/↓ select · Enter open · Tab focus · V view · S save · D discard · ←/→ panels · F2 details · F1 commands · Esc back · Ctrl+C exit |

Settings 不足 66 列时省略部分辅助提示，保留选择、打开、切页、帮助和退出。只读内容不显示 Save/Discard。

方向键规则：

- 全局 Settings 浏览态：Left 到 Jobs；Right 已位于最右端，保持当前页面。Enter 打开子项，Tab/Shift+Tab 切换内部焦点。
- 有未保存内容时，离开仍走已有 Keep editing / Discard and leave / Save，不绕过保存逻辑。
- 文本编辑、确认弹窗、Inspector 和项目工作区内部保留各自的光标/选择/内容导航。此时底部显示本地动作，不能误标为 panels。
- Agent 自身的设置页保留内部导航，不等同于全局 Settings Panel。
- 详情滚动提示先显示滚动/翻页，再显示刷新与关闭。

## 同次验证发现的退出问题

实际 PTY 从多个 Panel 切到 Settings 时复现退出 101：`session_tui.rs` 的 `sort_rows` 对 `lifecycle` 调用 `expect("agent lifecycle")`。初始快照未完成、Agent Bus 查询失败时，投影会合法地将 lifecycle 设置为 None，因此这一假设不成立。

修复：已观测状态仍按原顺序排，未知状态排在其后、系统入口之前；保留未知含义，不伪造 Offline。新增混合已知/未知/系统行排序回归。此项与键位修复共同进入 r31，不能仅归因于用户操作或 PyCharm。

## 页面与创建入口

- Alt+1 / 2 / 3 / 4 / 5 / 6：Agents / Sessions / Projects / Tasks / Jobs / Settings。旧字母跳页键移除；数字直达放在 F1 帮助，底栏保留左右切页提示。
- Agents、Sessions：Alt+M 新建 managed Agent；Alt+N 新建普通 Session。普通 Session 先选择 profile，退出后回到 Cutex；不会自动登记为 Agent。
- Projects：Alt+N 仍创建 Project。
- `cutex new` 选择 profile；`cutex new aemeath` 直接以该 profile 新建。`cutex run aemeath -- …` 保留传递 CLI 参数的入口。
- 窄窗口底栏省略部分辅助快捷键，但保留 F1；完整动作仍可在 F1 找到。
- 新建 managed Agent 内部自动创建空 native thread 再 adopt，不要求用户手动先 adopt。
