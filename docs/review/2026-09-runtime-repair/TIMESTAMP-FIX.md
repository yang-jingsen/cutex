# 分隔线时间恢复（2026-09-14）

cute-codex light 的普通分隔线和 `Worked for` 分隔线缺少旧版的本地 HH:MM。提交 `411bbc7250cbb4295665a6a5a66c71a954414e9d` 在分隔线对象构造时保存本地时间，普通显示和 raw transcript 共用该值；重绘不会更新时间。保留原有超过一分钟显示耗时的规则及运行指标显示。

本次没有增加历史事件时间协议；重新构造的历史分隔线不保证还原原始事件时间。

验证：59 项针对性测试通过，包含固定时间快照、无耗时/短耗时/长耗时、窄窗口及零宽度、重复重绘、exec flow 和 final separator。快照中的当前时间仅在测试中归一化为 HH:MM。`just fmt`、`git diff --check` 完成；离线锁定依赖的 CLI 构建成功。未重新运行全量 TUI，上一轮全量失败是否既存没有完整前后基线对照，不能仅凭代码区域断言与修改无关。

安装使用新的 release-native-r3 文件，app-server 与 Code Mode host 复制原有未改动产物，MCP facade 继续使用 r21。Job daemon 程序未修改，闲置时更新 launcher 白名单以加入新 CLI 并保留全部旧路径。默认安装与现有 35 个 native 身份使用新的期望清单；不重启 native owner。现有 UI 需要重新进入才能加载新 CLI。
