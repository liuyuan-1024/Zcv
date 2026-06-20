# zom-workspace

`zom-workspace` 是 zom 宿主层的文档 / 模型层 crate，拥有 `Buffer` 实例并管理缓冲区与文件的生命周期。

## 定位

`zom-workspace` 回答「有什么」：当前打开了哪些缓冲区、它们绑定到哪些文件、磁盘状态如何。

它拥有 `zom_engine::Buffer` 实例本身，负责文件路径、来源、脏状态、只读、保存点等属于文件本身的状态。

它不负责视图状态（滚动、光标、折叠）—— 那些「同一文件开两个分屏会不同」的状态属于 `view` 模块。它也不做文件监听、冲突弹窗、保存 UI、项目索引或跨文件搜索。

判据：同一文件开两个分屏不会不同的状态归这里；会不同的归 `view` 模块。

## 核心类型

- `Workspace` —— 当前打开的全部缓冲区的拥有者。
- `WorkspaceBuffer` —— 一个被持有的缓冲区，连同它的文件边界状态。
- `BufferId` —— workspace 自己的缓冲区标识（`zom_engine::BufferId` 已降为 `pub(crate)`，下游唯一入口）。
- `BufferOrigin` —— 缓冲区来源：绑定文件或未命名草稿。
- `WorkspaceError` / `WorkspaceResult` —— workspace 文件生命周期错误边界。

文件生命周期 API：`open_file` / `open_text` / `save_file` / `save_as` / `close_buffer`。

活动缓冲区模型：打开新缓冲区后自动成为活动项；关闭非活动缓冲区不改变活动项；关闭活动缓冲区后切换到仍打开缓冲区中最近分配的一个；关闭最后一个缓冲区后活动项为空。

状态查询 API：`active_buffer_id` / `set_active_buffer` / `active_buffer` / `buffer_path` / `is_buffer_dirty` / `is_buffer_read_only`。

## 依赖

```text
zom-workspace → zom-engine
```

只依赖 `zom-engine`。

## 目录概览

```text
src/lib.rs                Workspace / WorkspaceBuffer / BufferId / BufferOrigin
src/buffer_search.rs      单缓冲区搜索状态
src/project_tree.rs       项目文件树数据源
src/syntax/               语法高亮识别、调度、后台 worker 与 provider
tests/                    workspace 生命周期契约测试
```

核心类型仍由 `src/lib.rs` 对外汇总；搜索、文件树和语法高亮按能力域拆分到独立模块。

## 相关文档

- `../AGENTS_GLOBAL.md`、`../AGENTS_PROJECT.md`：workspace 全局规则与项目规则。

## 文档维护

本 README 只维护稳定边界、核心类型、依赖关系与目录概览。
