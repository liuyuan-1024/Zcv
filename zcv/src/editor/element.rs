//! Editor 的逐帧文本布局、绘制与像素命中测试。

use std::sync::Arc;

use gpui::{
    App, Bounds, DispatchPhase, Element, ElementId, Entity, GlobalElementId, HitboxBehavior,
    InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent, PaintQuad, Pixels,
    Point, ShapedLine, Style, TextRun, Window, fill, point, px, relative, size,
};
use zcv_engine::{Line, LogicalColumn, SelectionSet, Snapshot};

use super::display_map::{BufferPoint, DisplayRow};
use super::editor::Editor;
use crate::theme::color;

pub(super) struct EditorElement {
    editor: Entity<Editor>,
}

impl EditorElement {
    pub(super) fn new(editor: Entity<Editor>) -> Self {
        Self { editor }
    }
}

struct LayoutLine {
    row: DisplayRow,
    origin: Point<Pixels>,
    shaped: ShapedLine,
}

struct EditorLayout {
    lines: Vec<LayoutLine>,
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
        let column = line.shaped.text[..byte_index].chars().count();
        Some(BufferPoint::new(
            Line::new(line.row.get()),
            LogicalColumn::new(column),
        ))
    }
}

pub(super) struct PrepaintState {
    layout: Arc<EditorLayout>,
    selections: Vec<PaintQuad>,
    carets: Vec<PaintQuad>,
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
        let (snapshot, selections, start_row, scroll_offset) = {
            let editor = self.editor.read(cx);
            (
                editor.render_snapshot(),
                editor.selections(),
                editor.scroll_anchor().row(),
                editor.scroll_offset(),
            )
        };
        let line_height = window.line_height();
        let layout = Arc::new(layout_visible_lines(
            &snapshot,
            bounds,
            start_row,
            scroll_offset,
            line_height,
            window,
        ));
        let (selections, carets) = layout_selections(&snapshot, &selections, &layout, line_height);

        PrepaintState {
            layout,
            selections,
            carets,
            hitbox: window.insert_hitbox(bounds, HitboxBehavior::Normal),
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let editor = self.editor.clone();
        let event_layout = Arc::clone(&prepaint.layout);
        let hitbox = prepaint.hitbox.clone();
        let focus = self.editor.read(cx).focus_handle();
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
            window.focus(&focus);
            cx.stop_propagation();
        });

        for selection in prepaint.selections.drain(..) {
            window.paint_quad(selection);
        }
        for line in &prepaint.layout.lines {
            line.shaped
                .paint(line.origin, prepaint.layout.line_height, window, cx)
                .expect("Editor 文本行绘制失败");
        }
        if self.editor.read(cx).focus_handle().is_focused(window) {
            for caret in prepaint.carets.drain(..) {
                window.paint_quad(caret);
            }
        }
    }
}

fn layout_visible_lines(
    snapshot: &Snapshot,
    bounds: Bounds<Pixels>,
    start_row: DisplayRow,
    scroll_offset: Point<Pixels>,
    line_height: Pixels,
    window: &mut Window,
) -> EditorLayout {
    let line_count = snapshot.line_count();
    let start = start_row.get().min(line_count.saturating_sub(1));
    let visible_count = ((bounds.size.height + scroll_offset.y) / line_height).ceil() as usize + 1;
    let end = (start + visible_count).min(line_count);
    let text_style = window.text_style();
    let font_size = text_style.font_size.to_pixels(window.rem_size());
    let mut lines = Vec::with_capacity(end.saturating_sub(start));

    for row in start..end {
        let text = snapshot
            .slice_line(Line::new(row))
            .expect("可见行必须位于 Snapshot 边界内")
            .as_str()
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        let run = TextRun {
            len: text.len(),
            font: text_style.font(),
            color: text_style.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped = window
            .text_system()
            .shape_line(text.into(), font_size, &[run], None);
        lines.push(LayoutLine {
            row: DisplayRow::new(row),
            origin: point(
                bounds.left() - scroll_offset.x,
                bounds.top() + line_height * (row - start) - scroll_offset.y,
            ),
            shaped,
        });
    }

    EditorLayout { lines, line_height }
}

fn layout_selections(
    snapshot: &Snapshot,
    selections: &SelectionSet,
    layout: &EditorLayout,
    line_height: Pixels,
) -> (Vec<PaintQuad>, Vec<PaintQuad>) {
    let mut selection_quads = Vec::new();
    let mut caret_quads = Vec::new();

    for selection in selections.as_slice().iter().copied() {
        if selection.is_caret() {
            let Ok(position) = snapshot.byte_to_position(selection.head()) else {
                continue;
            };
            let Some(line) = layout
                .lines
                .iter()
                .find(|line| line.row.get() == position.line().get())
            else {
                continue;
            };
            let byte_index = column_to_byte(&line.shaped.text, position.column().get());
            caret_quads.push(fill(
                Bounds::new(
                    point(
                        line.origin.x + line.shaped.x_for_index(byte_index),
                        line.origin.y,
                    ),
                    size(px(2.), line_height),
                ),
                color::current().blue.s[6],
            ));
            continue;
        }

        let (Ok(start), Ok(end)) = (
            snapshot.byte_to_position(selection.start()),
            snapshot.byte_to_position(selection.end()),
        ) else {
            continue;
        };
        for line in layout.lines.iter().filter(|line| {
            let row = line.row.get();
            start.line().get() <= row && row <= end.line().get()
        }) {
            let row = line.row.get();
            let start_byte = if row == start.line().get() {
                column_to_byte(&line.shaped.text, start.column().get())
            } else {
                0
            };
            let end_byte = if row == end.line().get() {
                column_to_byte(&line.shaped.text, end.column().get())
            } else {
                line.shaped.len()
            };
            let start_x = line.shaped.x_for_index(start_byte);
            let mut end_x = line.shaped.x_for_index(end_byte);
            if row < end.line().get() && end_x <= start_x {
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

    (selection_quads, caret_quads)
}

fn column_to_byte(text: &str, column: usize) -> usize {
    text.char_indices()
        .nth(column)
        .map_or(text.len(), |(byte, _)| byte)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::display_map::DisplayPoint;
    use gpui::{Empty, TestAppContext, font};
    use zcv_engine::DisplayColumn;

    #[test]
    fn logical_columns_map_to_utf8_boundaries() {
        let text = "a你😀";
        assert_eq!(column_to_byte(text, 0), 0);
        assert_eq!(column_to_byte(text, 1), 1);
        assert_eq!(column_to_byte(text, 2), 4);
        assert_eq!(column_to_byte(text, 3), 8);
        assert_eq!(column_to_byte(text, 99), 8);
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
                let layout = EditorLayout {
                    lines: vec![LayoutLine {
                        row: DisplayRow::new(3),
                        origin: point(px(10.), px(20.)),
                        shaped,
                    }],
                    line_height: px(24.),
                };

                assert_eq!(
                    layout.buffer_point_for_position(point(px(10.) + cursor_x, px(32.))),
                    Some(BufferPoint::new(Line::new(3), LogicalColumn::new(2)))
                );
                assert_eq!(
                    DisplayPoint::new(DisplayRow::new(3), DisplayColumn::new(2)).row(),
                    DisplayRow::new(3)
                );
            })
            .expect("测试窗口应保持可用");
    }
}
