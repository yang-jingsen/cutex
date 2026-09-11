# 私有迁移演练：当前只看计划，等待组保留策略修正

本次不是线上迁移。34 个对象的材料化、私有导入、原始组和项目成员关系
已验证。GLM 代表的历史 TUI 正常退出及终端恢复已通过；但启动注册会为
29 个对象补入默认组，从而触发审查版本失效。不要直接运行组合测试、
更改原有组来绕过检查，或改线上目录。

继承配置的 scpolya-2 也已 Ready、读到旧历史并正常退出，但90秒内没有匹配
到预期状态栏，不能算显示通过。R3 两个失败额度现已用完，不再自动测试。

查看固定计划（只读，无模型调用、无凭据输出）：

```bash
python3 -B /mnt/mambo/PersonaProjects/cutex-mcp-facade-r1/source/scripts/selected_history_rehearsal.py plan \
  /mnt/mambo/PersonaProjects/cutex-mcp-facade-r1/rehearsal-r2-db5WwG/plan.json
```

计划中 `source_status` 是原选择快照，`apply_runtime` 不会自动启动对象。
34 个迁移对象之外的 vce Director 仅为私有项目权限前置记录，不含其历史，
不启动它。真实主机上的项目权限没有改动。

工具分工：

- `capture`：固定名单加注明时间的非敏感配置快照，不重新筛选活跃对象；
  不打开 auth 文件或含服务凭据的通用配置。
- `plan`：对既有快照给出确定性计划，不写任何状态。
- `apply`：只向新的 owner-private 目录复制冻结历史；存在、符号链接或
  中断残留目录一律拒绝，不覆盖、不删除、不自动重试。不是线上安装器。
- `tests/selected_history_composition_run.py`：另建私有 namespace，通过
  正常 Cutex API 导入，再在独立探针 store 测历史。不是线上迁移入口；
  对已知组差异的对象，必须先获得并完成精确的产品修正，不能重复碰运气。

当前保留目录：`rehearsal-r2-db5WwG`（计划/历史材料），
`history-r2-9aqcro6n`、`history-r2-q2mudgye`（两次实际结果）。
私有进程已结束，合成 auth/历史/错误证据保留；没有需要 Human 现在关闭的
线上或 VM 会话。不要用旧程序打开这些新 store 作为“回滚”。

R3 新证据目录：`history-r3-f91s2qf2`（34组/成员关系），
`history-r3-311fohz6`（组注册失败），`history-r3-60pne22a`（GLM通过），
`history-r3-xn_3f0px`（继承配置显示条件失败，但正常退出）。
这些 private namespace 已结束，**没有可直接连接的常驻测试会话**。
若后续批准人工观察，准备者必须先恢复一个独立、无外网的私有 owner，
再在该 namespace 内提供最短 `cutex session stock-attach <精确私有ID>`。
不要在主机直接对原ID执行此命令；当前不提供假装已就绪的入口，也不由
Human 重新启动本次自动测试脚本。

下一步是修正受审查对象注册时的组保留边界，再完成受影响代表的历史路径，
不能削弱 Ready 的版本校验。随后才讨论真实凭据目录
权限维护、角色停机顺序、一次性备份和正式迁移。详见
[最新结果与覆盖矩阵](selected-history-rehearsal-r3-result.md)。
