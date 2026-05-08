# zom-engine 当前阶段快照

## 当前阶段

- 当前推进：M12 当前 Buffer 内搜索与替换
- 已完成：M0-M8 机器契约基线；M9A Anchor / Mark；M9B TrackedRange；M9C Selection 与外部 range 映射；M9 GPUI testbed；M10A MetadataRange / MetadataLayer；M10B Metadata 查询；M10 GPUI testbed；M11A LineRange 与文本切片；M11B Viewport 读取；M11 GPUI testbed；M12A 普通搜索；M12B 替换；M12C 正则搜索 / 替换；M12 GPUI testbed
- 未完成：M15 搜索任务取消 / 异步调度
- 结构调整：`src/types/`、`src/config/`、`src/text_loading/`、`src/storage/`、`src/coordinates/`、`src/selection/`、`src/tracking/`、`src/transaction/`、`src/metadata/` 已按稳定能力域目录化拆分。对外 public API 收敛到 crate root re-export，目录模块作为实现分层，不承诺外部稳定 import path。

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

## 建议验证命令

```bash
cargo fmt
cargo test --test m11_viewport_slicing
cargo test --test m12_search
cargo test --test m12_replace
cargo test --test m12_regex
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
