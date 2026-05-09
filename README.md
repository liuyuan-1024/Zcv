# zom-engine

`zom-engine` 是一个独立的 Rust 文本编辑引擎 crate，为编辑器宿主提供底层文本编辑能力。

## 底线规范

`zom-engine` 只做纯文本编辑引擎能力。后续阶段规划、实现和测试都必须先判断能力是否属于 engine core；不属于文本存储、编辑、坐标、事务、历史、快照、区间追踪、投影映射、读取切片、文件文本边界、错误防御或性能验证的内容，不进入本 crate 的 milestone。

## 项目边界

负责：

- 文本存储（RopeyStorage）
- 文本编辑（CharOffset/TextRange）
- 事务与变更映射
- Undo/Redo 历史
- 坐标系统（byte/char/utf16/grapheme/display）
- Snapshot 与版本绑定

不负责：

- UI 渲染与窗口系统
- LSP 协议与语法树实现
- diagnostics / semantic tokens / inlay hints / code lens 等业务结果生成
- 项目级索引/文件树/插件系统
- 快捷键、菜单、命令面板、Command 语义层
- 宏录制、用户操作回放
- 后台任务调度器、取消令牌、线程池
- 实时多人协作

## 当前代码分层

- `src/lib.rs`：对外稳定门面导出；外部使用者优先从 crate root 导入 public API
- `src/buffer/mod.rs`：Buffer 状态聚合与 public 入口
- `src/buffer/transaction_pipeline/`：事务准备/提交/映射/历史收尾
- `src/buffer/edit_ops/`：文本变异与多选区编辑入口
- `src/buffer/history/`：Undo/Redo 历史状态与条目
- `src/buffer/lifecycle.rs`：Buffer 身份、只读状态、保存点与 dirty 判断
- `src/buffer/events.rs`：DeltaEvent 队列与最近事件快照
- `src/buffer/composition/`：IME composition 状态/校验/流程
- `src/types/`：offset、position、range、version、Buffer identity 与换行风格强类型
- `src/config/`：Buffer、encoding、display、word、line ending 与大文件策略
- `src/text_loading/`：外部 bytes 进入 Buffer 时的编码策略与加载元信息
- `src/transaction/`：Edit、EditList、Transaction record、metadata、Delta 与 ChangeSet
- `src/position_map.rs`：PositionMap 强类型、前后版本坐标映射结果与吸附策略
- `src/tracking/`：Anchor / Mark、TrackedRange、跟随策略与更新结果
- `src/metadata/`：MetadataRange / MetadataLayer 外部区间承载与版本推进
- `src/coordinates/`：Buffer/Snapshot 共享坐标数学
- `src/selection/`：Cursor、Selection、SelectionSet、movement 与 composition selection 类型
- `src/storage/`：TextStorage trait、文本指纹与 RopeyStorage 生产实现

目录模块是实现分层，不作为长期 public import path 承诺；稳定外部 API 由 `src/lib.rs` 统一 re-export。

## 测试与验证目录职责

- `tests/`：机器契约测试（CI 主体）
- `examples/`：可选 GPUI 交互式 testbed（人类体感验证，不作为 M13 之后阶段验收底线）
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
cargo test --test m9_anchor
cargo test --test m10_metadata_layer
cargo test --test m11_viewport_slicing
cargo test --test m12_search
cargo test --test m12_replace
cargo test --test m12_regex
cargo run --example gpui_m5_testbed
cargo run --example gpui_m6_testbed
cargo run --example gpui_m7_testbed
cargo run --example gpui_m8_testbed
cargo run --example gpui_m9_testbed
cargo run --example gpui_m10_testbed
cargo run --example gpui_m11_testbed
cargo run --example gpui_m12_testbed
```

## 相关文档

- `AGENTS.md`：协作规则与阶段边界
- `docs/编辑引擎能力.md`：能力清单与里程碑
- `docs/编辑引擎测试策略.md`：测试哲学与测试放置策略
- `docs/STATUS.md`：当前阶段快照（易变信息：文件清单、进度、命令）

## 文档维护约定

- 主文档（`AGENTS.md`、`docs/编辑引擎能力.md`、`docs/编辑引擎测试策略.md`）只维护稳定规则与边界。
- 易变信息统一维护在 `docs/STATUS.md`。
- 全部阶段以 engine-only 为底线；非编辑引擎核心能力直接从 milestone 中移除，而不是标成”后续阶段”。
