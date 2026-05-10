# zom-engine 当前阶段快照

## 当前阶段

- 当前推进：M14A `VersionedResult<T>` 完成；下一步进入 M14B Versioned Range Set
- 已完成：M0–M12 机器契约基线（含 M9 Anchor / Mark / TrackedRange / Selection 映射、M10 MetadataLayer 与查询、M11 LineRange 切片与 Viewport、M12 普通 / 正则搜索与替换）；M13A FoldRange / FoldSet / HiddenRange 折叠模型；M13B Projection / ProjectedLine / TextLine / FoldPlaceholder 行级折叠投影；M13C LogicalPoint / ProjectedPoint / LogicalRange / ProjectedRange 双向 point/range 映射 + selection 穿越 fold；M13D ProjectedViewport / ProjectedViewportSlice 折叠后视口切片；M14A `VersionedResult<T>` 泛型版本化结果与 PositionMap remap；GPUI testbed 覆盖至 M12
- 未完成：M14B Versioned Range Set、M14C UTF-16 边界 helper 及后续 engine-only 阶段
- 路线收口：**全部阶段按纯编辑引擎标准取舍**；Command / Macro Recording / LSP 或 Tree-sitter provider / diagnostics 专用 adapter / 后台任务调度器 / 正式 UI 绘制不进入 `zom-engine` milestone。
- 结构调整：`src/types/`、`src/config/`、`src/text_loading/`、`src/storage/`、`src/coordinates/`、`src/selection/`、`src/tracking/`、`src/transaction/`、`src/metadata/` 已按稳定能力域目录化拆分。对外 public API 收敛到 crate root re-export，目录模块作为实现分层，不承诺外部稳定 import path。
- engine-only 词汇表收敛（破坏性变更）：
  - `TransactionSource` 仅保留引擎内部分支用变体 `{ Programmatic, Composition, Undo, Redo }`；`Mouse / Keyboard / Paste / Delete / Formatter / External` 等宿主输入分类已移除，宿主自行维护并通过 `TransactionMetadata::description` 透传。
  - `MetadataLayerKind` 仅保留 `{ SearchMatch, Custom(String) }`；`Diagnostics / SyntaxHighlight / SemanticToken / Breakpoint / Bookmark / InlayHint / CodeLens` 等业务类别均迁移为 `Custom("diagnostics")` 等宿主自定义键。
  - `Buffer::insert_at_selections / replace_selections / delete_*_at_selections` 默认 `TransactionMetadata` 改为 `TransactionSource::Programmatic`，引擎不再替宿主猜测输入设备。

## M9 文件

- `src/tracking/`：Anchor / Mark、TrackedRange、删除 / 塌缩策略、批量版本推进
  - `anchor.rs`：Anchor / Mark 版本绑定与 PositionMap 跟随
  - `tracked_range.rs`：由两个 Anchor 表达的区间跟随
  - `policy.rs`：AnchorDeletedPolicy 与 TrackedRangeUpdatePolicy
  - `update.rs`：AnchorUpdate 与 TrackedRangeUpdate
- `tests/m9_anchor.rs`：M9A-M9C 机器契约测试，按子模块聚合
- `examples/gpui_m9_testbed.rs`：继承 M8 体感，并叠加 tracked range 创建、清除、移动 / 收缩 / 失效观察
- `src/lib.rs`：M9 public API 导出
- `src/errors.rs`：AnchorError 与 EngineError 接入
- `src/position_map.rs`：Selection / SelectionSet / TrackedRange 映射门面

## M10 文件

- `src/metadata/`：MetadataRange / MetadataLayer / MetadataLayers、LayerKind、range id、版本绑定、范围追踪、失效移除、LineRange / line window 查询、按 layer 查询、批量替换与过期丢弃
  - `id.rs`：MetadataRangeId 与 layer 内递增身份
  - `kind.rs`：MetadataLayerKind 通用类别
  - `line_window.rs`：M10B metadata line window 查询边界
  - `range_spec.rs`：批量替换输入规格
  - `range.rs`：单条 metadata payload 与 TrackedRange 绑定
  - `update.rs`：MetadataRangeUpdate 更新事实
  - `layer.rs`：单层 metadata ranges 管理、版本推进和查询入口
  - `layers.rs`：多 layer 集合、按 kind 查询、替换和过期丢弃
  - `query.rs`：TextRange / LineRange 查询数学
- `tests/m10_metadata_layer.rs`：M10A-M10B 机器契约测试，覆盖泛型 payload、多 layer、DeltaEvent 跟随、失效策略、基础查询、LineRange / line window 查询、按 layer 查询、批量替换与过期丢弃
- `examples/gpui_m10_testbed.rs`：继承 M9 体感，并叠加 search / diagnostics / bookmark 模拟 metadata layer 创建、跟随、查询、替换、过期丢弃与文本标记观察
- `src/lib.rs`：M10 public API 导出
- `src/errors.rs`：MetadataError 与 EngineError 接入
- `src/types/ranges.rs`：LineRange 强类型

## M11 文件

- `src/slicing.rs`：TextSlice / LineSlice / Viewport / ViewportSlice / VisibleLine public 只读切片类型、byte range / line range / viewport 到 TextRange 的边界数学
- `src/buffer/slicing.rs`：Buffer 上的 char range、byte range、单行、LineRange 与 Viewport 读取入口
- `src/snapshot.rs`：Snapshot 上与 Buffer 同形的只读切片和 viewport 读取入口
- `tests/m11_viewport_slicing.rs`：M11A-M11B 机器契约测试，覆盖 TextSlice、LineSlice、按 char / byte / line range 读取、Viewport 可见行、visible line metadata、超长行截断策略、大 line window 读取、错误边界和 Snapshot 版本只读语义
- `examples/gpui_m11_testbed.rs`：继承 M10 体感，并叠加 ViewportSlice 可见行面板、跳转光标行、滚动 viewport、行数调整、长行截断切换、大文本样本和 Snapshot viewport 预览
- `src/lib.rs`：M11 public API 导出
- `src/errors.rs`：InvalidByteRange 接入 CoordinateError

## M12 文件

- `src/search.rs`：SearchOptions / SearchResult / SearchMatch / SearchMatchMetadata / RegexSearchOptions / RegexSearchResult，以及普通字符串和正则搜索核心实现
- `src/buffer/search.rs`：Buffer 当前版本搜索入口、正则搜索入口、搜索结果单次替换与 replace all 事务入口
- `src/snapshot.rs`：Snapshot 版本绑定普通搜索与正则搜索入口
- `tests/m12_search.rs`：M12A 机器契约测试，覆盖普通搜索、大小写敏感 / 不敏感、whole word、多行、范围限定、Snapshot 搜索、SearchResult 版本绑定、MetadataLayer 挂载和 range tracking
- `tests/m12_replace.rs`：M12B 机器契约测试，覆盖搜索结果 replace、replace all 原子事务、Undo / Redo、SelectionSet 恢复、DeltaEvent、过期结果拒绝和 no-op 边界
- `tests/m12_regex.rs`：M12C 机器契约测试，覆盖正则搜索、大小写 / 范围 / 多行选项、Snapshot 正则搜索、正则替换、capture 展开、replace all 原子事务、Undo / Redo、过期结果拒绝和空匹配
- `examples/gpui_m12_testbed.rs`：继承 M11 体感，并叠加 literal / regex 搜索、搜索结果跳转、单次替换、replace all、版本过期提示和 SearchMatch metadata 高亮观察
- `src/lib.rs`：M12A public API 导出
- `src/errors.rs`：SearchError 接入 EngineError，覆盖空 query、过期结果、缺失 match 和非法正则

## M13A 文件

- `src/fold/`：FoldRange / FoldSet / HiddenRange 折叠模型，复用 TrackedRange 跟随策略
  - `id.rs`：FoldRangeId 单 FoldSet 内单调递增身份
  - `range.rs`：单条 FoldRange，绑定 BufferVersion + TrackedRange + 默认 invalidate_when_fully_deleted 策略
  - `update.rs`：FoldRangeUpdate（Mapped / Deleted / Collapsed / Invalidated）
  - `hidden.rs`：HiddenRange 半开行区间
  - `set.rs`：FoldSet 维护版本绑定、id 单调、嵌套合法、部分重叠拒绝、normalize、line-based fold、unfold/unfold_at/unfold_all、toggle、is_line_hidden、derive_hidden_ranges、update_through_delta_event
  - `geometry.rs`：M13A/B 共用的 LineGeometry trait + fold_line_span/line_boundary_offset/char_range_for_line_range helper（同时支持 Buffer 与 Snapshot）
- `src/errors.rs`：FoldError（IdOverflow / VersionMismatch / OverlapWithoutNesting / EmptyRange）接入 EngineError
- `src/lib.rs`：M13A public API 导出（FoldRange / FoldRangeId / FoldRangeUpdate / FoldSet / FoldToggleOutcome / HiddenRange / FoldError）
- `tests/m13_fold_set.rs`：22 个机器契约测试，覆盖 fold/unfold/toggle/unfold all、嵌套合法、部分重叠拒绝、line-based fold、line hidden 查询、HiddenRange 合并、编辑后 fold 跟随、保留/塌缩/失效策略、版本不匹配原子拒绝

## M13B 文件

- `src/projection/`：基于 Snapshot + FoldSet 的不可变行级折叠投影
  - `index.rs`：ProjectedLineIndex 投影行强类型索引
  - `line.rs`：TextLine / FoldPlaceholder / ProjectedLine / ProjectedLineKind / LogicalProjection（Visible / Hidden）
  - `projection.rs`：Projection 主体，承担 build(snapshot, folds)、line_count / logical_line_count、logical_to_projected、projected_line / projected_line_kind / iter、is_logical_line_hidden、fold_anchor_for_logical_line、fold_anchor_for_projected_line、is_stale_for_version；嵌套与重叠 fold 在投影空间合并为单条 placeholder
- `src/errors.rs`：ProjectionError::VersionMismatch 接入 EngineError
- `src/lib.rs`：M13B public API 导出（Projection / ProjectedLine / ProjectedLineIndex / ProjectedLineKind / TextLine / FoldPlaceholder / LogicalProjection / ProjectionError）
- `tests/m13_projection_line_map.rs`：14 个机器契约测试，覆盖空 fold 1:1 映射、单 fold placeholder 注入、双向 logical↔projected 映射、hidden 行 -> anchor 回溯、placeholder -> anchor 回溯、嵌套 fold 合并为单 placeholder、非嵌套 fold 各自独立 placeholder、intra-line fold 不产 placeholder、版本不匹配原子拒绝、projection 不可变性、line 越界返回 CoordinateError、错误经 EngineError 透传

## M13C 文件

- `src/projection/`：在 M13B 行级映射上叠加 point / range 双向映射
  - `point.rs`：LogicalPoint / ProjectedPoint 强类型 (line, column) point；LogicalPointProjection（Visible / Hidden）+ ProjectedPointMapping（Text / Placeholder）映射结果 enum，把 fold anchor / hidden_lines 等事实直接暴露
  - `range.rs`：LogicalRange / ProjectedRange 半开范围，构造器拒绝反向区间
  - `projection.rs`：扩展 logical_to_projected_point、projected_to_logical_point、logical_to_projected_range_segments（按 row kind 切换分段，跨 fold 自动展开端点）、projected_to_logical_range（placeholder 端点折叠到 anchor 或 hidden 区结束）、project_text_range（基于 Snapshot 把 Selection::range() 投影成段，多 selection 由 caller 循环调用）；新增 verify_snapshot_version 保证版本绑定
- `src/lib.rs`：M13C public API 导出（LogicalPoint / LogicalPointProjection / LogicalRange / ProjectedPoint / ProjectedPointMapping / ProjectedRange）
- `tests/m13_projection_range_map.rs`：17 个机器契约测试，覆盖 LogicalRange / ProjectedRange 反向构造拒绝、可见点直投、hidden 点回溯到 anchor、Text 投影点直回、Placeholder 投影点回到 anchor + 隐藏行区间、空范围零段、无 fold 单段、跨 fold 三段（text / placeholder / text）、起点在 fold 内收缩到 anchor、终点在 fold 内延伸过 placeholder、projected→logical placeholder 端点收敛、selection 单段 + 多选区分别投影、snapshot 版本不匹配 selection 投影原子拒绝、越界 logical point 返回 CoordinateError

## M13D 文件

- `src/projection/viewport.rs`：ProjectedViewport / ProjectedViewportSlice / ProjectedViewportRow / ProjectedViewportRowKind / ProjectedLineRange，承载折叠后视口的描述与切片结果；text 行返回 VisibleLine，placeholder 行返回 FoldPlaceholder
- `src/projection/projection.rs`：扩展 slice_viewport(snapshot, viewport) 入口，自动 clamp 末尾、汇总 logical_line_spans 与 placeholders；新增内部 build_visible_line helper（从 Snapshot 公共 API 派生 VisibleLine，包含 max_line_chars 截断与 CRLF/LF 行尾识别）
- `src/lib.rs`：M13D public API 导出（ProjectedViewport / ProjectedViewportSlice / ProjectedViewportRow / ProjectedViewportRowKind / ProjectedLineRange）
- `tests/m13_projected_viewport.rs`：8 个机器契约测试，覆盖纯文本视口、含 placeholder 视口、line_count clamp、起点超界返回 CoordinateError、max_line_chars 截断、整投影空间逻辑行 spans 合并、snapshot 版本不匹配原子拒绝、Text/Placeholder kind 解构

## M14A 文件

- `src/versioned/`：泛型版本化结果载体
  - `mod.rs`：M14 versioned 模块入口；当前只导出 `VersionedResult`，给后续 M14B/C 留位
  - `result.rs`：`VersionedResult<T>` 结构体；承担版本绑定 (`new` / `version` / `value` / `into_value` / `into_parts`)、payload 变换 (`map`)、过期判断 (`is_stale`)、过期丢弃 helper (`discard_if_stale`)、通过 `DeltaEvent` 的 remap (`try_remap`，校验 `event.old_version`) 与显式 `PositionMap` + 新版本的低层 remap (`try_remap_with`)
- `src/errors.rs`：新增 `VersionedResultError`（`VersionMismatch` / `RemapFailed { reason }`）并接入 `EngineError::Versioned`
- `src/lib.rs`：M14A public API 导出（`VersionedResult` / `VersionedResultError`）
- `tests/m14_versioned_result.rs`：12 个机器契约测试，覆盖版本绑定、`is_stale` 边界、过期丢弃 helper、`map` 不动版本、`try_remap` 在 `event.old_version` 不匹配时原子拒绝且不调用闭包、成功路径推进到 `event.new_version`、`RemapFailed` 透传、`CharOffset` payload 通过 `PositionMap::map_old_position` 推进、`TextRange` payload 通过 `map_old_range_with_stickiness` 推进、`try_remap_with` 跳过版本核对的成功 / 失败两条路径

## M13 GPUI testbed（可选）

- `examples/gpui_m13_testbed.rs`：聚焦 M13 fold/projection 公共 API 的最小体感台。
  - 不继承 M11/M12 全套体感（搜索 / 替换 / 多光标 / 组合输入 / Undo/Redo / 保存边界等请使用对应阶段 testbed）；
  - 体感能力：方向键移动 + Shift 扩展选区、Home/End、Enter / Backspace / Delete / 普通输入、Cmd-F 折叠当前行选区、Cmd-T 在光标处切换 fold（命中已有 fold 即展开，否则单行折叠当前行）、Cmd-U 全部展开、Cmd-R 重置、Cmd-Q 退出；
  - 视图：左侧 ProjectedViewport 切片（按 placeholder 形态展示折叠后视口，含逻辑行号 + 截断标记），右侧调试面板（FoldSet / HiddenRange / Projection 概览 / 可见与隐藏逻辑行摘要）；
  - 状态栏：char offset、Buffer 长度、逻辑 (line, col)、对应投影点（可见 proj 行 / 隐藏 anchor 回溯）、Buffer version、逻辑/投影行数、FoldSet 长度、selection 起止 char offset；
  - 编辑后 FoldSet 通过 `update_through_delta_event` 跟随 DeltaEvent 平移；FoldSet 错误 / Projection 构建错误 / 越界等均落到状态栏。

## 建议验证命令

```bash
cargo fmt
cargo test --test m11_viewport_slicing
cargo test --test m12_search
cargo test --test m12_replace
cargo test --test m12_regex
cargo test --test m13_fold_set
cargo test --test m13_projection_line_map
cargo test --test m13_projection_range_map
cargo test --test m13_projected_viewport
cargo test --test m14_versioned_result
cargo test --test m10_metadata_layer
cargo test --test m9_anchor
cargo check --example gpui_m10_testbed
cargo check --example gpui_m11_testbed
cargo check --example gpui_m12_testbed
cargo check --example gpui_m9_testbed
cargo run --example gpui_m10_testbed
cargo run --example gpui_m11_testbed
cargo run --example gpui_m12_testbed
cargo test
```
