# AGENTS.md

> AI 工具参与 `zom` workspace 前先读本文档。子 crate 可以有自己的 `AGENTS.md`，但不得放宽本文的全局规则。

## 1. 项目结构

`zom` 是 Cargo workspace 根项目，用于统一管理多个相互黑盒隔离的 crate：

```text
zom-engine      核心文本编辑引擎
zom-workspace   工作区、文件、buffer 编排
zom-command     命令系统
zom-ai          AI 抽象与集成
zom-desktop     桌面入口，组合其他 crate
```

crate 之间只能通过 public API 连接。不要跨 crate 依赖私有实现、源码路径或测试专用细节。

## 2. 语言规范

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

## 3. Git 规范

提交信息使用中文，保持简洁、具体、可检索。

推荐格式：

```text
接入 zom-engine 到工作区
补充中文优先协作规范
修复事务版本校验
```

避免使用空泛提交信息：

```text
update
fix
wip
调整
```

## 4. 工作区规则

根目录只保留一个 Git 仓库和一个 workspace 级 `Cargo.lock`。

不要在子 crate 中重新初始化 Git 仓库。需要保留外部历史时，使用 subtree 或其他明确的历史迁移方式。

通常在根目录验证：

```bash
cargo fmt
cargo test --workspace
```

如果只运行了定向检查，回复时必须明确说明范围。
