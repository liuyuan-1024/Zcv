# zcv-text

Zcv 的纯文本内核：文本存储、坐标模型、事务变异、历史系统与只读快照。

它不负责 UI 渲染、文件读写、语法树、项目索引、搜索、LSP 或协作。

## 版本、编辑日志与坐标索引

一次文本修改必须经过带版本校验的事务；提交后同时追加两份事实：

- **编辑日志（`EditLog`）**：保存带文本的净编辑，供 `edits_since` 增量同步与 undo/redo 回放使用；受 `max_edit_history_entries` / `max_edit_history_bytes` 从最老端裁剪。
- **坐标索引（`CoordinateIndex`）**：只保存 old range 与 replacement 长度等坐标增量，不复制替换文本，永不裁剪。`Anchor::resolve_in` 通过它把任意仍存在的旧版本坐标映射到当前版本。

reset / 外部基线替换会开启新的内容代际（`BufferGeneration`）；`Anchor::resolve_in` 对旧代际锚点返回显式错误（`AnchorError::GenerationMismatch`），不会被当作普通版本推进，也不会被静默钳到邻近坐标。跨代际坐标映射仍保留在坐标索引中，只有调用方显式调用 `Anchor::rebase_across_generations` 才会重锚（例如外部 reload 后的光标恢复）。目标快照比锚点更旧时同样显式失败（`AnchorError::TargetBeforeSource`）。

`edits_since` 的 `TextError::VersionEvicted` 只表示带文本批次已退出编辑日志窗口，不再影响长期锚点解析。

## 关键类型

- `Buffer`：文本内容、版本、保存点、事务管线与历史的唯一可写所有者。
- `Snapshot`：不可变、可廉价克隆的读取边界，携带版本与代际。
- `Anchor`：绑定内容代际与版本的稳定位置，不持有 Buffer。
- `EditLog` / `CoordinateIndex`：带文本编辑事实与不衰减坐标增量，二者在同一次提交追加。

## 验证

```bash
cargo test -p zcv-text <相关测试过滤条件>
```

测试位于 `tests` 目录或模块内的测试模块中。
