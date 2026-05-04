use gpui::{
    App, Application, Bounds, Context, Div, FocusHandle, IntoElement, KeyBinding, KeyDownEvent,
    Render, Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, rgb,
};

use zom_engine::{
    Buffer, BufferConfig, CharOffset, EngineResult, Line, LogicalColumn, Position, TextRange,
};

actions!(
    m1_testbed,
    [
        Backspace,
        DeleteForward,
        Enter,
        Left,
        Right,
        Home,
        End,
        Save,
        Reset
    ]
);

const INITIAL_TEXT: &str = "🚀 Zom Engine M1 GPUI Testbed\n\n可以输入、回车、退格、Delete、左右移动光标。\nHome / End 可跳到当前行首尾。\nCmd-S 标记 saved，Cmd-R 重置文本。\n当前只是 Buffer 体验，不包含 Undo/Redo/Selection。\n";

pub struct M1Testbed {
    buffer: Buffer,
    cursor: CharOffset,
    focus_handle: FocusHandle,
    last_error: Option<String>,
}

impl M1Testbed {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let buffer = Buffer::from_text(INITIAL_TEXT.to_string(), BufferConfig::default())
            .expect("initial buffer should be valid");

        Self {
            buffer,
            cursor: CharOffset::ZERO,
            focus_handle: cx.focus_handle(),
            last_error: None,
        }
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        match Buffer::from_text(INITIAL_TEXT.to_string(), BufferConfig::default()) {
            Ok(buffer) => {
                self.buffer = buffer;
                self.cursor = CharOffset::ZERO;
                self.last_error = None;
            }
            Err(err) => self.last_error = Some(err.to_string()),
        }

        cx.notify();
    }

    fn mark_saved(&mut self, cx: &mut Context<Self>) {
        self.buffer.mark_saved();
        self.last_error = None;
        cx.notify();
    }

    fn apply(&mut self, result: EngineResult<()>, cx: &mut Context<Self>) {
        match result {
            Ok(()) => self.last_error = None,
            Err(err) => self.last_error = Some(err.to_string()),
        }

        cx.notify();
    }

    fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let result = self.buffer.insert(self.cursor, text);

        if result.is_ok() {
            self.cursor = CharOffset::new(self.cursor.get() + text.chars().count());
        }

        self.apply(result, cx);
    }

    fn backspace(&mut self, cx: &mut Context<Self>) {
        let Some(prev) = previous_edit_boundary(self.buffer.text().as_ref(), self.cursor) else {
            return;
        };

        let range = TextRange::new(prev, self.cursor)
            .expect("previous cursor boundary should form a valid range");

        let result = self.buffer.delete(range);

        if result.is_ok() {
            self.cursor = prev;
        }

        self.apply(result, cx);
    }

    fn delete_forward(&mut self, cx: &mut Context<Self>) {
        let Some(next) = next_edit_boundary(self.buffer.text().as_ref(), self.cursor) else {
            return;
        };

        let range = TextRange::new(self.cursor, next)
            .expect("next cursor boundary should form a valid range");

        let result = self.buffer.delete(range);
        self.apply(result, cx);
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

impl Render for M1Testbed {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cursor = self.cursor;
        let position = self.cursor_position();

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
            .on_action(cx.listener(|this, _: &Backspace, _window, cx| {
                this.backspace(cx);
            }))
            .on_action(cx.listener(|this, _: &DeleteForward, _window, cx| {
                this.delete_forward(cx);
            }))
            .on_action(cx.listener(|this, _: &Enter, _window, cx| {
                this.insert_text("\n", cx);
            }))
            .on_action(cx.listener(|this, _: &Left, _window, cx| {
                this.move_left(cx);
            }))
            .on_action(cx.listener(|this, _: &Right, _window, cx| {
                this.move_right(cx);
            }))
            .on_action(cx.listener(|this, _: &Home, _window, cx| {
                this.move_to_line_start(cx);
            }))
            .on_action(cx.listener(|this, _: &End, _window, cx| {
                this.move_to_line_end(cx);
            }))
            .on_action(cx.listener(|this, _: &Save, _window, cx| {
                this.mark_saved(cx);
            }))
            .on_action(cx.listener(|this, _: &Reset, _window, cx| {
                this.reset(cx);
            }))
            .child(
                div()
                    .border_b_1()
                    .border_color(rgb(0x3F3F46))
                    .pb_4()
                    .mb_4()
                    .child(format!(
                        "Zom Engine M1 | char={} / {} | line={} col={} | lines={} | version={} saved={} | dirty={}",
                        cursor.get(),
                        self.buffer.len_chars().get(),
                        position.line().get(),
                        position.column().get(),
                        self.buffer.line_count(),
                        self.buffer.version().get(),
                        self.buffer.saved_version().get(),
                        self.buffer.is_dirty(),
                    )),
            )
            .child(
                div()
                    .mb_4()
                    .text_color(rgb(0xA1A1AA))
                    .child("输入字符 / Space / Tab / Enter；Backspace / Delete；← →；Home / End；Cmd-S 保存；Cmd-R 重置"),
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
                    .children(render_lines_with_cursor(self.buffer.text().as_ref(), cursor)),
            )
    }
}

fn render_lines_with_cursor(text: &str, cursor: CharOffset) -> Vec<Div> {
    let mut rows = Vec::new();
    let cursor_char = cursor.get();

    if text.is_empty() {
        rows.push(cursor_row());
        return rows;
    }

    let mut line_start = 0usize;

    for line_with_newline in text.split_inclusive('\n') {
        let line_end = line_start + line_with_newline.chars().count();
        let display_line = line_with_newline
            .trim_end_matches('\n')
            .trim_end_matches('\r');
        let display_end = line_start + display_line.chars().count();

        let mut row_children: Vec<gpui::AnyElement> = Vec::new();
        let mut char_offset = line_start;

        for c in display_line.chars() {
            if char_offset == cursor_char {
                row_children.push(cursor_element().into_any());
            }

            row_children.push(div().child(c.to_string()).into_any());
            char_offset += 1;
        }

        if cursor_char >= line_start && cursor_char <= display_end && char_offset == cursor_char {
            row_children.push(cursor_element().into_any());
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
        ]);

        let bounds = Bounds::centered(None, gpui::size(px(900.0), px(640.0)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_window, cx| cx.new(|cx| M1Testbed::new(cx)),
        )
        .unwrap();

        cx.activate(true);
    });
}
