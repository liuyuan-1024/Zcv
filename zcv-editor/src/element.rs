//! Editor 的逐帧文本布局、绘制与像素命中测试。

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::Arc;

use gpui::{
    App, Bounds, ContentMask, Context, DispatchPhase, Element, ElementId, ElementInputHandler,
    Entity, GlobalElementId, HitboxBehavior, InspectorElementId, InteractiveElement, IntoElement,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    ScrollWheelEvent, ShapedLine, SharedString, Style, TextRun, TransformationMatrix, Window, fill,
    point, px, relative, size,
};
use zcv_buffer_diff::{DiffHunk, DiffHunkKind};
use zcv_engine::{ByteOffset, Line, SelectionSet, TextRange};
use zcv_language::BracketPair;
use zcv_theme::color;

use super::display_map::{
    BufferPoint, DisplayColumn, DisplayPoint, DisplayRow, DisplaySnapshot, LineStyles,
    ProjectedRange, StreamLineSource, WrapViewportRowKind, byte_for_display_column, chunks_to_runs,
    synthesize_line_chunks,
};
use super::gutter::{GutterDimensions, GutterLayout, GutterRow};
use super::scroll::ScrollbarThumbState;
use super::scrollbar::{SCROLLBAR_WIDTH, ScrollbarLayout, marker_column_x_range, marker_geometry};
use super::view::{Editor, EditorMode, EditorPresentation, SoftWrap};

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
            .on_action(cx.listener(Editor::handle_toggle_fold))
            .on_action(cx.listener(Editor::handle_unfold_all))
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
    /// 可折叠行集合（crease 显示判断；prepaint 从语言层折叠范围计算）。
    foldable_lines: &'a BTreeSet<Line>,
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
    /// hunk 色带 hitbox（起点行可见时插入；点击切换折叠/展开；类型 + 展开态标志）。
    deleted_hunk_hitboxes: Arc<Vec<HunkHitbox>>,
    /// crease 点击 hitbox（可折叠行的 gutter 指示区；点击切换折叠/展开）。
    crease_hitboxes: Arc<Vec<(gpui::Hitbox, Line)>>,
    /// 折叠省略号点击 hitbox（anchor 行行尾；点击展开）。
    ellipsis_hitboxes: Arc<Vec<(gpui::Hitbox, Line)>>,
    /// 折叠入口行 → 结尾闭合符（如 `}`；无闭合符的折叠为 None）：行尾省略号样式用。
    fold_anchor_end_chars: Arc<BTreeMap<Line, Option<char>>>,
    /// hunk 竖条范围与状态色（竖条色不随展开变化；行背景按行状态另行绘制）。
    hunk_strips: Arc<Vec<(Range<usize>, DiffHunkKind)>>,
    scrollbar: Option<ScrollbarLayout>,
}

/// hunk 色带 hitbox：命中区域 + 点击目标范围 + 类型 + 展开态标志。
type HunkHitbox = (gpui::Hitbox, Range<usize>, DiffHunkKind, bool);

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
        // 折叠入口行集合（anchor 行行尾绘制省略号；display_snapshot 后续被 autoscroll 分支移动）。
        let fold_anchor_end_chars: Arc<BTreeMap<Line, Option<char>>> = Arc::new(
            display_snapshot
                .fold_anchor_lines()
                .into_iter()
                .map(|line| (line, display_snapshot.fold_end_char(line)))
                .collect(),
        );
        // 可折叠行集合（crease 显示判断：折叠范围起点行即折叠入口行）。
        let foldable_lines: BTreeSet<Line> = {
            let editor = self.editor.read(cx);
            let snapshot = display_snapshot.buffer_snapshot();
            editor
                .fold_ranges()
                .iter()
                .filter_map(|range| {
                    snapshot
                        .byte_to_line(ByteOffset::new(range.range.start))
                        .ok()
                })
                .collect()
        };
        // git diff 显示行区间：gutter 指示、内容背景与滚动轴 marker 共用（只依赖 snapshot 与注入 hunks，与滚动位置无关，autoscroll 重排可复用）。
        let diff_rows = {
            let editor = self.editor.read(cx);
            diff_hunk_rows(
                &display_snapshot,
                editor.diff_hunks(cx),
                editor.expanded_deleted_hunks(),
                editor.expanded_modified_hunks(),
            )
        };
        let mut layout = layout_visible_lines(
            display_snapshot.clone(),
            presentation.clone(),
            VisibleLineLayoutParams {
                geometry,
                active_lines: &active_lines,
                foldable_lines: &foldable_lines,
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
                    foldable_lines: &foldable_lines,
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
        // hunk 竖条范围与状态色（竖条不随展开变化；行背景按行状态另行绘制）。
        let hunk_strips = Arc::new({
            let editor = self.editor.read(cx);
            hunk_strip_rows(
                &layout.display_snapshot,
                editor.diff_hunks(cx),
                editor.expanded_deleted_hunks(),
                editor.expanded_modified_hunks(),
            )
        });
        // hunk 色带 hitbox：点击切换折叠/展开（对齐 Zed：hitbox 挂在色带区域，BlockMouse 不穿透）。
        let deleted_hunk_hitboxes = Arc::new({
            let editor = self.editor.read(cx);
            let mut hitboxes = Vec::new();
            if let Some(gutter) = &layout.gutter {
                let strip_width = gutter_strip_width(line_height);
                for (rows, old_range, kind) in hunk_hit_regions(
                    &layout.display_snapshot,
                    editor.diff_hunks(cx),
                    editor.expanded_deleted_hunks(),
                    editor.expanded_modified_hunks(),
                ) {
                    // 色带起点行可见才可点击（滚动后起点进入视口自然恢复）。
                    let Some(start_line) = layout
                        .lines
                        .iter()
                        .find(|line| line.row.get() == rows.start)
                    else {
                        continue;
                    };
                    // 折叠的删除块是红色三角标记；其余是普通色带区域。
                    let expanded = match kind {
                        DiffHunkKind::Deleted => {
                            editor.expanded_deleted_hunks().contains(&old_range)
                        }
                        DiffHunkKind::Modified => {
                            editor.expanded_modified_hunks().contains(&old_range)
                        }
                        DiffHunkKind::Added => false,
                    };
                    let width = strip_width;
                    hitboxes.push((
                        window.insert_hitbox(
                            Bounds::from_corners(
                                point(gutter.bounds.left(), start_line.origin.y),
                                point(
                                    gutter.bounds.left() + width,
                                    start_line.origin.y
                                        + line_height * (rows.end - rows.start) as f32,
                                ),
                            ),
                            HitboxBehavior::BlockMouse,
                        ),
                        old_range,
                        kind,
                        expanded,
                    ));
                }
            }
            hitboxes
        });
        // 折叠省略号点击 hitbox：anchor 行行尾的省略号区域（点击展开；交互型直接调 Entity 方法）。
        let ellipsis_hitboxes = Arc::new({
            let mut hitboxes = Vec::new();
            if !fold_anchor_end_chars.is_empty() {
                let text_style = window.text_style();
                let font_size = text_style.font_size.to_pixels(window.rem_size());
                let run = TextRun {
                    len: 0,
                    font: text_style.font(),
                    color: text_style.color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                for (index, line) in layout.lines.iter().enumerate() {
                    let Some(logical_line) = line.logical_line else {
                        continue;
                    };
                    // 软换行下省略号只落在逻辑行的最后显示行行尾。
                    let is_last_fragment = layout
                        .lines
                        .get(index + 1)
                        .is_none_or(|next| next.logical_line != line.logical_line);
                    if !is_last_fragment {
                        continue;
                    }
                    let Some(end_char) = fold_anchor_end_chars.get(&logical_line) else {
                        continue;
                    };
                    let suffix = match end_char {
                        Some(end_char) => format!("…{end_char}"),
                        None => "…".to_owned(),
                    };
                    let suffix_run = TextRun {
                        len: suffix.len(),
                        ..run.clone()
                    };
                    let suffix_width = window
                        .text_system()
                        .shape_line(suffix.into(), font_size, &[suffix_run], None)
                        .width;
                    hitboxes.push((
                        window.insert_hitbox(
                            Bounds::from_corners(
                                point(line.origin.x + line.shaped.width, line.origin.y),
                                point(
                                    line.origin.x + line.shaped.width + suffix_width,
                                    line.origin.y + line_height,
                                ),
                            ),
                            HitboxBehavior::BlockMouse,
                        ),
                        logical_line,
                    ));
                }
            }
            hitboxes
        });
        // crease 点击 hitbox：可折叠行的 gutter 左侧指示区（对齐 Zed：交互型点击直接调 Entity 方法）。
        let crease_hitboxes = Arc::new({
            let mut hitboxes = Vec::new();
            if let (Some(gutter), Some((gutter_bounds, dimensions))) =
                (&layout.gutter, geometry.gutter)
            {
                for row in &gutter.rows {
                    if row.crease.is_none() {
                        continue;
                    }
                    hitboxes.push((
                        window.insert_hitbox(
                            Bounds::from_corners(
                                point(
                                    gutter_bounds.right() - dimensions.crease_width,
                                    row.origin.y,
                                ),
                                point(gutter_bounds.right(), row.origin.y + line_height),
                            ),
                            HitboxBehavior::BlockMouse,
                        ),
                        row.logical_line,
                    ));
                }
            }
            hitboxes
        });
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
            // 折叠的删除块行内无标记，滚动条 marker 仍指示删除位置。
            let folded_deleted_markers: Vec<(Range<usize>, DiffHunkKind)> = {
                let editor = self.editor.read(cx);
                hunk_hit_regions(
                    &layout.display_snapshot,
                    editor.diff_hunks(cx),
                    editor.expanded_deleted_hunks(),
                    editor.expanded_modified_hunks(),
                )
                .iter()
                .filter(|(_, old_range, kind)| {
                    *kind == DiffHunkKind::Deleted
                        && !editor.expanded_deleted_hunks().contains(old_range)
                })
                .map(|(rows, _, _)| (rows.clone(), DiffHunkKind::Deleted))
                .collect()
            };
            scrollbar_layout.markers = marker_geometry(
                diff_rows.iter().cloned().chain(folded_deleted_markers),
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
            deleted_hunk_hitboxes,
            crease_hitboxes,
            ellipsis_hitboxes,
            fold_anchor_end_chars,
            hunk_strips,
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
        let deleted_hunk_hitboxes = prepaint.deleted_hunk_hitboxes.clone();
        let crease_hitboxes = prepaint.crease_hitboxes.clone();
        let ellipsis_hitboxes = prepaint.ellipsis_hitboxes.clone();
        let mouse_focus = focus.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble
                || event.button != MouseButton::Left
                || !hitbox.is_hovered(window)
            {
                return;
            }
            // hunk 色带点击：切换折叠/展开（先于 gutter 行号选行命中）。
            if let Some((_, old_range, kind, _)) = deleted_hunk_hitboxes
                .iter()
                .find(|(hitbox, _, _, _)| hitbox.is_hovered(window))
            {
                editor.update(cx, |editor, cx| {
                    match kind {
                        DiffHunkKind::Deleted => editor.toggle_deleted_hunk(old_range.clone(), cx),
                        DiffHunkKind::Modified => {
                            editor.toggle_modified_hunk(old_range.clone(), cx)
                        }
                        DiffHunkKind::Added => {}
                    }
                    cx.notify();
                });
                window.focus(&mouse_focus);
                cx.stop_propagation();
                return;
            }
            // 折叠省略号 / crease 点击：切换该行折叠/展开（交互型，直接调 Entity 方法；
            // 省略号更具体，先命中；两者都先于 gutter 行号选行）。
            if let Some((_, line)) = ellipsis_hitboxes
                .iter()
                .chain(crease_hitboxes.iter())
                .find(|(hitbox, _)| hitbox.is_hovered(window))
            {
                editor.update(cx, |editor, cx| editor.toggle_fold_at_line(*line, cx));
                window.focus(&mouse_focus);
                cx.stop_propagation();
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
            // hunk 色带可点击：hover 时手型光标（对齐 Zed 的 PointingHand）。
            for (hitbox, _, _, _) in prepaint.deleted_hunk_hitboxes.iter() {
                if hitbox.is_hovered(window) {
                    window.set_cursor_style(gpui::CursorStyle::PointingHand, hitbox);
                }
            }
            // 折叠省略号与折叠箭头可点击：hover 时手型光标。
            for (hitbox, _) in prepaint
                .ellipsis_hitboxes
                .iter()
                .chain(prepaint.crease_hitboxes.iter())
            {
                if hitbox.is_hovered(window) {
                    window.set_cursor_style(gpui::CursorStyle::PointingHand, hitbox);
                }
            }
        }
        if let Some(gutter) = &prepaint.layout.gutter {
            let colors = color::current(cx);
            // diff 行 gutter 背景：按布局行的行状态绘制（展开的修改块旧行红、修改行绿）。
            let strip_width = gutter_strip_width(gutter.line_height);
            for line in &prepaint.layout.lines {
                let Some(kind) = line.git_diff else {
                    continue;
                };
                let background = match kind {
                    DiffHunkKind::Added => colors.editor_diff_added_background,
                    DiffHunkKind::Modified => colors.editor_diff_modified_background,
                    DiffHunkKind::Deleted => colors.editor_diff_deleted_background,
                };
                window.paint_quad(fill(
                    Bounds::from_corners(
                        point(gutter.bounds.left(), line.origin.y),
                        point(gutter.bounds.right(), line.origin.y + gutter.line_height),
                    ),
                    background,
                ));
            }
            // git diff 竖条：hunk 状态色（不随展开变化，对齐 Zed paint_gutter_diff_hunks）；
            // 展开的修改块竖条保持黄色并覆盖整个 hunk（旧行 + 修改行）。
            for (rows, kind) in prepaint.hunk_strips.iter() {
                let strip_color = match kind {
                    DiffHunkKind::Added => colors.status_created,
                    DiffHunkKind::Modified => colors.status_modified,
                    DiffHunkKind::Deleted => colors.status_deleted,
                };
                for line in prepaint
                    .layout
                    .lines
                    .iter()
                    .filter(|line| rows.contains(&line.row.get()))
                {
                    window.paint_quad(fill(
                        Bounds::from_corners(
                            point(gutter.bounds.left(), line.origin.y),
                            point(
                                gutter.bounds.left() + strip_width,
                                line.origin.y + gutter.line_height,
                            ),
                        ),
                        strip_color,
                    ));
                }
            }
            // 折叠的删除块：gutter 红色三角,行内无整行着色，点击展开。
            // 分界线 = 锚定行底部（删除点在被删行原位置，即锚定行之后一行）。
            for (hitbox, _, kind, expanded) in prepaint.deleted_hunk_hitboxes.iter() {
                if *kind == DiffHunkKind::Deleted && !expanded {
                    let bounds = hitbox.bounds;
                    let half = strip_width * 0.8;
                    let mut triangle = gpui::PathBuilder::fill();
                    triangle.move_to(point(bounds.left(), bounds.bottom() - half));
                    triangle.line_to(point(bounds.left(), bounds.bottom() + half));
                    triangle.line_to(point(bounds.right(), bounds.bottom()));
                    triangle.close();
                    if let Ok(path) = triangle.build() {
                        window.paint_path(path, colors.status_deleted);
                    }
                }
            }
            // crease 箭头：未折叠显示向下箭头（点击折叠），已折叠显示向右箭头（点击展开）。
            // 用 SVG 资源而非 unicode 字形：多字节字符在 TextRun 的字节索引语义下会越界。
            let crease_size = (gutter.line_height * 0.75).min(gutter.crease_width);
            let crease_icon_color = colors.icon;
            for row in &gutter.rows {
                let Some(folded) = row.crease else {
                    continue;
                };
                let path: SharedString = if folded {
                    "icons/editor/chevron_right.svg".into()
                } else {
                    "icons/editor/chevron_down.svg".into()
                };
                let crease_left = gutter.bounds.right() - gutter.crease_width;
                let bounds = Bounds::from_corners(
                    point(
                        crease_left + (gutter.crease_width - crease_size) / 2.0,
                        row.origin.y + (gutter.line_height - crease_size) / 2.0,
                    ),
                    point(
                        crease_left + (gutter.crease_width + crease_size) / 2.0,
                        row.origin.y + (gutter.line_height + crease_size) / 2.0,
                    ),
                );
                if let Err(error) = window.paint_svg(
                    bounds,
                    path,
                    TransformationMatrix::default(),
                    crease_icon_color.into(),
                    cx,
                ) {
                    // 单个图标绘制失败只跳过该行，不能让整个窗口崩溃（对齐 Zed 的降级策略）。
                    log::error!("Editor crease 绘制失败：{error}");
                    continue;
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
                // 折叠省略号：绘制在折叠入口行的最后显示行行尾（对齐 Zed 行内占位样式）。
                // 不进入文本流，命中测试与光标定位不受影响。
                let text_style = window.text_style();
                let ellipsis_font_size = text_style.font_size.to_pixels(window.rem_size());
                let ellipsis_run = TextRun {
                    len: 0,
                    font: text_style.font(),
                    color: color::current(cx).text_placeholder.into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let fold_anchor_end_chars = &prepaint.fold_anchor_end_chars;
                for (index, line) in prepaint.layout.lines.iter().enumerate() {
                    let Some(logical_line) = line.logical_line else {
                        continue;
                    };
                    // 软换行下省略号只落在逻辑行的最后显示行行尾。
                    let is_last_fragment = prepaint
                        .layout
                        .lines
                        .get(index + 1)
                        .is_none_or(|next| next.logical_line != line.logical_line);
                    if !is_last_fragment {
                        continue;
                    }
                    // 块折叠在省略号后补闭合符（如 `}`），呈现 `{...}`；注释组等只有省略号。
                    let suffix = match fold_anchor_end_chars.get(&logical_line) {
                        Some(Some(end_char)) => format!("…{end_char}"),
                        Some(None) => "…".to_owned(),
                        None => continue,
                    };
                    let suffix_run = TextRun {
                        len: suffix.len(),
                        ..ellipsis_run.clone()
                    };
                    let shaped = window.text_system().shape_line(
                        suffix.into(),
                        ellipsis_font_size,
                        &[suffix_run],
                        None,
                    );
                    let origin = point(line.origin.x + line.shaped.width, line.origin.y);
                    if let Err(error) =
                        shaped.paint(origin, prepaint.layout.line_height, window, cx)
                    {
                        // 单个字形绘制失败只跳过该行，不能让整个窗口崩溃（对齐 Zed 的降级策略）。
                        log::error!("Editor 折叠省略号绘制失败：{error}");
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

/// hunks（逻辑行）→ 行级标记的显示行区间：wrap 下行映射出的全部显示行都覆盖。
///
/// 覆盖终点取 hunk 之后第一行的行首显示行（对齐 Zed：end 行首显示行 − 1 即 hunk 最后一个显示行，左闭右开区间 [start, end) 恰好盖住全部 wrap 片段）；
/// hunk 到达文件末尾时以显示快照行数为终点。
/// 纯删除 hunk（空范围）：折叠时行内不做标记（gutter 红色三角提示）；
/// 展开后标记移到被删除的合成行上（锚定行之后的显示行），幸存的锚定行不被标记。
/// 修改 hunk 展开后：旧行（合成行）按删除色、修改行按新增色（对齐 Zed：base 旧行红、新行绿）。
/// 映射失败（越界等）跳过该 hunk。
fn diff_hunk_rows(
    snapshot: &DisplaySnapshot,
    hunks: &[DiffHunk],
    expanded_deleted: &[Range<usize>],
    expanded_modified: &[Range<usize>],
) -> Vec<(Range<usize>, DiffHunkKind)> {
    let mut rows = Vec::new();
    for hunk in hunks {
        if hunk.kind == DiffHunkKind::Added {
            let Some(start) = snapshot.line_to_display_row(Line::new(hunk.range.start)) else {
                continue;
            };
            let start = start.get();
            let end = match snapshot.line_to_display_row(Line::new(hunk.range.end)) {
                Some(row) => row.get(),
                None => snapshot.line_count(),
            };
            rows.push((start..end.max(start + 1), DiffHunkKind::Added));
            continue;
        }
        if hunk.kind == DiffHunkKind::Modified {
            let expanded = expanded_modified.contains(&hunk.old_range);
            if expanded {
                // 展开：旧行（合成行，删除色）+ 修改行（新增色）。
                if let Some(old_rows) = modified_hunk_rows(snapshot, hunk, true) {
                    rows.push((old_rows, DiffHunkKind::Deleted));
                }
                if let Some(new_rows) = modified_hunk_rows(snapshot, hunk, false) {
                    rows.push((new_rows, DiffHunkKind::Added));
                }
            } else if let Some(hunk_rows) = modified_hunk_rows(snapshot, hunk, false) {
                rows.push((hunk_rows, DiffHunkKind::Modified));
            }
            continue;
        }
        let expanded = expanded_deleted.contains(&hunk.old_range);
        if !expanded {
            continue;
        }
        if let Some(deleted_rows) = deleted_hunk_rows(snapshot, hunk, true) {
            rows.push((deleted_rows, DiffHunkKind::Deleted));
        }
    }
    rows
}

/// 纯删除 hunk 的显示行范围：未展开 = 锚定行（删除点所在显示行，色带顶部指向分界线）；
/// 展开 = 锚定行之后的连续合成行（被删除行显示在原位置，左闭右开）。
fn deleted_hunk_rows(
    snapshot: &DisplaySnapshot,
    hunk: &DiffHunk,
    expanded: bool,
) -> Option<Range<usize>> {
    let anchor = snapshot
        .line_to_display_row(Line::new(hunk.range.start))?
        .get();
    if !expanded {
        return Some(anchor..anchor + 1);
    }
    // 被删合成行插在锚定行（range.start）之后：从锚定行后第一个合成行数到连续合成行结束。
    let line_count = snapshot.line_count();
    let mut start = anchor + 1;
    while start < line_count && !display_row_is_inserted(snapshot, start) {
        start += 1;
    }
    let mut end = start;
    while end < line_count && display_row_is_inserted(snapshot, end) {
        end += 1;
    }
    (start < end).then_some(start..end)
}

/// 显示行是否为合成行（外部文本；wrap 片段同样计入）。
fn display_row_is_inserted(snapshot: &DisplaySnapshot, row: usize) -> bool {
    snapshot
        .slice_viewport(DisplayRow::new(row), 1)
        .is_ok_and(|viewport| {
            viewport.rows().first().is_some_and(|row| {
                matches!(
                    row.kind(),
                    WrapViewportRowKind::Text {
                        source: StreamLineSource::Inserted { .. },
                        ..
                    }
                )
            })
        })
}

/// 可点击的删除块色带区域（显示行范围 + old_range；折叠/展开两态都覆盖，供 gutter 点击切换）。
/// 修改 hunk 的显示行范围：未展开 = 修改行本身；展开 = 修改行上方的连续合成行（旧行）。
fn modified_hunk_rows(
    snapshot: &DisplaySnapshot,
    hunk: &DiffHunk,
    expanded: bool,
) -> Option<Range<usize>> {
    let anchor = snapshot
        .line_to_display_row(Line::new(hunk.range.start))?
        .get();
    if !expanded {
        let end = match snapshot.line_to_display_row(Line::new(hunk.range.end)) {
            Some(row) => row.get(),
            None => snapshot.line_count(),
        };
        return Some(anchor..end.max(anchor + 1));
    }
    // 合成行（HEAD 旧行）插在修改行上方：从修改行首显示行往前数连续合成行。
    let mut start = anchor;
    while start > 0 && display_row_is_inserted(snapshot, start - 1) {
        start -= 1;
    }
    (start < anchor).then_some(start..anchor)
}

/// hunk 竖条范围与状态色。
///
/// 竖条颜色不随展开变化（对齐 Zed：色带始终用 hunk 状态着色）；展开的修改块竖条覆盖
/// 整个 hunk（旧行 + 修改行）；折叠的删除块无竖条（gutter 红色三角标记）。
fn hunk_strip_rows(
    snapshot: &DisplaySnapshot,
    hunks: &[DiffHunk],
    expanded_deleted: &[Range<usize>],
    expanded_modified: &[Range<usize>],
) -> Vec<(Range<usize>, DiffHunkKind)> {
    let mut rows = Vec::new();
    for hunk in hunks {
        match hunk.kind {
            DiffHunkKind::Added => {
                let Some(start) = snapshot.line_to_display_row(Line::new(hunk.range.start)) else {
                    continue;
                };
                let start = start.get();
                let end = match snapshot.line_to_display_row(Line::new(hunk.range.end)) {
                    Some(row) => row.get(),
                    None => snapshot.line_count(),
                };
                rows.push((start..end.max(start + 1), DiffHunkKind::Added));
            }
            DiffHunkKind::Modified => {
                let expanded = expanded_modified.contains(&hunk.old_range);
                if expanded {
                    // 覆盖整个 hunk：旧行合成行到修改行结束。
                    let Some(old_rows) = modified_hunk_rows(snapshot, hunk, true) else {
                        continue;
                    };
                    let Some(new_rows) = modified_hunk_rows(snapshot, hunk, false) else {
                        continue;
                    };
                    rows.push((old_rows.start..new_rows.end, DiffHunkKind::Modified));
                } else if let Some(hunk_rows) = modified_hunk_rows(snapshot, hunk, false) {
                    rows.push((hunk_rows, DiffHunkKind::Modified));
                }
            }
            DiffHunkKind::Deleted => {
                if expanded_deleted.contains(&hunk.old_range)
                    && let Some(deleted_rows) = deleted_hunk_rows(snapshot, hunk, true)
                {
                    rows.push((deleted_rows, DiffHunkKind::Deleted));
                }
            }
        }
    }
    rows
}

/// 可点击的 hunk 色带区域（显示行范围 + 点击目标范围 + 类型）。
///
/// Deleted：折叠态在锚定行、展开态覆盖合成行；Modified：未展开覆盖修改行、展开覆盖旧行合成行；
/// Added 不参与点击（保持原状）。
fn hunk_hit_regions(
    snapshot: &DisplaySnapshot,
    hunks: &[DiffHunk],
    expanded_deleted: &[Range<usize>],
    expanded_modified: &[Range<usize>],
) -> Vec<(Range<usize>, Range<usize>, DiffHunkKind)> {
    hunks
        .iter()
        .filter_map(|hunk| {
            let rows = match hunk.kind {
                DiffHunkKind::Deleted => {
                    let expanded = expanded_deleted.contains(&hunk.old_range);
                    deleted_hunk_rows(snapshot, hunk, expanded)?
                }
                DiffHunkKind::Modified => {
                    let expanded = expanded_modified.contains(&hunk.old_range);
                    if expanded {
                        // 覆盖整个 hunk（旧行 + 修改行，与竖条范围一致）。
                        let old_rows = modified_hunk_rows(snapshot, hunk, true)?;
                        let new_rows = modified_hunk_rows(snapshot, hunk, false)?;
                        old_rows.start..new_rows.end
                    } else {
                        modified_hunk_rows(snapshot, hunk, false)?
                    }
                }
                DiffHunkKind::Added => return None,
            };
            Some((rows, hunk.old_range.clone(), hunk.kind))
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
    let WrapViewportRowKind::Text {
        text, byte_range, ..
    } = row.kind();
    let text = &text.as_ref()[byte_range.clone()];
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
        foldable_lines,
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
                } = row.kind();
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

    // 基础 run：样式段在其上合并（对齐 Zed from_chunks 的 base 合并）。
    let base = TextRun {
        len: 0,
        font: text_style.font(),
        color: text_style.color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };

    let mut push_line = |row: usize,
                         logical_line: Option<Line>,
                         gutter_line: Option<Line>,
                         text: &str,
                         utf16_start: usize,
                         wrap_info: Option<WrapRowInfo>,
                         runs: Vec<TextRun>| {
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
            // 折叠指示：已折叠常显，可折叠行常显（不依赖光标位置）。
            let crease = if display_snapshot.is_line_folded(logical_line) {
                Some(true)
            } else if foldable_lines.contains(&logical_line) {
                Some(false)
            } else {
                None
            };
            gutter_rows.push(GutterRow {
                logical_line,
                origin: point(
                    gutter_bounds.right()
                        - dimensions.right_padding
                        - dimensions.crease_width
                        - shaped_line_number.width,
                    text_bounds.top() + line_height * (row - start) - scroll_offset.y,
                ),
                shaped_line_number,
                active,
                crease,
            });
        }
    };

    if let Ok(viewport) =
        display_snapshot.slice_viewport(DisplayRow::new(start), end.saturating_sub(start))
    {
        for row in viewport.rows() {
            match row.kind() {
                WrapViewportRowKind::Text {
                    source,
                    text,
                    byte_range,
                    global_byte_start,
                    fragment_index,
                    indent,
                    column_base,
                    ..
                } => {
                    // 对齐 Zed highlighted_chunks：
                    // 整行合成 chunk 流（inlay 注入 + 样式切分 + tab 展开），软换行片段按投影范围裁剪；
                    // 展开后字符列 = 显示列。
                    let tab_width = display_snapshot.buffer_snapshot().config().tab.tab_width();
                    // 行内提示（inlay）：经消费链查询行的注入段（投影偏移已含此前注入前缀）。
                    let inlay_snapshot = display_snapshot
                        .wrap_snapshot()
                        .tab_snapshot()
                        .fold_snapshot()
                        .inlay_snapshot();
                    let stream_line = match source {
                        StreamLineSource::Buffer(buffer_line) => inlay_snapshot
                            .stream()
                            .buffer_to_stream(Line::new(*buffer_line)),
                        StreamLineSource::Inserted { anchor, index } => {
                            let start = inlay_snapshot
                                .stream()
                                .inserted_block_start(*anchor)
                                .expect("合成行必须属于锚定块的插入表");
                            Line::new(start.get() + index)
                        }
                    };
                    let inlays = inlay_snapshot.line_inlays(stream_line);
                    // 合成行是外部文本：无语法高亮、不可编辑/不可选（spans/marked 是锚定行的 buffer 坐标，套用到合成行文本会产生非字符边界切片）。
                    let line_styles = match source {
                        StreamLineSource::Buffer(_) => LineStyles {
                            spans: visible_highlights,
                            styles: highlight_styles,
                            marked: presentation.marked_ranges(),
                        },
                        StreamLineSource::Inserted { .. } => LineStyles::default(),
                    };
                    let synthesized = synthesize_line_chunks(
                        text.as_ref(),
                        tab_width,
                        *global_byte_start,
                        &inlays,
                        line_styles,
                        byte_range.clone(),
                    );
                    // 显示文本：wrap 假空格 + 展开 chunk 文本拼接（对齐 Zed from_chunks）。
                    let display_len: usize = *indent
                        + synthesized
                            .chunks
                            .iter()
                            .map(|chunk| chunk.text.len())
                            .sum::<usize>();
                    let mut display_text = String::with_capacity(display_len);
                    if *indent > 0 {
                        display_text.push_str(&" ".repeat(*indent));
                    }
                    for chunk in &synthesized.chunks {
                        display_text.push_str(chunk.text);
                    }
                    let mut runs = Vec::with_capacity(synthesized.chunks.len() + 1);
                    if *indent > 0 {
                        runs.push(TextRun {
                            len: *indent,
                            ..base.clone()
                        });
                    }
                    runs.extend(chunks_to_runs(&synthesized.chunks, base.clone()));
                    let utf16_start = synthesized.utf16_start;
                    let logical_line = match source {
                        StreamLineSource::Buffer(buffer_line) => Some(Line::new(*buffer_line)),
                        StreamLineSource::Inserted { .. } => None,
                    };
                    let wrap_info = (*fragment_index > 0).then_some(WrapRowInfo {
                        line: logical_line.unwrap_or(Line::ZERO),
                        indent: *indent,
                        column_base: *column_base,
                    });
                    push_line(
                        row.index().get(),
                        logical_line,
                        // 行号只在逻辑行首显示行出现。
                        match source {
                            StreamLineSource::Buffer(buffer_line) if *fragment_index == 0 => {
                                Some(Line::new(*buffer_line))
                            }
                            _ => None,
                        },
                        &display_text,
                        utf16_start,
                        wrap_info,
                        runs,
                    );
                }
            }
        }
    }

    EditorLayout {
        lines,
        gutter: gutter_geometry.map(|(bounds, dimensions)| GutterLayout {
            bounds,
            line_height,
            rows: gutter_rows,
            crease_width: dimensions.crease_width,
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
        let local = byte_for_display_column(
            fragment,
            info.indent,
            point.column().get(),
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

fn column_to_byte(text: &str, column: usize) -> usize {
    text.char_indices()
        .nth(column)
        .map_or(text.len(), |(byte, _)| byte)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display_map::DisplayMap;
    use gpui::{Empty, TestAppContext, font};
    use zcv_engine::{Buffer, BufferConfig, ByteOffset, Line, LineRange};
    use zcv_language::HighlightSpan;
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
        let base = TextRun {
            len: 0,
            font: font("Helvetica"),
            color: Default::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let synthesized = synthesize_line_chunks(
            text,
            4,
            0,
            &[],
            LineStyles {
                spans: &[],
                styles: &[],
                marked: &[TextRange::new(ByteOffset::new(1), ByteOffset::new(7)).unwrap()],
            },
            0..text.len(),
        );
        let runs = chunks_to_runs(&synthesized.chunks, base);

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
            len: 0,
            font: font("Helvetica"),
            color: Default::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let highlight_styles =
            syntax::style_table(&[Arc::from("keyword"), Arc::from("text.strong")]);
        let synthesized = synthesize_line_chunks(
            text,
            4,
            100,
            &[],
            LineStyles {
                spans: &[
                    HighlightSpan {
                        range: 100..102,
                        capture: 0,
                    },
                    HighlightSpan {
                        range: 103..109,
                        capture: 1,
                    },
                ],
                styles: &highlight_styles,
                marked: &[],
            },
            0..text.len(),
        );
        let runs = chunks_to_runs(&synthesized.chunks, base.clone());

        assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), text.len());
        assert_ne!(runs[0].color, base.color);
        assert_eq!(runs[2].font.weight, gpui::FontWeight::BOLD);
    }

    #[gpui::test]
    #[gpui::test]
    fn inserted_lines_render_without_applying_anchor_spans(cx: &mut TestAppContext) {
        // 回归：合成行（外部文本）无语法高亮/选区——锚定行的 span 端点套用到合成行文本
        // 会落在中文中间（非字符边界切片 panic）。
        let window = cx.add_window(|_, _| Empty);
        window
            .update(cx, |_, window, cx| {
                let snapshot = Buffer::scratch("// a\n// b".to_owned(), BufferConfig::default())
                    .expect("测试 Buffer 应能创建")
                    .snapshot();
                let mut map = DisplayMap::new(snapshot.clone());
                // 展开删除块：锚定行 0 后插入含中文的 HEAD 行；锚定行短，span 端点在锚定行外。
                map.set_inserted(crate::display_map::InsertedLines::from([(
                    Line::ZERO,
                    vec![std::sync::Arc::from(
                        "// 展开产生的新行在重建时现查 git 状态，无需单独补齐。",
                    )],
                )]));
                map.set_highlights(
                    std::sync::Arc::from([HighlightSpan {
                        range: 0..16,
                        capture: 0,
                    }]),
                    snapshot.version(),
                    std::sync::Arc::from([]),
                );
                let layout = layout_visible_lines(
                    map.snapshot(),
                    EditorPresentation::new(&snapshot, None),
                    VisibleLineLayoutParams {
                        geometry: EditorGeometry {
                            text_bounds: Bounds::new(
                                point(px(0.), px(0.)),
                                size(px(600.), px(100.)),
                            ),
                            text_clip_bounds: Bounds::new(
                                point(px(0.), px(0.)),
                                size(px(600.), px(100.)),
                            ),
                            gutter: None,
                        },
                        active_lines: &BTreeSet::new(),
                        foldable_lines: &BTreeSet::new(),
                        start_row: DisplayRow::ZERO,
                        scroll_offset: point(px(0.), px(0.)),
                        line_height: px(20.),
                        diff_rows: &[],
                    },
                    window,
                    cx,
                );
                // 合成行完整渲染（无逻辑行、无样式）。
                let inserted = layout
                    .lines
                    .iter()
                    .find(|line| line.logical_line.is_none())
                    .expect("应有合成行");
                assert_eq!(
                    inserted.shaped.text.as_ref(),
                    "// 展开产生的新行在重建时现查 git 状态，无需单独补齐。"
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
                        foldable_lines: &BTreeSet::new(),
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
                    crease_width: px(8.),
                    left_padding: px(8.),
                    right_padding: px(8.),
                    width: px(56.),
                    margin: px(3.),
                };
                let gutter_bounds =
                    Bounds::new(point(px(0.), px(0.)), size(dimensions.width, px(100.)));
                let text_bounds = Bounds::new(point(px(59.), px(0.)), size(px(341.), px(100.)));
                let layout = layout_visible_lines(
                    DisplayMap::new(snapshot.clone()).snapshot(),
                    EditorPresentation::new(&snapshot, None),
                    VisibleLineLayoutParams {
                        geometry: EditorGeometry {
                            text_bounds,
                            text_clip_bounds: Bounds::new(
                                point(px(56.), px(0.)),
                                size(px(344.), px(100.)),
                            ),
                            gutter: Some((gutter_bounds, dimensions)),
                        },
                        active_lines: &BTreeSet::from([Line::new(1)]),
                        foldable_lines: &BTreeSet::new(),
                        start_row: DisplayRow::ZERO,
                        scroll_offset: point(px(20.), px(0.)),
                        line_height: px(20.),
                        diff_rows: &[],
                    },
                    window,
                    cx,
                );
                let gutter = layout.gutter.as_ref().expect("Full Editor 应布局 gutter");

                assert_eq!(layout.lines[0].origin.x, px(39.));
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
    fn folded_projection_rows_drive_layout_and_hit_testing(cx: &mut TestAppContext) {
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
                        foldable_lines: &BTreeSet::new(),
                        start_row: DisplayRow::ZERO,
                        scroll_offset: point(px(0.), px(0.)),
                        line_height: px(20.),
                        diff_rows: &[],
                    },
                    window,
                    cx,
                );

                assert_eq!(layout.lines.len(), 2);
                assert_eq!(layout.lines[0].shaped.text.as_ref(), "anchor");
                assert_eq!(layout.lines[1].shaped.text.as_ref(), "after");
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
                        foldable_lines: &BTreeSet::new(),
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
                                old_range: 1..2,
                                kind: DiffHunkKind::Modified,
                            },
                            DiffHunk {
                                range: 3..3,
                                old_range: 2..3,
                                kind: DiffHunkKind::Deleted,
                            },
                            DiffHunk {
                                range: 4..5,
                                old_range: 4..4,
                                kind: DiffHunkKind::Added,
                            },
                        ],
                        &[],
                        &[],
                    ),
                    vec![
                        (1..2, DiffHunkKind::Modified),
                        // 折叠的删除块行内不做标记（gutter 红色胶囊提示）。
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
                            old_range: 0..1,
                            kind: DiffHunkKind::Modified,
                        }],
                        &[],
                        &[],
                    ),
                    vec![(0..row_1, DiffHunkKind::Modified)]
                );

                // 越界 hunk（超出文件末尾）用 line_count 收尾。
                assert_eq!(
                    diff_hunk_rows(
                        &snapshot,
                        &[DiffHunk {
                            range: 1..10,
                            old_range: 1..10,
                            kind: DiffHunkKind::Modified,
                        }],
                        &[],
                        &[],
                    ),
                    vec![(row_1..line_count, DiffHunkKind::Modified)]
                );
            })
            .expect("测试窗口应保持可用");
    }

    #[test]
    fn diff_hunk_rows_expanded_deleted_marks_inserted_lines() {
        // 无 wrap：展开的删除块背景覆盖锚定行之后的合成行（被删除行），锚定幸存行不再标记。
        let buffer = Buffer::scratch(
            "line 0\nline 1\nline 2\n".to_owned(),
            BufferConfig::default(),
        )
        .expect("应创建 Buffer");
        let mut map = DisplayMap::new(buffer.snapshot());
        // 删除块展开：锚定行 1 后插入 2 行被删除文本（HEAD 的 1..3 行）。
        map.set_inserted(crate::display_map::InsertedLines::from([(
            Line::new(1),
            vec![std::sync::Arc::from("old 1"), std::sync::Arc::from("old 2")],
        )]));
        let snapshot = map.snapshot();
        let hunk = DiffHunk {
            range: 1..1,
            old_range: 1..3,
            kind: DiffHunkKind::Deleted,
        };
        // 未展开：行内无标记（删除点由 gutter 红色三角提示）。
        assert_eq!(diff_hunk_rows(&snapshot, &[hunk.clone()], &[], &[]), vec![]);
        // 展开：背景覆盖被删除的合成行（锚定行 1 之后，显示行 2..4）。
        assert_eq!(
            diff_hunk_rows(&snapshot, &[hunk.clone()], &[1..3], &[]),
            vec![(2..4, DiffHunkKind::Deleted)]
        );
        // 点击区域：折叠态在锚定行（分界线在锚定行底部），展开态覆盖合成行。
        assert_eq!(
            hunk_hit_regions(&snapshot, &[hunk.clone()], &[], &[]),
            vec![(1..2, 1..3, DiffHunkKind::Deleted)]
        );
        assert_eq!(
            hunk_hit_regions(&snapshot, &[hunk], &[1..3], &[]),
            vec![(2..4, 1..3, DiffHunkKind::Deleted)]
        );
    }

    #[test]
    fn modified_hunk_expansion_marks_old_and_new_rows() {
        // 修改块展开：HEAD 旧行（合成行，锚定修改行上方）按删除色、修改行按新增色。
        let buffer = Buffer::scratch(
            "line 0\nline 1\nline 2\n".to_owned(),
            BufferConfig::default(),
        )
        .expect("应创建 Buffer");
        let mut map = DisplayMap::new(buffer.snapshot());
        map.set_inserted(crate::display_map::InsertedLines::from([(
            Line::ZERO, // 修改块锚定 range.start - 1（旧行在修改行上方）。
            vec![std::sync::Arc::from("old 1")],
        )]));
        let snapshot = map.snapshot();
        let hunk = DiffHunk {
            range: 1..2,
            old_range: 1..2,
            kind: DiffHunkKind::Modified,
        };
        // 未展开：修改行（显示行 2..3）Modified 色。
        assert_eq!(
            diff_hunk_rows(&snapshot, &[hunk.clone()], &[], &[]),
            vec![(2..3, DiffHunkKind::Modified)]
        );
        // 展开：旧行合成行（显示行 1..2）删除色 + 修改行（显示行 2..3）新增色。
        assert_eq!(
            diff_hunk_rows(&snapshot, &[hunk.clone()], &[], &[1..2]),
            vec![(1..2, DiffHunkKind::Deleted), (2..3, DiffHunkKind::Added),]
        );
        // 点击区域：未展开 = 修改行；展开 = 旧行合成行（都在修改行附近）。
        assert_eq!(
            hunk_hit_regions(&snapshot, &[hunk.clone()], &[], &[]),
            vec![(2..3, 1..2, DiffHunkKind::Modified)]
        );
        // 展开的点击区域覆盖整个 hunk（旧行 + 修改行，与竖条一致）。
        assert_eq!(
            hunk_hit_regions(&snapshot, &[hunk], &[], &[1..2]),
            vec![(1..3, 1..2, DiffHunkKind::Modified)]
        );
    }

    #[test]
    fn modified_hunk_strip_stays_yellow_when_expanded() {
        // 竖条色不随展开变化（对齐 Zed）：展开的修改块竖条保持黄色并覆盖旧行 + 修改行。
        let buffer = Buffer::scratch(
            "line 0\nline 1\nline 2\n".to_owned(),
            BufferConfig::default(),
        )
        .expect("应创建 Buffer");
        let mut map = DisplayMap::new(buffer.snapshot());
        map.set_inserted(crate::display_map::InsertedLines::from([(
            Line::ZERO,
            vec![std::sync::Arc::from("old 1")],
        )]));
        let snapshot = map.snapshot();
        let hunk = DiffHunk {
            range: 1..2,
            old_range: 1..2,
            kind: DiffHunkKind::Modified,
        };
        // 未展开：竖条覆盖修改行（显示行 2..3），黄色。
        assert_eq!(
            hunk_strip_rows(&snapshot, &[hunk.clone()], &[], &[]),
            vec![(2..3, DiffHunkKind::Modified)]
        );
        // 展开：竖条仍黄，覆盖旧行 + 修改行（显示行 1..3）。
        assert_eq!(
            hunk_strip_rows(&snapshot, &[hunk], &[], &[1..2]),
            vec![(1..3, DiffHunkKind::Modified)]
        );
    }
}
