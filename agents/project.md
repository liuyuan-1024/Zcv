# project.md

> 本文记录 `zom` workspace 的项目独有结构与边界。全局协作、验证命令和代码风格见 `global.md`。

## 1. 项目结构

`zom` 是 Cargo workspace 根项目，用于统一管理多个相互黑盒隔离的 crate：

```text
zom-engine      核心文本编辑引擎
zom-workspace   工作区、文件、buffer 编排
zom-view        编辑面状态：view、滚动、selection、fold
zom-command     命令系统
zom-ai          AI 抽象与集成
zom-desktop     桌面入口，组合其他 crate
```

crate 之间只能通过 public API 连接。不要跨 crate 依赖私有实现、源码路径或测试专用细节。

## 2. 工作区规则

根目录只保留一个 Git 仓库和一个 workspace 级 `Cargo.lock`。

不要在子 crate 中重新初始化 Git 仓库。需要保留外部历史时，使用 subtree 或其他明确的历史迁移方式。
