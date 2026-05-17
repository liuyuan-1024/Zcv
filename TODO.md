# zom 开发规划

> 本规划以 `zom-engine/docs/引擎能力.md` 的能力域为主轴。`zom-engine` 已作为纯文本编辑引擎底座收口，后续开发重点是把 engine 能力稳定接入 `zom-workspace`、`zom-view`、`zom-command`、`zom-ai` 和 `zom-desktop`，形成可用编辑器骨架。
>
> 宿主层的 crate 划分、依赖图与各 crate 核心 public API 形状已在《zom 宿主层架构规划》中锁定，骨架已铺好。本文的能力域与 P0–P4 阶段都建立在那个稳定边界上。

## 总原则

- `zom-engine` 继续保持纯粹底层编辑引擎定位，不塞入 UI、命令语义、项目管理、AI provider 或跨文件业务。
- crate 之间只通过 public API 连接，不跨 crate 依赖私有实现、源码路径或测试细节。
- 6 个 crate 的职责:
  - `zom-engine` —— 文本编辑底座(冻结)。
  - `zom-workspace` —— 文档/模型层:buffer 与文件生命周期(「有什么」)。
  - `zom-view` —— 编辑面状态层:view/pane、滚动、本视图 selection 与 fold(「我怎么看它」)。
  - `zom-command` —— 命令派发脊柱 + 键位模型;「所有操作均是命令」。
  - `zom-ai` —— AI 抽象:provider trait + request/proposal + proposal→transaction 转换。
  - `zom-desktop` —— GPUI 外壳 + 组合根;内部 `shell` / `app` 分模块。
- 依赖图(无环):`zom-ai` 只依赖 `zom-engine` 且保持零网络依赖;`zom-ai` 与 `zom-command` **无依赖边**,在 `zom-desktop` 组合根相遇;`zom-command` 依赖 `workspace`/`view`/`engine` 但不依赖 `ai`。
- 命令参数采用方案 A:唯一派发路径 `(CommandId, CommandArgs)`,每条命令在自己模块里 `TryFrom<CommandArgs>` 解析。
- 新增 public API 前先明确长期语义、调用方影响、错误边界和测试覆盖。
- 修改 Rust 代码后默认在根目录运行 `cargo fmt` 和 `cargo test --workspace`；如果只运行定向检查，需要在回复中说明范围。

## 能力域规划

### 1. 文本存储与文件边界

Engine 已提供：

- `Buffer` 创建、加载、reload、保存文本边界。
- dirty、只读、大文件和超大事务防御。
- 编码、BOM、换行策略等文件文本进入和离开 Buffer 的底层边界。

宿主侧规划：

- 在 `zom-workspace` 中完善 `Workspace`、`WorkspaceBuffer`、`BufferId` 的生命周期模型。
- 增加 `open_file`、`open_text`、`save_file`、`save_as`、`close_buffer`。
- 管理路径、保存点、dirty 状态、只读状态和活动 buffer。
- 暂不做文件监听、冲突弹窗和保存 UI；这些属于宿主交互层，后续单独规划。

### 2. 坐标与文本读取

Engine 已提供：

- 以 `ByteOffset` / `TextRange` 为核心的强类型坐标。
- `Position`、`CharOffset`、`Utf16Position`、`DisplayColumn` 等派生坐标转换。
- line / viewport / visible line 读取切片。

宿主侧规划：

- `zom-view` 保存视图状态:看哪个 buffer、滚动位置、可见范围、本视图的 selection 与 fold 实例。
- 文本显示通过 `Snapshot`、`ViewportSlice` 或 projection 结果读取。
- `zom-desktop` 不自行维护核心编辑坐标，不裸用 `usize` 表达编辑语义。
- 所有坐标转换统一走 engine public API。

### 3. 编辑事务与变更映射

Engine 已提供：

- `Transaction`、`EditList`、`Delta`、`ChangeSet`、`PositionMap`。
- 事务版本校验和原子提交。
- selection after edit、changed ranges 和事件记录。

宿主侧规划：

- `CommandContext` 是具体结构体，持有 `&mut Workspace` 与 `&mut ViewSet`(全部 buffer 与 view，含活动指向)+ `&mut CommandQueue`。
- 增加基础编辑命令：
  - `editor.insert_text`
  - `editor.delete_backward`
  - `editor.delete_forward`
  - `editor.replace_selection`
  - `editor.select_all`
- 命令层只表达用户操作意图，不重复实现 engine 编辑算法。
- 所有文本变异最终归一到 `Buffer` / `Transaction` 管线。
- 命令组合走 `CommandQueue`,不重入;执行器不自管历史,`editor.undo` 的 handler 直接调 `buffer.undo()`。

### 4. 历史、快照与版本

Engine 已提供：

- Undo / Redo、分支历史、历史预算。
- `TransactionRecord`、回放、`Snapshot`、`BufferVersion`。
- `VersionedResult` 和过期结果处理。

宿主侧规划：

- 在 `zom-command` 中提供 `editor.undo`、`editor.redo`。
- 在 `zom-workspace` 中提供活动 buffer 的历史能力转发。
- AI 提案、搜索结果、未来 diagnostics 等外部结果都必须绑定版本。
- 版本不匹配时拒绝直接应用，必要时走 remap 或重新生成。

### 5. 光标、选区与组合输入

Engine 已提供：

- `SelectionSet`、多光标、word / subword / symbol movement。
- selection after edit。
- IME composition 的底层状态和提交流程。

宿主侧规划：

- 在 `zom-command` 中增加 movement 命令：
  - 字符级左右移动。
  - 行内首尾移动。
  - word / subword / symbol 移动。
  - 扩展选区移动。
- 活动 view 的 `SelectionSet` 实例由 `zom-view` 持有,movement 命令经 `CommandContext` 改它。
- `zom-desktop` 接收键盘、鼠标或 IME 事件后转换为 command，不直接修改 Buffer;OS 按键 → 归一化 `KeyChord` 的解码在 `zom-desktop`。
- IME 先完成最小闭环：start / update / commit / cancel;start / commit 走命令,update 走直接通道喂活动 view 的 `CompositionState`。

### 6. 区间追踪与外部结果承载

Engine 已提供：

- `Anchor`、`TrackedRange`、`MetadataLayer`、`VersionedRangeSet`。
- 版本绑定、范围跟随、失效策略和 payload 承载。
- UTF-16 边界转换能力。

宿主侧规划：

- `zom-ai` 的 `ProposedEdit` 已用 engine `TextRange`,`AiRequest` / `AiProposal` 已携带 `BufferVersion`(骨架已定型)。
- AI 编辑流程：
  - 从当前 buffer 获取 `Snapshot`。
  - 构造带版本的 `AiRequest`。
  - `AiProvider::propose`(one-shot async)返回 `AiProposal`。
  - `proposal_to_transaction` 校验版本和 range。
  - 转换为 `Transaction`。
  - 应用或拒绝。
- 搜索高亮、AI 建议、未来 diagnostics / semantic tokens 都优先用 metadata / versioned range 承载，不在 engine 中膨胀成专用业务 adapter。

### 7. 折叠、投影与读取切片

Engine 已提供：

- `FoldSet`、`HiddenRange`、`Projection`、`ProjectedRange`。
- logical line 与 projected line 映射。
- projected viewport slicing。

宿主侧规划：

- 第一阶段只接 `ViewportSlice`，保证能显示当前 buffer 文本;视图侧的 `ViewportState` 在 `zom-view`。
- 第二阶段接 `FoldSet` / `Projection`，支持折叠后的可见文本和坐标映射;`FoldSet` 实例由 `zom-view` 按视图持有(同一 buffer 的两个分屏可有不同折叠)。
- 折叠按钮、占位符样式、像素布局和绘制策略只放在 `zom-desktop`。

### 8. 单 Buffer 搜索与替换

Engine 已提供：

- literal / regex 搜索。
- 结果版本绑定。
- 单次替换和 replace all 原子事务。
- 搜索结果转 metadata layer。

宿主侧规划：

- 在 `zom-command` 中增加：
  - `editor.find`
  - `editor.find_next`
  - `editor.find_previous`
  - `editor.replace_current`
  - `editor.replace_all`
- `zom-workspace` 先只编排当前 buffer 搜索。
- 跨文件搜索、任务调度、取消策略和结果面板后续放在 workspace / desktop 层，不进入 engine。

### 9. 防御、性能与验证

Engine 已提供：

- 坐标、编辑、事务、存储、历史等分层错误模型。
- 大文件策略、历史预算、超大事务防御。
- property 回归和粗粒度内存观测入口。

宿主侧规划：

- `zom-workspace` 测试 buffer 生命周期、文件保存边界、活动 buffer 切换。
- `zom-view` 测试 view/pane 多重性、活动 view 切换、viewport / fold 状态(无头可测,不经 GPUI)。
- `zom-command` 测试命令注册、快捷键绑定、命令执行上下文和错误返回。
- `zom-ai` 测试版本不匹配、非法 range、提案应用原子性。
- `zom-desktop` 先保持轻量 smoke test，避免 UI 测试反向污染 engine API。

## 阶段计划

### P0：Workspace 骨架收口

目标：让 `zom-workspace` 成为 engine 的稳定宿主外壳。

- [x] 明确 `Workspace` 的活动 buffer 模型。
- [x] 增加 `open_file` / `save_file` / `save_as` / `close_buffer`。
- [x] 暴露 buffer dirty / path / readonly 等状态查询。
- [x] 补充 workspace 生命周期测试。
- [x] 根目录运行 `cargo fmt` 和 `cargo test --workspace`。（`cargo fmt` 已完成；`cargo test --workspace` 已完成。）

### P1：Command 到编辑事务闭环

目标：所有基础编辑都通过 command 进入 engine。

- [x] 落地 `CommandArgs` 表示与 `TryFrom<CommandArgs>` 解析约定。
- [x] 落地 `CommandExecutor::run` 排空队列逻辑。
- [x] 在 `register_builtin_editor_commands` 中接入插入、删除、替换选区、全选。
- [x] 接入 undo / redo(handler 调 `buffer.undo()` / `buffer.redo()`)。
- [x] 接入基础 selection movement(改活动 view 的 `SelectionSet`)。
- [x] 落地 `Keymap` 前缀 trie 解析与 `KeymapResolution`。
- [x] 补充 command 契约测试。

### P2：Desktop 最小可用编辑循环

目标：`zom-desktop` 能跑通最小编辑体验。

- [x] 接入 GPUI 外壳、embedded assets、TopBar / Body / BottomBar 基础布局和 panel 骨架。
- [ ] `app` 启动时创建 workspace、活动 buffer 和活动 view。
- [ ] 显示当前 view 文本。
- [ ] 输入解码:OS 按键 → 归一化 `KeyChord` → keymap 解析 → 命令队列 → 执行器。
- [ ] 支持删除、移动光标、撤销、重做。
- [ ] 保持 `shell` 只做事件转换和显示，不复制编辑语义。

### P3：搜索、替换、折叠与 viewport

目标：把 engine 的读取、搜索和投影能力接入宿主体验。

- [ ] 接入当前 buffer find / replace。
- [ ] 用 metadata 或 versioned result 承载搜索高亮。
- [ ] 接入 viewport slice。
- [ ] 接入 fold / projection 的最小可用路径。
- [ ] 补充搜索与投影接入测试。

### P4：AI 编辑提案闭环

目标：AI 不直接改文本，而是生成可校验、可预览、可应用的事务提案。

- [ ] 为 `AiRequest` 增加 buffer 版本和必要上下文。
- [ ] 为 `AiProposal` 增加版本绑定和 engine range 表达。
- [ ] 实现 proposal -> transaction 的校验转换。
- [ ] 支持 apply / reject。
- [ ] 版本不匹配时拒绝应用或重新生成。
- [ ] 补充 AI 提案原子性和错误路径测试。

## 近期优先级

1. 先做 P0，让 `zom-workspace` 具备稳定 buffer / 文件生命周期。
2. 再做 P1，让所有编辑入口通过 `zom-command` 统一进入 engine。
3. 继续做 P2，用 `zom-desktop` 验证最小可用编辑闭环。
4. 搜索、折叠、AI 都建立在 P0-P2 的稳定边界之上。
