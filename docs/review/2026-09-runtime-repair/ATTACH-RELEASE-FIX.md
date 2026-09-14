# Cutex attach 校验与 release 构建（2026-09-14）

进入 cute-codex 之前的延迟是独立于 legacy 时间线读取的问题。原部署使用 debug Cutex，诊断 attach 进程占满单核并持有 app-server 产物读句柄，45 秒仍未创建 CLI 子进程。attach 先 StockBundle::load，随后 verify_stock_process 再加载和哈希同一套约 1 GB 产物。

提交 `8fbd40e57062ed352d8c28317f42dec5cf75817b` 使用已经验证的 bundle 执行进程身份检查，不重复计算文件哈希。schema、PID 出生时间、进程组及 endpoint 所有权检查保留。stock_lifecycle 定向 4 项测试通过；cargo build --release --offline --locked --bin cutex 成功（optimized）。release-runtime-r23 的 CLI、Management API、Agent Bus 均使用该 release 产物。服务切换保留当时全部 5 个 native owner，Agent Bus 心跳继续推进，393 个身份和 Task 数据保留。

PTY 实测，未提交模型输入：

- release 直接 human attach：1.287 秒创建 cute-codex 子进程。初次探针遇到帧间清屏，不能用当时的 resuming_gone 数字衡量完成时间。
- release CLI、旧管理服务的完整 session foreground：11.556 秒创建 CLI，18.100 秒恢复画面；另等 2 秒确认画面稳定。
- release CLI + release 管理服务的完整 session foreground：10.435 秒创建 CLI，14.709 秒恢复画面；另等 2 秒确认画面稳定。后两次以文本清屏后持续稳定的画面判断，属于本机粗粒度端到端观测。

完整 foreground 会先执行 online / 管理端 reconnect，仍比直接 attach 多约 9 秒。本次不能宣称完整启动已达到 1 秒；仍可继续调查该重连链的重复校验和注册成本。legacy 时间线的独立完整对照实验详见 LEGACY-TIMELINE-FIX.md，41 页内容逐页一致，82.406 秒降至 2.195 秒。

当前 cute-codex native 是包含 legacy 修复的 r4；本次 optimized release 指 Cutex CLI 和两项 Cutex 服务，不把 native 的 dev 构建误称为 release。已重启四个适用的在线 native agent。director-r13 在操作期间自行离线并保持离线；owner-agent-manager-r2 使用更早的独立 runtime，未迁移或替换。
