//! M4 GPUI testbed：在 M3 历史体验之上展示 RopeyStorage、快照和存储指标。
//!
//! 本文件用于观察生产存储后端接入后的 UI 手感与指标变化，不承担 Rope 语义差分或性能结论的最终判定。

use gpui::{
    App, Application, Bounds, Context, Div, FocusHandle, IntoElement, KeyBinding, KeyDownEvent,
    Render, Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, rgb,
};

use zom_engine::{
    Buffer, BufferConfig, BufferVersion, CharOffset, Edit, EditList, EngineResult, Line,
    LogicalColumn, Position, SelectionSet, Snapshot, TextRange, Transaction,
    TransactionMergePolicy, TransactionMetadata, TransactionSource,
};

// M4 testbed：必须是 M3 testbed 的 superset。
// 保留 M3 的输入 / Delete / Home / End / Save / Reset / 批量事务 / Delta / ChangeSet / Undo / Redo / Snapshot 可视化能力，
// 再叠加 RopeyStorage 后端、bytes / chars / UTF-16 / lines metrics 与长行插入 smoke。
actions!(
    m4_testbed,
    [
        Backspace,
        DeleteForward,
        Enter,
        Left,
        Right,
        Home,
        End,
        Save,
        Reset,
        BatchEdit,
        Undo,
        Redo,
        MergeDemo,
        CaptureSnapshot,
        InsertLongLine
    ]
);

const INITIAL_TEXT: &str = "🚀 Zom Engine M4 中文测试台

[A] 这是锚点A，[B] 这是锚点B。
可以输入、回车、退格、Delete、左右移动光标。
Home / End 可跳到当前行首尾。
Cmd-S 标记已保存，Cmd-R 重置文本。
Cmd-B 触发批量事务修改，连续字符输入会合并成一个撤销步骤，Cmd-M 触发一次合并到上一步的事务。
Cmd-Z / Ctrl-Z 撤销，Cmd-Shift-Z / Ctrl-Y 重做。
Cmd-K 捕获当前快照，并观察后续编辑后快照是否过期。
Cmd-L 在当前光标插入一段中等长度的单行文本，用于快速验证 RopeyStorage 的局部插入与指标更新。
M4 生产后端是 RopeyStorage；字符串参考模型只应存在于测试文件中。
所有写入都走事务；黄色区间是变更范围，A/B 会通过 PositionMap 跟随文本变化。
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeGroup {
    Typing,
    Backspace,
    DeleteForward,
}

pub struct M4Testbed {
    buffer: Buffer,
    cursor: CharOffset,
    anchor_a: CharOffset,
    anchor_b: CharOffset,
    last_changed_ranges: Vec<TextRange>,
    last_delta: Option<(BufferVersion, BufferVersion, usize)>,
    last_history_event: Option<String>,
    pinned_snapshot: Option<Snapshot>,
    merge_group: Option<MergeGroup>,
    focus_handle: FocusHandle,
    last_error: Option<String>,
}

impl M4Testbed {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (buffer, anchor_a, anchor_b) = initial_buffer_and_anchors();

        Self {
            buffer,
            cursor: CharOffset::ZERO,
            anchor_a,
            anchor_b,
            last_changed_ranges: Vec::new(),
            last_delta: None,
            last_history_event: None,
            pinned_snapshot: None,
            merge_group: None,
            focus_handle: cx.focus_handle(),
            last_error: None,
        }
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        let (buffer, anchor_a, anchor_b) = initial_buffer_and_anchors();

        self.buffer = buffer;
        self.cursor = CharOffset::ZERO;
        self.anchor_a = anchor_a;
        self.anchor_b = anchor_b;
        self.last_changed_ranges.clear();
        self.last_delta = None;
        self.last_history_event = Some("重置".to_string());
        self.pinned_snapshot = None;
        self.merge_group = None;
        self.last_error = None;

        cx.notify();
    }

    fn mark_saved(&mut self, cx: &mut Context<Self>) {
        self.buffer.mark_saved();
        self.last_history_event = Some("标记已保存".to_string());
        self.merge_group = None;
        self.last_error = None;
        cx.notify();
    }

    fn capture_snapshot(&mut self, cx: &mut Context<Self>) {
        self.pinned_snapshot = Some(self.buffer.snapshot());
        self.last_history_event = Some("捕获快照".to_string());
        self.merge_group = None;
        self.last_error = None;
        cx.notify();
    }

    fn submit_edits(
        &mut self,
        edits: Vec<Edit>,
        metadata: TransactionMetadata,
        cx: &mut Context<Self>,
    ) -> bool {
        let edit_list = match EditList::new(edits) {
            Ok(edit_list) => edit_list,
            Err(err) => {
                self.last_error = Some(err.to_string());
                cx.notify();
                return false;
            }
        };
        let before_selection = SelectionSet::caret(self.cursor);

        let after_cursor = map_position_after_edits(self.cursor, edit_list.as_slice());
        let after_selection = SelectionSet::caret(after_cursor);

        let tx = match Transaction::new(self.buffer.version(), edit_list) {
            Ok(tx) => tx
                .with_metadata(metadata)
                .with_selection(Some(before_selection), Some(after_selection)),
            Err(err) => {
                self.last_error = Some(err.to_string());
                cx.notify();
                return false;
            }
        };

        self.apply_transaction(tx, cx)
    }

    fn apply_transaction(&mut self, tx: Transaction, cx: &mut Context<Self>) -> bool {
        let applied = match self.buffer.apply_transaction(tx) {
            Ok((delta, changeset)) => {
                self.last_error = None;
                self.last_delta = Some((
                    delta.old_version,
                    delta.new_version,
                    delta.edits.as_slice().len(),
                ));

                // M2 核心体感：游标和锚点都通过 PositionMap 跟随文本变化。
                // M4 继承 M3：如果事务携带了 selection snapshot，则优先用历史系统里的 caret 恢复光标。
                let mapped_cursor = changeset
                    .position_map()
                    .map_old_position(self.cursor)
                    .value();
                self.anchor_a = changeset
                    .position_map()
                    .map_old_position(self.anchor_a)
                    .value();
                self.anchor_b = changeset
                    .position_map()
                    .map_old_position(self.anchor_b)
                    .value();
                self.last_changed_ranges = changeset.changed_ranges();

                if let Some(cursor) = self.cursor_from_engine_selection() {
                    self.cursor = cursor;
                } else {
                    self.cursor = mapped_cursor;
                }

                self.last_history_event = Some("应用事务".to_string());
                true
            }
            Err(err) => {
                self.last_error = Some(err.to_string());
                false
            }
        };

        cx.notify();
        applied
    }

    fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        match Edit::insert(self.cursor, text.to_string()) {
            Ok(edit) => {
                let metadata = self
                    .metadata_for_group(MergeGroup::Typing, TransactionSource::Keyboard)
                    .with_description(format!("插入 {text:?}"));

                if self.submit_edits(vec![edit], metadata, cx) {
                    self.merge_group = Some(MergeGroup::Typing);
                }
            }
            Err(err) => {
                self.merge_group = None;
                self.last_error = Some(err.to_string());
                cx.notify();
            }
        }
    }

    fn backspace(&mut self, cx: &mut Context<Self>) {
        let Some(prev) = previous_edit_boundary(self.buffer.text().as_ref(), self.cursor) else {
            self.merge_group = None;
            return;
        };

        self.delete_range_with_group(prev, self.cursor, MergeGroup::Backspace, cx);
    }

    fn delete_forward(&mut self, cx: &mut Context<Self>) {
        let Some(next) = next_edit_boundary(self.buffer.text().as_ref(), self.cursor) else {
            self.merge_group = None;
            return;
        };

        self.delete_range_with_group(self.cursor, next, MergeGroup::DeleteForward, cx);
    }

    fn delete_range_with_group(
        &mut self,
        start: CharOffset,
        end: CharOffset,
        group: MergeGroup,
        cx: &mut Context<Self>,
    ) {
        match TextRange::new(start, end) {
            Ok(range) => {
                let metadata = self
                    .metadata_for_group(group, TransactionSource::Delete)
                    .with_description("删除");

                if self.submit_edits(vec![Edit::delete(range)], metadata, cx) {
                    self.merge_group = Some(group);
                }
            }
            Err(err) => {
                self.merge_group = None;
                self.last_error = Some(err.to_string());
                cx.notify();
            }
        }
    }

    fn batch_edit(&mut self, cx: &mut Context<Self>) {
        let marker = Edit::insert(CharOffset::ZERO, "[批量标记] ".to_string()).unwrap();

        // 避免在 cursor=0 时制造两个同 offset 的插入，让 testbed 的视觉结果更可预期。
        let sparkle_offset = if self.cursor == CharOffset::ZERO {
            self.buffer.len_chars()
        } else {
            self.cursor
        };
        let sparkle = Edit::insert(sparkle_offset, "✨".to_string()).unwrap();

        self.merge_group = None;
        self.submit_edits(
            vec![marker, sparkle],
            TransactionMetadata::new(TransactionSource::Command).with_description("批量事务"),
            cx,
        );
    }

    fn insert_long_line(&mut self, cx: &mut Context<Self>) {
        let payload = format!("[M4 Ropey 长行探针] {}\n", "中🙂Ropey".repeat(128));

        match Edit::insert(self.cursor, payload) {
            Ok(edit) => {
                self.merge_group = None;
                self.submit_edits(
                    vec![edit],
                    TransactionMetadata::new(TransactionSource::Command)
                        .with_description("M4 插入长行探针"),
                    cx,
                );
            }
            Err(err) => {
                self.merge_group = None;
                self.last_error = Some(err.to_string());
                cx.notify();
            }
        }
    }

    fn merge_demo(&mut self, cx: &mut Context<Self>) {
        match Edit::insert(self.cursor, "⌁".to_string()) {
            Ok(edit) => {
                self.merge_group = None;
                self.submit_edits(
                    vec![edit],
                    TransactionMetadata::new(TransactionSource::Keyboard)
                        .with_merge_policy(TransactionMergePolicy::MergeWithPrevious)
                        .with_description("合并演示"),
                    cx,
                );
            }
            Err(err) => {
                self.last_error = Some(err.to_string());
                cx.notify();
            }
        }
    }

    fn metadata_for_group(
        &self,
        group: MergeGroup,
        source: TransactionSource,
    ) -> TransactionMetadata {
        let metadata = TransactionMetadata::new(source);

        if self.merge_group == Some(group) {
            metadata.with_merge_policy(TransactionMergePolicy::MergeWithPrevious)
        } else {
            metadata
        }
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        self.merge_group = None;
        let result = self.buffer.undo();
        self.apply_history_result(result, "撤销", cx);
    }

    fn redo(&mut self, cx: &mut Context<Self>) {
        self.merge_group = None;
        let result = self.buffer.redo();
        self.apply_history_result(result, "重做", cx);
    }

    fn apply_history_result(
        &mut self,
        result: EngineResult<Option<(zom_engine::Delta, zom_engine::ChangeSet)>>,
        label: &str,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(Some((delta, changeset))) => {
                self.last_error = None;
                self.last_delta = Some((
                    delta.old_version,
                    delta.new_version,
                    delta.edits.as_slice().len(),
                ));

                let mapped_cursor = changeset
                    .position_map()
                    .map_old_position(self.cursor)
                    .value();
                self.anchor_a = changeset
                    .position_map()
                    .map_old_position(self.anchor_a)
                    .value();
                self.anchor_b = changeset
                    .position_map()
                    .map_old_position(self.anchor_b)
                    .value();
                self.last_changed_ranges = changeset.changed_ranges();

                if let Some(cursor) = self.cursor_from_engine_selection() {
                    self.cursor = cursor;
                } else {
                    self.cursor = mapped_cursor;
                }

                self.last_history_event = Some(label.to_string());
            }
            Ok(None) => {
                self.last_error = None;
                self.last_history_event = Some(format!("{label}: 历史为空"));
            }
            Err(err) => {
                self.last_error = Some(err.to_string());
            }
        }

        cx.notify();
    }

    fn move_left(&mut self, cx: &mut Context<Self>) {
        self.merge_group = None;
        if let Some(prev) = previous_edit_boundary(self.buffer.text().as_ref(), self.cursor) {
            self.cursor = prev;
            self.sync_engine_selection_to_cursor();
            self.last_error = None;
            cx.notify();
        }
    }

    fn move_right(&mut self, cx: &mut Context<Self>) {
        self.merge_group = None;
        if let Some(next) = next_edit_boundary(self.buffer.text().as_ref(), self.cursor) {
            self.cursor = next;
            self.sync_engine_selection_to_cursor();
            self.last_error = None;
            cx.notify();
        }
    }

    fn move_to_line_start(&mut self, cx: &mut Context<Self>) {
        self.merge_group = None;
        let line = self.cursor_position().line();

        match self.buffer.line_start(line) {
            Ok(offset) => {
                self.cursor = offset;
                self.sync_engine_selection_to_cursor();
                self.last_error = None;
            }
            Err(err) => self.last_error = Some(err.to_string()),
        }

        cx.notify();
    }

    fn move_to_line_end(&mut self, cx: &mut Context<Self>) {
        self.merge_group = None;
        let line = self.cursor_position().line();

        match self.line_content_end(line) {
            Ok(offset) => {
                self.cursor = offset;
                self.sync_engine_selection_to_cursor();
                self.last_error = None;
            }
            Err(err) => self.last_error = Some(err.to_string()),
        }

        cx.notify();
    }

    fn line_content_end(&self, line: Line) -> EngineResult<CharOffset> {
        let line_start = self.buffer.line_start(line)?.get();
        let next_line_start = if line.get() + 1 < self.buffer.line_count() {
            self.buffer.line_start(Line::new(line.get() + 1))?.get()
        } else {
            self.buffer.len_chars().get()
        };

        Ok(CharOffset::new(line_content_end(
            self.buffer.text().as_ref(),
            line_start,
            next_line_start,
        )))
    }

    fn cursor_position(&self) -> Position {
        self.buffer
            .char_to_position(self.cursor)
            .unwrap_or_else(|_| Position::new(Line::ZERO, LogicalColumn::ZERO))
    }

    fn sync_engine_selection_to_cursor(&mut self) {
        if let Err(err) = self.buffer.set_selection(SelectionSet::caret(self.cursor)) {
            self.last_error = Some(err.to_string());
        }
    }

    fn cursor_from_engine_selection(&self) -> Option<CharOffset> {
        Some(self.buffer.selection().primary().head())
    }
}

impl Render for M4Testbed {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cursor = self.cursor;
        let position = self.cursor_position();
        let history = self.buffer.history_status();
        let delta_label = self
            .last_delta
            .map(|(old, new, edits)| format!("增量=v{}→v{} 编辑数={}", old.get(), new.get(), edits))
            .unwrap_or_else(|| "增量=无".to_string());
        let history_label = format!(
            "撤销栈={} 重做栈={} 可撤销={} 可重做={} 上次事件={}",
            history.undo_depth,
            history.redo_depth,
            bool_label(self.buffer.can_undo()),
            bool_label(self.buffer.can_redo()),
            self.last_history_event.as_deref().unwrap_or("无"),
        );
        let snapshot_label = self
            .pinned_snapshot
            .as_ref()
            .map(|snapshot| {
                format!(
                    "快照=v{} 字符={} 字节={} UTF-16={} 行数={} 状态={}",
                    snapshot.version().get(),
                    snapshot.len_chars().get(),
                    snapshot.len_bytes(),
                    snapshot.len_utf16_cu(),
                    snapshot.line_count(),
                    snapshot_state_label(self.buffer.is_snapshot_stale(snapshot)),
                )
            })
            .unwrap_or_else(|| "快照=无".to_string());
        let storage_label = format!(
            "存储=RopeyStorage 字符={} 字节={} UTF-16={} 行数={}",
            self.buffer.len_chars().get(),
            self.buffer.len_bytes(),
            self.buffer.len_utf16_cu(),
            self.buffer.line_count(),
        );

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x18181B))
            .text_color(rgb(0xE4E4E7))
            .p_6()
            .track_focus(&self.focus_handle)
            .tab_index(0)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                let key = &event.keystroke.key;
                let modifiers = &event.keystroke.modifiers;

                let has_modifier = modifiers.platform || modifiers.control || modifiers.alt;

                if has_modifier {
                    return;
                }

                if key == "space" {
                    this.insert_text(" ", cx);
                } else if key == "tab" {
                    this.insert_text("    ", cx);
                } else if key.chars().count() == 1 {
                    this.insert_text(key, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Backspace, _window, cx| this.backspace(cx)))
            .on_action(cx.listener(|this, _: &DeleteForward, _window, cx| {
                this.delete_forward(cx)
            }))
            .on_action(cx.listener(|this, _: &Enter, _window, cx| this.insert_text("\n", cx)))
            .on_action(cx.listener(|this, _: &Left, _window, cx| this.move_left(cx)))
            .on_action(cx.listener(|this, _: &Right, _window, cx| this.move_right(cx)))
            .on_action(cx.listener(|this, _: &Home, _window, cx| {
                this.move_to_line_start(cx)
            }))
            .on_action(cx.listener(|this, _: &End, _window, cx| {
                this.move_to_line_end(cx)
            }))
            .on_action(cx.listener(|this, _: &Save, _window, cx| this.mark_saved(cx)))
            .on_action(cx.listener(|this, _: &Reset, _window, cx| this.reset(cx)))
            .on_action(cx.listener(|this, _: &BatchEdit, _window, cx| this.batch_edit(cx)))
            .on_action(cx.listener(|this, _: &Undo, _window, cx| this.undo(cx)))
            .on_action(cx.listener(|this, _: &Redo, _window, cx| this.redo(cx)))
            .on_action(cx.listener(|this, _: &MergeDemo, _window, cx| this.merge_demo(cx)))
            .on_action(cx.listener(|this, _: &CaptureSnapshot, _window, cx| {
                this.capture_snapshot(cx)
            }))
            .on_action(cx.listener(|this, _: &InsertLongLine, _window, cx| {
                this.insert_long_line(cx)
            }))
            .child(
                div()
                    .border_b_1()
                    .border_color(rgb(0x3F3F46))
                    .pb_4()
                    .mb_4()
                    .child(format!(
                        "Zom Engine M4 | 光标={} / 总字符={} | 行={} 列={} | 当前版本=v{} 保存点=v{} | 修改状态={} | 锚点A={} | 锚点B={} | 变更范围={} | {} | {} | {} | {}",
                        cursor.get(),
                        self.buffer.len_chars().get(),
                        position.line().get(),
                        position.column().get(),
                        self.buffer.version().get(),
                        self.buffer.saved_version().get(),
                        dirty_label(self.buffer.is_dirty()),
                        self.anchor_a.get(),
                        self.anchor_b.get(),
                        self.last_changed_ranges.len(),
                        delta_label,
                        history_label,
                        snapshot_label,
                        storage_label,
                    )),
            )
            .child(
                div()
                    .mb_4()
                    .text_color(rgb(0xA1A1AA))
                    .child("输入字符 / 空格 / Tab / 回车；连续输入会合并为一个撤销步骤；退格 / Delete；← →；Home / End；Cmd-S 保存；Cmd-R 重置；Cmd-B 批量事务；Cmd-M 合并事务；Cmd-Z/Ctrl-Z 撤销；Cmd-Shift-Z/Ctrl-Y 重做；Cmd-K 捕获快照；Cmd-L 插入长行探针"),
            )
            .when_some(self.last_error.clone(), |el, error| {
                el.child(
                    div()
                        .mb_4()
                        .text_color(rgb(0xFCA5A5))
                        .child(format!("错误：{error}")),
                )
            })
            .child(
                div()
                    .flex_1()
                    .font_family(".AppleSystemUIFont")
                    .text_xl()
                    .line_height(px(28.0))
                    .children(render_lines_with_markers(
                        self.buffer.text().as_ref(),
                        cursor,
                        self.anchor_a,
                        self.anchor_b,
                        &self.last_changed_ranges,
                    )),
            )
    }
}

fn initial_buffer_and_anchors() -> (Buffer, CharOffset, CharOffset) {
    let text = INITIAL_TEXT.to_string();
    let anchor_a =
        byte_to_char_offset(&text, text.find("[A]").expect("fixture should contain [A]"));
    let anchor_b =
        byte_to_char_offset(&text, text.find("[B]").expect("fixture should contain [B]"));
    let mut buffer =
        Buffer::from_text(text, BufferConfig::default()).expect("initial buffer should be valid");
    buffer
        .set_selection(SelectionSet::caret(CharOffset::ZERO))
        .expect("zero caret should be valid");

    (buffer, anchor_a, anchor_b)
}

fn byte_to_char_offset(text: &str, byte_offset: usize) -> CharOffset {
    CharOffset::new(text[..byte_offset].chars().count())
}

fn render_lines_with_markers(
    text: &str,
    cursor: CharOffset,
    anchor_a: CharOffset,
    anchor_b: CharOffset,
    changed_ranges: &[TextRange],
) -> Vec<Div> {
    let mut rows = Vec::new();
    let cursor_char = cursor.get();
    let a_char = anchor_a.get();
    let b_char = anchor_b.get();

    if text.is_empty() {
        rows.push(cursor_row());
        return rows;
    }

    let mut line_start = 0;

    for line_with_newline in text.split_inclusive('\n') {
        let line_end = line_start + line_with_newline.chars().count();
        let display_line = line_with_newline
            .trim_end_matches('\n')
            .trim_end_matches('\r');

        let mut row_children: Vec<gpui::AnyElement> = Vec::new();
        let mut char_offset = line_start;

        for c in display_line.chars() {
            let next_offset = char_offset + 1;

            if char_offset == cursor_char {
                row_children.push(cursor_element().into_any());
            }

            let mut is_highlighted = false;
            let mut is_deletion_scar = false;

            for range in changed_ranges {
                let start = range.start().get();
                let end = range.end().get();

                if start == end && char_offset == start {
                    is_deletion_scar = true;
                } else if char_offset >= start && char_offset < end {
                    is_highlighted = true;
                }
            }

            let mut char_div = div().child(c.to_string());

            if is_highlighted {
                char_div = char_div.bg(rgb(0x854D0E)).text_color(rgb(0xFEF08A));
            } else if char_offset == a_char {
                char_div = char_div.bg(rgb(0x991B1B)).text_color(rgb(0xFECACA));
            } else if char_offset == b_char {
                char_div = char_div.bg(rgb(0x166534)).text_color(rgb(0xBBF7D0));
            }

            if is_deletion_scar {
                char_div = char_div.border_l_2().border_color(rgb(0xEF4444));
            }

            row_children.push(char_div.into_any());
            char_offset = next_offset;
        }

        if char_offset == cursor_char {
            row_children.push(cursor_element().into_any());
        }

        for range in changed_ranges {
            if range.start().get() == range.end().get() && char_offset == range.start().get() {
                row_children.push(
                    div()
                        .h(px(22.0))
                        .border_l_2()
                        .border_color(rgb(0xEF4444))
                        .into_any(),
                );
            }
        }

        rows.push(
            div()
                .flex()
                .flex_row()
                .min_h(px(28.0))
                .children(row_children),
        );
        line_start = line_end;
    }

    if text.ends_with('\n') {
        if cursor_char == text.chars().count() {
            rows.push(cursor_row());
        } else {
            rows.push(div().min_h(px(28.0)).child(""));
        }
    }

    rows
}

fn cursor_row() -> Div {
    div()
        .flex()
        .flex_row()
        .min_h(px(28.0))
        .child(cursor_element())
}

#[allow(dead_code)]
fn dirty_label(is_dirty: bool) -> &'static str {
    if is_dirty { "已修改" } else { "干净" }
}

#[allow(dead_code)]
fn bool_label(value: bool) -> &'static str {
    if value { "是" } else { "否" }
}

#[allow(dead_code)]
fn snapshot_state_label(is_stale: bool) -> &'static str {
    if is_stale { "已过期" } else { "有效" }
}

#[allow(dead_code)]
fn line_ending_label<T: core::fmt::Debug>(style: T) -> String {
    match format!("{style:?}").as_str() {
        "None" => "未检测".to_string(),
        "Lf" | "LF" => "LF".to_string(),
        "Crlf" | "CRLF" => "CRLF".to_string(),
        "Mixed" => "混合".to_string(),
        other => other.to_string(),
    }
}

#[allow(dead_code)]
fn display_width_policy_label<T: core::fmt::Debug>(policy: T) -> String {
    format!("{policy:?}")
        .replace("DisplayWidthPolicy", "显示宽度策略")
        .replace("cjk_width", "CJK宽度")
        .replace("emoji_width", "emoji宽度")
        .replace("ambiguous_width", "模糊宽度")
        .replace("control_width", "控制字符宽度")
        .replace("combining_mark_width", "组合标记宽度")
}

fn cursor_element() -> Div {
    div().w(px(2.0)).h(px(22.0)).bg(rgb(0x3B82F6))
}

fn map_position_after_edits(pos: CharOffset, edits: &[Edit]) -> CharOffset {
    let mut diff = 0isize;
    let pos_val = pos.get() as isize;

    for edit in edits {
        let start = edit.range.start().get() as isize;
        let end = edit.range.end().get() as isize;
        let replacement_len = edit.replacement.chars().count() as isize;

        if pos_val < start {
            break;
        }

        if pos_val < end {
            return CharOffset::new((start + diff).max(0) as usize);
        }

        diff += replacement_len - (end - start);
    }

    CharOffset::new((pos_val + diff).max(0) as usize)
}

fn previous_edit_boundary(text: &str, cursor: CharOffset) -> Option<CharOffset> {
    let mut current = cursor.get();
    let len_chars = text.chars().count();

    if current == 0 || current > len_chars {
        return None;
    }

    loop {
        let prev = current.checked_sub(1)?;

        if !is_crlf_middle(text, prev) {
            return Some(CharOffset::new(prev));
        }

        current = prev;
    }
}

fn next_edit_boundary(text: &str, cursor: CharOffset) -> Option<CharOffset> {
    let len_chars = text.chars().count();
    let mut current = cursor.get();

    if current >= len_chars {
        return None;
    }

    loop {
        let next = current + 1;

        if next > len_chars {
            return None;
        }

        if !is_crlf_middle(text, next) {
            return Some(CharOffset::new(next));
        }

        current = next;
    }
}

fn line_content_end(text: &str, line_start: usize, next_line_start: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();

    if next_line_start > line_start && chars.get(next_line_start - 1) == Some(&'\n') {
        if next_line_start >= line_start + 2 && chars.get(next_line_start - 2) == Some(&'\r') {
            next_line_start - 2
        } else {
            next_line_start - 1
        }
    } else {
        next_line_start
    }
}

fn is_crlf_middle(text: &str, offset: usize) -> bool {
    let chars: Vec<char> = text.chars().collect();

    offset > 0
        && offset < chars.len()
        && chars.get(offset - 1) == Some(&'\r')
        && chars.get(offset) == Some(&'\n')
}

fn main() {
    Application::new().run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("backspace", Backspace, None),
            KeyBinding::new("delete", DeleteForward, None),
            KeyBinding::new("enter", Enter, None),
            KeyBinding::new("left", Left, None),
            KeyBinding::new("right", Right, None),
            KeyBinding::new("home", Home, None),
            KeyBinding::new("end", End, None),
            KeyBinding::new("cmd-s", Save, None),
            KeyBinding::new("cmd-r", Reset, None),
            KeyBinding::new("cmd-b", BatchEdit, None),
            KeyBinding::new("cmd-m", MergeDemo, None),
            KeyBinding::new("cmd-k", CaptureSnapshot, None),
            KeyBinding::new("cmd-l", InsertLongLine, None),
            KeyBinding::new("cmd-z", Undo, None),
            KeyBinding::new("ctrl-z", Undo, None),
            KeyBinding::new("cmd-shift-z", Redo, None),
            KeyBinding::new("ctrl-y", Redo, None),
        ]);

        let bounds = Bounds::centered(None, gpui::size(px(980.0), px(740.0)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_window, cx| cx.new(|cx| M4Testbed::new(cx)),
        )
        .unwrap();

        cx.activate(true);
    });
}
