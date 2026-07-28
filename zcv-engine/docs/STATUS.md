# zcv-engine 当前状态

## 总览

- 主线状态：M0-M19 编辑引擎主线已收口。
- 当前方向：已完成统一 Editor 的阶段 0 前置拆分；后续工作以 Editor 接入所需的增量修复、fuzz / property 扩展、可观测性改进和 API 语义收敛为主。
- 边界原则：继续保持工业级、纯粹编辑引擎底座；不把宿主层能力塞回 engine core。

## 已收口的大能力

- 文本存储、基础编辑、强类型坐标和错误模型。
- Transaction、Delta、ChangeSet、PositionMap 和原子提交。
- Undo / Redo、分支历史、历史预算、事务记录和回放。
- Snapshot、BufferVersion、本地读写边界和版本化结果承载。
- Selection / SelectionSet 纯数据与 PositionMap 映射、word / subword / symbol 边界查询。
- 显式 `TransactionOutcome` 与纯文本 HistoryEntry / TransactionRecord。
- engine 已移除 IME composition 类型、状态机和专用事务来源；输入法提交前的 marked text 完全留在宿主层。
- Anchor、TrackedRange、MetadataLayer、VersionedRangeSet。
- 逻辑文本 viewport slicing；Tab/display-column、FoldSet、合并隐藏区间和 Projection 已迁移到宿主 Editor DisplayMap。
- 单 Buffer 同步 literal / regex 匹配与 replace all；异步调度、取消和进度不进入 engine。
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
buffer/       Buffer 文本聚合、显式选区编辑入口、事务管线、纯文本历史和生命周期
types/        坐标、范围、版本、身份等强类型
config/       Buffer、编码、换行、显示、大文件等策略
storage/      文本存储抽象和生产 RopeyStorage
transaction/  Edit、Transaction、Delta、ChangeSet、事务记录
selection/    Cursor、Selection、SelectionSet 与纯文本边界词汇
tracking/     Anchor、Mark、TrackedRange
metadata/     泛型外部区间承载
```

## 验证建议

文档修改通常只需审阅 diff。修改 Rust 代码时，默认运行：

```bash
cargo fmt
cargo test
```

需要定向排查时，按触及能力选择对应测试目标；测试组织遵循 workspace 测试策略（见 `../../agents.md`）。
