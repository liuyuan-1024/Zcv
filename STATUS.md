# STATUS

> 这是项目“易变层”文档，允许频繁更新。主文档（AGENTS/能力/测试策略）只维护稳定规则。

## 更新时间

- 2026-05-06

## 当前阶段快照

- M7A Buffer 身份与状态已完成。
- 已新增 `BufferId` / `BufferKind` / `BufferState` public API。
- Buffer 现在可表达 Untitled / File / URI / Scratch 来源，并支持只读防线与关闭前 dirty 查询。
- M7B 保存点与 dirty state 已完成。
- 已新增 `last_saved_version` / `last_synced_external_version` 查询，以及 `mark_synced_external()`。
- dirty state 表示当前文本内容是否相对保存点有未保存变更；`saved_version` 仅记录最后保存时的版本。
- M7C 文件文本进入 Buffer 已完成。
- 已新增 `Buffer::from_loaded_text(...)`、`LoadedTextInfo`、UTF-8 BOM / 非法 UTF-8 加载策略与换行元信息。
- M7D reload 与保存边界已完成。
- 已新增 `reload_from_text(...)` / `reload_from_snapshot(...)` / `to_save_text(...)`，reload 会重建文本状态并清空编辑历史。
- M8A PositionMap 强类型已完成。
- 已新增 `PositionMap` / `MappingResult` public API，并支持 old/new char position 与 range 双向映射、删除 / 塌缩 / 歧义结果，以及与 `ChangeSet` 互操作。
- `ChangeSet` 不再保留 `map_position(...)` / `map_range(...)` 兼容映射入口，位置映射统一由 `PositionMap` 表达。
- M8B Affinity / Bias / Stickiness 基础语义已完成。
- 已新增同点插入吸附、反向歧义偏向和区间边界 stickiness 策略，服务后续 Anchor / TrackedRange / Selection 映射。
- M8C DeltaEvent 与事件队列已完成。
- 已新增 `DeltaEvent` public API、`buffer.take_pending_events()` 与 `buffer.last_delta_event()`；成功事务、Undo / Redo 会按版本顺序入队，失败事务不产生事件。

## 当前测试文件（快照）

```text
tests/
- m0_domain_model.rs
- m1_buffer.rs
- m2_transaction.rs
- m3_history.rs
- m4_storage.rs
- m5_coordinates.rs
- m6_selection.rs
- m7_buffer_lifecycle.rs
- m8_position_map.rs
```

## 当前示例文件（快照）

```text
examples/
- gpui_m1_testbed.rs
- gpui_m2_testbed.rs
- gpui_m3_testbed.rs
- gpui_m4_testbed.rs
- gpui_m5_testbed.rs
- gpui_m6_testbed.rs
- gpui_m7_testbed.rs
```

## 常用验证命令（快照）

```bash
cargo fmt
cargo test
```

按改动范围追加：

```bash
cargo test --test <target>
cargo run --example <target>
```
