# zcv-text

Zcv 的纯文本内核：文本存储、坐标模型、事务变异、历史系统与只读快照。

它不负责 UI 渲染、文件读写、语法树、项目索引、搜索、LSP 或协作。

## 版本、编辑日志与坐标索引

一次文本修改必须经过带版本校验的事务；提交后同时追加两份事实：

- **编辑日志（`EditLog`）**：保存带文本的净编辑，供 `edits_since` 增量同步与 undo/redo 回放使用；受 `max_edit_history_entries` / `max_edit_history_bytes` 从最老端裁剪。
- **坐标索引（`CoordinateIndex`）**：只保存 old range 与 replacement 长度等坐标增量，不复制替换文本，永不裁剪。`Anchor::resolve_in` 通过它把任意仍存在的旧版本坐标映射到当前版本。

外部文本更新先计算旧、新文本的差异，再作为普通事务提交；只有差异本身覆盖全文时，订阅才收到全文替换编辑。不存在独立的 `reset`、内容代际或重锚路径。`Anchor::resolve_in` 始终沿坐标索引映射到目标版本；目标快照比锚点更旧，或坐标索引无法覆盖锚点版本时，返回显式错误（`AnchorError::TargetBeforeSource` / `AnchorError::VersionNotIndexed`）。

`edits_since` 的 `TextError::VersionEvicted` 只表示带文本批次已退出编辑日志窗口，不再影响长期锚点解析。

## 关键类型

- `Buffer`：文本内容、版本、保存点、事务管线与历史的唯一可写所有者。
- `Snapshot`：不可变、可廉价克隆的读取边界，携带版本。
- `Anchor`：绑定版本与吸附方向的稳定位置，不持有 Buffer。
- `EditLog` / `CoordinateIndex`：带文本编辑事实与不衰减坐标增量，二者在同一次提交追加。

## 验证

```bash
cargo test -p zcv-text <相关测试过滤条件>
```

测试位于 `tests` 目录或模块内的测试模块中。
