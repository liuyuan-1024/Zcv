# zom-engine 当前阶段快照

## 当前阶段

- 当前推进：M10 MetadataLayer 与外部区间承载
- 已完成：M0-M8 机器契约基线；M9A Anchor / Mark；M9B TrackedRange；M9C Selection 与外部 range 映射；M9 GPUI testbed；M10A MetadataRange / MetadataLayer；M10B Metadata 查询
- 未完成：M10 GPUI testbed

## M9 文件

- `src/anchor.rs`：Anchor / Mark、删除策略、批量版本推进
- `src/tracked_range.rs`：TrackedRange、Range stickiness、塌缩 / 失效策略、批量版本推进
- `tests/m9_anchor.rs`：M9A-M9C 机器契约测试，按子模块聚合
- `examples/gpui_m9_testbed.rs`：继承 M8 体感，并叠加 tracked range 创建、清除、移动 / 收缩 / 失效观察
- `src/lib.rs`：M9 public API 导出
- `src/errors.rs`：AnchorError 与 EngineError 接入
- `src/position_map.rs`：Selection / SelectionSet / TrackedRange 映射门面

## M10 文件

- `src/metadata.rs`：MetadataRange / MetadataLayer / MetadataLayers、LayerKind、range id、版本绑定、范围追踪、失效移除、LineRange / viewport 查询、按 layer 查询、批量替换与过期丢弃
- `tests/m10_metadata_layer.rs`：M10A-M10B 机器契约测试，覆盖泛型 payload、多 layer、DeltaEvent 跟随、失效策略、基础查询、LineRange / viewport 查询、按 layer 查询、批量替换与过期丢弃
- `src/lib.rs`：M10 public API 导出
- `src/errors.rs`：MetadataError 与 EngineError 接入
- `src/types.rs`：LineRange 强类型

## 建议验证命令

```bash
cargo fmt
cargo test --test m10_metadata_layer
cargo test --test m9_anchor
cargo check --example gpui_m9_testbed
cargo run --example gpui_m9_testbed
cargo test
```
