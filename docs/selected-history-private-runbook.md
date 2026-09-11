# 私有迁移演练：当前只看计划，不启动第三次测试

本次不是线上迁移。34 个对象的材料化与私有导入已验证；历史 TUI 退出检查
仍未通过，两次运行额度已用完。不要直接运行组合测试，也不要改线上目录。

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
  正常 Cutex API 导入，再在独立探针 store 测历史。当前禁止第三次执行，
  等 Director 给出下一次有限验证授权。

当前保留目录：`rehearsal-r2-db5WwG`（计划/历史材料），
`history-r2-9aqcro6n`、`history-r2-q2mudgye`（两次实际结果）。
私有进程已结束，合成 auth/历史/错误证据保留；没有需要 Human 现在关闭的
线上或 VM 会话。不要用旧程序打开这些新 store 作为“回滚”。

下一步只需完成退出/终端恢复诊断和其余代表性历史路径，再讨论真实凭据目录
权限维护、角色停机顺序、一次性备份和正式迁移。详见
[结果与覆盖矩阵](selected-history-rehearsal-r2-result.md)。
