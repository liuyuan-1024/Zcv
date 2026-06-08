# zom 开发规划

> 本规划以 `zom-engine/docs/引擎能力.md` 的能力域为主轴。`zom-engine` 已作为
> 纯文本编辑引擎底座收口，后续开发重点是把 engine 能力稳定接入 `zom-workspace`、
> `zom-view`、`zom-command`、`zom-ai` 和 `zom-desktop`，形成可用编辑器骨架。
>
> 宿主层 crate 划分、依赖图与各 crate 核心 public API 形状已在《zom 宿主层架构
> 规划》中锁定。本文的能力域与阶段都建立在那个稳定边界上。

## 总原则

- `zom-engine` 继续保持纯粹底层编辑引擎定位，不塞入 UI、命令语义、项目管理、
  AI provider 或跨文件业务。
- crate 之间只通过 public API 连接，不跨 crate 依赖私有实现、源码路径或测试细节。
- 6 个 crate 的职责：
  - `zom-engine` —— 文本编辑底座（冻结）
  - `zom-workspace` —— 文档 / 模型层：buffer 与文件生命周期（「有什么」）
  - `zom-view` —— 编辑面状态层：view / pane、滚动、本视图 selection 与 fold
  - `zom-command` —— 命令派发脊柱 + 键位模型；「所有操作均是命令」
  - `zom-ai` —— AI 抽象：provider trait + request / proposal + proposal→transaction
  - `zom-desktop` —— GPUI 外壳 + 组合根；内部 `shell` / `app` 分模块
- 依赖图（无环）：`zom-ai` 只依赖 `zom-engine` 且保持零网络依赖；`zom-ai` 与
  `zom-command` **无依赖边**，在 `zom-desktop` 组合根相遇；`zom-command` 依赖
  `workspace` / `view` / `engine` 但不依赖 `ai`。
- 命令参数：唯一派发路径 `(CommandId, CommandArgs)`，每条命令在自己模块里
  `TryFrom<CommandArgs>` 解析。
- 新增 public API 前先明确长期语义、调用方影响、错误边界和测试覆盖。
- 修改 Rust 代码后默认在根目录运行 `cargo fmt` 和 `cargo test --workspace`；
  只跑定向检查时需在回复中说明范围。

## 已完成

| 阶段 / 主题 | 收口 |
|---|---|
| P0 Workspace 骨架 | `Workspace` / `WorkspaceBuffer` / `BufferId` 生命周期；`open_file` / `save_file` / `save_as` / `close_buffer`；dirty / path / readonly 状态查询 |
| P1 Command 到编辑事务闭环 | `CommandArgs` / `TryFrom`、`CommandExecutor::run`、editor 命令 catalog（插入 / 删除 / 替换 / 选择 / undo / redo / movement）、`Keymap` 前缀 trie |
| P2 Desktop 最小编辑循环 | GPUI 外壳 + TopBar / Body / BottomBar；keymap → 命令队列 → 执行器；IME commit + preedit 直通；阶段 2 范围背景渲染原语；selection / 多光标 caret |
| P3 搜索与视口 | `BufferSearch` 数据模型（per-buffer 共享 + `try_remap`）；search 接入阶段 2 高亮；search 系列 handler；`ViewportSlice` 替换全文读取 |
| 语法高亮 render-time query | 单全局 `SyntaxWorker` + tree-sitter 增量 reparse；worker 把 `Arc<BufferSyntaxTree>` 写共享 slot，paint 端按 viewport 现查 Query。主线程 `tree_slot.try_edit` 同步推坐标消掉 token 内插入的首帧错色。`MAX_HIGHLIGHT_BYTES = 16 MiB`。详见 [`zom-desktop/docs/桌面端语法高亮.md`](zom-desktop/docs/桌面端语法高亮.md) |
| 搜索异步化 | 引擎：`SearchHandle` + 协作式取消 + `SearchProgress`。Workspace：`BufferSearch` 持 pending handle，`sync` 非阻塞，`pump_pending_search` 渲染线程每帧 drain。`DEFAULT_REGEX_HAYSTACK_BYTE_LIMIT` 8 MiB 硬限已删（10 MiB 全文 regex 测试 320 ms） |

## 进行中 / 未开工

### P3 剩余

- [ ] 接入 fold / projection 的最小可用路径——`FoldSet` 已挂在 `ViewState` 上，
  desktop 渲染尚未消费 `Projection`，折叠按钮 / 占位符 / 坐标映射都没接。
- [ ] 补充搜索与投影接入测试。
- [ ] IME marked text 作为阶段 2 第二个消费者接入（暂缓，等真有 IME 体验问题再开）。

### P4 AI 提案闭环（草案）

`zom-ai` 当前仅有 lib.rs 骨架。需要：

- `ProposedEdit` 从裸 byte range 收口到 engine `TextRange`；`AiRequest` /
  `AiProposal` 携带 `BufferVersion`。
- 流程：取 `Snapshot` → 带版本的 `AiRequest` → `AiProvider::propose` (one-shot
  async) → `proposal_to_transaction` 校验版本和 range → 应用 / 拒绝。
- 在 desktop 组合根接入；`zom-ai` 与 `zom-command` 仍保持无直接依赖。

### 外部文件系统变更同步（独立工作流）

不绑定 P 阶段，可与 P3 / P4 并行。

**M1 最小可用**

- [ ] 引入 `notify` 依赖，封装 `zom-workspace::fs_watch`：监听线程 + 事件归一
  （Created / Removed / Modified；重命名拆 remove+create）+ 主线程 channel。
- [ ] 项目树监听：根目录变更按增量更新 `project_tree`，命中忽略规则的事件短路；
  debounce 50–150ms。
- [ ] 打开 buffer 监听：干净 buffer 收 Modified 走静默 reload；buffer 被
  Removed 进入 orphan 状态。
- [ ] 写回声抑制：保存路径在 `(mtime, hash)` 窗口内的事件忽略。
- [ ] 集成测试：临时目录 fixture，覆盖 4 类事件的归一与去抖。

**M2 冲突解决**

- [ ] dirty buffer 外部变更：标 "存在外部变更"，提供 reload / keep local 命令。
- [ ] 冲突解决 UI：最小提示条 + 命令入口（三方合并暂不做）。
- [ ] orphan buffer 的 save_as 路径与项目树重新绑定。

**M3 风险与性能**

- [ ] 大项目首次启动监听的内存 / 句柄预算评估。
- [ ] git checkout / rebase 风暴的批量去抖回归。
- [ ] 符号链接、外部挂载、网络盘的边界处理与降级策略。

### 搜索异步化可选 polish

不阻塞主线，性能 / UX 优化项：

- [ ] regex 跨 chunk DFA：用 `regex-automata` 消除连续 haystack 物化，让 100 MiB+
  文件搜索不再一次性分配。
- [ ] pending 期 GPUI wake：用定时器 / `cx.spawn` 在 `is_searching()` 期间发
  `cx.notify()`，让用户不操作也能看到结果自动落地。
- [ ] `SearchHandle::progress()` 面板可视化进度条。

## 能力域参考

各能力域在引擎层已基本冻结；本节作为"哪条 API 服务哪个用途"的索引，**不再
重复列已完成清单**。详细完成情况见上方"已完成"表。

| 能力域 | Engine API | 宿主侧消费点 | 状态 |
|---|---|---|---|
| 文本存储与文件边界 | `Buffer::from_text` / `Buffer::from_reader` / `Buffer::save_to` | `zom-workspace::Workspace::open_file` 等 | 收口 |
| 坐标与文本读取 | `ByteOffset` / `TextRange` / `Snapshot::slice_*` / `ViewportSlice` | `zom-view::ViewState`、desktop 渲染 | 收口 |
| 编辑事务与变更映射 | `Transaction` / `EditList` / `Delta` / `ChangeSet` / `PositionMap` | `editor.*` 命令 | 收口 |
| 历史、快照与版本 | `Buffer::undo` / `redo` / `Snapshot` / `BufferVersion` / `VersionedResult` | `editor.undo` / `editor.redo`；AI 提案版本校验 | 收口（AI 路径待 P4） |
| 光标、选区与组合输入 | `SelectionSet` / movement API / `CompositionState` | `editor.*` movement 命令；desktop IME 路径 | 收口（marked text UI 待补） |
| 区间追踪与外部结果承载 | `Anchor` / `TrackedRange` / `MetadataLayer<T>` / `VersionedRangeSet` | 语法高亮 / 诊断 / inlay hint（统一）；AI 待 P4；**搜索高亮是例外，直接持 `SearchResult`** | 语法已用；其他待生 |
| 折叠、投影与读取切片 | `FoldSet` / `Projection` / `ProjectedRange` / `ProjectedViewport` | `zom-view::ViewState.folds`；desktop 渲染消费 `Projection` | viewport 已接，fold 待接 |
| 单 Buffer 搜索与替换 | `Buffer::search` / `search_regex` / `replace_*` / `SearchHandle` / `SearchProgress` | `zom-workspace::BufferSearch` + `editor.find_*` / `search.replace_*` | 收口 |
| 防御、性能与验证 | 分层错误模型、大文件策略、property 回归 | 各 crate 测试 | 持续 |
| 外部文件系统变更同步 | 不在 engine | `zom-workspace::fs_watch`（未实现） | 未开工 |

## 近期优先级

1. **fold / projection 最小可用路径**——P3 唯一未完成项，desktop 接 `Projection`
   消费折叠后的可见文本与坐标映射，配上折叠按钮 / 占位符的最小 UI。
2. **外部文件系统同步 M1**——`notify` 接入、项目树增量、干净 buffer 静默 reload、
   写回声抑制、集成测试。
3. **P4 AI 提案闭环草案**——`AiRequest` / `AiProposal` 收口到 engine `TextRange`
   + `BufferVersion`，跑通"一次性 propose 后转 Transaction 应用"的最短路径。
4. （可选）搜索异步化的 polish 项——按需打开。
