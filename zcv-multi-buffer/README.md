# zcv-multi-buffer

`zcv-editor` 与具体文本 Buffer 之间的组合文档边界。

`MultiBuffer` 按路径键升序组织多个来源的 excerpt，同路径窗口不重叠，并保留组合坐标到源文件坐标的映射。普通编辑器是「整文件单 excerpt」的组合文档；多文件差异、搜索结果等在此组织显示 excerpt。`Editor` 始终只消费本层，不感知来源数量。

## 坐标空间

- `BufferOffset`：底层文档可见文本内偏移。
- 未删除拼接偏移（组合层内部 `ExcerptOffset`）：各 excerpt 可见内容拼接后的偏移，不含展开的删除段。
- `MultiBufferOffset` / `MultiBufferPoint` / `MultiBufferRow`：最终组合文档偏移，含展开的删除段。

excerpt 结构由 `SumTree` 承载，查询通过 summary 与连续 cursor 推进，不构造扁平数组后二分。`MultiBufferAnchor` 是长期位置；excerpt 的完整路径身份由 `PathKey` 表达，锚点只携带快照内的紧凑序号。`anchor_offset` 为选择、滚动等位置状态提供总坐标解析：路径或片段退出投影时按当前结构落到相邻边界，空投影落到文首。`projected_anchor_offset` 只返回仍属于当前源片段的位置，供折叠、搜索等附属状态在源退出投影时失效；源 Anchor 的版本错误不会被转换成边界坐标。

`MultiBufferLineCursor::source` 复用已定位的行游标取得源映射。`MultiBufferSource::project_range` 从该映射开始，只访问候选跨越的连续工作区片段，返回当前快照内的 `MultiBufferOffset` 范围；不能跨过未展示的源区间。消费方直接用这些坐标筛选可见候选，需要长期保存时再创建组合 Anchor。

`MultiBufferSnapshot::input_scope_at` 把组合输出光标映射到可编辑 excerpt 的源字节位置，再从该源固化的文本／语法快照查询输入政策。不同源和注入语言沿同一入口向 `Editor` 提供政策；组合层不复制配对表或维护语法状态。

`MultiBufferSnapshot::newline_only_bracket_at` 对一个选区查询源 excerpt 的 `brackets.scm` 元数据，并拒绝只读映射、跨 excerpt 选区、跨换行内容或括号外选区。它只返回可在当前 Editor 事务中消费的结构性额外空行事实。
`newline_input_pair_at` 在同一可编辑源行内检查语言配置的输入配对和选区两侧空白；`MultiBufferSource::project_point` 在事务重做时把源内光标重新映射回当前 excerpt。

大纲查询在源快照中固定标签及其相对语法高亮，组合层仅在同一快照内投影源范围并生成 `MultiBufferAnchor`。面板可在 diff 拓扑变化后的重算窗口继续展示旧标签；点击时由当前组合快照解析名称锚点。

Anchor 查询通过路径键、源身份与 excerpt 摘要中的末端 Anchor 定位，源快照通过身份索引读取。删除侧长期位置以 working hunk 起点为主 Anchor，另携带基线 Anchor：展开时解析并排序旧侧位置，收起后投影查询失效，总坐标解析落到 working 边界。

`excerpt_boundaries_in_range` 只遍历指定输出范围中的逻辑窗口起点；`buffer_range` 返回文件全部窗口和删除侧的输出跨度。显示块使用稳定 Anchor 取得当前逻辑窗口，不保存随结构变化而偏移的 excerpt 序号。

## 权威与订阅

- 源文本仍由各 `LanguageBuffer` / `Buffer` 拥有；`MultiBuffer` 不持有源文本的第二份可写副本。
- 源文档只置「已变化」脏位，`MultiBuffer` 在读取快照时按源版本差拉取净编辑；文本变化与语言／设置等元数据变化都推进组合快照链。
- 快照按源身份去重文件，保存可编辑源的 dirty 计数。局部源变化只更新对应源快照与计数；窗口集合变化重新派生文件集合，退出窗口的源不参与保存状态。`is_dirty` 不扫描 excerpts。
- 组合编辑先映射为源 `Buffer` 的编辑再按源事务提交；多源组合文档的组合历史只保存「组合事务身份 → 各源事务身份」映射，excerpt 增删与 diff 展开折叠等结构变更必须在文本事务之外进行。
- 对外同时发布粗粒度语义事件与细粒度输出偏移增量：前者驱动语义刷新，后者让选择与显示缓存增量平移。
- 源编辑的输出增量同时覆盖 excerpt 内容和合成分隔换行；源末尾换行与分隔换行相互替换时，按旧、新输出坐标合并相邻失效范围。普通编辑、外部编辑与撤销／重做共用这一投影入口，文档末尾不补合成换行。

## diff 投影

diff 显示拓扑由 `diff_projection` 持有：`DiffFile`、`DiffDisplaySnapshot`、`DisplayHunk`、`ResolvedDiffHunk`、`WordDiffs`。唯一 diff 事实来自 `zcv-buffer-diff`，组合层只把它叠加为投影，不把 diff 视图做成另一种文档类型。

每个文件独立记录已采用后台 hunk 结果的版本。excerpt 物化对齐 Zed 的 `update_excerpts_for_path`：`add_diff` 按调用方给出的可见范围同步建立该路径的 excerpts，后台 diff 结果只补充旧侧行与词级范围，不决定文件能否进入投影；结果就绪后只替换该源路径的全部 excerpts，新增路径也走同一入口。`set_excerpts_for_path` 保持替换语义，不能把它当作无条件追加。整体替换列表时移除退出列表的路径，仍在计算的文件保留已有映射；文件移除时取消其 diff 订阅，后续结果不再进入投影。

用户选择的展开／折叠状态按 working 源与区块起点 `Anchor` 保存，在当前源快照上匹配；差异类型不参与身份判断。同一区块在删除与修改之间转换，或起点插入文本后重算，保留已有选择。

`MultiBufferSnapshot::diff_hunks_in_lines` 按可见范围查询，同时提供显示几何和 `DiffHunkSource`（工作区 `BufferId`、完整源 `Anchor` 范围）。源范围随变换节点保存为不可变派生数据，旧侧片段仍携带工作区身份；视口裁剪、展开旧侧和组合顺序变化不改变操作来源。纯删除的操作范围为空范围，整文件新增的范围为 `None`。宿主按源身份查找领域操作，不能用显示几何或显示序号反查源 hunk。

差异块身份索引按路径保存在带区块计数摘要的树中。挂接、更新或移除单个文件时，只从该路径的输出变换派生身份，并按路径替换树节点；显示序号通过树摘要定位。整体替换文件列表及全局展开策略切换才重新派生全部路径。索引是可重建的派生快照，源文本和差异计算结果仍分别由源文档与 `BufferDiff` 持有。

词级范围由源 diff 按文档顺序提供。组合查询先将视口与相应侧的片段相交，再二分定位、向前读取相交词级范围；工作区 Anchor 在当前源快照上解析，基线范围按相同协议映射。大变更块超过源 diff 的词级计算限制时仍采用行级差异。

## 边界

- 不做语法解析（属于 `zcv-language`）、不做显示行与软换行（属于 `zcv-editor::DisplayMap`）、不持有选择与滚动（属于 `Editor`）。
- 不依赖 `zcv-editor`、UI 或工作区；不引入协作、远程与 LSP。

## 验证

```bash
cargo test -p zcv-multi-buffer
```
