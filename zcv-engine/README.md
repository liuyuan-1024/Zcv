# zcv-engine

`zcv-engine` 是一个独立的 Rust 纯文本编辑引擎 crate，提供工业级底层文本编辑能力。

## 定位

`zcv-engine` 只做编辑引擎底座：文本存储、编辑、坐标、事务、历史、快照、通用区间追踪、读取切片、文件文本边界和错误防御。

UI、产品业务、项目编排、协议集成、任务调度和网络协作不属于本 crate。

## 核心能力

- 基于 `ByteOffset` / `TextRange` 的强类型编辑坐标。
- 基于 `RopeyStorage` 的生产文本存储。
- 事务化编辑，以及统一承载事务身份、历史归属、Delta 和 ChangeSet 的 `TransactionOutcome`。
- 独立 `TextSubscription`：事件只负责唤醒，Snapshot 表达当前真相，组合 TextPatch 保证连续编辑不丢失。
- Undo / Redo、历史节点、事务记录和回放。
- Snapshot、BufferVersion 和版本化结果承载。
- Selection / SelectionSet 原语、PositionMap 映射和纯文本 word / grapheme 边界查询。
- Anchor、TrackedRange、VersionedResult、VersionedRangeSet 等通用区间追踪。
- 逻辑文本 Viewport slicing。
- 单 Buffer 同步匹配、替换和 replace all。
- 文件加载、保存文本边界、大文件策略和防御式错误处理。
- 机器契约测试和差分回归。

## 目录概览

```text
src/       编辑引擎实现
tests/     crate 级集成测试
examples/  可选交互式 testbed
docs/      能力边界和当前状态
```

`src/` 按稳定能力域分层：

```text
buffer/       Buffer 文本状态、编辑入口、事务管线、历史、生命周期
types/        offset、range、position、version 等强类型
config/       Buffer、encoding、line ending、display、large file 等策略
storage/      TextStorage 抽象与 RopeyStorage
transaction/  Edit、Transaction、Delta、ChangeSet、record
text_changes.rs  独立订阅、连续 Patch 合成与消费
selection/    Cursor、Selection、SelectionSet 与纯文本边界词汇
tracking/     Anchor、Mark、TrackedRange 及跟随策略
```

目录模块是实现分层，不承诺长期 import path；外部使用者优先从 crate root 导入 public API。

## 相关文档

- `docs/引擎能力.md`：能力边界与长期方向。
- `docs/STATUS.md`：当前状态、结构概览和建议验证命令。
- `../CODEX.md`：workspace 项目结构与协作规则。

## 文档维护

主文档只维护稳定规则和大方向。阶段流水、逐文件账本和临时实现细节不进入长期文档；确需记录时，优先放入 `docs/STATUS.md`，并保持高层、可维护。
