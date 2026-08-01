//! Editor 的逐帧文本布局、绘制与像素命中测试。

use std::collections::BTreeSet;
use std::sync::Arc;

use gpui::{
    App, Bounds, ContentMask, Context, DispatchPhase, Element, ElementId, ElementInputHandler,
    Entity, GlobalElementId, HitboxBehavior, InspectorElementId, InteractiveElement, IntoElement,
    LayoutId, MouseButton, MouseDownEvent, PaintQuad, Pixels, Point, ScrollWheelEvent, ShapedLine,
    Style, TextRun, UnderlineStyle, Window, fill, point, px, relative, size,
};
use zcv_engine::{ByteOffset, DisplayColumn, Line, SelectionSet};

use super::display_map::{
    BufferPoint, DisplayPoint, DisplayRow, DisplaySnapshot, ProjectedRange,
    ProjectedViewportRowKind,
};
use super::gutter::{GutterDimensions, GutterLayout, GutterRow};
use super::view::{Editor, EditorPresentation};
use zcv_theme::color;

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
            .on_action(cx.listener(Editor::handle_move_to_beginning))
            .on_action(cx.listener(Editor::handle_move_to_end))
            .on_action(cx.listener(Editor::handle_move_page_up))
            .on_action(cx.listener(Editor::handle_move_page_down))
            .on_action(cx.listener(Editor::handle_select_left))
            .on_action(cx.listener(Editor::handle_select_right))
            .on_action(cx.listener(Editor::handle_select_up))
            .on_action(cx.listener(Editor::handle_select_down))
            .on_action(cx.listener(Editor::handle_select_to_previous_word))
            .on_action(cx.listener(Editor::handle_select_to_next_word))
            .on_action(cx.listener(Editor::handle_select_to_beginning_of_line))
            .on_action(cx.listener(Editor::handle_select_to_end_of_line))
            .on_action(cx.listener(Editor::handle_select_to_beginning))
            .on_action(cx.listener(Editor::handle_select_to_end))
            .on_action(cx.listener(Editor::handle_select_page_up))
            .on_action(cx.listener(Editor::handle_select_page_down))
            .on_action(cx.listener(Editor::handle_select_all))
            .on_action(cx.listener(Editor::handle_backspace))
            .on_action(cx.listener(Editor::handle_delete))
            .on_action(cx.listener(Editor::handle_delete_to_previous_word_start))
            .on_action(cx.listener(Editor::handle_delete_to_next_word_end))
            .on_action(cx.listener(Editor::handle_delete_to_beginning_of_line))
            .on_action(cx.listener(Editor::handle_delete_to_end_of_line))
            .on_action(cx.listener(Editor::handle_newline))
            .on_action(cx.listener(Editor::handle_undo))
            .on_action(cx.listener(Editor::handle_redo))
            .on_action(cx.listener(Editor::handle_cut))
            .on_action(cx.listener(Editor::handle_copy))
            .on_action(cx.listener(Editor::handle_paste))
            .on_action(cx.listener(Editor::handle_indent))
            .on_action(cx.listener(Editor::handle_outdent))
    }
}

#[derive(Clone)]
struct LayoutLine {
    row: DisplayRow,
    logical_line: Option<Line>,
    origin: Point<Pixels>,
    shaped: ShapedLine,
    global_byte_start: usize,
    global_utf16_start: usize,
}

struct EditorLayout {
    lines: Vec<LayoutLine>,
    gutter: Option<GutterLayout>,
    text_clip_bounds: Bounds<Pixels>,
    line_height: Pixels,
    display_snapshot: DisplaySnapshot,
    presentation: EditorPresentation,
}

#[derive(Clone, Copy)]
struct EditorGeometry {
    text_bounds: Bounds<Pixels>,
    text_clip_bounds: Bounds<Pixels>,
    gutter: Option<(Bounds<Pixels>, GutterDimensions)>,
}

struct VisibleLineLayoutParams<'a> {
    geometry: EditorGeometry,
    active_lines: &'a BTreeSet<Line>,
    start_row: DisplayRow,
    scroll_offset: Point<Pixels>,
    line_height: Pixels,
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
        if self.presentation.is_composing() {
            let display_byte = line.global_byte_start + byte_index;
            let buffer_byte = self.presentation.display_byte_to_buffer_byte(display_byte);
            return self
                .display_snapshot
                .buffer_snapshot()
                .byte_to_position(buffer_byte)
                .ok()
                .map(BufferPoint::from);
        }
        if let Some(logical_line) = line.logical_line {
            return Some(BufferPoint::new(
                logical_line,
                zcv_engine::LogicalColumn::new(line.shaped.text[..byte_index].chars().count()),
            ));
        }
        let offset = self
            .display_snapshot
            .display_point_to_offset(DisplayPoint::new(line.row, DisplayColumn::ZERO))
            .ok()?;
        self.display_snapshot
            .buffer_snapshot()
            .byte_to_position(offset)
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
    gutter_hitbox: Option<gpui::Hitbox>,
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
        let visible_line_count = (bounds.size.height / line_height).ceil() as usize + 2;
        self.editor.update(cx, |editor, _| {
            editor.measure_display_rows(editor.scroll_anchor().row(), visible_line_count);
        });
        let (display_snapshot, presentation, selections, longest_row, shows_gutter, active_lines) = {
            let editor = self.editor.read(cx);
            (
                editor.display_snapshot(),
                editor.presentation(),
                editor.selections(),
                editor.longest_display_row(),
                editor.shows_gutter(),
                editor.active_lines().into_iter().collect::<BTreeSet<_>>(),
            )
        };
        let gutter_dimensions = shows_gutter.then(|| gutter_dimensions(&display_snapshot, window));
        let gutter_bounds = gutter_dimensions.map(|dimensions| Bounds {
            origin: bounds.origin,
            size: size(dimensions.width, bounds.size.height),
        });
        let text_left =
            bounds.left() + gutter_dimensions.map_or(Pixels::ZERO, GutterDimensions::full_width);
        let text_clip_left =
            bounds.left() + gutter_dimensions.map_or(Pixels::ZERO, |dimensions| dimensions.width);
        let text_bounds = Bounds {
            origin: point(text_left, bounds.top()),
            size: size(
                (bounds.right() - text_left).max(Pixels::ZERO),
                bounds.size.height,
            ),
        };
        let geometry = EditorGeometry {
            text_bounds,
            text_clip_bounds: Bounds {
                origin: point(text_clip_left, bounds.top()),
                size: size(
                    (bounds.right() - text_clip_left).max(Pixels::ZERO),
                    bounds.size.height,
                ),
            },
            gutter: gutter_bounds.zip(gutter_dimensions),
        };
        let content_width = layout_line_width(&display_snapshot, longest_row, window) + CARET_WIDTH;
        self.editor.update(cx, |editor, _| {
            editor.prepare_scroll_viewport(text_bounds.size, content_width, line_height);
        });
        let (start_row, scroll_offset) = {
            let editor = self.editor.read(cx);
            (editor.scroll_anchor().row(), editor.scroll_offset())
        };
        let mut layout = layout_visible_lines(
            display_snapshot.clone(),
            presentation.clone(),
            VisibleLineLayoutParams {
                geometry,
                active_lines: &active_lines,
                start_row,
                scroll_offset,
                line_height,
            },
            window,
        );
        let mut ime_caret_bounds = layout_primary_caret(&selections, &layout, line_height);
        let autoscrolled = self.editor.update(cx, |editor, _| {
            editor.complete_autoscroll(
                ime_caret_bounds.map(|caret| caret.left() - text_bounds.left() + scroll_offset.x),
                ime_caret_bounds.map(|caret| caret.right() - text_bounds.left() + scroll_offset.x),
            )
        });
        if autoscrolled {
            let editor = self.editor.read(cx);
            layout = layout_visible_lines(
                display_snapshot,
                presentation,
                VisibleLineLayoutParams {
                    geometry,
                    active_lines: &active_lines,
                    start_row: editor.scroll_anchor().row(),
                    scroll_offset: editor.scroll_offset(),
                    line_height,
                },
                window,
            );
            ime_caret_bounds = layout_primary_caret(&selections, &layout, line_height);
        }
        let layout = Arc::new(layout);
        let (selections, carets) = layout_selections(&selections, &layout, line_height);
        let gutter_hitbox = layout
            .gutter
            .as_ref()
            .map(|gutter| window.insert_hitbox(gutter.bounds, HitboxBehavior::Normal));

        PrepaintState {
            layout,
            selections,
            carets,
            ime_caret_bounds,
            hitbox: window.insert_hitbox(bounds, HitboxBehavior::Normal),
            gutter_hitbox,
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
            if let Some(gutter) = &event_layout.gutter {
                if let Some(line) = gutter.logical_line_for_position(event.position) {
                    editor.update(cx, |editor, cx| {
                        editor.select_line(line, event.modifiers.shift);
                        cx.notify();
                    });
                    window.focus(&mouse_focus);
                    cx.stop_propagation();
                    return;
                }
                if gutter.bounds.contains(&event.position) {
                    return;
                }
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

        if let Some(gutter) = &prepaint.layout.gutter {
            window.paint_quad(fill(
                gutter.bounds,
                color::current().editor_gutter_background,
            ));
            for bounds in gutter.active_row_bounds(prepaint.layout.text_clip_bounds.right()) {
                window.paint_quad(fill(bounds, color::current().editor_active_line_background));
            }
            if let Some(hitbox) = &prepaint.gutter_hitbox {
                window.set_cursor_style(gpui::CursorStyle::IBeam, hitbox);
            }
        }
        if let Some(gutter) = &prepaint.layout.gutter {
            for row in &gutter.rows {
                row.shaped_line_number
                    .paint(row.origin, gutter.line_height, window, cx)
                    .expect("Editor gutter 行号绘制失败");
            }
        }
        let show_local_cursors = self.editor.read(cx).show_local_cursors(window, cx);
        window.with_content_mask(
            Some(ContentMask {
                bounds: prepaint.layout.text_clip_bounds,
            }),
            |window| {
                for selection in prepaint.selections.drain(..) {
                    window.paint_quad(selection);
                }
                for line in &prepaint.layout.lines {
                    line.shaped
                        .paint(line.origin, prepaint.layout.line_height, window, cx)
                        .expect("Editor 文本行绘制失败");
                }
                if show_local_cursors {
                    for caret in prepaint.carets.drain(..) {
                        window.paint_quad(caret);
                    }
                }
            },
        );
        let input_layout = prepaint.layout.input_layout();
        self.editor.update(cx, |editor, _| {
            editor.set_input_layout(input_layout);
            editor.set_ime_caret_geometry(bounds, prepaint.ime_caret_bounds);
        });
    }
}

fn layout_line_width(
    display_snapshot: &DisplaySnapshot,
    row: DisplayRow,
    window: &mut Window,
) -> Pixels {
    let Ok(viewport) = display_snapshot.slice_viewport(row, 1) else {
        return Pixels::ZERO;
    };
    let Some(row) = viewport.rows().first() else {
        return Pixels::ZERO;
    };
    let text = match row.kind() {
        ProjectedViewportRowKind::Text { visible, .. } => visible.as_str(),
        ProjectedViewportRowKind::Placeholder(_) => "…",
    };
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
    display_snapshot: DisplaySnapshot,
    presentation: EditorPresentation,
    params: VisibleLineLayoutParams<'_>,
    window: &mut Window,
) -> EditorLayout {
    let VisibleLineLayoutParams {
        geometry:
            EditorGeometry {
                text_bounds,
                text_clip_bounds,
                gutter: gutter_geometry,
            },
        active_lines,
        start_row,
        scroll_offset,
        line_height,
    } = params;
    let line_count = presentation
        .composed_line_count()
        .unwrap_or_else(|| display_snapshot.line_count());
    let start = start_row.get().min(line_count.saturating_sub(1));
    let visible_count =
        ((text_bounds.size.height + scroll_offset.y) / line_height).ceil() as usize + 1;
    let end = (start + visible_count).min(line_count);
    let text_style = window.text_style();
    let font_size = text_style.font_size.to_pixels(window.rem_size());
    let mut lines = Vec::with_capacity(end.saturating_sub(start));
    let mut gutter_rows = Vec::with_capacity(end.saturating_sub(start));

    let mut push_line = |row: usize,
                         logical_line: Option<Line>,
                         gutter_line: Option<Line>,
                         text: &str,
                         byte_start: usize,
                         utf16_start: usize| {
        let runs = text_runs(
            text,
            byte_start,
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
            logical_line,
            origin: point(
                text_bounds.left() - scroll_offset.x,
                text_bounds.top() + line_height * (row - start) - scroll_offset.y,
            ),
            shaped,
            global_byte_start: byte_start,
            global_utf16_start: utf16_start,
        });
        if let (Some(logical_line), Some((gutter_bounds, dimensions))) =
            (gutter_line, gutter_geometry)
        {
            let number = (logical_line.get() + 1).to_string();
            let active = active_lines.contains(&logical_line);
            let run = TextRun {
                len: number.len(),
                font: text_style.font(),
                color: if active {
                    color::current().editor_active_line_number.into()
                } else {
                    color::current().editor_line_number.into()
                },
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped_line_number =
                window
                    .text_system()
                    .shape_line(number.into(), font_size, &[run], None);
            gutter_rows.push(GutterRow {
                logical_line,
                origin: point(
                    gutter_bounds.right() - dimensions.right_padding - shaped_line_number.width,
                    text_bounds.top() + line_height * (row - start) - scroll_offset.y,
                ),
                shaped_line_number,
                active,
            });
        }
    };

    if presentation.is_composing() {
        for row in start..end {
            let Some(line) = presentation.composed_line(row) else {
                continue;
            };
            let gutter_line = display_snapshot
                .buffer_snapshot()
                .byte_to_line(presentation.display_byte_to_buffer_byte(line.byte_start))
                .ok();
            push_line(
                row,
                None,
                gutter_line,
                line.text,
                line.byte_start,
                line.utf16_start,
            );
        }
    } else if let Ok(viewport) =
        display_snapshot.slice_viewport(DisplayRow::new(start), end.saturating_sub(start))
    {
        for row in viewport.rows() {
            match row.kind() {
                ProjectedViewportRowKind::Text {
                    logical_line,
                    visible,
                } => {
                    let byte_start = visible.visible_range().start().get();
                    let utf16_start = display_snapshot
                        .buffer_snapshot()
                        .byte_to_utf16_cu(ByteOffset::new(byte_start))
                        .map_or(0, |offset| offset.get());
                    push_line(
                        row.index().get(),
                        Some(*logical_line),
                        Some(*logical_line),
                        visible.as_str(),
                        byte_start,
                        utf16_start,
                    );
                }
                ProjectedViewportRowKind::Placeholder(placeholder) => {
                    let byte_start = display_snapshot
                        .buffer_snapshot()
                        .line_start_byte(placeholder.hidden_lines().start())
                        .map_or(0, ByteOffset::get);
                    let utf16_start = display_snapshot
                        .buffer_snapshot()
                        .byte_to_utf16_cu(ByteOffset::new(byte_start))
                        .map_or(0, |offset| offset.get());
                    push_line(
                        row.index().get(),
                        None,
                        Some(placeholder.hidden_lines().start()),
                        "…",
                        byte_start,
                        utf16_start,
                    );
                }
            }
        }
    }

    EditorLayout {
        lines,
        gutter: gutter_geometry.map(|(bounds, _)| GutterLayout {
            bounds,
            line_height,
            rows: gutter_rows,
        }),
        text_clip_bounds,
        line_height,
        display_snapshot,
        presentation,
    }
}

fn gutter_dimensions(display_snapshot: &DisplaySnapshot, window: &mut Window) -> GutterDimensions {
    let text_style = window.text_style();
    let font_size = text_style.font_size.to_pixels(window.rem_size());
    let digits = "0000000000";
    let run = TextRun {
        len: digits.len(),
        font: text_style.font(),
        color: text_style.color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let shaped_digits = window
        .text_system()
        .shape_line(digits.into(), font_size, &[run], None);
    GutterDimensions::line_numbers_only(
        display_snapshot.buffer_snapshot().line_count(),
        shaped_digits.width / digits.len() as f32,
        shaped_digits.descent,
    )
}

fn layout_selections(
    selections: &SelectionSet,
    layout: &EditorLayout,
    line_height: Pixels,
) -> (Vec<PaintQuad>, Vec<PaintQuad>) {
    let mut selection_quads = Vec::new();
    let mut caret_quads = Vec::new();

    if let Some(range_utf16) = layout.presentation.selected_range_utf16()
        && let Some(text) = layout.presentation.composed_text()
        && let Some(range) = byte_range_for_utf16(text, range_utf16)
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
        if selection.is_caret() {
            if let Some(caret) =
                layout_caret_at_buffer_offset(selection.head(), layout, line_height)
            {
                caret_quads.push(caret);
            }
            continue;
        }
        if !layout.presentation.is_composing()
            && let Ok(ranges) = layout
                .display_snapshot
                .project_text_range(selection.range())
        {
            for range in ranges {
                layout_projected_range(range, layout, line_height, &mut selection_quads);
            }
            continue;
        }
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

fn layout_projected_range(
    range: ProjectedRange,
    layout: &EditorLayout,
    line_height: Pixels,
    selection_quads: &mut Vec<PaintQuad>,
) {
    let start = range.start();
    let end = range.end();
    for line in &layout.lines {
        let row = super::display_map::ProjectedLineIndex::new(line.row.get());
        if row < start.line() || row > end.line() {
            continue;
        }
        if row == end.line()
            && row != start.line()
            && end.column() == zcv_engine::LogicalColumn::ZERO
        {
            continue;
        }

        let line_columns = line.shaped.text.chars().count();
        let start_column = if row == start.line() {
            start.column().get().min(line_columns)
        } else {
            0
        };
        let end_column = if row == end.line() {
            end.column().get().min(line_columns)
        } else {
            line_columns
        };
        let (local_start, local_end) = if line.logical_line.is_none() {
            (0, line.shaped.len())
        } else {
            (
                column_to_byte(&line.shaped.text, start_column),
                column_to_byte(&line.shaped.text, end_column),
            )
        };
        let start_x = line.shaped.x_for_index(local_start);
        let mut end_x = line.shaped.x_for_index(local_end);
        if end_x <= start_x && row != end.line() {
            end_x = start_x + px(8.);
        }
        if end_x <= start_x {
            continue;
        }
        selection_quads.push(fill(
            Bounds::from_corners(
                point(line.origin.x + start_x, line.origin.y),
                point(line.origin.x + end_x, line.origin.y + line_height),
            ),
            color::current().editor_selection_background,
        ));
    }
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

    let head = selections.primary().head();
    let display_point = layout.display_snapshot.offset_to_display_point(head).ok()?;
    let line = layout
        .lines
        .iter()
        .find(|line| line.row == display_point.row())?;
    let local_byte = local_byte_for_display_point(line, display_point, &layout.display_snapshot);
    Some(Bounds::new(
        point(
            line.origin.x + line.shaped.x_for_index(local_byte),
            line.origin.y,
        ),
        size(px(2.), line_height),
    ))
}

fn layout_caret_at_buffer_offset(
    offset: ByteOffset,
    layout: &EditorLayout,
    line_height: Pixels,
) -> Option<PaintQuad> {
    let display_point = layout
        .display_snapshot
        .offset_to_display_point(offset)
        .ok()?;
    let line = layout
        .lines
        .iter()
        .find(|line| line.row == display_point.row())?;
    let local_byte = local_byte_for_display_point(line, display_point, &layout.display_snapshot);
    Some(fill(
        Bounds::new(
            point(
                line.origin.x + line.shaped.x_for_index(local_byte),
                line.origin.y,
            ),
            size(px(2.), line_height),
        ),
        color::current().editor_cursor,
    ))
}

fn local_byte_for_display_point(
    line: &LayoutLine,
    point: DisplayPoint,
    display_snapshot: &DisplaySnapshot,
) -> usize {
    let logical_column = line
        .logical_line
        .and_then(|logical_line| {
            display_snapshot
                .display_to_logical_column(logical_line, point.column())
                .ok()
        })
        .map_or(0, zcv_engine::LogicalColumn::get);
    column_to_byte(&line.shaped.text, logical_column)
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
            color::current().editor_cursor,
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
            color::current().editor_selection_background,
        ));
    }
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

fn column_to_byte(text: &str, column: usize) -> usize {
    text.char_indices()
        .nth(column)
        .map_or(text.len(), |(byte, _)| byte)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display_map::{DisplayMap, DisplayPoint};
    use gpui::{Empty, TestAppContext, font};
    use zcv_engine::{
        Buffer, BufferConfig, ByteOffset, DisplayColumn, Line, LineRange, LogicalColumn,
    };

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
                let display_snapshot = DisplayMap::new(snapshot.clone()).snapshot();
                let layout = EditorLayout {
                    lines: vec![LayoutLine {
                        row: DisplayRow::new(3),
                        logical_line: Some(Line::ZERO),
                        origin: point(px(10.), px(20.)),
                        shaped,
                        global_byte_start: 0,
                        global_utf16_start: 0,
                    }],
                    gutter: None,
                    text_clip_bounds: Bounds::new(point(px(0.), px(0.)), size(px(400.), px(100.))),
                    line_height: px(24.),
                    presentation: EditorPresentation::new(&snapshot, None),
                    display_snapshot,
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
                let display_snapshot = DisplayMap::new(snapshot.clone()).snapshot();
                let layout = layout_visible_lines(
                    display_snapshot,
                    presentation,
                    VisibleLineLayoutParams {
                        geometry: EditorGeometry {
                            text_bounds: Bounds::new(
                                point(px(0.), px(0.)),
                                size(px(800.), px(100.)),
                            ),
                            text_clip_bounds: Bounds::new(
                                point(px(0.), px(0.)),
                                size(px(800.), px(100.)),
                            ),
                            gutter: None,
                        },
                        active_lines: &BTreeSet::new(),
                        start_row: DisplayRow::new(5_000),
                        scroll_offset: point(px(0.), px(10.)),
                        line_height: px(20.),
                    },
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
    fn gutter_and_text_share_vertical_rows_but_only_text_scrolls_horizontally(
        cx: &mut TestAppContext,
    ) {
        let window = cx.add_window(|_, _| Empty);
        window
            .update(cx, |_, window, _| {
                let snapshot =
                    Buffer::scratch("one\ntwo\nthree".to_owned(), BufferConfig::default())
                        .expect("测试 Buffer 应能创建")
                        .snapshot();
                let dimensions = GutterDimensions {
                    left_padding: px(8.),
                    right_padding: px(8.),
                    width: px(48.),
                    margin: px(3.),
                };
                let gutter_bounds =
                    Bounds::new(point(px(0.), px(0.)), size(dimensions.width, px(100.)));
                let text_bounds = Bounds::new(point(px(51.), px(0.)), size(px(349.), px(100.)));
                let layout = layout_visible_lines(
                    DisplayMap::new(snapshot.clone()).snapshot(),
                    EditorPresentation::new(&snapshot, None),
                    VisibleLineLayoutParams {
                        geometry: EditorGeometry {
                            text_bounds,
                            text_clip_bounds: Bounds::new(
                                point(px(48.), px(0.)),
                                size(px(352.), px(100.)),
                            ),
                            gutter: Some((gutter_bounds, dimensions)),
                        },
                        active_lines: &BTreeSet::from([Line::new(1)]),
                        start_row: DisplayRow::ZERO,
                        scroll_offset: point(px(20.), px(0.)),
                        line_height: px(20.),
                    },
                    window,
                );
                let gutter = layout.gutter.as_ref().expect("Full Editor 应布局 gutter");

                assert_eq!(layout.lines[0].origin.x, px(31.));
                assert_eq!(gutter.rows[0].shaped_line_number.text.as_ref(), "1");
                assert_eq!(gutter.rows[1].shaped_line_number.text.as_ref(), "2");
                assert!(!gutter.rows[0].active);
                assert!(gutter.rows[1].active);
                assert_eq!(gutter.rows[0].origin.y, layout.lines[0].origin.y);
                assert!(gutter.rows[0].origin.x > gutter_bounds.left());
                assert_eq!(layout.text_clip_bounds.left(), gutter_bounds.right());
            })
            .expect("测试窗口应保持可用");
    }

    #[gpui::test]
    fn folded_projection_rows_drive_layout_and_placeholder_hit_testing(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Empty);
        window
            .update(cx, |_, window, _| {
                let snapshot = Buffer::scratch(
                    "anchor\nhidden one\nhidden two\nafter".to_owned(),
                    BufferConfig::default(),
                )
                .expect("测试 Buffer 应能创建")
                .snapshot();
                let mut map = DisplayMap::new(snapshot.clone());
                map.fold_lines(LineRange::new(Line::ZERO, Line::new(3)).expect("测试行区间应合法"))
                    .expect("折叠应成功");
                let layout = layout_visible_lines(
                    map.snapshot(),
                    EditorPresentation::new(&snapshot, None),
                    VisibleLineLayoutParams {
                        geometry: EditorGeometry {
                            text_bounds: Bounds::new(
                                point(px(0.), px(0.)),
                                size(px(400.), px(100.)),
                            ),
                            text_clip_bounds: Bounds::new(
                                point(px(0.), px(0.)),
                                size(px(400.), px(100.)),
                            ),
                            gutter: None,
                        },
                        active_lines: &BTreeSet::new(),
                        start_row: DisplayRow::ZERO,
                        scroll_offset: point(px(0.), px(0.)),
                        line_height: px(20.),
                    },
                    window,
                );

                assert_eq!(layout.lines.len(), 3);
                assert_eq!(layout.lines[0].shaped.text.as_ref(), "anchor");
                assert_eq!(layout.lines[1].shaped.text.as_ref(), "…");
                assert_eq!(layout.lines[2].shaped.text.as_ref(), "after");
                assert_eq!(
                    layout.buffer_point_for_position(point(px(1.), px(25.))),
                    Some(BufferPoint::new(Line::ZERO, LogicalColumn::ZERO))
                );
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
                let display_snapshot = DisplayMap::new(snapshot.clone()).snapshot();
                let layout = layout_visible_lines(
                    display_snapshot,
                    presentation,
                    VisibleLineLayoutParams {
                        geometry: EditorGeometry {
                            text_bounds: Bounds::new(
                                point(px(0.), px(0.)),
                                size(px(200.), px(40.)),
                            ),
                            text_clip_bounds: Bounds::new(
                                point(px(0.), px(0.)),
                                size(px(200.), px(40.)),
                            ),
                            gutter: None,
                        },
                        active_lines: &BTreeSet::new(),
                        start_row: DisplayRow::new(10),
                        scroll_offset: point(px(0.), px(0.)),
                        line_height: px(20.),
                    },
                    window,
                );
                let (_, carets) =
                    layout_selections(&SelectionSet::caret(ByteOffset::ZERO), &layout, px(20.));

                assert!(carets.is_empty());
            })
            .expect("测试窗口应保持可用");
    }
}
