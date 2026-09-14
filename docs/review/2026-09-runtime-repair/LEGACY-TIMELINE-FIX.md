# Legacy 时间线分页缓存（2026-09-14）

cute-codex 提交 `4e3e2b1fc9b1c8931e7a472ab01b665a089f42d3` 修复旧格式历史每页全量重建的问题。之前 `3d48e2ab5` 仅缓存 paginated 路径的 presentation 校验；legacy 在进入该缓存之前分流到独立重建器，因此上次修复覆盖不完整。

LocalThreadStore 现在保存单个 legacy 时间线重建结果；同源连续分页复用 Arc，二分定位游标，只克隆请求页。缓存键包含 thread ID、解析后的源路径、文件大小、修改/创建时间，Unix 额外包含设备、inode 和 ctime。重建前后检查源元数据；变化时不缓存，失败时清空旧缓存。单槽限制保留的历史数量，超过 256 MiB 的源仍可读取但不缓存。解析、显示记录校验、排序及游标协议保持不变；没有修改模型上下文或迁移历史。

验证：codex-thread-store 全部 244 项测试通过。新增覆盖并发复用、追加/缩短、同大小替换、损坏后拒绝旧缓存、多页顺序与内容一致性。just fmt 与 git diff --check 完成；CLI 和 app-server 离线锁定依赖构建成功。

真实 scpolya-2 历史副本通过独立 app-server JSON-RPC 测速，没有发起 thread/resume、模型请求或工具调用。41 页（每页最多 100 项）的完整内容逐页摘要一致：旧服务 82.406 秒，新服务 2.195 秒，约 37.5 倍。该数字测量 timeline/list，不代表完整启动耗时。副本、原始日志和私有数据未发布。

仍有边界：TUI 仍等待完整 presentation timeline；本次修复消除重复解析，没有实现启动惰性加载。首次重建仍随历史规模增长；更新历史后需要重建一次。较大的历史或跨线程轮流读取可能触发缓存未命中。

用户明确授权安全重启 scpolya 和其他 agent。部署采用 release-native-r4，更新默认与已有 native 的期望配置，并重启在线 owner，使 app-server 修复实际生效；不传 --cancel-tasks、不删除历史。离线 agent 保持离线，下次启动使用新版。
