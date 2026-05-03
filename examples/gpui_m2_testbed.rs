use gpui::{
    App, Application, Bounds, Context, Div, FocusHandle, IntoElement, KeyBinding, KeyDownEvent,
    Render, Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, rgb,
};

use zom_engine::{
    Buffer, BufferConfig, BufferVersion, ByteOffset, Edit, EditList, EngineResult, Line,
    LogicalColumn, Position, TextRange, Transaction,
};

// M2 testbed：必须是 M1 testbed 的 superset。
// 保留 M1 的输入 / Delete / Home / End / Save / Reset / 状态栏能力，
// 仅把所有写入路径切换为 Transaction，并额外可视化 Delta / ChangeSet。
actions!(
    m2_testbed,
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
        BatchEdit
    ]
);

const INITIAL_TEXT: &str = "🚀 Zom Engine M2 GPUI Testbed\n\n[A] 这是锚点A，[B] 这是锚点B。\n可以输入、回车、退格、Delete、左右移动光标。\nHome / End 可跳到当前行首尾。\nCmd-S 标记 saved，Cmd-R 重置文本。\nCmd-B 触发批量事务修改。\n所有写入都走 Transaction；黄色区间是 changed_ranges，A/B 会跟随 ChangeSet 映射。\n";

pub struct M2Testbed {
    buffer: Buffer,
    cursor: ByteOffset,
    anchor_a: ByteOffset,
    anchor_b: ByteOffset,
    last_changed_ranges: Vec<TextRange>,
    last_delta: Option<(BufferVersion, BufferVersion, usize)>,
    focus_handle: FocusHandle,
    last_error: Option<String>,
}

impl M2Testbed {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (buffer, anchor_a, anchor_b) = initial_buffer_and_anchors();

        Self {
            buffer,
            cursor: ByteOffset::ZERO,
            anchor_a,
            anchor_b,
            last_changed_ranges: Vec::new(),
            last_delta: None,
            focus_handle: cx.focus_handle(),
            last_error: None,
        }
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        let (buffer, anchor_a, anchor_b) = initial_buffer_and_anchors();

        self.buffer = buffer;
        self.cursor = ByteOffset::ZERO;
        self.anchor_a = anchor_a;
        self.anchor_b = anchor_b;
        self.last_changed_ranges.clear();
        self.last_delta = None;
        self.last_error = None;

        cx.notify();
    }

    fn mark_saved(&mut self, cx: &mut Context<Self>) {
        self.buffer.mark_saved();
        self.last_error = None;
        cx.notify();
    }

    fn submit_edits(&mut self, edits: Vec<Edit>, cx: &mut Context<Self>) {
        let edit_list = match EditList::new(edits) {
            Ok(edit_list) => edit_list,
            Err(err) => {
                self.last_error = Some(err.to_string());
                cx.notify();
                return;
            }
        };

        let tx = match Transaction::new(self.buffer.version(), edit_list) {
            Ok(tx) => tx,
            Err(err) => {
                self.last_error = Some(err.to_string());
                cx.notify();
                return;
            }
        };

        self.apply_transaction(tx, cx);
    }

    fn apply_transaction(&mut self, tx: Transaction, cx: &mut Context<Self>) {
        match self.buffer.apply_transaction(tx) {
            Ok((delta, changeset)) => {
                self.last_error = None;
                self.last_delta = Some((
                    delta.old_version,
                    delta.new_version,
                    delta.edits.as_slice().len(),
                ));

                // M2 核心体感：游标和锚点都通过 ChangeSet 跟随文本变化。
                self.cursor = changeset.map_position(self.cursor);
                self.anchor_a = changeset.map_position(self.anchor_a);
                self.anchor_b = changeset.map_position(self.anchor_b);
                self.last_changed_ranges = changeset.changed_ranges();
            }
            Err(err) => {
                self.last_error = Some(err.to_string());
            }
        }

        cx.notify();
    }

    fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        match Edit::insert(self.cursor, text.to_string()) {
            Ok(edit) => self.submit_edits(vec![edit], cx),
            Err(err) => {
                self.last_error = Some(err.to_string());
                cx.notify();
            }
        }
    }

    fn backspace(&mut self, cx: &mut Context<Self>) {
        let Some(prev) = previous_edit_boundary(self.buffer.text(), self.cursor) else {
            return;
        };

        self.delete_range(prev, self.cursor, cx);
    }

    fn delete_forward(&mut self, cx: &mut Context<Self>) {
        let Some(next) = next_edit_boundary(self.buffer.text(), self.cursor) else {
            return;
        };

        self.delete_range(self.cursor, next, cx);
    }

    fn delete_range(&mut self, start: ByteOffset, end: ByteOffset, cx: &mut Context<Self>) {
        match TextRange::new(start, end) {
            Ok(range) => self.submit_edits(vec![Edit::delete(range)], cx),
            Err(err) => {
                self.last_error = Some(err.to_string());
                cx.notify();
            }
        }
    }

    fn batch_edit(&mut self, cx: &mut Context<Self>) {
        let marker = Edit::insert(ByteOffset::ZERO, "[批量标记] ".to_string()).unwrap();

        // 避免在 cursor=0 时制造两个同 offset 的插入，让 testbed 的视觉结果更可预期。
        let sparkle_offset = if self.cursor == ByteOffset::ZERO {
            self.buffer.len_bytes()
        } else {
            self.cursor
        };
        let sparkle = Edit::insert(sparkle_offset, "✨".to_string()).unwrap();

        self.submit_edits(vec![marker, sparkle], cx);
    }

    fn move_left(&mut self, cx: &mut Context<Self>) {
        if let Some(prev) = previous_edit_boundary(self.buffer.text(), self.cursor) {
            self.cursor = prev;
            self.last_error = None;
            cx.notify();
        }
    }

    fn move_right(&mut self, cx: &mut Context<Self>) {
        if let Some(next) = next_edit_boundary(self.buffer.text(), self.cursor) {
            self.cursor = next;
            self.last_error = None;
            cx.notify();
        }
    }

    fn move_to_line_start(&mut self, cx: &mut Context<Self>) {
        let line = self.cursor_position().line();

        match self.buffer.line_start(line) {
            Ok(offset) => {
                self.cursor = offset;
                self.last_error = None;
            }
            Err(err) => self.last_error = Some(err.to_string()),
        }

        cx.notify();
    }

    fn move_to_line_end(&mut self, cx: &mut Context<Self>) {
        let line = self.cursor_position().line();

        match self.line_content_end(line) {
            Ok(offset) => {
                self.cursor = offset;
                self.last_error = None;
            }
            Err(err) => self.last_error = Some(err.to_string()),
        }

        cx.notify();
    }

    fn line_content_end(&self, line: Line) -> EngineResult<ByteOffset> {
        let line_start = self.buffer.line_start(line)?.get();
        let next_line_start = if line.get() + 1 < self.buffer.line_count() {
            self.buffer.line_start(Line::new(line.get() + 1))?.get()
        } else {
            self.buffer.len_bytes().get()
        };

        Ok(ByteOffset::new(line_content_end(
            self.buffer.text(),
            line_start,
            next_line_start,
        )))
    }

    fn cursor_position(&self) -> Position {
        self.buffer
            .byte_to_position(self.cursor)
            .unwrap_or_else(|_| Position::new(Line::ZERO, LogicalColumn::ZERO))
    }
}

impl Render for M2Testbed {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cursor = self.cursor;
        let position = self.cursor_position();
        let delta_label = self
            .last_delta
            .map(|(old, new, edits)| format!("delta={}→{} edits={}", old.get(), new.get(), edits))
            .unwrap_or_else(|| "delta=<none>".to_string());

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
            .child(
                div()
                    .border_b_1()
                    .border_color(rgb(0x3F3F46))
                    .pb_4()
                    .mb_4()
                    .child(format!(
                        "Zom Engine M2 | byte={} / {} | line={} col={} | lines={} | version={} saved={} | dirty={} | A={} | B={} | changed_ranges={} | {}",
                        cursor.get(),
                        self.buffer.len_bytes().get(),
                        position.line().get(),
                        position.column().get(),
                        self.buffer.line_count(),
                        self.buffer.version().get(),
                        self.buffer.saved_version().get(),
                        self.buffer.is_dirty(),
                        self.anchor_a.get(),
                        self.anchor_b.get(),
                        self.last_changed_ranges.len(),
                        delta_label,
                    )),
            )
            .child(
                div()
                    .mb_4()
                    .text_color(rgb(0xA1A1AA))
                    .child("输入字符 / Space / Tab / Enter；Backspace / Delete；← →；Home / End；Cmd-S 保存；Cmd-R 重置；Cmd-B 批量事务"),
            )
            .when_some(self.last_error.clone(), |el, error| {
                el.child(
                    div()
                        .mb_4()
                        .text_color(rgb(0xFCA5A5))
                        .child(format!("error: {error}")),
                )
            })
            .child(
                div()
                    .flex_1()
                    .font_family(".AppleSystemUIFont")
                    .text_xl()
                    .line_height(px(28.0))
                    .children(render_lines_with_markers(
                        self.buffer.text(),
                        cursor,
                        self.anchor_a,
                        self.anchor_b,
                        &self.last_changed_ranges,
                    )),
            )
    }
}

fn initial_buffer_and_anchors() -> (Buffer, ByteOffset, ByteOffset) {
    let text = INITIAL_TEXT.to_string();
    let anchor_a = ByteOffset::new(text.find("[A]").expect("fixture should contain [A]"));
    let anchor_b = ByteOffset::new(text.find("[B]").expect("fixture should contain [B]"));
    let buffer =
        Buffer::from_text(text, BufferConfig::default()).expect("initial buffer should be valid");

    (buffer, anchor_a, anchor_b)
}

fn render_lines_with_markers(
    text: &str,
    cursor: ByteOffset,
    anchor_a: ByteOffset,
    anchor_b: ByteOffset,
    changed_ranges: &[TextRange],
) -> Vec<Div> {
    let mut rows = Vec::new();
    let cursor_byte = cursor.get();
    let a_byte = anchor_a.get();
    let b_byte = anchor_b.get();

    if text.is_empty() {
        rows.push(cursor_row());
        return rows;
    }

    let mut line_start = 0;

    for line_with_newline in text.split_inclusive('\n') {
        let line_end = line_start + line_with_newline.len();
        let display_line = line_with_newline
            .trim_end_matches('\n')
            .trim_end_matches('\r');

        let mut row_children: Vec<gpui::AnyElement> = Vec::new();
        let mut char_offset = line_start;

        for c in display_line.chars() {
            let next_offset = char_offset + c.len_utf8();

            if char_offset == cursor_byte {
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
            } else if char_offset == a_byte {
                char_div = char_div.bg(rgb(0x991B1B)).text_color(rgb(0xFECACA));
            } else if char_offset == b_byte {
                char_div = char_div.bg(rgb(0x166534)).text_color(rgb(0xBBF7D0));
            }

            if is_deletion_scar {
                char_div = char_div.border_l_2().border_color(rgb(0xEF4444));
            }

            row_children.push(char_div.into_any());
            char_offset = next_offset;
        }

        if char_offset == cursor_byte {
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
        if cursor_byte == text.len() {
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

fn cursor_element() -> Div {
    div().w(px(2.0)).h(px(22.0)).bg(rgb(0x3B82F6))
}

fn previous_edit_boundary(text: &str, cursor: ByteOffset) -> Option<ByteOffset> {
    let mut current = cursor.get();

    if current == 0 || current > text.len() || !text.is_char_boundary(current) {
        return None;
    }

    loop {
        let prev = text[..current].char_indices().last()?.0;

        if !is_crlf_middle(text, prev) {
            return Some(ByteOffset::new(prev));
        }

        current = prev;
    }
}

fn next_edit_boundary(text: &str, cursor: ByteOffset) -> Option<ByteOffset> {
    let current = cursor.get();

    if current >= text.len() || !text.is_char_boundary(current) {
        return None;
    }

    for (relative, _) in text[current..].char_indices().skip(1) {
        let next = current + relative;

        if !is_crlf_middle(text, next) {
            return Some(ByteOffset::new(next));
        }
    }

    Some(ByteOffset::new(text.len()))
}

fn line_content_end(text: &str, line_start: usize, next_line_start: usize) -> usize {
    let bytes = text.as_bytes();

    if next_line_start > line_start && bytes[next_line_start - 1] == b'\n' {
        if next_line_start >= line_start + 2 && bytes[next_line_start - 2] == b'\r' {
            next_line_start - 2
        } else {
            next_line_start - 1
        }
    } else {
        next_line_start
    }
}

fn is_crlf_middle(text: &str, offset: usize) -> bool {
    let bytes = text.as_bytes();

    offset > 0 && offset < bytes.len() && bytes[offset - 1] == b'\r' && bytes[offset] == b'\n'
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
        ]);

        let bounds = Bounds::centered(None, gpui::size(px(900.0), px(680.0)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_window, cx| cx.new(|cx| M2Testbed::new(cx)),
        )
        .unwrap();

        cx.activate(true);
    });
}
