# zcv-engine 当前状态

## 总览

- 主线状态：M0-M19 编辑引擎主线已收口。
- 当前方向：后续工作以增量修复、fuzz / property 扩展、可观测性改进和 API 语义收敛为主。
- 边界原则：继续保持工业级、纯粹编辑引擎底座；不把宿主层能力塞回 engine core。

## 已收口的大能力

- 文本存储、基础编辑、强类型坐标和错误模型。
- Transaction、Delta、ChangeSet、PositionMap 和原子提交。
- Undo / Redo、分支历史、历史预算、事务记录和回放。
- Snapshot、BufferVersion、本地读写边界和版本化结果承载。
- SelectionSet、多光标、word / subword / symbol movement、IME composition。
- Anchor、TrackedRange、MetadataLayer、VersionedRangeSet。
- Fold、Projection、ProjectedViewport 和读取切片。
- 单 Buffer literal / regex 搜索与 replace all。
- 文件文本加载、reload、保存文本边界、大文件和超大事务防御。
- property 回归和粗粒度内存观测入口。

## 明确不进入主线

- UI 渲染和正式编辑器界面。
- LSP / Tree-sitter provider。
- diagnostics、semantic tokens、inlay hints、code lens 生成。
- 项目索引、文件树、插件系统、跨文件搜索。
- 快捷键、菜单、命令面板、Command 语义层。
- 宏录制、用户操作回放。
- 后台任务调度器、取消令牌、线程池。
- 实时多人协作、CRDT / OT 和网络同步。

## 当前结构

```text
src/       编辑引擎实现
tests/     crate 级集成测试
examples/  可选交互式 testbed
docs/      项目文档
```

`src/` 主要能力域：

```text
buffer/       Buffer 聚合、编辑入口、事务管线、历史和生命周期
types/        坐标、范围、版本、身份等强类型
config/       Buffer、编码、换行、显示、大文件等策略
storage/      文本存储抽象和生产 RopeyStorage
transaction/  Edit、Transaction、Delta、ChangeSet、事务记录
selection/    Cursor、Selection、SelectionSet、移动和组合输入
tracking/     Anchor、Mark、TrackedRange
metadata/     泛型外部区间承载
projection/   Fold、投影坐标和 viewport
```

## 验证建议

文档修改通常只需审阅 diff。修改 Rust 代码时，默认运行：

```bash
cargo fmt
cargo test
```

需要定向排查时，按触及能力选择对应测试目标；测试组织遵循 workspace 测试策略（见 `../../agents.md`）。
