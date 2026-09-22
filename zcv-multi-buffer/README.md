# zcv-multi-buffer

`zcv-editor` 与具体文本 Buffer 之间的组合文档边界。

`MultiBuffer` 按调用方给出的顺序组织多个来源的 excerpt，并保留组合坐标到源文件坐标的映射。普通编辑器是「整文件单 excerpt」的组合文档；多文件差异、搜索结果等在此重排显示 excerpt。`Editor` 始终只消费本层，不感知来源数量。

## 坐标空间

- `BufferOffset`：底层文档可见文本内偏移。
- 未删除拼接偏移（组合层内部 `ExcerptOffset`）：各 excerpt 可见内容拼接后的偏移，不含展开的删除段。
- `MultiBufferOffset` / `MultiBufferPoint` / `MultiBufferRow`：最终组合文档偏移，含展开的删除段。

excerpt 结构由 `SumTree` 承载，查询通过 summary 与连续 cursor 推进，不构造扁平数组后二分。`MultiBufferAnchor` 是长期位置；excerpt 的完整路径身份由 `PathKey` 表达，锚点只携带快照内的紧凑序号。

## 权威与订阅

- 源文本仍由各 `LanguageBuffer` / `Buffer` 拥有；`MultiBuffer` 不持有源文本的第二份可写副本。
- 源文档只置「已变化」脏位，`MultiBuffer` 在读取快照时按源版本差拉取净编辑；文本变化与语言／设置等元数据变化都推进组合快照链。
- 组合编辑先映射为源 `Buffer` 的编辑再按源事务提交；多源组合文档的组合历史只保存「组合事务身份 → 各源事务身份」映射，excerpt 增删与 diff 展开折叠等结构变更必须在文本事务之外进行。
- 对外同时发布粗粒度语义事件与细粒度输出偏移增量：前者驱动语义刷新，后者让选择与显示缓存增量平移。

## diff 投影

diff 显示拓扑由 `diff_projection` 持有：`DiffFile`、`DiffDisplaySnapshot`、`DisplayHunk`、`ResolvedDiffHunk`、`WordDiffs`。唯一 diff 事实来自 `zcv-buffer-diff`，组合层只把它叠加为投影，不把 diff 视图做成另一种文档类型。

## 边界

- 不做语法解析（属于 `zcv-language`）、不做显示行与软换行（属于 `zcv-editor::DisplayMap`）、不持有选择与滚动（属于 `Editor`）。
- 不依赖 `zcv-editor`、UI 或工作区；不引入协作、远程与 LSP。

## 验证

```bash
cargo test -p zcv-multi-buffer
```
