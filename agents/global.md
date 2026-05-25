# global.md

> 本文记录全局 agent 工作约定、验证命令和代码风格。项目独有结构与边界见 `project.md`。

## 1. 语言规范

中文是本项目协作中的第一语言。

默认使用中文的内容包括：

```text
提交信息
代码注释
文档
测试说明
错误提示、日志、诊断信息等面向人的字符串
CLI 输出、UI 文案、解释性字符串
PR / review / issue / changelog 描述
AI 回复与修改说明
```

允许保留英文的内容包括：

```text
Rust 标识符、类型名、trait 名、函数名、字段名、模块名
crate / package / binary 名称
协议名、标准名、错误码、环境变量、命令行参数
第三方 API、上游术语、精确引用
必须与外部生态保持一致的固定文本
```

需要同时照顾可读性和检索性时，优先使用“中文说明 + 英文术语”的形式，例如：

```text
事务基准版本（base_version）不匹配。
```

不要为了中文化而强行翻译代码符号、公共 API 名称或外部协议术语。

## 2. Git 规范

提交信息使用中文，尽量用一句话总结改动，保持简练、具体、可检索。

优先使用一个动宾短语，说明“做了什么”；不要在提交信息里展开背景、过程或多句说明，必要细节放到回复、PR 或 changelog 中。

推荐格式：

```text
接入 zom-engine
补充协作规范
修复事务版本校验
```

避免使用空泛提交信息：

```text
update
fix
wip
调整
```

## 3. 验证命令

通常在仓库根目录验证：

```bash
cargo fmt
cargo test --workspace
```

如果只运行了定向检查，回复时必须明确说明范围。

## 4. 模块组织规范

代码组织遵循 **高内聚、低耦合**。这是结构性规则，AI 在新增 / 重构代码时必须主动遵守。

**高内聚**：一段代码只服务一个消费方时，物理上跟那个消费方放在一起；不要让"被谁用"的关系散落在目录中。

```text
window_controls 只被 top_bar 用 →  shell/components/top_bar/window_controls.rs
                                   而不是 shell/components/window_controls.rs
```

**低耦合**：被多个模块共用的原语单独抽出，让消费方各自依赖原语，而不是互相依赖。

```text
bar_frame / glyph / bar_region 被 top_bar 和 bottom_bar 共用
  →  shell/components/primitives/{bar_frame,glyph,bar_region}.rs
top_bar 不直接 import bottom_bar，反之亦然。
```

判断流程：

1. 新增组件或抽象前先看消费方数量。
2. 单一消费方 —— 内嵌或放进消费方目录。
3. 多个消费方 —— 抽到共享层（`primitives/`、`shared/`、`common/`，按上下文命名）。
4. 不要预建抽象层。先有 ≥ 2 个真实消费方再抽，避免"为未来准备"的死代码。

命名跟随位置语义：在 `shell/` 模块下，文件和类型不再带 `shell_` / `Shell` 前缀，避免冗余（例如 `glyph.rs` 中的 `Glyph` 而不是 `ShellGlyph`）。

## 5. Warning 规范

不要用 `#[allow(...)]`、`#![allow(...)]` 或类似方式隐藏 warning。

warning 是协作信号，应当暴露出来。能当场解决就修掉；暂时不能解决时，保留 warning，并在回复、issue、TODO 或相关文档中说明原因、影响范围和后续处理方向。

确需保留例外时，必须先有明确的项目级约定，并在代码旁用中文说明为什么这个 lint 不适用于当前场景。
