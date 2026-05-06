# zom-engine

`zom-engine` 是一个独立的 Rust 文本编辑引擎 crate，面向 IDE/编辑器底层能力。

## 项目边界

负责：

- 文本存储（RopeyStorage）
- 文本编辑（CharOffset/TextRange）
- 事务与变更映射
- Undo/Redo 历史
- 坐标系统（byte/char/utf16/grapheme/display）
- Snapshot 与版本过期判断

不负责：

- UI 渲染与窗口系统
- LSP 协议与语法树实现
- 项目级索引/文件树/插件系统
- 实时多人协作

## 当前代码分层

- `src/lib.rs`：对外稳定门面导出
- `src/buffer/mod.rs`：Buffer 状态聚合与 public 入口
- `src/buffer/transaction_pipeline/`：事务准备/提交/映射/历史收尾
- `src/buffer/edit_ops/`：文本变异与多选区编辑入口
- `src/buffer/history/`：Undo/Redo 历史状态与条目
- `src/buffer/events.rs`：DeltaEvent 队列与最近事件快照
- `src/buffer/composition/`：IME composition 状态/校验/流程
- `src/buffer/mod.rs`：BufferId / BufferKind / BufferState 与保存点状态
- `src/position_map.rs`：PositionMap 强类型、前后版本坐标映射结果与吸附策略
- `src/coordinates_core.rs`：Buffer/Snapshot 共享坐标数学
- `src/storage/ropey_storage.rs`：生产文本存储实现

## 测试与验证目录职责

- `tests/`：机器契约测试（CI 主体）
- `examples/`：GPUI 交互式 testbed（人类体感验证）
- `benches/`：性能基准测试
- `src/tests/`：仅在 public API 无法覆盖关键内部不变量时使用

## 常用命令

```bash
cargo fmt
cargo test
cargo test --test m0_domain_model
cargo test --test m1_buffer
cargo test --test m2_transaction
cargo test --test m3_history
cargo test --test m4_storage
cargo test --test m5_coordinates
cargo test --test m6_selection
cargo test --test m7_buffer_lifecycle
cargo test --test m8_position_map
cargo test --lib storage_consistency
cargo run --example gpui_m5_testbed
cargo run --example gpui_m6_testbed
cargo run --example gpui_m7_testbed
```

## 相关文档

- `AGENTS.md`：协作规则与阶段边界
- `编辑引擎能力.md`：能力清单与里程碑
- `编辑引擎测试策略.md`：测试哲学与测试放置策略
- `docs/STATUS.md`：当前阶段快照（易变信息：文件清单、进度、命令）

## 文档维护约定

- 主文档（`AGENTS.md`、`编辑引擎能力.md`、`编辑引擎测试策略.md`）只维护稳定规则与边界。
- 易变信息统一维护在 `docs/STATUS.md`。
