# zcv-buffer-diff

单个文件的版本化 diff 能力域。

`BufferDiff` 是「一个文件的 diff 结果」的唯一权威：持有 base / index / working 三个来源、版本绑定的 `BufferDiffSnapshot`、在途计算任务与 `DiffOperations`。它不订阅 working buffer，也不决定何时重算；宿主在源文本变化时调用 `BufferDiff::recompute_with_refresh`，本层只负责后台计算、版本门控与结果发布。hunk 的暂存语义统一相对 index 参照判定，所有视图共用同一套。

diff 的显示拓扑（git hunk、展开／折叠、跟踪区间与显示坐标）不属于本 crate，由 `zcv-multi-buffer` 的 diff 投影持有。

## 所有权与生命周期

- `BufferDiff` 是实体，拥有唯一在途计算任务；下次重算替换即取消，实体销毁随字段取消，不 `detach`。
- 工作区、base 与 index 任一来源前进都会使在途结果过期；安装前比较三者的输入版本，过期结果丢弃并补算。
- 只发布 `BufferDiffEvent::DiffChanged { refresh }`；`refresh` 决定订阅方是否重建组合投影。

## 关键类型

- `BufferDiff` / `BufferDiffInput` / `BufferDiffSnapshot`：实体、创建输入与版本绑定结果。
- `DiffHunk` / `DiffHunkKind` / `DiffHunkStaging` / `PendingHunk`：hunk 事实与暂存语义。
- `DiffOperations`：宿主注入的暂存／还原操作集合。

## 边界

- 不做行内高亮、不做坐标投影、不做显示行布局。
- 不依赖 `zcv-multi-buffer`、`zcv-editor`、`zcv-project` 或 UI。
- 不引入协作、远程与 LSP。

## 验证

```bash
cargo test -p zcv-buffer-diff
```
