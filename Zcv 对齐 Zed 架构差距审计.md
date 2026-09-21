# Zcv 对齐 Zed 架构差距审计

> 本文是对 Zcv 编辑器核心与 Zed 原型在架构实现上差异的只读复审记录。
> 判定标准是 [docs/编辑器架构.md](docs/编辑器架构.md) 第 1–17 节的职责边界、状态所有权、依赖方向、数据流与稳定不变量（`T-*`、`L-*`、`M-*`、`D-*`、`E-*`、`R-*`、`P-*`），
> 审计方法见 [.agents/skills/zcv-architecture-maintenance/references/architecture-assessment.md](.agents/skills/zcv-architecture-maintenance/references/architecture-assessment.md)。
> 原型源码基线为本机 `/Users/liuyuan/projects/zed`。
>
> 本文只记录职责、所有权、依赖、坐标、版本、增量协议和生命周期上的可取证据差异；
> 文件名、模块形状、类型命名不同不构成差距。当前实现更简单、改动更大、调用方更多都不是偏离理由。
>
> 与 [演进计划.local/架构迁移计划.md](演进计划.local/架构迁移计划.md) 第 4.1 节 `2026-09-20` 审计相比，本文是同一批层的独立复审；
> 两者结论不一致时，以本文复核后的代码证据为准。
>
> **更新记录（2026-09-21）**：本文同时作为对齐进度台账维护。第 2.2 节表格带「状态」列，第 4 节各条带状态行，记录已落地阶段、提交与保留偏离。
> 已修正：M-A、M-B、E-A、E-B、D-B、D-A、D-D、E-C、L-B、C-C、C-D、D-C；登记为产品裁剪：L-A（不引入运行期语言注册）。

---

## 1. 审计范围与方法

审计覆盖编辑器核心唯一数据流上的六层，以及工程边界：

~~~text
zcv-text::Buffer
  → zcv-language::LanguageBuffer
  → zcv-multi-buffer::MultiBuffer（含 zcv-buffer-diff）
  → zcv-editor::DisplayMap（Fold → Tab → Wrap → Block）
  → zcv-editor::Editor（选择 / 滚动 / 输入 / 事务）
  → zcv-editor::EditorElement
Project / GitStore / Workspace / Item
~~~

方法：先建立每层现状事实图（所有者、快照、坐标、增量入口、生命周期、依赖），再与架构文档不变量和 Zed 对应实现逐条对照。
所有结论都给出最窄文件与行号；标注为「未确认」的条目只在第 5 节列为待验证线索，不计入第 4 节已确认差距。

只读审计本身未运行构建、测试或运行时验证；对齐落地阶段的验证记录见第 9 节。

---

## 2. 结论摘要

### 2.1 总体判断

- 六层分层、唯一数据流与状态所有权整体成立；文本、语言、组合文档、显示、交互、渲染各有唯一可写所有者，未发现第二份可写文本、excerpt 或显示行权威。
- 单一 `Editor` 管线成立：普通文档、搜索结果、diff、预览都经 `Editor::for_multi_buffer`，没有按文档形态复制编辑器或平行交互状态（E-7）。
- 依赖方向与 Zed 一致：`zcv-text ← zcv-language / zcv-buffer-diff ← zcv-multi-buffer ← zcv-editor ← zcv-project / zcv-workspace`；`zcv-project` 不依赖 `zcv-multi-buffer`、`zcv-editor`、`zcv-workspace`（与 Zed `crates/project` 相同）。
- 第 18.2 节「复刻偏离」当前登记为空这一事实无法维持：本次确认存在若干应登记的复刻偏离，集中在组合文档增量回退、跨代际锚点解析、显示层 block 增量协议、显示热路径物化、选择唯一入口、语言注入待解析层、组合坐标类型误用和项目搜索旁路物化。
- 其余差异属于两类允许偏离：已声明裁剪（协作/远程/LSP 及 InlayMap）或语义一致的实现技术差异；已登记暂不处理项（A11、C6、F-14、R5）仍然存在但不改变数据流与所有权。
- 对齐进度：第 2.2 节 19 条已确认差距中，12 条已修正，6 条待处理（E-G、E-H、T-A、T-B、E-D、E-E），1 条登记为产品裁剪（L-A）。已落地阶段与提交见第 2.2 节「状态」列与第 4 节各条状态行。

### 2.2 已确认差距总表

严重度：高＝违反稳定不变量且会产生全量失效、错误位置或状态分叉；中＝违反稳定不变量但当前影响局部或有边界；低＝契约/生命周期缺口，影响有限。

状态：✅ 已修正并提交；⚠️ 部分修正（保留项见第 4 节状态行）；➖ 已登记为产品裁剪（不再作为待修正项）；⬜ 待处理。提交号对应本轮对齐各阶段。

| 编号 | 严重度 | 层 | 一句话 | 违反不变量 | 状态 |
| --- | --- | --- | --- | --- | --- |
| M-A | 高 | 组合文档 | 增量推导 `None` 与多次变化合并被发布为 reset，显示链整链重建 | M-5、2.2、8.4、D-3、D-5 | ✅ `9c7825f1` |
| M-B | 高 | 组合文档 | 通用 `resolve_anchor` 在代际失配时静默 `rebase_across_generations` | T-8、4.2、M-4 | ✅ `a034e79b` |
| D-C | 中 | 显示投影 | Fold/Tab 层 Edit 不是本层坐标类型，靠全局 delta 与裸区间兜底 | D-2、4.1 | ✅（本轮） |
| D-A | 中 | 显示投影 | Block 层无本层 edit 协议，wrap 编辑时整体 `place()`，结构变化全量重建 | D-2、D-5、D-8 | ✅ `9bb9587d` |
| D-B | 中 | 显示投影 | 布局热路径 `row_text` 物化整行，调用方自行剥离 `\n` | D-7、8.2 | ✅ `bd371e8b` |
| D-D | 中 | 显示投影 | 异步 wrap 完成由 observe 回调直接推进快照，与 `snapshot→sync` 双路径 | D-5、13.2 | ✅ `9bb9587d` |
| E-A | 中 | 交互 | 选择变更存在多个直接写点，未统一入口；相邻选择被错误合并 | E-8、9.3 | ✅ `4b377820` |
| E-B | 中 | 交互 | 搜索命中/自动闭合用 `zcv_text::Anchor` 承载组合坐标并直接读 offset | T-3、T-8、E-4、4.1、8.5 | ✅ `96e9304d` |
| E-C | 中低 | 交互 | 滚动条标记后台结果安装前不校验显示版本 | 13.3 | ✅ `a4e58d57` |
| L-A | 中 | 语言 | 未加载注入语言被静默丢弃，无 Pending 层与注册表版本补解析 | L-8、6.3、6.6 | ➖ `Pending` 已表示；注册表版本补解析登记为产品裁剪 |
| L-B | 中 | 语言 | 注入层范围用裸 `Range<usize>`；`edits_since` 失败时整层丢弃/全文失效 | 6.3、增量不得回退 | ✅ `b6fe41ef` |
| C-C | 中 | 组合文档 | 缺少 `ExcerptOffset` 独立坐标 newtype，源/输入/输出以裸 usize 混算 | 4.1、7.2 | ✅ `（本轮）` |
| E-G | 中 | 工程边界 | 项目搜索 `read_to_string` 自建 Buffer 并注册为该路径权威文档，绕过文件解码边界 | P-1、11.1、3.1 | ⬜ |
| T-A | 中 | 文本事实 | T-7 历史可见性 / 当前→旧版本映射 / 历史文本重建能力未提供 | T-7 | ⬜ |
| C-D | 低中 | 组合文档 | hunk 身份承载在输入 excerpt 树、以 `visible_hunks` 下标为身份 | M-2、7.3 | ✅ `（本轮）` |
| E-D | 低 | 交互 | `SelectionHistory` 只增不减，失败会话留孤儿记录 | E-5 生命周期关联 | ⬜ |
| E-E | 低 | 交互 | `BlinkManager` 定时任务 `detach`，不可显式取消 | 13.3 | ⬜ |
| E-H | 低 | 工程边界 | `Item`/`ItemHandle` 暴露 `uses_editor_document_toolbar`、`receives_git_projection`，超出 11.2 | 11.2 | ⬜ |
| T-B | 中 | 文本事实 | 无 T-9「基线派生快照 + 版本校验后原子安装」文本层入口 | T-9 | ⬜ |

> 说明：`T-B`、`E-D`、`E-E`、`C-D` 的部分影响未做运行时复现，按已确认的契约/结构偏离登记，实际用户可观察程度见各条「影响」。

### 2.3 已登记暂不处理项（复核仍存在）

见第 6 节：A11、C6、F-14、R5。它们在 [docs/架构决策记录.md](docs/架构决策记录.md) 第 6 条与迁移计划中已登记，本次不重复计为新差距。

### 2.4 非差距（裁剪与实现差异）

见第 7 节。包含协作/远程/LSP 裁剪、InlayMap 删除，以及 rope 存储、本地单调版本、多入口编辑语义、crate 内互相引用等实现差异。

---

## 3. 分层对齐事实图

> 以下为审计时的现状事实图；各层「缺口」行的完成状态以第 2.2 节表格与第 4 节状态行为准。

### 3.1 文本事实层 `zcv-text`

- 所有者：`Buffer` 私有聚合 `storage / version / generation / saved_version / edit_log / coordinate_index / history / session`（`zcv-text/src/buffer/mod.rs:38-55`）；文本、版本、代际的写入只在 `commit_prepared_text_change` 一处（`zcv-text/src/buffer/transaction_pipeline/apply.rs:234-264`）。T-1/T-6 成立。
- 事务：`Transaction` 携带 `base_version`；失配显式返回 `VersionMismatch`（`apply.rs:209-215`）；两阶段原子替换（`apply.rs:169-197`）；空 `EditList` 被拒绝（`zcv-text/src/transaction/core.rs:21-23`）。T-2 成立。
- 快照：`Snapshot` 不可变、一次绑定 `version/generation/storage/edit_log/coordinate_index`（`zcv-text/src/snapshot.rs:24-33`）。T-4 成立。
- 锚点：`Anchor::resolve_in` 只走不衰减 `CoordinateIndex`（`zcv-text/src/tracking/anchor.rs:87-92`）；`CoordinateIndex` 与受预算裁剪的 `EditLog` 分离、同一次提交追加（`apply.rs:249-262`）。T-3/T-5 成立。
- 代际：reset 开启新代际（`apply.rs:246-248`）；普通解析比对代际并显式失败（`snapshot.rs:72-80`）；显式重锚为 `rebase_across_generations`（`anchor.rs:98-107`）。T-8 的文本层半边成立（跨层误用见 M-B/E-B）。
- 订阅：每消费者独立 `SubscriptionState`、无全局队列、批次携带旧/新范围（`zcv-text/src/text_changes.rs:305-379`）。5.4 成立。
- 缺口：T-7 的历史查询与 T-9 的派生快照入口未复刻（T-A、T-B）。

### 3.2 语言快照层 `zcv-language`

- 所有者：`LanguageBuffer` 直接持有文本 `Buffer` 与 `Mutex<LanguageState>`，同生命周期；`ParseTask` 的 `Drop` 取消后台解析（`zcv-language/src/language_buffer.rs:105-109, 72-76`）。L-1 方向成立。
- 一致快照：`snapshot()` 在返回前 `interpolate`，使 `syntax.version == text.version`（`language_buffer.rs:158-171`）。L-1 成立。
- 插值/解析分离：`parsed_version`/`interpolated_version` 分离，`did_parse` 要求版本与语言匹配否则拒绝安装（`zcv-language/src/syntax_map.rs:24-32, 263-277`）。L-2 成立。
- 单任务与唯一安装：`start_reparse` 替换旧任务、`install_parse_result` 为唯一安装入口（`language_buffer.rs:363-365, 426-436`）。L-3 成立。
- 查询编译：查询在装配期编译进 `CompiledLanguageQueries`/`Arc<Query>`（`zcv-language/src/registry.rs:34-43`）。6.2 成立。
- 缺口（审计时）：注入层待解析语义缺失（L-A，已补 `Pending` 表示）、注入层坐标为裸偏移且失败整层丢弃（L-B，已修正）、`LanguageSettings` 仅覆盖 tab（已登记 R5）。

### 3.3 组合文档层 `zcv-multi-buffer` / `zcv-buffer-diff`

- 所有者：`MultiBuffer` 恒为 excerpts 形态，普通文档为整文件单 excerpt（`zcv-multi-buffer/src/multi_buffer.rs:3084-3113`）；源文本仍由各 `LanguageBuffer` 拥有。
- 快照所有权：`MultiBuffer` 持有唯一 `snapshot: MultiBufferSnapshot` 与 `snapshot_dirty`，源事件只登记 `pending_source_syncs`，`snapshot(cx)` 批量消费并整帧提交（`multi_buffer.rs:3052, 3104-3113, 4837-4890`）。阶段 8 的「源事件只置脏、读取时拉取」成立。
- 双树与连续 cursor：输入 `SumTree<Excerpt>` + 输出 `SumTree<DiffTransform>`，查询经 `MultiBufferCursor`/summary 推进（`multi_buffer.rs:1528-1530, 1173-1253`）。M-2/M-3 成立。
- diff 域：`zcv-buffer-diff::BufferDiff` 拥有 hunk 事实，`MultiBuffer` 只持展示态 `DiffState` 与独立版本的 `DiffDisplaySnapshot`（`zcv-buffer-diff/src/buffer_diff.rs:244-257`，`zcv-multi-buffer/src/diff_projection.rs:75-91, 195-213`）。7.3 方向成立。
- 事务：组合编辑映射为源 `Buffer::edit`，组合历史只保存源事务身份映射（`multi_buffer.rs:1737-1740, 4438-4485`）。M-6/M-8 成立。
- 缺口（审计时）：增量推导 `None` → reset（M-A，已修正）、通用解析静默跨代际重锚（M-B，已修正）、缺 `ExcerptOffset`（C-C，待处理）、hunk 身份位置（C-D，待处理）。

### 3.4 显示投影层 `zcv-editor::DisplayMap`

- 层顺序：`FoldMap → TabMap → WrapMap(Entity) → BlockSnapshot`，每层快照嵌套下层（`zcv-editor/src/display_map.rs:746-779, 1088-1113`）。D-1/D-4 成立。
- 唯一推进入口：`DisplayMap::snapshot` 消费组合订阅并逐层同步；`Editor` 只经 `cached_snapshot` 读取，不缓存第二份（`display_map.rs:797-821`，`zcv-editor/src/view/mod.rs:892-894`）。E-1/E-2 方向成立。
- 坐标与 Bias：`ProjectedLineIndex/ProjectedPoint`、`WrapRow/WrapPoint`、`DisplayPoint/DisplayRow`、`TabColumn`、`FoldBias` 等逐层 newtype 与显式 Bias 广泛存在（`display_map.rs:74-161`，`display_map/tab_map.rs:30`，`display_map/fold_map.rs:144`）。D-11 成立。
- 异步：只有 wrap 层是 `Entity`，自持 `background_task`/`pending_edits`/`interpolated_edits`，落地时先反转插值再叠加真实编辑（`display_map/wrap_map.rs:1114-1125, 1240-1260`）。8.4/D-12 方向成立。
- 折叠候选：`CreaseMap` 只存宿主显式注入锚点，语法候选由 `DisplaySnapshot` 按可见逻辑行即时查询并带视口缓存（`display_map/crease_map.rs:46-88`，`display_map.rs:302-391`）。8.5 方向成立，且已无组合层全源 fold list（`fold_anchors`/`fold_sources` 搜索为 0）。
- 缺口（审计时）：Block 层增量协议（D-A，已修正）、热路径物化整行（D-B，已修正）、Fold/Tab Edit 坐标类型（D-C，已修正）、observe 与 snapshot 双路径（D-D，已修正）。

### 3.5 交互与渲染层 `zcv-editor::Editor` / `EditorElement`

- 持有关系：`Editor` 持 `multi_buffer: Entity<MultiBuffer>` 与 `display_map: Entity<DisplayMap>`，二者消费同一组合文档（`view/mod.rs:247-250, 1583-1585`）。9.2 成立。
- 事务唯一入口：`change` / `change_with_after` / `change_with_after_post` 全部经 `commit_session`，唯一 `MultiBuffer::edit` 调用点在 `commit_session`（`view/mod.rs:1674-1721, 1727-1769`）；普通编辑、IME、搜索替换、重命名、自动闭合、undo/redo 均接入。E-3 成立。
- 落地顺序与事件：文本 → `advance_snapshots` → 选区落位 → `end_transaction` 返回真实身份才发布 `Edited{TransactionId}`；失败结束空事务并恢复编辑前选择（`view/mod.rs:1748-1768, 1837-1852`）。E-6/E-9 成立。
- 长期位置：选择 `SelectionSet<MultiBufferAnchor>`、滚动 `ScrollAnchor { MultiBufferAnchor, offset }`，消费时按当前快照解析（`view/mod.rs:257`，`scroll.rs:35-39, 376-383`，`selection/core.rs:110-140`）。E-4（选择/滚动/折叠）成立。
- 渲染边界：`EditorElement` 无 `.edit(`，布局只读 `editor.snapshot()`/`selections`，写回仅几何缓存与显示配置（`element.rs:1114-1121, 1443-1454, 2042-2046`）。R-1/R-2 成立。
- 缺口（审计时）：选择唯一入口与相邻合并（E-A，已修正）、组合坐标类型误用（E-B，已修正）、滚动条标记版本校验（E-C，已修正）、选择历史生命周期（E-D，待处理）、blink 任务取消（E-E，待处理）。

### 3.6 工程边界 `zcv-project` / `zcv-workspace`

- 文档索引：`BufferStore` 用弱引用按路径复用（`zcv-project/src/buffer_store.rs:19-22, 80-102`）；Git 修订文本由 `GitStore` 按 `(revision, path)` 唯一持有（`zcv-project/src/git_store/mod.rs:264-269`）。P-1 方向成立。
- 依赖：`zcv-project` 只依赖 text/language/buffer-diff/path/fs-watch/git，不依赖 multi-buffer/editor/workspace（`zcv-project/Cargo.toml:9-25`），与 Zed `crates/project` 相同。P-4/14.2 成立。
- Workspace：只持窗口容器、Pane/Dock、Item 句柄、布局持久化与命令分发，不复制 Item 领域状态（`zcv-workspace/src/workspace_state.rs:52-77`，`pane.rs:95-109`）。P-3 成立。
- 工具区：`Pane` 拥有 `Toolbar`，`Item` 不返回工具区视图（`pane.rs:95-109, 646-651`，`toolbar.rs:36-43`）。11.2 主体成立。
- 缺口（审计时）：项目搜索旁路物化（E-G，待处理）、GitStore optimistic index 第二份文本（待验证）、Item 能力标志扩展（E-H，待处理）。

---

## 4. 已确认差距

每条给出：严重度、证据位置、当前数据流、违反不变量、影响、目标边界、需一起迁移的调用方/测试/文档、定向验证。

> 各条证据为审计时事实；条目顶部的状态行记录后续对齐的落地阶段、提交与保留偏离。

### 4.1 组合文档层

#### M-A（高）增量推导 `None` 与多次变化合并被发布为 reset，显示链整链重建

> 状态：已修正（`9c7825f1 收敛组合投影增量批次`）。

- 位置：`zcv-multi-buffer/src/multi_buffer.rs:3332-3426`（`source_incremental_change`）、`multi_buffer.rs:4300-4325`、`multi_buffer.rs:3195-3230`、`multi_buffer.rs:1639-1652`、`multi_buffer.rs:1682-1708`；消费端 `zcv-editor/src/display_map.rs:650-673`。
- 证据：
  - `source_incremental_change` 在 `patch().is_empty()`（返回 `None`）、`old_records.len() != new_records.len()`（返回 `None`）、`output_edits.is_empty()`（返回 `None`）三种情况下丢弃增量信息。
  - `apply_source_change` 把 `Option` 直接交给 `publish_projection_change`。
  - `MultiBufferSubscription::consume` 在 `pending_batch == None` 时返回 `TextChangeBatch::reset(old, current)`（`multi_buffer.rs:1651`）。
  - `ProjectionChangeTopic::publish` 在第二次变化时把 `pending_batch` 置 `None`（`multi_buffer.rs:1698-1704`），因此「一次读取前多个源变化」也被合并为 reset。
  - `DisplayMap::buffer_edits_from_batch` 对 `requires_reset()` 返回 `0..old_len → 0..new_len` 的整链替换编辑（`display_map.rs:655-659`）。
- 违反不变量：2.2「组合文档在读取时按源版本差拉取净编辑」、M-5、8.4、D-3、D-5；审计原则「增量入口不得静默回退到全量」。
- 影响：源编辑完全落在未展示区域（diff 折叠上下文、搜索未命中区、多视图共享源）时本应「无输出变化」，却触发 Fold→Tab→Wrap→Block 整链重建；excerpt 数不一致的推导失败被 reset 掩盖，失去失败信号；一次读取前编辑多个源时显示链全量重建。
- 目标边界：`source_incremental_change` 必须区分「无输出变化」（返回空增量，不重载）与「无法推导」（显式失败/不变量失败）；reset 只能来自 `source_change.requires_reset()` 这一显式基线替换语义。`ProjectionChangeTopic` 必须能组合多个投影批次，而不是第二个变化即丢弃。
- 迁移项：`ProjectionChangeTopic`/`MultiBufferSubscription` 的 pending 语义、`SourceIncremental` 类型、`DisplayMap::buffer_edits_from_batch`，以及 `zcv-multi-buffer` 与 `zcv-editor` 的同步测试。
- 定向验证：多 excerpt 组合文档中，对所有 excerpt 之外的源区间提交一次编辑，断言 `consume()` 的批次为空且不 `requires_reset`；一次读取前编辑两个源，断言显示链只收到受影响的局部 edit。

#### M-B（高）通用锚点解析静默执行跨代际重锚

> 状态：已修正（`a034e79b 显式化锚点跨代际重锚`）。

- 位置：`zcv-multi-buffer/src/multi_buffer.rs:5405-5437`（重锚在 `5428`），入口 `multi_buffer.rs:2356-2365`、`multi_buffer.rs:5070-5079`，调用方 `multi_buffer.rs:5555-5601`。
- 证据：
  - `excerpt_anchor_source_offset` 先 `anchor.text_anchor.resolve_in(text)`；失败即调用 `anchor.text_anchor.rebase_across_generations(text)`，成功则返回旧代际锚点映射后的坐标。
  - 上游 `resolve_anchor_in_mappings` 是通用 `resolve_anchor`，被选择、滚动等长期位置消费。
  - 注释自称「显式重锚」，但入口是通用解析；`zcv-text/src/snapshot.rs:67-80` 明确普通解析必须让代际失配显式失败，`rebase_across_generations` 只应由明确知晓 reset 语义的调用方使用。
- 违反不变量：T-8、4.2「不能把显式重锚路径隐藏在普通解析里」、M-4。
- 影响：外部 reload / Git 基线替换后，落在旧代际的选择与滚动会在普通解析中被静默挪到新坐标，调用方无法区分「正常解析」与「跨代际重锚」，可能定位到错误位置。
- 目标边界：`resolve_anchor` 在代际失配或目标更旧时返回显式失败（`None`/`Invalid`）；另设命名清晰的显式重锚入口，只由外部 reload 路径调用。
- 迁移项：`SourceAnchorResolution`、`resolve_anchor_in_mappings`、`Editor` 的选择/滚动解析调用方、`multi_buffer` 锚点测试。
- 定向验证：对源执行一次 reset 产生新代际后，旧锚点调用 `MultiBuffer::resolve_anchor` 必须失败；显式重锚入口成功后，外部 reload 光标恢复用例仍通过。

#### C-C（中）缺少 `ExcerptOffset` 独立坐标 newtype

> 状态：已修正（本轮）。
> 引入 `ExcerptOffset` 表示输入（未删除拼接）坐标；`output_records_for_path_source` 返回类型化为 `(MultiBufferOffset, ExcerptOffset, TextRange)`，`source_incremental_change` 的源内相对偏移统一经命名的 `excerpt_relative` 转换，输出侧显式为 `MultiBufferOffset`。
> `DiffTransformSummary.input` 改为 `ExcerptInputSummary`，字节长度用 `ExcerptOffset`；`MappingPosition` 累加 `input_offset` 维度并提供 `SeekTarget for ExcerptOffset`，与 Zed 的 `ExcerptOffset` 游标维度对齐。

- 位置：`zcv-multi-buffer/src/multi_buffer.rs:554-561`（`ExcerptSummary.text: MBTextSummary`）、`multi_buffer.rs:596-600`（`DiffTransformSummary { input, output }`）、`multi_buffer.rs:911-938`（`MappingPosition` 多 `usize` 维度并存）、`multi_buffer.rs:1099-1118`、`multi_buffer.rs:3332-3426`。
- 证据：全仓 `ExcerptOffset` 出现 0 次；源偏移用 `zcv_text::ByteOffset`，输出偏移用 `MultiBufferOffset`，但「未删除拼接偏移」没有独立类型，仅在 `DiffTransformSummary.input` 中以 `MBTextSummary` 表达。`source_incremental_change` 用 `*old_output_at + overlap.start().get() - excerpt_range.start().get()` 把源/输入/输出坐标以裸整数算术串联。Zed 原型定义了 `ExcerptOffset = ExcerptDimension<MultiBufferOffset>` 与 `BufferOffset`。
- 违反不变量：4.1「三个坐标空间必须有明确 newtype 与转换边界……不允许直接线性映射；不同层不得复用一个含义不同的裸 `usize`」、7.2。
- 影响：存在展开删除 hunk 时「未删除拼接偏移」与「输出偏移」不再相等，类型系统不阻止二者混用；后续改动可能把输入拼接长度当输出偏移，产生错位坐标，正确性完全依赖局部算术约定。
- 目标边界：引入 `ExcerptOffset`（必要时 `ExcerptPoint/ExcerptRow`），使 `DiffTransformSummary.input` 使用该维度，source→excerpt→output 的转换成为显式命名映射。
- 迁移项：`ExcerptSummary`、`DiffTransformSummary`、`MappingPosition` 维度、`output_records_for_path_source`、`source_incremental_change`、`MBTextSummary` 使用点与相关测试。
- 定向验证：类型层面消除输入/输出维度混用；一个展开删除 hunk 的 diff 用例断言 source→excerpt→output 往返在删除段两侧正确。

#### C-D（低中）hunk 身份承载在输入 excerpt 树而非输出变换节点

> 状态：已修正（本轮）。
> hunk 元数据从输入 `Excerpt.diff_hunks` 迁到输出 `DiffTransform::{BufferContent, DeletedHunk}.hunks`，输入 excerpts 树恢复为纯源坐标；`ExcerptRange` 仍作为物化期的构造载体。
> 各构造/重建路径都显式带着 hunk 重建输出节点：`replace_all_excerpts`/`build_entries_for_excerpts` 用物化产生的 hunk；`splice_source_path`/`splice_excerpt_entries`/`fix_document_tail_newline`/`rebuild_diff_transforms_from_excerpts` 按同序从旧输出节点取回。
> hunk 身份为 hunk 起点的工作区 `Anchor`；`buffer_diff_hunk_at` 按 anchor 解析到当前 hunk，diff 重算后不再随 `visible_hunks` 下标漂移；`projection_items_equal` 改比较输出节点的 hunks，输入树元数据变化不再被当作输入结构变化。

- 位置：`zcv-multi-buffer/src/multi_buffer.rs:244-256`（`DiffTransformHunkInfo` 绑定 `visible_hunks()` 下标）、`multi_buffer.rs:298`（`Excerpt.diff_hunks`）、`multi_buffer.rs:417-476`（`DiffTransform` 只持 `summary`）、`multi_buffer.rs:1010-1019`（`projection_items_equal` 比较 `diff_hunks`）、`zcv-multi-buffer/src/diff_projection.rs:1314-1419`。
- 证据：输入侧 `Excerpt` 承载 staging/expanded/kind/base_lines 等输出侧显示元数据；输出侧 `DiffTransform` 只有摘要。Zed 把 `DiffTransformHunkInfo` 放在输出 `DiffTransform::BufferContent/DeletedHunk` 节点上，用 anchor 身份而非下标。
- 违反不变量：M-2、7.3「hunk 身份随投影变换节点承载」。
- 影响：本应位置无关的输入树承载输出显示元数据，`projection_items_equal` 会把显示元数据变化当作输入结构变化来推导增量范围；下标身份在 diff 重算后不稳定，目前靠 revision 门控规避。
- 目标边界：hunk 身份移动到输出 `DiffTransform` 节点，使用工作区 anchor 作为身份；输入 excerpt 树保持纯源坐标。
- 迁移项：`DiffTransform`、`from_excerpt`、`splice_excerpt_entries`、`replace_all_excerpts`、`projection_items_equal`、`derive_diff_display_for_path`、diff 测试。
- 定向验证：hunk 重排/重算后显示 hunk 身份随节点迁移而非随下标漂移；仅编辑既有 hunk 内容不触发 excerpt 身份重建。

#### C-E（低，待验证）结构变更入口没有「必须在文本事务之外」的守卫

> 状态：待验证，未处理。

- 位置：`zcv-multi-buffer/src/multi_buffer.rs:4546-4610`（`start_transaction`）、`multi_buffer.rs:3431`、`multi_buffer.rs:4030`、`multi_buffer.rs:4340-4505`。
- 证据：`start_transaction` 置 `active_transaction = Some(id)`，但 `set_excerpts_for_path`/`replace_all_excerpts`/diff 重建均未检查 `active_transaction.is_some()`；`end_transaction` 仍会按已变化的拓扑收尾。
- 违反不变量：M-8「excerpt 增删、diff 展开折叠等结构性变更必须在文本事务之外进行」。
- 影响：若外部调用方在事务中触发结构重建，组合事务身份与坐标基准会错配。当前未找到实际调用点，故列入第 5 节待验证。
- 目标边界：结构变更入口显式断言 `active_transaction.is_none()`。
- 定向验证：`start_transaction` 后调用 `set_excerpts_for_path`，断言失败。

### 4.2 显示投影层

#### D-A（中）Block 层没有本层增量 edit 协议，任一 wrap 编辑整体重排 transforms

> 状态：已修正（`9bb9587d 移除显示同步快速路径并收口块投影同步`）。

- 位置：`zcv-editor/src/display_map.rs:1149-1167`（`current_block_snapshot`）、`zcv-editor/src/display_map/block_map.rs:375-444`（`new`/`place`）、`block_map.rs:526-580`（`resync`）。
- 证据：`current_block_snapshot` 先 `resync`；`resync` 在 `folded_buffers` 变化（`block_map.rs:533-535`）或 `block_start_indices` 变化（`block_map.rs:542-544`）时返回 `None`，调用方随即 `BlockSnapshot::new` 全量重建。即便走「增量」路径，只要有 wrap 编辑就重新 `sort_by_key` 并 `place()` 重建整棵 transforms 树（`block_map.rs:561-579`），不做前缀/后缀子树复用。Block 层不产出本层 `BlockEdit`。Zed `crates/editor/src/display_map/block_map.rs:806-1019` 的 `sync` 消费 `WrapPatch`，用 `cursor.slice` 复用未受影响前缀，`read` 在 `snapshot` 路径调用它。
- 违反不变量：D-2（每层提供增量 edit 映射）、D-8（同构段不合并、input 精确覆盖）、D-5。
- 影响：整文件折叠、excerpt 结构变化触发块投影全量重建；Block 层不参与逐层 edit 协议，快照链的叶层是「整棵重建」而非「增量推进」。当前 `place()` 成本为 O(excerpt 数/块数) 而非 O(行数)，因此影响以协议缺失和结构边界全量为主。
- 目标边界：Block 层持有可变层状态与唯一 `sync(wrap_snapshot, WrapEdit) → (BlockSnapshot, BlockEdit)`；`folded_buffers`/excerpt 结构变化表达为显式编辑语义，删除无名 `None` 回退。
- 迁移项：`display_map.rs`、`block_map.rs`、`display_map/test/*`、`view/test/display_tests.rs`。
- 定向验证：整文件折叠后断言只产生受影响区间的 edit，且前后缀 transforms 为 Arc 复用。

#### D-B（中）显示热路径物化整行并要求调用方剥离行终止符

> 状态：已修正（`bd371e8b 显示热路径改用无终止符 chunk`）。

- 位置：`zcv-editor/src/view/mod.rs:2510-2548`（`layout_line_width`）、`zcv-editor/src/display_map.rs:525-535`（`row_text`）、`display_map/fold_map.rs:585-606`。
- 证据：`layout_line_width` 调 `display_snapshot.row_text(*projected_line)` 取整行，再在调用方 `strip_suffix('\n')`（`view/mod.rs:2532-2534`）；折叠行的 `row_text` 还会拼接 `'\n'`。8.2 明确「面向单行 shaping 的文本行 chunk 只携带行内容，不携带 `\r`/`\n` 终止符；不得把去除终止符的责任分散给各个渲染调用点」。
- 违反不变量：D-7、8.2。
- 影响：未换行模式下最长行宽度测量对整行分配 + 直接 shaping，绕过 `DisplayChunks` 游标；终止符剥离逻辑分散在渲染调用点，CRLF/折叠合并行易不一致。
- 目标边界：布局经 `DisplayChunks`/游标按可见片段测量；`row_text` 退出生产热路径，终止符不由调用方处理。
- 迁移项：`view/mod.rs:2510-2548, 960-987`、element 布局测试。
- 定向验证：生产代码中不再出现 `strip_suffix('\n')`；多字节/CRLF/折叠行宽度与 chunk 输出一致。

#### D-C（中）Fold/Tab 层 Edit 不是本层坐标类型，靠有界补齐兜底

> 状态：已修正（`24fd79d6 折叠与 Tab 编辑改用本层坐标`、`bfb8115a 对齐折叠占位符文本模型与字形`，以及本轮字节偏移重写）。
> fold 变换树已改为 Zed 的字节偏移文本变换：`Transform { summary: TransformSummary { input: MBTextSummary, output: MBTextSummary }, placeholder }`、`TransformSummary { input, output }`、`is_fold()`、`FoldOffset`/`FoldPoint`/`to_point`/`to_offset`、`FoldEdit = Edit<FoldOffset>`。
> `FoldMap::sync` 按折叠字节范围切分输入，折叠区间输出占位符文本（默认省略号，或 `collapsed_text`），相邻且都 `merge_adjacent` 的折叠合并；每条下层字节编辑经旧/新变换树映射为精确的输出字节区间，`linear_fold_edit` 与 `global_delta` 补齐已删除。
> `TabMap` 按 fold 偏移经旧/新 `FoldSnapshot` 映射到 `TabPoint`；`row_text` 与段表由输出变换推导，不再手工合成合并行。

- 位置：`zcv-editor/src/display_map/fold_map.rs`、`display_map/tab_map.rs`。
- 证据：`FoldEdit` 是 `FoldOffset` 字节区间；`TransformSummary` 同时携带输入与输出 `MBTextSummary`；`sync` 直接搬运未变变换并按折叠边界重建受影响区间，折叠内端点按 Bias 吸附到折叠边界。
- 违反不变量：D-2、4.1、D-11（跨层 Bias 语义）。
- 影响：失效区间已按字节偏移精确表达，行粒度映射的守恒补齐不再存在；`TabMap` 行数守恒断言由字节映射自然成立。
- 目标边界：`FoldEdit/TabEdit` 使用本层 `FoldOffset`/`FoldPoint`；无 `global_delta` 补齐；跨层显式 Bias。
- 迁移项：`fold_map.rs`、`tab_map.rs`、`chunk.rs`、`wrap_map.rs` 段表消费端、`fold_map_tests.rs`。
- 定向验证：`cargo test -p zcv-editor --lib`（285）覆盖折叠/展开、折叠内编辑、行移动与暂存 hunk；字节编辑映射与正反坐标一致。

#### D-D（中）异步 wrap 完成由 observe 回调直接推进快照，与 `snapshot→sync` 双路径

> 状态：已修正（`9bb9587d`）。

- 位置：`zcv-editor/src/display_map.rs:766-778`（observe 回调内 `commit_snapshot`）、`display_map.rs:795-813` 与 `1088-1113`（`snapshot→sync→commit_snapshot`）。
- 证据：`DisplayMap::new` 的 `cx.observe(&wrap_map, …)` 在回调里直接取 `take_edits_since_sync` 并 `commit_snapshot`；`snapshot()` 经 `sync` 也 `commit_snapshot`。Zed `crates/editor/src/display_map.rs:394` 的 observe 只 `cx.notify()`，Block 重建只在 `snapshot()` 的 `block_map.read` 路径发生。
- 违反不变量：D-5「同步只有一个入口」、13.2「同一状态不得同时由事件回调和显式刷新推进」。
- 影响：异步换行完成后由观察者即时重建块投影，与 Editor 的 `advance_snapshots` 并行，推进职责不唯一；难以证明显示版本只推进一次。
- 目标边界：observe 只置 pending/通知；快照推进统一在 `snapshot()→sync`，`WrapEdit` 由 sync 单一消费。
- 迁移项：`display_map.rs`、`wrap_map` 观察路径、显示同步测试。
- 定向验证：后台落地后显示版本只推进一次。

#### D-E（低，待验证）显示坐标缓存只按文本版本失效

> 状态：待验证，未处理。

- 位置：`zcv-editor/src/view/mod.rs:960-987`（`LineWidthCache` 仅比较 buffer 版本、row、字体）。
- 说明：折叠、换行、block 结构变化可能不改变文本版本，却改变 display row 内容；若缓存命中旧宽度，属 R-3 缺口。是否可实际观察未确认。

### 4.3 交互层

#### E-A（中）选择变更存在多个直接写点，未统一唯一入口；相邻选择被错误合并

> 状态：已修正（`4b377820 统一选择变更入口与合并规则`）。

- 位置：`zcv-editor/src/view/mod.rs:2050-2069`（`move_selections` 直接写 `self.selections`）、`view/mod.rs:2478-2491`（`SelectSmallerSyntaxNode` 直接恢复）、`view/mod.rs:1042-1056`（唯一清理入口）、`view/input.rs:456-461, 644-656`、`view/editing.rs:398-437`、`selection/selection_set.rs:186-201`（合并判定 `194`）。
- 证据：
  - 只有 `set_selections_without_clearing_structured_history` 清 `pending_selection` 再写 `self.selections`；`move_selections` 直接 `self.selections = …`，只清 structured，不清 pending；`SelectSmallerSyntaxNode`、自动闭合配对扩展、IME 落点同样直接写。
  - `replay_history`（undo/redo）直接恢复 `self.selections`，`synchronize_after_history_edit` 不清 structured。
  - 合并判定 `if current.end() >= selection.start()` 把首尾相接的非空选区 `[0,5)` 与 `[5,10)` 合并为 `[0,10)`；9.3/E-8 规定「仅仅相邻不合并」。
- 违反不变量：E-8「所有选择变更经唯一入口；锚定吸附规则与合并规则固定」，并波及 E-2（选择权威状态分叉）与 E-4。
- 影响：鼠标拖拽过程中按方向键/undo/IME，`pending_selection` 不终止，迟到的 `update_selection` 会用旧锚点复活选区；undo 后 structured 链残留，`SelectSmallerSyntaxNode` 会恢复撤销前选区；多选区被错误吞并。
- 目标边界：所有选择写点（含失败恢复与历史回放）迁移到唯一入口；合并条件区分「重叠/包含/光标贴边」与「仅相邻」。
- 迁移项：`view/mod.rs`、`view/editing.rs`、`view/input.rs`、`selection/selection_set.rs`；`mouse_selection_tests.rs`、`actions_tests.rs`、`ime_tests.rs`、`selection_set_tests.rs`。
- 定向验证：`begin_selection → update_selection → 方向键 → update_selection` 断言为移动后光标；undo 后 `SelectSmallerSyntaxNode` 不恢复过期选区；`[0,5)`+`[5,10)` 不合并。

#### E-B（中）搜索命中与自动闭合区域用 `zcv_text::Anchor` 承载组合坐标并按裸 offset 消费

> 状态：已修正（`96e9304d 搜索与自动闭合改用组合锚点`）。

- 位置：`zcv-editor/src/view/search.rs:26-48, 98-104, 350-360, 389-391`、`zcv-editor/src/view/input.rs:28-36, 356-368, 388-404`、`zcv-editor/src/view/mod.rs:1868-1896`。
- 证据：`SearchMatchAnchor.range: Range<zcv_text::Anchor>`；`from_range(version, range: MultiBufferRange)` 用 `Anchor::new(BufferGeneration::INITIAL, version, range.start().into())` 构造，`range()` 直接读 `offset()` 当组合偏移，从不 `resolve_in` 目标快照。`AutocloseRegion.range: Range<Anchor>` 同样用 `Anchor::range_outside(BufferGeneration::INITIAL, MultiBuffer version, …)`；自动闭合区域会经 `map_through_position_map` 推进，但搜索命中不会。组合长期位置的正确类型是 `MultiBufferAnchor`（绑定 excerpt 身份 + 源 Anchor）。
- 违反不变量：T-3、T-8、E-4、4.1、8.5（搜索命中应为「领域键 + 组合锚点范围」）。
- 影响：单文件时组合偏移等于源偏移掩盖问题；多 excerpt（搜索、diff）时为组合偏移，`is_stale` 只比较文本版本；组合拓扑在文本版本不变时重建（例如 diff 展开/折叠）不会使命中失效，高亮/跳转可能落到错误位置；reset/基线替换的代际语义不参与。
- 目标边界：命中与自动闭合长期态改用 `MultiBufferAnchor`，消费时经 `MultiBufferSnapshot::resolve_anchor` 解析，显示装饰输入来自解析结果。
- 迁移项：`view/search.rs`、`view/input.rs`、`view/mod.rs`（`update_autoclose_regions_with`）；`search_tests.rs`、`auto_pair_tests.rs`、`ime_tests.rs`。
- 定向验证：多 excerpt + diff 展开场景断言高亮与自动闭合区域仍指向正确源文本；reset 后搜索高亮不静默错位。

#### E-C（中低）滚动条标记后台结果安装前不校验显示版本

> 状态：已修正（`a4e58d57 滚动条标记安装前校验显示版本`）。

- 位置：`zcv-editor/src/scrollbar.rs:58-91`、`zcv-editor/src/view/mod.rs:924-958, 2113`。
- 证据：后台任务捕获 `display_snapshot.clone()` 计算，完成后 `finish_refresh(track_bounds.size, groups)` 直接安装，没有与当前 `cached_snapshot` 版本比较；`invalidate` 只置 dirty。13.3 要求「后台结果安装前必须校验版本；过期结果必须丢弃」。
- 影响：后台计算期间显示版本推进时，旧版本标记被安装并至少显示一帧；可恢复的短暂显示错误，无长期分叉。
- 目标边界：后台任务回传计算所用显示版本，安装前与当前快照版本比较，过期丢弃。
- 迁移项：`scrollbar.rs`、`view/mod.rs`；`scrollbar_tests.rs`。
- 定向验证：模拟版本推进后调用安装，断言过期标记不安装。

#### E-D（低）`SelectionHistory` 只增不减，失败会话留下孤儿记录

> 状态：待处理。

- 位置：`zcv-editor/src/selection/state.rs:299-335`、`zcv-editor/src/view/mod.rs:1727-1769`。
- 证据：`SelectionHistory` 只有 `insert_transaction`/`transaction_mut`/`remove_transaction`；只有成功且合并时删除；两条失败分支只 `end_transaction` 不删本会话记录。文本层历史按预算裁剪，选择历史不受同一生命周期约束。
- 影响：选择历史随编辑线性增长，与文本历史节点脱节。
- 目标边界：选择历史随文本历史节点失效同步清理；失败会话结束即删除自身记录。
- 定向验证：大量/失败编辑后断言选择历史长度与文本历史节点一致。

#### E-E（低）`BlinkManager` 定时任务不可显式取消

> 状态：待处理。

- 位置：`zcv-editor/src/blink_manager.rs:44-77, 100-107`。
- 证据：`pause_blinking`/`blink_cursors` 均 `cx.spawn(...).detach()`，不保留 `Task`；`disable` 只置 `enabled=false`，在途 timer 仍运行到下次回调。
- 违反不变量：13.3「后台任务必须可取消，并与拥有它的实体同生命周期」。
- 影响：每次输入产生一个在途 timer；实体销毁后靠 `this.update` 失败停止。可测量资源滞留未确认。
- 目标边界：持有 `Task`，随实体/禁用取消。

#### E-F（低，待验证）`Editor` 持有第二个 `DisplayMap`（placeholder）

> 状态：待验证，未处理。

- 位置：`zcv-editor/src/view/mod.rs:256, 779-798, 802-816`、`element.rs:1236-1237`。
- 说明：`placeholder_display_map: Option<Entity<DisplayMap>>` 携带独立 `Buffer`，空文档时接入渲染。它不是同一文档的影子权威，属「复用真实渲染管线」的实现选择；是否违反 E-1 未确认，列入第 5 节。

### 4.4 语言层

#### L-A（中）未加载注入语言被静默丢弃，无 Pending 层与注册表版本补解析

> 状态：已按产品范围裁剪（`b6fe41ef 注入层改用锚点范围并保留待处理层`）。
> 已删除静默 `continue`，未注册注入语言保留为 `SyntaxLayerContent::Pending { language_name }`，范围用锚点保存。
> **已决策不引入运行期语言注册与注册表版本补解析**：Zcv 的 `LanguageRegistry` 由 `builtin_languages()` 在构造期静态装配、没有运行期注册入口，`language_for_injection` 对可匹配名必然同步加载并返回 `Some`，该机制当前没有触发路径；审计点名的 `graphql`/`glsl`/`wgsl`/`latex`/`phpdoc` 在 Zcv 也没有打包 grammar。此项登记为产品范围裁剪（见第 7.1 节），不再作为待修正项；将来若引入运行期语言注册，`Pending` 层即为补解析接入点。

- 位置：`zcv-language/src/syntax_map.rs:684-689`（`continue` 丢弃点）、`syntax_map.rs:109-115`（`SyntaxLayer` 无 Pending）、`syntax_map.rs:24-32, 76-84`（无注册表版本）、`zcv-language/src/registry.rs:196-199`（无版本号）。
- 证据：注入收集遇到 `registry.language_for_injection(&language_name)` 为 `None` 时直接 `continue`，既不保留待解析层也不记录语言名。树内查询文件引用了未注册语言名（如 `graphql`、`glsl`、`wgsl`、`latex`、`phpdoc` 等），全部被静默吞掉。Zed `crates/language/src/syntax_map.rs:193-235` 用 `SyntaxLayerContent::{Parsed, Pending}` 保留待解析层，`SyntaxSnapshot` 带 `language_registry_version`，注册表变化时补解析。
- 违反不变量：L-8、6.3、6.6。
- 影响：未内置语言的围栏代码块/模板字符串没有高亮与语法节点，且当前没有恢复路径；丢失 L-8 的待解析层与 6.6 的注册表版本驱动补解析。
- 目标边界：层内容增加 `Pending` 变体并保存语言名与范围；`LanguageRegistry` 增加单调版本；`SyntaxMap/SyntaxSnapshot` 记录并比较注册表版本；注册表变化时补解析；删除静默 `continue`。
- 迁移项：`syntax_map.rs`、`registry.rs`、`language.rs`、`language_buffer.rs`；`syntax_map_tests.rs`、`registry_tests.rs`。
- 定向验证：注入一段未注册语言，断言存在 Pending 层；注册表解析该名并推进版本后该层变为 Parsed 且产出高亮。

#### L-B（中）注入层范围为裸 `Range<usize>`；`edits_since` 失败时整层丢弃或全文失效

> 状态：已修正（`b6fe41ef 注入层改用锚点范围并保留待处理层`）。

- 位置：`zcv-language/src/syntax_map.rs:113`、`syntax_map.rs:227-247`、`syntax_map.rs:361-369`、`syntax_map.rs:419-421`。
- 证据：`SyntaxLayer.range: Range<usize>`，插值时 `map_range_through_changes` 手工映射；当 `edits_since` 取不到（编辑日志被裁剪或合并为 reset）时走 `else` 分支丢弃全部注入树（`242-247`）。重解析时 `snapshot.edits_since(self.parsed_version).ok()...unwrap_or_else(|| 一次全文区间)`（`364-368`）。同层折叠候选已使用 `Range<Anchor>`，注入层却退回裸偏移。
- 违反不变量：6.3「语法层的范围用 `Anchor` 保存」；增量入口不得静默回退全量。
- 影响：大文件/长编辑会话（超过 `max_edit_history_entries` 或 reset）后，所有注入层被丢弃并全文重跑注入查询与解析；手写偏移映射在编辑边界存在与 Anchor 亲和性语义漂移风险。
- 目标边界：注入层范围改为 `Range<Anchor>`，查询时才解析为字节范围；删除失败即整层丢弃与全文兜底，只保留命名清楚的 reset 边界；文本层需为语言层提供锚点化编辑区间。
- 迁移项：`syntax_map.rs`、`tree_sitter_utils.rs`，必要时 `zcv-text` 暴露锚点化 `edits_since`；`syntax_map_tests.rs`。
- 定向验证：超过编辑日志预算后触发裁剪，断言未受影响注入层不失效、范围坐标正确。

#### L-C（低，已登记 R5）`LanguageSettings` 只覆盖 tab，软换行/目标行宽仍全局

- 位置：`zcv-language/src/language_settings.rs:12-25`、`zcv-editor/src/view/mod.rs:1609-1637, 1655-1662`。
- 证据：`LanguageSettings { tab: TabConfig }` 只有 tab；`resolve` 只取 `tab_for_language`。`Editor` 的 `soft_wrap`/`preferred_line_length` 在构造与全局设置观察中直接从 `SettingsStore` 读取。
- 违反不变量：L-6、6.5（tab 宽度、软换行、preferred line length 等按语言解析并由快照下发）。
- 处置：迁移计划阶段 7 R5 已登记「无消费方，保持现状」。本次复核仍存在，按已登记项处理。

### 4.5 文本事实层

#### T-A（中）T-7 历史可见性、当前→旧版本映射与历史文本重建能力未提供

> 状态：待处理。

- 位置：`zcv-text/src/snapshot.rs:104-120`（只有 `edits_since`/`edits_since_in_range`）、`zcv-text/src/tracking/edit_log.rs:107-169`、`zcv-text/src/position_map.rs:110-350`（只有 old→new）。
- 证据：`Snapshot` 没有 `has_edits_since(_in_range)`；`PositionMap` 没有 new→old（`range_to_version`/`offsets_to_version` 等价物）；没有按版本重建文本的入口。Zed 原型有 `rope_for_version`、`has_edits_since(_in_range)`、`range_to_version`、`offsets_to_version`。
- 违反不变量：T-7（架构文档 5.3 明确要求「历史可见性、任意跨度净编辑、历史文本重建、把当前坐标映射回旧版本」）。
- 影响：无法判断「某段文本在某历史版本是否可见」，无法把当前坐标映射回旧版本，上层做历史对比只能整份物化。当前 diff 全文物化与 Git 只读预览属已登记实现差异，可暂缓，但契约未落地。
- 目标边界：能力归 `Snapshot`（唯一读取边界），编辑事实由 `EditLog` 的版本区间提供；不得在 `MultiBuffer`/`zcv-buffer-diff` 各自重建历史文本。
- 迁移项：`zcv-language/src/syntax_map.rs:208,365`、`zcv-buffer-diff`、`zcv-project` GitStore、`zcv-text/tests/versioned_edits_anchor.rs`、`zcv-text/README.md`；应登记进第 18.1 节「尚未复刻」。
- 定向验证：`has_edits_since(_in_range)` 真值；`offsets_to_version` 与 `edits_since` 互逆；按旧版本重建文本；对照 Zed 同用例。

#### T-B（中）无 T-9「基线派生快照 + 版本校验后原子安装」的文本层入口

> 状态：待处理。

- 位置：`zcv-text/src/snapshot.rs:35-224`（`Snapshot` 只读、无派生/安装接口）。
- 证据：全仓无 `snapshot_with_edits`/`fast_forward`/`EditedBufferSnapshot` 等价能力。Zed 对应 `Buffer::snapshot_with_edits`、`Buffer::fast_forward`、`EditedBufferSnapshot`。
- 违反不变量：T-9。
- 影响：Git 修订文本、diff 基线这类「稳定基线上计算」的场景缺文本层规范入口，只能整份物化或上层自建，缺版本校验与唯一安装点。是否已由 `zcv-language`/`zcv-multi-buffer` 以其他形态承接未确认。
- 目标边界：派生在快照副本上完成；安装前校验基准版本一致，过期结果丢弃；唯一安装入口，与订阅/历史解耦。
- 定向验证：基线上应用编辑得到派生快照，主文档未变时安装成功；主文档前进后安装被版本校验拒绝。

### 4.6 工程边界层

#### E-G（中）项目搜索自建 Buffer 并注册为该路径权威文档，绕过文件解码边界

> 状态：待处理。

- 位置：`zcv-project/src/search/mod.rs:157-172`（`std::fs::read_to_string` 与 `Buffer::from_text`）、`zcv-search/src/project_search.rs:299-307`（`register_loaded_buffer`）、`zcv-project/src/buffer_store.rs:70-102`。
- 证据：搜索遇到未打开文件时执行 `std::fs::read_to_string(path)` 并 `Buffer::from_text(text, BufferConfig::default())`，结果携带 `loaded_buffer`；UI 线程把它经 `Project::register_loaded_buffer` 登记进 `BufferStore`，成为该路径后续复用的权威文档。而 `Project` 文件边界默认解码是 `EncodingConfig::default()` 的 `BomPolicy::Strip`（`zcv-project/src/text_file.rs:51-58`），`read_to_string` 不剥离 BOM。
- 违反不变量：P-1（同一路径只有一个权威文档实体，含内容语义）、11.1（解码与 Buffer 创建属 Project 文件边界）、3.1。
- 影响：带 BOM 的文件若先被项目搜索读到、再被编辑器打开，编辑器沿用的 Buffer 首字符为 U+FEFF；反之先打开则被剥离。同一路径的权威文本内容取决于哪条入口先物化它；未来编码/非法 UTF-8 策略变化也会漏过搜索路径。
- 目标边界：文件解码与 Buffer 创建只有唯一入口（Project 文件边界 + `decode_to_string`）；搜索不再自行建 Buffer，而经 Project 请求「按路径加载并复用文档」；删除 `FileSearchResult::loaded_buffer` 旁路。
- 迁移项：`zcv-project/src/search/mod.rs`、`project_store.rs::search`、`zcv-search/src/project_search.rs`、`zcv-project/src/test/search_tests.rs`。
- 定向验证：写一个带 BOM、未被打开的文件；让 `Project::search` 成为该路径首个物化者；断言返回文档首字符无 U+FEFF，且与 `Project::open_buffer` 首次打开的字节一致。

#### E-H（低）`Item`/`ItemHandle` 能力标志超出 11.2 的「只通过 show_toolbar 与面包屑数据」

> 状态：待处理。

- 位置：`zcv-workspace/src/item.rs:51-65, 176-178, 247-253`；消费方 `zcv-search/src/buffer_search.rs:92`、`zcv-version-control/src/editor_diff.rs:32`。
- 证据：`Item`/`ItemHandle` 暴露 `uses_editor_document_toolbar`（决定通用文档工具栏位置）与 `receives_git_projection`（决定是否注入 git 投影），超出 11.2 描述的能力面。
- 影响：新增文档形态需追加 capability 标志，工具区/投影注册规则分裂在 Item 与注册方之间；不涉及所有权转移。
- 目标边界：工具项/投影注册方自行按 Item 类型或明确能力决定；Item 只保留 `show_toolbar` 与面包屑；若保留则应在架构文档中登记为显式实现差异。
- 迁移项：`zcv-workspace/src/item.rs`、`zcv-search/src/buffer_search.rs`、`zcv-version-control/src/editor_diff.rs` 及相关测试。

---

## 5. 待验证线索

以下条目有代码证据但未能证明实际用户影响或可达性，不作为已确认差距；需定向测试或运行时复现后再决定是否登记。

1. `SelectionHistory` 只增不减与失败会话孤儿（E-D）：未量化长期内存影响。
2. `BlinkManager` detach 任务（E-E）：未量化资源滞留。
3. `LineWidthCache` 仅按文本版本失效（D-E）：未复现折叠/换行结构变化命中旧宽度。
4. `Editor` 第二个 `DisplayMap`（placeholder）（E-F）：未判定是否违反 E-1。
5. `GitStore` 的 `optimistic_index_bases: HashMap<AbsolutePathBuf, Arc<str>>`（`zcv-project/src/git_store/mod.rs:270-271, 629-668`）与 `revision_documents` 中同一 index 文本并存，构成第二份 index 文本；是否属「事务回滚基线」而非影子权威未确认。
6. `no-op` 编辑仍推进版本、生成历史节点并发布事件（`zcv-text/src/buffer/edit_ops/mod.rs:5` 的声明与实现不符）；Zed 的 `apply_edit_internal` 同样保留空操作，因此版本推进本身不算原型偏离，只有注释与实现不一致需修正。
7. `MergeWithPrevious` 跨过未记录历史的编辑可能使 undo 命中 `undo: None` 的编辑日志条目并报 `InvariantViolation`（`zcv-text/src/tracking/edit_log.rs:142-159`）；纯静态推断，需运行复现。
8. 结构变更入口未断言文本事务之外（C-E）；未找到实际在事务内触发的调用方。
9. 注入层裸偏移 + 手工映射在具体编辑序列下是否产生错误区间（D-C/L-B）：L-B 已改用 `Range<Anchor>`，增量改由 `zcv-text::Snapshot::coordinate_edits_since`（不衰减坐标索引）提供，`map_range_through_changes` 与 `edits_since` 失败时的全文兜底已删除，此线索所指「裸偏移手工映射」已消除；D-C 已改为字节偏移变换，不再保留行数守恒锚点。

---

## 6. 已登记暂不处理项的复核

以下项在 [docs/架构决策记录.md](docs/架构决策记录.md) 第 6 条或迁移计划中已登记，本次复核确认仍存在，不计为新差距。

| 项 | 内容 | 证据 | 状态 |
| --- | --- | --- | --- |
| A11 | 移除 `SettingsStore::try_get` 的生产回退 | `zcv-editor/src/view/input.rs:213`、`zcv-editor/src/view/mod.rs:1609, 1656`、`zcv-editor/src/status_items.rs:76`、`zcv-language/src/language_settings.rs:21` | 仍存在，暂不处理 |
| C6 | 收窄 `zcv-project` 仅测试构造的编码配置生产面 | `zcv-project/src/text_file.rs` 的 `BomPolicy/EncodingConfig/InvalidUtf8Policy/LineEndingConfig` | 仍存在，暂不处理 |
| F-14 | 拆分 `zcv-multi-buffer/src/multi_buffer.rs` 的混合职责 | `multi_buffer.rs` 同时承载 excerpt 布局、diff 投影、事务历史、订阅 | 仍存在，暂不处理 |
| R5 | `LanguageSettings` 未覆盖软换行/auto_indent 等按语言设置 | `zcv-language/src/language_settings.rs:12-25` | 仍存在，暂不处理 |

---

## 7. 合理裁剪与实现差异（非差距）

### 7.1 已声明裁剪

- 实时协作、远程开发、LSP 及其专属基础设施（架构文档第 17 节）：Zcv 无 CRDT 副本、协作者选择、远程序列化、语言服务器客户端/诊断/semantic token/hover/code action。
- InlayMap：Zed 的 `crates/editor/src/display_map/inlay_map.rs` 对应 LSP inlay hints，Zcv 显示链为 `MultiBufferSnapshot → FoldMap → TabMap → WrapMap → BlockMap`，属对已排除能力的裁剪。
- `zcv-text` 本地 rope 存储与本地单调版本，而非 CRDT 片段树与多副本向量；保留版本、Anchor、增量与历史查询契约。
- 语言智能以 Tree-sitter 与 `.scm` 查询为边界，未建立 LSP 兼容层。
- 语言注入补解析：Zcv 保留 `SyntaxLayerContent::Pending`（未注册注入语言不再静默丢弃），但**不引入** Zed 的 `LanguageRegistry` 单调版本与注册表变化后补解析。Zcv 注册表在构造期静态装配、没有运行期注册入口，未内置语言没有可注册路径，该机制在当前范围内没有触发点。
- `BufferStore` 只做本地路径索引，`GitStore` 只做本地修订文本，不含远程协商与传输。

### 7.2 语义一致的实现差异

- 文本存储用 rope + 两阶段提交；编辑日志与不衰减坐标索引分离。
- 后台 diff 计算物化全文（`zcv-buffer-diff/src/buffer_diff.rs:451` 的 `full_text`），不在编辑或显示热路径；架构文档 18.4 已登记。
- `Editor` 用 `change`/`change_with_after`/`change_with_after_post` 多入口表达不同编辑语义，全部收敛到 `commit_session`，语义等价；18.4 已登记。
- `Editor` 与 `EditorElement` 同 crate 互相引用；渲染不写回文档/选择/显示权威，几何缓存只写回 Editor 缓存字段。
- `zcv-editor` 依赖 `zcv-project`/`zcv-workspace`：与 Zed `crates/editor` 依赖 `project`/`workspace` 一致，属允许的集成关系；架构文档 14.1 的箭头图对此表达较含糊，建议后续在文档中明确「编辑器核心依赖 Project/Workspace 的 Item/Provider 协议」。
- 坐标 newtype 的具体命名、高亮缓存淘汰策略、异步 wrap 预算值为实现细节。

---

## 8. 与架构文档第 18 节的回写建议

1. 第 18.2 节不能继续为空。已修正项（M-A、M-B、D-A、D-B、D-D、E-A、E-B、E-C、L-B、C-C、C-D）按目标落地后不必再登记为偏离；仍需登记的是待处理项 E-G、E-H、T-A、T-B、E-D、E-E，以及一项有证据的保留偏离：
   - D-C：已把 fold 变换树从行粒度改为 Zed 的字节偏移文本变换（`Transform { input/output: MBTextSummary }` + `FoldEdit = Edit<FoldOffset>`），`global_delta` 补齐已删除；见第 4.2 节状态行。

   L-A 的注册表版本补解析登记为产品裁剪（第 7.1 节），不列入 18.2 的复刻偏离。
2. 第 18.1 节「尚未复刻」补充 T-A（T-7 历史查询与历史文本重建）与 T-B（T-9 派生快照入口），并注明当前无直接消费方。
3. 第 18.4 节实现差异中，把「Editor 持有第二个 DisplayMap（placeholder）」按结论补登记或消除；把 14.1 依赖箭头与 Zed 实际依赖（editor → project/workspace）对齐说明。
4. 消除第 6 节 A11/C6/F-14/R5 中的任一项后，同步更新决策记录，而不是只改本审计。

---

## 9. 验证状态与未覆盖边界

- 本次为只读静态审计，未运行 `cargo check`、`cargo test`、基准或真实 UI/平台运行时验证；所有「定向验证」均为设计，未执行。
- 对齐落地阶段（1–10）已执行：`cargo test -p zcv-editor --lib` 285/285、`cargo test -p zcv-language --lib` 82/82、`cargo test -p zcv-multi-buffer --lib` 68/68、`cargo test -p zcv-text --lib` 32/32；`cargo clippy -p zcv-editor -p zcv-language -p zcv-multi-buffer -p zcv-text --all-targets -- -D warnings` 干净；`cargo fmt --all -- --check` 干净。真实 UI/平台运行时仍未被本审计执行。
- 已知未通过：`zcv-text/tests/versioned_edits_anchor.rs::explicit_rebase_maps_an_old_anchor_through_a_reset` 在改动前即失败（reset 后显式重锚返回 `0` 而非 `3`），本轮未修复。
- 已逐文件核对：`zcv-text` 的 buffer/tracking/transaction/snapshot/text_changes/history；`zcv-language` 的 language_buffer/syntax_map/registry/language_settings/queries；`zcv-multi-buffer` 与 `zcv-buffer-diff`；`zcv-editor` 的 display_map 各层、view、selection、scrollbar、blink、element 关键区段；`zcv-project` 的 buffer_store/text_file/project_store/search/git_store；`zcv-workspace` 的 item/pane/toolbar/workspace_state；Zed 的 `text`、`language`、`multi_buffer`、`editor/display_map`、Cargo 依赖。
- 未逐行核对：`zcv-editor/src/element.rs` 全文、`zcv-editor/src/display_map/chunk.rs` 与 `decorations.rs` 全部渲染内部、`zcv-editor` 除 display_map 外的锚点解析调用方、`zcv-version-control`/`zcv-preview-*` 内部、`zcv-git` 与 `git_store/background.rs`/`jobs.rs` 全文。
- 运行时影响复现状态：M-A、M-B、D-A、D-B、D-D、E-A、E-B、E-C、L-B、C-C、C-D 已由各阶段回归测试覆盖（见第 2.2 节状态列的提交）；L-A 的未注册注入语言高亮、E-G 的 BOM 分叉仍未做运行时复现。D-C 的字节偏移映射由「暂存 hunk」等结构编辑回归覆盖，行数守恒断言由精确字节映射自然成立。
