//! M13 GPUI testbed：Fold Model + Projection Coordinate 最小体感验证。
//!
//! 本 example 仅做 M13 公共 API 的接入手感验证，不替代
//! `tests/m13_fold_set.rs` / `tests/m13_projection_line_map.rs` /
//! `tests/m13_projection_range_map.rs` / `tests/m13_projected_viewport.rs`
//! 这些机器契约。
//!
//! UX 设计要点：
//! - 启动时自动建好 2 个示例 fold，打开窗口就能看到 placeholder 与折叠效果；
//! - 当前光标行用浅色背景标记，光标在文本中以蓝色竖线显示在精确列上；
//! - 非空选区在文本背景上高亮，让 Shift+方向键的扩展立刻可见；
//! - Cmd-T 把光标所在行折叠成「当前行 + 后 2 行」(共 3 行 = anchor + 2 隐藏)，保证
//!   一定会出现 placeholder；如果当前行已在 fold 内则展开；
//! - Cmd-F 折叠当前选区，要求选区横跨至少 2 行，否则给出明确提示（M13 中折叠单行不会
//!   隐藏任何行，spec：anchor=line 自身，hidden=空）；
//! - Cmd-D 重新生成示例 fold；Cmd-U 全部展开；Cmd-R 完整重置。

use gpui::{
    App, Application, Bounds, Context, FocusHandle, Focusable, IntoElement, KeyBinding,
    KeyDownEvent, Render, Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, rgb,
    size,
};

use zom_engine::{
    Buffer, BufferConfig, CharOffset, Edit, EngineError, EngineResult, FoldRange, FoldSet,
    HiddenRange, Line, LineRange, LogicalPoint, LogicalProjection, Position, ProjectedLineIndex,
    ProjectedViewport, ProjectedViewportRow, ProjectedViewportRowKind, Projection, Selection,
    Snapshot, TextRange, Transaction, TransactionMetadata,
};

actions!(
    m13_testbed,
    [
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        Home,
        End,
        Backspace,
        DeleteForward,
        Enter,
        FoldSelection,
        ToggleFoldAtCursor,
        UnfoldAll,
        DemoFolds,
        Reset,
        Quit,
    ]
);

// 行号好定位的样本：每行内容自带行号注释，方便看哪一行被折叠。
const SAMPLE_TEXT: &str = "// L0  M13 Fold / Projection 体感台。Cmd-T、Cmd-F 立即看 placeholder\n\
// L1\n\
fn outer() {                  // L2\n\
    println!(\"outer head\"); // L3\n\
    let x = 1;                // L4\n\
    let y = 2;                // L5\n\
    fn inner() {              // L6\n\
        println!(\"inner\");  // L7\n\
        let z = 3;            // L8\n\
        let w = 4;            // L9\n\
    }                         // L10\n\
    println!(\"outer tail\"); // L11\n\
}                             // L12\n\
\n\
struct Sample {               // L14\n\
    field_a: u32,             // L15\n\
    field_b: String,          // L16\n\
    field_c: Vec<u8>,         // L17\n\
}                             // L18\n\
\n\
// 操作提示：\n\
// - 上下左右 / Shift+方向键扩展选区 / Home / End\n\
// - Cmd-T：当前行 + 后 2 行折叠（一定会出现 placeholder）；命中已有 fold 则展开\n\
// - Cmd-F：折叠选区（需要横跨 ≥ 2 行）\n\
// - Cmd-U：全部展开；Cmd-D：恢复演示用 fold；Cmd-R：完整重置\n\
// - 输入 / Backspace 编辑后观察 fold 跟随 DeltaEvent 平移\n";

pub struct M13Testbed {
    buffer: Buffer,
    folds: FoldSet,
    selection: Selection,
    focus_handle: FocusHandle,
    last_message: Option<Message>,
}

#[derive(Debug, Clone)]
enum Message {
    Info(String),
    Error(String),
}

impl M13Testbed {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let buffer = build_buffer();
        let mut folds = FoldSet::new(buffer.version());
        // 启动就建两个 fold：函数体 (L2..=L12) 与 struct 体 (L14..=L18)
        // 让用户打开窗口就能看到 placeholder。
        let _ = folds.fold_lines(&buffer, line_range(2, 13));
        let _ = folds.fold_lines(&buffer, line_range(14, 19));
        Self {
            buffer,
            folds,
            selection: Selection::caret(CharOffset::ZERO),
            focus_handle: cx.focus_handle(),
            last_message: Some(Message::Info(
                "已自动建立 2 条示例 fold（函数体 + struct 体）。".into(),
            )),
        }
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        let buffer = build_buffer();
        let mut folds = FoldSet::new(buffer.version());
        let _ = folds.fold_lines(&buffer, line_range(2, 13));
        let _ = folds.fold_lines(&buffer, line_range(14, 19));
        self.buffer = buffer;
        self.folds = folds;
        self.selection = Selection::caret(CharOffset::ZERO);
        self.last_message = Some(Message::Info("已重置为初始示例。".into()));
        cx.notify();
    }

    fn demo_folds(&mut self, cx: &mut Context<Self>) {
        self.folds.unfold_all();
        let mut messages = Vec::new();
        if let Err(err) = self.folds.fold_lines(&self.buffer, line_range(2, 13)) {
            messages.push(format!("L2..L13 失败: {err}"));
        }
        if let Err(err) = self.folds.fold_lines(&self.buffer, line_range(14, 19)) {
            messages.push(format!("L14..L19 失败: {err}"));
        }
        if messages.is_empty() {
            self.last_message = Some(Message::Info("已恢复演示 fold。".into()));
        } else {
            self.last_message = Some(Message::Error(messages.join("; ")));
        }
        cx.notify();
    }

    fn projection(&self) -> EngineResult<Projection> {
        Projection::build(&self.buffer.snapshot(), &self.folds)
    }

    // ---------- 编辑 ----------

    fn apply_edits(&mut self, edits: Vec<Edit>, new_caret: CharOffset, cx: &mut Context<Self>) {
        if edits.is_empty() {
            self.selection = Selection::caret(new_caret);
            cx.notify();
            return;
        }
        match Transaction::from_edits(self.buffer.version(), edits) {
            Ok(tx) => {
                let tx = tx.with_metadata(TransactionMetadata::default());
                match self.buffer.apply_transaction(tx) {
                    Ok(_) => {
                        if let Some(event) = self.buffer.last_delta_event().cloned() {
                            if let Err(err) = self.folds.update_through_delta_event(&event) {
                                self.set_error(EngineError::Fold(err));
                                return;
                            }
                        }
                        self.selection = Selection::caret(new_caret);
                        self.last_message = None;
                    }
                    Err(err) => self.set_error(err),
                }
            }
            Err(err) => self.set_error(err),
        }
        cx.notify();
    }

    fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let head = self.selection.head();
        if let Ok(edit) = Edit::insert(head, text.to_string()) {
            let new_caret = CharOffset::new(head.get() + text.chars().count());
            self.apply_edits(vec![edit], new_caret, cx);
        }
    }

    fn backspace(&mut self, cx: &mut Context<Self>) {
        let head = self.selection.head();
        if head.get() == 0 {
            return;
        }
        let prev = CharOffset::new(head.get() - 1);
        if let Ok(range) = TextRange::new(prev, head) {
            self.apply_edits(vec![Edit::delete(range)], prev, cx);
        }
    }

    fn delete_forward(&mut self, cx: &mut Context<Self>) {
        let head = self.selection.head();
        let len = self.buffer.len_chars();
        if head >= len {
            return;
        }
        let next = CharOffset::new(head.get() + 1);
        if let Ok(range) = TextRange::new(head, next) {
            self.apply_edits(vec![Edit::delete(range)], head, cx);
        }
    }

    // ---------- 移动 ----------

    fn move_horizontally(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let head = self.selection.head();
        let new = if delta < 0 {
            head.get().saturating_sub((-delta) as usize)
        } else {
            (head.get() + delta as usize).min(self.buffer.len_chars().get())
        };
        let new_offset = CharOffset::new(new);
        self.update_selection(new_offset, extend);
        cx.notify();
    }

    fn move_vertically(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let head = self.selection.head();
        let position = match self.buffer.char_to_position(head) {
            Ok(pos) => pos,
            Err(err) => {
                self.set_error(err);
                cx.notify();
                return;
            }
        };
        let line_count = self.buffer.line_count();
        if line_count == 0 {
            return;
        }
        let mut target_line_value = (position.line().get() as isize + delta)
            .max(0)
            .min(line_count.saturating_sub(1) as isize)
            as usize;

        // 跳过 fold 隐藏的行 —— 沿移动方向找最近的可见行（落到 anchor 行）。
        if let Ok(projection) = self.projection() {
            let direction = if delta < 0 { -1isize } else { 1isize };
            loop {
                let line = Line::new(target_line_value);
                let mapping = match projection.logical_to_projected(line) {
                    Ok(m) => m,
                    Err(_) => break,
                };
                match mapping {
                    LogicalProjection::Visible(_) => break,
                    LogicalProjection::Hidden {
                        anchor_logical_line,
                        ..
                    } => {
                        if direction < 0 {
                            target_line_value = anchor_logical_line.get();
                            break;
                        }
                        let next = target_line_value as isize + 1;
                        if next as usize >= line_count {
                            target_line_value = anchor_logical_line.get();
                            break;
                        }
                        target_line_value = next as usize;
                    }
                }
            }
        }

        let target_line = Line::new(target_line_value);
        let snapshot = self.buffer.snapshot();
        let target_position = clamp_column(&snapshot, target_line, position.column().get());
        match self.buffer.position_to_char(target_position) {
            Ok(offset) => {
                self.update_selection(offset, extend);
                self.last_message = None;
            }
            Err(err) => self.set_error(err),
        }
        cx.notify();
    }

    fn move_to_line_edge(&mut self, to_end: bool, cx: &mut Context<Self>) {
        let head = self.selection.head();
        match self.buffer.char_to_position(head) {
            Ok(position) => {
                let line = position.line();
                let target_offset = if to_end {
                    line_end_offset(&self.buffer.snapshot(), line)
                } else {
                    self.buffer.line_start(line).unwrap_or(CharOffset::ZERO)
                };
                self.update_selection(target_offset, false);
            }
            Err(err) => self.set_error(err),
        }
        cx.notify();
    }

    fn update_selection(&mut self, new_head: CharOffset, extend: bool) {
        if extend {
            self.selection = self.selection.with_head(new_head);
        } else {
            self.selection = Selection::caret(new_head);
        }
    }

    // ---------- Fold 操作 ----------

    fn fold_selection(&mut self, cx: &mut Context<Self>) {
        let snapshot = self.buffer.snapshot();
        let range = self.selection.range();
        let start_line = match snapshot.char_to_position(range.start()) {
            Ok(p) => p.line(),
            Err(err) => {
                self.set_error(err);
                cx.notify();
                return;
            }
        };
        let end_line = match snapshot.char_to_position(range.end()) {
            Ok(p) => p.line(),
            Err(err) => {
                self.set_error(err);
                cx.notify();
                return;
            }
        };

        if start_line == end_line {
            self.last_message = Some(Message::Error(
                "选区只在 1 行内 — Cmd-F 至少需要选区横跨 ≥ 2 行才能产生 hidden 行（M13 语义：anchor 是首行，hidden 是后续行）。请 Shift+Down 多选几行后再试。"
                    .into(),
            ));
            cx.notify();
            return;
        }

        // selection 横跨 [start_line, end_line]，连同 end_line 自身一起折叠：
        // line_range(start_line, end_line + 1) → anchor=start_line, hidden=start_line+1..=end_line。
        let line_range_end = (end_line.get() + 1).min(self.buffer.line_count());
        match LineRange::new(start_line, Line::new(line_range_end))
            .map_err(EngineError::from)
            .and_then(|lr| self.folds.fold_lines(&self.buffer, lr))
        {
            Ok(_) => {
                self.last_message = Some(Message::Info(format!(
                    "已折叠 L{}..L{} (anchor L{}, 隐藏 {} 行)",
                    start_line.get(),
                    line_range_end,
                    start_line.get(),
                    line_range_end - start_line.get() - 1,
                )));
            }
            Err(err) => self.set_error(err),
        }
        cx.notify();
    }

    fn toggle_fold_at_cursor(&mut self, cx: &mut Context<Self>) {
        let head = self.selection.head();

        // 命中已有 fold（含 anchor 上对应的 char range）→ 展开
        if let Some(fold) = self
            .folds
            .as_slice()
            .iter()
            .find(|fold| fold_contains_offset(fold.range(), head))
            .copied()
        {
            self.folds.unfold(fold.id());
            self.last_message = Some(Message::Info(format!(
                "已展开 fold #{}（char {}..{}）",
                fold.id().get(),
                fold.range().start().get(),
                fold.range().end().get()
            )));
            cx.notify();
            return;
        }

        let position = match self.buffer.char_to_position(head) {
            Ok(p) => p,
            Err(err) => {
                self.set_error(err);
                cx.notify();
                return;
            }
        };

        // 折叠当前行 + 后 2 行 (共 3 行)，保证至少 2 行被隐藏 + 1 个 placeholder。
        let start_line = position.line();
        let line_count = self.buffer.line_count();
        let end_line_value = (start_line.get() + 3).min(line_count);
        if end_line_value <= start_line.get() + 1 {
            self.last_message = Some(Message::Error(
                "当前行附近不足 3 行可折叠。试 Cmd-F 自定义选区折叠。".into(),
            ));
            cx.notify();
            return;
        }
        let line_range =
            LineRange::new(start_line, Line::new(end_line_value)).expect("ordered line range");
        match self.folds.fold_lines(&self.buffer, line_range) {
            Ok(id) => {
                self.last_message = Some(Message::Info(format!(
                    "已折叠 L{}..L{}（fold #{}, anchor L{}, 隐藏 {} 行）",
                    start_line.get(),
                    end_line_value,
                    id.get(),
                    start_line.get(),
                    end_line_value - start_line.get() - 1,
                )));
            }
            Err(err) => self.set_error(err),
        }
        cx.notify();
    }

    fn unfold_all(&mut self, cx: &mut Context<Self>) {
        let count = self.folds.len();
        self.folds.unfold_all();
        self.last_message = Some(Message::Info(format!("已展开全部 {count} 条 fold。")));
        cx.notify();
    }

    fn set_error(&mut self, err: EngineError) {
        self.last_message = Some(Message::Error(err.to_string()));
    }
}

fn build_buffer() -> Buffer {
    Buffer::from_text(SAMPLE_TEXT.to_string(), BufferConfig::default())
        .expect("initial sample text should be a valid Buffer")
}

fn line_range(start: usize, end: usize) -> LineRange {
    LineRange::new(Line::new(start), Line::new(end)).expect("ordered line range")
}

fn fold_contains_offset(range: TextRange, offset: CharOffset) -> bool {
    if range.is_empty() {
        return false;
    }
    range.start() <= offset && offset < range.end()
}

fn line_end_offset(snapshot: &Snapshot, line: Line) -> CharOffset {
    let line_value = line.get();
    let line_count = snapshot.line_count();
    if line_value + 1 >= line_count {
        return snapshot.len_chars();
    }
    let next_start = match snapshot.line_start(Line::new(line_value + 1)) {
        Ok(s) => s,
        Err(_) => return snapshot.len_chars(),
    };
    let line_text = snapshot
        .slice_line(line)
        .map(|slice| slice.as_str().to_string())
        .unwrap_or_default();
    let newline = if line_text.ends_with("\r\n") {
        2
    } else if line_text.ends_with('\n') || line_text.ends_with('\r') {
        1
    } else {
        0
    };
    CharOffset::new(next_start.get().saturating_sub(newline))
}

fn clamp_column(snapshot: &Snapshot, line: Line, desired_col: usize) -> Position {
    let line_value = line.get();
    let line_start = snapshot
        .line_start(line)
        .unwrap_or(snapshot.len_chars())
        .get();
    let next_start = if line_value + 1 >= snapshot.line_count() {
        snapshot.len_chars().get()
    } else {
        snapshot
            .line_start(Line::new(line_value + 1))
            .map(|c| c.get())
            .unwrap_or(snapshot.len_chars().get())
    };
    let line_text = snapshot
        .slice_line(line)
        .map(|slice| slice.as_str().to_string())
        .unwrap_or_default();
    let newline_chars = if line_text.ends_with("\r\n") {
        2
    } else if line_text.ends_with('\n') || line_text.ends_with('\r') {
        1
    } else {
        0
    };
    let content_chars = next_start
        .saturating_sub(line_start)
        .saturating_sub(newline_chars);
    let column = desired_col.min(content_chars);
    Position::new(line, zom_engine::LogicalColumn::new(column))
}

impl Focusable for M13Testbed {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for M13Testbed {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let projection = match self.projection() {
            Ok(p) => p,
            Err(err) => return error_view(format!("Projection 构建失败：{err}")),
        };

        let snapshot = self.buffer.snapshot();
        let head = self.selection.head();
        let logical = snapshot
            .char_to_position(head)
            .map(LogicalPoint::from)
            .unwrap_or_else(|_| LogicalPoint::line_start(Line::new(0)));
        let projected_mapping = projection.logical_to_projected_point(logical).ok();

        let viewport = ProjectedViewport::new(ProjectedLineIndex::ZERO, projection.line_count());
        let viewport_slice = projection.slice_viewport(&snapshot, viewport).ok();
        let hidden_ranges = self
            .folds
            .derive_hidden_ranges(&self.buffer)
            .unwrap_or_default();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x111827))
            .text_color(rgb(0xE5E7EB))
            .p_4()
            .track_focus(&self.focus_handle)
            .tab_index(0)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                let key = &event.keystroke.key;
                let modifiers = &event.keystroke.modifiers;
                if modifiers.platform || modifiers.control || modifiers.alt {
                    return;
                }
                if key == "space" {
                    this.insert_text(" ", cx);
                } else if key == "tab" {
                    this.insert_text("\t", cx);
                } else if key.chars().count() == 1 {
                    this.insert_text(key, cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &MoveLeft, _, cx| this.move_horizontally(-1, false, cx)),
            )
            .on_action(
                cx.listener(|this, _: &MoveRight, _, cx| this.move_horizontally(1, false, cx)),
            )
            .on_action(cx.listener(|this, _: &MoveUp, _, cx| this.move_vertically(-1, false, cx)))
            .on_action(cx.listener(|this, _: &MoveDown, _, cx| this.move_vertically(1, false, cx)))
            .on_action(
                cx.listener(|this, _: &SelectLeft, _, cx| this.move_horizontally(-1, true, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectRight, _, cx| this.move_horizontally(1, true, cx)),
            )
            .on_action(cx.listener(|this, _: &SelectUp, _, cx| this.move_vertically(-1, true, cx)))
            .on_action(cx.listener(|this, _: &SelectDown, _, cx| this.move_vertically(1, true, cx)))
            .on_action(cx.listener(|this, _: &Home, _, cx| this.move_to_line_edge(false, cx)))
            .on_action(cx.listener(|this, _: &End, _, cx| this.move_to_line_edge(true, cx)))
            .on_action(cx.listener(|this, _: &Backspace, _, cx| this.backspace(cx)))
            .on_action(cx.listener(|this, _: &DeleteForward, _, cx| this.delete_forward(cx)))
            .on_action(cx.listener(|this, _: &Enter, _, cx| this.insert_text("\n", cx)))
            .on_action(cx.listener(|this, _: &FoldSelection, _, cx| this.fold_selection(cx)))
            .on_action(
                cx.listener(|this, _: &ToggleFoldAtCursor, _, cx| this.toggle_fold_at_cursor(cx)),
            )
            .on_action(cx.listener(|this, _: &UnfoldAll, _, cx| this.unfold_all(cx)))
            .on_action(cx.listener(|this, _: &DemoFolds, _, cx| this.demo_folds(cx)))
            .on_action(cx.listener(|this, _: &Reset, _, cx| this.reset(cx)))
            .on_action(cx.listener(|_this, _: &Quit, _, cx| cx.quit()))
            .child(header(
                self,
                &projection,
                logical,
                projected_mapping.as_ref(),
            ))
            .when_some(self.last_message.clone(), |el, msg| {
                el.child(message_view(msg))
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_4()
                    .mt_3()
                    .flex_grow()
                    .child(projected_view_panel(
                        viewport_slice.as_ref(),
                        self.selection,
                        logical,
                    ))
                    .child(debug_panel(self, &projection, &hidden_ranges)),
            )
            .child(footer())
    }
}

fn header(
    state: &M13Testbed,
    projection: &Projection,
    logical: LogicalPoint,
    projected_mapping: Option<&zom_engine::LogicalPointProjection>,
) -> gpui::Div {
    let projected_label = match projected_mapping {
        Some(zom_engine::LogicalPointProjection::Visible(point)) => format!(
            "可见 proj{}, 列 {}",
            point.line().get(),
            point.column().get()
        ),
        Some(zom_engine::LogicalPointProjection::Hidden {
            anchor_logical,
            anchor_projected,
        }) => format!(
            "隐藏 → anchor proj{} (logical L{})",
            anchor_projected.line().get(),
            anchor_logical.line().get()
        ),
        None => "无效".to_string(),
    };

    let placeholder_count = count_placeholders(projection);

    div()
        .border_b_1()
        .border_color(rgb(0x374151))
        .pb_2()
        .child(
            div()
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(rgb(0x60A5FA))
                .child("M13 Fold / Projection 体感台"),
        )
        .child(format!(
            "光标：char {}/{} · 逻辑 L{} 列 {} · {}",
            state.selection.head().get(),
            state.buffer.len_chars().get(),
            logical.line().get(),
            logical.column().get(),
            projected_label,
        ))
        .child(format!(
            "Buffer v{} · 逻辑 {} 行 · 投影 {} 行（含 {} placeholder）· FoldSet {} 条 · 选区 char[{}, {})",
            state.buffer.version().get(),
            state.buffer.line_count(),
            projection.line_count(),
            placeholder_count,
            state.folds.len(),
            state.selection.start().get(),
            state.selection.end().get(),
        ))
}

fn message_view(message: Message) -> gpui::Div {
    let (color, prefix, body) = match message {
        Message::Info(text) => (rgb(0x86EFAC), "提示", text),
        Message::Error(text) => (rgb(0xFCA5A5), "错误", text),
    };
    div()
        .my_2()
        .px_3()
        .py_2()
        .bg(rgb(0x1F2937))
        .rounded_md()
        .text_color(color)
        .child(format!("{prefix}：{body}"))
}

fn footer() -> gpui::Div {
    div()
        .mt_3()
        .pt_2()
        .border_t_1()
        .border_color(rgb(0x374151))
        .text_color(rgb(0x9CA3AF))
        .child("↑↓←→ 移动 / Shift+方向键扩展选区 / Home / End / 普通输入 / Backspace / Delete / Enter")
        .child("Cmd-F 折叠选区（≥2 行）／Cmd-T 折叠当前行+后 2 行（命中已有 fold 即展开）／Cmd-U 全部展开／Cmd-D 恢复演示 fold／Cmd-R 完整重置／Cmd-Q 退出")
}

fn projected_view_panel<'a>(
    slice: Option<&'a zom_engine::ProjectedViewportSlice<'a>>,
    selection: Selection,
    logical_cursor: LogicalPoint,
) -> gpui::Div {
    let mut panel = div()
        .flex()
        .flex_col()
        .min_w(px(640.0))
        .border_1()
        .border_color(rgb(0x1F2937))
        .rounded_md()
        .p_3()
        .bg(rgb(0x0B1220))
        .child(
            div()
                .text_color(rgb(0x93C5FD))
                .pb_2()
                .child("ProjectedViewport（折叠后视口）"),
        );

    let Some(slice) = slice else {
        return panel.child("(无可用 viewport 切片)");
    };
    if slice.is_empty() {
        return panel.child("(空)");
    }

    for row in slice.rows() {
        panel = panel.child(render_projected_row(row, selection, logical_cursor));
    }
    panel
}

fn render_projected_row(
    row: &ProjectedViewportRow<'_>,
    selection: Selection,
    logical_cursor: LogicalPoint,
) -> gpui::Div {
    let row_label = format!("proj{:>3}", row.index().get());

    match row.kind() {
        ProjectedViewportRowKind::Text {
            logical_line,
            visible,
        } => {
            let raw = visible.as_str().replace('\t', "  ");
            let on_cursor_line = logical_cursor.line() == *logical_line;
            let row_bg = if on_cursor_line {
                rgb(0x1E293B)
            } else {
                rgb(0x0B1220)
            };

            // 选区在该逻辑行上的列范围
            let (sel_start_col, sel_end_col) =
                selection_columns_on_line(visible.full_range(), selection);

            let mut text_row = div().flex().flex_row().min_h(px(24.0)).bg(row_bg);

            // gutter
            text_row = text_row
                .child(
                    div()
                        .min_w(px(60.0))
                        .text_color(rgb(0x6B7280))
                        .child(format!("L{:>3}", logical_line.get())),
                )
                .child(
                    div()
                        .min_w(px(64.0))
                        .text_color(rgb(0x4B5563))
                        .child(row_label),
                );

            // 行内字符 — 用 char index 控制 cursor 与 selection 高亮
            let chars: Vec<char> = raw.chars().collect();
            let cursor_col = if on_cursor_line {
                Some(logical_cursor.column().get())
            } else {
                None
            };

            let mut char_strip: Vec<gpui::AnyElement> = Vec::new();
            for (idx, ch) in chars.iter().enumerate() {
                if cursor_col == Some(idx) {
                    char_strip.push(cursor_marker().into_any());
                }
                let mut cell = div().child(visible_char(*ch));
                if let (Some(s), Some(e)) = (sel_start_col, sel_end_col) {
                    if idx >= s && idx < e {
                        cell = cell.bg(rgb(0x1D4ED8));
                    }
                }
                char_strip.push(cell.into_any());
            }
            // 行尾 cursor
            if cursor_col == Some(chars.len()) {
                char_strip.push(cursor_marker().into_any());
            }
            if visible.is_truncated() {
                char_strip.push(
                    div()
                        .ml_2()
                        .text_color(rgb(0xFB923C))
                        .child("…(截断)")
                        .into_any(),
                );
            }

            text_row.child(div().flex().flex_row().children(char_strip))
        }
        ProjectedViewportRowKind::Placeholder(placeholder) => div()
            .flex()
            .flex_row()
            .min_h(px(24.0))
            .bg(rgb(0x422006))
            .child(
                div()
                    .min_w(px(60.0))
                    .text_color(rgb(0xFB923C))
                    .child("…")
                    .into_any(),
            )
            .child(
                div()
                    .min_w(px(64.0))
                    .text_color(rgb(0x4B5563))
                    .child(row_label)
                    .into_any(),
            )
            .child(
                div()
                    .text_color(rgb(0xFB923C))
                    .child(format!(
                        "▶ 折叠 {} 行：anchor L{}，隐藏 L{}..L{}（exclusive）",
                        placeholder.hidden_line_count(),
                        placeholder.anchor_line().get(),
                        placeholder.hidden_lines().start().get(),
                        placeholder.hidden_lines().end().get(),
                    ))
                    .into_any(),
            ),
    }
}

fn selection_columns_on_line(
    line_range: TextRange,
    selection: Selection,
) -> (Option<usize>, Option<usize>) {
    if selection.is_caret() {
        return (None, None);
    }
    let sel_start = selection.start();
    let sel_end = selection.end();
    if sel_end <= line_range.start() || sel_start >= line_range.end() {
        return (None, None);
    }
    let line_start = line_range.start().get();
    let line_end = line_range.end().get();
    let s = sel_start.get().max(line_start) - line_start;
    let e = sel_end.get().min(line_end) - line_start;
    (Some(s), Some(e))
}

fn cursor_marker() -> gpui::Div {
    div().w(px(2.0)).h(px(20.0)).bg(rgb(0x60A5FA))
}

fn visible_char(c: char) -> String {
    match c {
        '\t' => "  ".to_string(),
        ' ' => "·".to_string(),
        _ => c.to_string(),
    }
}

fn debug_panel(
    state: &M13Testbed,
    projection: &Projection,
    hidden_ranges: &[HiddenRange],
) -> gpui::Div {
    let mut panel = div()
        .flex()
        .flex_col()
        .min_w(px(360.0))
        .border_1()
        .border_color(rgb(0x1F2937))
        .rounded_md()
        .p_3()
        .bg(rgb(0x0B1220))
        .child(div().text_color(rgb(0x86EFAC)).pb_2().child("调试面板"));

    panel = panel.child(div().pt_2().text_color(rgb(0xA7F3D0)).child("FoldSet"));
    if state.folds.is_empty() {
        panel = panel.child(div().text_color(rgb(0x6B7280)).child("(空)"));
    } else {
        for fold in state.folds.as_slice() {
            panel = panel.child(format_fold_range(fold, &state.buffer));
        }
    }

    panel = panel.child(div().pt_3().text_color(rgb(0xA7F3D0)).child("HiddenRange"));
    if hidden_ranges.is_empty() {
        panel = panel.child(div().text_color(rgb(0x6B7280)).child("(空)"));
    } else {
        for hidden in hidden_ranges {
            panel = panel.child(format!(
                "L{}..L{}（{} 行）",
                hidden.first_hidden_line().get(),
                hidden.end_line_exclusive().get(),
                hidden.len()
            ));
        }
    }

    panel = panel.child(
        div()
            .pt_3()
            .text_color(rgb(0xA7F3D0))
            .child("Projection 概览"),
    );
    panel = panel.child(format!(
        "投影行 {} · 逻辑行 {} · placeholder {}",
        projection.line_count(),
        projection.logical_line_count(),
        count_placeholders(projection),
    ));

    panel = panel.child(
        div()
            .pt_3()
            .text_color(rgb(0xA7F3D0))
            .child("LogicalProjection 摘要"),
    );
    let hidden_logical: Vec<String> = (0..projection.logical_line_count())
        .filter_map(|i| {
            let line = Line::new(i);
            match projection.logical_to_projected(line).ok()? {
                LogicalProjection::Hidden { .. } => Some(format!("L{i}")),
                LogicalProjection::Visible(_) => None,
            }
        })
        .collect();
    panel = panel.child(if hidden_logical.is_empty() {
        "隐藏逻辑行：(无)".to_string()
    } else {
        format!("隐藏逻辑行：{}", hidden_logical.join(","))
    });

    panel
}

fn format_fold_range(fold: &FoldRange, buffer: &Buffer) -> String {
    let range = fold.range();
    let start_line = buffer
        .char_to_position(range.start())
        .map(|p| p.line().get())
        .unwrap_or(0);
    let end_line = buffer
        .char_to_position(range.end())
        .map(|p| p.line().get())
        .unwrap_or(0);
    format!(
        "#{} char[{},{}) L{}..L{}",
        fold.id().get(),
        range.start().get(),
        range.end().get(),
        start_line,
        end_line
    )
}

fn count_placeholders(projection: &Projection) -> usize {
    projection
        .iter()
        .filter(|line| line.kind().is_placeholder())
        .count()
}

fn error_view(message: String) -> gpui::Div {
    div()
        .size_full()
        .bg(rgb(0x111827))
        .text_color(rgb(0xFCA5A5))
        .p_5()
        .child(message)
}

fn main() {
    Application::new().run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("left", MoveLeft, None),
            KeyBinding::new("right", MoveRight, None),
            KeyBinding::new("up", MoveUp, None),
            KeyBinding::new("down", MoveDown, None),
            KeyBinding::new("shift-left", SelectLeft, None),
            KeyBinding::new("shift-right", SelectRight, None),
            KeyBinding::new("shift-up", SelectUp, None),
            KeyBinding::new("shift-down", SelectDown, None),
            KeyBinding::new("home", Home, None),
            KeyBinding::new("end", End, None),
            KeyBinding::new("backspace", Backspace, None),
            KeyBinding::new("delete", DeleteForward, None),
            KeyBinding::new("enter", Enter, None),
            KeyBinding::new("cmd-f", FoldSelection, None),
            KeyBinding::new("cmd-t", ToggleFoldAtCursor, None),
            KeyBinding::new("cmd-u", UnfoldAll, None),
            KeyBinding::new("cmd-d", DemoFolds, None),
            KeyBinding::new("cmd-r", Reset, None),
            KeyBinding::new("cmd-q", Quit, None),
        ]);
        let bounds = Bounds::centered(None, size(px(1200.0), px(780.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_window, cx| cx.new(|cx| M13Testbed::new(cx)),
        )
        .unwrap();
        cx.activate(true);
    });
}
