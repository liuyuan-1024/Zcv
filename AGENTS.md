# AGENTS.md

> This file is the first document any AI coding assistant should read before helping with this repository.
>
> 本文件用于让 AI 在参与 `zom-engine` 开发前，先理解项目定位、阶段边界、测试策略和代码规范，避免每次重新介绍背景。

---

## 0. 必读规则

在开始任何开发、重构、测试生成或代码评审前，AI 必须先阅读：

```text
AGENTS.md
README.md
编辑引擎能力.md
编辑引擎测试策略.md
docs/STATUS.md（如果存在）
```

如果当前环境不能读取文件，AI 应该先提醒用户提供相关文件内容，而不是凭空假设项目规范。

---

## 1. 项目定位

`zom-engine` 是一个独立的 Rust 编辑引擎 crate。

它的核心职责是：

```text
文本存储
文本编辑
坐标模型
事务系统
变更映射
历史系统
快照
底层能力接口
```

它不负责：

```text
UI 渲染
GPUI 组件本身
LSP
语法树
项目索引
文件树
插件系统
完整 IDE 产品形态
```

后续 `zom ide` 可以依赖 `zom-engine`，但 `zom-engine` 本身必须保持为独立、可测试、低耦合的编辑引擎底座。

---

## 2. 当前目录语义

本项目文档采用“稳定层 + 易变层”：

```text
稳定层（本文件维护）：
只定义目录职责和依赖边界，不维护逐文件清单。

易变层（docs/STATUS.md 维护）：
当前阶段文件列表、具体测试清单、阶段进展快照。
```

稳定职责边界：

```text
src/        编辑引擎实现（按能力域拆模块）
tests/      机器契约测试（CI 主体）
examples/   交互式 testbed（人类体感）
benches/    性能基准测试
src/tests/  可选内部测试区，仅测试 public API 无法覆盖的重要内部不变量
docs/       文档与状态快照（STATUS.md）
```

---

## 3. 核心测试哲学

本项目测试分两类：

```text
tests/    = 机器契约
examples/ = 人类体感
```

### 3.1 `tests/`

`tests/` 是给机器跑的自动化契约测试。

特点：

```text
cargo test 可运行
不依赖 GPUI
不依赖窗口系统
不依赖真实渲染
结果确定
适合 CI
```

它应该测试：

```text
public API 是否稳定
阶段性能力是否正确
错误输入是否返回明确错误
未来重构是否破坏已有契约
```

它不应该测试：

```text
GPUI 事件
UI 渲染
窗口行为
鼠标体感
私有实现细节
```

### 3.2 `examples/`

`examples/` 是给开发者把玩的交互式 testbed。

特点：

```text
cargo run --example xxx
服务 Dogfooding
服务肉眼排查 Bug
服务 API 接入手感验证
服务 GPUI 与 engine 桥接验证
```

它不应该重复测试编辑算法正确性。核心算法正确性应该放在 `tests/`。

---

## 4. Testbed 继承原则

这是硬规则：

```text
M(n) testbed 不能丢 M(n-1) 已经具备的手感能力，只能继承并扩展。
```

也就是说：

```text
gpui_m2_testbed.rs 不能因为切换到底层 Transaction 架构，
就丢掉 gpui_m1_testbed.rs 已经具备的输入、删除、光标移动、Home / End、Save / Reset、状态栏等体验。
```

正确演化方式：

```text
M(n) testbed = M(n-1) testbed 的完整体验 + 当前阶段新增能力的可视化 / 可交互验证
```

示例：

```text
M1 testbed = Buffer + 基础输入 + 删除 + 光标移动 + Home/End + Save/Reset + 状态栏
M2 testbed = M1 完整体验 + Transaction / Delta / ChangeSet 可视化
M3 testbed = M2 完整体验 + Undo / Redo / History 状态可视化
M4 testbed = M3 完整体验 + 高性能存储指标 / Snapshot / Storage 行为可视化
M6 testbed = M4/M5 完整体验 + Selection / Multi Cursor 可视化
```

每次修改 `examples/gpui_mN_testbed.rs` 前，必须检查：

```text
1. 上一阶段支持的输入方式是否保留？
2. 上一阶段支持的光标移动是否保留？
3. 上一阶段支持的快捷键是否保留？
4. 上一阶段状态栏信息是否保留或增强？
5. CRLF、末尾换行、UTF-8 边界等体感是否保持一致？
6. 新阶段能力是否是叠加，而不是替代？
```

---

## 5. 阶段边界

### 5.1 M0：Domain Model

M0 关注基础领域模型和错误类型。

包括：

```text
ByteOffset
CharOffset
Utf16Offset
Line
LogicalColumn
DisplayColumn
TextRange
Position
BufferVersion
TransactionId
BufferConfig
错误类型
```

M0 测试放在：

```text
tests/m0_domain_model.rs
```

M0 不应该测试：

```text
Buffer 编辑行为
LineIndex 重建
Transaction
ChangeSet
Undo / Redo
GPUI
```

重要约束：

```text
不要恢复 TextRange::new_unchecked 作为 public API。
TextRange 必须通过安全构造器维护 start <= end 不变量。
不要为了测试污染 public API。
```

---

### 5.2 M1：Buffer

M1 关注最小可用 Buffer。

包括：

```text
创建 Buffer
读取文本
insert
delete
replace
TextRange 校验
UTF-8 边界
CRLF 边界
LineIndex
ByteOffset <-> Position
BufferVersion
dirty state
```

M1 测试放在：

```text
tests/m1_buffer.rs
examples/gpui_m1_testbed.rs
```

M1 的原则：

```text
StringStorage 是语义验证后端，不是最终高性能后端。
LineIndex 可以全量重建，先保证语义正确。
M1 不做 Undo / Redo。
M1 不做 Transaction。
M1 不做 Selection / Multi Cursor。
M1 不做高性能 Rope / PieceTree。
```

M1 testbed 至少应支持：

```text
普通字符输入
Space / Tab / Enter
Backspace / Delete
Left / Right
Home / End
Cmd-S 标记 saved
Cmd-R 重置文本
状态栏显示 byte / len / line / col / lines / version / saved / dirty
CRLF 边界跳过
末尾换行光标显示
```

---

### 5.3 M2：Transaction

M2 关注事务边界。

包括：

```text
Edit
EditList
Transaction
Delta
ChangeSet
批量编辑
编辑排序
重叠检测
版本检查
事务失败原子性
old position -> new position 映射
changed_ranges
```

M2 测试放在：

```text
tests/m2_transaction.rs
examples/gpui_m2_testbed.rs
```

M2 Transaction 契约：

```text
1. EditList 负责排序和重叠检测。
2. Transaction 必须绑定 base_version。
3. Transaction 不能为空。
4. Buffer::apply_transaction 必须检查 base_version。
5. 所有 edit 在应用前必须完成边界校验。
6. 事务失败后 text / version / dirty / line_index 必须完全不变。
7. Delta 必须记录 old_version / new_version / edits。
8. ChangeSet 只能从已验证的 EditList 构造。
9. ChangeSet 不应依赖 public unchecked range 构造器。
```

M2 testbed 必须继承 M1 testbed 的全部体验，然后新增：

```text
所有写入都走 Transaction
显示 Delta old_version -> new_version
显示本次事务 edit 数量
显示 changed_ranges
可视化 ChangeSet 映射
可视化 Anchor / Marker 跟随编辑移动
批量事务操作入口
```

---

### 5.4 M3：Undo / Redo 与基础 Snapshot

M3 关注历史系统与不可变快照。

包括：

```text
Undo
Redo
历史栈
Redo stack 清理
Transaction merge
SelectionSnapshot 恢复
Snapshot
Snapshot 绑定 BufferVersion
Snapshot 跨线程读取
is_version_stale / is_snapshot_stale
```

M3 测试放在：

```text
tests/m3_history.rs
examples/gpui_m3_testbed.rs
```

M3 的原则：

```text
Undo / Redo 必须恢复文本。
Undo / Redo 必须恢复当前阶段已有的 selection snapshot。
Snapshot 是后台读取和版本过期判断的基础。
M3 先验证历史语义，不在这一阶段追求高性能存储。
```

---

### 5.5 M3.5：核心编辑坐标迁移到 CharOffset

M3.5 关注把核心编辑坐标彻底迁移为 `CharOffset`。

包括：

```text
TextRange 基于 CharOffset
SelectionSnapshot 基于 CharOffset
Buffer insert / delete / replace 使用 CharOffset / TextRange
Edit / EditList / Transaction 使用 CharOffset / TextRange
Delta / ChangeSet / changed_ranges 使用 CharOffset / TextRange
Undo / Redo inverse edits 使用 CharOffset / TextRange
LineIndex 记录 char line starts
删除旧 byte-based 编辑 API，不做兼容层
```

M3.5 的原则：

```text
核心编辑 API 不再接受 ByteOffset。
ByteOffset 只用于文件字节、编码边界、外部协议适配和显式坐标转换。
不要恢复旧 byte-based 编辑 API。
不要为了兼容保留两套编辑模型。
```

---

### 5.6 M4：高性能文本存储替换

M4 关注把生产文本存储替换为 `RopeyStorage`。

包括：

```text
引入 ropey 依赖
实现 RopeyStorage
Buffer 默认使用 RopeyStorage
TextStorage 抽象摆脱全文连续 &str 假设
支持基于 CharOffset / TextRange 的局部插入、删除、替换
支持 bytes / chars / UTF-16 code units / lines 等 metrics
Snapshot 升级为基于 ropey clone 的低成本快照
不把 ropey::Rope 暴露为 public API
```

M4 测试放在：

```text
tests/m4_storage.rs
src/tests/storage_consistency.rs
```

M4 的生产代码边界：

```text
src/storage/ 只保留生产存储实现，例如 RopeyStorage。
M4 之后不要继续在 src/storage/ 中保留 StringStorage。
StringStorage 只作为测试 reference model 存在。
如果需要 StringStorage 参考模型，应在 src/tests/storage_consistency.rs 或测试 helper 中定义测试专用 StringStorageRef。
```

M4 differential testing 契约：

```text
同一组编辑同时应用到 RopeyStorage 和 StringStorageRef。
最终文本必须一致。
行数必须一致。
char offset <-> line/column 转换必须一致。
按 char range slice 的结果必须一致。
失败时优先相信 StringStorageRef 的语义结果，再定位 RopeyStorage 的实现偏差。
```

M4 不应该做：

```text
不自研 Rope / Piece Table / Piece Tree。
不把 ropey::Rope 泄漏到 public API。
不把 StringStorage 继续作为生产后端。
不在 M4 顺手实现 Selection / Multi Cursor。
不为了 ropey 改变核心编辑坐标，核心仍然是 CharOffset。
```

---

## 6. `src/tests/` 使用边界

默认不要创建 `src/tests/`。

只有同时满足以下条件，才可以引入：

```text
1. 这个行为很重要，破坏后会导致引擎出错。
2. 这个行为无法通过 public API 精准验证。
3. 这个行为不应该为了测试而暴露成 public API。
4. 这个行为属于 crate 内部不变量，而不是临时实现细节。
5. 测试它能够提高重构安全性，而不是阻碍重构。
```

适合 `src/tests/` 的例子：

```text
Piece Tree / Rope 内部节点不变量
ChangeSet 内部 segment 合并规则
PositionMap 内部映射单调性
Anchor / TrackedRange stickiness 规则
Transaction normalize 后的内部排序与不重叠保证
Storage trait 多实现一致性
Snapshot 与 buffer revision 对应关系
M4 RopeyStorage 与测试 reference model 的 differential consistency
```

不适合 `src/tests/`：

```text
可以通过 public API 测到的行为
短期实现细节
单纯追求覆盖率
GPUI 行为
性能 benchmark
```

---

## 7. 代码设计原则

### 7.1 类型安全

优先使用强类型，不要在 public API 中裸用 `usize` 表达语义不同的量。

推荐：

```text
ByteOffset
Line
LogicalColumn
TextRange
BufferVersion
TransactionId
```

避免：

```text
fn edit(start: usize, end: usize)
fn map(pos: usize)
```

### 7.2 Public API 先稳定语义，不急着暴露内部

默认原则：

```text
能不 public 就不 public。
能 pub(crate) 就不要 pub。
不要为了测试把 private 改成 public。
不要为了 examples 暴露底层细节。
```

### 7.3 不要过早优化

当前早期阶段优先顺序：

```text
正确性 > 可测试性 > API 清晰度 > 性能
```

M4 之前不要过早引入高性能 Rope / PieceTree，先让简单字符串模型成为可靠参考语义。

M4 之后的边界是：生产代码使用 `RopeyStorage`，测试代码保留字符串 reference model。
不要把 `StringStorage` 继续放在 `src/storage/` 生产核心模块中。

### 7.4 错误要显式

错误应该返回明确类型，不要用字符串糊弄。

例如：

```text
CoordinateError
EditError
TransactionError
StorageError
EngineError
```

---

## 8. 修改代码时的工作流

AI 修改代码前应先做：

```text
1. 阅读 AGENTS.md。
2. 阅读相关源码文件。
3. 阅读对应阶段测试文件。
4. 判断修改属于 M0 / M1 / M2 / 后续阶段。
5. 判断应该改 tests/、examples/、src/ 还是文档。
6. 给出简短修改计划。
```

修改时：

```text
优先小步改动。
不要无理由大重写。
不要把多个阶段的能力混在一个改动里。
不要删除已有测试覆盖的能力，除非明确说明原因。
不要让新阶段 testbed 丢失旧阶段体验。
```

修改后：

```text
cargo fmt
cargo test

# 可选：按改动范围跑定向测试/示例
# 具体清单见 docs/STATUS.md
```

如果当前环境不能运行命令，AI 必须明确说明“未实际运行”。

---

## 9. 测试放置决策树

新增测试时按这个顺序判断：

```text
1. 这是 UI / GPUI / 人类体感 / API 接入手感测试吗？
   是 -> examples/
   否 -> 继续

2. 这是性能测试吗？
   是 -> benches/
   否 -> 继续

3. 这个测试能否只通过 public API 完成？
   是 -> tests/
   否 -> 继续

4. 这个测试是否保护重要的 pub(crate) 内部不变量？
   是 -> src/tests/
   否 -> 不要写，或者改成 public 行为测试
```

简化版：

```text
默认 tests/
UI 进 examples/
性能进 benches/
只有重要内部不变量才 src/tests/
```

---

## 10. 文档更新原则

当项目规则发生变化时，必须同步更新文档。

常见场景：

```text
新增阶段能力 -> 先更新 docs/STATUS.md；若能力边界变化再更新 README.md / 编辑引擎能力.md
修改测试目录职责 -> 更新 编辑引擎测试策略.md
新增 AI 协作规范 -> 更新 AGENTS.md
修改 public API 契约 -> 更新对应测试和文档
```

低维护规则（必须遵守）：

```text
1. 非原则性变化（文件名调整、目录移动、阶段文件增减）默认不改主文档。
2. 主文档只维护稳定规则与边界，不维护易变清单。
3. 易变信息统一写入 docs/STATUS.md。
4. 只有“职责边界 / public API 契约 / 测试哲学”变化才更新主文档。
```

不要只改代码，不改必要文档；也不要为非原则性变化频繁改主文档。

---

## 11. AI 回答风格要求

AI 在协助本项目时：

```text
用中文回复。
先说明判断，再给具体改法。
优先给可直接落地的代码。
指出哪些文件要改。
说明哪些测试要跑。
如果不能实际运行测试，要明确说明。
不要假装已经运行过没有运行的命令。
```

代码建议应尽量：

```text
小步
明确
可复制
符合当前阶段边界
不引入额外复杂度
```

---

## 12. 当前重要决策记录

### 12.1 `TextRange::new_unchecked`

项目已删除 public `TextRange::new_unchecked`。

后续不要恢复 public unchecked range 构造器。

如果内部确实需要构造“数学上已证明合法”的 range，应优先使用：

```rust
TextRange::new(start, end).expect("internal invariant: start <= end")
```

并确保该调用不暴露给 public API。

### 12.2 M2 Transaction

M2 Transaction 应该带 `base_version`。

推荐形态：

```rust
pub struct Transaction {
    base_version: BufferVersion,
    edits: EditList,
}
```

`Buffer::apply_transaction` 必须检查版本：

```text
tx.base_version == buffer.version()
```

版本不匹配时返回 `TransactionError::VersionMismatch`。

### 12.3 Examples 不是一次性 demo

`examples/gpui_mN_testbed.rs` 是递进式实验台，不是互相独立的小 demo。

每个阶段都应该继承上一阶段的体感能力。

### 12.4 M4 StringStorage 只作为测试 reference model

M4 之后，`StringStorage` 不应继续作为生产 storage 模块存在。

正确边界：

```text
src/storage/ropey_storage.rs              生产存储实现
src/tests/storage_consistency.rs          内部一致性 / differential testing
src/tests/... 或 tests helper             测试专用 StringStorageRef
```

禁止做法：

```text
src/storage/string_storage.rs             不要在 M4 后继续保留
pub(crate) use string_storage::StringStorage
Buffer 默认使用 StringStorage
为了测试把 StringStorage 暴露到生产模块边界
```

允许做法：

```text
在 src/tests/storage_consistency.rs 中定义 StringStorageRef。
在测试 helper 中定义字符串参考模型。
让 StringStorageRef 实现与 RopeyStorage 相同的内部 trait，用于 differential testing。
```

原则：

```text
reference model 是测试基础设施，不是引擎核心能力。
M4 的生产核心只有 RopeyStorage 和 TextStorage 抽象。
```

### 12.5 Buffer 模块拆分

M6C 之后，`src/buffer/mod.rs` 不应继续承载所有 Buffer 职责。

推荐边界：

```text
src/buffer/mod.rs              Buffer 状态聚合、模块组织、基础构造与简单访问器
src/snapshot.rs                public Snapshot 类型与只读查询
src/coordinates_core.rs        Buffer / Snapshot 共享坐标数学核心
src/buffer/versioning.rs       BufferVersion、低成本 Snapshot 创建与过期判断
src/buffer/coordinates.rs      坐标转换、grapheme、CRLF、DisplayColumn 数学
src/buffer/selection_ops.rs    SelectionSet 状态管理
src/buffer/movement.rs         M6B Word / Identifier / Subword / Symbol 移动
src/buffer/composition/        M6C IME composition（state / validation / workflow）
src/buffer/edit_ops/           Transaction 外围的文本变异与多光标编辑入口
src/buffer/history/            Undo / Redo、HistoryEntry、历史合并
src/buffer/transaction_pipeline/ 事务准备、提交、selection 映射、history 收尾
src/buffer/validation.rs       Buffer 级边界校验
```

原则：

```text
保持 public API 稳定。
按能力域拆实现，不按测试阶段无限扩大 mod.rs。
跨子模块 helper 优先使用 pub(super)，不要提升为 public API。
重构后必须继续通过现有 m0-m6 集成测试。
```

### 12.6 Command 后置阶段（M16 及以后）

当前阶段（M0-M15）不引入 `Command` 层。`Command` 属于宿主输入语义适配，
在编辑引擎内核稳定后再后置到 M16 及以后实现。

推荐边界：

```text
src/command.rs              Command / CommandContext / CommandOutcome 数据模型
src/command_executor.rs     Command -> Buffer 的 crate 内部适配层
src/buffer/*.rs             Buffer 状态与底层编辑能力，不依赖 Command
```

依赖方向必须保持：

```text
UI / GPUI / 快捷键 / 菜单 / 命令面板
        ↓
Command / CommandContext
        ↓
CommandExecutor
        ↓
Buffer public / pub(crate) 能力
        ↓
Transaction / SelectionSet / Movement / Composition / History
```

原则：

```text
CommandExecutor 可以依赖 Buffer。
buffer/ 子模块不要依赖 Command。
transaction.rs、selection.rs、storage/*、coordinates.rs、snapshot.rs 不要依赖 Command。
不只禁止编译依赖倒置，也禁止语义倒置：底层模块不得以 Command 术语命名状态、策略和不变量。
历史合并、事务策略等底层语义必须使用中性领域语言（如 History / Transaction），不能由 Command 概念反向塑形。
如果同一逻辑仅由 Command 触发，也要把状态归属在底层领域模块（history/transaction），Command 只做映射，不持有底层事实。
Command 表达用户意图，Transaction 表达文本变异提交单位。
外部 formatter / LSP apply edit 可以继续直接构造 Transaction，不必伪装成 Command。
```

---

## 13. 最终原则

```text
默认 tests/
UI examples/
性能 benches/
只有重要内部不变量才 src/tests/

M(n) testbed = M(n-1) testbed 的完整体验 + 当前阶段新增能力。

M4 生产存储使用 RopeyStorage。
M4 StringStorage 只能作为测试 reference model，不放进 src/storage/ 生产核心模块。

不要为了测试污染 public API。
不要为了展示新阶段能力丢掉旧阶段体验。
不要把编辑算法正确性寄托在 UI testbed 上。
```
