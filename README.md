# Zcv

Zcv 是一个以本地编辑体验为核心的代码编辑器，使用 Rust 与 GPUI 构建。

在 macOS 上关闭最后一个窗口后，应用仍会运行；再次点击 Dock 中的图标会重新打开最近项目，若没有有效的最近项目则打开空工作区。使用“退出”命令才会结束应用进程。

macOS 打包版可在 Finder 中作为本地文本文件的打开方式。文件会进入当前工作区；若所有窗口已关闭，则先打开空工作区。

Windows 版启动时自动把当前 Zcv.exe 注册为本地文件的候选打开程序，无需运行任何脚本；之后可在 Windows“默认应用”设置中按扩展名选择 Zcv。注册的扩展名与 macOS 打包版共用 [`assets/document-extensions.txt`](assets/document-extensions.txt)；macOS 由系统的 LaunchServices 自动完成同等注册，还通过 `public.text` 内容类型支持其他文本文件。更换可执行文件位置后，直接启动新位置的 Zcv.exe 一次即可把注册指向新位置；取消注册需删除 `HKCU` 下的 `Software\Classes\Applications\Zcv.exe`、`Software\Classes\Zcv.Document`、`Software\Zcv`、各扩展名 `OpenWithProgids` 中的 `Zcv.Document` 值和 `Software\RegisteredApplications` 中的 `Zcv` 值。文件路径由启动参数传入；Windows 已有 Zcv 进程时，后续启动会把打开请求交给现有进程，并在当前工作区窗口打开文件。目录参数仍作为项目根目录打开。

项目处于快速演进阶段，编辑器核心以 Zed 的架构为目标，优先建立清晰的模块边界、状态所有权和单一数据源，不为错误的历史设计保留兼容层。

## 当前范围

- 本地项目、文本编辑、文件搜索、Git 变更、终端与预览。
- 语法能力基于 Tree-sitter 与 `.scm` 查询文件。
- 当前不引入实时协作、远程开发或 LSP 基础设施。
- 打包版自动更新支持 macOS Apple Silicon 与 Windows x86_64，更新清单和产物分别经过签名与完整性校验。

自动更新要求完整的应用安装目录与更新辅助程序；通过 Cargo 直接运行源码时不启用自动更新。打包、发布与更新协议见 [`zcv-update`](zcv-update/README.md)。

## 开发与运行

### 构建环境

- Rust stable 工具链与 Cargo；工作区采用 Rust 2024 edition。
- Git 命令行工具；应用的 Git 功能通过外部 Git 命令执行。
- macOS：准备 Xcode 与 Metal Toolchain，确保 `xcrun metal -v` 可执行。
- Windows：准备 MSVC 编译工具与 Windows SDK，使用 `x86_64-pc-windows-msvc` 工具链。

平台构建与打包配置见 [发布流程](.github/workflows/release.yml)、[macOS 打包脚本](scripts/bundle-mac) 与 [Windows 打包脚本](scripts/bundle-windows.ps1)。

应用图标以 [`assets/branding/Zcv.svg`](assets/branding/Zcv.svg) 为原稿。修改原稿后，在 macOS 上运行 `scripts/generate-app-icons`，重新生成提交到仓库的 `Zcv.icns` 和 `Zcv.ico`。该脚本需要 `rsvg-convert`、`iconutil` 和 `python3`。macOS 打包时将 `Zcv.icns` 放入 `.app`；Windows 构建时将 `Zcv.ico` 嵌入 `Zcv.exe` 的图标资源 ID 1，打包脚本直接收集该 exe。

### 启动应用

以下命令在项目根目录执行，由 [Cargo 配置](.cargo/config.toml) 定义。

启动开发构建（dev 配置，默认产物目录为 `target/debug`）：

```bash
cargo rd
```

等价于 `cargo run -p Zcv`。Cargo 默认使用 dev 配置，无需 `--debug` 参数；项目已为开发构建配置部分优化。

启动发布构建（release 配置）：

```bash
cargo rr
```

等价于 `cargo run --release -p Zcv`。

## 架构入口

编辑器核心的职责、状态所有权与不变量以 [编辑器架构](docs/编辑器架构.md) 为准，数据流为：

```text
Buffer → LanguageBuffer → MultiBuffer → DisplayMap → Editor → EditorElement
```

普通文件、组合文档、搜索结果与差异视图共用一个 `Editor` 和同一套交互、事务与渲染管线。

修改某个领域前，优先阅读对应模块的 README 与公共入口：

| 模块 | 职责 |
| --- | --- |
| [`zcv-text`](zcv-text/README.md) | 管理权威文本、版本、事务与历史，提供不可变快照。 |
| [`zcv-language`](zcv-language/README.md) | 直接持有文本 `Buffer`，管理 Tree-sitter 解析、语法查询与文本/语法一致快照。 |
| [`zcv-multi-buffer`](zcv-multi-buffer/README.md) | 组织源文档片段，管理组合坐标、源映射与差异投影。 |
| [`zcv-editor`](zcv-editor/README.md) | 显示投影、选择、滚动、输入与事务编排，以及布局、渲染和命中测试。 |
| [`zcv-project`](zcv-project/README.md) | 本地项目、文件文档生命周期、Git 状态与项目级搜索。 |
| [`zcv-workspace`](zcv-workspace/README.md) | `Item`、`Pane`、`Dock`、窗口状态与工作区装配。 |
| [`zcv-ui`](zcv-ui/README.md) | 设计系统与可复用视觉组件。 |
| [`zcv-update`](zcv-update/README.md) | 更新清单、产物校验与跨进程替换事务。 |
| [`zcv`](zcv/src/main.rs) | 应用入口与产品级装配。 |

内部 crate 和需要统一版本的第三方依赖在根 [`Cargo.toml`](Cargo.toml) 的 `workspace.dependencies` 中声明，由使用方通过 `workspace = true` 引用。仅单个 crate 使用且无统一版本需求的第三方依赖可在该 crate 声明。

## 项目级规范

`docs/` 保存跨 crate 且需要统一遵守的架构与规范：

- [交互架构](docs/交互架构.md)
- [重导出规范](docs/重导出规范.md)
- [数据目录规范](docs/数据目录规范.md)
- [代码组织规范](docs/代码组织规范.md)
- [架构决策记录](docs/架构决策记录.md)
- [跨平台能力边界](docs/跨平台能力边界.md)

协作与修改原则以 [`AGENTS.md`](AGENTS.md) 为准。

## 验证

检查应用编译：

```bash
cargo check -p Zcv
```

根据改动选择受影响的 crate 与相关测试过滤条件。例如，验证搜索栏的替换导航：

```bash
cargo test -p zcv-search search_bar::tests --lib
```

仅运行与修改范围匹配的定向测试；未经开发者明确要求，不运行全量测试。

涉及跨平台行为时，还应运行 [平台边界检查](scripts/check-platform-boundaries)，并分别记录平台无关测试、macOS 原生验证和 Windows 原生验证结果。一个平台的通过结果不能替代另一个平台的运行时验证。
