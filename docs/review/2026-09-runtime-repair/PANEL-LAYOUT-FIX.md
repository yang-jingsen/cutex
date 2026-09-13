# Managed / Tasks 分栏修复（2026-09-14）

Managed 原先把详情外框限制在 58 列，放大窗口时继续扩大已经无法利用额外空间的列表。Tasks 使用无上限的 62:38 分栏。

两页现在共用分栏计算：正常窗口约 62% 给列表；列表外框最多 130 列（容纳 Managed 所有列上限、边框与选择标记），剩余宽度全部给详情。保留一列间隔，分栏时左侧至少 74、右侧至少 40 列；低于 115 列继续使用原来的单栏切换。隐藏详情时列表仍可占满窗口。Tasks 在该上限仍显示全部列及活动摘要。

验证：TUI 测试串行运行，修改后 270 通过、3 失败、2 忽略；相同环境与命令运行未修改 HEAD，268 通过、相同 3 失败、2 忽略。失败为 new_agent_without_install_explains_runtime_selection、management_commands_use_service_semantics_without_mutating_runtime_identity、management_success_refreshes_the_row_and_survives_a_stale_snapshot。本次两项新增测试验证超宽窗口所有新增空间进入详情，以及 0–500 列的最小尺寸、间隔和窄屏回退。原有 Managed / Tasks 渲染测试通过。命令：`cargo test --offline --locked --bin cutex session_tui -- --test-threads=1 --skip archive_view_is_secret_free_and_offline`。

仅替换 Cutex CLI；Bus / Management 服务、native runtime 与 MCP facade 继续使用现有版本，不为布局修改重启 agent。
