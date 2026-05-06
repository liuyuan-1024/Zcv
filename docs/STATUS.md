# zom-engine 当前阶段快照

## 当前阶段

- 当前推进：M9B TrackedRange
- 已完成：M0-M8 机器契约基线；M9A Anchor / Mark；M9B TrackedRange public API 与机器契约测试
- 未完成：M9C Selection 与外部 range 映射；M9 GPUI testbed

## M9 文件

- `src/anchor.rs`：Anchor / Mark、删除策略、批量版本推进
- `tests/m9_anchor.rs`：M9A 机器契约测试
- `src/tracked_range.rs`：TrackedRange、Range stickiness、塌缩 / 失效策略、批量版本推进
- `tests/m9_tracked_range.rs`：M9B 机器契约测试
- `src/lib.rs`：M9 public API 导出
- `src/errors.rs`：AnchorError 与 EngineError 接入

## 建议验证命令

```bash
cargo fmt
cargo test --test m9_anchor
cargo test --test m9_tracked_range
cargo test
```
