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
>
> **复审记录（2026-09-21，Zcv 工作树基线 `660fca54` + 暂存未提交改动；原型 Zed `bcf6582c`）**：本轮独立只读复审确认 12 条修正仍成立、6 条待处理仍存在。
> M-B 的修正方式更新为「代际机制整体删除，`Anchor` 只沿单一版本化坐标链解析」；D-A 降级为部分修正。
> 新增 D-F（Block 快速路径未覆盖 `show_headers`）、D-G（Block 层缺 input 覆盖与越界断言）与契约层 H-1；H-1 已按「以 Zed 原型为准」在 [docs/编辑器架构.md](docs/编辑器架构.md) §4.2/T-8 修正（删除 Zed 不存在的代际 / reset / 显式重锚）。
> 关闭第 5 节两条待验证线索（GitStore `optimistic_index_bases`、`explicit_rebase` 用例），并补记已落地的 BufferId / ExcerptBoundary / D-13 工作。
>
> **闭环记录**：第 2.2 节 25 条已确认差距全部落地——本轮修正 D-A 保留项、D-F、D-G、R-A…R-D、E-D、E-E、T-A、T-B、E-G、E-H；第 5 节 D-E 线索确认并修正、E-F 判定不构成 E-1 违反；架构文档 §18.2 随之清空。验证见第 9 节。
>
> **复审记录（渲染层目标确认为 Zed 架构后，本轮）**：独立只读复审确认 25 条闭环仍成立。新增 1 条渲染层复刻缺口 **R-E**（行内元素实测宽度未回写显示层、`ChunkRendererId` 从渲染片段丢失，违反 R-9）。同时把两项通用渲染能力登记为架构文档 §18.1「尚未复刻」：字体回退下的行内列位置（Zed `font_id_for_index`）、完整 invisible/whitespace 策略（Zed `Invisible`）；LSP 诊断绘制（diagnostic underline、point diagnostics）归入 §17 裁剪。
>
> **复审记录（diff 投影推进入口，本轮）**：从失败回归 `staging_one_hunk_rebuilds_the_projection_once_after_refresh`（暂存一个 hunk 后投影版本推进两次）出发，确认组合文档层新增差距 **M-C**：旧侧删除 hunk 以基线修订 Buffer 作为组合 excerpt 源，基线文本变化与 `BufferDiff` 结果并列推进同一 hunk 几何，违反单入口。已按 Zed 单入口结构性修正：diff 基线/参照源不进入组合源订阅表（`apply_source_change` 对其没有调用路径），改由 `DiffState` 直接订阅 base/index 触发 `BufferDiff` 重算，组合投影只由 `diff_changed` 推进；回归恢复原断言并通过。删除 hunk 已进一步对齐 Zed 数据模型：只由输出 `DiffTransform::DeletedHunk` 承载并自带基线文本，组合 excerpt 树只含工作区文档，输入/输出坐标彻底分离；§18.4 的原实现差异随之删除。
>
> **复审记录（2026-09-21，Zcv 工作树基线 `0cb6630f`，工作树干净；原型 Zed `bcf6582c`，独立只读复审）**：本轮对第 2.2 节全部条目与第 5 节线索在当前 HEAD 上独立复核，并由分层子审计逐层取证。
> 已确认全部已落地条目仍成立：M-A、M-B、M-C、C-C、C-D、T-A（能力层面）、T-B、D-B…D-G、E-A…E-H、R-A…R-D；定向 `cargo test` 通过（`zcv-editor --lib` 296、`zcv-multi-buffer --lib` 68、`zcv-language --lib` 82、`zcv-text --lib` 35），`cargo check --workspace --all-targets` 通过。
> 本轮修正两条状态：**D-A 降为 ⚠️ 部分修正**（excerpt 边界变化与 wrap 编辑分支只复用前缀，仍保留两处条件性从投影起点重建），**L-B 降为 ⚠️ 部分修正**（锚点范围与坐标索引增量已落地，但 `coordinate_edits_since` 为 `None` 时仍无命名地整层丢弃并全文重跑）。
> 本轮新增 12 条已确认代码差距：T-C、T-D、M-D、M-E、M-F、D-H、E-I、E-J、E-K、R-F、R-G、R-H；R-E 仍待处理。新增 4 条契约层条目 H-2…H-5（见 §2.5、§4.7）。
> §5 线索 7 升级为 T-C；线索 6、C-E 与若干新线索继续保留。本轮为只读审计，新增差距均未做运行时复现，验证见第 9 节。
>
> **对齐落地（D-A、L-B，本轮）**：两条已按 Zed 实现彻底对齐。
> D-A：`BlockSnapshot::sync` 统一为「公共前缀 + 受影响区间 + 公共后缀」——结构变化按锚点表等价性定位后缀，纯换行编辑取所有编辑之后的尾部（整体平移、块序列与相对间距不变）直接追加旧变换子树；删除 `None` suffix 与两处从投影起点重建；锚点重定位使旧前缀越过新块起点时只回退前缀边界，不整份重建。折叠不合成 `WrapEdit`：它是块层策略变化，由 `BlockSnapshot::sync` 依据 `folded_buffers` 重算并推进独立的块几何代际；diff 装饰缓存改为按块几何代际判据复用，不再用 `wrap_edits.is_empty()` 代理显示几何（同时彻底修正 D-H）。块结构变化的权威信号是组合投影版本 `MultiBufferSnapshot::version()`，删除了手写的 excerpt 边界签名代理：此前 diff 编辑改变了 excerpt 范围但边界签名不变时，块层会复用过期锚点，使分隔线落到错误位置——这与「折叠伪装成换行编辑」是同一类「用派生代理代替下层权威信号」的偏差。
> L-B：`interpolate`/`reparse` 删除 `coordinate_edits_since` 为 `None` 时丢弃全部语法状态并全文重跑的分支，改由 `coordinate_edits_or_fail` 在不变量破坏时显式失败；首次解析与语言切换仍按显式边界做全文收集。
> 验证：`cargo check -p zcv-editor -p zcv-language --all-targets` 与 `cargo check --workspace --all-targets` 通过；`cargo test -p zcv-editor --lib` 296/296、`zcv-language --lib` 82/82、`zcv-multi-buffer --lib` 68/68、`zcv-version-control --lib` 55/55；`cargo fmt --all -- --check` 与 `cargo clippy -p zcv-editor -p zcv-language --all-targets --no-deps -- -D warnings` 干净。

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
- 第 18.2 节「复刻偏离」曾登记为空这一事实无法维持：本次确认存在若干复刻偏离，集中在组合文档增量回退、跨代际锚点解析、显示层 block 增量协议、显示热路径物化、选择唯一入口、语言注入待解析层、组合坐标类型误用、项目搜索旁路物化，以及渲染层的行片段模型缺失与行内替换描述缺失（R-A…R-D）。上述条目已在本轮按目标全部修正，18.2 节随之清空；本轮渲染层复审新增 R-E，18.2 节重新登记该条。
- 其余差异属于两类允许偏离：已声明裁剪（协作/远程/LSP 生产者；InlayMap 只裁剪 inlay 生产者，其共享的行内替换渲染底座已纳入对齐）或语义一致的实现技术差异；已登记暂不处理项（A11、C6、F-14、R5）仍然存在但不改变数据流与所有权。
- 对齐进度：第 2.2 节原有 25 条已确认差距已全部闭环（本轮修正 E-G、E-H、D-A 保留项、D-F、D-G、R-A…R-D、E-D、E-E、T-A、T-B；1 条登记为产品裁剪 L-A）。本轮复审新增两条：渲染层 R-E（待处理），以及组合文档层 M-C（删除 hunk 双推进入口，已按单入口修正，并进一步使删除 hunk 只驻输出变换树）。已落地状态见第 2.2 节「状态」列与第 4 节各条状态行。
- 本轮复审新增 D-F、D-G 两条低严重度保留项与契约层条目 H-1；H-1 已按 Zed 原型在架构文档 §4.2/T-8 修正（见第 2.5、4.7 节）。
- 渲染层补审（本轮，架构文档新增 §10.2/§10.4 目标后）：对齐目标此前只覆盖「渲染不越权」（R-1..R-4），未覆盖行布局内部模型；新增 R-A…R-D 四条渲染层差距，见第 3.5、4.8 节。
- 渲染层目标确认（前轮复审）：架构文档 §10.1 明确 `EditorElement` 按 Zed `crates/editor/src/element.rs` 复刻。R-A…R-D 已闭环；新增 R-E（元素实测宽度未回写显示层），并登记两项尚未复刻的通用渲染能力与一项裁剪，见第 4.8 节与架构文档 §18.1/§17。
- 本轮独立复审（`0cb6630f`）：已在代码层确认全部落地条目；D-A、L-B 降为部分修正，新增 12 条代码差距与契约条目 H-2…H-5。最严重的新差距是显示投影层 D-H：diff 装饰缓存键用 `wrap_edits.is_empty()` 冒充显示几何未变，折叠全部文件后装饰沿用旧显示行；其余集中在文本历史回放（T-C/T-D）、组合投影同步帧的增量协议（M-D）、后台 diff 任务生命周期（M-E/M-F）、折叠命令与自动闭合区域生命周期（E-I/E-J）、水平窗口化渲染（R-F/R-G）。

### 2.2 已确认差距总表

严重度：高＝违反稳定不变量且会产生全量失效、错误位置或状态分叉；中＝违反稳定不变量但当前影响局部或有边界；低＝契约/生命周期缺口，影响有限。

状态：✅ 已修正并提交；⚠️ 部分修正（保留项见第 4 节状态行）；➖ 已登记为产品裁剪（不再作为待修正项）；⬜ 待处理。提交号对应本轮对齐各阶段。

| 编号 | 严重度 | 层 | 一句话 | 违反不变量 | 状态 |
| --- | --- | --- | --- | --- | --- |
| M-A | 高 | 组合文档 | 增量推导 `None` 与多次变化合并被发布为 reset，显示链整链重建 | M-5、2.2、8.4、D-3、D-5 | ✅ `9c7825f1` |
| M-B | 高 | 组合文档 | 通用 `resolve_anchor` 在代际失配时静默 `rebase_across_generations` | T-8、4.2、M-4 | ✅ `a034e79b` 显式化；`660fca54` 删除代际机制，单一版本化坐标链 |
| M-C | 中 | 组合文档 | 展开删除 hunk 以基线修订 Buffer 为 excerpt 源，基线文本变化与 diff 结果构成两个推进入口 | M-2、8.4、D-5、7.5 | ✅（本轮）基线文本只喂 diff；删除 hunk 改为输出 `DiffTransform::DeletedHunk` 自带基线文本，输入 excerpt 树只含工作区文档 |
| D-C | 中 | 显示投影 | Fold/Tab 层 Edit 不是本层坐标类型，靠全局 delta 与裸区间兜底 | D-2、4.1 | ✅（本轮） |
| D-A | 中 | 显示投影 | Block 层无本层 edit 协议，wrap 编辑时整体 `place()`，结构变化全量重建 | D-2、D-5、D-8 | ✅（本轮）公共前缀 + 受影响区间 + 公共后缀；结构变化按锚点表等价性、换行编辑取编辑之后尾部复用旧子树；删除 `None` suffix 与从起点重建 |
| D-B | 中 | 显示投影 | 布局热路径 `row_text` 物化整行，调用方自行剥离 `\n` | D-7、8.2 | ✅ `bd371e8b` |
| D-D | 中 | 显示投影 | 异步 wrap 完成由 observe 回调直接推进快照，与 `snapshot→sync` 双路径 | D-5、13.2 | ✅ `9bb9587d` |
| E-A | 中 | 交互 | 选择变更存在多个直接写点，未统一入口；相邻选择被错误合并 | E-8、9.3 | ✅ `4b377820` |
| E-B | 中 | 交互 | 搜索命中/自动闭合用 `zcv_text::Anchor` 承载组合坐标并直接读 offset | T-3、T-8、E-4、4.1、8.5 | ✅ `96e9304d` |
| E-C | 中低 | 交互 | 滚动条标记后台结果安装前不校验显示版本 | 13.3 | ✅ `a4e58d57` |
| L-A | 中 | 语言 | 未加载注入语言被静默丢弃，无 Pending 层与注册表版本补解析 | L-8、6.3、6.6 | ➖ `Pending` 已表示；注册表版本补解析登记为产品裁剪 |
| L-B | 中 | 语言 | 注入层范围用裸 `Range<usize>`；`edits_since` 失败时整层丢弃/全文失效 | 6.3、增量不得回退 | ✅（本轮）删除 `None` 时整层丢弃/全文重跑；不变量破坏显式失败，首次解析/语言切换按显式边界全文收集 |
| C-C | 中 | 组合文档 | 缺少 `ExcerptOffset` 独立坐标 newtype，源/输入/输出以裸 usize 混算 | 4.1、7.2 | ✅ `（本轮）` |
| E-G | 中 | 工程边界 | 项目搜索 `read_to_string` 自建 Buffer 并注册为该路径权威文档，绕过文件解码边界 | P-1、11.1、3.1 | ✅（本轮） |
| T-A | 中 | 文本事实 | T-7 历史可见性 / 当前→旧版本映射 / 历史文本重建能力未提供 | T-7 | ✅（本轮）`Snapshot` 提供 `has_edits_since(_in_range)` / `offsets_to_version` / `range_to_version` / `text_for_version` |
| C-D | 低中 | 组合文档 | hunk 身份承载在输入 excerpt 树、以 `visible_hunks` 下标为身份 | M-2、7.3 | ✅ `（本轮）` |
| E-D | 低 | 交互 | `SelectionHistory` 只增不减，失败会话留孤儿记录 | E-5 生命周期关联 | ✅（本轮）有界化 + 失败会话清理自身记录 |
| E-E | 低 | 交互 | `BlinkManager` 定时任务 `detach`，不可显式取消 | 13.3 | ✅（本轮）持有 `Task`，禁用即取消 |
| E-H | 低 | 工程边界 | `Item`/`ItemHandle` 暴露 `uses_editor_document_toolbar`、`receives_git_projection`，超出 11.2 | 11.2 | ✅（本轮） |
| T-B | 中 | 文本事实 | 无 T-9「基线派生快照 + 版本校验后原子安装」文本层入口 | T-9 | ✅（本轮）`EditedBufferSnapshot` / `snapshot_with_edits` / `fast_forward` |
| D-F | 低 | 显示投影 | Block 快速路径复用分支未比较 `show_headers` 策略 | D-5、D-13 | ✅（本轮）`show_headers` 纳入结构失效键 |
| D-G | 低 | 显示投影 | Block 层缺变换 input 精确覆盖断言与越界显式失败 | D-8、D-10 | ✅（本轮）input 精确覆盖断言 + 越界显式失败 |
| R-A | 高 | 渲染 | 行布局是单一整行塑形结果，不能承载行内元素 | R-5、R-8 | ✅（本轮）行布局改为有序片段序列，跨片段坐标/命中 |
| R-B | 中 | 渲染 | chunk 流没有行内替换描述与渲染描述 | R-6、R-9 | ✅（本轮）`Chunk.renderer` 携带行内替换描述 |
| R-C | 中 | 渲染 | 折叠占位符没有 `render` 回调，`constrain_width` 未被消费 | R-6 | ✅（本轮）`FoldPlaceholder.render` 注入元素，`constrain_width` 决定可用宽度 |
| R-D | 低 | 渲染 | 行布局可脱离 Element 生命周期计算，元素生命周期契约不存在 | R-7 | ✅（本轮）元素在 `request_layout`/`prepaint` 内测量，行原点确定后 prepaint |
| R-E | 低中 | 渲染 | 行内元素实测宽度未回写显示层，`ChunkRendererId` 从渲染片段丢失 | R-9 | ⬜ |
| T-C | 中 | 文本事实 | `MergeWithPrevious` 可跨 `undo: None` 日志条目合并历史节点，undo 报 `InvariantViolation` 且历史与文本分叉 | T-7、5.2、5.3 | ⬜（本轮新增） |
| T-D | 中 | 文本事实 | undo/redo 回放条目以 `undo: None` 落盘，`text_for_version` 在任意撤销/重做后返回 `HistoryTextUnavailable` | T-7 | ⬜（本轮新增） |
| M-D | 中 | 组合文档 | 投影同步帧丢弃精确源增量，改用树比较推导粗范围；等长替换可能漏更新 | 2.2、7.5、M-A 目标 | ⬜（本轮新增） |
| M-E | 中 | 组合文档 | `BufferDiff` 后台 diff 任务 `detach`、不可取消、无在途去重 | 13.3 | ⬜（本轮新增） |
| M-F | 低 | 组合文档 | diff 结果安装只校验 working 版本，未校验 base/index | 13.3、4.3 | ⬜（本轮新增） |
| D-H | 高 | 显示投影 | diff 装饰缓存键以 `wrap_edits.is_empty()` 冒充显示几何未变；折叠全部文件后装饰沿用旧显示行 | D-4、D-5、8.5 | ✅（本轮）改为按 `BlockSnapshot` 的块几何代际（`geometry_epoch`）失效 |
| E-I | 中 | 交互 | `ToggleFold` 命令使用空 `render` 的默认占位符，折叠区无可见元素；与 crease 点击路径不一致 | R-6、10.2 | ⬜（本轮新增） |
| E-J | 中 | 交互 | `autoclose_regions` 只增不减，输入路径线性扫描无界历史区域 | 9.1、13.2 | ⬜（本轮新增） |
| E-K | 低 | 交互 | `EditorEvent::DiffHunksExpandedChanged` 已声明且被订阅但无生产者 | 9.7、7.4 | ⬜（本轮新增） |
| R-F | 中 | 渲染 | 水平窗口化行的选区/装饰几何把整行列用于窗口内文本，高亮整体偏移 | R-8、10.2、4.1 | ⬜（本轮新增） |
| R-G | 中 | 渲染 | 水平窗口裁剪折叠占位符时静默丢弃 `ChunkRenderer`，回退文本绘制 | R-6、10.2 | ⬜（本轮新增） |
| R-H | 低 | 渲染 | `FoldPlaceholder::ellipsis` 构造时捕获主题色，主题切换后陈旧 | 10.2 | ⬜（本轮新增） |

> 说明：前 25 条已为重点条目补齐回归测试（见第 9 节）；`D-F`、`R-D` 的运行时用户可见程度有限，真实 UI 交互仍由开发者手动验证。R-E 为渲染层目标确认后新增的复刻缺口；本轮新增的 T-C、T-D、M-D、M-E、M-F、D-H、E-I、E-J、E-K、R-F、R-G、R-H 均为只读静态审计结论，未做运行时复现（见第 9 节）。`font_id_for_index` 与 invisible/whitespace 完整策略登记为架构文档 §18.1「尚未复刻」，不计入本表已确认差距。

### 2.3 已登记暂不处理项（复核仍存在）

见第 6 节：A11、C6、F-14、R5。它们在 [docs/架构决策记录.md](docs/架构决策记录.md) 第 6 条与迁移计划中已登记，本次不重复计为新差距。

### 2.4 非差距（裁剪与实现差异）

见第 7 节。包含协作/远程/LSP 裁剪、InlayMap 生产者裁剪（共享的行内替换渲染底座纳入对齐），以及 rope 存储、本地单调版本、多入口编辑语义、crate 内互相引用等实现差异。

### 2.5 契约层差距（文档）

- H-1：架构文档 §4.2、T-8 曾描述已从代码与 §5.3/§5.4 删除的「内容代际 / reset / 显式重锚」模型；本轮已按 Zed 原型修正。见第 4.7 节。
- H-2：架构文档 §6.3/§6.6 仍要求「语言注册表版本变化驱动注入层补解析」，与已登记的 L-A 产品裁剪冲突，§17 与 §18.3 均未登记该裁剪。见第 4.7 节。
- H-3：架构文档 §14.1 的依赖箭头图与 §2.1 分层未反映 `zcv-editor → zcv-project`/`zcv-workspace` 的实际依赖（与 Zed `crates/editor` 一致）；第 8 节回写建议尚未落地。见第 4.7 节。
- H-4：架构文档 §18.1 与 §18.2 重复登记「行内元素实测宽度回写」（R-E），且 §18.2 保留「已按目标落地」的历史说明，违反 §19.2「差距消除后删除」。见第 4.7 节。
- H-5：架构文档 §8.2 与 §15 D-8 写作「同构段不得被合并」，而 Zcv 与 Zed 都要求合并相邻同构变换（不得存在相邻同构变换）；措辞与实现相反。见第 4.7 节。

---

## 3. 分层对齐事实图

> 以下为审计时的现状事实图；各层「缺口」行的完成状态以第 2.2 节表格与第 4 节状态行为准。

### 3.1 文本事实层 `zcv-text`

- 所有者：`Buffer` 私有聚合 `buffer_id / read_only / config / storage / version / saved_version / next_transaction_id / text_changes / edit_log / coordinate_index / history / session`（`zcv-text/src/buffer/mod.rs:38-57`）；文本与版本的写入只在 `commit_prepared_text_change` 一处（`zcv-text/src/buffer/transaction_pipeline/apply.rs:217-238`）。T-1/T-6 成立。
- 事务：`Transaction` 携带 `base_version`；失配显式返回 `VersionMismatch`（`apply.rs:209-215`）；两阶段原子替换（`apply.rs:169-197`）；空 `EditList` 被拒绝（`zcv-text/src/transaction/core.rs:21-23`）。T-2 成立。
- 快照：`Snapshot` 不可变、一次绑定 `storage/version/config/edit_log/coordinate_index`（`zcv-text/src/snapshot.rs:24-32`）。T-4 成立。
- 锚点：`Anchor::resolve_in` 只走不衰减 `CoordinateIndex`（`zcv-text/src/tracking/anchor.rs:71-76`）；`CoordinateIndex` 与受预算裁剪的 `EditLog` 分离、同一次提交追加（`apply.rs:227-236`）。T-3/T-5 成立。
- 版本链：`Anchor` 只含 `version/offset/affinity`（`anchor.rs:15-21`），沿不衰减坐标索引在同一版本链上解析；代际 / `reset` / `rebase_across_generations` 已整体删除，外部文本更新经 `replace_text` 算差异后作为普通事务提交（`replace.rs:18-32`）。T-8 的「失败显式暴露」半边成立（跨层误用见 M-B/E-B）；架构文档残留的代际叙述见 H-1。
- 订阅：每消费者独立 `SubscriptionState`、无全局队列、批次携带旧/新范围（`zcv-text/src/text_changes.rs:305-379`）。5.4 成立。
- 缺口：T-A/T-B 能力已补齐但只有测试消费、无生产调用方；`text_for_version` 在任意 undo/redo 后返回 `HistoryTextUnavailable`（T-D），`MergeWithPrevious` 跨 `undo: None` 合并导致 undo 失败并分叉（T-C）。见第 4.5 节。

### 3.2 语言快照层 `zcv-language`

- 所有者：`LanguageBuffer` 直接持有文本 `Buffer` 与 `Mutex<LanguageState>`，同生命周期；`ParseTask` 的 `Drop` 取消后台解析（`zcv-language/src/language_buffer.rs:105-109, 72-76`）。L-1 方向成立。
- 一致快照：`snapshot()` 在返回前 `interpolate`，使 `syntax.version == text.version`（`language_buffer.rs:158-171`）。L-1 成立。
- 插值/解析分离：`parsed_version`/`interpolated_version` 分离，`did_parse` 要求版本与语言匹配否则拒绝安装（`zcv-language/src/syntax_map.rs:24-32, 263-277`）。L-2 成立。
- 单任务与唯一安装：`start_reparse` 替换旧任务、`install_parse_result` 为唯一安装入口（`language_buffer.rs:363-365, 426-436`）。L-3 成立。
- 查询编译：查询在装配期编译进 `CompiledLanguageQueries`/`Arc<Query>`（`zcv-language/src/registry.rs:34-43`）。6.2 成立。
- 缺口：注入层待解析语义已补 `Pending`（L-A）；注入层范围与增量已改 `Range<Anchor>` + 坐标索引（L-B 部分修正），但 `coordinate_edits_since` 为 `None` 时仍无命名地整层丢弃并全文重跑（见 §4.4）；`LanguageSettings` 仅覆盖 tab（已登记 R5）。

### 3.3 组合文档层 `zcv-multi-buffer` / `zcv-buffer-diff`

- 所有者：`MultiBuffer` 恒为 excerpts 形态，普通文档为整文件单 excerpt（`zcv-multi-buffer/src/multi_buffer.rs:3084-3113`）；源文本仍由各 `LanguageBuffer` 拥有。
- 快照所有权：`MultiBuffer` 持有唯一 `snapshot: MultiBufferSnapshot` 与 `snapshot_dirty`，源事件只登记 `pending_source_syncs`，`snapshot(cx)` 批量消费并整帧提交（`multi_buffer.rs:3052, 3104-3113, 4837-4890`）。阶段 8 的「源事件只置脏、读取时拉取」成立。
- 双树与连续 cursor：输入 `SumTree<Excerpt>` + 输出 `SumTree<DiffTransform>`，查询经 `MultiBufferCursor`/summary 推进（`multi_buffer.rs:1528-1530, 1173-1253`）。M-2/M-3 成立。
- diff 域：`zcv-buffer-diff::BufferDiff` 拥有 hunk 事实，`MultiBuffer` 只持展示态 `DiffState` 与独立版本的 `DiffDisplaySnapshot`（`zcv-buffer-diff/src/buffer_diff.rs:244-257`，`zcv-multi-buffer/src/diff_projection.rs:75-91, 195-213`）。7.3 方向成立。
- 事务：组合编辑映射为源 `Buffer::edit`，组合历史只保存源事务身份映射（`multi_buffer.rs:1737-1740, 4438-4485`）。M-6/M-8 成立。
- 缺口（审计时）：增量推导 `None` → reset（M-A，已修正）、通用解析静默跨代际重锚（M-B，已随代际机制整体删除解决）、缺 `ExcerptOffset`（C-C，已修正）、hunk 身份位置（C-D，已修正）。
- 本轮新增对齐：`BufferId` 稳定身份、`ExcerptBoundary` / `starts_logical_excerpt` / `show_headers` / `singleton`（见第 5 节）。

### 3.4 显示投影层 `zcv-editor::DisplayMap`

- 层顺序：`FoldMap → TabMap → WrapMap(Entity) → BlockSnapshot`，每层快照嵌套下层（`zcv-editor/src/display_map.rs:746-779, 1088-1113`）。D-1/D-4 成立。
- 唯一推进入口：`DisplayMap::snapshot` 消费组合订阅并逐层同步；`Editor` 只经 `cached_snapshot` 读取，不缓存第二份（`display_map.rs:797-821`，`zcv-editor/src/view/mod.rs:892-894`）。E-1/E-2 方向成立。
- 坐标与 Bias：`ProjectedLineIndex/ProjectedPoint`、`WrapRow/WrapPoint`、`DisplayPoint/DisplayRow`、`TabColumn`、`FoldBias` 等逐层 newtype 与显式 Bias 广泛存在（`display_map.rs:74-161`，`display_map/tab_map.rs:30`，`display_map/fold_map.rs:144`）。D-11 成立。
- 异步：只有 wrap 层是 `Entity`，自持 `background_task`/`pending_edits`/`interpolated_edits`，落地时先反转插值再叠加真实编辑（`display_map/wrap_map.rs:1114-1125, 1240-1260`）。8.4/D-12 方向成立。
- 折叠候选：`CreaseMap` 只存宿主显式注入锚点，语法候选由 `DisplaySnapshot` 按可见逻辑行即时查询并带视口缓存（`display_map/crease_map.rs:46-88`，`display_map.rs:302-391`）。8.5 方向成立，且已无组合层全源 fold list（`fold_anchors`/`fold_sources` 搜索为 0）。
- 缺口：Block 层增量协议（D-A 部分修正：wrap 编辑与 excerpt 边界变化分支只复用前缀，仍保留两处条件性从投影起点重建）、热路径物化整行（D-B，已修正）、Fold/Tab Edit 坐标类型（D-C，已修正）、observe 与 snapshot 双路径（D-D，已修正）。
- 前轮新增：`show_headers` 纳入 Block 快速路径失效键（D-F）、Block 层补 input 覆盖与越界断言（D-G）；D-13 分类已与 Zed 对齐。
- 本轮新增：diff 装饰快照缓存键用 `wrap_edits.is_empty()` 冒充显示几何未变（D-H，高）；折叠全部文件后 gutter/hunk/滚动条装饰沿用旧显示行。

### 3.5 交互与渲染层 `zcv-editor::Editor` / `EditorElement`

- 持有关系：`Editor` 持 `multi_buffer: Entity<MultiBuffer>` 与 `display_map: Entity<DisplayMap>`，二者消费同一组合文档（`view/mod.rs:247-250, 1583-1585`）。9.2 成立。
- 事务唯一入口：`change` / `change_with_after` / `change_with_after_post` 全部经 `commit_session`，唯一 `MultiBuffer::edit` 调用点在 `commit_session`（`view/mod.rs:1674-1721, 1727-1769`）；普通编辑、IME、搜索替换、重命名、自动闭合、undo/redo 均接入。E-3 成立。
- 落地顺序与事件：文本 → `advance_snapshots` → 选区落位 → `end_transaction` 返回真实身份才发布 `Edited{TransactionId}`；失败结束空事务并恢复编辑前选择（`view/mod.rs:1748-1768, 1837-1852`）。E-6/E-9 成立。
- 长期位置：选择 `SelectionSet<MultiBufferAnchor>`、滚动 `ScrollAnchor { MultiBufferAnchor, offset }`，消费时按当前快照解析（`view/mod.rs:257`，`scroll.rs:35-39, 376-383`，`selection/core.rs:110-140`）。E-4（选择/滚动/折叠）成立。
- 渲染边界：`EditorElement` 无 `.edit(`，布局只读 `editor.snapshot()`/`selections`，写回仅几何缓存与显示配置（`element.rs:1114-1121, 1443-1454, 2042-2046`）。R-1/R-2 成立。
- 渲染内部模型（目标已确认为 Zed 渲染层）：行布局已是 `FragmentedLine` 片段序列（`element.rs` 的 `LineFragment::{Text, Element}`），跨片段 `x_for_index`/`closest_index_for_x` 与命中按片段解析；`Chunk.renderer` 携带 `ChunkRenderer`，`FoldPlaceholder.render` 注入元素；元素在 `request_layout`/`prepaint` 内测量、行原点确定后 prepaint。R-A…R-D 成立。
- 渲染缺口：元素实测宽度未回写显示层（R-E）；水平窗口化行的选区/装饰几何列空间错误（R-F）；窗口裁剪占位符丢 `ChunkRenderer` 回退文本（R-G）；`ellipsis` 构造时捕获主题色（R-H）；字体回退下的行内列位置与 invisible/whitespace 完整策略尚未复刻（架构文档 §18.1）。见 §4.8。
- 本轮新增交互缺口：`ToggleFold` 命令使用空 `render` 默认占位符（E-I）、`autoclose_regions` 无生命周期清理（E-J）、`DiffHunksExpandedChanged` 无生产者（E-K）。见 §4.3。
- 其余交互缺口（审计时）：选择唯一入口与相邻合并（E-A，已修正）、组合坐标类型误用（E-B，已修正）、滚动条标记版本校验（E-C，已修正）、选择历史生命周期（E-D，已修正）、blink 任务取消（E-E，已修正）。

### 3.6 工程边界 `zcv-project` / `zcv-workspace`

- 文档索引：`BufferStore` 用弱引用按路径复用（`zcv-project/src/buffer_store.rs:19-22, 80-102`）；Git 修订文本由 `GitStore` 按 `(revision, path)` 唯一持有（`zcv-project/src/git_store/mod.rs:264-269`）。P-1 方向成立。
- 依赖：`zcv-project` 只依赖 text/language/buffer-diff/path/fs-watch/git，不依赖 multi-buffer/editor/workspace（`zcv-project/Cargo.toml:9-25`），与 Zed `crates/project` 相同。P-4/14.2 成立。
- Workspace：只持窗口容器、Pane/Dock、Item 句柄、布局持久化与命令分发，不复制 Item 领域状态（`zcv-workspace/src/workspace_state.rs:52-77`，`pane.rs:95-109`）。P-3 成立。
- 工具区：`Pane` 拥有 `Toolbar`，`Item` 不返回工具区视图（`pane.rs:95-109, 646-651`，`toolbar.rs:36-43`）。11.2 主体成立。
- 缺口（审计时）：项目搜索旁路物化（E-G，本轮已消除）、Item 能力标志扩展（E-H，本轮已消除）；GitStore optimistic index 第二份文本经本轮复核判定不成立（见第 5 节）。

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

> 状态：已解决（`a034e79b` 先显式化重锚；`660fca54` 进一步删除代际机制，`Anchor` 只沿单一版本化坐标链解析，重锚路径不再存在）。
> 复审说明：Zed 原型本身没有 generation / `reset` / `rebase_across_generations` 概念，「删除代际」比「显式重锚」更贴合目标；架构文档残留的代际措辞见 H-1。
> 下方「证据」保留审计时事实（当时 `BufferGeneration` 与 `rebase_across_generations` 尚在）。

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
> 各构造/重建路径都显式带着 hunk 重建输出节点：`replace_all_excerpts`/`build_entries_for_excerpts` 用物化产生的 hunk；`splice_source_path`/`splice_excerpt_entries`/`fix_document_tail_newline`/`rebuild_display` 按输出序从旧节点取回。
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

#### M-C（中）展开删除 hunk 以 diff 基线的修订 Buffer 作为组合 excerpt 源，基线文本变化与 BufferDiff 结果构成两个推进入口

> 状态：已修正（本轮）。删除 hunk 已按 Zed 数据模型落到输出侧：`DiffTransform::DeletedHunk` 自带被删文本的只读 `Excerpt`，只存在于输出坐标，不进入输入 excerpt 树；`DiffTransform::from_excerpt` 对 `Deleted` 片段 `debug_assert`，组合 excerpt 树只含工作区文档。diff 基线/参照源仍不进入组合源订阅表：`replace_all_excerpts` 与 `register_sources` 只为含非 `Deleted` excerpt 的源建立文本/事件订阅；基线文本变化由 `DiffState` 订阅 `BufferDiff::base_source()` / `index_source()` 触发 `recompute_diff_for_source`，组合投影只由 `diff_changed` 单入口推进，基线源文本在物化时刷新。回归 `staging_one_hunk_rebuilds_the_projection_once_after_refresh` 以原断言（`initial_version + 1`）通过。

- 位置：`zcv-multi-buffer/src/diff_projection.rs`（`materialize_file` 用 `base` 作为旧侧 excerpt 的源；`DiffState` 订阅 base/index）、`zcv-multi-buffer/src/multi_buffer.rs:429-471`（`DiffTransform::DeletedHunk { summary, hunks, excerpt }`）、`multi_buffer.rs`（`from_excerpt`/`deleted_hunk`、`build_entries_for_excerpts`、`splice_excerpt_entries`、`fix_document_tail_newline`、`splice_source_path`、`rebuild_display`、`excerpt_anchor_source_offset`）。
- 证据：此前暂存一个 hunk 时投影版本连续推进两次——一次来自基线修订文档的文本变化（`sync_pending_sources` → `apply_source_change` → `publish_projection_change`），一次来自新 `BufferDiff` 结果（`diff_changed` → `rebuild_diff_projection`）。旧侧删除 hunk 曾是组合 excerpt，其源是基线 Buffer，因此基线 Buffer 被注册为组合层源并订阅其文本事件。现在输入 excerpt 树不再含删除片段，`apply_source_change` 对它没有调用路径。
- 原型：Zed `DiffTransform::DeletedHunk { base_text_byte_range, summary, buffer_id, hunk_info }`（`crates/multi_buffer/src/multi_buffer.rs:715-726`）只承载输出摘要与基线字节范围；组合 excerpt 始终来自工作区文档。Zcv 以节点自带只读 `Excerpt`（其 `source_index` 指向基线源）承载被删文本，等价表达基线范围并复用显示、锚点与文本读取路径。
- 违反不变量：M-2（组合层只有一个权威结构）、8.4/D-5（每项派生状态只有一个推进入口）、7.5（diff 显示几何使用独立的不可变显示输入，基线变化不直接推进组合拓扑）。
- 影响：同一 diff 几何曾由「基线源编辑」与「diff 结果」两路推进，每暂存一次投影版本与显示链多推进一次。删除 hunk 留在输入树还会让输入/输出坐标在删除段两侧不再相等，C-C 的独立坐标维度正是为消除该歧义而引入。
- 目标边界：删除 hunk 只由 `DiffTransform::DeletedHunk` 承载，携带基线文本描述，不进入输入 excerpt 树；基线文本变化只使 `BufferDiff` 重新计算，组合投影只由 `diff_changed` 单入口推进。
- 迁移项：`diff_projection.rs`（`materialize_file`）、`multi_buffer.rs`（`DiffTransform`、`from_excerpt`、`deleted_hunk`、`build_entries_for_excerpts`、`splice_excerpt_entries`、`fix_document_tail_newline`、`splice_source_path`、`rebuild_display`、`excerpt_anchor_source_offset`）、相关测试。
- 定向验证：暂存一个 hunk 后组合投影版本只推进一次；展开删除 hunk 的文本仍能正确显示、选择与导航（`materialized_diff_old_side_is_selectable_but_only_new_side_is_editable`）；多文件/多 hunk 暂存不产生额外推进。


#### M-D（中）组合投影同步帧丢弃精确源增量，改用树比较推导范围

> 状态：待处理（本轮新增）。

- 位置：`zcv-multi-buffer/src/multi_buffer.rs:1537-1547`（`ProjectionSync` 只有 `changed: bool`）、`multi_buffer.rs:3393-3401`（`publish_projection_change` 丢弃 `incremental`）、`multi_buffer.rs:3428-3452` 与 `multi_buffer.rs:1119-1186`（帧末从前后投影树比较出单一范围）、`multi_buffer.rs:391-395` 与 `1093`（`ExcerptContext::eq` 只比解析偏移、忽略 Anchor 版本）。
- 证据：`diff_changed` 开始同步帧后，`sync_pending_sources` 经 `apply_source_change` 产出的 `SourceIncremental` 在 `publish_projection_change` 中被 `sync.changed = true; return` 丢弃；帧结束 `finish_projection_sync` 用 `projection_changed_ranges` 比较 excerpt 摘要/结构，得到单一 `(old_range, new_range)` 批次，等长替换时该范围为空。Zed `sync_from_buffer_changes` 累积 `edits_since_in_range` 并把精确 excerpt edits 交给 `sync_diff_transforms`（`crates/multi_buffer/src/multi_buffer.rs:2540-2699`）。
- 违反不变量：2.2/7.5「同时发布细粒度输出偏移增量」、M-A 目标边界「增量入口不得静默回退」。
- 影响：diff 同步帧内到达的外部源编辑若为等长替换，显示层收不到该变化的输出编辑，软换行/装饰可能沿旧宽度；即使摘要改变也只发布 excerpt 边界范围而非精确增量。
- 目标边界：`ProjectionSync` 累积组合后的 `TextChangeBatch`（或各源 patch），帧末发布该增量；树比较只用于拓扑增删。
- 定向验证：注入 diff 后对源做等长替换并令 `diff_changed` 先于 `snapshot()` 消费，断言 `MultiBufferSubscription::consume()` 含该输出编辑。

#### M-E（中）BufferDiff 后台 diff 任务不可取消、无在途去重

> 状态：待处理（本轮新增）。

- 位置：`zcv-buffer-diff/src/buffer_diff.rs:287-326`（无条件 `cx.spawn(...).detach()`）、`buffer_diff.rs:244-257`（结构体无 `Task` 字段）、`buffer_diff.rs:332-344`（过期即再 spawn）；调用方 `zcv-multi-buffer/src/diff_projection.rs:1003-1013`、`multi_buffer.rs:4417`。
- 证据：working 源每次文本变化与 base/index 每个事件都会调用 `recompute_with_refresh`，每次新起一个后台全文 diff 任务且互不取消；`calculation_pending` 只是布尔标志，调用前不检查。Zed `crates/buffer_diff/src/buffer_diff.rs:2219-2231` 以「drop 返回的 Task 取消上一次更新」为契约。
- 违反不变量：13.3。
- 影响：附着 git diff 的编辑器连续输入时并发堆积全文 diff 计算；实体销毁无法取消在途任务。
- 目标边界：`recompute_with_refresh` 返回 `Task`，由 `BufferDiff` 保存并在下次调用替换（drop 即取消），禁止 `.detach()`。
- 定向验证：N 次快速源编辑后断言只有一个在途计算；drop 实体后任务不落地。

#### M-F（低）diff 结果安装只校验 working 版本

> 状态：待处理（本轮新增）。

- 位置：`zcv-buffer-diff/src/buffer_diff.rs:339-347`；捕获端 `buffer_diff.rs:293-300`；base/index 原位刷新 `zcv-project/src/git_store/mod.rs:1124-1138`。
- 证据：`apply_recomputed_hunks` 只比较 working 文本版本；base/index 快照没有版本门控，working 未变而 base/index 前进时旧结果仍被安装。
- 违反不变量：13.3、4.3。
- 影响：hunk 旧侧裸字节范围短暂对应旧 base 快照，直到正确结果覆盖。
- 目标边界：install 前同时校验 base/index 版本或引入 diff 输入 generation，并与 M-E 的单在途任务一并解决。
- 定向验证：base/index 两次变化、working 不变的重叠重算，断言仅最新结果落地。

### 4.2 显示投影层

#### D-A（中）Block 层没有本层增量 edit 协议，任一 wrap 编辑整体重排 transforms

> 状态：**已修正**（本轮）。`BlockSnapshot::sync` 统一为「公共前缀 + 受影响区间 + 公共后缀」：结构变化由 `changed_spec_range` 定位前后缀，纯换行编辑由 `suffix_start_after_edits` 取所有编辑之后的尾部直接追加旧变换子树；删除 `None` suffix 与两处从投影起点重建。锚点重定位使旧前缀越过新块起点时（折叠吞行、删除段把锚点拉回 excerpt 起点）逐步回退前缀边界，不整份重建。折叠作为块层策略变化由 `BlockSnapshot::sync` 依据 `folded_buffers` 重算，并推进 `geometry_epoch`；`set_buffer_folded` 不合成 `WrapEdit`。结构变化的权威信号是 `MultiBufferSnapshot::version()`（组合投影版本）：投影版本变化即重算块分类并复用前后缀，仅异步换行重排（投影版本不变）走 `relocate_specs`；已删除手写的 excerpt 边界签名代理。下方证据保留审计时事实。

- 位置（当前实现）：`zcv-editor/src/display_map.rs:1052-1067`（`current_block_snapshot`）、`zcv-editor/src/display_map/block_map.rs:703-789`（`sync`）、`block_map.rs:804-856`（`relocate_specs`/`rebuild_after_edits`）、`block_map.rs:878-897`（`suffix_from`）。
- 证据：`sync` 只在 `!structural && wrap_edits.is_empty()` 时整树复用（`reuse_transforms`）；wrap 编辑走 `relocate_specs`+`rebuild_after_edits`，`Self::rebuild(prefix, specs, first_changed, spec_count, None, inputs)` 的 suffix 为 `None`，即从首个变化块重建到末尾；结构变化且 excerpt 边界改变时同样只复用前缀（`block_map.rs:768-788`）。前缀几何不再有效时两处回退为 `BlockLayoutPrefix::empty()` 并从投影起点重建。
- 原型对照：Zed `BlockMap::sync`（`crates/editor/src/display_map/block_map.rs:806-1205`）对每条 `WrapPatch` 用 `cursor.slice(&old_start, Bias::Left)` 保留前缀、处理完变更块后 `new_transforms.append(cursor.suffix(), ())` 保留后缀，仅重建受影响块，没有「从投影起点重建」分支；整文件折叠也经 `fold_or_unfold_buffers` 合成行区间 `WrapEdit` 后走同一 `sync`（`block_map.rs:2086-2170`）。Zcv 的层级契约（唯一 `sync` 入口、消费 `WrapEdit`、结果正确）已对齐，残留的是**增量复用粒度**与两处 Zed 不存在的条件性全量重建。
- 违反不变量：D-2、D-5；两处从投影起点重建分支触碰「增量入口不得静默回退到全量」。职责、所有权与数据流方向未越界；D-8 的措辞问题见 H-5。
- 影响：excerpt 拓扑变化或含 wrap 编辑的同步会重建首个变化块之后的全部变换，复用粒度低于 Zed（O(剩余块数) 而非 O(受影响块数)）；两处从起点重建为防御分支，本轮审计未构造出可达路径。
- 目标边界：Block 层消费 `WrapEdit` 并原地推进（Zed 顶层 `BlockMap::sync` 同样只返回 `()`、原地更新 transforms，不产出向上 edit；D-2 的「增量 edit」在链顶层表现为消费下层 edit）；`folded_buffers`/excerpt 结构变化表达为受影响的 `WrapEdit` 区间增量，删除无名 `None` 回退。
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

#### D-F（低）Block 快速路径复用分支未覆盖 `show_headers` 策略

> 状态：已修正（本轮）。`BlockSnapshot` 保存构建时的 `show_headers` 策略；`sync` 把它纳入结构失效键，纯策略变化不再复用旧分类。

- 位置：`zcv-editor/src/display_map/block_map.rs:609-625`。
- 证据：`sync` 的复用分支只比较 `folded_buffers + excerpt_boundaries + wrap_edits`，而 header/divider 分类还依赖 `wrap_snapshot.buffer_snapshot().show_headers()`；`MultiBufferSnapshot::show_headers` 当前恒为 `true` 且无 setter（`zcv-multi-buffer/src/multi_buffer.rs:3286, 3351, 5061`）。
- 影响：一旦 `show_headers` 成为可切换策略，纯策略变化（`wrap_edits` 为空）会复用旧分类，违反 D-5「快速路径覆盖全部影响结果的状态」。Zed 用 `MultiBuffer::without_headers` 与 `BlockMap::buffers_with_disabled_headers` 表达抑制，Zcv 无消费方，属裁剪。
- 目标边界：把策略纳入失效键，或在文档中写明其不可变。

#### D-G（低）Block 层缺 input 精确覆盖断言与越界显式失败

> 状态：已修正（本轮）。`rebuild` 末尾 `debug_assert_eq!` 变换输入行与换行投影；`wrap_row_to_display_row` 越界显式失败，`display_row_mapping` 不再返回 `WrapRow::ZERO` 兜底。回归测试见 `display_map/test/block_map_tests.rs`。

- 位置：`zcv-editor/src/display_map/block_map.rs:708-757`。
- 证据：`place_from` 的 input 精确覆盖未配 `debug_assert`（Zed `block_map.rs:1197-1201` 有）；`wrap_row_to_display_row` 对越界 wrap_row 直接 clamp（`728-731`），`display_row_mapping` 找不到 transform 时返回 `WrapRow::ZERO`（`746-757`）。
- 影响：D-8/D-10 的不变量被静默降级为兜底，失败不可见。
- 目标边界：补 input 覆盖断言，越界走显式失败。

#### D-E（低）显示坐标缓存只按文本版本失效

> 状态：已确认并修正（本轮）。`LineWidthCache` 改以显示快照版本失效：折叠、换行、tab 或块拓扑等仅显示版本推进的变化也会使缓存失效。回归测试 `longest_line_width_cache_invalidates_when_only_the_display_changes`。

- 位置：`zcv-editor/src/view/mod.rs:960-987`（`LineWidthCache` 仅比较 buffer 版本、row、字体）。
- 说明：折叠、换行、block 结构变化可能不改变文本版本，却改变 display row 内容；若缓存命中旧宽度，属 R-3 缺口。是否可实际观察未确认。

#### D-H（高）diff 装饰缓存键以 `wrap_edits.is_empty()` 冒充显示几何未变

> 状态：**已修正**（本轮）。diff 装饰缓存改为按 `BlockSnapshot` 的块几何代际（`geometry_epoch`）判断显示几何是否变化：变换树整棵复用时保持不变，折叠、显示策略与 excerpt 拓扑变化都会使代际推进；不再用 `wrap_edits.is_empty()` 代理显示几何。

- 位置：`zcv-editor/src/display_map.rs:910-919`（缓存键）、`display_map.rs:941-955`（`set_buffer_folded` 传 `&[]`）、`display_map/block_map.rs:714-716, 748-766`（`folded_buffers` 改变块几何）、`display_map/decorations.rs:245-311`（装饰快照固化显示行区间）。
- 证据：`commit_snapshot` 在 `wrap_edits.is_empty() && editor_hunks.is_empty() && tab_width 相同 && same_diff_display(...)` 时直接复用 `previous.decorations.diff()`；`set_buffer_folded` 固定传空 wrap 编辑，但 `folded_buffers` 变化会让 Block 走结构分支并收缩显示行数；`DiffDecorationSnapshot` 的 `diff_rows/strips/hit_regions/controls/scrollbar_diff_markers` 都是构建时固化的显示行整数区间。项目 diff 非冲突视图处于 `editor_hunks` 为空、`diff_display` 非空状态（`zcv-version-control/src/project_diff.rs:805-808`），点「折叠全部文件」即触发。
- 违反不变量：D-4、D-5、8.5。
- 影响：折叠文件后 gutter 行标记、hunk 背景、控制条命中区与滚动条 diff 标记仍指向折叠前的显示行，直到下一次 diff 重算。
- 目标边界：把「几何未变」判据换成块层几何身份（如 `BlockSnapshot` 只在变换树真正重建/重定位时推进的几何版本），删除用 wrap 编辑有无代表显示几何的代理判据。
- 定向验证：注入 diff（不注入 editor_hunks）→ 取 `diff_decorations()` → `toggle_buffer_fold` → 断言装饰被重建且 `diff_rows` 落在折叠后的显示行。

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

> 状态：已修正（本轮）。记录改用 `BTreeMap` 并有上限（超出丢弃最老事务）；`commit_session` 两条失败分支结束会话时删除本会话记录。回归测试见 `selection/test/state_tests.rs`。

- 位置：`zcv-editor/src/selection/state.rs:299-335`、`zcv-editor/src/view/mod.rs:1727-1769`。
- 证据：`SelectionHistory` 只有 `insert_transaction`/`transaction_mut`/`remove_transaction`；只有成功且合并时删除；两条失败分支只 `end_transaction` 不删本会话记录。文本层历史按预算裁剪，选择历史不受同一生命周期约束。
- 影响：选择历史随编辑线性增长，与文本历史节点脱节。
- 目标边界：选择历史随文本历史节点失效同步清理；失败会话结束即删除自身记录。
- 定向验证：大量/失败编辑后断言选择历史长度与文本历史节点一致。

#### E-E（低）`BlinkManager` 定时任务不可显式取消

> 状态：已修正（本轮）。`BlinkManager` 持有在途 `Task`，替换或 `disable` 时丢弃即取消；不再 `detach`。回归测试 `disable_cancels_the_in_flight_blink_timer`。

- 位置：`zcv-editor/src/blink_manager.rs:44-77, 100-107`。
- 证据：`pause_blinking`/`blink_cursors` 均 `cx.spawn(...).detach()`，不保留 `Task`；`disable` 只置 `enabled=false`，在途 timer 仍运行到下次回调。
- 违反不变量：13.3「后台任务必须可取消，并与拥有它的实体同生命周期」。
- 影响：每次输入产生一个在途 timer；实体销毁后靠 `this.update` 失败停止。可测量资源滞留未确认。
- 目标边界：持有 `Task`，随实体/禁用取消。

#### E-F（低）`Editor` 持有第二个 `DisplayMap`（placeholder）

> 状态：已判定，不构成 E-1 违反（本轮）。`placeholder_display_map` 携带独立 `Buffer`，只在真实文档为空时作为展示内容源接入同一渲染管线；它不是同一文档的第二份可写权威，只是一处与 Zed `placeholder_text` 不同的实现选择。已登记进架构文档 §18.4。

- 位置：`zcv-editor/src/view/mod.rs:256, 779-798, 802-816`、`element.rs:1236-1237`。
- 说明：`placeholder_display_map: Option<Entity<DisplayMap>>` 携带独立 `Buffer`，空文档时接入渲染。它不是同一文档的影子权威，属「复用真实渲染管线」的实现选择；是否违反 E-1 未确认，列入第 5 节。

#### E-I（中）`ToggleFold` 命令使用空 `render` 的默认折叠占位符

> 状态：待处理（本轮新增）。

- 位置：`zcv-editor/src/view/mod.rs:714-762`（`toggle_fold_at_cursor`，`:755` 用 `FoldPlaceholder::default()`）、`view/mod.rs:680-708`（`toggle_fold_at_line`，`:700` 用 `FoldPlaceholder::ellipsis(cx)`）、`view/mod.rs:2081-2086`（`handle_toggle_fold` → `toggle_fold_at_cursor`）、`display_map/fold_map.rs:222-232`（`default` 渲染 `gpui::Empty`）。
- 证据：命令入口 `ToggleFold` 走 `toggle_fold_at_cursor`，其 `fold_range` 传入 `FoldPlaceholder::default()`，render 产出 `gpui::Empty`；`element.rs:2912-2925` 对带 `renderer` 的 chunk 走元素路径、不再绘制文本，因此折叠区没有可见占位符。crease 点击走的 `toggle_fold_at_line` 使用 `ellipsis`；`view/mod.rs:680` 注释声称二者共享实现，实际并未共享。
- 违反不变量：R-6、10.2。
- 影响：按 `alt-cmd-[` 折叠后折叠区无省略号/可点击元素，且与鼠标折叠呈现不一致。
- 目标边界：命令与 crease 共用同一折叠实现（`toggle_fold_at_line`），使用 `FoldPlaceholder::ellipsis`；删除重复的 `toggle_fold_at_cursor` 逻辑。
- 定向验证：经 `handle_toggle_fold` 折叠一行后断言产生元素片段而非空元素；与 crease 点击结果一致。

#### E-J（中）`autoclose_regions` 无生命周期清理

> 状态：待处理（本轮新增）。

- 位置：`zcv-editor/src/view/mod.rs:293-295`（字段）、`view/input.rs:354-369`（只 `extend`）、`input.rs:392-404` 与 `418-426`（每次输入全量扫描）。
- 证据：`autoclose_regions: Vec<AutocloseRegion>` 只在自动配对成功后 `extend`，全仓无 `retain`/`clear`/`remove`；失效区域仅因 `resolve_anchor` 返回 `None` 被跳过，条目永久保留；`finish_edit` 只清结构化历史与 pending。Zed 在选区变化/编辑后调用 `invalidate_autoclose_regions` 只保留存活区域（`crates/editor/src/input.rs:2098-2126`）。
- 违反不变量：9.1、13.2。
- 影响：长时间编辑会话中区域数无上界，每次输入/退格 O(n) 扫描。
- 目标边界：在唯一的选择/编辑落地路径调用 Zed 式失效，保留存活区域；删除纯追加逻辑。
- 定向验证：连续输入 N 对括号后断言 `autoclose_regions.len()` 有界。

#### E-K（低）`EditorEvent::DiffHunksExpandedChanged` 无生产者

> 状态：待处理（本轮新增）。

- 位置：`zcv-editor/src/view/mod.rs:78-79`；消费方 `zcv-version-control/src/project_diff.rs:637`、`zcv-search/src/project_search.rs:213`、`zcv-editor/src/workspace_item.rs:32`。
- 证据：全仓无 `emit(EditorEvent::DiffHunksExpandedChanged)`；展开状态变化只经 `advance_snapshots`/`cx.notify`。§9.7 把「diff 展开状态变化」列为领域事件。
- 违反不变量：9.7、7.4。
- 影响：宿主无法观察展开变化；消费方分支为死路径。
- 目标边界：在 diff 展开/折叠唯一路径确有重建时发布该事件；若无消费需求则删除变体与全部匹配臂。
- 定向验证：切换一个 hunk 展开后断言订阅方收到一次事件（或断言变体已删除）。

### 4.4 语言层

#### L-A（中）未加载注入语言被静默丢弃，无 Pending 层与注册表版本补解析

> 状态：已按产品范围裁剪（`b6fe41ef 注入层改用锚点范围并保留待处理层`）。
> 已删除静默 `continue`，未注册注入语言保留为 `SyntaxLayerContent::Pending { language_name }`，范围用锚点保存。
> **已决策不引入运行期语言注册与注册表版本补解析**：Zcv 的 `LanguageRegistry` 由 `builtin_languages()` 在构造期静态装配、没有运行期注册入口，注册表版本不可能变化，补解析没有触发点。随仓库分发的 `.scm`（如 `graphql`/`glsl`/`wgsl`/`latex`/`phpdoc`，见 `queries/*/injections.scm`）确实会命中未注册名并产生 `Pending` 层，只是没有可注册路径使其升级。此项登记为产品范围裁剪（见第 7.1 节），不再作为待修正项；将来若引入运行期语言注册，`Pending` 层即为补解析接入点。

- 位置：`zcv-language/src/syntax_map.rs:684-689`（`continue` 丢弃点）、`syntax_map.rs:109-115`（`SyntaxLayer` 无 Pending）、`syntax_map.rs:24-32, 76-84`（无注册表版本）、`zcv-language/src/registry.rs:196-199`（无版本号）。
- 证据：注入收集遇到 `registry.language_for_injection(&language_name)` 为 `None` 时直接 `continue`，既不保留待解析层也不记录语言名。树内查询文件引用了未注册语言名（如 `graphql`、`glsl`、`wgsl`、`latex`、`phpdoc` 等），全部被静默吞掉。Zed `crates/language/src/syntax_map.rs:193-235` 用 `SyntaxLayerContent::{Parsed, Pending}` 保留待解析层，`SyntaxSnapshot` 带 `language_registry_version`，注册表变化时补解析。
- 违反不变量：L-8、6.3、6.6。
- 影响：未内置语言的围栏代码块/模板字符串没有高亮与语法节点，且当前没有恢复路径；丢失 L-8 的待解析层与 6.6 的注册表版本驱动补解析。
- 目标边界：层内容增加 `Pending` 变体并保存语言名与范围；`LanguageRegistry` 增加单调版本；`SyntaxMap/SyntaxSnapshot` 记录并比较注册表版本；注册表变化时补解析；删除静默 `continue`。
- 迁移项：`syntax_map.rs`、`registry.rs`、`language.rs`、`language_buffer.rs`；`syntax_map_tests.rs`、`registry_tests.rs`。
- 定向验证：注入一段未注册语言，断言存在 Pending 层；注册表解析该名并推进版本后该层变为 Parsed 且产出高亮。

#### L-B（中）注入层范围为裸 `Range<usize>`；`edits_since` 失败时整层丢弃或全文失效

> 状态：**已修正**（本轮）。范围已改 `Range<Anchor>`，增量由 `zcv-text::Snapshot::coordinate_edits_since` 提供；`None` 时不再丢弃全部语法状态并全文重跑，改由 `coordinate_edits_or_fail` 在不变量破坏时显式失败（`zcv-language/src/syntax_map.rs:178-188`），首次解析与语言切换仍按显式边界全文收集。下方证据保留审计时事实。

- 位置（当前实现）：`zcv-language/src/syntax_map.rs:107-127`（`SyntaxLayerContent`/`SyntaxLayer.range: Range<Anchor>`）、`syntax_map.rs:240-296`（`interpolate`）、`syntax_map.rs:400-471`（`reparse`）。
- 证据：注入层范围已锚点化并在查询时 `resolve_in`；增量走 `coordinate_edits_since`（不衰减坐标索引）。但 `changes` 为 `None` 时 `interpolate` 的 `else` 分支 `std::mem::take(&mut state.injections)` 并丢弃主树，`reparse` 把 `old_tree` 置 `None`、`changed` 回退 `0..len` 全文重收集。该分支按坐标索引语义（`first_version <= since <= version` 恒成立）当前不可达。
- 原型对照：Zed `SyntaxMap::interpolate` 用 `BufferSnapshot::anchored_edits_since`、`reparse_` 用 `edits_since`（`crates/language/src/syntax_map.rs:424-541, 565-626`）；Zed 的 `edits_since` 是从 fragment 树与版本时钟重建的**全函数迭代器**（`crates/text/src/text.rs:2692-2760`），没有「版本被裁剪导致 edits 不可得」的失败分支，因此 Zed 根本没有「丢弃全部层 + 全文重跑」的兜底。Zed 的注册表版本补解析机制（`syntax_map.rs:585-622`）属已裁剪的 L-A。
- 违反不变量：6.3 的范围与增量部分已满足；残留分支触碰「增量入口不得静默回退到全量」。当前不可达，属潜在兜底而非活跃偏离。
- 影响：当前不可达、无即时用户影响；一旦坐标索引语义变化或快照跨 Buffer，主树与全部注入层（含 `Pending`）整体失效并全文重跑，且台账此前声称该兜底已删除。
- 目标边界：删除 `changes.is_none()` 的整层丢弃分支，令其为 `None` 时按不变量失败；显式边界（构造、语言切换、整体替换）另行命名，不保留无名 reset 兜底。
- 迁移项：`syntax_map.rs`、`syntax_map_tests.rs`。
- 定向验证：删除分支后 `injection_layers_survive_edit_log_eviction` 等行为测试保持通过；对 `coordinate_edits_since` 断言同 Buffer 生命周期内恒为 `Some`。

#### L-C（低，已登记 R5）`LanguageSettings` 只覆盖 tab，软换行/目标行宽仍全局

- 位置：`zcv-language/src/language_settings.rs:12-25`、`zcv-editor/src/view/mod.rs:1609-1637, 1655-1662`。
- 证据：`LanguageSettings { tab: TabConfig }` 只有 tab；`resolve` 只取 `tab_for_language`。`Editor` 的 `soft_wrap`/`preferred_line_length` 在构造与全局设置观察中直接从 `SettingsStore` 读取。
- 违反不变量：L-6、6.5（tab 宽度、软换行、preferred line length 等按语言解析并由快照下发）。
- 处置：迁移计划阶段 7 R5 已登记「无消费方，保持现状」。本次复核仍存在，按已登记项处理。

### 4.5 文本事实层

#### T-A（中）T-7 历史可见性、当前→旧版本映射与历史文本重建能力未提供

> 状态：已修正（本轮，`zcv-text`）。`Snapshot` 提供 `has_edits_since` / `has_edits_since_in_range` / `offsets_to_version` / `range_to_version` / `text_for_version`，`PositionMap` 提供 new→old 反向映射。历史重建依赖事务保留的逆编辑：跨 `SkipHistory` 大事务返回显式 `HistoryTextUnavailable`，已登记为实现差异。

- 位置：`zcv-text/src/snapshot.rs:104-120`（只有 `edits_since`/`edits_since_in_range`）、`zcv-text/src/tracking/edit_log.rs:107-169`、`zcv-text/src/position_map.rs:110-350`（只有 old→new）。
- 证据：`Snapshot` 没有 `has_edits_since(_in_range)`；`PositionMap` 没有 new→old（`range_to_version`/`offsets_to_version` 等价物）；没有按版本重建文本的入口。Zed 原型有 `rope_for_version`、`has_edits_since(_in_range)`、`range_to_version`、`offsets_to_version`。
- 违反不变量：T-7（架构文档 5.3 明确要求「历史可见性、任意跨度净编辑、历史文本重建、把当前坐标映射回旧版本」）。
- 影响：无法判断「某段文本在某历史版本是否可见」，无法把当前坐标映射回旧版本，上层做历史对比只能整份物化。当前 diff 全文物化与 Git 只读预览属已登记实现差异，可暂缓，但契约未落地。
- 目标边界：能力归 `Snapshot`（唯一读取边界），编辑事实由 `EditLog` 的版本区间提供；不得在 `MultiBuffer`/`zcv-buffer-diff` 各自重建历史文本。
- 迁移项：`zcv-language/src/syntax_map.rs:208,365`、`zcv-buffer-diff`、`zcv-project` GitStore、`zcv-text/tests/versioned_edits_anchor.rs`、`zcv-text/README.md`；应登记进第 18.1 节「尚未复刻」。
- 定向验证：`has_edits_since(_in_range)` 真值；`offsets_to_version` 与 `edits_since` 互逆；按旧版本重建文本；对照 Zed 同用例。

#### T-B（中）无 T-9「基线派生快照 + 版本校验后原子安装」的文本层入口

> 状态：已修正（本轮，`zcv-text`）。新增 `EditedBufferSnapshot` / `Buffer::snapshot_with_edits` / `Buffer::fast_forward`：派生在快照副本上完成，安装前校验 `base_version`，过期结果返回 `VersionMismatch`，安装经唯一事务路径落地。

- 位置：`zcv-text/src/snapshot.rs:35-224`（`Snapshot` 只读、无派生/安装接口）。
- 证据：全仓无 `snapshot_with_edits`/`fast_forward`/`EditedBufferSnapshot` 等价能力。Zed 对应 `Buffer::snapshot_with_edits`、`Buffer::fast_forward`、`EditedBufferSnapshot`。
- 违反不变量：T-9。
- 影响：Git 修订文本、diff 基线这类「稳定基线上计算」的场景缺文本层规范入口，只能整份物化或上层自建，缺版本校验与唯一安装点。是否已由 `zcv-language`/`zcv-multi-buffer` 以其他形态承接未确认。
- 目标边界：派生在快照副本上完成；安装前校验基准版本一致，过期结果丢弃；唯一安装入口，与订阅/历史解耦。
- 定向验证：基线上应用编辑得到派生快照，主文档未变时安装成功；主文档前进后安装被版本校验拒绝。

#### T-C（中）`MergeWithPrevious` 可跨 `undo: None` 日志条目合并历史节点

> 状态：待处理（本轮新增，由第 5 节线索 7 升级）。

- 位置：`zcv-text/src/buffer/history/state.rs:88-101`（`merge_into_current` 只要求当前节点无子节点）、`buffer/history/api.rs:136-140`（无条件 merge）、`tracking/edit_log.rs:136-153`（遇 `undo: None` 返回 `InvariantViolation`）、`buffer/transaction_pipeline/apply.rs:152-165`（回放以 `None` 提交逆编辑）、`buffer/history/api.rs:57-64, 75-83`（先动游标后回放，失败不回滚）。
- 证据：`edit(A) → undo（回放条目 undo:None）→ redo（回放条目 undo:None）→ edit(B, MergeWithPrevious)` 时 `merge` 把节点区间扩到包含回放条目，再次 `undo` 在 `undo_batches` 命中 `undo: None` 报 `InvariantViolation`，且游标已前移、文本未回退。超过默认 16MiB 的 `SkipHistory` 事务后接 `MergeWithPrevious` 同理（`config/large_file.rs:45-46`）。生产 `MergeWithPrevious` 仅 IME 组合路径（`zcv-editor/src/view/input.rs:130-137, 715-723`）。
- 违反不变量：T-7、5.2、5.3。
- 影响：undo 返回内部错误、无法撤销，历史与文本分叉。
- 目标边界：回放路径像普通提交一样记录逆编辑（`apply.rs:29-32`），或 `merge_into_current` 增加版本连续/可回放前置条件；undo/redo 先取齐全部编辑再推进游标或失败回滚。删除无条件 merge 与回放写 `undo: None` 的旧路径。
- 定向验证：`edit(A) → undo → redo → edit(B, MergeWithPrevious) → undo` 断言 undo 成功返回 v0 文本。

#### T-D（中）回放条目 `undo: None` 使 `text_for_version` 在任意 undo/redo 后失效

> 状态：待处理（本轮新增）。

- 位置：`zcv-text/src/tracking/edit_log.rs:159-176`（`reverse_batches` 要求全部条目有 undo）、`zcv-text/src/snapshot.rs:159-186`（`text_for_version` 除当前版本外都走 `reverse_batches`）、`buffer/transaction_pipeline/apply.rs:152-165` 与 `buffer/history/api.rs:120-126`（回放无逆编辑）。
- 证据：`edit(A) → undo` 后文本恰为 v0，但 `text_for_version(v0)` 的区间包含回放条目 [v1,v2]（`undo: None`）→ `HistoryTextUnavailable`；`text_for_version(v1)` 同样失败。第 4.5 节 T-A、`zcv-text/README.md:26`、架构文档 §18.4 只把该错误归因于 `SkipHistory` 大事务。
- 违反不变量：T-7。
- 影响：T-7 的「历史文本重建」只要发生过一次 undo/redo 就不可用；当前无生产消费者，但按 §5.3 迁移 diff/Git 对比时会直接踩到。
- 目标边界：回放条目记录可重放逆编辑（回放的逆即 redo，`build_inverse_edit_list` 已具备，`buffer/history/api.rs:171-201`）。
- 定向验证：`edit(A) → undo() → snapshot.text_for_version(v_before_A)` 返回旧文本而非 `HistoryTextUnavailable`。

### 4.6 工程边界层

#### E-G（中）项目搜索自建 Buffer 并注册为该路径权威文档，绕过文件解码边界

> 状态：已修正（本轮）。

- 落地：文件解码与文本 Buffer 创建收敛到 `zcv-project/src/buffer_store.rs` 的 `load_buffer`（`EncodingConfig::default()` 的 `BomPolicy::Strip`）；后台搜索经它读取只读文本视图匹配，`FileSearchResult::loaded_buffer` 与 `Project::register_loaded_buffer` 已删除；UI 对每个命中路径调用 `Project::open_buffer` 打开并复用权威文档。

- 位置：`zcv-project/src/search/mod.rs:157-172`（`std::fs::read_to_string` 与 `Buffer::from_text`）、`zcv-search/src/project_search.rs:299-307`（`register_loaded_buffer`）、`zcv-project/src/buffer_store.rs:70-102`。
- 证据：搜索遇到未打开文件时执行 `std::fs::read_to_string(path)` 并 `Buffer::from_text(text, BufferConfig::default())`，结果携带 `loaded_buffer`；UI 线程把它经 `Project::register_loaded_buffer` 登记进 `BufferStore`，成为该路径后续复用的权威文档。而 `Project` 文件边界默认解码是 `EncodingConfig::default()` 的 `BomPolicy::Strip`（`zcv-project/src/text_file.rs:51-58`），`read_to_string` 不剥离 BOM。
- 违反不变量：P-1（同一路径只有一个权威文档实体，含内容语义）、11.1（解码与 Buffer 创建属 Project 文件边界）、3.1。
- 影响：带 BOM 的文件若先被项目搜索读到、再被编辑器打开，编辑器沿用的 Buffer 首字符为 U+FEFF；反之先打开则被剥离。同一路径的权威文本内容取决于哪条入口先物化它；未来编码/非法 UTF-8 策略变化也会漏过搜索路径。
- 目标边界：文件解码与 Buffer 创建只有唯一入口（Project 文件边界 + `decode_to_string`）；搜索不再自行建 Buffer，而经 Project 请求「按路径加载并复用文档」；删除 `FileSearchResult::loaded_buffer` 旁路。
- 迁移项：`zcv-project/src/search/mod.rs`、`project_store.rs::search`、`zcv-search/src/project_search.rs`、`zcv-project/src/test/search_tests.rs`。
- 定向验证：写一个带 BOM、未被打开的文件；让 `Project::search` 成为该路径首个物化者；断言返回文档首字符无 U+FEFF，且与 `Project::open_buffer` 首次打开的字节一致。

#### E-H（低）`Item`/`ItemHandle` 能力标志超出 11.2 的「只通过 show_toolbar 与面包屑数据」

> 状态：已修正（本轮）。

- 落地：`Item`/`ItemHandle` 删除 `uses_editor_document_toolbar` 与 `receives_git_projection`；`DocumentToolbar` 按「`act_as_type` 暴露的编辑器实体是否就是 Item 自身」判定，`refresh_pane_git_projection` 按「暴露编辑器且有单文件身份路径」判定。

- 位置：`zcv-workspace/src/item.rs:51-65, 176-178, 247-253`；消费方 `zcv-search/src/buffer_search.rs:92`、`zcv-version-control/src/editor_diff.rs:32`。
- 证据：`Item`/`ItemHandle` 暴露 `uses_editor_document_toolbar`（决定通用文档工具栏位置）与 `receives_git_projection`（决定是否注入 git 投影），超出 11.2 描述的能力面。
- 影响：新增文档形态需追加 capability 标志，工具区/投影注册规则分裂在 Item 与注册方之间；不涉及所有权转移。
- 目标边界：工具项/投影注册方自行按 Item 类型或明确能力决定；Item 只保留 `show_toolbar` 与面包屑；若保留则应在架构文档中登记为显式实现差异。
- 迁移项：`zcv-workspace/src/item.rs`、`zcv-search/src/buffer_search.rs`、`zcv-version-control/src/editor_diff.rs` 及相关测试。
- 复审补充（本轮）：两个谓词不涉及所有权转移；但默认值均为 `true`，新文档形态忘记覆盖即隐式落入通用工具栏/投影，属隐式默认；模块头注释（`item.rs:1-4`）与主接口实现自相矛盾。

### 4.7 架构契约层

#### H-1（中）架构文档对「内容代际」的叙述自相矛盾，且与代码、原型均不符

> 状态：已修正（本轮，按「以 Zed 原型为准」处理）。架构文档 §4.2 与 T-8 已删除「内容代际 / `reset` / 显式重锚」，统一为「外部更新算差异后作为普通版本化编辑提交 + 单一版本化坐标链 + 解析失败显式暴露」，与 §5.3/§5.4、代码及 Zed `crates/text` 一致。

- 位置：[docs/编辑器架构.md](docs/编辑器架构.md) §4.2（第 201、203 行）、§15 T-8（第 284、823 行）与 §5.3（第 256 行）、§5.4（第 268 行）。
- 证据：旧模型措辞仍在——「`Anchor` 绑定**内容代际**」「reset / 基线替换开启新代际……必须走**显式重锚**」「T-8：代际被 reset / 基线替换……调用方丢弃或**重锚**」；新模型措辞为「锚点只绑定版本和吸附方向……**不建立代际或重锚旁路**」「显示层、组合投影与语法层不得引入独立的 `reset` 或代际信号」。代码侧已无代际：`Anchor` 只含 `version/offset/affinity`（`zcv-text/src/tracking/anchor.rs:15-21`），`AnchorError` 无代际变体（`zcv-text/src/errors.rs:93-114`），`rebase_across_generations` / `BufferGeneration` / `TextChangeBatch::reset` / `requires_reset` 全仓 0 命中，外部更新走 `replace_text`（`zcv-text/src/buffer/replace.rs:18-32`）。Zed `crates/text` 也没有 generation / reset / rebase。
- 违反不变量（修正前）：§1.4「本文是架构契约」、§19.2「当修改改变职责、所有权、生命周期或依赖方向时同步更新本文」；T-8 的旧措辞会诱导后续实现重新引入代际旁路，直接冲突 §5.3/§5.4。
- 目标边界（已落地）：T-8 与 §4.2 统一为「单一版本化坐标链 + 解析失败显式暴露」，删除「代际 / 显式重锚」措辞。
- 定向验证（已执行）：全文档 `代际|重锚|reset` 检索剩余出现均为否定表述（「不存在内容代际」「不建立代际或重锚旁路」「不得引入独立的 `reset` 或代际信号」），无与 §5.3/§5.4 冲突的内容。

#### H-2（中）架构文档 §6.3/§6.6 的注册表版本补解析与 L-A 裁剪冲突

> 状态：待处理（本轮新增）。

- 位置：`docs/编辑器架构.md:316`（§6.3）、`:332`（§6.6）与 `:986-993`（§18.3）、`:954-962`（§17）；`zcv-language/src/registry.rs:196-199`（无版本字段/访问器）。
- 证据：§6.3/§6.6 明确要求「注册表版本变化驱动注入层补解析」，而 Zcv 注册表由 `builtin_languages()` 静态装配、无运行期注册入口；第 7.1 节已把该能力登记为产品裁剪，但 §17/§18.3 未登记，§15 L-8 也只保留「Pending 不静默丢弃」。
- 违反不变量：§1.4、§19.2（与 H-1 同类）。
- 影响：契约与实现、台账互相矛盾；后续维护者会按契约重新引入没有消费路径的注册表版本机制。随仓库分发的 `.scm`（graphql/glsl/latex 等）产生的 Pending 层永远无高亮且无恢复入口。
- 目标边界：在 §18.3 登记该裁剪并同步 §6.3/§6.6 表述；或在 `registry.rs` 引入唯一版本入口。只保留一处权威登记。
- 定向验证：检索 §18.3 是否存在该条目；若保留 §6.3/§6.6 表述则应存在运行期注册 API 与版本推进测试。

#### H-3（低）架构文档 §14.1 依赖箭头未反映实际依赖

> 状态：待处理（本轮新增）。

- 位置：`docs/编辑器架构.md:812-825`；`zcv-editor/Cargo.toml:27, 31`；`zcv-project/Cargo.toml:9-25`。
- 证据：§14.1 箭头图把 `zcv-project / zcv-workspace` 画在 `zcv-editor` 之上，实际是 `zcv-editor` 依赖 `zcv-project`/`zcv-workspace`（与 Zed `crates/editor` 一致）；第 7.2 节已登记为实现差异、第 8 节建议对齐，但文档未改。
- 违反不变量：§1.4、§19.2。
- 目标边界：在 §14.1 明确「编辑器核心依赖 Project/Workspace 的 Item/Provider 协议」，并把箭头图按实际依赖修正。
- 定向验证：文档检索 `zcv-editor` 依赖方向与 Cargo.toml 一致。

#### H-4（低）架构文档 §18.1 与 §18.2 重复登记 R-E

> 状态：待处理（本轮新增）。

- 位置：`docs/编辑器架构.md:975`（§18.1 行内元素实测宽度回写）与 `:982`（§18.2 R-E）；`:984`（§18.2 历史说明）。
- 证据：同一「实测宽度回写」同时登记在「尚未复刻」与「复刻偏离」；§18.2 还保留「此前登记……均已按目标落地」的历史说明，违反 §19.2。
- 违反不变量：§19.2。
- 目标边界：§18.1 只保留 `font_id_for_index` 与 invisible/whitespace 两项；R-E 只留 §18.2；删除 §18.2 历史说明。
- 定向验证：§18.1/§18.2 无重复条目。

#### H-5（低）架构文档 D-8/§8.2「同构段不得被合并」与实现相反

> 状态：待处理（本轮新增）。

- 位置：`docs/编辑器架构.md:525`（§8.2）、`:889`（§15 D-8）；`zcv-editor/src/display_map/fold_map.rs:1300-1324`、`block_map.rs:573-584`；Zed `crates/editor/src/display_map/fold_map.rs:413-431, 1073`。
- 证据：Zcv 与 Zed 都显式合并相邻同构变换；Zed 的实际不变量是「不得存在相邻同构变换」。
- 违反不变量：§1.4、§19.2（与 H-1 同类措辞问题）。
- 目标边界：把 D-8/§8.2 改述为「相邻同构变换必须合并为规范形；input 必须精确覆盖下层」。
- 定向验证：文档表述与 `push_isomorphic`/Zed 一致。

### 4.8 渲染层

#### R-A（高）行布局是单一整行塑形结果，不能承载行内元素

> 状态：已修正（本轮）。`EditorLayout` 的行改为 `FragmentedLine { fragments, text }`，`LineFragment::{Text, Element}`；`x_for_index` / `closest_index_for_x` 跨片段解析，被替换文本仍计入行索引空间。跨片段坐标回归测试见 `folded_element_participates_in_cross_fragment_coordinates`。

- 位置：`zcv-editor/src/element.rs:110-127`（`LayoutLine { shaped: ShapedLine, ... }`）、`element.rs:2529-2544`（逐行一次性 `shape_line`）、`element.rs:264-266, 313, 345-346`（坐标/命中只经 `shaped`）。
- 证据：`EditorLayout` 的每个 `LayoutLine` 持有一个 `ShapedLine`；行文本、run 背景、whitespace、选区片段与 caret 全部按该单一 `ShapedLine` 的 `text`/`x_for_index`/`closest_index_for_x` 计算，没有任何「片段序列」结构。
- 原型：Zed 的行是 `LineWithInvisibles { fragments: SmallVec<[LineFragment; 1]> }`，`LineFragment::Text(ShapedLine) | Element { id, element, size, len }`（`crates/editor/src/element.rs:7426-7452`）；坐标按片段累加（`element.rs:8281-8338`）。
- 违反不变量：R-5、R-8。
- 影响：折叠占位符 `render`、inlay 等任何行内替换都无处安放；要落地只能另起渲染路径或退化为文本，直接阻塞 §10.2 目标。命中、选区与宽度也无法表达元素的像素占位。
- 目标边界：`EditorLayout` 的行改为片段序列；`x_for_index`/`index_for_x`/命中/宽度查询按片段解析；被替换文本仍计入行索引空间。
- 迁移项：`zcv-editor/src/element.rs`、`zcv-editor/src/display_map.rs`（chunk 出口）、`zcv-editor/src/test/element_tests.rs`、`zcv-editor/src/view/test/*`。
- 定向验证：对含折叠占位符的行断言 `x_for_index`/`index_for_x` 跨片段往返一致；元素区间命中返回元素边界；无元素行行为不变。

#### R-B（中）chunk 流没有行内替换描述与渲染描述

> 状态：已修正（本轮）。`Chunk` 增加 `renderer: Option<ChunkRenderer>`，折叠占位符 chunk 携带行内替换描述，普通文本 chunk 为 `None`。回归测试 `fold_placeholder_chunk_carries_inline_renderer`。

- 位置：`zcv-editor/src/display_map/chunk.rs:40-55`（`Chunk { is_placeholder, ... }`）、`chunk.rs:599-674`（`FoldChunks` 只透传 `is_placeholder`）、`zcv-editor/src/display_map/fold_map.rs:1498-1507`（`FoldRowSegmentKind::Placeholder { text }`）。
- 证据：chunk 对折叠占位符只携带一个 `is_placeholder: bool`，渲染端据此把文本染成占位色；没有替换描述、渲染描述，也没有「被替换文本 + 元素」的组合。
- 原型：Zed 的 `HighlightedChunk` 携带 `replacement: Option<ChunkReplacement>`，`ChunkReplacement::Renderer(ChunkRenderer)`（`crates/editor/src/display_map.rs:1405-1417`）；`Chunk.renderer: Option<ChunkRenderer>`（`fold_map.rs:1422-1444`）。
- 违反不变量：R-6、R-9。
- 影响：行内替换在显示投影层没有载体，渲染层无从得知「这段文本应由元素替代」。
- 目标边界：chunk 携带行内替换描述（至少 `render` + `constrain_width` + 稳定身份）；折叠占位符由生产方填入，其它 chunk 保持 `None`。
- 迁移项：`display_map/chunk.rs`、`display_map/fold_map.rs`、`display_map.rs`。
- 定向验证：折叠占位符 chunk 带替换描述；普通文本 chunk 为 `None`；渲染端只对带描述者走元素路径。

#### R-C（中）折叠占位符没有 render 回调，constrain_width 未被消费

> 状态：已修正（本轮）。`FoldPlaceholder` 增加 `render`；折叠变换把稳定身份、锚点范围与 `constrain_width` 一并包成 `ChunkRenderer`。`constrain_width` 决定元素收到的可用宽度。回归测试 `constrain_width_bounds_element_fragment`。

- 位置：`zcv-editor/src/display_map/fold_map.rs:164-200`（`FoldPlaceholder` / `TransformPlaceholder` 字段）、`fold_map.rs:1067-1071`（`constrain_width`/`type_tag` 只写入变换）。
- 证据：`FoldPlaceholder` 有 `collapsed_text`/`constrain_width`/`merge_adjacent`/`type_tag`，没有 `render`；`constrain_width` 只被复制到 `TransformPlaceholder`，全仓无读取点。占位符内容与形状完全由渲染层内置。
- 原型：Zed `FoldPlaceholder.render: Arc<dyn Fn(FoldId, Range<Anchor>, &mut App) -> AnyElement>`（`fold_map.rs:26-39`），折叠变换构造时把它包成 `ChunkRenderer`（`fold_map.rs:597-610`）。
- 违反不变量：R-6。
- 影响：宿主无法自定义折叠占位符（内联编辑提示、语言服务占位 UI、交互元素都做不到）；`constrain_width` 语义不存在。
- 目标边界：`FoldPlaceholder` 增加 `render`；折叠变换把它与稳定身份、`constrain_width` 一起交给 chunk 渲染描述。
- 迁移项：`display_map/fold_map.rs`、其调用方（`view/mod.rs`、`display_map.rs`）、`display_map/test/fold_map_tests.rs`。
- 定向验证：默认占位符与自定义 `render` 都能布局成元素；`constrain_width` 真/假分别约束/放开宽度。

#### R-D（低）行布局可脱离 Element 生命周期计算，元素生命周期契约不存在

> 状态：已修正（本轮，生产契约）。元素在布局阶段 `layout_as_root` 测量，行原点（含水平平移）最终确定后统一 `prepaint_at`，绘制阶段只 `paint`。回归测试 `inline_element_prepaints_at_the_final_translated_origin`。附注：测试 helper 仍直接调用 `layout_visible_lines_from_viewport`（仅包在 `Window::draw` 内，`zcv-editor/src/test/element_tests.rs:33`），台账此前「测试 helper 改为经真实生命周期驱动」的描述不准确；生产契约已满足。

- 位置：`zcv-editor/src/element.rs:108-152`（`LayoutLine` 可 `Clone`）、`element.rs:517-518`（`PrepaintState.layout: Arc<EditorLayout>`）、`element.rs:2406-2564`（`layout_visible_lines_from_viewport` 可直接被测试调用）。
- 证据：当前行布局只是塑形，不涉及不可克隆的元素，因此可以在 `request_layout`/`prepaint` 之外（如测试 helper）直接调用；`EditorLayout` 放进 `Arc` 以共享。
- 原型：Zed 的行布局产生不可克隆的 `LineFragment::Element`（`AnyElement`）；元素先在布局阶段 `layout_as_root`，再在行原点（含滚动平移）最终确定后 `prepaint_at`，绘制阶段才 `paint`（`crates/editor/src/element.rs:7540-7575, 7855-7900`）。
- 违反不变量：R-7。
- 影响：一旦引入元素，现有「布局函数可在任意上下文调用」「布局 `Arc` 共享后不可再变」的隐含假设会失效；元素会在错误原点 prepaint 或无法 prepaint。
- 目标边界：明确行布局与元素测量的生命周期；元素 prepaint 固定在行原点最终确定之后；不再假设行布局结果可克隆或脱离生命周期计算。
- 迁移项：`zcv-editor/src/element.rs`、`zcv-editor/src/test/element_tests.rs`。
- 定向验证：含元素的行在滚动/水平平移后元素出现在正确原点；测试 helper 不再脱离生命周期直接布局元素。

#### R-E（低中）行内元素实测宽度未回写显示层，`ChunkRendererId` 从渲染片段丢失

> 状态：待处理（渲染层目标确认为 Zed 架构后新增，本轮）。

- 位置：`zcv-editor/src/element.rs`（`LineFragment::Element` 无 `id`；`EditorElement::prepaint` 末无宽度回写）、`zcv-editor/src/display_map/fold_map.rs`（`ChunkRenderer` 无 `measured_width`）、`zcv-editor/src/display_map.rs`。
- 证据：Zcv 只在当前行内按 `size.width` 累加 x；`ChunkRenderer` 只有 `id`/`render`/`constrain_width`，`id` 在构造渲染片段时被丢弃，全仓无 `update_renderer_widths`/`update_fold_widths`/`measured_width`。
- 原型：Zed 在 `EditorElement::prepaint` 收集 `(ChunkRendererId, size.width)` 调 `Editor::update_renderer_widths`（`crates/editor/src/element.rs:9178-9205`），后者经 `FoldMap::update_fold_widths` 更新 `ChunkRenderer.measured_width`、在宽度变化时返回 edit 并触发再次 prepaint（`crates/editor/src/fold.rs:710-717`、`display_map/fold_map.rs:317-340, 1460-1468`）。
- 违反不变量：R-9 的「实测宽度都必须回写，供后续布局与依赖宽度的查询复用」；同时影响 8.4/D-5：宽度变化应经显示层增量推进，而不是停留在单帧渲染内。
- 影响：自定义折叠占位符（以及未来 inlay）的实测宽度不能影响软换行与后续宽度查询；`constrain_width` 只在单行内生效，跨帧/跨层不闭环。
- 目标边界：`LineFragment::Element` 重新携带 `ChunkRendererId`；`ChunkRenderer` 记录 `measured_width`；渲染层在 prepaint 后收集并回写 `DisplayMap`/`FoldMap`，宽度变化时经显示版本推进并触发再次 prepaint；渲染层不得维护第二份宽度权威。
- 迁移项：`element.rs`、`display_map/fold_map.rs`、`display_map.rs`、`display_map/chunk.rs`、`display_map/test/fold_map_tests.rs`、`zcv-editor/src/test/element_tests.rs`。
- 定向验证：含自定义元素的折叠行在 prepaint 后宽度回写到折叠层；下一帧换行/宽度查询使用实测宽度；宽度未变时不产生显示 edit。

> 另：字体回退下的行内列位置（Zed `font_id_for_index`）与完整 invisible/whitespace 策略（Zed `Invisible` 的 tab／空格／换行标记及设置驱动）属尚未复刻的通用渲染能力，登记于架构文档 §18.1，不计入本表；LSP 诊断绘制（diagnostic underline、point diagnostics）随 LSP 裁剪（架构文档 §17）。

#### R-F（中）水平窗口化行的选区/装饰几何把整行列用于窗口内文本

> 状态：待处理（本轮新增）。

- 位置：`zcv-editor/src/element.rs:3149, 3163, 3167`（选区）、`:3206, 3220, 3224`（词级 diff）、`:3390, 3405, 3406`（括号背景）、`:1579-1590`（折叠占位符 hitbox）、`:3505-3509`（`column_to_byte`）。
- 证据：未换行时渲染层按水平滚动建立列窗口，`row_text` 只含窗口内 chunk，`window_start_column` 只用于命中/光标/UTF-16/空白的显式补偿（`:439-445, 3482-3487, 520-521, 2904, 3077-3081`）；但选区/词级 diff/括号背景直接以整行列 `column_to_byte(&line.line.text, column)` 取窗口文本，折叠 hitbox 也以整行合并字节对窗口文本取 `x_for_index`。
- 违反不变量：R-8、10.2、4.1。
- 影响：默认 `SoftWrap::None` 下水平滚动超长未换行行后，选区/括号/词级 diff 高亮整体偏移窗口起点；折叠热区错位。
- 目标边界：统一走显式「整行列/合并偏移 → 窗口内偏移」换算（复用 `byte_for_display_column(..., window_start_column, ...)` 或 `window_prefix.len()`），不得各调用点各自近似。
- 定向验证：滚动出窗口起点 >0 后断言高亮矩形 x 与光标/命中 x 一致。

#### R-G（中）水平窗口裁剪折叠占位符时静默丢弃 `ChunkRenderer`

> 状态：待处理（本轮新增）。

- 位置：`zcv-editor/src/display_map/chunk.rs:631-647`（`unclipped.then(|| renderer.clone())`）；消费 `element.rs:2912-2925`。
- 证据：占位符段被窗口部分覆盖时 `renderer: None`（`chunk.rs:646`），渲染层当普通文本绘制并只染占位色；同一折叠的渲染方式取决于水平视口。
- 违反不变量：R-6、10.2；「推导失败不得静默回退」。
- 影响：自定义折叠占位符/未来 inlay 在水平滚动裁到边缘时消失；该帧 `constrain_width` 与实测宽度语义断裂。
- 目标边界：占位符段始终携带 `ChunkRenderer`，可见性由 content mask 裁剪；若需回退必须显式命名且对渲染层可观察。
- 定向验证：窗口部分覆盖占位符段时断言 chunk 仍带 renderer、仍生成元素片段。

#### R-H（低）`FoldPlaceholder::ellipsis` 构造时捕获主题色

> 状态：待处理（本轮新增）。

- 位置：`zcv-editor/src/display_map/fold_map.rs:265-276`；调用方 `zcv-editor/src/view/mod.rs:700`；折叠变换跨同步克隆 `fold_map.rs:104-116`；主题观察者 `view/mod.rs:1674-1684`。
- 证据：`text_color` 在 `ellipsis(cx)` 构造时被闭包捕获，主题切换后已有折叠仍用旧色，直到重新折叠。
- 违反不变量：10.2。
- 目标边界：`render` 闭包在调用时读取当前主题（对照 Zed `crates/editor/src/display_map.rs:978-996`），或由主题变化驱动重建。
- 定向验证：折叠 → 切主题 → 布局，断言元素使用新主题色。

---

## 5. 待验证线索

以下条目有代码证据但未能证明实际用户影响或可达性，不作为已确认差距；需定向测试或运行时复现后再决定是否登记。

1. ~~`SelectionHistory` 只增不减与失败会话孤儿（E-D）~~ **本轮已确认并修正**：记录有界化，失败会话结束即删除自身记录；回归测试见第 4.3 节 E-D。
2. ~~`BlinkManager` detach 任务（E-E）~~ **本轮已确认并修正**：持有在途 `Task`，禁用即取消；回归测试见第 4.3 节 E-E。
3. ~~`LineWidthCache` 仅按文本版本失效（D-E）~~ **本轮已确认并修正**：缓存改以显示版本失效，tab 宽度变化即可命中旧宽度的问题已由 `longest_line_width_cache_invalidates_when_only_the_display_changes` 覆盖。
4. ~~`Editor` 第二个 `DisplayMap`（placeholder）（E-F）~~ **本轮判定不构成 E-1 违反，已关闭**：placeholder 携带独立 `Buffer`，只在真实文档为空时作为展示内容源接入同一渲染管线，不是同一文档的第二份可写权威；与 Zed `placeholder_text` 的差异登记进架构文档 §18.4。
5. ~~`GitStore` 的 `optimistic_index_bases` 构成第二份 index 文本~~ **本轮复核判定不成立，已关闭**：该 map 只保存「写入前旧 index 文本 + 在途写入互斥 + 失败回滚基线」，权威始终是 `revision_documents[(Index, path)]` 的唯一 `Entity<LanguageBuffer>`；写入后不可变、单写者，随 `ApplyHunkEdits` job 终结即移除（`zcv-project/src/git_store/mod.rs:270-271, 638-641, 660-668`；`zcv-project/src/git_store/snapshots.rs:160-181`）。属 §3.2 允许的可丢弃缓存，不构成影子权威。
6. `no-op` 编辑仍推进版本、生成历史节点并发布事件（`zcv-text/src/buffer/edit_ops/mod.rs:5` 的声明与实现不符）；Zed 的 `apply_edit_internal` 同样保留空操作，因此版本推进本身不算原型偏离，只有注释与实现不一致需修正。
7. ~~`MergeWithPrevious` 跨过未记录历史的编辑可能使 undo 命中 `undo: None` 的编辑日志条目并报 `InvariantViolation`~~ **本轮升级为已确认差距 T-C（第 4.5 节）**：任何一次 undo/redo 回放本身也会产生 `undo: None` 条目，`merge_into_current` 缺版本连续性守卫；未做运行时复现。
8. 结构变更入口未断言文本事务之外（C-E）；未找到实际在事务内触发的调用方。
9. 注入层裸偏移 + 手工映射（D-C/L-B）：范围已改 `Range<Anchor>`、增量走 `coordinate_edits_since`、`map_range_through_changes` 已删除，「裸偏移手工映射」已消除；D-C 已改为字节偏移变换。但 `coordinate_edits_since` 为 `None` 时的整层丢弃/全文重跑分支仍在（L-B 部分修正），按坐标索引语义当前不可达。
10. `LineWithInvisibles` 中与行内元素无关的其余渲染能力，已在「渲染层目标确认为 Zed 架构」后重新分类：元素实测宽度回写是已确认差距 **R-E**（违反 R-9，见 §4.8）；`font_id_for_index` 与 invisible/whitespace 完整策略登记为架构文档 §18.1「尚未复刻」；诊断下划线 handler、point diagnostics 由 LSP 诊断驱动，随 LSP 裁剪（架构文档 §17）。原「当前阶段不实现」的结论已不再适用。
11. T-A/T-B 新增能力目前只有测试消费者：`Snapshot::text_for_version`/`offsets_to_version`/`range_to_version`、`snapshot_with_edits`/`fast_forward`/`EditedBufferSnapshot` 全仓非测试调用为 0；Git 修订仍以 `Buffer::from_text` 建独立实体（`zcv-project/src/git_store/mod.rs:1102-1142`），diff 仍全文物化（`zcv-buffer-diff/src/buffer_diff.rs:289-300, 451`）。是否算差距取决于是否按 §5.3 迁移消费者。
12. `Snapshot::has_edits_since(_in_range)` 用「净 Patch 是否为空」实现（`zcv-text/src/snapshot.rs:109-122`），不是 Zed 的 fragment 可见性语义（`crates/text/src/text.rs:2781-2822`），`has_edits_since_in_range` 取旧坐标 `TextRange` 而非 `Range<Anchor>`；「删除后又原位插回同一文本」等序列可能给出不同答案，因无生产消费者未确认。
13. `SelectionSet<MultiBufferAnchor>::resolve` 对不可解析锚点 `filter_map` 丢弃，全部失败时静默返回 caret 于 ZERO（`zcv-editor/src/selection/selection_set.rs:140-153`），与 T-8/E-4 的显式失败要求相悖；`resolve_anchor_in_mappings` 仅在锚点版本完全无法映射时返回 `None`，可达性罕见。
14. `EditorSearch::activate_match_in_direction` 先从旧 `ranges` 缓存取 `range`，再 `advance_snapshots`（可能重建 `self.search`），然后把旧 `range` 交给 `select_byte_range`（`zcv-editor/src/view/search.rs:186-198`），属同一命令混用新旧快照（§9.2），越界会触发 `select_byte_range` 的 `assert!`；未证版本在期间变化。
15. `LocalRenameState` 长期保存裸组合 offset 与 `Range<usize>`（`zcv-editor/src/view/rename.rs:24-34`），提交仅校验版本（`:206-218`），但每帧无条件下发淡化范围（`:302-306`、`view/mod.rs:1133-1138`）；外部编辑期间可能落到错误位置。
16. `Editor.hovered_diff_hunk: Option<usize>` 以 hunk 列表下标作跨帧身份（`zcv-editor/src/view/mod.rs:277, 550-555`），diff 重算后下标可能漂移（与已修 C-D 同类）；当前每帧按鼠标位置重判可掩盖。
17. `ScrollManager::refresh` 在滚动锚点解析失败时静默保留旧 `display_point`（`zcv-editor/src/scroll.rs:110-115`），派生缓存级、下一帧可能自愈。
18. `SelectionHistory` 上限 `MAX_SELECTION_HISTORY_ENTRIES = 1024` 为硬编码，独立于文本层 `max_edit_history_entries`（默认 1000，`zcv-text/src/config/large_file.rs:43`）；若文本预算配置 >1024，撤销/重做可能只恢复文本而丢选择。
19. `research_after_edit` 对 `SearchResultKind::External` 在版本变化时改用本地 `execute_search` 重算（`zcv-editor/src/view/search.rs:105-111, 406-429`），把外部结果集换成 `Query`；未证组合结果视图在可编辑状态下触发。
20. 外部删除文件产生的 `PathEventKind::Removed` 不清理 `BufferStore` 索引（`zcv-project/src/project_store.rs:594-601` 只处理 Changed/Created），仅程序化删除经 `remove_path`（`:449`）；若仍有强引用持有旧 `LanguageBuffer`，再次 `open_buffer` 会复用已删除内容。
21. diff 展开覆盖 `DiffExpansionState`/`HunkExpansionOverride` 以 base 行范围（裸 `Range<usize>`）作身份（`zcv-multi-buffer/src/diff_projection.rs:364-378, 1542-1566`），`migrate_expansion_state` 要求 working Anchor 版本相等（`:1601-1610`），working 版本一变即可能整体失效。
22. `MultiBuffer::edit` 对多个源按顺序 `source.edit(...)?`（`zcv-multi-buffer/src/multi_buffer.rs:4706-4712`），前者成功后者失败会留下部分已提交文本；未找到实际失败调用点。
23. `DiffState::subscribe_inputs` 的闭包忽略事件种类（`zcv-multi-buffer/src/diff_projection.rs:131-133`），base/index 的纯元数据/重解析事件也会触发一次全文 diff 重算；是否造成可测开销未确认。
24. `zcv-language` 的 `parse_slice` 用同一个 `None` 表达「预算用尽」与 `set_language`/`set_included_ranges`/`point_at` 失败（`zcv-language/src/tree_sitter_utils.rs:75-126`），`syntax_map.rs:430-449` 主树循环只在取消时退出；grammar ABI 不匹配时可能忙等。
25. `structure/folds.rs` 折叠候选两端都用 `Anchor::new`（默认 `Affinity::After`，`zcv-language/src/structure/folds.rs:138-140`），不是 §4.2 的 fold「inside」语义（终点应为 `Before`）；当前所有消费都在同版本 `resolve_in`，未产生可观察差异。
26. `zcv-editor/src/view/mod.rs:306-308, 322-324` 为 `single_line`/`auto_height` 各新建一个 `LanguageRegistry`，与 §6.6「由应用装配层创建并注入」不同；这些 buffer 无 `file_path`，当前无实际影响。
27. `EditorInputLayout` 把整份 `EditorLayout`（含 `LineFragment::Element` 的 `RefCell<Option<AnyElement>>`）以 `Rc` 存回 `Editor.input_layout`（`zcv-editor/src/element.rs:2199-2203`、`view/mod.rs:264`）；paint 阶段元素已被 `take`，未观察到跨帧复用，但 prepaint 后未 paint 时可能滞留一帧。
28. 水平窗口起点列是显示列（含 CJK 宽字符，`zcv-editor/src/display_map/chunk.rs:856`），而选区范围列来自字符计数语义（`wrap_map.rs:717-851`）；含 CJK 时即使补偿 `window_start_column` 仍可能不同空间。
29. `zcv-editor/src/display_map/tab_map.rs:273-277` 仍剥离行终止符，被 `fold_map.rs:836-865` 的 `row_text` 消费用于 Tab/Fold 坐标换算与 wrap 度量；不在 shaping 热路径，未构成 D-B 实际违反。
30. Fold 层缺本层 input 精确覆盖断言与相邻同构 canonical-form 检查（Block 有 `debug_assert_eq!`、Wrap 有 `check_invariants`，Fold 无）；未构造可达路径。
31. 空同步也无条件推进显示版本（`zcv-editor/src/display_map.rs:899-901` 的 `next_display_version`），会让按显示版本失效的缓存（`LineWidthCache`、`syntax_crease_cache`）在无输入变化时重建；非不变量违反。
32. 未换行模式每帧遍历全部 Tab 行求最长行（`zcv-editor/src/display_map.rs:981-993`，`element.rs:1366-1373`），只有最终行长经 `LineWidthCache` 缓存，行扫描未缓存；未做性能复现。
33. `display_map` 与 `scrollbar` 同 crate 互相引用（`display_map.rs:30, 263-295`、`scrollbar.rs:9`）；§14.2 允许同 crate 互相引用，`DisplaySnapshot::scrollbar_marker_groups` 只接收 track 几何而非交互状态，未判定越界。

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
- InlayMap：Zed 的 `crates/editor/src/display_map/inlay_map.rs` 对应 LSP inlay hints，Zcv 裁剪 inlay 生产者与 LSP 注入路径；但 inlay 与折叠占位符共享的行内替换渲染底座（`ChunkReplacement`/`ChunkRenderer`/片段化行布局/跨片段坐标）不是 LSP 专属能力，已纳入对齐目标（见第 3.5、4.8 节）。
- `zcv-text` 本地 rope 存储与本地单调版本，而非 CRDT 片段树与多副本向量；保留版本、Anchor、增量与历史查询契约。
- 语言智能以 Tree-sitter 与 `.scm` 查询为边界，未建立 LSP 兼容层。
- 语言注入补解析：Zcv 保留 `SyntaxLayerContent::Pending`（未注册注入语言不再静默丢弃），但**不引入** Zed 的 `LanguageRegistry` 单调版本与注册表变化后补解析。Zcv 注册表在构造期静态装配、没有运行期注册入口，未内置语言没有可注册路径，注册表版本不可能变化；随仓库分发的 `.scm` 仍会产生 `Pending` 层，只是永不升级。该裁剪需在架构文档 §18.3 登记（见 H-2）。
- `show_headers` 策略当前恒为真、无 `without_headers` / `buffers_with_disabled_headers` 生产方；Zcv 无消费方，属对 Zed split-diff / 禁用 entity header 能力的裁剪。
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

1. 第 18.2 节「复刻偏离」当前只登记 R-E；本轮已确认的代码偏离应一并登记并按消除状态删除：D-H、T-C、T-D、M-D、M-E、M-F、E-I、E-J、E-K、R-E、R-F、R-G、R-H。D-A 与 L-B 已在本轮彻底对齐（见顶部「对齐落地」记录），不再需要登记。原「此前登记……已按目标落地」的历史说明按 §19.2 删除。
2. 第 18.1 节不得把 T-A/T-B 登记为「尚未复刻」：`Snapshot` 已提供 T-7 能力，`EditedBufferSnapshot`/`fast_forward` 已提供 T-9 入口。第 18.4 节的实现差异须补正：`text_for_version` 不仅跨 `SkipHistory` 失败，任何一次 undo/redo 回放后也返回 `HistoryTextUnavailable`（T-D）；`fast_forward` 经版本校验后以正常事务路径重放派生编辑，而非直接置换 storage。
3. 第 18.4 节保留「Editor 持有第二个 placeholder DisplayMap」与「`show_headers` 无 `without_headers` / `buffers_with_disabled_headers` 生产方，属裁剪」；并按 H-3 把 §14.1 依赖箭头与实际依赖（editor → project/workspace）对齐说明。
4. 契约层 H-2…H-5 按各自目标边界修正文档：H-2 在 §18.3/§17 登记语言注册表版本补解析裁剪并同步 §6.3/§6.6；H-3 修正 §14.1 依赖图；H-4 去重 §18.1/§18.2 的 R-E 登记；H-5 改述 D-8/§8.2 的「同构段不得被合并」。
5. 第 18.1 节登记两项尚未复刻的通用渲染能力：`font_id_for_index` 与 invisible/whitespace 完整策略；第 18.3 节登记「渲染层不实现 LSP 诊断绘制」。
6. 消除第 6 节 A11/C6/F-14/R5 中的任一项后，同步更新决策记录，而不是只改本审计。

---

## 9. 验证状态与未覆盖边界

- 首次审计为只读静态审计，未运行 `cargo check`、`cargo test`、基准或真实 UI/平台运行时验证；所有「定向验证」均为设计，未执行。
- 本轮复审额外执行定向类型检查：`cargo check -p zcv-text -p zcv-language -p zcv-multi-buffer -p zcv-editor -p zcv-search -p zcv-version-control` 通过（exit 0，仅有依赖 `block v0.1.6` 的既有 future-incompat 提示）；未运行 `cargo test`、clippy、基准或真实 UI/平台运行时验证。
- 对齐落地阶段（1–10）已执行：`cargo test -p zcv-editor --lib` 285/285、`cargo test -p zcv-language --lib` 82/82、`cargo test -p zcv-multi-buffer --lib` 68/68、`cargo test -p zcv-text --lib` 32/32；`cargo clippy -p zcv-editor -p zcv-language -p zcv-multi-buffer -p zcv-text --all-targets -- -D warnings` 干净；`cargo fmt --all -- --check` 干净。真实 UI/平台运行时仍未被本审计执行。
- E-G/E-H 本轮已落地并验证：`cargo check -p zcv-project -p zcv-search -p zcv-workspace -p zcv-version-control --all-targets` 与 `cargo check --workspace --all-targets` 均通过（exit 0，仅依赖 `block v0.1.6` 的既有 future-incompat 提示）；`cargo test -p zcv-search --lib` 9/9、`cargo test -p zcv-workspace --lib` 69/69、`cargo test -p zcv-preview-markdown --lib` 24/24、`cargo test -p zcv-preview-svg --lib` 7/7 通过。`cargo test -p zcv-project --lib` 103 通过、1 失败（`trashing_path_moves_file_to_system_trash`，macOS Finder 废纸篓权限，环境问题），BOM 回归 `search_decodes_unopened_files_through_the_project_boundary` 通过；`cargo test -p zcv-version-control --lib` 54 通过、1 失败（`staging_one_hunk_rebuilds_the_projection_once_after_refresh` 断言投影版本只前进一次；当时已在干净 HEAD 工作树上复现同样失败，属基线既有问题；已在本轮删除 hunk 数据模型对齐后以原断言通过，见下）。真实 UI/平台运行时未执行。
- 原「已知未通过」`zcv-text/tests/versioned_edits_anchor.rs::explicit_rebase_maps_an_old_anchor_through_a_reset` 已随代际机制删除而不存在；该文件现由 `replace_text_maps_old_anchors_through_the_same_coordinate_chain` 等用例覆盖（`zcv-text/tests/versioned_edits_anchor.rs:145`）。
- 已逐文件核对：`zcv-text` 的 buffer/tracking/transaction/snapshot/text_changes/history；`zcv-language` 的 language_buffer/syntax_map/registry/language_settings/queries；`zcv-multi-buffer` 与 `zcv-buffer-diff`；`zcv-editor` 的 display_map 各层、view、selection、scrollbar、blink、element 关键区段；`zcv-project` 的 buffer_store/text_file/project_store/search/git_store；`zcv-workspace` 的 item/pane/toolbar/workspace_state；Zed 的 `text`、`language`、`multi_buffer`、`editor/display_map`、Cargo 依赖。
- 未逐行核对：`zcv-editor/src/element.rs` 的诊断/装饰绘制分支、`zcv-editor/src/display_map/chunk.rs` 的样式与 tab 变换内部、`decorations.rs` 全部渲染内部、`zcv-editor` 除 display_map 外的锚点解析调用方、`zcv-version-control`/`zcv-preview-*` 内部、`zcv-git` 与 `git_store/background.rs`/`jobs.rs` 全文。（行布局、chunk 行内替换与折叠占位符路径已随 §4.8 做只读静态比对。）
- 本轮（架构对齐闭环）已执行：`cargo test -p zcv-editor --lib` 296/296（新增 D-A/D-G 的 `display_map/test/block_map_tests.rs`、R-A…R-D 的跨片段坐标/元素生命周期/`constrain_width`/chunk 行内替换描述、D-E 行宽缓存显示版本失效、E-D 选择历史有界化、E-E 定时任务取消，以及多 excerpt 折叠的前后缀复用回归）；`cargo clippy -p zcv-editor --all-targets --no-deps -- -D warnings` 与 `cargo fmt -p zcv-editor -- --check` 干净；`cargo check --workspace --all-targets` exit 0。`cargo test -p zcv-text` 108/108（T-A/T-B 新增 `versioned_history` 8、`derived_snapshot` 5 等）。T-A/T-B 的 `fast_forward` 安装路径未断言 storage 身份置换（无公开观测点）；真实 UI 交互（折叠占位符点击、滚动、软换行元素定位）仍由开发者手动验证。
- 运行时影响复现状态：M-A、M-B、D-B、D-D、E-A、E-B、E-C、L-B、C-C、C-D 已由各阶段回归测试覆盖（见第 2.2 节状态列的提交）；本轮新增覆盖 D-A（折叠集合／excerpt 边界增量）、D-G（input 覆盖与越界显式失败）、D-E、E-D、E-E、T-A、T-B、R-A…R-D；D-F 的运行时用户可见程度有限（`show_headers` 当前恒为真）；L-A 的未注册注入语言高亮仍未做运行时复现；E-G 的 BOM 分叉已由 `search_decodes_unopened_files_through_the_project_boundary` 覆盖。D-C 的字节偏移映射由「暂存 hunk」等结构编辑回归覆盖，行数守恒断言由精确字节映射自然成立。
- 上一轮新落地部分（`BufferId` / `ExcerptBoundary` / D-13）在本轮已纳入 `cargo test -p zcv-editor --lib` 回归并通过；其 header/divider 分类语义已与 Zed `header_and_footer_blocks` 逐条对照一致。
- 渲染层复审（渲染层目标确认为 Zed 后，本轮）：只读比对 Zcv 的 `element.rs`/`chunk.rs`/`fold_map.rs`/`display_map.rs` 与 Zed `crates/editor/src/element.rs`、`display_map.rs`、`fold_map.rs`。确认 R-A…R-D 已按 Zed 落地，新增 R-E（元素实测宽度未回写，违反 R-9）；`font_id_for_index` 与 invisible/whitespace 完整策略登记为架构文档 §18.1 尚未复刻；LSP 诊断绘制归入 §17 裁剪。R-E 及两项尚未复刻能力均未做运行时复现。
- 本轮复审未运行新的行为测试（纯只读比对）；可执行证据为前述 `cargo test -p zcv-editor --lib` 296/296、`cargo test -p zcv-multi-buffer --lib` 68/68、`cargo check --workspace --all-targets` 与 `cargo clippy --workspace --all-targets`（无本仓警告）。
- 删除 hunk 数据模型对齐（M-C 收尾）已执行：`cargo test -p zcv-multi-buffer --lib` 68/68（含此前失败的 `staging_one_hunk_rebuilds_the_projection_once_after_refresh` 以原断言通过，以及 `materialized_diff_old_side_is_selectable_but_only_new_side_is_editable` 的旧侧锚点往返）；`cargo test -p zcv-editor -p zcv-language -p zcv-version-control -p zcv-search -p zcv-workspace --lib` 分别 296/296、82/82、55/55、9/9、69/69；`cargo check --workspace --all-targets` exit 0；`cargo clippy -p zcv-multi-buffer --all-targets --no-deps -- -D warnings` 与 `cargo fmt --all -- --check` 干净。真实 UI 中展开删除 hunk 的选择/导航仍由开发者手动验证。
- 本轮独立复审（HEAD `0cb6630f`，工作树干净）：六个分层子审计逐层取证并复核第 2.2 节全部条目与第 5 节线索。可执行证据：`cargo test -p zcv-editor --lib` 296/296、`cargo test -p zcv-multi-buffer --lib` 68/68、`cargo test -p zcv-language --lib` 82/82、`cargo test -p zcv-text --lib` 35/35、`cargo check --workspace --all-targets` exit 0（仅依赖 `block v0.1.6` 的既有 future-incompat）。结论：记录条目的落地在代码层面成立；D-A、L-B 降为部分修正；新增代码差距 T-C、T-D、M-D、M-E、M-F、D-H、E-I、E-J、E-K、R-F、R-G、R-H 与契约条目 H-2…H-5。
- 本轮新增差距均为静态审计结论，未做运行时复现（除 R-E 此前亦未复现）：D-H 的缓存命中路径有既有回归 `geometry_preserving_diff_edit_reuses_diff_decorations` 佐证复用成立，但「折叠后装饰错位」未实测；T-C/T-D 的触发路径为静态推演；M-D/M-E/M-F 的并发与等长替换影响未实测；E-I/E-J/E-K、R-F/R-G/R-H 为代码级确认、UI 表现未实测。真实 UI/平台运行时仍由开发者手动验证。
- D-A/L-B 对齐落地验证：`cargo check -p zcv-editor -p zcv-language --all-targets` 与 `cargo check --workspace --all-targets` 通过；`cargo test -p zcv-editor --lib` 297/297（含新增折叠后编辑组合文档的输入覆盖回归 `editing_a_folded_composite_document_keeps_block_input_coverage`）、`zcv-language --lib` 82/82、`zcv-multi-buffer --lib` 68/68、`zcv-version-control --lib` 55/55；`cargo fmt --all -- --check` 与 `cargo clippy -p zcv-editor -p zcv-language --all-targets --no-deps -- -D warnings` 干净。diff 装饰缓存改为按块几何代际复用（D-H），由既有 `geometry_preserving_diff_edit_reuses_diff_decorations`（几何不变时复用）与折叠失效路径共同覆盖。未新增「换行编辑复用尾部变换」的 `Arc::ptr_eq` 断言回归（受 `WrapEdit` 生成路径与白盒访问限制）；该路径由既有组合编辑/折叠回归与 `rebuild` 的输入覆盖 debug 断言间接覆盖，真实 UI 行为仍由开发者手动验证。
