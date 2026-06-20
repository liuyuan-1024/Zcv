# zom-command

`zom-command` 是 zom 宿主层的命令与快捷键基础设施，承载「可命名的离散动作是命令」这一原则与键位模型。

## 定位

键盘快捷键、命令面板、AI、菜单等入口都可以把可命名动作收敛成 `(CommandId, CommandArgs)`，经唯一命令派发路径进入执行器。连续输入设备事件由桌面端输入意图层处理。

它是命令的汇聚点（sink），不是被各层 import 的服务：输入源产出意图，由组合根喂给执行器。

它依赖「它要编辑的东西」（`workspace` / `view` / `engine`），但不依赖扩展域（`ai`，以及将来的 lsp / git）。内建编辑命令是本 crate 存在的理由，放在这里并保持可无头测试；扩展域的命令由 `zom-desktop` 组合根注册，handler 闭包捕获扩展服务。

## 核心能力

- **类型擦除的开放注册表**：`CommandRegistry` 存 `CommandId -> (元数据, handler)`，支持 handler 外部注册。不用闭合枚举，让内建 / AI / 插件命令同形。
- **具体的 `CommandContext`**：持有 `&mut Workspace` / `&mut ViewSet` / `&mut CommandQueue` / `&mut EffectQueue`，暴露整个 workspace 与 view-set。扩展域不在 context 里；需要宿主接力的动作通过 `HostEffect` 发出。
- **命令队列组合**：`CommandQueue` 让 handler 入队子命令，执行器排空，不重入。宏 = 录队列，AI agent = 灌队列。
- **执行器不管历史**：`editor.undo` 是一条命令，其 handler 调 `buffer.undo()`；历史由 `zom-engine` 的事务系统记录。
- **键位模型**：`KeyChord` / `KeySequence`（多段 leader key）/ `Keymap`（前缀 trie + when 谓词）/ `KeymapResolution`。键盘解码（OS 事件 → 归一化 `KeyChord`）在 `zom-desktop`，本 crate 只吃归一化结果。
- **参数模型方案 A**：唯一派发路径 `(CommandId, CommandArgs)`，每条命令在自己模块里 `TryFrom<CommandArgs>` 解析；typed 构造器只能是汇入这条路径的语法糖，不开第二扇门。

## 依赖

```text
zom-command → zom-engine
zom-command → zom-workspace
zom-command → zom-workspace
```

**不依赖 `zom-ai`** —— 与 `zom-ai` 无依赖边，两者在 `zom-desktop` 组合根相遇。

## 目录概览

```text
src/lib.rs    CommandId / CommandArgs / Command / CommandRegistry / CommandHandler
              CommandContext / CommandQueue / CommandExecutor / CommandOutcome
              KeyChord / KeySequence / KeyBinding / Keymap / KeymapResolution
              CommandBuilder / CommandError
src/effects.rs
              HostEffect / EffectQueue
src/keymap_format.rs
              chord → 平台快捷键文案投影
src/commands/
              editor / workspace / window / panels / diagnostics / settings catalog
tests/        command 契约测试
```

核心机制在 `src/lib.rs`；具体命令按 catalog 放在 `src/commands/`，handler、typed args、typed builder 与默认键位同处声明。

## 相关文档

- [`docs/命令与快捷键.md`](docs/命令与快捷键.md)：完整设计文档 —— 模块边界、数据模型、catalog 模式、HostEffect 解耦、键位约定、加新命令的步骤、反例清单。先看这一份。
- `../AGENTS_GLOBAL.md`、`../AGENTS_PROJECT.md`：workspace 全局规则与项目规则。

## 文档维护

本 README 只维护稳定边界、核心能力和依赖关系；当前命令以 `src/commands/` catalog 与 `tests/command_contract.rs` 为准。
