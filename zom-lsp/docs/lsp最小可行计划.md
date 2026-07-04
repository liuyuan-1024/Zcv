# zom-lsp 最小可行版本计划

> 本文描述 LSP 功能的端到端最小可行版本（MVP）——从协议层到 UI 的完整链路，以及每个阶段的交付判据。

## 总览

```
zom-lsp (本 crate)          zom-workspace                zom-desktop
═══════════════════         ════════════════              ════════════════
LspClient                    LspHighlightProvider           LspHost
  ├─ transport (stdio)         ├─ impl HighlightProvider      ├─ 管理 LspClient 实例池
  ├─ lifecycle (init/shutdown) ├─ semantic tokens → slot      ├─ 按需启动 server
  ├─ document sync             └─ 转发 publishDiagnostics      ├─ 接线 diagnostics 面板
  └─ feature requests                                        ├─ 接线 outline 面板
      (semantic tokens,                                       ├─ 接线 hover/goto/completion
       diagnostics,                                           └─ 更新底栏状态
       hover, goto, ...)
```

## 阶段一：LSP 协议层（本 crate — 已就绪）

### 已交付

- [x] `LspClient`：启动/停止 server、`initialize`/`shutdown` 生命周期
- [x] `StdioTransport`：header-based 组帧、JSON-RPC 收发
- [x] `didOpen` / `didChange` / `didClose` 文档同步
- [x] `LspError`：按 UI 决策路径分类的错误变体
- [x] `ServerCapabilities` 查询（`has_semantic_tokens()` 等）

### 待升级（阶段二之前）

- [ ] **后台 I/O 任务**：当前收发都是同步阻塞。需要把 `StdioTransport` 的读循环放到独立线程/channel，让 `LspClient` 的 public 方法变成非阻塞的 `async` 或 channel 投递。
- [ ] **server 推送通知的路由**：`textDocument/publishDiagnostics`、`window/logMessage` 等通知到达后，需要按类型分发给上层回调。当前只处理 request/response 配对，notification 被 `send_request_sync` 的轮询跳过（这是对的——但上层需要一种方式订阅这些通知）。
- [ ] **多 server 管理**：当前 `LspClient` 一对一管理一个子进程。zom-desktop 需要 `LspHost` 维护 `HashMap<LanguageId, LspClient>`，按打开文件的 `language_id` 路由文档同步。

### 第一阶段验证标准

用一个 mock language server（或直接连 `rust-analyzer`）验证：

1. `LspClient::launch("rust-analyzer", &[], root_uri)` 返回 `Ok`
2. `did_open` 发送后 server 不报错
3. `did_change` 发送增量内容后 server 不报错
4. `shutdown()` 后子进程正常退出

## 阶段二：HighlightProvider 适配

### 目标

在 `zom-workspace`（或 `zom-desktop` 边界层）实现 `HighlightProvider` trait 的 LSP 变体：

```rust
/// LSP semantic tokens 驱动的 HighlightProvider。
///
/// 与 tree-sitter provider 的关键区别：
/// - on_edit 不做 reparse，只更新内部版本号
/// - 真实高亮数据来自 LSP server 异步推送的 semantic tokens
/// - export_syntax_tree 把收到的 tokens 转成 BufferSyntaxTree 写入 slot
struct LspHighlightProvider {
    language_id: LanguageId,
    // 后台任务句柄
    client: Arc<LspClientHandle>,     
    buffer_version: BufferVersion,
    // 缓存最近一次收到的 semantic tokens
    cached_tokens: Option<...>,
}

impl HighlightProvider for LspHighlightProvider {
    fn language(&self) -> LanguageId { self.language_id }

    fn attach(&mut self, buffer: BufferHandle) {
        // 发送 textDocument/didOpen（如果 LspHost 还没发过）
        // 请求一次 full semantic tokens
    }

    fn on_edit(&mut self, buffer: BufferHandle, change: &ChangeSet, version: BufferVersion) {
        // 不 reparse，只记版本号
        // LSP server 的 semantic tokens 推送是异步的
        self.buffer_version = version;
    }

    fn detach(&mut self) {
        // 清理缓存
    }

    fn export_syntax_tree(&self, slot: &BufferSyntaxTreeSlot) {
        // 把 cached_tokens 转成 BufferSyntaxTree 写入 slot
    }
}
```

### 关键设计决策

- **provider 在哪注册**：`LanguageRegistry::register` 目前为 tree-sitter 语言注册 `HighlightWorker::factory`。LSP provider 需要**与 tree-sitter provider 共存**——同一种语言可能同时有 tree-sitter（fallback）和 LSP（优先）。调度层需要支持"多 provider 叠加"或"LSP 优先、tree-sitter fallback"的链式策略。这个决策在阶段二解决。
- **坐标转换**：LSP semantic tokens 使用 UTF-16 行列坐标。`zom-engine` 已有 `Utf16Offset` / `Utf16Position`，需要在 `zom-engine` 的 storage 层补充 byte ↔ utf16 的投影函数。

### 验证标准

打开一个 `.rs` 文件，触发编辑，观察到：
1. `did_open` → server 首次返回 semantic tokens → 文件正确着色
2. 编辑一行 → `did_change` → server 返回增量/全量 tokens → 着色更新
3. 关闭文件 → `did_close` → server 停止推送

## 阶段三：诊断与大纲接入

### 诊断面板

当前状态：
- `diagnostics.show_problems` 命令已注册（`zom-command/src/commands/features/diagnostics.rs`）
- `HostEffect::ShowDiagnostics` 已定义
- 诊断面板 UI 尚未实现（目前无对应 surface）

需要做的：
1. `LspHost` 收到 `textDocument/publishDiagnostics` 通知后，存入 `BTreeMap<Url, Vec<Diagnostic>>`
2. 在 `zom-desktop` 中实现 `DiagnosticsRuntime` surface（参考 `LanguageServersRuntime` 的模式）
3. 命令 `diagnostics.show_problems` → 打开诊断面板 → 列出当前文件的所有诊断
4. 底栏诊断图标显示当前文件的问题数量

### Outline 面板

当前状态：
- `OutlineRuntime` 渲染占位灰字："Outline占位中"
- 注释写 "LSP 接入后填充符号大纲"

需要做的：
1. `LspHost` 收到 buffer 后就发送 `textDocument/documentSymbol` 请求
2. 将 `DocumentSymbol` 层级转为 outline 面板的树形数据
3. `OutlineRuntime::render` 渲染真实符号列表
4. 点击符号 → 跳转到对应位置（通过命令系统：新增 `editor.go_to_position` 或复用现有跳转命令）

### 验证标准

1. 打开 Rust 文件 → Outline 面板显示 struct/fn/impl 树
2. 保存文件后诊断面板刷新（cargo check 的 warning/error 显示在面板中）
3. 点击 outline 符号 → 光标跳转到对应位置

## 阶段四：代码智能（Hover / Goto / Completion）

这些是 LSP 最核心的用户价值，但也是实现最复杂的部分。放在阶段四是因为它们依赖阶段三的 UI 基础设施（diagnostics panel、outline panel 已跑通），并且需要更精细的 UI 交互。

### Hover

1. 快捷键/鼠标悬停触发 `textDocument/hover`
2. 渲染 hover 卡片：markdown 内容 + 可选语法高亮
3. GPUI 实现：floating tooltip 定位在光标位置

### Go to Definition

1. `editor.go_to_definition` 命令 → `textDocument/definition`
2. 返回 `Location` → 如果是同文件跳转，移动光标；跨文件则打开新 buffer
3. 返回 `LocationLink[]` 的情况也需要处理

### Completion

1. 输入时触发 `textDocument/completion`（debounce ~200ms）
2. 渲染补全列表（GPUI popover）
3. `completionItem/resolve` 补充文档/详情

### 验证标准

以 rust-analyzer 为目标 server，任意打开一个 Rust 项目：
1. 悬停变量显示类型信息
2. 点击函数名 → 跳转到定义
3. 输入 `Vec::` 弹出方法列表
4. 输入时自动触发补全（不卡顿输入）

---

## 不在此 MVP 范围内

以下能力明确不做，记下来避免 scope creep：

- **多 workspace / 多 root**：一个 `LspHost` 对应一个项目根目录
- **tcp/socket transport**：只支持 stdio
- **server 配置 UI**：不提供 GUI 配置 language server 路径，先硬编码常见 server 名（`rust-analyzer` 等），后续通过 config.toml 配置
- **didSave 通知**：先不做——`did_change` 已经覆盖编辑态，保存语义留给后续
- **code action / rename / formatting**：阶段四之后再做
- **inlay hints**：阶段四之后
- **snippet 补全**：阶段四先做纯文本补全，snippet 占位符解析留后
