# zom-workspace

`zom-workspace` 是 zom 宿主层的文档/模型层 crate，拥有 `Buffer` 实例并管理 buffer 与文件的生命周期。

## 定位

`zom-workspace` 回答「有什么」：当前打开了哪些 buffer、它们绑定到哪些文件、磁盘状态如何。

它拥有 `zom_engine::Buffer` 实例本身，负责文件路径、origin、dirty、只读、保存点等**属于文件本身**的状态。

它不负责视图状态（滚动、光标、折叠）—— 那些「同一文件开两个分屏会不同」的状态属于 `zom-view`。它也不做文件监听、冲突弹窗、保存 UI、项目索引或跨文件搜索。

判据：同一文件开两个分屏*不会*不同的状态归这里；*会*不同的归 `zom-view`。

## 核心类型

- `Workspace` —— 当前打开的全部 buffer 的拥有者。
- `WorkspaceBuffer` —— 一个被持有的 buffer，连同它的文件边界状态。
- `BufferId` —— workspace 自己的 buffer 标识（与 `zom_engine::BufferId` 区分）。
- `BufferOrigin` —— buffer 来源：绑定文件或未命名 scratch。
- `WorkspaceError` / `WorkspaceResult` —— workspace 文件生命周期错误边界。

文件生命周期 API：`open_file` / `open_text` / `save_file` / `save_as` / `close_buffer`。

活动 buffer 模型：打开新 buffer 后自动成为活动项；关闭非活动 buffer 不改变活动项；关闭活动 buffer 后切换到仍打开 buffer 中最近分配的一个；关闭最后一个 buffer 后活动项为空。

状态查询 API：`active_buffer_id` / `set_active_buffer` / `active_buffer` / `buffer_path` / `is_buffer_dirty` / `is_buffer_read_only`。

## 依赖

```text
zom-workspace → zom-engine
```

只依赖 `zom-engine`。

## 结构概览

```text
src/lib.rs    Workspace / WorkspaceBuffer / BufferId / BufferOrigin
```

当前保持单文件 `lib.rs`；后续按能力域增长时再分模块。

## 相关文档

- `../AGENTS.md`：workspace 全局协作规则。
- `../TODO.md`：宿主层开发规划，本 crate 对应能力域 1（文本存储与文件边界），阶段 P0。

## 状态

P0 已完成：`zom-workspace` 已落地活动 buffer、文件打开/保存/另存/关闭、dirty/path/readonly 状态查询和生命周期测试。
