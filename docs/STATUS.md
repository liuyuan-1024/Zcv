# zom-engine 当前阶段快照

## 当前阶段

- 当前推进：M9A Anchor / Mark
- 已完成：M0-M8 机器契约基线；M9A Anchor / Mark public API 与机器契约测试
- 未完成：M9B TrackedRange；M9C Selection 与外部 range 映射；M9 GPUI testbed

## M9A 文件

- `src/anchor.rs`：Anchor / Mark、删除策略、批量版本推进
- `tests/m9_anchor.rs`：M9A 机器契约测试
- `src/lib.rs`：M9A public API 导出
- `src/errors.rs`：AnchorError 与 EngineError 接入

## 建议验证命令

```bash
cargo fmt
cargo test --test m9_anchor
cargo test
```
