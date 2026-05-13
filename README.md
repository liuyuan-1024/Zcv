# zom-engine

`zom-engine` 是一个独立的 Rust 纯文本编辑引擎 crate，为编辑器宿主提供工业级底层文本编辑能力。

## 定位

`zom-engine` 只做编辑引擎底座：文本存储、编辑、坐标、事务、历史、快照、区间追踪、投影、读取切片、文件文本边界、错误防御和性能验证。

它不做 UI 渲染、LSP / Tree-sitter provider、diagnostics 生成、项目索引、命令系统、宏录制、后台任务调度或实时多人协作。

## 核心能力

- 基于 `ByteOffset` / `TextRange` 的强类型编辑坐标。
- 基于 `RopeyStorage` 的生产文本存储。
- 事务化编辑、Delta、ChangeSet 和 PositionMap。
- Undo / Redo、历史节点、事务记录和回放。
- Snapshot、BufferVersion 和版本化结果承载。
- SelectionSet、多光标、移动语义和 IME composition。
- Anchor、TrackedRange、MetadataLayer 等通用区间追踪。
- Fold / Projection / Viewport slicing。
- 单 Buffer 搜索、替换和 replace all。
- 文件加载、保存文本边界、大文件策略和防御式错误处理。
- 机器契约测试、property 回归和 benchmark 验证。

## 目录概览

```text
src/       编辑引擎实现
tests/     public API 机器契约测试
examples/  可选交互式 testbed
benches/   性能基准
docs/      能力边界、测试策略和当前状态
```

`src/` 按稳定能力域分层：

```text
buffer/       Buffer 状态、编辑入口、事务管线、历史、生命周期
types/        offset、range、position、version 等强类型
config/       Buffer、encoding、line ending、display、large file 等策略
storage/      TextStorage 抽象与 RopeyStorage
transaction/  Edit、Transaction、Delta、ChangeSet、record
selection/    Cursor、Selection、SelectionSet、movement、composition
tracking/     Anchor、Mark、TrackedRange 及跟随策略
metadata/     MetadataRange / MetadataLayer
projection/   Fold 后的逻辑坐标、投影坐标和 viewport
```

目录模块是实现分层，不承诺长期 import path；外部使用者优先从 crate root 导入 public API。

## 相关文档

- `AGENTS.md`：AI 协作规则与稳定工程约束。
- `docs/编辑引擎能力.md`：能力边界与长期方向。
- `docs/编辑引擎测试策略.md`：测试目录职责与验证原则。
- `docs/STATUS.md`：当前状态、结构概览和建议验证命令。

## 文档维护

主文档只维护稳定规则和大方向。阶段流水、逐文件账本和临时实现细节不进入长期文档；确需记录时，优先放入 `docs/STATUS.md`，并保持高层、可维护。
