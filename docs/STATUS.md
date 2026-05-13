# zom-engine 当前阶段快照

## 当前阶段

- 当前推进：M19 Verification / Benchmark / Observability 完成（proptest 6 项 property 回归 + 三个 criterion 基准文件 + `Buffer::approximate_memory_bytes` 观测入口）；引擎 milestone 主线 M0–M19 收口
- 已完成：M0–M12 机器契约基线；M13 折叠 + 投影；M14 Versioned Result / Range Set / 外部 UTF-16 边界；M15 Local Read / Write Boundary；M16 Transaction Record 与 Replay；M17A `HistoryNodeId` 单调身份 + `HistoryNode` parent/children 树 + `HistoryNodeView` 只读视图 + `current_history_node` / `parent_history_node` / `redo_branches` / `redo_to_branch` / `EngineError::InvalidHistoryBranch` 公共 API；线性 `undo` / `redo` 兼容；GPUI testbed 覆盖至 M12；M17B `LargeFilePolicy` 字节预算 + `LargeTransactionPolicy` + `Buffer::set_large_file_policy` + `HistoryStatus.node_count` / `memory_bytes`；M18 `LargeFilePolicy` 大文件 / 超长行阈值 + `auto_read_only_on_large_file` + `LoadedTextInfo` 字节 / 最长行 / is_large / has_long_line + `Buffer::is_large_file` / `has_long_line` / `longest_line_chars`；M19 `proptest` property 测试 + `criterion` 基准（核心编辑 / viewport projection / search replace）+ `Buffer::approximate_memory_bytes`
- 未完成：引擎主线 milestone 已全部收口；后续工作集中在 fuzz/observability/性能调优等增量任务，按需启动
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
- `src/buffer/slicing.rs`：Buffer 上的 byte range、char range 派生视图、单行、LineRange 与 Viewport 读取入口
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
  - `mod.rs`：M14 versioned 模块入口；当前导出 `VersionedResult` / `VersionedRangeSet` / `VersionedRangeEntry` / `VersionedRangeSpec`，给后续 M14C 留位
  - `result.rs`：`VersionedResult<T>` 结构体；承担版本绑定 (`new` / `version` / `value` / `into_value` / `into_parts`)、payload 变换 (`map`)、过期判断 (`is_stale`)、过期丢弃 helper (`discard_if_stale`)、通过 `DeltaEvent` 的 remap (`try_remap`，校验 `event.old_version`) 与显式 `PositionMap` + 新版本的低层 remap (`try_remap_with`)
- `src/errors.rs`：新增 `VersionedResultError`（`VersionMismatch` / `RemapFailed { reason }`）并接入 `EngineError::Versioned`
- `src/lib.rs`：M14A public API 导出（`VersionedResult` / `VersionedResultError`）
- `tests/m14_versioned_result.rs`：12 个机器契约测试，覆盖版本绑定、`is_stale` 边界、过期丢弃 helper、`map` 不动版本、`try_remap` 在 `event.old_version` 不匹配时原子拒绝且不调用闭包、成功路径推进到 `event.new_version`、`RemapFailed` 透传、`CharOffset` payload 通过 `PositionMap::map_old_position` 推进、`TextRange` payload 通过 `map_old_range_with_stickiness` 推进、`try_remap_with` 跳过版本核对的成功 / 失败两条路径

## M14B 文件

- `src/versioned/range_set.rs`：`VersionedRangeSet<T>` / `VersionedRangeEntry<T>` / `VersionedRangeSpec<T>`；不携带 `MetadataLayerKind` 与稳定 ID 的轻量泛型 (TrackedRange, payload) 集合
  - 集合：`new(version)` / `with_default_stickiness` / `with_default_update_policy` / `version` / `is_stale` / `len` / `is_empty` / `default_stickiness` / `default_update_policy` / `as_slice` / `iter` / `entry` / `entry_mut`
  - 写入：`insert` / `insert_with_stickiness` / `insert_with_options`（返回追加索引）/ `remove` / `clear` / `replace_all` / `replace_all_with_options`
  - 跟随：`update_through_delta_event(event) -> Result<Vec<TrackedRangeUpdate>, VersionedResultError>`，按 entry 原顺序返回更新事实，按 update policy 删除失效 entry，版本不匹配原子拒绝
  - 查询：`entries_intersecting(TextRange)` / `entries_containing(ByteOffset)` / `entries_in_line_range(buffer, LineRange)` / `entries_in_line_window(buffer, MetadataLineWindow)`
  - 互转：`From<MetadataLayer<T>> for VersionedRangeSet<T>`（丢弃 kind 与 id，保留 version / 默认策略 / 每 entry 的 tracked_range 与 update_policy）+ `into_metadata_layer(kind: MetadataLayerKind) -> MetadataLayer<T>`（沿用 version / 默认策略 / 每 entry 的 stickiness 与 update_policy，重新分配 `MetadataRangeId::INITIAL+0..`）
- `src/metadata/range.rs`：新增 `MetadataRange::into_parts(self) -> (MetadataRangeId, TrackedRange, TrackedRangeUpdatePolicy, T)`，供互转消费
- `src/metadata/layer.rs`：新增 `MetadataLayer::into_parts(self) -> (kind, version, default_stickiness, default_update_policy, ranges)`，供互转消费
- `src/metadata/mod.rs`：将 `query` 提升为 `pub(crate)`，让 `versioned` 复用 `ranges_intersect` / `range_contains_offset` / `text_range_for_line_range` 边界数学
- `src/lib.rs`：M14B public API 导出（`VersionedRangeSet` / `VersionedRangeEntry` / `VersionedRangeSpec`）
- `tests/m14_versioned_range_set.rs`：15 个机器契约测试，覆盖空 set 与版本绑定、insert / insert_with_stickiness / insert_with_options 默认与每条策略、remove / clear / replace_all / replace_all_with_options 推进 version、`update_through_delta_event` 普通跟随、删除策略下 Invalidated entry 自动出栈、版本不匹配原子拒绝、`entries_intersecting` / `entries_containing` / `entries_in_line_range` / `entries_in_line_window` 查询与越界错误、`MetadataLayer<T>` -> `VersionedRangeSet<T>` 丢 kind / id 保 payload 与策略、`VersionedRangeSet<T>` -> `MetadataLayer<T>` 重新分配 ID 并保留 kind / 默认策略、Layer↔Set 往返后 entry 仍可在 DeltaEvent 上正确跟随

## M14C 文件

- `src/versioned/result.rs`：`VersionedResult::try_map_at_snapshot(snapshot, f)` —— `snapshot.version()` 必须等于结果版本，否则 `EngineError::Versioned(VersionMismatch)` 原子拒绝；闭包接收 `(payload, &Snapshot)` 并返回 `EngineResult<U>`，常用于 `Position` ↔ UTF-16 / `CharOffset` ↔ UTF-16 / changed ranges → UTF-16 边界等转换。
- `src/versioned/range_set.rs`：
  - `try_map_payloads_at_snapshot(snapshot, f)` —— per-entry payload 转换，闭包签名 `(payload, TextRange, &Snapshot) -> EngineResult<U>`；保留每条 entry 的 tracked range / stickiness / update policy；版本不匹配原子拒绝。
  - `try_export_entries_to_utf16(&self, snapshot)` —— 按 `as_slice()` 顺序导出 `(Utf16Position, Utf16Position, &T)`；版本不匹配 / 边界越界透传 `EngineError`。
  - `try_insert_utf16_range` / `try_insert_utf16_range_with_options` —— 用 UTF-16 行列边界追加单条 entry；版本不匹配 / 反向区间 / 越界透传 `EngineError`，失败时不改动 set。
  - `VersionedRangeSpec::try_from_utf16(snapshot, start, end, payload)` —— 配合 `replace_all_with_options` 批量从 UTF-16 边界导入。
- `src/transaction/delta.rs`：`DeltaEvent::changed_ranges_result()` —— 把 `ChangeSet::changed_ranges()` 包成 `VersionedResult<Vec<TextRange>>` 并绑定 `new_version`，便于宿主复用 M14A / M14C 链路（`try_map_at_snapshot` 等）做 UTF-16 export 或推进版本时直接 `is_stale` 判断。
- `tests/m14_versioned_result.rs`：新增 5 个测试覆盖 `try_map_at_snapshot` 成功 / 版本不匹配 / 闭包错误透传，`changed_ranges_result` 绑定 `new_version` 与到 UTF-16 export 的端到端链路（合计 17 个测试）。
- `tests/m14_versioned_range_set.rs`：新增 9 个测试覆盖 `try_map_payloads_at_snapshot` 成功 / 版本不匹配，`try_export_entries_to_utf16` LSP 友好行列输出 / 版本不匹配，`try_insert_utf16_range` 与 `try_insert_utf16_range_with_options` 默认 / 自定义策略 / 版本不匹配且不改动 set，`VersionedRangeSpec::try_from_utf16` 批量替换与反向区间拒绝（合计 24 个测试）。

## M15 文件

- `src/buffer/events.rs`：新增 `Buffer::pending_delta_events() -> &[DeltaEvent]` —— 不消费地查看 pending 队列，配合既有 `pending_delta_event_count` / `take_pending_events` / `last_delta_event` 让本地订阅者按版本链检测漏读
- `tests/m15_local_read_write_boundary.rs`：14 个机器契约测试，按 M15A / M15B / M15C 三个子模块聚合
  - `m15a_single_writer`：写入入口必须 `&mut`（编译期）；成功提交推进版本并入 DeltaEvent；版本不匹配原子拒绝且不留 DeltaEvent / 不动文本；连续提交形成连续版本链；EditList 重叠在 `Transaction::from_edits` 阶段就拒绝，不影响 Buffer
  - `m15b_snapshot_reader`：编译期断言 `Snapshot: Send + Sync`；旧 snapshot 在后续提交后仍只读且文本/版本不变；snapshot 跨 `std::thread::spawn` 移动后做 `text()` / `search` 查询；snapshot 派生的搜索结果绑定 `snapshot.version()`，宿主可用 `is_stale(buffer.version())` 判断
  - `m15c_delta_consumer`：`last_delta_event` 跟随最近提交；`pending_delta_events()` peek 不消费；`take_pending_events()` 按提交顺序排空且后续提交继续累积；订阅者基于 `last_seen` + `event.old_version` 检测漏读；延迟订阅者用 snapshot 保留旧版本事实，配合 DeltaEvent 拼接 old/new 版本

## M16 文件

- `src/transaction/transaction_record.rs`：`TransactionRecord` 值类型——录制一次成功事务后所有可回放事实
  - 字段：`transaction_id` / `old_version` / `new_version` / `edits` / `inverse_edits` / `before_selection` / `after_selection` / `metadata`
  - 派生：`records_history()`（来自 metadata）、`is_merge_boundary()`（`metadata.merge_policy() != MergeWithPrevious`）
  - 重建：`to_transaction()` 重建可提交的 `Transaction`（使用 `old_version` 作为 base_version、显式装载 before / after selection 与 metadata）
- `src/transaction/mod.rs`：导出 `TransactionRecord`
- `src/buffer/transaction_pipeline/apply.rs`：把原 `apply_transaction` 的实现下沉到私有 `apply_transaction_inner` 中，`apply_transaction` / `apply_transaction_recorded` 共用同一管线、不引入二级路径
  - `apply_transaction_recorded(tx) -> EngineResult<TransactionRecord>`：成功提交并返回完整事实快照
  - `replay_transaction_record(record) -> EngineResult<TransactionRecord>`：版本守卫（`record.old_version == self.version` 否则 `TransactionError::VersionMismatch` 原子拒绝），通过 `record.to_transaction()` 走标准 `apply_transaction_recorded`，不绕过任何边界校验
- `src/lib.rs`：导出 `TransactionRecord`
- `tests/m16_transaction_record.rs`：9 个机器契约测试，覆盖版本 / forward & inverse edits / before & after selection（含显式 after 与 PositionMap 默认平移）/ metadata 透传与 merge boundary 派生 / `record_history=false` 不入历史 / `transaction_id` 与 `last_delta_event` 对齐 / `to_transaction()` 完整重建 / record 是值快照不跟随 Buffer 推进 / 版本不匹配不产生 record
- `tests/m16_transaction_replay.rs`：6 个机器契约测试，覆盖跨 Buffer 回放后状态等价、回放生成等价 DeltaEvent（除独立递增的 transaction_id）、版本不匹配原子拒绝且不动 Buffer / 不入事件队列、回放在更短 buffer 上触发 `EditError::RangeOutOfBounds`（不绕过边界校验）、独立 buffer 重放达到原 apply 完终态、回放后的事务进入历史栈支持后续 undo

## M19 文件

- `Cargo.toml`：新增 dev-dependency `proptest = "1.5.0"`；注册 `[[bench]] m19_core_editing` / `m19_viewport_projection` / `m19_search_replace`
- `src/buffer/lifecycle.rs`：新增 `Buffer::approximate_memory_bytes()` 内存粗估入口（文本字节 + 历史 `memory_bytes` + selection / pending DeltaEvent 队列固定大小估算；不承诺等同 RSS / `ropey` 内部节点精确字节）
- `tests/m19_property_regressions.rs`：6 个 proptest 测试，覆盖：随机编辑序列与 String 参考模型差分一致、`undo_roundtrip_restores_initial_text`、`undo_then_redo_returns_to_final_state`、`SelectionSet::new` 排序与不重叠不变量、多光标 caret insert 长度可预测、Snapshot 在后续编辑下不可变；每个 property 默认 64 cases，shrinking 自动收敛最小复现
- `benches/m19_core_editing.rs`：7 组 criterion 基准，覆盖单次插入（多行 / 超长单行）/ 删除 / 替换 / 64 个编辑批量事务 / Undo+Redo 循环 / 多光标 50 caret 插入 / 坐标转换（line_start / position_to_char）/ 50k 行 snapshot clone
- `benches/m19_viewport_projection.rs`：6 组 criterion 基准，覆盖 50k 行 viewport slicing、超长行 viewport 截断、20k 行 projection 构建（无 fold / 200 fold）、50 个 logical→projected point 查询、含 fold 的 projected viewport 切片
- `benches/m19_search_replace.rs`：6 组 criterion 基准，覆盖 literal 搜索（默认 / case-insensitive / whole-word）、regex 搜索（带 capture）/ replace_all literal+regex、100w chars 长行 literal 搜索

## M18 文件

- `src/config/large_file.rs`：`LargeFilePolicy` 增 `large_file_threshold_bytes` / `long_line_threshold_chars` / `auto_read_only_on_large_file` 三字段（默认 5 MiB / 10000 chars / false）+ `is_large_byte_size(byte_size)` / `is_long_line(chars)` helper（阈值=0 视为不限）
- `src/text_loading/loaded_text.rs`：`LoadedTextInfo` 增 `loaded_byte_size` / `is_large` / `longest_line_chars` / `has_long_line` 加载快照字段
- `src/buffer/loading.rs`：`from_loaded_text` 在解码后计算 `loaded_byte_size` + `longest_line_chars_in(text)`，并按 policy 填充 `is_large` / `has_long_line`；`longest_line_chars_in` 作为 `pub(crate)` helper 复用
- `src/buffer/lifecycle.rs`：新增 `Buffer::is_large_file()` / `has_long_line()` / `longest_line_chars()` 公共查询；新增 `apply_large_file_auto_read_only` 模块内 helper 在 `from_kind_text` 末尾调用，根据 policy + 当前 storage 长度决定是否切只读
- `src/buffer/reload.rs`：`reload_from_text` 末尾调用 `apply_large_file_auto_read_only`，让 reload 大文本同样获得自动只读语义；既有只读状态不会因 reload 小文本被取消（引擎只单向加固）
- `tests/m0_domain_model.rs`：扩展默认值断言覆盖三个新字段
- `tests/m17_history_budget.rs`：`LargeFilePolicy` 显式构造改用 `..LargeFilePolicy::default()` 兼容新字段
- `tests/m18_large_file_policy.rs`：17 个机器契约测试，覆盖默认阈值 / `is_large_byte_size` / `is_long_line` helper、`Buffer::is_large_file` / `has_long_line` / `longest_line_chars`（含 CRLF / Unicode / 空文本 / 纯换行）、`from_loaded_text` 填充 byte_size / longest_line_chars / is_large / has_long_line、`auto_read_only_on_large_file` 在 `from_loaded_text` / `from_kind_text` / reload 路径上的触发与不触发、reload 小文本不取消既有只读、阈值=0 关闭事实与 auto-read-only
- `tests/m18_defensive_runtime.rs`：10 个机器契约测试，覆盖 auto-read-only Buffer 写入返回 `StorageError::ReadOnly`、宿主可 `set_read_only(false)` 解除、`LargeTransactionPolicy::Reject` / `SkipHistory` 在大粘贴上的行为差异、版本不匹配原子拒绝、大文件越界编辑返回 `EditError::RangeOutOfBounds`、大文件 snapshot 上的 search 稳定性、极小预算 + SkipHistory + 重新截断不 panic、阈值=0 时 auto=true 不触发、`EngineError::VersionOverflow` 可诊断

## M17B 文件

- `src/config/large_file.rs`：`LargeFilePolicy` 增 `max_undo_history_bytes` / `large_transaction_threshold_bytes` / `large_transaction_policy` 字段；新增 `LargeTransactionPolicy { SkipHistory, Reject }`；默认值 `max_undo_history=1000` / `max_undo_history_bytes=64 MiB` / `large_transaction_threshold_bytes=16 MiB` / `large_transaction_policy=SkipHistory`
- `src/config/mod.rs` / `src/lib.rs`：导出 `LargeTransactionPolicy`
- `src/buffer/history/entry.rs`：新增 `HistoryEntry::byte_size()`，按 `undo_batches` + `redo_batches` 中所有 `Edit::replacement` 的 UTF-8 字节和度量；selection / description / TextRange 容器不计入
- `src/buffer/history/node.rs`：`HistoryNode` 新增 `entry_bytes` 缓存字段 + `replace_entry` helper；`MergeWithPrevious` 后通过 `replace_entry` 重新计算 byte_size
- `src/buffer/history/state.rs`：`truncate_to_max_nodes` 替换为 `truncate_to_budget(max_nodes, max_bytes)`；新增 `node_count` / `total_bytes` / `find_oldest_disposable` + `splice_out_and_remove` + 模块级 `splice_children` helper（按 sequence_number 丢弃最老的非 current 节点，子节点 splice 到原父位置保持兄弟顺序，current 永不丢弃）；`HistoryStatus` 增 `node_count` / `memory_bytes` 字段
- `src/buffer/history/api.rs`：新增 `Buffer::set_large_file_policy(policy)` public API（替换 policy 后立即按新预算截断）；`truncate_undo_history_to_budget` 改用双预算调用
- `src/buffer/transaction_pipeline/apply.rs`：在 `prepare_transaction` 之后、`commit_prepared_transaction` 之前插入 `apply_large_transaction_policy`；`Reject` 策略原子拒绝并返回 `EditError::PayloadTooLarge { size, limit }`，`SkipHistory` 把 metadata 的 `record_history` 切到 false 复用既有路径；新增模块级 `edit_list_replacement_bytes` helper 与 `HistoryEntry::byte_size` 同口径
- `tests/m0_domain_model.rs`：扩展默认值断言覆盖新字段
- `tests/m17_history_budget.rs`：15 个机器契约测试，覆盖默认预算、字节预算驱动的最老节点丢弃、current 仅存时不被丢弃、节点数 + 字节双预算、`LargeTransactionPolicy::Reject` 原子拒绝、`SkipHistory` 提交文本但不入历史、`set_large_file_policy` 即时截断、`max_undo_history=0` 清空、`MergeWithPrevious` 字节累加、`HistoryStatus` 字段同步、阈值 0 关闭超大事务策略、SkipHistory 路径作废 redo 分支、deletion 通过 inverse_edits.replacement 占用字节

## M17A 文件

- `src/buffer/history/node.rs`：`HistoryNodeId`（u64 包装的稳定身份，跨 Buffer 寿命单调递增）+ `HistoryNode { id, sequence_number, parent, children, entry: HistoryEntry }`
- `src/buffer/history/state.rs`：原线性 `undo_stack` / `redo_stack` 重构为 `HistoryState { nodes, roots, current, next_id, next_sequence }` 历史图；新方法 `current` / `node(id)` / `parent_of_current` / `children_of_current` / `push_child` / `merge_into_current` / `step_undo` / `step_redo_into` / `default_redo_target` / `drop_children_of_current` / `truncate_to_max_nodes`；`HistoryStatus` 新增 `current_node` 字段并保留 `undo_depth` / `redo_depth` 与线性历史等价的语义
- `src/buffer/history/api.rs`：`undo` 推 cursor 到父节点（节点不丢弃）；`redo` 沿默认分支前进；新增 `current_history_node` / `history_node(id) -> Option<HistoryNodeView>` / `parent_history_node` / `redo_branches`（最近优先排序）/ `redo_to_branch(node_id)`（非子节点返回 `EngineError::InvalidHistoryBranch`）；`push_history` 用 `merge_into_current` / `push_child` 替代旧栈操作；新增 `drop_unrecorded_redo_branches` 在 `record_history=false` 提交后删除当前节点子树
- `src/buffer/transaction_pipeline/apply.rs`：`record_history=false` 路径调用 `drop_unrecorded_redo_branches` 替代旧的 `clear_redo`
- `src/buffer/history/mod.rs` / `src/buffer/mod.rs` / `src/lib.rs`：导出 `HistoryNodeId` / `HistoryNodeView`
- `src/errors.rs`：新增 `EngineError::InvalidHistoryBranch(HistoryNodeId)`
- `tests/m17_advanced_history.rs`：12 个机器契约测试，覆盖空 Buffer 节点缺失、单调序号、`undo` 不丢节点、`undo + new commit` 产生兄弟分支、默认 redo 走最近分支、`redo_to_branch` 显式切换、非子节点拒绝、`MergeWithPrevious` 合并到当前节点不开新分支、分支按最近创建优先排序、`HistoryNodeView` 携带 selection / description、多次分支切换分支节点保留、`record_history=false` 删除当前节点子树

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
cargo test --test m14_versioned_range_set
cargo test --test m15_local_read_write_boundary
cargo test --test m16_transaction_record
cargo test --test m16_transaction_replay
cargo test --test m17_advanced_history
cargo test --test m17_history_budget
cargo test --test m18_large_file_policy
cargo test --test m18_defensive_runtime
cargo test --test m19_property_regressions
cargo bench --no-run
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
