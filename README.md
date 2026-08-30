# Zcv

Zcv 是一个以本地编辑体验为核心的代码编辑器，使用 Rust 与 GPUI 构建。

项目处于快速演进阶段，优先建立清晰的模块边界、状态所有权和单一数据源，不为错误的历史设计保留兼容层。

## 当前范围

- 本地项目、文本编辑、文件搜索、Git 变更、终端与预览。
- 语法能力基于 Tree-sitter 与 `.scm` 查询文件。
- 当前不引入实时协作、远程开发或 LSP 基础设施。
- macOS 自动更新当前只支持 Apple Silicon。

## 架构入口

工作区由多个职责明确的 crate 组成。修改某个领域前，优先阅读对应 crate 的 README 与公共入口：

- [`zcv-editor`](zcv-editor/README.md)：编辑状态、显示投影与输入交互。
- [`zcv-ui`](zcv-ui/README.md)：设计系统与可复用基础组件。
- [`zcv-workspace`](zcv-workspace/README.md)：`Item`、`Pane`、`Dock` 与工作区装配。
- [`zcv-update`](zcv-update/README.md)：更新清单、产物校验与跨进程替换事务。

其他核心边界：

- `zcv-text`：文本内容与编辑事务。
- `zcv-language`：Tree-sitter 语言配置与语法状态。
- `zcv-multi-buffer`：单个或多个缓冲区的组合视图。
- `zcv-project`：本地项目及其文件状态。
- `zcv`：应用入口与产品级装配，不承载可下沉的领域逻辑。

依赖版本统一在根 [`Cargo.toml`](Cargo.toml) 的 `workspace.dependencies` 中声明；子 crate 通过 `workspace = true` 引用。

## 项目级规范

`docs/` 只保留跨 crate 且需要统一遵守的规则：

- [交互架构](docs/交互架构.md)
- [重导出规范](docs/重导出规范.md)
- [数据目录规范](docs/数据目录规范.md)
- [代码组织规范](docs/代码组织规范.md)
- [架构决策记录](docs/架构决策记录.md)

协作与修改原则以 [`AGENTS.md`](AGENTS.md) 为准。

## 开发与验证

```bash
cargo check -p zcv
cargo test -p <受影响的-crate> <相关测试过滤条件>
```

验证应与修改范围匹配。优先运行最小且能直接证明目标行为的测试，不默认运行整个工作区的全量测试。
