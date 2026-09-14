# Cutex r32 阶段审核

状态：实现与验证完成后仅切换 CLI；不重启 Bus、Management 或现有 native owners。

## 本阶段内容

- Agents、Sessions、Projects、Tasks：先左右分；左侧筛选在列表上方，右侧对应 Details。左侧最多 130 列，额外宽度给右侧。
- 窄于 115 列：Alt+I 打开整页 Details；Esc 回列表，保留选中项。
- Settings 保留三列、不加筛选。Jobs 仍是明确占位页。
- Alt+1…6 按顶部顺序跳页；左右方向键保留，旧字母跳页键移除。
- Agents / Sessions：Alt+M 创建 managed Agent，Alt+N 创建普通 Session。Projects 的 Alt+N 仍创建 Project。
- `cutex new` 总是显示 profile 选择；输入 q 取消并返回 Cutex。`cutex new aemeath` 直接新建。
- 普通 light 启动改用显式认证文件、模型/配置参数、状态栏数据；`cutex run aemeath` 同步使用此路径。profile 未设置的模型/推理/TUI 继承 native home；不改写 profile 内容。

## 验证

- TUI 277 通过，3 个此前基线失败，2 忽略。基线失败仍涉及两个管理 fixture 与一个未安装 runtime 假设。
- CLI 参数测试 13 通过。
- 实际 PTY：80/120/180/280 列切页及进出 Details；一轮仅一次进入 alternate screen，中途未退出重建。
- 实际 Agents / Sessions Alt+N 进入 profile picker，q 取消后返回原页面。
- 实际新建 aemeath session，无模型消息发送；显示 Bon voyage ! / aemeath / gpt-6-astra，Job MCP 不再握手失败。

## 明确边界

- 普通 Session 没有 managed Agent 身份，Job MCP 要求该身份，因此普通启动禁用该适配器。若希望普通 Session 也能用 Job Service，需要另做身份/服务接入设计；当前不自动登记 Agent。
- 直接运行裸 `cute-codex` 的缺失状态栏标签仍需 native 前端处理；本阶段修的是 Cutex 传入 profile 的启动路径。
- 筛选态 Enter 的行为保持原样。通知出站、历史分隔线的持久时间、真实 Jobs 列表仍未完成。
