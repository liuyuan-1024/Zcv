# zom

> 一个用 Rust 写的现代桌面文本编辑器。引擎纯净、命令统一、可无头测试。

[特性](#特性) · [安装](#安装) · [快速上手](#快速上手) · [架构](#架构) · [路线图](#路线图) · [贡献](#贡献)

---

## 特性

- ⚡ **原生性能** —— Rust + GPU 加速的 GPUI 渲染，启动毫秒级、滚动零卡顿。
- ✍️ **现代编辑体验** —— 多光标、IME、软换行、折叠、跳转到行、多标签页。
- 🌳 **语法高亮** —— 内置 Tree-sitter 语法高亮，覆盖 Rust / TOML / Markdown / JSON / YAML / Bash / HTML / CSS / JavaScript / TypeScript / Java / Python。
- 🔌 **LSP 集成** —— 语言服务器协议层，统一的 json-rpc 客户端与生命周期管理。
- 📝 **Markdown 预览** —— 实时渲染预览，支持图片、可点击链接、锚点滚动、GFM 表格与任务列表、代码块语法高亮。
- 🎯 **一切皆命令** —— 键盘、命令面板和菜单都通过同一条派发路径；宏即“录命令队列”。
- ⌨️ **可编排键位** —— 多段 leader key、前缀 trie、`when` 谓词，向 Emacs / Vim / VS Code 看齐。
- 🎨 **主题跟随系统** —— 自动跟随 macOS/Windows 系统亮暗模式。
- 🔬 **可无头测试的内核** —— 编辑引擎、视口数学、命令派发全部能脱离 GUI 单独测，回归不靠人眼。

## 安装

### 预编译版本

预编译版本请见 [Releases](../../releases) 页面。

### 从源码构建

需要 [Rust 工具链](https://rustup.rs)（edition 2024）。

```bash
git clone <repo-url> zom
cd zom
cargo run -p zom-desktop --release
```

### 平台支持

| 平台    | 状态        |
| ------- | ----------- |
| macOS   | ✅ 已支持   |
| Windows | ✅ 已支持   |
| Linux   | ⏳ 暂未支持 |

## 快速上手

启动后先进入项目选择器，选一个目录即进入编辑工作区。默认快捷键（macOS `⌘`，Windows `Ctrl`，下表记作 `Mod`）：

**项目与面板**

| 快捷键        | 操作            |
| ------------- | --------------- |
| `Mod+O`       | 打开项目选择器  |
| `Mod+Shift+E` | 切换文件树面板  |
| `Mod+Shift+F` | 项目内搜索      |
| `Mod+,`       | 打开设置        |
| `Mod+Shift+K` | 查看全部快捷键  |

**编辑**

| 快捷键              | 操作                |
| ------------------- | ------------------- |
| `Mod+S`             | 保存当前文件        |
| `Mod+W`             | 关闭当前标签        |
| `Mod+L` / `Mod+H`   | 切到下一个 / 上一个标签 |
| `Mod+Z` / `Mod+Shift+Z` | 撤销 / 重做     |
| `Mod+A`             | 全选                |
| `Mod+C` / `Mod+X` / `Mod+V` | 复制 / 剪切 / 粘贴 |
| `Mod+F`             | 当前文件查找        |
| `Mod+G`             | 跳转到指定行列      |

**窗口**

| 快捷键          | 操作       |
| --------------- | ---------- |
| `Mod+Q`         | 退出       |
| `Mod+M`         | 最小化窗口 |
| `Mod+Shift+M`   | 最大化窗口 |

键位由 `Keymap` 注册，后续会开放用户自定义。

## 架构

本节只描述重构前的当前实现，不能作为目标边界。破坏性重构的唯一规范和验收基线见 [`目标架构设计.md`](目标架构设计.md)。

`zom` 是一个 Cargo workspace，按职责拆成 6 个互为黑盒的 crate：

```
zom-desktop  ─┐  组合根：GPUI 外壳、输入解码、wiring
              │
zom-command  ─┤  命令派发脊柱 + 键位模型
zom-workspace┤  缓冲区与文件生命周期、视图状态
zom-lsp      ─┤  LSP 协议层：JSON-RPC 客户端与能力抽象
              │
zom-engine   ─┘  纯文本编辑引擎底座
zom-bench       （独立基线测量套件）
```

**核心设计**：

- 每个 crate 只通过 public API 连接，跨 crate 不依赖私有实现。
- `zom-engine` 不知道 UI 和命令的存在，因此可独立演进并被复用。
- "同一文件开两个分屏会不同的状态"归 view 模块，"不会不同的状态"归 `zom-workspace`。
- 历史不归命令执行器——`editor.undo` 只是一条命令，真实事务由引擎记录。

各 crate README：[engine](zom-engine/README.md) · [workspace](zom-workspace/README.md) · [command](zom-command/README.md) · [desktop](zom-desktop/README.md)。

## 路线图

以 `zom-engine` 的能力域为主轴，逐步把能力稳定接入宿主层与桌面外壳。

近期重点：

- 宿主层 public API 形状打磨。
- 桌面外壳的面板、设置、语言服务入口落地。
- 第一个 release 渠道。

## 开发

```bash
# 构建整个 workspace
cargo build --workspace

# 跑全部测试
cargo test --workspace

# 格式化
cargo fmt

# 启动桌面端（debug）
cargo run -p zom-desktop
```

协作约定见 [`CLAUDE.md`](CLAUDE.md)，项目结构见 [`agents.md`](agents.md)。

## 贡献

欢迎 issue 与 PR。提交前请：

1. 阅读 [`CLAUDE.md`](CLAUDE.md) 与 [`agents.md`](agents.md)。
2. 运行 `cargo fmt` 与 `cargo test --workspace`。
3. 新增 public API 时同步说明长期语义、调用方影响与测试覆盖。
