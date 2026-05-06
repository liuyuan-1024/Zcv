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

## 当前测试文件（快照）

```text
tests/
- m0_domain_model.rs
- m1_buffer.rs
- m2_transaction.rs
- m3_history.rs
- m4_storage.rs
- m5a_coordinates.rs
- m5b_coordinates.rs
- m6a_selection.rs
- m6b_word_movement.rs
- m6c_composition.rs
- m7_buffer_lifecycle.rs
- m7_file_boundary.rs
```

## 当前示例文件（快照）

```text
examples/
- gpui_m1_testbed.rs
- gpui_m2_testbed.rs
- gpui_m3_testbed.rs
- gpui_m4_testbed.rs
- gpui_m5a_testbed.rs
- gpui_m5b_testbed.rs
- gpui_m6a_testbed.rs
- gpui_m6b_testbed.rs
- gpui_m6c_testbed.rs
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
