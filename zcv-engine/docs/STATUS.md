# zcv-engine 当前状态

## 总览

- 主线状态：文本存储、事务、历史、快照、追踪和文件边界已形成稳定主干。
- 当前方向：围绕既有文本内核继续收敛 public API 语义，扩展 fuzz / property
  验证，并改进性能与内存可观测性。
- 边界原则：保持工业级、纯粹的文本编辑引擎底座，只维护宿主无关的文本事实。

## 已收口的大能力

- 文本存储、基础编辑、强类型坐标和错误模型。
- Transaction、Delta、ChangeSet、PositionMap 和原子提交。
- 每订阅者独立累积的组合 TextPatch；Snapshot 作为当前文本真相。
- Undo / Redo、分支历史、历史预算、事务记录和回放。
- Snapshot、BufferVersion、本地读写边界和版本化结果承载。
- Selection / SelectionSet 纯数据与 PositionMap 映射、word / subword / symbol 边界查询。
- 显式 `TransactionOutcome` 与纯文本 HistoryEntry / TransactionRecord。
- Anchor、TrackedRange、VersionedResult、VersionedRangeSet。
- 逻辑文本 viewport slicing。
- 单 Buffer 同步 literal / regex 匹配、单次替换与 replace all。
- 文件文本加载、reload、保存文本边界、大文件和超大事务防御。
- crate 级契约回归、差分验证和粗粒度内存观测入口。

## 边界

本 crate 不承载 UI、产品业务、项目编排、协议集成、任务调度和网络协作。新增能力
只有在直接维护文本存储、坐标、事务、版本、历史、快照、通用追踪或文件文本边界
不变量时，才进入引擎主线。

## 当前结构

```text
src/       编辑引擎实现
tests/     crate 级集成测试
examples/  可选交互式 testbed
docs/      项目文档
```

`src/` 主要能力域：

```text
buffer/       Buffer 文本聚合、事务管线、纯文本历史和生命周期
types/        坐标、范围、版本、身份等强类型
config/       Buffer、编码、换行、显示、大文件等策略
storage/      文本存储抽象和生产 RopeyStorage
transaction/  Edit、Transaction、Delta、ChangeSet、事务记录
selection/    Cursor、Selection、SelectionSet 与纯文本边界词汇
tracking/     Anchor、Mark、TrackedRange
versioned/    版本化结果与泛型外部区间集合
text_changes.rs  连续 Patch 订阅
```

## 验证建议

文档修改通常只需审阅 diff。修改 Rust 代码时，默认运行：

```bash
cargo fmt --all -- --check
cargo test -p zcv-engine
```

需要定向排查时，按触及能力选择对应测试目标；测试组织遵循 workspace 测试策略（见 `../../CODEX.md`）。
