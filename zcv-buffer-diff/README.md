# zcv-buffer-diff

单个文件的版本化 diff 能力域。

`BufferDiff` 是「一个文件的 diff 结果」的唯一权威：持有 working 实体、由文本创建并自持的 base/index 语言缓冲、版本绑定的 `BufferDiffSnapshot`、在途计算任务与 `DiffOperations`。它订阅 working 的文本事件；宿主经 `set_revisions` 一次提交完整的 base/index 输入。源编辑和修订变化共用一个任务，先准备语言快照与差异结果，再通过版本校验整体安装，投影层只消费结果。hunk 的暂存语义统一相对 index 参照判定，所有视图共用同一套。

diff 的显示拓扑（git hunk、展开／折叠、跟踪区间与显示坐标）不属于本 crate，由 `zcv-multi-buffer` 的 diff 投影持有。

hunk 的 working 半开范围两端均使用 `anchor_before`：起点在边界插入后仍定位区块开始，终点不吸收后续插入；纯删除的两端保持同一点，在后台重算前也不会反转。

## 所有权与生命周期

- `BufferDiff` 是实体，拥有唯一在途计算任务；下次重算替换即取消，实体销毁随字段取消，不 `detach`。
- 工作区、base 与 index 任一来源前进都会使在途结果过期；安装前比较三者的输入版本，过期结果丢弃并补算。
- base/index 语言缓冲由 `BufferDiff` 创建并持有；首次构造只登记输入，不在前台从完整修订文本创建 Buffer。修订准备、hunk 计算与发布属于同一个任务，更新经 `LanguageBuffer::snapshot_with_text`/`fast_forward` 在版本校验后整体安装。源编辑复用未变化的修订快照。
- 发布 `BufferDiffEvent::DiffChanged { changed_range }`；范围在当前 working 快照中。Git diff 视图负责按 hunks 装配可见 excerpts，组合投影只同步事件范围覆盖的 excerpts。范围缺失时不做 diff transform 范围同步，不转为整文件重建。
- `BufferDiffSnapshot` 的可见 hunk 查询是窗口装配与组合投影的共同输入。暂存／取消暂存的 pending 保留 hunk 并叠加显示状态；还原工作区的 pending 才临时抑制 hunk。权威修订安装后清除 pending，重新计算暂存状态。

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
