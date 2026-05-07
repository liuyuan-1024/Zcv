# zom-engine 当前阶段快照

## 当前阶段

- 当前推进：M9C Selection 与外部 range 映射
- 已完成：M0-M8 机器契约基线；M9A Anchor / Mark；M9B TrackedRange；M9C Selection 与外部 range 映射 public API 与机器契约测试
- 未完成：M9 GPUI testbed

## M9 文件

- `src/anchor.rs`：Anchor / Mark、删除策略、批量版本推进
- `src/tracked_range.rs`：TrackedRange、Range stickiness、塌缩 / 失效策略、批量版本推进
- `tests/m9_anchor.rs`：M9A-M9C 机器契约测试，按子模块聚合
- `src/lib.rs`：M9 public API 导出
- `src/errors.rs`：AnchorError 与 EngineError 接入
- `src/position_map.rs`：Selection / SelectionSet / TrackedRange 映射门面

## 建议验证命令

```bash
cargo fmt
cargo test --test m9_anchor
cargo test
```
