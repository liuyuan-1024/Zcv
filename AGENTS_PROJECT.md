# project.md

> 本文记录 `zom` workspace 的项目独有结构与边界。全局协作、验证命令和代码风格见 `global.md`。

## 1. 项目结构

`zom` 是 Cargo workspace 根项目，用于统一管理多个相互黑盒隔离的 crate：

```text
zom-engine      核心文本编辑引擎
zom-workspace   工作区、文件、buffer 编排
zom-view        编辑面状态：view、滚动、selection、fold
zom-command     命令系统
zom-ai          AI 抽象与集成
zom-desktop     桌面入口，组合其他 crate
```

crate 之间只能通过 public API 连接。不要跨 crate 依赖私有实现、源码路径或测试专用细节。

## 2. 工作区规则

根目录只保留一个 Git 仓库和一个 workspace 级 `Cargo.lock`。

不要在子 crate 中重新初始化 Git 仓库。需要保留外部历史时，使用 subtree 或其他明确的历史迁移方式。

## 3. 测试策略

workspace 统一遵循 Rust 常规测试分层：

```text
src/**      源码旁边的单元测试，验证模块内部不变量和私有 helper
tests/      集成测试，只通过 crate public API 验证外部契约
examples/   可选接入实验台和人工体感验证，不替代机器测试
```

单元测试优先写在被测源码旁边，通常使用文件内 `#[cfg(test)] mod tests`。当一个模块由多个文件组成，或测试需要跨同一模块内多个实现文件协作时，可以在该模块目录内放测试子模块；测试仍应跟随被测模块归属，不集中到 crate 根部。

`tests/` 目录只放外部使用者视角的集成测试。集成测试不得依赖 crate 内部模块路径、私有实现、源码布局或测试专用细节；如果测试需要访问私有函数或内部构造器，应迁回源码旁边做单元测试，或者重新审视该能力是否真的需要成为 public API。

不要为了测试把 `private` / `pub(crate)` 提升为 `pub`。新增 public API 前必须先说明长期语义、调用方影响和测试覆盖。

文档测试暂不作为当前 workspace 测试策略的一部分。

UI / GPUI 相关测试可以使用框架要求的测试入口和 harness，但仍遵守同一分工：组件内部状态与 helper 放源码旁边，跨组件组装和外部可观察行为放集成测试或明确的跨模块测试入口。视觉呈现和人工体感验证不替代机器测试。
