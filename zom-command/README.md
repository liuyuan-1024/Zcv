# zom-command

`zom-command` 是 zom 宿主层的命令派发脊柱 crate，承载「所有操作均是命令」这一核心原则与键位模型。

## 定位

键盘、命令面板、AI、菜单都把意图收敛成 `(CommandId, CommandArgs)`，经唯一派发路径进入执行器。`zom-command` 是这条路径的基础设施。

它是命令的**汇聚点（sink）**，不是被各层 import 的服务：输入源产出意图，由组合根喂给执行器。

它依赖「它要编辑的东西」（`workspace` / `view` / `engine`），但**不依赖扩展域**（`ai`，以及将来的 lsp / git）。内建编辑命令是本 crate 存在的理由，co-located 在此、可无头测试；扩展域的命令由 `zom-desktop` 组合根注册 —— handler 闭包捕获扩展服务。

## 核心能力

- **类型擦除的开放注册表**：`CommandRegistry` 存 `CommandId -> (元数据, handler)`，支持 handler 外部注册。不用闭合 enum，让内建 / AI / 插件命令同形。
- **具体的 `CommandContext`**：持有 `&mut Workspace` / `&mut ViewSet` / `&mut CommandQueue`，暴露整个 workspace 与 view-set。扩展域不在 context 里。
- **命令队列组合**：`CommandQueue` 让 handler 入队子命令，执行器排空，不重入。宏 = 录队列，AI agent = 灌队列。
- **执行器不管历史**：`editor.undo` 是一条命令，其 handler 调 `buffer.undo()`；历史由 `zom-engine` 的事务系统记录。
- **键位模型**：`KeyChord` / `KeySequence`（多段 leader key）/ `Keymap`（前缀 trie + when 谓词）/ `KeymapResolution`。键盘解码（OS 事件 → 归一化 `KeyChord`）在 `zom-desktop`，本 crate 只吃归一化结果。
- **参数模型方案 A**：唯一派发路径 `(CommandId, CommandArgs)`，每条命令在自己模块里 `TryFrom<CommandArgs>` 解析；typed 构造器只能是汇入这条路径的语法糖，不开第二扇门。

## 依赖

```text
zom-command → zom-engine
zom-command → zom-workspace
zom-command → zom-view
```

**不依赖 `zom-ai`** —— 与 `zom-ai` 无依赖边，两者在 `zom-desktop` 组合根相遇。

## 结构概览

```text
src/lib.rs    CommandId / CommandArgs / Command / CommandRegistry / CommandHandler
              CommandContext / CommandQueue / CommandExecutor / CommandOutcome
              KeyChord / KeySequence / KeyBinding / Keymap / KeymapResolution
              register_builtin_editor_commands / CommandError
```

骨架阶段为单文件 `lib.rs`。

## 相关文档

- `../AGENTS.md`：workspace 全局协作规则。
- `../TODO.md`：宿主层开发规划，本 crate 对应能力域 3 / 5 / 8（编辑事务、movement 命令、搜索替换命令），阶段 P1。

## 状态

骨架阶段：类型形状与 public API 签名已定，前缀 trie 解析、执行器排空、内建命令实现留待 `TODO.md` P1。
