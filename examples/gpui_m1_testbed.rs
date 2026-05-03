use gpui::{
    App, Application, Bounds, Context, FocusHandle, IntoElement, KeyBinding, KeyDownEvent, Render,
    Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, rgb,
};

use zom_engine::{
    Buffer, BufferConfig, ByteOffset, EngineResult, Line, LogicalColumn, Position, TextRange,
};

actions!(m1_testbed, [Backspace, Enter, Left, Right]);

pub struct M1Testbed {
    buffer: Buffer,
    cursor: ByteOffset,
    focus_handle: FocusHandle,
    last_error: Option<String>,
}

impl M1Testbed {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let text = "🚀 Zom Engine M1 GPUI Testbed\n\n可以输入、回车、退格、左右移动光标。\n当前只是 Buffer 体验，不包含 Undo/Redo/Selection。\n".to_string();

        let buffer = Buffer::from_text(text, BufferConfig::default())
            .expect("initial buffer should be valid");

        Self {
            buffer,
            cursor: ByteOffset::ZERO,
            focus_handle: cx.focus_handle(),
            last_error: None,
        }
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
            self.cursor = ByteOffset::new(self.cursor.get() + text.len());
        }

        self.apply(result, cx);
    }

    fn backspace(&mut self, cx: &mut Context<Self>) {
        let Some(prev) = previous_char_boundary(self.buffer.text(), self.cursor) else {
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

    fn move_left(&mut self, cx: &mut Context<Self>) {
        if let Some(prev) = previous_char_boundary(self.buffer.text(), self.cursor) {
            self.cursor = prev;
            cx.notify();
        }
    }

    fn move_right(&mut self, cx: &mut Context<Self>) {
        if let Some(next) = next_char_boundary(self.buffer.text(), self.cursor) {
            self.cursor = next;
            cx.notify();
        }
    }

    fn cursor_position(&self) -> Position {
        self.buffer
            .byte_to_position(self.cursor)
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
            .on_action(cx.listener(|this, _: &Enter, _window, cx| {
                this.insert_text("\n", cx);
            }))
            .on_action(cx.listener(|this, _: &Left, _window, cx| {
                this.move_left(cx);
            }))
            .on_action(cx.listener(|this, _: &Right, _window, cx| {
                this.move_right(cx);
            }))
            .child(
                div()
                    .border_b_1()
                    .border_color(rgb(0x3F3F46))
                    .pb_4()
                    .mb_4()
                    .child(format!(
                        "Zom Engine M1 | byte={} | line={} col={} | lines={} | version={} | dirty={}",
                        cursor.get(),
                        position.line().get(),
                        position.column().get(),
                        self.buffer.line_count(),
                        self.buffer.version().get(),
                        self.buffer.is_dirty(),
                    )),
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
                    .children(render_lines_with_cursor(self.buffer.text(), cursor)),
            )
    }
}

fn render_lines_with_cursor(text: &str, cursor: ByteOffset) -> Vec<impl IntoElement> {
    let mut rows = Vec::new();
    let cursor_byte = cursor.get();

    let mut line_start = 0;

    for line_with_newline in text.split_inclusive('\n') {
        let line_end = line_start + line_with_newline.len();
        let display_line = line_with_newline
            .trim_end_matches('\n')
            .trim_end_matches('\r');

        if cursor_byte >= line_start && cursor_byte <= line_end {
            let cursor_in_line = cursor_byte
                .saturating_sub(line_start)
                .min(display_line.len());

            let before = &display_line[..cursor_in_line];
            let after = &display_line[cursor_in_line..];

            rows.push(
                div()
                    .flex()
                    .flex_row()
                    .min_h(px(28.0))
                    .child(before.to_string())
                    .child(div().w(px(2.0)).h(px(22.0)).bg(rgb(0x3B82F6)))
                    .child(after.to_string()),
            );
        } else {
            rows.push(div().min_h(px(28.0)).child(display_line.to_string()));
        }

        line_start = line_end;
    }

    if text.is_empty() || text.ends_with('\n') {
        rows.push(
            div()
                .flex()
                .flex_row()
                .min_h(px(28.0))
                .child(div().w(px(2.0)).h(px(22.0)).bg(rgb(0x3B82F6))),
        );
    }

    rows
}

fn previous_char_boundary(text: &str, cursor: ByteOffset) -> Option<ByteOffset> {
    let current = cursor.get();

    if current == 0 {
        return None;
    }

    text[..current]
        .char_indices()
        .last()
        .map(|(idx, _)| ByteOffset::new(idx))
}

fn next_char_boundary(text: &str, cursor: ByteOffset) -> Option<ByteOffset> {
    let current = cursor.get();

    if current >= text.len() {
        return None;
    }

    text[current..]
        .char_indices()
        .nth(1)
        .map(|(idx, _)| ByteOffset::new(current + idx))
        .or_else(|| Some(ByteOffset::new(text.len())))
}

fn main() {
    Application::new().run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("backspace", Backspace, None),
            KeyBinding::new("enter", Enter, None),
            KeyBinding::new("left", Left, None),
            KeyBinding::new("right", Right, None),
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
