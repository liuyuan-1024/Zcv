//! Editor 的逐帧文本布局、绘制与像素命中测试。

use std::sync::Arc;

use gpui::{
    App, Bounds, Context, DispatchPhase, Element, ElementId, ElementInputHandler, Entity,
    GlobalElementId, HitboxBehavior, InspectorElementId, InteractiveElement, IntoElement, LayoutId,
    MouseButton, MouseDownEvent, PaintQuad, Pixels, Point, ScrollWheelEvent, ShapedLine, Style,
    TextRun, UnderlineStyle, Window, fill, point, px, relative, size,
};
use zcv_engine::{Line, SelectionSet, Snapshot};

use super::display_map::{BufferPoint, DisplayRow};
use super::view::{Editor, EditorPresentation};
use crate::theme::color;

const CARET_WIDTH: Pixels = px(2.);

pub(super) struct EditorElement {
    editor: Entity<Editor>,
}

impl EditorElement {
    pub(super) fn new(editor: Entity<Editor>) -> Self {
        Self { editor }
    }

    pub(super) fn register_actions<E: InteractiveElement>(
        element: E,
        cx: &mut Context<Editor>,
    ) -> E {
        element
            .on_action(cx.listener(Editor::handle_move_left))
            .on_action(cx.listener(Editor::handle_move_right))
            .on_action(cx.listener(Editor::handle_move_up))
            .on_action(cx.listener(Editor::handle_move_down))
            .on_action(cx.listener(Editor::handle_move_to_previous_word))
            .on_action(cx.listener(Editor::handle_move_to_next_word))
            .on_action(cx.listener(Editor::handle_move_to_beginning_of_line))
            .on_action(cx.listener(Editor::handle_move_to_end_of_line))
            .on_action(cx.listener(Editor::handle_select_left))
            .on_action(cx.listener(Editor::handle_select_right))
            .on_action(cx.listener(Editor::handle_select_up))
            .on_action(cx.listener(Editor::handle_select_down))
            .on_action(cx.listener(Editor::handle_select_to_previous_word))
            .on_action(cx.listener(Editor::handle_select_to_next_word))
            .on_action(cx.listener(Editor::handle_select_to_beginning_of_line))
            .on_action(cx.listener(Editor::handle_select_to_end_of_line))
            .on_action(cx.listener(Editor::handle_select_all))
            .on_action(cx.listener(Editor::handle_backspace))
            .on_action(cx.listener(Editor::handle_delete))
            .on_action(cx.listener(Editor::handle_newline))
            .on_action(cx.listener(Editor::handle_undo))
            .on_action(cx.listener(Editor::handle_redo))
            .on_action(cx.listener(Editor::handle_cut))
            .on_action(cx.listener(Editor::handle_copy))
            .on_action(cx.listener(Editor::handle_paste))
    }
}

#[derive(Clone)]
struct LayoutLine {
    row: DisplayRow,
    origin: Point<Pixels>,
    shaped: ShapedLine,
    global_byte_start: usize,
    global_utf16_start: usize,
}

struct EditorLayout {
    lines: Vec<LayoutLine>,
    line_height: Pixels,
    snapshot: Snapshot,
    presentation: EditorPresentation,
}

impl EditorLayout {
    fn buffer_point_for_position(&self, position: Point<Pixels>) -> Option<BufferPoint> {
        let first = self.lines.first()?;
        let last = self.lines.last()?;
        let line = if position.y <= first.origin.y {
            first
        } else if position.y >= last.origin.y + self.line_height {
            last
        } else {
            self.lines
                .iter()
                .find(|line| position.y < line.origin.y + self.line_height)
                .unwrap_or(last)
        };

        let byte_index = line.shaped.closest_index_for_x(position.x - line.origin.x);
        let display_byte = line.global_byte_start + byte_index;
        let buffer_byte = self.presentation.display_byte_to_buffer_byte(display_byte);
        self.snapshot
            .byte_to_position(buffer_byte)
            .ok()
            .map(BufferPoint::from)
    }

    fn input_layout(&self) -> EditorInputLayout {
        EditorInputLayout {
            lines: self.lines.clone(),
            line_height: self.line_height,
        }
    }
}

#[derive(Clone)]
pub(super) struct EditorInputLayout {
    lines: Vec<LayoutLine>,
    line_height: Pixels,
}

impl EditorInputLayout {
    pub(super) fn bounds_for_utf16_range(
        &self,
        range: std::ops::Range<usize>,
    ) -> Option<Bounds<Pixels>> {
        let (start, start_row) = self.point_for_utf16(range.start)?;
        let (end, end_row) = self.point_for_utf16(range.end)?;
        if start_row != end_row {
            return Some(Bounds::new(start, size(Pixels::ZERO, self.line_height)));
        }
        Some(Bounds::from_corners(
            start,
            point(end.x, end.y + self.line_height),
        ))
    }

    pub(super) fn utf16_index_for_point(&self, point: Point<Pixels>) -> Option<usize> {
        let first = self.lines.first()?;
        let last = self.lines.last()?;
        let line = if point.y <= first.origin.y {
            first
        } else if point.y >= last.origin.y + self.line_height {
            last
        } else {
            self.lines
                .iter()
                .find(|line| point.y < line.origin.y + self.line_height)
                .unwrap_or(last)
        };
        let byte = line.shaped.closest_index_for_x(point.x - line.origin.x);
        Some(line.global_utf16_start + line.shaped.text[..byte].encode_utf16().count())
    }

    fn point_for_utf16(&self, index: usize) -> Option<(Point<Pixels>, DisplayRow)> {
        let line = self
            .lines
            .iter()
            .rev()
            .find(|line| line.global_utf16_start <= index)?;
        let local_utf16 = index.saturating_sub(line.global_utf16_start);
        let byte = byte_for_utf16_offset(&line.shaped.text, local_utf16)
            .unwrap_or_else(|| line.shaped.text.len());
        Some((
            point(line.origin.x + line.shaped.x_for_index(byte), line.origin.y),
            line.row,
        ))
    }
}

pub(super) struct PrepaintState {
    layout: Arc<EditorLayout>,
    selections: Vec<PaintQuad>,
    carets: Vec<PaintQuad>,
    ime_caret_bounds: Option<Bounds<Pixels>>,
    hitbox: gpui::Hitbox,
}

impl IntoElement for EditorElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let line_height = window.line_height();
        let (snapshot, presentation, selections, longest_row) = {
            let editor = self.editor.read(cx);
            (
                editor.render_snapshot(),
                editor.presentation(),
                editor.selections(),
                editor.longest_display_row(),
            )
        };
        let content_width = layout_line_width(&snapshot, longest_row, window) + CARET_WIDTH;
        self.editor.update(cx, |editor, _| {
            editor.prepare_scroll_viewport(bounds.size, content_width, line_height);
        });
        let (start_row, scroll_offset) = {
            let editor = self.editor.read(cx);
            (editor.scroll_anchor().row(), editor.scroll_offset())
        };
        let mut layout = layout_visible_lines(
            &snapshot,
            presentation.clone(),
            bounds,
            start_row,
            scroll_offset,
            line_height,
            window,
        );
        let mut ime_caret_bounds = layout_primary_caret(&selections, &layout, line_height);
        let autoscrolled = self.editor.update(cx, |editor, _| {
            editor.complete_autoscroll(
                ime_caret_bounds.map(|caret| caret.left() - bounds.left() + scroll_offset.x),
                ime_caret_bounds.map(|caret| caret.right() - bounds.left() + scroll_offset.x),
            )
        });
        if autoscrolled {
            let editor = self.editor.read(cx);
            layout = layout_visible_lines(
                &snapshot,
                presentation,
                bounds,
                editor.scroll_anchor().row(),
                editor.scroll_offset(),
                line_height,
                window,
            );
            ime_caret_bounds = layout_primary_caret(&selections, &layout, line_height);
        }
        let layout = Arc::new(layout);
        let (selections, carets) = layout_selections(&selections, &layout, line_height);

        PrepaintState {
            layout,
            selections,
            carets,
            ime_caret_bounds,
            hitbox: window.insert_hitbox(bounds, HitboxBehavior::Normal),
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.editor.read(cx).focus_handle();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.editor.clone()),
            cx,
        );
        let editor = self.editor.clone();
        let event_layout = Arc::clone(&prepaint.layout);
        let hitbox = prepaint.hitbox.clone();
        let mouse_focus = focus.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble
                || event.button != MouseButton::Left
                || !hitbox.is_hovered(window)
            {
                return;
            }
            let Some(point) = event_layout.buffer_point_for_position(event.position) else {
                return;
            };
            editor.update(cx, |editor, cx| {
                if let Ok(offset) = editor
                    .render_snapshot()
                    .position_to_byte(zcv_engine::Position::new(point.line(), point.column()))
                {
                    editor.set_caret(offset);
                    cx.notify();
                }
            });
            window.focus(&mouse_focus);
            cx.stop_propagation();
        });

        let scroll_editor = self.editor.clone();
        let scroll_hitbox = prepaint.hitbox.clone();
        let scroll_line_height = prepaint.layout.line_height;
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !scroll_hitbox.should_handle_scroll(window) {
                return;
            }
            let delta = event.delta.pixel_delta(scroll_line_height);
            let handled = scroll_editor.update(cx, |editor, cx| editor.scroll_by(delta, cx));
            if handled {
                cx.stop_propagation();
            }
        });

        for selection in prepaint.selections.drain(..) {
            window.paint_quad(selection);
        }
        for line in &prepaint.layout.lines {
            line.shaped
                .paint(line.origin, prepaint.layout.line_height, window, cx)
                .expect("Editor 文本行绘制失败");
        }
        if self.editor.read(cx).show_local_cursors(window, cx) {
            for caret in prepaint.carets.drain(..) {
                window.paint_quad(caret);
            }
        }
        let input_layout = prepaint.layout.input_layout();
        self.editor.update(cx, |editor, _| {
            editor.set_input_layout(input_layout);
            editor.set_ime_caret_geometry(bounds, prepaint.ime_caret_bounds);
        });
    }
}

fn layout_line_width(snapshot: &Snapshot, row: DisplayRow, window: &mut Window) -> Pixels {
    let Ok(line) = snapshot.slice_line(Line::new(row.get())) else {
        return Pixels::ZERO;
    };
    let text = line.as_str().trim_end_matches(['\r', '\n']);
    let text_style = window.text_style();
    let font_size = text_style.font_size.to_pixels(window.rem_size());
    let run = TextRun {
        len: text.len(),
        font: text_style.font(),
        color: text_style.color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(text.to_owned().into(), font_size, &[run], None)
        .width
}

fn layout_visible_lines(
    snapshot: &Snapshot,
    presentation: EditorPresentation,
    bounds: Bounds<Pixels>,
    start_row: DisplayRow,
    scroll_offset: Point<Pixels>,
    line_height: Pixels,
    window: &mut Window,
) -> EditorLayout {
    let line_count = presentation_lines(presentation.text()).count();
    let start = start_row.get().min(line_count.saturating_sub(1));
    let visible_count = ((bounds.size.height + scroll_offset.y) / line_height).ceil() as usize + 1;
    let end = (start + visible_count).min(line_count);
    let text_style = window.text_style();
    let font_size = text_style.font_size.to_pixels(window.rem_size());
    let mut lines = Vec::with_capacity(end.saturating_sub(start));

    for (row, presentation_line) in presentation_lines(presentation.text())
        .enumerate()
        .take(end)
        .skip(start)
    {
        let text = presentation_line.text;
        let runs = text_runs(
            text,
            presentation_line.byte_start,
            presentation.marked_byte_range(),
            TextRun {
                len: text.len(),
                font: text_style.font(),
                color: text_style.color,
                background_color: None,
                underline: None,
                strikethrough: None,
            },
        );
        let shaped =
            window
                .text_system()
                .shape_line(text.to_owned().into(), font_size, &runs, None);
        lines.push(LayoutLine {
            row: DisplayRow::new(row),
            origin: point(
                bounds.left() - scroll_offset.x,
                bounds.top() + line_height * (row - start) - scroll_offset.y,
            ),
            shaped,
            global_byte_start: presentation_line.byte_start,
            global_utf16_start: presentation_line.utf16_start,
        });
    }

    EditorLayout {
        lines,
        line_height,
        snapshot: snapshot.clone(),
        presentation,
    }
}

fn layout_selections(
    selections: &SelectionSet,
    layout: &EditorLayout,
    line_height: Pixels,
) -> (Vec<PaintQuad>, Vec<PaintQuad>) {
    let mut selection_quads = Vec::new();
    let mut caret_quads = Vec::new();

    if let Some(range_utf16) = layout.presentation.selected_range_utf16()
        && let Some(range) = byte_range_for_utf16(layout.presentation.text(), range_utf16)
    {
        layout_display_range(
            range.clone(),
            range.is_empty(),
            layout,
            line_height,
            &mut selection_quads,
            &mut caret_quads,
        );
        return (selection_quads, caret_quads);
    }

    for selection in selections.as_slice().iter().copied() {
        let start = layout
            .presentation
            .buffer_byte_to_display_byte(selection.start());
        let end = layout
            .presentation
            .buffer_byte_to_display_byte(selection.end());
        layout_display_range(
            start..end,
            selection.is_caret(),
            layout,
            line_height,
            &mut selection_quads,
            &mut caret_quads,
        );
    }

    (selection_quads, caret_quads)
}

fn layout_primary_caret(
    selections: &SelectionSet,
    layout: &EditorLayout,
    line_height: Pixels,
) -> Option<Bounds<Pixels>> {
    if let Some(range) = layout.presentation.selected_range_utf16() {
        return layout
            .input_layout()
            .bounds_for_utf16_range(range.end..range.end);
    }

    let display_byte = layout
        .presentation
        .buffer_byte_to_display_byte(selections.primary().head());
    let line = layout.lines.iter().find(|line| {
        line.global_byte_start <= display_byte
            && display_byte <= line.global_byte_start + line.shaped.len()
    })?;
    let local_byte = display_byte
        .saturating_sub(line.global_byte_start)
        .min(line.shaped.len());
    Some(Bounds::new(
        point(
            line.origin.x + line.shaped.x_for_index(local_byte),
            line.origin.y,
        ),
        size(px(2.), line_height),
    ))
}

fn layout_display_range(
    range: std::ops::Range<usize>,
    is_caret: bool,
    layout: &EditorLayout,
    line_height: Pixels,
    selection_quads: &mut Vec<PaintQuad>,
    caret_quads: &mut Vec<PaintQuad>,
) {
    if is_caret {
        let Some(line) = layout.lines.iter().find(|line| {
            line.global_byte_start <= range.start
                && range.start <= line.global_byte_start + line.shaped.len()
        }) else {
            return;
        };
        let local_byte = range
            .start
            .saturating_sub(line.global_byte_start)
            .min(line.shaped.len());
        caret_quads.push(fill(
            Bounds::new(
                point(
                    line.origin.x + line.shaped.x_for_index(local_byte),
                    line.origin.y,
                ),
                size(px(2.), line_height),
            ),
            color::current().blue.s[6],
        ));
        return;
    }

    for line in &layout.lines {
        let line_start = line.global_byte_start;
        let line_end = line_start + line.shaped.len();
        if range.end <= line_start || range.start > line_end {
            continue;
        }
        let local_start = range
            .start
            .saturating_sub(line_start)
            .min(line.shaped.len());
        let local_end = range.end.saturating_sub(line_start).min(line.shaped.len());
        let start_x = line.shaped.x_for_index(local_start);
        let mut end_x = line.shaped.x_for_index(local_end);
        if range.end > line_end && end_x <= start_x {
            end_x = start_x + px(8.);
        }
        if start_x == end_x {
            continue;
        }
        selection_quads.push(fill(
            Bounds::from_corners(
                point(line.origin.x + start_x, line.origin.y),
                point(line.origin.x + end_x, line.origin.y + line_height),
            ),
            color::current().blue.a[2],
        ));
    }
}

struct PresentationLine<'a> {
    text: &'a str,
    byte_start: usize,
    utf16_start: usize,
}

fn presentation_lines(text: &str) -> impl Iterator<Item = PresentationLine<'_>> {
    let mut byte_start = 0;
    let mut utf16_start = 0;
    let lines = text.split_inclusive('\n').map(move |part| {
        let visible = part.trim_end_matches(['\r', '\n']);
        let line = PresentationLine {
            text: visible,
            byte_start,
            utf16_start,
        };
        byte_start += part.len();
        utf16_start += part.encode_utf16().count();
        line
    });
    let trailing_line = (text.is_empty() || text.ends_with('\n')).then(|| PresentationLine {
        text: "",
        byte_start: text.len(),
        utf16_start: text.encode_utf16().count(),
    });
    lines.chain(trailing_line)
}

fn text_runs(
    text: &str,
    global_byte_start: usize,
    marked_range: Option<std::ops::Range<usize>>,
    base: TextRun,
) -> Vec<TextRun> {
    let Some(marked) = marked_range else {
        return vec![base];
    };
    let line_end = global_byte_start + text.len();
    let marked_start = marked.start.max(global_byte_start).min(line_end) - global_byte_start;
    let marked_end = marked.end.max(global_byte_start).min(line_end) - global_byte_start;
    if marked_start >= marked_end {
        return vec![base];
    }

    let mut runs = Vec::with_capacity(3);
    if marked_start > 0 {
        runs.push(TextRun {
            len: marked_start,
            ..base.clone()
        });
    }
    runs.push(TextRun {
        len: marked_end - marked_start,
        underline: Some(UnderlineStyle {
            color: Some(base.color),
            thickness: px(1.),
            wavy: false,
        }),
        ..base.clone()
    });
    if marked_end < text.len() {
        runs.push(TextRun {
            len: text.len() - marked_end,
            ..base
        });
    }
    runs
}

fn byte_range_for_utf16(
    text: &str,
    range: std::ops::Range<usize>,
) -> Option<std::ops::Range<usize>> {
    Some(byte_for_utf16_offset(text, range.start)?..byte_for_utf16_offset(text, range.end)?)
}

fn byte_for_utf16_offset(text: &str, target: usize) -> Option<usize> {
    let mut utf16_offset = 0;
    for (byte_offset, character) in text.char_indices() {
        if utf16_offset == target {
            return Some(byte_offset);
        }
        utf16_offset += character.len_utf16();
        if utf16_offset > target {
            return None;
        }
    }
    (utf16_offset == target).then_some(text.len())
}

#[cfg(test)]
fn column_to_byte(text: &str, column: usize) -> usize {
    text.char_indices()
        .nth(column)
        .map_or(text.len(), |(byte, _)| byte)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::DisplayPoint;
    use gpui::{Empty, TestAppContext, font};
    use zcv_engine::{Buffer, BufferConfig, ByteOffset, DisplayColumn, Line, LogicalColumn};

    #[test]
    fn logical_columns_map_to_utf8_boundaries() {
        let text = "a你😀";
        assert_eq!(column_to_byte(text, 0), 0);
        assert_eq!(column_to_byte(text, 1), 1);
        assert_eq!(column_to_byte(text, 2), 4);
        assert_eq!(column_to_byte(text, 3), 8);
        assert_eq!(column_to_byte(text, 99), 8);
    }

    #[test]
    fn marked_text_is_a_separate_underlined_text_run() {
        let text = "a中文b";
        let runs = text_runs(
            text,
            0,
            Some(1..7),
            TextRun {
                len: text.len(),
                font: font("Helvetica"),
                color: Default::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            },
        );

        assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), text.len());
        assert_eq!(runs.len(), 3);
        assert!(runs[0].underline.is_none());
        assert!(runs[1].underline.is_some());
        assert!(runs[2].underline.is_none());
    }

    #[gpui::test]
    fn hit_test_uses_the_same_shaped_line_measurement_as_cursor_paint(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Empty);
        window
            .update(cx, |_, window, _| {
                let text = "a你b";
                let shaped = window.text_system().shape_line(
                    text.into(),
                    px(16.),
                    &[TextRun {
                        len: text.len(),
                        font: font("Helvetica"),
                        color: Default::default(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    None,
                );
                let cursor_x = shaped.x_for_index(column_to_byte(text, 2));
                let snapshot = Buffer::scratch(text.to_owned(), BufferConfig::default())
                    .expect("测试 Buffer 应能创建")
                    .snapshot();
                let layout = EditorLayout {
                    lines: vec![LayoutLine {
                        row: DisplayRow::new(3),
                        origin: point(px(10.), px(20.)),
                        shaped,
                        global_byte_start: 0,
                        global_utf16_start: 0,
                    }],
                    line_height: px(24.),
                    presentation: EditorPresentation::new(&snapshot, None),
                    snapshot,
                };

                assert_eq!(
                    layout.buffer_point_for_position(point(px(10.) + cursor_x, px(32.))),
                    Some(BufferPoint::new(Line::ZERO, LogicalColumn::new(2)))
                );
                assert_eq!(
                    DisplayPoint::new(DisplayRow::new(3), DisplayColumn::new(2)).row(),
                    DisplayRow::new(3)
                );
            })
            .expect("测试窗口应保持可用");
    }

    #[gpui::test]
    fn large_buffer_layout_shapes_only_visible_rows(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Empty);
        window
            .update(cx, |_, window, _| {
                let text = (0..10_000)
                    .map(|row| format!("line {row}\n"))
                    .collect::<String>();
                let snapshot = Buffer::scratch(text, BufferConfig::default())
                    .expect("大文本测试 Buffer 应能创建")
                    .snapshot();
                let presentation = EditorPresentation::new(&snapshot, None);
                let layout = layout_visible_lines(
                    &snapshot,
                    presentation,
                    Bounds::new(point(px(0.), px(0.)), size(px(800.), px(100.))),
                    DisplayRow::new(5_000),
                    point(px(0.), px(10.)),
                    px(20.),
                    window,
                );

                assert_eq!(
                    layout.lines.first().map(|line| line.row),
                    Some(DisplayRow::new(5_000))
                );
                assert_eq!(
                    layout.lines.last().map(|line| line.row),
                    Some(DisplayRow::new(5_006))
                );
                assert_eq!(layout.lines.len(), 7);
            })
            .expect("测试窗口应保持可用");
    }

    #[gpui::test]
    fn caret_outside_visible_rows_is_not_painted_on_viewport_edge(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Empty);
        window
            .update(cx, |_, window, _| {
                let snapshot = Buffer::scratch(
                    (0..20).map(|_| "x\n").collect::<String>(),
                    BufferConfig::default(),
                )
                .expect("测试 Buffer 应能创建")
                .snapshot();
                let presentation = EditorPresentation::new(&snapshot, None);
                let layout = layout_visible_lines(
                    &snapshot,
                    presentation,
                    Bounds::new(point(px(0.), px(0.)), size(px(200.), px(40.))),
                    DisplayRow::new(10),
                    point(px(0.), px(0.)),
                    px(20.),
                    window,
                );
                let (_, carets) =
                    layout_selections(&SelectionSet::caret(ByteOffset::ZERO), &layout, px(20.));

                assert!(carets.is_empty());
            })
            .expect("测试窗口应保持可用");
    }
}
