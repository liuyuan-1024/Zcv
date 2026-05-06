//! M2 GPUI testbed：在完整继承 M1 手感的基础上展示 Transaction、Delta 与 ChangeSet。
//!
//! 本文件只验证事务架构接入 UI 后的体感和可观察性，不把编辑排序、原子性或映射正确性寄托在 example 上。

use gpui::{
    App, Application, Bounds, Context, Div, FocusHandle, IntoElement, KeyBinding, KeyDownEvent,
    Render, Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, rgb,
};

use zom_engine::{
    Buffer, BufferConfig, BufferVersion, CharOffset, Edit, EditList, EngineResult, Line,
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

const INITIAL_TEXT: &str = "🚀 Zom Engine M2 中文测试台

[A] 这是锚点A，[B] 这是锚点B。
可以输入、回车、退格、Delete、左右移动光标。
Home / End 可跳到当前行首尾。
Cmd-S 标记已保存，Cmd-R 重置文本。
Cmd-B 触发批量事务修改。
所有写入都走事务；黄色区间是变更范围，A/B 会通过 PositionMap 跟随文本变化。
";

pub struct M2Testbed {
    buffer: Buffer,
    cursor: CharOffset,
    anchor_a: CharOffset,
    anchor_b: CharOffset,
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
            cursor: CharOffset::ZERO,
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
        self.cursor = CharOffset::ZERO;
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

                // M2 核心体感：游标和锚点都通过 PositionMap 跟随文本变化。
                self.cursor = changeset
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
        let Some(prev) = previous_edit_boundary(self.buffer.text().as_ref(), self.cursor) else {
            return;
        };

        self.delete_range(prev, self.cursor, cx);
    }

    fn delete_forward(&mut self, cx: &mut Context<Self>) {
        let Some(next) = next_edit_boundary(self.buffer.text().as_ref(), self.cursor) else {
            return;
        };

        self.delete_range(self.cursor, next, cx);
    }

    fn delete_range(&mut self, start: CharOffset, end: CharOffset, cx: &mut Context<Self>) {
        match TextRange::new(start, end) {
            Ok(range) => self.submit_edits(vec![Edit::delete(range)], cx),
            Err(err) => {
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

        self.submit_edits(vec![marker, sparkle], cx);
    }

    fn move_left(&mut self, cx: &mut Context<Self>) {
        if let Some(prev) = previous_edit_boundary(self.buffer.text().as_ref(), self.cursor) {
            self.cursor = prev;
            self.last_error = None;
            cx.notify();
        }
    }

    fn move_right(&mut self, cx: &mut Context<Self>) {
        if let Some(next) = next_edit_boundary(self.buffer.text().as_ref(), self.cursor) {
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
}

impl Render for M2Testbed {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cursor = self.cursor;
        let position = self.cursor_position();
        let delta_label = self
            .last_delta
            .map(|(old, new, edits)| format!("增量=v{}→v{} 编辑数={}", old.get(), new.get(), edits))
            .unwrap_or_else(|| "增量=无".to_string());

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
                        "Zom Engine M2 | 光标={} / 总字符={} | 行={} 列={} | 总行数={} | 当前版本=v{} 保存点=v{} | 修改状态={} | 锚点A={} | 锚点B={} | 变更范围={} | {}",
                        cursor.get(),
                        self.buffer.len_chars().get(),
                        position.line().get(),
                        position.column().get(),
                        self.buffer.line_count(),
                        self.buffer.version().get(),
                        self.buffer.saved_version().get(),
                        dirty_label(self.buffer.is_dirty()),
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
                    .child("输入字符 / 空格 / Tab / 回车；退格 / Delete；← →；Home / End；Cmd-S 保存；Cmd-R 重置；Cmd-B 批量事务"),
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
    let buffer =
        Buffer::from_text(text, BufferConfig::default()).expect("initial buffer should be valid");

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
