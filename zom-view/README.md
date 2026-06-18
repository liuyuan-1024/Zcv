# zom-view

`zom-view` 是 zom 宿主层的编辑面状态层 crate，持有「我怎么看一个缓冲区（buffer）」的视图状态。

## 定位

`zom-view` 回答「我怎么看它」：看哪个缓冲区、滚到哪、本视图的光标与折叠。

它独立于 `zom-workspace`，因为这些状态会随视图多重：同一个缓冲区可被多个视图（分屏）观察，每个视图有独立的光标、折叠和滚动。把它独立成 crate 让视口数学、折叠状态转移、光标移动可以无头测试，不必透过 GPUI 外壳。

`SelectionSet` / `FoldSet` 的实例归视图层持有；`zom-engine` 提供类型、光标移动与编辑后状态转移算法。

它不渲染像素（那是 `zom-desktop`），不拥有 `Buffer`（那是 `zom-workspace`）。

判据：同一文件开两个分屏会不同的状态归这里（光标、折叠、滚动）；属于文件本身的归 `zom-workspace`。

## 核心类型

- `View` —— 对某个缓冲区的一次观察：`BufferId` 引用 + `SelectionSet` + `FoldSet` + `ViewportState`。
- `ViewSet` —— 全部视图的集合，并记录当前活动视图。
- `ViewId` —— 视图标识。
- `ViewportState` —— 滚动位置 / 可见区域。

`FoldSet` 必须版本绑定，因此 `View::new` 构造时必须提供被观察缓冲区的 `BufferVersion`。

## 依赖

```text
zom-view → zom-engine
zom-view → zom-workspace
```

依赖 `zom-engine`（`SelectionSet` / `FoldSet` / `BufferVersion`）和 `zom-workspace`（`BufferId`）。

## 目录概览

```text
src/lib.rs    View / ViewSet / ViewId / ViewportState
```

目前为单文件 crate，公共 API 集中在 `src/lib.rs`。

## 相关文档

- `../AGENTS_GLOBAL.md`、`../AGENTS_PROJECT.md`：workspace 全局规则与项目规则。

## 文档维护

本 README 只维护稳定边界、核心类型和依赖关系。
