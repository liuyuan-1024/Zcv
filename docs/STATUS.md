# zom-engine 当前阶段快照

## 当前阶段

- 当前推进：M11 Viewport Slicing 与读取接口
- 已完成：M0-M8 机器契约基线；M9A Anchor / Mark；M9B TrackedRange；M9C Selection 与外部 range 映射；M9 GPUI testbed；M10A MetadataRange / MetadataLayer；M10B Metadata 查询；M10 GPUI testbed；M11A LineRange 与文本切片；M11B Viewport 读取
- 未完成：M11 GPUI testbed
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
- `src/lib.rs`：M11 public API 导出
- `src/errors.rs`：InvalidByteRange 接入 CoordinateError

## 建议验证命令

```bash
cargo fmt
cargo test --test m11_viewport_slicing
cargo test --test m10_metadata_layer
cargo test --test m9_anchor
cargo check --example gpui_m10_testbed
cargo check --example gpui_m9_testbed
cargo run --example gpui_m10_testbed
cargo test
```
