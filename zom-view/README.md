# zom-view

`zom-view` 是 zom 宿主层的编辑面状态层 crate，持有「我怎么看一个 buffer」的视图状态。

## 定位

`zom-view` 回答「我怎么看它」：看哪个 buffer、滚到哪、本视图的光标与折叠。

它独立于 `zom-workspace`，因为这些状态**会随视图多重**：同一个 buffer 可被多个 view（分屏）观察，每个 view 有独立的光标、折叠和滚动。把它独立成 crate 让 viewport 数学、fold 状态转移、光标移动可以无头测试，不必透过 GPUI 外壳。

`SelectionSet` / `FoldSet` 的*实例*归 view —— `zom-engine` 只提供类型和移动 / after-edit 算法，实例由宿主按视图持有。

它不渲染像素（那是 `zom-desktop`），不拥有 `Buffer`（那是 `zom-workspace`）。

判据：同一文件开两个分屏*会*不同的状态归这里（光标、fold、滚动）；属于文件本身的归 `zom-workspace`。

## 核心类型

- `View` —— 对某个 buffer 的一次观察：`BufferId` 引用 + `SelectionSet` + `FoldSet` + `ViewportState`。
- `ViewSet` —— 全部 view 的集合，并记录当前活动 view。
- `ViewId` —— view 的标识。
- `ViewportState` —— 滚动位置 / 可见区域。

`FoldSet` 必须版本绑定，因此 `View::new` 构造时必须提供被观察 buffer 的 `BufferVersion`。

## 依赖

```text
zom-view → zom-engine
zom-view → zom-workspace
```

依赖 `zom-engine`（`SelectionSet` / `FoldSet` / `BufferVersion`）和 `zom-workspace`（`BufferId`）。

## 结构概览

```text
src/lib.rs    View / ViewSet / ViewId / ViewportState
```

骨架阶段为单文件 `lib.rs`。

## 相关文档

- `../AGENTS.md`：workspace 全局协作规则。
- `../TODO.md`：宿主层开发规划，本 crate 对应能力域 2 / 5 / 7（坐标读取、选区、折叠投影），阶段 P2 / P3。

## 状态

骨架阶段：类型形状已定，viewport 数学、光标移动接入、fold / projection 接入留待 `TODO.md` P2 / P3。
