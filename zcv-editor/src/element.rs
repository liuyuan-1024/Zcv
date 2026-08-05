//! Editor 的逐帧文本布局、绘制与像素命中测试。

use std::collections::BTreeSet;
use std::ops::Range;
use std::sync::Arc;

use gpui::{
    App, Bounds, ContentMask, Context, DispatchPhase, Element, ElementId, ElementInputHandler,
    Entity, GlobalElementId, HighlightStyle, HitboxBehavior, InspectorElementId,
    InteractiveElement, IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PaintQuad, Pixels, Point, ScrollWheelEvent, ShapedLine, Style, TextRun,
    UnderlineStyle, Window, fill, point, px, relative, size,
};
use zcv_engine::{ByteOffset, DisplayColumn, Line, SelectionSet, TextRange};
use zcv_language::{BracketPair, HighlightSpan};

use super::display_map::{
    BufferPoint, DisplayPoint, DisplayRow, DisplaySnapshot, ProjectedRange, WrapViewportRowKind,
    byte_for_display_column,
};
use super::gutter::{GutterDimensions, GutterLayout, GutterRow};
use super::scroll::ScrollbarThumbState;
use super::scrollbar::{SCROLLBAR_WIDTH, ScrollbarLayout, marker_column_x_range, marker_geometry};
use super::view::{DiffHunk, DiffHunkKind, Editor, EditorMode, EditorPresentation, SoftWrap};
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
            .on_action(cx.listener(Editor::handle_expand_selection))
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
            .on_action(cx.listener(Editor::handle_move_line_up))
            .on_action(cx.listener(Editor::handle_move_line_down))
    }
}

#[derive(Clone)]
struct LayoutLine {
    row: DisplayRow,
    logical_line: Option<Line>,
    origin: Point<Pixels>,
    shaped: ShapedLine,
    global_utf16_start: usize,
    wrap_info: Option<WrapRowInfo>,
    /// 该显示行所属的 git diff 类型（内容背景用；wrap 续行同样标注）。
    git_diff: Option<DiffHunkKind>,
}

/// 软换行续行信息：片段所属逻辑行、假空格缩进数与片段起始逻辑字符列。
///
/// 命中测试与光标定位都通过它把"显示行内位置"换算回逻辑行坐标。
#[derive(Clone, Copy)]
struct WrapRowInfo {
    line: Line,
    indent: usize,
    column_base: usize,
}

struct EditorLayout {
    lines: Vec<LayoutLine>,
    gutter: Option<GutterLayout>,
    text_clip_bounds: Bounds<Pixels>,
    line_height: Pixels,
    display_snapshot: DisplaySnapshot,
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
    /// git diff 显示行区间（prepaint 从 `diff_hunk_rows` 计算，gutter/内容共用）。
    diff_rows: &'a [(Range<usize>, DiffHunkKind)],
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
        // 软换行续行：命中假空格区落在片段起点，其余按"片段起始列 + 段内字符数"换算。
        if let Some(info) = line.wrap_info {
            let local_chars = line.shaped.text[..byte_index].chars().count();
            let column = if local_chars <= info.indent {
                info.column_base
            } else {
                info.column_base + local_chars - info.indent
            };
            return Some(BufferPoint::new(
                info.line,
                zcv_engine::LogicalColumn::new(column),
            ));
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
}

pub(super) struct PrepaintState {
    layout: Arc<EditorLayout>,
    selections: Vec<PaintQuad>,
    carets: Vec<PaintQuad>,
    ime_caret_bounds: Option<Bounds<Pixels>>,
    hitbox: gpui::Hitbox,
    gutter_hitbox: Option<gpui::Hitbox>,
    scrollbar: Option<ScrollbarLayout>,
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
        let text_style = window.text_style();
        let font = text_style.font();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        // 用上一帧的 snapshot 计算文本区域宽度；wrap 生效后行数变化会让 gutter
        // 位数在下一帧自动修正，不影响正确性。
        let (
            display_snapshot,
            presentation,
            selections,
            longest_row,
            shows_gutter,
            active_lines,
            soft_wrap,
            mode,
            preferred_line_length,
            matching_bracket_pair,
        ) = {
            let editor = self.editor.read(cx);
            (
                editor.display_snapshot(),
                editor.presentation(),
                editor.selections(),
                editor.longest_display_row(),
                editor.shows_gutter(),
                editor.active_lines().into_iter().collect::<BTreeSet<_>>(),
                editor.soft_wrap(),
                editor.mode(),
                editor.preferred_line_length(),
                editor.matching_bracket_pair(),
            )
        };
        let mode = mode.clone();
        let gutter_dimensions = shows_gutter.then(|| gutter_dimensions(&display_snapshot, window));
        let gutter_bounds = gutter_dimensions.map(|dimensions| Bounds {
            origin: bounds.origin,
            size: size(dimensions.width, bounds.size.height),
        });
        let text_left =
            bounds.left() + gutter_dimensions.map_or(Pixels::ZERO, GutterDimensions::full_width);
        let text_clip_left =
            bounds.left() + gutter_dimensions.map_or(Pixels::ZERO, |dimensions| dimensions.width);
        // 滚动轴让位：Full 模式下文本区右缘收窄一个滚动轴宽度。
        let scrollbar_width = if mode == EditorMode::Full {
            SCROLLBAR_WIDTH
        } else {
            Pixels::ZERO
        };
        let text_right = bounds.right() - scrollbar_width;
        let text_bounds = Bounds {
            origin: point(text_left, bounds.top()),
            size: size(
                (text_right - text_left).max(Pixels::ZERO),
                bounds.size.height,
            ),
        };
        let scrollbar_bounds = Bounds {
            origin: point(text_right, bounds.top()),
            size: size(scrollbar_width, bounds.size.height),
        };
        let geometry = EditorGeometry {
            text_bounds,
            text_clip_bounds: Bounds {
                origin: point(text_clip_left, bounds.top()),
                size: size(
                    (text_right - text_clip_left).max(Pixels::ZERO),
                    bounds.size.height,
                ),
            },
            gutter: gutter_bounds.zip(gutter_dimensions),
        };
        let wrap_width = match (soft_wrap, &mode) {
            (SoftWrap::None, _) | (_, EditorMode::SingleLine | EditorMode::AutoHeight { .. }) => {
                None
            }
            (SoftWrap::EditorWidth, _) => Some(text_bounds.size.width),
            (SoftWrap::Bounded, _) => {
                // em 宽用 'm' 的字形 advance 近似，与 Zed 的 wrap_width_for 一致。
                let run = TextRun {
                    len: 1,
                    font: font.clone(),
                    color: text_style.color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let em_width = window
                    .text_system()
                    .shape_line("m".into(), font_size, &[run], None)
                    .width;
                Some(
                    text_bounds
                        .size
                        .width
                        .min(em_width * preferred_line_length as f32),
                )
            }
        };
        // 设置换行宽度（变化才重排），随后读取最新 snapshot 供本帧布局使用。
        let display_snapshot = self.editor.update(cx, |editor, cx| {
            editor.set_wrap_width(wrap_width, font, font_size, cx);
            editor.display_snapshot()
        });
        // 软换行模式下显示行不再由 TabMap 测量（水平滚动收敛到视口宽度）。
        if !display_snapshot.is_wrapped() {
            self.editor.update(cx, |editor, _| {
                editor.measure_display_rows(editor.scroll_anchor().row(), visible_line_count);
            });
        }
        let content_width = if display_snapshot.is_wrapped() {
            text_bounds.size.width
        } else {
            layout_line_width(&display_snapshot, longest_row, window) + CARET_WIDTH
        };
        self.editor.update(cx, |editor, _| {
            editor.prepare_scroll_viewport(text_bounds.size, content_width, line_height);
        });
        let (start_row, scroll_offset) = {
            let editor = self.editor.read(cx);
            (editor.scroll_anchor().row(), editor.scroll_offset())
        };
        // git diff 显示行区间：gutter 指示、内容背景与滚动轴 marker 共用（只依赖 snapshot 与注入 hunks，与滚动位置无关，autoscroll 重排可复用）。
        let diff_rows = {
            let editor = self.editor.read(cx);
            diff_hunk_rows(&display_snapshot, editor.diff_hunks(cx))
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
                diff_rows: &diff_rows,
            },
            window,
            cx,
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
                    diff_rows: &diff_rows,
                },
                window,
                cx,
            );
            ime_caret_bounds = layout_primary_caret(&selections, &layout, line_height);
        }
        let layout = Arc::new(layout);
        let (mut selections, carets) = layout_selections(&selections, &layout, line_height, cx);
        if let Some(pair) = matching_bracket_pair {
            layout_bracket_pair(pair, &layout, line_height, &mut selections, cx);
        }
        let gutter_hitbox = layout
            .gutter
            .as_ref()
            .map(|gutter| window.insert_hitbox(gutter.bounds, HitboxBehavior::Normal));
        let scrollbar = (mode == EditorMode::Full).then(|| {
            let editor = self.editor.read(cx);
            let mut scrollbar_layout = ScrollbarLayout::new(
                scrollbar_bounds,
                editor.max_scroll_top(),
                editor.scroll_top(),
                editor.scrollbar_thumb_state(),
                window,
            );
            // marker 每帧计算（hunks 数量级小；滚动中实时跟随，无需缓存/后台任务）。
            // scroll_per_pixel 取 layout 自身算好的值，与 thumb 换算严格一致。
            scrollbar_layout.markers = marker_geometry(
                diff_rows.iter().cloned(),
                scrollbar_layout.hitbox.bounds,
                scrollbar_layout.scroll_per_pixel,
                line_height,
            );
            scrollbar_layout
        });

        PrepaintState {
            layout,
            selections,
            carets,
            ime_caret_bounds,
            hitbox: window.insert_hitbox(bounds, HitboxBehavior::Normal),
            gutter_hitbox,
            scrollbar,
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

        if let Some(scrollbar_layout) = &prepaint.scrollbar {
            self.register_scrollbar_handlers(scrollbar_layout, window, cx);
        }

        if let Some(gutter) = &prepaint.layout.gutter {
            window.paint_quad(fill(
                gutter.bounds,
                color::current(cx).editor_gutter_background,
            ));
            for bounds in gutter.active_row_bounds(prepaint.layout.text_clip_bounds.right()) {
                window.paint_quad(fill(
                    bounds,
                    color::current(cx).editor_active_line_background,
                ));
            }
            if let Some(hitbox) = &prepaint.gutter_hitbox {
                window.set_cursor_style(gpui::CursorStyle::IBeam, hitbox);
            }
        }
        if let Some(gutter) = &prepaint.layout.gutter {
            let colors = color::current(cx);
            // diff 行 gutter 背景（与内容区同色，整行贯通避免割裂）。
            for row in &gutter.rows {
                if let Some(kind) = row.git_diff {
                    let background = match kind {
                        DiffHunkKind::Added => colors.editor_diff_added_background,
                        DiffHunkKind::Modified => colors.editor_diff_modified_background,
                        DiffHunkKind::Deleted => colors.editor_diff_deleted_background,
                    };
                    window.paint_quad(fill(
                        Bounds::from_corners(
                            point(gutter.bounds.left(), row.origin.y),
                            point(gutter.bounds.right(), row.origin.y + gutter.line_height),
                        ),
                        background,
                    ));
                }
            }
            // git diff 色条（对齐 Zed paint_gutter_diff_hunks：行号左侧竖条，状态色）。
            let strip_width = gutter_strip_width(gutter.line_height);
            for row in &gutter.rows {
                if let Some(kind) = row.git_diff {
                    let strip_color = match kind {
                        DiffHunkKind::Added => colors.status_created,
                        DiffHunkKind::Modified => colors.status_modified,
                        DiffHunkKind::Deleted => colors.status_deleted,
                    };
                    window.paint_quad(fill(
                        Bounds::from_corners(
                            point(gutter.bounds.left(), row.origin.y),
                            point(
                                gutter.bounds.left() + strip_width,
                                row.origin.y + gutter.line_height,
                            ),
                        ),
                        strip_color,
                    ));
                }
            }
            for row in &gutter.rows {
                if let Err(error) =
                    row.shaped_line_number
                        .paint(row.origin, gutter.line_height, window, cx)
                {
                    // 单个字形绘制失败只跳过该行，不能让整个窗口崩溃（对齐 Zed 的降级策略）。
                    log::error!("Editor gutter 行号绘制失败：{error}");
                    continue;
                }
            }
        }
        let show_local_cursors = self.editor.read(cx).show_local_cursors(window, cx);
        window.with_content_mask(
            Some(ContentMask {
                bounds: prepaint.layout.text_clip_bounds,
            }),
            |window| {
                // git diff 整行淡背景（diff 行在 selection 之下、文本之上）。
                let diff_colors = color::current(cx);
                for line in &prepaint.layout.lines {
                    if let Some(kind) = line.git_diff {
                        let background = match kind {
                            DiffHunkKind::Added => diff_colors.editor_diff_added_background,
                            DiffHunkKind::Modified => diff_colors.editor_diff_modified_background,
                            DiffHunkKind::Deleted => diff_colors.editor_diff_deleted_background,
                        };
                        window.paint_quad(fill(
                            Bounds::from_corners(
                                point(prepaint.layout.text_clip_bounds.left(), line.origin.y),
                                point(
                                    prepaint.layout.text_clip_bounds.right(),
                                    line.origin.y + prepaint.layout.line_height,
                                ),
                            ),
                            background,
                        ));
                    }
                }
                for selection in prepaint.selections.drain(..) {
                    window.paint_quad(selection);
                }
                for line in &prepaint.layout.lines {
                    if let Err(error) =
                        line.shaped
                            .paint(line.origin, prepaint.layout.line_height, window, cx)
                    {
                        // 单个字形绘制失败只跳过该行，不能让整个窗口崩溃（对齐 Zed 的降级策略）。
                        log::error!("Editor 文本行绘制失败：{error}");
                        continue;
                    }
                }
                if show_local_cursors {
                    for caret in prepaint.carets.drain(..) {
                        window.paint_quad(caret);
                    }
                }
            },
        );
        if let Some(scrollbar) = &prepaint.scrollbar {
            let colors = color::current(cx);
            // 轨道背景透明，只画一个占位 quad（后续 marker 会叠加在这一层）。
            window.paint_quad(fill(
                scrollbar.hitbox.bounds,
                colors.scrollbar_track_background,
            ));
            // git diff marker 列（track 之上、thumb 之下绘制；颜色对齐项目树 git 状态色）。
            let column_x = marker_column_x_range(scrollbar.hitbox.bounds);
            for marker in &scrollbar.markers {
                let marker_color = match marker.kind {
                    DiffHunkKind::Added => colors.status_created,
                    DiffHunkKind::Modified => colors.status_modified,
                    DiffHunkKind::Deleted => colors.status_deleted,
                };
                window.paint_quad(fill(
                    Bounds::from_corners(
                        point(column_x.start, marker.y_range.start),
                        point(column_x.end, marker.y_range.end),
                    ),
                    marker_color,
                ));
            }
            if let Some(thumb_bounds) = scrollbar.thumb_bounds {
                let thumb_color = match scrollbar.thumb_state {
                    ScrollbarThumbState::Dragging => colors.scrollbar_thumb_active_background,
                    ScrollbarThumbState::Hovered => colors.scrollbar_thumb_hover_background,
                    ScrollbarThumbState::Idle => colors.scrollbar_thumb_background,
                };
                window.paint_quad(fill(thumb_bounds, thumb_color));
                // 拖动中整窗用 Arrow（指针可能已移出轨道），否则仅轨道内 Arrow。
                if scrollbar.thumb_state == ScrollbarThumbState::Dragging {
                    window.set_window_cursor_style(gpui::CursorStyle::Arrow);
                } else {
                    window.set_cursor_style(gpui::CursorStyle::Arrow, &scrollbar.hitbox);
                }
            }
        }
        let input_layout = prepaint.layout.input_layout();
        self.editor.update(cx, |editor, _| {
            editor.set_input_layout(input_layout);
            editor.set_ime_caret_geometry(bounds, prepaint.ime_caret_bounds);
        });
    }
}

impl EditorElement {
    /// 注册滚动轴鼠标交互：悬停三态、拖动滚动、点击轨道跳页。
    ///
    /// 三个 handler 都在文本 MouseDown / ScrollWheel handler 之后注册，gpui 的 Bubble 阶段逆序分发保证滚动轴优先处理并 stop_propagation；
    /// 点击轨道时用 hitbox.is_hovered 门控，文本区点击不会被误判为跳页。
    /// 按下/松开按上一帧状态条件注册（对齐 Zed）：未拖动时注册 MouseDown，拖动中注册 MouseUp，松开后的兜底由无按键 MouseMove 复位。
    fn register_scrollbar_handlers(
        &self,
        scrollbar_layout: &ScrollbarLayout,
        window: &mut Window,
        cx: &mut App,
    ) {
        // 悬停与拖动共用 MouseMove：无按键时更新三态，按住左键且处于拖动态时以上一事件位置为基准做增量滚动（移出轨道即停、移回继续）。
        window.on_mouse_event({
            let editor = self.editor.clone();
            let scrollbar_layout = scrollbar_layout.clone();
            let mut mouse_position = window.mouse_position();
            move |event: &MouseMoveEvent, phase, _window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                editor.update(cx, |editor, cx| {
                    if event.dragging()
                        && editor.scrollbar_thumb_state() == ScrollbarThumbState::Dragging
                    {
                        let old_position = mouse_position.y;
                        let new_position = event.position.y;
                        if (scrollbar_layout.hitbox.bounds.top()
                            ..scrollbar_layout.hitbox.bounds.bottom())
                            .contains(&old_position)
                        {
                            let delta = new_position - old_position;
                            let scroll_top =
                                editor.scroll_top() + delta * scrollbar_layout.scroll_per_pixel;
                            editor.scroll_to(scroll_top.max(Pixels::ZERO), cx);
                        }
                        cx.stop_propagation();
                    } else if !event.dragging() && scrollbar_layout.thumb_hovered(&event.position) {
                        editor.set_scrollbar_thumb_hovered(cx);
                    } else if !event.dragging() {
                        // 兜底：无按键移动也会复位（覆盖"窗口外释放后移回"等漏网场景）。
                        editor.reset_scrollbar_thumb_state(cx);
                    }
                    mouse_position = event.position;
                });
            }
        });

        let dragging =
            self.editor.read(cx).scrollbar_thumb_state() == ScrollbarThumbState::Dragging;
        if !dragging {
            // 按下：点击轨道（thumb 外）以点击处为中心跳页，点中 thumb 则进入拖动态。
            window.on_mouse_event({
                let editor = self.editor.clone();
                let scrollbar_layout = scrollbar_layout.clone();
                move |event: &MouseDownEvent, phase, _window, cx| {
                    if phase != DispatchPhase::Bubble
                        || event.button != MouseButton::Left
                        || !scrollbar_layout.hitbox.is_hovered(_window)
                    {
                        return;
                    }
                    editor.update(cx, |editor, cx| {
                        editor.set_scrollbar_thumb_dragged(cx);
                        if let Some(thumb_bounds) = scrollbar_layout.thumb_bounds
                            && (event.position.y < thumb_bounds.top()
                                || thumb_bounds.bottom() < event.position.y)
                        {
                            // 点击轨道（thumb 外）：以点击处为中心跳页，钳制由 scroll_to 完成。
                            let click_px = event.position.y - scrollbar_layout.hitbox.bounds.top();
                            let target = click_px * scrollbar_layout.scroll_per_pixel
                                - scrollbar_layout.hitbox.bounds.size.height * 0.5;
                            editor.scroll_to(target.max(Pixels::ZERO), cx);
                        }
                        cx.stop_propagation();
                    });
                }
            });
        } else {
            // 松开：鼠标仍在轨道内 → Hovered，否则 → Idle。
            window.on_mouse_event({
                let editor = self.editor.clone();
                let scrollbar_layout = scrollbar_layout.clone();
                move |_: &MouseUpEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    editor.update(cx, |editor, cx| {
                        if scrollbar_layout.hitbox.is_hovered(window) {
                            editor.set_scrollbar_thumb_hovered(cx);
                        } else {
                            editor.reset_scrollbar_thumb_state(cx);
                        }
                        cx.stop_propagation();
                    });
                }
            });
        }
    }
}

/// hunks（逻辑行）→ 显示行区间：wrap 下行映射出的全部显示行都覆盖。
///
/// 覆盖终点取 hunk 之后第一行的行首显示行（对齐 Zed：end 行首显示行 − 1 即 hunk 最后一个显示行，左闭右开区间 [start, end) 恰好盖住全部 wrap 片段）；
/// hunk 到达文件末尾时以显示快照行数为终点。
/// 纯删除 hunk（空范围）锚定到所在显示行。
/// 映射失败（越界等）跳过该 hunk。
fn diff_hunk_rows(
    snapshot: &DisplaySnapshot,
    hunks: &[DiffHunk],
) -> Vec<(Range<usize>, DiffHunkKind)> {
    hunks
        .iter()
        .filter_map(|hunk| {
            let start = snapshot
                .line_to_display_row(Line::new(hunk.range.start))?
                .get();
            let end = if hunk.range.end > hunk.range.start {
                match snapshot.line_to_display_row(Line::new(hunk.range.end)) {
                    Some(row) => row.get(),
                    None => snapshot.line_count(),
                }
            } else {
                start + 1
            };
            Some((start..end.max(start + 1), hunk.kind))
        })
        .collect()
}

/// 查询显示行所属的 diff 类型（gutter 与内容背景共用；线性扫描，hunks 数量级小）。
fn diff_kind_for_row(
    diff_rows: &[(Range<usize>, DiffHunkKind)],
    row: usize,
) -> Option<DiffHunkKind> {
    diff_rows
        .iter()
        .find(|(range, _)| range.contains(&row))
        .map(|(_, kind)| *kind)
}

/// gutter diff 色条宽度（对齐 Zed `gutter_strip_width`：0.275 × 行高）。
fn gutter_strip_width(line_height: Pixels) -> Pixels {
    (line_height * 0.275).floor()
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
        WrapViewportRowKind::Text {
            content,
            byte_range,
            ..
        } => &content.as_str()[byte_range.clone()],
        WrapViewportRowKind::Placeholder(_) => "…",
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
    cx: &App,
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
        diff_rows,
    } = params;
    let line_count = display_snapshot.line_count();
    let start = start_row.get().min(line_count.saturating_sub(1));
    let visible_count =
        ((text_bounds.size.height + scroll_offset.y) / line_height).ceil() as usize + 1;
    let end = (start + visible_count).min(line_count);
    let text_style = window.text_style();
    let font_size = text_style.font_size.to_pixels(window.rem_size());
    let mut lines = Vec::with_capacity(end.saturating_sub(start));
    let mut gutter_rows = Vec::with_capacity(end.saturating_sub(start));
    // 可见范围的语法高亮来自 display_map 注入的全量缓存（解析完成时构建），渲染侧只做有序切片，不再每帧树遍历；
    // 缓存版本与 buffer 不一致时返回空。
    let visible_highlights = display_snapshot
        .slice_viewport(DisplayRow::new(start), end.saturating_sub(start))
        .ok()
        .and_then(|viewport| {
            let mut range: Option<std::ops::Range<usize>> = None;
            for row in viewport.rows() {
                let WrapViewportRowKind::Text {
                    byte_range,
                    global_byte_start,
                    ..
                } = row.kind()
                else {
                    continue;
                };
                let row_range = *global_byte_start..*global_byte_start + byte_range.len();
                range = Some(match range {
                    Some(range) => range.start.min(row_range.start)..range.end.max(row_range.end),
                    None => row_range,
                });
            }
            range
        })
        .unwrap_or_default();
    let visible_highlights = display_snapshot.highlighted_spans(&visible_highlights);
    // capture 索引 → 样式的预展开表：渲染每 run 一次数组索引，不再逐 run 做字符串回退查找。
    let highlight_styles = display_snapshot.highlight_styles();

    let mut push_line = |row: usize,
                         logical_line: Option<Line>,
                         gutter_line: Option<Line>,
                         text: &str,
                         byte_start: usize,
                         utf16_start: usize,
                         wrap_info: Option<WrapRowInfo>| {
        let display_prefix_len = wrap_info.as_ref().map_or(0, |info| info.indent);
        let highlights = if logical_line.is_none() {
            &[][..]
        } else {
            let source_len = text.len().saturating_sub(display_prefix_len);
            let byte_end = byte_start + source_len;
            let start = visible_highlights.partition_point(|span| span.range.end <= byte_start);
            let end = visible_highlights.partition_point(|span| span.range.start < byte_end);
            &visible_highlights[start..end]
        };
        let runs = text_runs(
            text,
            byte_start,
            display_prefix_len,
            highlights,
            highlight_styles,
            presentation.marked_ranges(),
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
        let git_diff = diff_kind_for_row(diff_rows, row);
        lines.push(LayoutLine {
            row: DisplayRow::new(row),
            logical_line,
            origin: point(
                text_bounds.left() - scroll_offset.x,
                text_bounds.top() + line_height * (row - start) - scroll_offset.y,
            ),
            shaped,
            global_utf16_start: utf16_start,
            wrap_info,
            git_diff,
        });
        if let (Some(logical_line), Some((gutter_bounds, dimensions))) =
            (gutter_line, gutter_geometry)
        {
            let number = (logical_line.get() + 1).to_string();
            let active = active_lines.contains(&logical_line);
            let colors = color::current(cx);
            // 行号按 diff 状态着色（对齐 Zed：DiffAdded → version_control_added）。
            let number_color = match (active, git_diff) {
                (_, Some(DiffHunkKind::Added)) => colors.status_created,
                (_, Some(DiffHunkKind::Deleted)) => colors.status_deleted,
                (_, Some(DiffHunkKind::Modified)) => colors.status_modified,
                (true, None) => colors.editor_active_line_number,
                (false, None) => colors.editor_line_number,
            };
            let run = TextRun {
                len: number.len(),
                font: text_style.font(),
                color: number_color.into(),
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
                git_diff,
            });
        }
    };

    if let Ok(viewport) =
        display_snapshot.slice_viewport(DisplayRow::new(start), end.saturating_sub(start))
    {
        for row in viewport.rows() {
            match row.kind() {
                WrapViewportRowKind::Text {
                    logical_line,
                    content,
                    byte_range,
                    global_byte_start,
                    fragment_index,
                    indent,
                    column_base,
                } => {
                    // 续行的假空格是显示文本的一部分，坐标与命中测试都把它算进列。
                    let text = &content.as_str()[byte_range.clone()];
                    let display_text = if *indent > 0 {
                        format!("{}{}", " ".repeat(*indent), text)
                    } else {
                        text.to_owned()
                    };
                    let utf16_start = display_snapshot
                        .buffer_snapshot()
                        .byte_to_utf16_cu(ByteOffset::new(*global_byte_start))
                        .map_or(0, |offset| offset.get());
                    let wrap_info = (*fragment_index > 0).then_some(WrapRowInfo {
                        line: *logical_line,
                        indent: *indent,
                        column_base: *column_base,
                    });
                    push_line(
                        row.index().get(),
                        Some(*logical_line),
                        // 行号只在逻辑行首显示行出现。
                        (*fragment_index == 0).then_some(*logical_line),
                        &display_text,
                        *global_byte_start,
                        utf16_start,
                        wrap_info,
                    );
                }
                WrapViewportRowKind::Placeholder(placeholder) => {
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
                        None,
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
    cx: &App,
) -> (Vec<PaintQuad>, Vec<PaintQuad>) {
    let mut selection_quads = Vec::new();
    let mut caret_quads = Vec::new();

    for selection in selections.as_slice().iter().copied() {
        // 选区存在时也在 head（活动端）绘制光标，表示输入插入点。
        let caret = layout_caret_at_buffer_offset(selection.head(), layout, line_height, cx);
        if selection.is_caret() {
            if let Some(caret) = caret {
                caret_quads.push(caret);
            }
            continue;
        }
        if let Ok(ranges) = layout
            .display_snapshot
            .project_text_range(selection.range())
        {
            for range in ranges {
                layout_projected_range(range, layout, line_height, &mut selection_quads, cx);
            }
            if let Some(caret) = caret {
                caret_quads.push(caret);
            }
            continue;
        }
    }

    (selection_quads, caret_quads)
}

fn layout_bracket_pair(
    pair: BracketPair,
    layout: &EditorLayout,
    line_height: Pixels,
    quads: &mut Vec<PaintQuad>,
    cx: &App,
) {
    for range in [pair.open, pair.close] {
        let Ok(range) = TextRange::new(ByteOffset::new(range.start), ByteOffset::new(range.end))
        else {
            continue;
        };
        let Ok(projected) = layout.display_snapshot.project_text_range(range) else {
            continue;
        };
        for range in projected {
            layout_projected_range(range, layout, line_height, quads, cx);
        }
    }
}

fn layout_projected_range(
    range: ProjectedRange,
    layout: &EditorLayout,
    line_height: Pixels,
    selection_quads: &mut Vec<PaintQuad>,
    cx: &App,
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
            color::current(cx).editor_selection_background,
        ));
    }
}

fn layout_primary_caret(
    selections: &SelectionSet,
    layout: &EditorLayout,
    line_height: Pixels,
) -> Option<Bounds<Pixels>> {
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
    cx: &App,
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
        color::current(cx).editor_cursor,
    ))
}

fn local_byte_for_display_point(
    line: &LayoutLine,
    point: DisplayPoint,
    display_snapshot: &DisplaySnapshot,
) -> usize {
    if let Some(info) = line.wrap_info {
        // 显示行文本 = 假空格 + 片段；目标列落在缩进区内时返回片段起点。
        let fragment = &line.shaped.text[info.indent..];
        let affinity = display_snapshot
            .buffer_snapshot()
            .config()
            .display_width
            .affinity;
        let local = byte_for_display_column(
            fragment,
            info.indent,
            point.column().get(),
            affinity,
            display_snapshot.buffer_snapshot(),
        );
        return info.indent + local;
    }
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

fn text_runs(
    text: &str,
    global_byte_start: usize,
    display_prefix_len: usize,
    highlights: &[HighlightSpan],
    highlight_styles: &[HighlightStyle],
    marked_ranges: &[TextRange],
    base: TextRun,
) -> Vec<TextRun> {
    if highlights.is_empty() && marked_ranges.is_empty() {
        return vec![base];
    }

    let source_len = text.len().saturating_sub(display_prefix_len);
    let global_byte_end = global_byte_start + source_len;
    let mut boundaries = vec![0, text.len()];
    for highlight in highlights {
        let start = highlight
            .range
            .start
            .max(global_byte_start)
            .min(global_byte_end);
        let end = highlight
            .range
            .end
            .max(global_byte_start)
            .min(global_byte_end);
        if start < end {
            boundaries.push(display_prefix_len + start - global_byte_start);
            boundaries.push(display_prefix_len + end - global_byte_start);
        }
    }
    for marked in marked_ranges {
        let start = marked
            .start()
            .get()
            .max(global_byte_start)
            .min(global_byte_end);
        let end = marked
            .end()
            .get()
            .max(global_byte_start)
            .min(global_byte_end);
        if start < end {
            boundaries.push(display_prefix_len + start - global_byte_start);
            boundaries.push(display_prefix_len + end - global_byte_start);
        }
    }
    boundaries.sort_unstable();
    boundaries.dedup();

    let mut runs = Vec::with_capacity(boundaries.len().saturating_sub(1));
    let mut highlight_index = 0;
    for boundary in boundaries.windows(2) {
        let local_start = boundary[0];
        let local_end = boundary[1];
        if local_start == local_end {
            continue;
        }
        let global_offset = local_start
            .checked_sub(display_prefix_len)
            .map(|offset| global_byte_start + offset);
        let mut run = TextRun {
            len: local_end - local_start,
            ..base.clone()
        };
        if let Some(global_offset) = global_offset {
            while highlights
                .get(highlight_index)
                .is_some_and(|span| span.range.end <= global_offset)
            {
                highlight_index += 1;
            }
            if let Some(highlight) = highlights
                .get(highlight_index)
                .filter(|span| span.range.contains(&global_offset))
            {
                // capture 索引查预展开样式表；索引越界视为无样式，不崩溃渲染。
                let style = highlight_styles
                    .get(highlight.capture as usize)
                    .copied()
                    .unwrap_or_default();
                if let Some(color) = style.color {
                    run.color = color;
                }
                if let Some(weight) = style.font_weight {
                    run.font.weight = weight;
                }
                if let Some(font_style) = style.font_style {
                    run.font.style = font_style;
                }
                run.background_color = style.background_color;
                run.underline = style.underline;
                run.strikethrough = style.strikethrough;
            }
            if marked_ranges.iter().any(|marked| {
                marked.start().get() <= global_offset && global_offset < marked.end().get()
            }) {
                run.underline = Some(UnderlineStyle {
                    color: Some(run.color),
                    thickness: px(1.),
                    wavy: false,
                });
            }
        }
        runs.push(run);
    }
    runs
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
    use zcv_theme::syntax;

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
    fn diff_kind_for_row_matches_display_row_ranges() {
        // 输入是 diff_hunk_rows 的输出：Deleted 已从空区间展开为锚定行的单行区间。
        let diff_rows = vec![
            (2..5, DiffHunkKind::Modified),
            (7..8, DiffHunkKind::Deleted),
        ];

        assert_eq!(diff_kind_for_row(&diff_rows, 1), None);
        assert_eq!(
            diff_kind_for_row(&diff_rows, 2),
            Some(DiffHunkKind::Modified)
        );
        assert_eq!(
            diff_kind_for_row(&diff_rows, 4),
            Some(DiffHunkKind::Modified)
        );
        assert_eq!(diff_kind_for_row(&diff_rows, 5), None);
        assert_eq!(
            diff_kind_for_row(&diff_rows, 7),
            Some(DiffHunkKind::Deleted)
        );
        assert_eq!(diff_kind_for_row(&diff_rows, 8), None);
        assert_eq!(diff_kind_for_row(&[], 0), None);
    }

    #[test]
    fn marked_text_is_a_separate_underlined_text_run() {
        let text = "a中文b";
        let runs = text_runs(
            text,
            0,
            0,
            &[],
            &[],
            &[TextRange::new(ByteOffset::new(1), ByteOffset::new(7)).unwrap()],
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
    fn syntax_captures_apply_color_and_font_modifiers(cx: &mut TestAppContext) {
        // 显式挂载内置深色主题：capture 样式表来自 zcv-theme 的静态状态，
        // 不依赖其他测试的执行顺序。
        cx.update(|cx| {
            zcv_theme::ThemeChoice::Named("one-dark").apply(cx, None);
        });
        let text = "fn strong";
        let base = TextRun {
            len: text.len(),
            font: font("Helvetica"),
            color: Default::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let highlight_styles =
            syntax::style_table(&[Arc::from("keyword"), Arc::from("text.strong")]);
        let runs = text_runs(
            text,
            100,
            0,
            &[
                HighlightSpan {
                    range: 100..102,
                    capture: 0,
                },
                HighlightSpan {
                    range: 103..109,
                    capture: 1,
                },
            ],
            &highlight_styles,
            &[],
            base.clone(),
        );

        assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), text.len());
        assert_ne!(runs[0].color, base.color);
        assert_eq!(runs[2].font.weight, gpui::FontWeight::BOLD);
    }

    #[gpui::test]
    fn hit_test_uses_the_same_shaped_line_measurement_as_cursor_paint(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Empty);
        window
            .update(cx, |_, window, _cx| {
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
                        global_utf16_start: 0,
                        wrap_info: None,
                        git_diff: None,
                    }],
                    gutter: None,
                    text_clip_bounds: Bounds::new(point(px(0.), px(0.)), size(px(400.), px(100.))),
                    line_height: px(24.),
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
            .update(cx, |_, window, cx| {
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
                        diff_rows: &[],
                    },
                    window,
                    cx,
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
            .update(cx, |_, window, cx| {
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
                        diff_rows: &[],
                    },
                    window,
                    cx,
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
            .update(cx, |_, window, cx| {
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
                        diff_rows: &[],
                    },
                    window,
                    cx,
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
            .update(cx, |_, window, cx| {
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
                        diff_rows: &[],
                    },
                    window,
                    cx,
                );
                let (_, carets) =
                    layout_selections(&SelectionSet::caret(ByteOffset::ZERO), &layout, px(20.), cx);

                assert!(carets.is_empty());
            })
            .expect("测试窗口应保持可用");
    }

    #[gpui::test]
    fn diff_hunk_rows_maps_logical_rows_to_display_rows(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Empty);
        window
            .update(cx, |_, window, _cx| {
                let text_system = window.text_system().clone();
                let font = window.text_style().font();
                let font_size = window.text_style().font_size.to_pixels(window.rem_size());

                // 无 wrap：逻辑行 == 显示行；纯删除空范围锚定一个显示行。
                let buffer = Buffer::scratch(
                    "line 0\nline 1\nline 2\nline 3\nline 4\n".to_owned(),
                    BufferConfig::default(),
                )
                .expect("应创建 Buffer");
                let snapshot = DisplayMap::new(buffer.snapshot()).snapshot();
                assert_eq!(
                    diff_hunk_rows(
                        &snapshot,
                        &[
                            DiffHunk {
                                range: 1..2,
                                kind: DiffHunkKind::Modified,
                            },
                            DiffHunk {
                                range: 3..3,
                                kind: DiffHunkKind::Deleted,
                            },
                            DiffHunk {
                                range: 4..5,
                                kind: DiffHunkKind::Added,
                            },
                        ],
                    ),
                    vec![
                        (1..2, DiffHunkKind::Modified),
                        (3..4, DiffHunkKind::Deleted),
                        (4..5, DiffHunkKind::Added),
                    ]
                );

                // wrap：宽行拆成多个显示行，marker 覆盖全部片段。
                let buffer = Buffer::scratch(
                    "aaaa bbbb cccc dddd eeee ".repeat(10) + "\nline 1\n",
                    BufferConfig::default(),
                )
                .expect("应创建 Buffer");
                let mut map = DisplayMap::new(buffer.snapshot());
                assert!(
                    map.set_wrap_width(Some(px(100.)), font.clone(), font_size, &text_system),
                    "宽行应产生换行"
                );
                let snapshot = map.snapshot();
                let line_count = snapshot.line_count();
                assert!(line_count > 2, "宽行应拆成多个显示行");
                let row_1 = snapshot
                    .line_to_display_row(Line::new(1))
                    .expect("行 1 应可映射")
                    .get();

                // 行 0 wrap 成 N 段：marker 覆盖 [0, N)，N = 行 1 的行首显示行。
                assert_eq!(
                    diff_hunk_rows(
                        &snapshot,
                        &[DiffHunk {
                            range: 0..1,
                            kind: DiffHunkKind::Modified,
                        }],
                    ),
                    vec![(0..row_1, DiffHunkKind::Modified)]
                );

                // 越界 hunk（超出文件末尾）用 line_count 收尾。
                assert_eq!(
                    diff_hunk_rows(
                        &snapshot,
                        &[DiffHunk {
                            range: 1..10,
                            kind: DiffHunkKind::Modified,
                        }],
                    ),
                    vec![(row_1..line_count, DiffHunkKind::Modified)]
                );
            })
            .expect("测试窗口应保持可用");
    }
}
