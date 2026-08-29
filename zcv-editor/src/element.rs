//! Editor 的逐帧文本布局、绘制与像素命中测试。

use std::collections::BTreeSet;
use std::ops::Range;
use std::sync::Arc;

use gpui::{
    AnyElement, App, AvailableSpace, Bounds, ContentMask, Context, DispatchPhase, Element,
    ElementId, ElementInputHandler, Entity, GlobalElementId, HitboxBehavior, InspectorElementId,
    InteractiveElement, IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PaintQuad, Pixels, Point, ScrollWheelEvent, ShapedLine, Style, TextRun, Window,
    div, fill, point, prelude::*, px, relative, size,
};
use zcv_actions::{OpenExcerpts, ToggleFold};
use zcv_git::DiffHunkKind;
use zcv_language::BracketPair;
use zcv_text::{ByteOffset, Line, LogicalColumn, SearchMatch, TextRange};
use zcv_theme::{color, space, typography};
use zcv_ui::{Button, ButtonSize, ButtonStyle, SvgIcon};

use crate::selection::SelectionSet;

use super::display_map::{
    BufferPoint, DisplayBlock, DisplayBlockKind, DisplayColumn, DisplayPoint, DisplayRow,
    DisplaySnapshot, FILE_HEADER_HEIGHT, FoldRowSegment, ProjectedLineIndex, ProjectedRange,
    RenderedWhitespace, RowStyleInput, StickyBufferHeader, StreamLineSource, WrapRowInfo,
    WrapViewportRowKind, byte_for_display_column, render_viewport_row,
};
use super::gutter::{GutterDimensions, GutterLayout, GutterRow};
use super::scroll::ScrollbarThumbState;
use super::scrollbar::{SCROLLBAR_WIDTH, ScrollbarLayout, marker_column_x_range, marker_geometry};
use super::view::{
    Editor, EditorMode, EditorPresentation, HunkRendering, SoftWrap, diff_kind_for_row,
    hunk_rendering,
};

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
            .on_action(cx.listener(Editor::handle_open_excerpts))
    }
}

#[derive(Clone)]
struct LayoutLine {
    row: DisplayRow,
    logical_line: Option<Line>,
    origin: Point<Pixels>,
    shaped: ShapedLine,
    /// run 背景源（搜索高亮、语法背景）：shaped 文本内的字节区间 + 颜色，与选区一同进入背景片段合成管线。
    background_runs: Vec<(Range<usize>, gpui::Rgba)>,
    whitespaces: Vec<RenderedWhitespace>,
    global_utf16_start: usize,
    wrap_info: Option<WrapRowInfo>,
    /// 折叠合并行的段表（anchor 文本 + 占位符 + 闭合尾段；命中测试与占位符点击用）。
    fold_segments: Option<Vec<FoldRowSegment>>,
    /// 该显示行所属的 git diff 类型（内容背景用；wrap 续行同样标注）。
    git_diff: Option<DiffHunkKind>,
    /// placeholder 提示行：命中测试不映射到 placeholder buffer（空 buffer 唯一合法坐标是 0）。
    is_placeholder: bool,
}

#[derive(Clone)]
struct LayoutBlock {
    row: DisplayRow,
    height: usize,
    origin: Point<Pixels>,
    block: DisplayBlock,
}

struct EditorLayout {
    lines: Vec<LayoutLine>,
    blocks: Vec<LayoutBlock>,
    gutter: Option<GutterLayout>,
    /// 文件标题和 excerpt 分界块覆盖 gutter；正文仍裁剪在 text_clip_bounds 内。
    block_clip_bounds: Bounds<Pixels>,
    text_clip_bounds: Bounds<Pixels>,
    line_height: Pixels,
    display_snapshot: DisplaySnapshot,
}

impl EditorLayout {
    /// 平移所有行的原点（水平自动滚动时复用本帧布局，避免整帧重排）。
    fn translate(&mut self, delta: Point<Pixels>) {
        for line in &mut self.lines {
            line.origin += delta;
        }
        for block in &mut self.blocks {
            block.origin += point(Pixels::ZERO, delta.y);
        }
        if let Some(gutter) = &mut self.gutter {
            for row in &mut gutter.rows {
                row.origin += delta;
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct EditorGeometry {
    pub(super) text_bounds: Bounds<Pixels>,
    pub(super) text_clip_bounds: Bounds<Pixels>,
    pub(super) gutter: Option<(Bounds<Pixels>, GutterDimensions)>,
}

pub(super) struct VisibleLineLayoutParams<'a> {
    pub(super) geometry: EditorGeometry,
    pub(super) active_lines: &'a BTreeSet<Line>,
    /// 可折叠行集合（crease 显示判断；prepaint 从语言层折叠范围计算）。
    pub(super) foldable_lines: &'a BTreeSet<Line>,
    /// 折叠入口行集合（crease 折叠态判断：已折叠 anchor 行显示展开箭头）。
    pub(super) fold_anchor_lines: &'a BTreeSet<Line>,
    pub(super) start_row: DisplayRow,
    pub(super) scroll_offset: Point<Pixels>,
    pub(super) line_height: Pixels,
    /// git diff 显示行区间（prepaint 从 `diff_hunk_rows` 计算，gutter/内容共用）。
    pub(super) diff_rows: &'a [(Range<usize>, DiffHunkKind)],
}

impl EditorLayout {
    /// 像素位置 → (布局行, 显示列)；显示列 = 行文本内字符数（含 wrap 假空格与折叠占位符）。
    fn line_column_at(&self, position: Point<Pixels>) -> Option<(&LayoutLine, usize)> {
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
        let local_chars = line.shaped.text[..byte_index].chars().count();
        // 软换行续行：命中假空格区落在片段起点，其余按"片段起始列 + 段内字符数"换算。
        let column = if let Some(info) = line.wrap_info {
            if local_chars <= info.indent {
                info.column_base
            } else {
                info.column_base + local_chars - info.indent
            }
        } else {
            local_chars
        };
        Some((line, column))
    }

    fn buffer_point_for_position(&self, position: Point<Pixels>) -> Option<BufferPoint> {
        let (line, column) = self.line_column_at(position)?;
        // placeholder 提示行：不映射到 placeholder buffer（空 buffer 唯一合法坐标是 0）。
        if line.is_placeholder {
            return Some(BufferPoint::new(Line::ZERO, LogicalColumn::ZERO));
        }
        // 折叠合并行：显示列 → buffer 字节走权威映射（占位符段吸附折叠终点，尾段映射到 close 行）。
        if line.fold_segments.is_some() {
            let offset = self
                .display_snapshot
                .display_point_to_offset(DisplayPoint::new(line.row, DisplayColumn::new(column)))
                .ok()?;
            return self
                .display_snapshot
                .buffer_snapshot()
                .byte_to_position(offset)
                .ok()
                .map(BufferPoint::from);
        }
        if let Some(info) = line.wrap_info {
            return Some(BufferPoint::new(
                info.line,
                zcv_text::LogicalColumn::new(column),
            ));
        }
        if let Some(logical_line) = line.logical_line {
            return Some(BufferPoint::new(
                logical_line,
                zcv_text::LogicalColumn::new(column),
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
}

/// IME 候选框命中测试：持有一帧布局的 Arc 引用，避免每帧深拷贝整表布局。
#[derive(Clone)]
pub(super) struct EditorInputLayout {
    layout: Arc<EditorLayout>,
}

impl EditorInputLayout {
    fn from_layout(layout: &Arc<EditorLayout>) -> Self {
        Self {
            layout: Arc::clone(layout),
        }
    }

    pub(super) fn utf16_index_for_point(&self, point: Point<Pixels>) -> Option<usize> {
        let lines = &self.layout.lines;
        let line_height = self.layout.line_height;
        let first = lines.first()?;
        let last = lines.last()?;
        let line = if point.y <= first.origin.y {
            first
        } else if point.y >= last.origin.y + line_height {
            last
        } else {
            lines
                .iter()
                .find(|line| point.y < line.origin.y + line_height)
                .unwrap_or(last)
        };
        let byte = line.shaped.closest_index_for_x(point.x - line.origin.x);
        Some(line.global_utf16_start + line.shaped.text[..byte].encode_utf16().count())
    }
}

pub(super) struct PrepaintState {
    layout: Arc<EditorLayout>,
    /// 每行一个背景片段表（选区 + run 背景合成，互不重叠，一次绘制）。
    background_fragments: Vec<Vec<BackgroundFragment>>,
    bracket_matches: Vec<PaintQuad>,
    selected_whitespace: Option<SelectedWhitespaceMarkers>,
    carets: Vec<PaintQuad>,
    ime_caret_bounds: Option<Bounds<Pixels>>,
    hitbox: gpui::Hitbox,
    gutter_hitbox: Option<gpui::Hitbox>,
    /// hunk 色带 hitbox（起点行可见时插入；点击切换折叠/展开；类型 + 展开态标志）。
    deleted_hunk_hitboxes: Arc<Vec<HunkHitbox>>,
    /// 折叠删除 hunk 的交互三角标记（禁止渲染层手绘交互图标）。
    deleted_hunk_buttons: Vec<AnyElement>,
    /// 由宿主提供内容、Editor 定位到 hunk 右上角并按整块悬停显隐的操作栏。
    diff_hunk_controls: Vec<DiffHunkControls>,
    /// crease 折叠开关（已按 gutter 绝对坐标布局；自带点击与 tooltip）。
    crease_toggles: Vec<Option<AnyElement>>,
    /// 折叠占位符点击 hitbox（合并行占位符段；点击展开）。
    placeholder_hitboxes: Arc<Vec<(gpui::Hitbox, Line)>>,
    /// hunk 竖条范围与状态色（竖条色不随展开变化；行背景按行状态另行绘制）。
    hunk_strips: Arc<Vec<(Range<usize>, DiffHunkKind)>>,
    /// 整行差异背景范围（新增行始终包含，修改/删除只在展开时包含）。
    expanded_rows: Arc<Vec<Range<usize>>>,
    scrollbar: Option<ScrollbarLayout>,
    block_elements: Vec<AnyElement>,
    sticky_buffer_header: Option<AnyElement>,
}

/// hunk 色带 hitbox：命中区域 + 点击目标范围 + 类型 + 展开态标志。
type HunkHitbox = (gpui::Hitbox, Range<usize>, DiffHunkKind, bool);

struct DiffHunkControls {
    hover_bounds: Bounds<Pixels>,
    element: Option<AnyElement>,
}

/// 背景片段合成管线：把选区与 run 背景（搜索高亮、语法背景）合成为每行互不重叠的着色片段，一次绘制。
/// 源按边界切分、重叠处按 base → 选区 → run 顺序混合（维持既有视觉层级），选区片段携带角样式（轮廓首末行外角圆角、行阶梯处宽行内凹/窄行外凸），run 片段为直角矩形。
const TOP_LEFT: usize = 0;
const TOP_RIGHT: usize = 1;
const BOTTOM_RIGHT: usize = 2;
const BOTTOM_LEFT: usize = 3;

/// 片段角样式。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CornerStyle {
    /// 直角（内部接缝与 run 片段）。
    Straight,
    /// 外角圆角（选区轮廓首末行）。
    Round,
    /// 内凹切角（阶梯处更宽一行）。
    Concave,
    /// 外凸补角（阶梯处更窄一行）。
    Convex,
}

/// 角样式 + 曲线横向跨度（Round 取轮廓行宽，阶梯角取两行外缘差，均封顶圆角半径）。
#[derive(Clone, Copy, Debug, PartialEq)]
struct Corner {
    style: CornerStyle,
    width: Pixels,
}

const STRAIGHT_CORNER: Corner = Corner {
    style: CornerStyle::Straight,
    width: Pixels::ZERO,
};

const ALL_STRAIGHT: [Corner; 4] = [STRAIGHT_CORNER; 4];

/// 选区在单行的投影片段：角样式携带轮廓上下文（首末行圆角、阶梯宽行内凹/窄行外凸）。
#[derive(Clone, Debug)]
struct SelectionLineSegment {
    start_x: Pixels,
    end_x: Pixels,
    corners: [Corner; 4],
}

/// 合成后的背景片段：一行内互不重叠的着色区间。
#[derive(Debug)]
struct BackgroundFragment {
    start_x: Pixels,
    end_x: Pixels,
    color: gpui::Rgba,
    /// 是否选区源（混合层级与绘制顺序：选区片段在下、run 片段在上）。
    selection: bool,
    corners: [Corner; 4],
}

impl BackgroundFragment {
    /// 全直角片段直接画矩形；带角样式的选区片段按四角规则构建轮廓路径。
    fn paint(&self, y: Pixels, line_height: Pixels, corner_radius: Pixels, window: &mut Window) {
        let top_left = point(self.start_x, y);
        let top_right = point(self.end_x, y);
        let bottom_right = point(self.end_x, y + line_height);
        let bottom_left = point(self.start_x, y + line_height);
        let curve_height = point(Pixels::ZERO, corner_radius);
        let [
            top_left_corner,
            top_right_corner,
            bottom_right_corner,
            bottom_left_corner,
        ] = self.corners;

        if self
            .corners
            .iter()
            .all(|corner| corner.style == CornerStyle::Straight)
        {
            window.paint_quad(fill(
                Bounds::from_corners(top_left, bottom_right),
                self.color,
            ));
            return;
        }

        // 顺时针：左上 → 顶边 → 右上 → 右边 → 右下 → 底边 → 左下 → 左边。
        let mut builder = gpui::PathBuilder::fill();
        builder.move_to(match top_left_corner.style {
            CornerStyle::Round | CornerStyle::Concave => {
                top_left + point(top_left_corner.width, Pixels::ZERO)
            }
            CornerStyle::Convex => top_left - point(top_left_corner.width, Pixels::ZERO),
            CornerStyle::Straight => top_left,
        });
        match top_right_corner.style {
            CornerStyle::Straight => builder.line_to(top_right),
            CornerStyle::Round | CornerStyle::Concave => {
                builder.line_to(top_right - point(top_right_corner.width, Pixels::ZERO));
                builder.curve_to(top_right + curve_height, top_right);
            }
            CornerStyle::Convex => {
                builder.line_to(top_right + point(top_right_corner.width, Pixels::ZERO));
                builder.curve_to(top_right + curve_height, top_right);
            }
        }
        match bottom_right_corner.style {
            CornerStyle::Straight => builder.line_to(bottom_right),
            CornerStyle::Round | CornerStyle::Concave => {
                builder.line_to(bottom_right - curve_height);
                builder.curve_to(
                    bottom_right - point(bottom_right_corner.width, Pixels::ZERO),
                    bottom_right,
                );
            }
            CornerStyle::Convex => {
                builder.line_to(bottom_right - curve_height);
                builder.curve_to(
                    bottom_right + point(bottom_right_corner.width, Pixels::ZERO),
                    bottom_right,
                );
            }
        }
        match bottom_left_corner.style {
            CornerStyle::Straight => builder.line_to(bottom_left),
            CornerStyle::Round | CornerStyle::Concave => {
                builder.line_to(bottom_left + point(bottom_left_corner.width, Pixels::ZERO));
                builder.curve_to(bottom_left - curve_height, bottom_left);
            }
            CornerStyle::Convex => {
                builder.line_to(bottom_left - point(bottom_left_corner.width, Pixels::ZERO));
                builder.curve_to(bottom_left - curve_height, bottom_left);
            }
        }
        match top_left_corner.style {
            CornerStyle::Straight => builder.line_to(top_left),
            CornerStyle::Round | CornerStyle::Concave => {
                builder.line_to(top_left + curve_height);
                builder.curve_to(
                    top_left + point(top_left_corner.width, Pixels::ZERO),
                    top_left,
                );
            }
            CornerStyle::Convex => {
                builder.line_to(top_left + curve_height);
                builder.curve_to(
                    top_left - point(top_left_corner.width, Pixels::ZERO),
                    top_left,
                );
            }
        }
        if let Ok(path) = builder.build() {
            window.paint_path(path, self.color);
        }
    }
}

/// 阶梯角：本行外缘比邻行更远（右缘更大、左缘更小）→ 本行更宽 → 内凹；反之外凸；齐平 → 直角。
fn step_corner(own: Pixels, other: Pixels, right_side: bool, corner_radius: Pixels) -> Corner {
    if own == other {
        return STRAIGHT_CORNER;
    }
    let width = if own > other {
        own - other
    } else {
        other - own
    };
    let wider = if right_side { own > other } else { own < other };
    Corner {
        style: if wider {
            CornerStyle::Concave
        } else {
            CornerStyle::Convex
        },
        width: (width / 2.).min(corner_radius),
    }
}

/// 按轮廓上下文为每行计算四角样式：首行外角圆角、末行下外角圆角，行阶梯处宽行内凹/窄行外凸。
fn finish_selection_contour(
    contour: &[(usize, Pixels, Pixels)],
    corner_radius: Pixels,
    per_line: &mut [Vec<SelectionLineSegment>],
) {
    let count = contour.len();
    if count == 0 {
        return;
    }
    for (index, &(line_ix, start_x, end_x)) in contour.iter().enumerate() {
        let round_width = ((end_x - start_x) / 2.).min(corner_radius);
        let mut corners = ALL_STRAIGHT;
        if index == 0 {
            corners[TOP_LEFT] = Corner {
                style: CornerStyle::Round,
                width: round_width,
            };
            corners[TOP_RIGHT] = Corner {
                style: CornerStyle::Round,
                width: round_width,
            };
        } else {
            let (_, prev_start, prev_end) = contour[index - 1];
            corners[TOP_LEFT] = step_corner(start_x, prev_start, false, corner_radius);
            corners[TOP_RIGHT] = step_corner(end_x, prev_end, true, corner_radius);
        }
        if index == count - 1 {
            corners[BOTTOM_LEFT] = Corner {
                style: CornerStyle::Round,
                width: round_width,
            };
            corners[BOTTOM_RIGHT] = Corner {
                style: CornerStyle::Round,
                width: round_width,
            };
        } else {
            let (_, next_start, next_end) = contour[index + 1];
            corners[BOTTOM_LEFT] = step_corner(start_x, next_start, false, corner_radius);
            corners[BOTTOM_RIGHT] = step_corner(end_x, next_end, true, corner_radius);
        }
        per_line[line_ix].push(SelectionLineSegment {
            start_x,
            end_x,
            corners,
        });
    }
}

struct SelectedWhitespaceMarkers {
    symbol: ShapedLine,
    origins: Vec<Point<Pixels>>,
    line_height: Pixels,
}

impl SelectedWhitespaceMarkers {
    fn paint(&self, window: &mut Window, cx: &mut App) {
        for origin in &self.origins {
            if let Err(error) = self.symbol.paint(*origin, self.line_height, window, cx) {
                eprintln!("Editor 空白标记绘制失败：{error}");
            }
        }
    }
}

/// 构建 crease 折叠开关：Button 组件（chevron 图标 + tooltip + 点击），按 gutter 绝对坐标 as_root 独立布局。
fn build_crease_toggles(
    layout: &EditorLayout,
    editor: &Entity<Editor>,
    window: &mut Window,
    cx: &mut App,
) -> Vec<Option<AnyElement>> {
    let mut toggles = Vec::new();
    let Some(gutter) = &layout.gutter else {
        return toggles;
    };
    let available_space = size(
        AvailableSpace::MinContent,
        AvailableSpace::Definite(layout.line_height * 0.55),
    );
    for row in &gutter.rows {
        let Some(folded) = row.crease else {
            toggles.push(None);
            continue;
        };
        let path = if folded {
            "icons/chevron_right.svg"
        } else {
            "icons/chevron_down.svg"
        };
        let line = row.logical_line;
        let editor = editor.clone();
        let focus = editor.read(cx).focus_handle();
        let mut toggle = Button::icon(("gutter_crease", line.get()), path)
            .label(if folded { "展开" } else { "折叠" })
            .shortcut(&ToggleFold, cx)
            .on_click(move |_event, window, cx| {
                window.focus(&focus);
                editor.update(cx, |editor, cx| editor.toggle_fold_at_line(line, cx));
            })
            .into_any_element();
        let toggle_size = toggle.layout_as_root(available_space, window, cx);
        let crease_left = gutter.bounds.right() - gutter.crease_width;
        let origin = point(
            crease_left + (gutter.crease_width - toggle_size.width) / 2.0,
            row.origin.y + (layout.line_height - toggle_size.height) / 2.0,
        );
        toggle.prepaint_as_root(origin, available_space, window, cx);
        toggles.push(Some(toggle));
    }
    toggles
}

fn build_deleted_hunk_buttons(
    hitboxes: &[HunkHitbox],
    editor: &Entity<Editor>,
    line_height: Pixels,
    window: &mut Window,
    cx: &mut App,
) -> Vec<AnyElement> {
    let mut buttons = Vec::new();
    for (index, (hitbox, old_range, kind, expanded)) in hitboxes.iter().enumerate() {
        if *kind != DiffHunkKind::Deleted || *expanded {
            continue;
        }
        let editor = editor.clone();
        let old_range = old_range.clone();
        let mut button = Button::icon(("deleted-hunk-toggle", index), "icons/triangle_right.svg")
            .color(color::current(cx).status_deleted)
            .label("展开删除内容")
            .on_click(move |_event, _window, cx| {
                editor.update(cx, |editor, cx| {
                    editor.toggle_deleted_hunk(old_range.clone(), cx)
                });
            })
            .into_any_element();
        let available_space = size(AvailableSpace::MinContent, AvailableSpace::MinContent);
        let button_size = button.layout_as_root(available_space, window, cx);
        let origin = point(
            hitbox.bounds.left(),
            hitbox.bounds.bottom() - button_size.height / 2.0 - line_height * 0.05,
        );
        button.prepaint_as_root(origin, available_space, window, cx);
        buttons.push(button);
    }
    buttons
}

fn build_diff_hunk_controls(
    layout: &EditorLayout,
    hunks: &[(Range<usize>, zcv_git::DiffHunk)],
    sticky_header_height: Pixels,
    editor: &Entity<Editor>,
    window: &mut Window,
    cx: &mut App,
) -> Vec<DiffHunkControls> {
    let Some(delegate) = editor.read(cx).diff_hunk_delegate() else {
        return Vec::new();
    };
    let mut controls = Vec::new();
    let sticky_top = layout.text_clip_bounds.top() + sticky_header_height;
    for (rows, hunk) in hunks {
        let Some(visible_line) = layout
            .lines
            .iter()
            .find(|line| rows.contains(&line.row.get()))
        else {
            continue;
        };
        let hunk_start_y = visible_line.origin.y
            - layout.line_height * visible_line.row.get().saturating_sub(rows.start) as f32;
        let hunk_end_y = hunk_start_y + layout.line_height * rows.len() as f32;
        let visible_top = hunk_start_y.max(sticky_top);
        let visible_bottom = hunk_end_y.min(layout.text_clip_bounds.bottom());
        if visible_top >= visible_bottom {
            continue;
        }
        let hover_bounds = Bounds {
            origin: point(layout.text_clip_bounds.left(), visible_top),
            size: size(
                layout.text_clip_bounds.size.width,
                visible_bottom - visible_top,
            ),
        };
        let element = if hover_bounds.contains(&window.mouse_position()) {
            let mut element = delegate.render_hunk_controls(
                rows.start,
                hunk,
                layout.line_height,
                editor,
                window,
                cx,
            );
            let available_space = size(AvailableSpace::MinContent, AvailableSpace::MinContent);
            let element_size = element.layout_as_root(available_space, window, cx);
            let origin_y = if hunk_start_y >= sticky_top {
                hunk_start_y
            } else {
                sticky_top.min(hunk_end_y - layout.line_height)
            };
            let origin = point(
                (layout.text_clip_bounds.right() - element_size.width - px(4.))
                    .max(layout.text_clip_bounds.left()),
                origin_y,
            );
            element.prepaint_as_root(origin, available_space, window, cx);
            Some(element)
        } else {
            None
        };
        controls.push(DiffHunkControls {
            hover_bounds,
            element,
        });
    }
    controls
}

/// 普通文件标题与悬浮文件标题共用的唯一渲染入口。
fn buffer_header_element(
    block: &DisplayBlock,
    row: DisplayRow,
    sticky: bool,
    editor: &Entity<Editor>,
    cx: &mut App,
) -> AnyElement {
    let colors = *color::current(cx);
    let display_path = block.excerpt.display_path();
    let filename = display_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| display_path.display().to_string());
    let parent = display_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| format!(" {}/", parent.display()));
    let open_excerpt = block.excerpt.clone();
    let open_from_path = block.excerpt.clone();
    let fold_path = block.excerpt.path().to_path_buf();
    let folded = editor.read(cx).is_buffer_folded(&fold_path);
    let editor_for_button = editor.clone();
    let editor_for_path = editor.clone();
    let editor_for_fold = editor.clone();
    let block_id: ElementId = if sticky {
        ("sticky-buffer-header-block", row.get()).into()
    } else {
        ("buffer-header-block", row.get()).into()
    };
    let header_id: ElementId = if sticky {
        ("sticky-buffer-header", row.get()).into()
    } else {
        ("buffer-header", row.get()).into()
    };
    let chevron_id: ElementId = if sticky {
        ("sticky-buffer-header-chevron", row.get()).into()
    } else {
        ("buffer-header-chevron", row.get()).into()
    };
    let path_id: ElementId = if sticky {
        ("sticky-buffer-header-path", row.get()).into()
    } else {
        ("buffer-header-path", row.get()).into()
    };
    let file_id: ElementId = if sticky {
        ("sticky-buffer-header-file", row.get()).into()
    } else {
        ("buffer-header-file", row.get()).into()
    };
    let open_id: ElementId = if sticky {
        ("sticky-buffer-header-open", row.get()).into()
    } else {
        ("buffer-header-open", row.get()).into()
    };

    div()
        .id(block_id)
        .w_full()
        .h_full()
        .when(sticky, |element| element.bg(colors.editor_background))
        .when(sticky, |element| {
            element
                .occlude()
                .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                    cx.stop_propagation()
                })
        })
        .p(space::S2)
        .child(
            div()
                .id(header_id)
                .size_full()
                .flex()
                .items_center()
                .justify_between()
                .gap(space::S6)
                .px(space::S6)
                .rounded_sm()
                .border_1()
                .border_color(colors.border)
                .bg(colors.editor_subheader_background)
                .child(
                    Button::icon(
                        chevron_id,
                        if folded {
                            "icons/chevron_right.svg"
                        } else {
                            "icons/chevron_down.svg"
                        },
                    )
                    .label(if folded {
                        "展开文件"
                    } else {
                        "折叠文件"
                    })
                    .on_click(move |_event, _window, cx| {
                        editor_for_fold.update(cx, |editor, cx| {
                            editor.toggle_buffer_fold(fold_path.clone(), cx)
                        });
                    }),
                )
                .child(
                    // 悬停反馈只在路径区（点击区）生效；
                    // 悬浮在“打开文件”等子按钮上时 header 背景保持不变。
                    div()
                        .id(path_id)
                        .min_w_0()
                        .flex_1()
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .cursor_pointer()
                        .hover(move |style| style.bg(colors.element_hover))
                        .on_click(move |_event, _window, cx| {
                            editor_for_path.update(cx, |editor, cx| {
                                editor.open_excerpt(&open_from_path, false, cx)
                            });
                        })
                        .child(SvgIcon::new("icons/file.svg").id(file_id))
                        .child(
                            div()
                                .text_size(typography::editor())
                                .text_color(colors.text)
                                .whitespace_nowrap()
                                .child(filename),
                        )
                        .when_some(parent, |element, parent| {
                            element.child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(typography::editor())
                                    .text_color(colors.text_muted)
                                    .child(parent),
                            )
                        }),
                )
                .child(
                    Button::text(open_id, "打开文件")
                        .style(ButtonStyle::Solid)
                        .size(ButtonSize::Loose)
                        .label("打开文件并跳转到指定位置")
                        .shortcut(&OpenExcerpts, cx)
                        .on_click(move |_event, _window, cx| {
                            editor_for_button.update(cx, |editor, cx| {
                                editor.open_excerpt(&open_excerpt, false, cx)
                            });
                        }),
                ),
        )
        .into_any_element()
}

/// 把 BlockMap 的虚拟块布局成真实 GPUI 元素。
/// 文件标题属于 Editor 基础设施；
/// 搜索、diff 等宿主只通过 MultiBuffer 元数据参与。
fn build_block_elements(
    layout: &EditorLayout,
    sticky_source_row: Option<DisplayRow>,
    editor: &Entity<Editor>,
    window: &mut Window,
    cx: &mut App,
) -> Vec<AnyElement> {
    let colors = *color::current(cx);
    let available_width = layout.block_clip_bounds.size.width;
    let mut elements = Vec::with_capacity(layout.blocks.len());

    for block in &layout.blocks {
        if block.block.kind == DisplayBlockKind::BufferHeader
            && sticky_source_row == Some(block.row)
        {
            continue;
        }
        let height = layout.line_height * block.height as f32;
        let mut element = match block.block.kind {
            DisplayBlockKind::ExcerptBoundary => div()
                .id(("excerpt-boundary", block.row.get()))
                .w_full()
                .h_full()
                .flex()
                .items_center()
                .px(space::S6)
                .child(div().w_full().h(px(1.)).bg(colors.border_variant))
                .into_any_element(),
            DisplayBlockKind::BufferHeader => {
                buffer_header_element(&block.block, block.row, false, editor, cx)
            }
        };

        let available_space = size(
            AvailableSpace::Definite(available_width),
            AvailableSpace::Definite(height),
        );
        element.layout_as_root(available_space, window, cx);
        element.prepaint_as_root(block.origin, available_space, window, cx);
        elements.push(element);
    }
    elements
}

/// 把当前文件标题固定在视口顶部；
/// 下一个文件标题接近时把它逐行顶出。
fn build_sticky_buffer_header(
    layout: &EditorLayout,
    sticky: StickyBufferHeader,
    start_row: DisplayRow,
    scroll_offset: Point<Pixels>,
    editor: &Entity<Editor>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let width = layout.block_clip_bounds.size.width;
    let height = layout.line_height * FILE_HEADER_HEIGHT as f32;
    let origin = point(
        layout.block_clip_bounds.left(),
        sticky_buffer_header_origin_y(
            layout.block_clip_bounds.top(),
            layout.line_height,
            start_row,
            scroll_offset.y,
            sticky.next_buffer_header_row,
        ),
    );
    let block = DisplayBlock {
        kind: DisplayBlockKind::BufferHeader,
        excerpt: sticky.excerpt,
    };
    let mut element = buffer_header_element(&block, sticky.source_row, true, editor, cx);
    let available_space = size(
        AvailableSpace::Definite(width),
        AvailableSpace::Definite(height),
    );
    element.layout_as_root(available_space, window, cx);
    element.prepaint_as_root(origin, available_space, window, cx);
    element
}

fn sticky_buffer_header_origin_y(
    viewport_top: Pixels,
    line_height: Pixels,
    start_row: DisplayRow,
    scroll_offset_y: Pixels,
    next_buffer_header_row: Option<DisplayRow>,
) -> Pixels {
    let height = line_height * FILE_HEADER_HEIGHT as f32;
    let Some(next_row) = next_buffer_header_row else {
        return viewport_top;
    };
    let next_y = viewport_top + line_height * (next_row.get() as f32 - start_row.get() as f32)
        - scroll_offset_y;
    if next_y < viewport_top + height {
        next_y - height
    } else {
        viewport_top
    }
}

/// 滚动轴布局：thumb 几何 + diff marker（折叠的删除块行内无标记，滚动条 marker 仍指示删除位置）。
fn layout_scrollbar(
    mode: &EditorMode,
    scrollbar_bounds: Bounds<Pixels>,
    hunk_render: &HunkRendering,
    line_height: Pixels,
    editor: &Entity<Editor>,
    cx: &App,
    window: &mut Window,
) -> Option<ScrollbarLayout> {
    (*mode == EditorMode::Full).then(|| {
        let editor = editor.read(cx);
        let mut scrollbar_layout = ScrollbarLayout::new(
            scrollbar_bounds,
            editor.max_scroll_top(),
            editor.scroll_top(),
            editor.scrollbar_thumb_state(),
            window,
        );
        // marker 每帧计算（hunks 数量级小；滚动中实时跟随，无需缓存/后台任务）。
        // scroll_per_pixel 取 layout 自身算好的值，与 thumb 换算严格一致。
        let folded_deleted_markers: Vec<(Range<usize>, DiffHunkKind)> = hunk_render
            .hit_regions
            .iter()
            .filter(|(_, old_range, kind)| {
                *kind == DiffHunkKind::Deleted
                    && !editor.expanded_deleted_hunks().contains(old_range)
            })
            .map(|(rows, _, _)| (rows.clone(), DiffHunkKind::Deleted))
            .collect();
        scrollbar_layout.markers = marker_geometry(
            hunk_render
                .diff_rows
                .iter()
                .cloned()
                .chain(folded_deleted_markers),
            scrollbar_layout.hitbox.bounds,
            scrollbar_layout.scroll_per_pixel,
            line_height,
        );
        scrollbar_layout
    })
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
        // 用上一帧的 snapshot 计算文本区域宽度；
        // wrap 生效后行数变化会让 gutter 位数在下一帧自动修正，不影响正确性。
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
                editor.mode().clone(),
                editor.preferred_line_length(),
            )
        };
        // 匹配括号按 (光标, buffer 版本, 语法版本) 缓存，滚动/纯重绘帧不重跑 tree-sitter 查询。
        // 在 read 块之后执行：缓存写入需要可变借用，read 块内的引用类型已在块内克隆。
        let matching_bracket_pair = self
            .editor
            .update(cx, |editor, _| editor.matching_bracket_pair());
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
        let font_id = window.text_system().resolve_font(&font);
        let em_advance = window
            .text_system()
            .em_advance(font_id, font_size)
            .expect("编辑器字体必须包含拉丁字形");
        let wrap_width = calculate_wrap_width(
            soft_wrap,
            text_bounds.size.width,
            preferred_line_length,
            em_advance,
        );
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
        let sticky_header_height = display_snapshot
            .sticky_buffer_header(DisplayRow::ZERO)
            .map_or(Pixels::ZERO, |_| line_height * FILE_HEADER_HEIGHT as f32);
        self.editor.update(cx, |editor, _| {
            editor.prepare_scroll_viewport(
                text_bounds.size,
                content_width,
                line_height,
                sticky_header_height,
            );
            // 垂直自动滚动在布局前应用：光标行进出视口的锚点修正只依赖行与视口几何，提前消费后首遍布局即为最终布局，光标移动帧不再整帧重排。
            editor.apply_pending_autoscroll_vertical();
        });
        let (start_row, scroll_offset) = {
            let editor = self.editor.read(cx);
            (editor.scroll_anchor().row(), editor.scroll_offset())
        };
        // 折叠入口行集合（crease 折叠态判断：anchor 行已折叠常显展开箭头）。
        let fold_anchor_lines: BTreeSet<Line> =
            display_snapshot.fold_anchor_lines().into_iter().collect();
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
        // git diff 渲染数据：行标记、竖条与点击区域单遍计算共用（只依赖 snapshot 与注入 hunks，与滚动位置无关，autoscroll 重排可复用）。
        let hunk_render = {
            let editor = self.editor.read(cx);
            hunk_rendering(
                &display_snapshot,
                editor.diff_hunks(cx),
                editor.expanded_deleted_hunks(),
                editor.expanded_modified_hunks(),
                editor.materialized_diff_hunks(cx),
            )
        };
        let diff_rows = &hunk_render.diff_rows;
        // placeholder 模式：空 buffer 时行数据源替换为 placeholder 快照（折行/行高一致）。
        let placeholder = self.editor.read(cx).placeholder_snapshot_if_empty(cx);
        let mut layout = layout_visible_lines(
            display_snapshot.clone(),
            placeholder.clone(),
            presentation.clone(),
            self.editor.read(cx).search_highlights(),
            VisibleLineLayoutParams {
                geometry,
                active_lines: &active_lines,
                foldable_lines: &foldable_lines,
                fold_anchor_lines: &fold_anchor_lines,
                start_row,
                scroll_offset,
                line_height,
                diff_rows,
            },
            window,
            cx,
        );
        let mut ime_caret_bounds = layout_primary_caret(&selections, &layout, line_height);
        // 水平自动滚动：光标 x 进出视口时只平移本帧布局（水平滚动是均匀平移），不再整帧重排。
        let scrolled_horizontal = self.editor.update(cx, |editor, _| {
            let scroll_offset = editor.scroll_offset();
            editor.complete_autoscroll_horizontal(
                ime_caret_bounds.map(|caret| caret.left() - text_bounds.left() + scroll_offset.x),
                ime_caret_bounds.map(|caret| caret.right() - text_bounds.left() + scroll_offset.x),
            )
        });
        if scrolled_horizontal {
            let new_scroll_offset = self.editor.read(cx).scroll_offset();
            // 内容原点 = text_left - scroll_offset.x：滚动增大时内容向左平移。
            let delta = point(scroll_offset.x - new_scroll_offset.x, Pixels::ZERO);
            layout.translate(delta);
            ime_caret_bounds =
                ime_caret_bounds.map(|bounds| Bounds::new(bounds.origin + delta, bounds.size));
        }
        let sticky_buffer_header = display_snapshot.sticky_buffer_header(start_row);
        let sticky_source_row = sticky_buffer_header
            .as_ref()
            .map(|header| header.source_row);
        let block_elements =
            build_block_elements(&layout, sticky_source_row, &self.editor, window, cx);
        let sticky_buffer_header = if let Some(sticky) = sticky_buffer_header {
            let element = build_sticky_buffer_header(
                &layout,
                sticky,
                start_row,
                scroll_offset,
                &self.editor,
                window,
                cx,
            );
            Some(element)
        } else {
            None
        };
        let layout = Arc::new(layout);
        let selected_whitespace =
            layout_selected_whitespace(&selections, &layout, line_height, window, cx);
        // 背景片段合成：选区与 run 背景逐行合成为互不重叠的片段。
        let (selection_segments, carets) = layout_selections(&selections, &layout, line_height, cx);
        let background_fragments = layout_background_fragments(&layout, &selection_segments, cx);
        let mut bracket_matches = Vec::new();
        if let Some(pair) = matching_bracket_pair {
            layout_bracket_pair(pair, &layout, line_height, &mut bracket_matches, cx);
        }
        let gutter_hitbox = layout
            .gutter
            .as_ref()
            .map(|gutter| window.insert_hitbox(gutter.bounds, HitboxBehavior::Normal));
        // hunk 竖条范围与状态色（竖条不随展开变化；行背景按行状态另行绘制）。
        let hunk_strips = Arc::new(hunk_render.strips.clone());
        // hunk 色带 hitbox：点击切换折叠/展开（对齐 Zed：hitbox 挂在色带区域，BlockMouse 不穿透）。
        let deleted_hunk_hitboxes = Arc::new({
            let editor = self.editor.read(cx);
            let mut hitboxes = Vec::new();
            if let Some(gutter) = &layout.gutter {
                let strip_width = gutter_strip_width(line_height);
                for (rows, old_range, kind) in &hunk_render.hit_regions {
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
                            editor.expanded_deleted_hunks().contains(old_range)
                        }
                        DiffHunkKind::Modified => {
                            editor.expanded_modified_hunks().contains(old_range)
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
                        old_range.clone(),
                        *kind,
                        expanded,
                    ));
                }
            }
            hitboxes
        });
        let deleted_hunk_buttons = build_deleted_hunk_buttons(
            &deleted_hunk_hitboxes,
            &self.editor,
            line_height,
            window,
            cx,
        );
        // 折叠占位符点击 hitbox：合并行占位符段区域（点击展开；交互型直接调 Entity 方法）。
        let placeholder_hitboxes = {
            let mut hitboxes = Vec::new();
            for line in &layout.lines {
                let Some(segments) = line.fold_segments.as_ref() else {
                    continue;
                };
                let indent = line.wrap_info.map_or(0, |info| info.indent);
                // 占位符段 = 合并文本中 anchor 段之后；显示文本 = 假空格 + 合并文本。
                let start = indent + segments[0].merged_range().end;
                let end = start + segments[1].merged_range().len();
                if start >= line.shaped.text.len() {
                    continue;
                }
                hitboxes.push((
                    window.insert_hitbox(
                        Bounds::from_corners(
                            point(
                                line.origin.x + line.shaped.x_for_index(start),
                                line.origin.y,
                            ),
                            point(
                                line.origin.x
                                    + line.shaped.x_for_index(end.min(line.shaped.text.len())),
                                line.origin.y + line_height,
                            ),
                        ),
                        HitboxBehavior::BlockMouse,
                    ),
                    line.logical_line.expect("折叠合并行必须携带逻辑行"),
                ));
            }
            Arc::new(hitboxes)
        };
        let crease_toggles = build_crease_toggles(&layout, &self.editor, window, cx);
        let diff_hunk_controls = build_diff_hunk_controls(
            &layout,
            &hunk_render.controls,
            sticky_header_height,
            &self.editor,
            window,
            cx,
        );
        let scrollbar = layout_scrollbar(
            &mode,
            scrollbar_bounds,
            &hunk_render,
            line_height,
            &self.editor,
            cx,
            window,
        );

        // 整行差异背景范围（内容区与 gutter 共用；未展开的修改/删除 hunk 只由竖条/三角提示）。
        let expanded_rows = Arc::new(hunk_render.expanded_rows.clone());
        PrepaintState {
            layout,
            background_fragments,
            bracket_matches,
            selected_whitespace,
            carets,
            ime_caret_bounds,
            hitbox: window.insert_hitbox(bounds, HitboxBehavior::Normal),
            gutter_hitbox,
            deleted_hunk_hitboxes,
            deleted_hunk_buttons,
            diff_hunk_controls,
            crease_toggles,
            placeholder_hitboxes,
            hunk_strips,
            expanded_rows,
            scrollbar,
            block_elements,
            sticky_buffer_header,
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
        let placeholder_hitboxes = prepaint.placeholder_hitboxes.clone();
        let mouse_focus = focus.clone();
        let hunk_hover_bounds = Arc::new(
            prepaint
                .diff_hunk_controls
                .iter()
                .map(|controls| controls.hover_bounds)
                .collect::<Vec<_>>(),
        );
        let hovered_hunk = hunk_hover_bounds
            .iter()
            .position(|bounds| bounds.contains(&window.mouse_position()));
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, _cx| {
            if phase == DispatchPhase::Capture
                && hunk_hover_bounds
                    .iter()
                    .position(|bounds| bounds.contains(&event.position))
                    != hovered_hunk
            {
                window.refresh();
            }
        });
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
            // 折叠占位符点击：展开该行（交互型，直接调 Entity 方法；先于 gutter 行号选行）。
            // crease 箭头点击由 Button 组件自带的 on_click 处理。
            if let Some((_, line)) = placeholder_hitboxes
                .iter()
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
                    .position_to_byte(zcv_text::Position::new(point.line(), point.column()))
                {
                    editor.begin_selection(offset, event.click_count, event.modifiers.shift, cx);
                }
            });
            window.focus(&mouse_focus);
            cx.stop_propagation();
        });

        // 拖拽扩展选区：按住左键移动时按点击粒度更新选区；
        // 鼠标拖出文本视口边缘时视口自动滚动、选区随之扩展；
        // 无按键移动时兜底结束拖拽（覆盖"窗口外释放后移回"等漏网场景，对齐滚动条复位策略）。
        let drag_editor = self.editor.clone();
        let drag_layout = Arc::clone(&prepaint.layout);
        let drag_text_bounds = prepaint.layout.text_clip_bounds;
        let drag_line_height = prepaint.layout.line_height;
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            if !event.dragging() {
                drag_editor.update(cx, |editor, _| editor.end_selection());
                return;
            }
            let scroll_delta =
                drag_autoscroll_delta(event.position, drag_text_bounds, drag_line_height);
            let Some(buffer_point) = drag_layout.buffer_point_for_position(event.position) else {
                return;
            };
            drag_editor.update(cx, |editor, cx| {
                // 拖拽事件是窗口级的（`dragging` 为任意面板的按下状态）：
                // 只有编辑器自身正在拖拽选区时才滚动/扩展选区，避免终端等面板拖拽时编辑器联动滚动。
                if !editor.has_pending_selection() {
                    return;
                }
                if scroll_delta != point(Pixels::ZERO, Pixels::ZERO) {
                    editor.scroll_by(scroll_delta, cx);
                }
                if let Ok(offset) =
                    editor
                        .render_snapshot()
                        .position_to_byte(zcv_text::Position::new(
                            buffer_point.line(),
                            buffer_point.column(),
                        ))
                {
                    editor.update_selection(offset, cx);
                }
            });
        });

        let up_editor = self.editor.clone();
        window.on_mouse_event(move |_: &MouseUpEvent, phase, _window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            up_editor.update(cx, |editor, _| editor.end_selection());
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
            // 折叠占位符可点击：hover 时手型光标（crease 箭头由 Button 自带 cursor_pointer）。
            for (hitbox, _) in prepaint.placeholder_hitboxes.iter() {
                if hitbox.is_hovered(window) {
                    window.set_cursor_style(gpui::CursorStyle::PointingHand, hitbox);
                }
            }
        }
        if let Some(gutter) = &prepaint.layout.gutter {
            let colors = color::current(cx);
            // diff 行 gutter 背景：只画展开态行（展开的修改块旧行红、修改行绿），未展开的 hunk 不整行着色，只保留左侧竖条提示。
            let strip_width = gutter_strip_width(gutter.line_height);
            for line in &prepaint.layout.lines {
                let Some(kind) = line.git_diff else {
                    continue;
                };
                if !prepaint
                    .expanded_rows
                    .iter()
                    .any(|range| range.contains(&line.row.get()))
                {
                    continue;
                }
                // 展开态没有 Modified 类型行（展开后旧行标 Deleted、修改行标 Added）。
                let background = match kind {
                    DiffHunkKind::Added => colors.editor_diff_added_background,
                    DiffHunkKind::Deleted => colors.editor_diff_deleted_background,
                    DiffHunkKind::Modified => continue,
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
            // crease 折叠开关：Button 组件（chevron 图标 + tooltip + 点击），prepaint 已按 gutter 绝对坐标布局，这里在 gutter 区域内绘制。
            window.paint_layer(gutter.bounds, |window| {
                for button in &mut prepaint.deleted_hunk_buttons {
                    button.paint(window, cx);
                }
                for toggle in prepaint.crease_toggles.iter_mut().flatten() {
                    toggle.paint(window, cx);
                }
            });
            for row in &gutter.rows {
                if let Err(error) =
                    row.shaped_line_number
                        .paint(row.origin, gutter.line_height, window, cx)
                {
                    // 单个字形绘制失败只跳过该行，不能让整个窗口崩溃。
                    eprintln!("Editor gutter 行号绘制失败：{error}");
                    continue;
                }
            }
        }
        let show_cursor = self.editor.read(cx).show_cursor(window, cx);
        // 文件 header 与 excerpt 分隔块使用独立的整编辑器裁剪区，并在 gutter 之后绘制以覆盖行号区域。
        window.with_content_mask(
            Some(ContentMask {
                bounds: prepaint.layout.block_clip_bounds,
            }),
            |window| {
                for block in &mut prepaint.block_elements {
                    block.paint(window, cx);
                }
            },
        );
        window.with_content_mask(
            Some(ContentMask {
                bounds: prepaint.layout.text_clip_bounds,
            }),
            |window| {
                // git diff 整行背景只画展开态行（未展开的 hunk 只由 gutter 竖条提示）。
                let diff_colors = color::current(cx);
                for line in &prepaint.layout.lines {
                    let Some(kind) = line.git_diff else {
                        continue;
                    };
                    if !prepaint
                        .expanded_rows
                        .iter()
                        .any(|range| range.contains(&line.row.get()))
                    {
                        continue;
                    }
                    // 展开态没有 Modified 类型行（展开后旧行标 Deleted、修改行标 Added）。
                    let background = match kind {
                        DiffHunkKind::Added => diff_colors.editor_diff_added_background,
                        DiffHunkKind::Deleted => diff_colors.editor_diff_deleted_background,
                        DiffHunkKind::Modified => continue,
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
                // 背景片段合成管线：选区与 run 背景逐行合成为互不重叠的片段，一次绘制。
                // 选区片段在前、run 片段在后（与既有层级一致：run 背景覆盖选区之上、文本之下）。
                let line_height = prepaint.layout.line_height;
                let corner_radius = line_height * 0.15;
                for (ix, line) in prepaint.layout.lines.iter().enumerate() {
                    for fragment in prepaint.background_fragments[ix]
                        .iter()
                        .filter(|fragment| fragment.selection)
                    {
                        fragment.paint(line.origin.y, line_height, corner_radius, window);
                    }
                }
                for bracket_match in prepaint.bracket_matches.drain(..) {
                    window.paint_quad(bracket_match);
                }
                for (ix, line) in prepaint.layout.lines.iter().enumerate() {
                    for fragment in prepaint.background_fragments[ix]
                        .iter()
                        .filter(|fragment| !fragment.selection)
                    {
                        fragment.paint(line.origin.y, line_height, corner_radius, window);
                    }
                }
                for line in &prepaint.layout.lines {
                    if let Err(error) =
                        line.shaped
                            .paint(line.origin, prepaint.layout.line_height, window, cx)
                    {
                        // 单个字形绘制失败只跳过该行，不能让整个窗口崩溃。
                        eprintln!("Editor 文本行绘制失败：{error}");
                        continue;
                    }
                }
                if let Some(markers) = &prepaint.selected_whitespace {
                    markers.paint(window, cx);
                }
                if show_cursor {
                    for caret in prepaint.carets.drain(..) {
                        window.paint_quad(caret);
                    }
                }
                for controls in &mut prepaint.diff_hunk_controls {
                    if let Some(element) = &mut controls.element {
                        element.paint(window, cx);
                    }
                }
            },
        );
        // 悬浮文件标题最后覆盖正文和 gutter；
        // 完整背景遮住其下文本，避免透明区域泄漏。
        window.with_content_mask(
            Some(ContentMask {
                bounds: prepaint.layout.block_clip_bounds,
            }),
            |window| {
                if let Some(header) = &mut prepaint.sticky_buffer_header {
                    header.paint(window, cx);
                }
            },
        );
        if let Some(scrollbar) = &prepaint.scrollbar {
            let colors = color::current(cx);
            // 轨道背景位于 marker 与 thumb 下方；默认主题使用透明色。
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
        let input_layout = EditorInputLayout::from_layout(&prepaint.layout);
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
    /// 按下/松开按上一帧状态条件注册：未拖动时注册 MouseDown，拖动中注册 MouseUp，松开后的兜底由无按键 MouseMove 复位。
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
    if row.block().is_some() {
        return Pixels::ZERO;
    }
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

fn calculate_wrap_width(
    soft_wrap: SoftWrap,
    text_width: Pixels,
    preferred_line_length: usize,
    em_advance: Pixels,
) -> Option<Pixels> {
    // 折行点只需为行尾光标让出实际宽度；按字体 em 预留会浪费可见空间，
    // 使本可容纳的中文和标点过早进入下一行。
    let available_width = (text_width - CARET_WIDTH).max(Pixels::ZERO);
    match soft_wrap {
        SoftWrap::None => None,
        SoftWrap::EditorWidth => Some(available_width),
        SoftWrap::Bounded => Some(available_width.min(em_advance * preferred_line_length as f32)),
    }
}

fn layout_visible_lines(
    display_snapshot: DisplaySnapshot,
    placeholder: Option<DisplaySnapshot>,
    presentation: EditorPresentation,
    search_highlights: Option<(&[SearchMatch], usize)>,
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
        fold_anchor_lines,
        start_row,
        scroll_offset,
        line_height,
        diff_rows,
    } = params;
    // placeholder 模式：行数据源替换为 placeholder 快照（折行/行高与真实文本同一管线）；
    // 无高亮/折叠的查询对 placeholder 快照自然返回空。
    let placeholder_mode = placeholder.is_some();
    let display_snapshot = placeholder.as_ref().unwrap_or(&display_snapshot);
    let line_count = display_snapshot.line_count();
    let start = start_row.get().min(line_count.saturating_sub(1));
    let visible_count =
        ((text_bounds.size.height + scroll_offset.y) / line_height).ceil() as usize + 1;
    let end = (start + visible_count).min(line_count);
    let text_style = window.text_style();
    let font_size = text_style.font_size.to_pixels(window.rem_size());
    let mut lines = Vec::with_capacity(end.saturating_sub(start));
    let mut blocks = Vec::new();
    let mut gutter_rows = Vec::with_capacity(end.saturating_sub(start));
    let block_left = gutter_geometry
        .as_ref()
        .map_or(text_clip_bounds.left(), |(bounds, _)| bounds.left());
    let block_clip_bounds = Bounds::new(
        point(block_left, text_clip_bounds.top()),
        size(
            (text_clip_bounds.right() - block_left).max(Pixels::ZERO),
            text_clip_bounds.size.height,
        ),
    );
    let viewport = display_snapshot
        .slice_viewport(DisplayRow::new(start), end.saturating_sub(start))
        .ok();
    // 语法高亮只在基础 buffer chunk 的真实行范围内查询；
    // inlay/fold/tab/wrap都是其后的 chunk 变换，不能用显示片段长度反推 buffer 字节坐标。
    let visible_highlights = viewport
        .as_ref()
        .map(|viewport| display_snapshot.highlighted_spans_for_viewport(viewport))
        .unwrap_or_default();
    // capture 索引 → 样式的预展开表：渲染每 run 一次数组索引，不再逐 run 做字符串回退查找。
    let highlight_styles = display_snapshot.highlight_styles();
    // 搜索高亮：独立背景覆盖层。
    let search_backgrounds: Vec<(Range<usize>, gpui::Rgba)> = match search_highlights {
        Some((matches, active_index)) => {
            let colors = color::current(cx);
            matches
                .iter()
                .enumerate()
                .map(|(index, search_match)| {
                    let range = search_match.range();
                    (
                        range.start().get()..range.end().get(),
                        if index == active_index {
                            colors.search_active_match_background
                        } else {
                            colors.search_match_background
                        },
                    )
                })
                .collect()
        }
        None => Vec::new(),
    };

    // 基础 run：样式段在其上合并（对齐 Zed from_chunks 的 base 合并）。
    let base = TextRun {
        len: 0,
        font: text_style.font(),
        // placeholder 行用提示色。
        color: if placeholder_mode {
            color::current(cx).text_placeholder.into()
        } else {
            text_style.color
        },
        background_color: None,
        underline: None,
        strikethrough: None,
    };

    // 水平视口窗口化：未换行时只塑形可见列附近（±边距）的文本；换行行已受行宽约束。
    // 列 ↔ 字节按等宽换算，行首含 tab 的行在管线侧退回整行上限。
    let font_id = window.text_system().resolve_font(&text_style.font());
    let em_advance = window
        .text_system()
        .em_advance(font_id, font_size)
        .expect("编辑器字体必须包含拉丁字形");
    let horizontal_window = if !display_snapshot.is_wrapped() {
        let scroll_cols = (scroll_offset.x / em_advance).floor() as usize;
        let visible_cols = (text_clip_bounds.size.width / em_advance).ceil() as usize;
        let margin = 64usize;
        Some((
            scroll_cols.saturating_sub(margin),
            scroll_cols + visible_cols + margin,
        ))
    } else {
        None
    };
    let mut push_line = |row: usize,
                         logical_line: Option<Line>,
                         gutter_line: Option<Line>,
                         gutter_number: Option<usize>,
                         text: &str,
                         utf16_start: usize,
                         wrap_info: Option<WrapRowInfo>,
                         fold_segments: Option<Vec<FoldRowSegment>>,
                         whitespaces: Vec<RenderedWhitespace>,
                         window_start_column: usize,
                         runs: Vec<TextRun>| {
        let shaped =
            window
                .text_system()
                .shape_line(text.to_owned().into(), font_size, &runs, None);
        // 收集 run 背景源（字节区间累计自 runs 的覆盖）；无背景的 run 跳过。
        let mut background_runs = Vec::new();
        let mut byte_offset = 0usize;
        for run in &runs {
            if let Some(background) = run.background_color {
                background_runs.push((byte_offset..byte_offset + run.len, background.into()));
            }
            byte_offset += run.len;
        }
        let git_diff = diff_kind_for_row(diff_rows, row);
        lines.push(LayoutLine {
            row: DisplayRow::new(row),
            logical_line,
            origin: point(
                // 窗口化行：shaped 文本从窗口起点开始，行原点随窗口起点列右移。
                text_bounds.left() - scroll_offset.x + em_advance * window_start_column as f32,
                text_bounds.top() + line_height * (row - start) - scroll_offset.y,
            ),
            shaped,
            background_runs,
            whitespaces,
            global_utf16_start: utf16_start,
            wrap_info,
            fold_segments,
            git_diff,
            is_placeholder: placeholder_mode,
        });
        if let (Some(logical_line), Some((gutter_bounds, dimensions))) =
            (gutter_line, gutter_geometry)
        {
            let number = gutter_number
                .unwrap_or_else(|| logical_line.get() + 1)
                .to_string();
            let active = active_lines.contains(&logical_line);
            let colors = color::current(cx);
            // 行号按 diff 状态着色。
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
            // 折叠指示：折叠入口行已折叠常显，可折叠行常显（不依赖光标位置）。
            let crease = if fold_anchor_lines.contains(&logical_line) {
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

    if let Some(viewport) = viewport {
        for row in viewport.rows() {
            if let Some(block) = row.block() {
                blocks.push(LayoutBlock {
                    row: row.index(),
                    height: row.height(),
                    origin: point(
                        block_clip_bounds.left(),
                        // 即使视口从多行块的中间开始，多行块也会返回其真实的第一显示行。
                        // 此时，其原点有意位于裁剪边界之上，因此该增量可能为负值（例如，对于从第 1 行开始的视口，则为第 0 行）。
                        text_bounds.top() + line_height * (row.index().get() as f32 - start as f32)
                            - scroll_offset.y,
                    ),
                    block: block.clone(),
                });
                continue;
            }
            match row.kind() {
                WrapViewportRowKind::Text { .. } => {
                    // 光标行不窗口化：光标像素定位基于 shaped 文本，窗口外光标会被夹到窗口内，导致水平 autoscroll 失效；
                    // 光标行退回整行上限（超长行仍有 1024 兜底）。
                    let window_for_row = match row.kind() {
                        WrapViewportRowKind::Text {
                            source: StreamLineSource::Buffer(line),
                            ..
                        } if active_lines.contains(&Line::new(*line)) => None,
                        _ => horizontal_window,
                    };
                    // 行解构、四层快照链穿透、chunk 合成与 run 映射都在管线侧完成，这里只消费渲染结果。
                    let rendered = render_viewport_row(
                        row.kind(),
                        display_snapshot,
                        &RowStyleInput {
                            visible_highlights: &visible_highlights,
                            highlight_styles,
                            search_backgrounds: &search_backgrounds,
                            marked_ranges: presentation.marked_ranges(),
                        },
                        base.clone(),
                        window_for_row,
                        cx,
                    );
                    let (gutter_line, gutter_number) = match rendered.gutter_line {
                        Some(line) => match display_snapshot.excerpt_for_output_line(line.get()) {
                            Some(excerpt) => {
                                match excerpt.source_line_for_output_line(line.get()) {
                                    Some(number) => (Some(line), Some(number)),
                                    None => (None, None),
                                }
                            }
                            None => (Some(line), Some(line.get() + 1)),
                        },
                        None => (None, None),
                    };
                    push_line(
                        row.index().get(),
                        rendered.logical_line,
                        gutter_line,
                        gutter_number,
                        &rendered.display_text,
                        rendered.utf16_start,
                        rendered.wrap_info,
                        rendered.fold_segments,
                        rendered.whitespaces,
                        rendered.window_start_column,
                        rendered.runs,
                    );
                }
            }
        }
    }

    EditorLayout {
        lines,
        blocks,
        gutter: gutter_geometry.map(|(bounds, dimensions)| GutterLayout {
            bounds,
            line_height,
            rows: gutter_rows,
            crease_width: dimensions.crease_width,
        }),
        block_clip_bounds,
        text_clip_bounds,
        line_height,
        // 命中测试用真实 buffer 的快照（placeholder 行的映射已单独拦截）。
        display_snapshot: display_snapshot.clone(),
    }
}

pub(crate) fn gutter_dimensions(
    display_snapshot: &DisplaySnapshot,
    window: &mut Window,
) -> GutterDimensions {
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

/// 把选区布局为每行的投影片段（供背景片段合成管线使用）与光标 quad。
fn layout_selections(
    selections: &SelectionSet,
    layout: &EditorLayout,
    line_height: Pixels,
    cx: &App,
) -> (Vec<Vec<SelectionLineSegment>>, Vec<PaintQuad>) {
    let mut per_line_segments = vec![Vec::new(); layout.lines.len()];
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
                layout_selection_segments(range, layout, line_height, &mut per_line_segments);
            }
            if let Some(caret) = caret {
                caret_quads.push(caret);
            }
            continue;
        }
    }

    (per_line_segments, caret_quads)
}

fn layout_selected_whitespace(
    selections: &SelectionSet,
    layout: &EditorLayout,
    line_height: Pixels,
    window: &mut Window,
    cx: &App,
) -> Option<SelectedWhitespaceMarkers> {
    let mut ranges = Vec::new();
    for selection in selections
        .as_slice()
        .iter()
        .copied()
        .filter(|selection| !selection.is_caret())
    {
        if let Ok(projected) = layout
            .display_snapshot
            .project_text_range(selection.range())
        {
            ranges.extend(projected);
        }
    }
    if ranges.is_empty() {
        return None;
    }

    let mut positions = Vec::new();
    for line in &layout.lines {
        let row = ProjectedLineIndex::new(line.row.get());
        for whitespace in &line.whitespaces {
            let column = LogicalColumn::new(whitespace.display_column);
            let selected = ranges.iter().any(|range| {
                let start = range.start();
                let end = range.end();
                (row > start.line() || (row == start.line() && column >= start.column()))
                    && (row < end.line() || (row == end.line() && column < end.column()))
            });
            if !selected || whitespace.byte_range.end > line.shaped.text.len() {
                continue;
            }
            let start_x = line.shaped.x_for_index(whitespace.byte_range.start);
            let end_x = line.shaped.x_for_index(whitespace.byte_range.end);
            positions.push((
                point(line.origin.x + start_x, line.origin.y),
                end_x - start_x,
            ));
        }
    }
    if positions.is_empty() {
        return None;
    }

    let marker = "•";
    let text_style = window.text_style();
    let font_size = text_style.font_size.to_pixels(window.rem_size()) / 2.;
    let symbol = window.text_system().shape_line(
        marker.into(),
        font_size,
        &[TextRun {
            len: marker.len(),
            font: text_style.font(),
            color: color::current(cx).editor_invisible.into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    );
    let origins = positions
        .into_iter()
        .map(|(origin, whitespace_width)| {
            point(
                origin.x + (whitespace_width - symbol.width).max(Pixels::ZERO) / 2.,
                origin.y,
            )
        })
        .collect();

    Some(SelectedWhitespaceMarkers {
        symbol,
        origins,
        line_height,
    })
}

/// 把单个投影选区切分为各显示行的片段：跨行连续段合并为轮廓（行间隙处断开），轮廓上下文在 finish_selection_contour 中计算每行四角样式。
fn layout_selection_segments(
    range: ProjectedRange,
    layout: &EditorLayout,
    line_height: Pixels,
    per_line: &mut [Vec<SelectionLineSegment>],
) {
    let corner_radius = line_height * 0.15;
    let line_end_overshoot = corner_radius * 2.;
    let start = range.start();
    let end = range.end();

    // 轮廓：连续显示行（行间隙处断开），行内为 (布局行索引, 起点 x, 终点 x)。
    let mut contour: Vec<(usize, Pixels, Pixels)> = Vec::new();
    let mut previous_y: Option<Pixels> = None;
    for (ix, line) in layout.lines.iter().enumerate() {
        let row = ProjectedLineIndex::new(line.row.get());
        if row < start.line() || row > end.line() {
            continue;
        }
        if row == end.line() && row != start.line() && end.column() == LogicalColumn::ZERO {
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
        let start_x = line.origin.x
            + line
                .shaped
                .x_for_index(column_to_byte(&line.shaped.text, start_column));
        let mut end_x = line.origin.x
            + line
                .shaped
                .x_for_index(column_to_byte(&line.shaped.text, end_column));
        if row != end.line() {
            end_x += line_end_overshoot;
        }
        if end_x <= start_x {
            continue;
        }
        if previous_y.is_some_and(|previous| line.origin.y != previous + line_height) {
            finish_selection_contour(&contour, corner_radius, per_line);
            contour.clear();
        }
        contour.push((ix, start_x, end_x));
        previous_y = Some(line.origin.y);
    }
    finish_selection_contour(&contour, corner_radius, per_line);
}

/// 背景片段合成：逐行把选区与 run 背景（搜索高亮、语法背景）合成为互不重叠的片段。
/// 混合顺序 base → 选区 → run，与既有视觉层级一致（run 背景覆盖选区之上、文本之下）。
fn layout_background_fragments(
    layout: &EditorLayout,
    selection_segments: &[Vec<SelectionLineSegment>],
    cx: &App,
) -> Vec<Vec<BackgroundFragment>> {
    let colors = color::current(cx);
    // 先与 Editor 基础背景展平：MultiBuffer 的 diff 行、普通上下文行与单文件 Editor 使用同一个最终选区颜色，不因后方背景不同产生另一种视觉颜色。
    let selection_background = colors
        .editor_background
        .blend(colors.editor_selection_background);
    layout
        .lines
        .iter()
        .zip(selection_segments)
        .map(|(line, segments)| {
            layout_line_background_fragments(line, segments, selection_background)
        })
        .collect()
}

fn layout_line_background_fragments(
    line: &LayoutLine,
    selection_segments: &[SelectionLineSegment],
    selection_background: gpui::Rgba,
) -> Vec<BackgroundFragment> {
    // 收集所有源的 x 边界（选区 + run 背景），按边界切分后逐段着色，再合并相邻同色段。
    let mut boundaries =
        Vec::with_capacity(selection_segments.len() * 2 + line.background_runs.len() * 2);
    for segment in selection_segments {
        boundaries.push(segment.start_x);
        boundaries.push(segment.end_x);
    }
    for (byte_range, _) in &line.background_runs {
        boundaries.push(line.shaped.x_for_index(byte_range.start));
        boundaries.push(line.shaped.x_for_index(byte_range.end));
    }
    if boundaries.is_empty() {
        return Vec::new();
    }
    boundaries.sort_unstable_by(|a, b| a.partial_cmp(b).expect("背景边界坐标必须可比较"));
    boundaries.dedup();

    let mut fragments: Vec<BackgroundFragment> = Vec::new();
    for span in boundaries.windows(2) {
        let start_x = span[0];
        let end_x = span[1];
        if end_x <= start_x {
            continue;
        }
        let selection = selection_segments
            .iter()
            .find(|segment| segment.start_x <= start_x && end_x <= segment.end_x);
        let run = line.background_runs.iter().find(|(byte_range, _)| {
            let run_start = line.shaped.x_for_index(byte_range.start);
            let run_end = line.shaped.x_for_index(byte_range.end);
            run_start <= start_x && end_x <= run_end
        });
        let (color, is_selection, corners) = match (selection, run) {
            (Some(segment), Some((_, run_color))) => (
                selection_background.blend(*run_color),
                true,
                segment_corners(segment, start_x, end_x),
            ),
            (Some(segment), None) => (
                selection_background,
                true,
                segment_corners(segment, start_x, end_x),
            ),
            (None, Some((_, run_color))) => (*run_color, false, ALL_STRAIGHT),
            (None, None) => continue,
        };
        // 相邻片段同色且接缝两侧无角样式时合并，保持片段数最小。
        if let Some(last) = fragments.last_mut()
            && last.end_x == start_x
            && last.selection == is_selection
            && last.color == color
            && last.corners[TOP_RIGHT].style == CornerStyle::Straight
            && last.corners[BOTTOM_RIGHT].style == CornerStyle::Straight
            && corners[TOP_LEFT].style == CornerStyle::Straight
            && corners[BOTTOM_LEFT].style == CornerStyle::Straight
        {
            last.end_x = end_x;
            last.corners[TOP_RIGHT] = corners[TOP_RIGHT];
            last.corners[BOTTOM_RIGHT] = corners[BOTTOM_RIGHT];
            continue;
        }
        fragments.push(BackgroundFragment {
            start_x,
            end_x,
            color,
            selection: is_selection,
            corners,
        });
    }
    fragments
}

/// 片段角样式：只有与选区轮廓外缘重合的角才携带样式，内部接缝一律直角。
fn segment_corners(segment: &SelectionLineSegment, start_x: Pixels, end_x: Pixels) -> [Corner; 4] {
    let mut corners = ALL_STRAIGHT;
    if start_x == segment.start_x {
        corners[TOP_LEFT] = segment.corners[TOP_LEFT];
        corners[BOTTOM_LEFT] = segment.corners[BOTTOM_LEFT];
    }
    if end_x == segment.end_x {
        corners[TOP_RIGHT] = segment.corners[TOP_RIGHT];
        corners[BOTTOM_RIGHT] = segment.corners[BOTTOM_RIGHT];
    }
    corners
}

fn layout_bracket_pair(
    pair: BracketPair,
    layout: &EditorLayout,
    line_height: Pixels,
    quads: &mut Vec<PaintQuad>,
    cx: &App,
) {
    let bracket_background = color::current(cx).editor_document_highlight_bracket_background;
    for range in [pair.open, pair.close] {
        let Ok(range) = TextRange::new(ByteOffset::new(range.start), ByteOffset::new(range.end))
        else {
            continue;
        };
        let Ok(projected) = layout.display_snapshot.project_text_range(range) else {
            continue;
        };
        for range in projected {
            layout_projected_range_quad(range, layout, line_height, bracket_background, quads);
        }
    }
}

fn layout_projected_range_quad(
    range: ProjectedRange,
    layout: &EditorLayout,
    line_height: Pixels,
    background: gpui::Rgba,
    selection_quads: &mut Vec<PaintQuad>,
) {
    let start = range.start();
    let end = range.end();
    for line in &layout.lines {
        let row = ProjectedLineIndex::new(line.row.get());
        if row < start.line() || row > end.line() {
            continue;
        }
        if row == end.line() && row != start.line() && end.column() == zcv_text::LogicalColumn::ZERO
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
            background,
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
    if line.fold_segments.is_some() {
        // 折叠合并行：显示列即合并文本字符列（占位符与尾段都在行文本内）。
        return column_to_byte(&line.shaped.text, point.column().get());
    }
    let logical_column = line
        .logical_line
        .and_then(|logical_line| {
            display_snapshot
                .display_to_logical_column(logical_line, point.column())
                .ok()
        })
        .map_or(0, zcv_text::LogicalColumn::get);
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
    use crate::display_map::{DisplayMap, InsertedLines, StyledLine};
    use gpui::{AppContext, Empty, TestAppContext};
    use std::path::{Path, PathBuf};
    use zcv_git::DiffHunk;
    use zcv_language::{LanguageBuffer, SyntaxSnapshot};
    use zcv_multi_buffer::{MultiBuffer, MultiBufferExcerpt};
    use zcv_text::{Buffer, BufferConfig, ByteOffset, Line, TextRange};

    /// 行级标记的显示行区间（`hunk_rendering` 的薄包装，测试专用）。
    fn diff_hunk_rows(
        snapshot: &DisplaySnapshot,
        hunks: &[DiffHunk],
        expanded_deleted: &[Range<usize>],
        expanded_modified: &[Range<usize>],
    ) -> Vec<(Range<usize>, DiffHunkKind)> {
        hunk_rendering(snapshot, hunks, expanded_deleted, expanded_modified, None).diff_rows
    }

    /// hunk 竖条范围与状态色（`hunk_rendering` 的薄包装，测试专用）。
    fn hunk_strip_rows(
        snapshot: &DisplaySnapshot,
        hunks: &[DiffHunk],
        expanded_deleted: &[Range<usize>],
        expanded_modified: &[Range<usize>],
    ) -> Vec<(Range<usize>, DiffHunkKind)> {
        hunk_rendering(snapshot, hunks, expanded_deleted, expanded_modified, None).strips
    }

    /// 可点击的 hunk 色带区域（`hunk_rendering` 的薄包装，测试专用）。
    fn hunk_hit_regions(
        snapshot: &DisplaySnapshot,
        hunks: &[DiffHunk],
        expanded_deleted: &[Range<usize>],
        expanded_modified: &[Range<usize>],
    ) -> Vec<(Range<usize>, Range<usize>, DiffHunkKind)> {
        hunk_rendering(snapshot, hunks, expanded_deleted, expanded_modified, None).hit_regions
    }
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
    fn editor_width_soft_wrap_keeps_mixed_cjk_inside_text_bounds(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Empty);
        window
            .update(cx, |_, window, _cx| {
                let text = "新增 Zcv 架构维护技能并整合可见性清理、架构体检与架构减法流程";
                let snapshot = Buffer::scratch(text.to_owned(), BufferConfig::default())
                    .expect("测试 Buffer 应能创建")
                    .snapshot();
                let font = typography::ui_font();
                let font_size = typography::ui();
                let font_id = window.text_system().resolve_font(&font);
                let em_advance = window
                    .text_system()
                    .em_advance(font_id, font_size)
                    .expect("UI 字体必须包含拉丁字形");
                let mut map = DisplayMap::new(snapshot);

                for quarter_pixels in 720..=2_400 {
                    let text_width = px(quarter_pixels as f32 / 4.);
                    let wrap_width =
                        calculate_wrap_width(SoftWrap::EditorWidth, text_width, 80, em_advance);
                    map.set_wrap_width(wrap_width, font.clone(), font_size, window.text_system());
                    let display = map.snapshot();
                    let viewport = display
                        .slice_viewport(DisplayRow::ZERO, display.line_count())
                        .expect("应读取完整软换行视口");
                    if quarter_pixels == 720 {
                        let rows: Vec<_> = viewport
                            .rows()
                            .iter()
                            .map(|row| {
                                let WrapViewportRowKind::Text {
                                    text, byte_range, ..
                                } = row.kind();
                                text.as_ref()[byte_range.clone()].to_owned()
                            })
                            .collect();
                        assert_eq!(
                            rows,
                            [
                                "新增 Zcv 架构维护技能并整合可见性清理、",
                                "架构体检与架构减法流程",
                            ]
                        );
                    }

                    for row in viewport.rows() {
                        let WrapViewportRowKind::Text {
                            text, byte_range, ..
                        } = row.kind();
                        let row_text = &text.as_ref()[byte_range.clone()];
                        let run = TextRun {
                            len: row_text.len(),
                            font: font.clone(),
                            color: gpui::black(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let shaped = window.text_system().shape_line(
                            row_text.to_owned().into(),
                            font_size,
                            &[run],
                            None,
                        );
                        assert!(
                            shaped.width + CARET_WIDTH <= text_width,
                            "软换行行尾应保留光标宽度：width={}, shaped={}，row={row_text:?}",
                            f32::from(text_width),
                            f32::from(shaped.width),
                        );
                    }
                }
            })
            .expect("测试窗口应保持可用");
    }

    #[gpui::test]
    fn inserted_lines_render_without_applying_anchor_spans(cx: &mut TestAppContext) {
        // 回归：合成行（外部文本）无语法高亮/选区——锚定行的 span 端点套用到合成行文本会落在中文中间（非字符边界切片 panic）。
        let window = cx.add_window(|_, _| Empty);
        window
            .update(cx, |_, window, cx| {
                let snapshot = Buffer::scratch("// a\n// b".to_owned(), BufferConfig::default())
                    .expect("测试 Buffer 应能创建")
                    .snapshot();
                let mut map = DisplayMap::new(snapshot.clone());
                // 展开删除块：锚定行 0 后插入含中文的 HEAD 行；锚定行短，span 端点在锚定行外。
                map.set_inserted(InsertedLines::from([(
                    Line::ZERO,
                    vec![StyledLine::plain(std::sync::Arc::from(
                        "// 展开产生的新行在重建时现查 git 状态，无需单独补齐。",
                    ))],
                )]));
                // 懒查询模式下渲染侧不再接收注入 spans：空语法快照使高亮查询为空，合成行（外部文本）仍走 LineStyles::default() 无高亮路径。
                map.set_syntax_snapshot(SyntaxSnapshot::empty(snapshot.version()));
                let layout = layout_visible_lines(
                    map.snapshot(),
                    None,
                    EditorPresentation::new(&snapshot, None),
                    None,
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
                        fold_anchor_lines: &BTreeSet::new(),
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
    fn multibuffer_header_can_start_above_viewport(cx: &mut TestAppContext) {
        let text = "引擎\n";
        let source_text = cx.new(|_| {
            Buffer::scratch(text.to_owned(), BufferConfig::default()).expect("应创建源 Buffer")
        });
        let source = cx.new({
            let source_text = source_text.clone();
            move |cx| LanguageBuffer::new(source_text, Some(PathBuf::from("文档/引擎.md")), cx)
        });
        let source = cx.new(move |cx| MultiBuffer::singleton(source, cx));
        let combined = cx.new(MultiBuffer::empty);
        cx.update_entity(&combined, |combined, cx| {
            combined.set_excerpts(
                vec![
                    MultiBufferExcerpt::new(
                        source,
                        TextRange::new(ByteOffset::ZERO, ByteOffset::new(text.len()))
                            .expect("片段范围应有效"),
                        Vec::new(),
                    )
                    .with_display_path(PathBuf::from("文档/引擎.md")),
                ],
                cx,
            );
        });
        cx.run_until_parked();

        let multi_snapshot = cx.read_entity(&combined, |combined, cx| combined.snapshot(cx));
        let text_snapshot = multi_snapshot.text().clone();
        let display_snapshot = DisplayMap::new(multi_snapshot).snapshot();
        let window = cx.add_window(|_, _| Empty);
        window
            .update(cx, |_, window, cx| {
                let dimensions = GutterDimensions {
                    crease_width: px(8.),
                    left_padding: px(8.),
                    right_padding: px(8.),
                    width: px(56.),
                    margin: px(3.),
                };
                let gutter_bounds =
                    Bounds::new(point(px(0.), px(0.)), size(dimensions.width, px(80.)));
                // 文件标题占两行。视口从标题的第二行开始时，标题真实起点在
                // viewport 上方一行；这是合法的负布局偏移，不能用 usize 相减。
                let layout = layout_visible_lines(
                    display_snapshot,
                    None,
                    EditorPresentation::new(&text_snapshot, None),
                    None,
                    VisibleLineLayoutParams {
                        geometry: EditorGeometry {
                            text_bounds: Bounds::new(
                                point(px(59.), px(0.)),
                                size(px(341.), px(80.)),
                            ),
                            text_clip_bounds: Bounds::new(
                                point(px(56.), px(0.)),
                                size(px(344.), px(80.)),
                            ),
                            gutter: Some((gutter_bounds, dimensions)),
                        },
                        active_lines: &BTreeSet::new(),
                        foldable_lines: &BTreeSet::new(),
                        fold_anchor_lines: &BTreeSet::new(),
                        start_row: DisplayRow::new(1),
                        scroll_offset: point(px(0.), px(0.)),
                        line_height: px(20.),
                        diff_rows: &[],
                    },
                    window,
                    cx,
                );

                assert_eq!(layout.blocks.len(), 1);
                assert_eq!(layout.blocks[0].row, DisplayRow::ZERO);
                assert_eq!(layout.blocks[0].origin.x, px(0.));
                assert_eq!(layout.blocks[0].origin.y, px(-20.));
                assert_eq!(layout.block_clip_bounds.size.width, px(400.));
                assert_eq!(layout.lines[0].shaped.text.as_ref(), "引擎");
            })
            .expect("测试窗口应保持可用");
    }

    #[gpui::test]
    fn sticky_buffer_header_follows_excerpts_and_points_to_the_next_file(cx: &mut TestAppContext) {
        let first_text = "a0\na1\na2\na3\na4\na5\na6\na7\n";
        let first_buffer = cx.new(|_| {
            Buffer::scratch(first_text.to_owned(), BufferConfig::default())
                .expect("应创建第一个源 Buffer")
        });
        let first = cx
            .new(move |cx| LanguageBuffer::new(first_buffer, Some(PathBuf::from("src/a.rs")), cx));
        let first = cx.new(move |cx| MultiBuffer::singleton(first, cx));

        let second_text = "b0\nb1\n";
        let second_buffer = cx.new(|_| {
            Buffer::scratch(second_text.to_owned(), BufferConfig::default())
                .expect("应创建第二个源 Buffer")
        });
        let second = cx
            .new(move |cx| LanguageBuffer::new(second_buffer, Some(PathBuf::from("src/b.rs")), cx));
        let second = cx.new(move |cx| MultiBuffer::singleton(second, cx));

        let combined = cx.new(MultiBuffer::empty);
        cx.update_entity(&combined, |combined, cx| {
            combined.set_excerpts(
                vec![
                    MultiBufferExcerpt::line_range(first.clone(), 0..2, cx),
                    MultiBufferExcerpt::line_range(first, 5..7, cx),
                    MultiBufferExcerpt::line_range(second, 0..2, cx),
                ],
                cx,
            );
        });
        cx.run_until_parked();

        let snapshot = cx.read_entity(&combined, |combined, cx| combined.snapshot(cx));
        let display = DisplayMap::new(snapshot).snapshot();
        let blocks = display
            .slice_viewport(DisplayRow::ZERO, display.line_count())
            .expect("应读取完整显示投影")
            .rows()
            .iter()
            .filter_map(|row| {
                row.block()
                    .map(|block| (row.index(), block.kind, block.excerpt.source_start_line()))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            blocks.iter().map(|(_, kind, _)| *kind).collect::<Vec<_>>(),
            vec![
                DisplayBlockKind::BufferHeader,
                DisplayBlockKind::ExcerptBoundary,
                DisplayBlockKind::BufferHeader,
            ]
        );

        let first_header = display
            .sticky_buffer_header(DisplayRow::new(blocks[0].0.get() + FILE_HEADER_HEIGHT))
            .expect("第一个文件正文应有悬浮标题");
        assert_eq!(first_header.excerpt.path(), Path::new("src/a.rs"));
        assert_eq!(first_header.excerpt.source_start_line(), 1);

        let later_excerpt = display
            .sticky_buffer_header(blocks[1].0)
            .expect("同文件后续 excerpt 应更新悬浮标题目标");
        assert_eq!(later_excerpt.excerpt.path(), Path::new("src/a.rs"));
        assert_eq!(later_excerpt.excerpt.source_start_line(), 6);
        assert_eq!(later_excerpt.next_buffer_header_row, Some(blocks[2].0));

        let second_header = display
            .sticky_buffer_header(blocks[2].0)
            .expect("到达下一个文件后应切换悬浮标题");
        assert_eq!(second_header.excerpt.path(), Path::new("src/b.rs"));
        assert_eq!(second_header.next_buffer_header_row, None);
    }

    #[test]
    fn next_file_header_pushes_sticky_header_out() {
        let start = DisplayRow::new(10);
        let next = Some(DisplayRow::new(12));
        assert_eq!(
            sticky_buffer_header_origin_y(px(100.), px(20.), start, px(0.), next),
            px(100.)
        );
        assert_eq!(
            sticky_buffer_header_origin_y(px(100.), px(20.), start, px(10.), next),
            px(90.)
        );
        assert_eq!(
            sticky_buffer_header_origin_y(px(100.), px(20.), start, px(10.), None),
            px(100.)
        );
    }

    #[gpui::test]
    fn wrapped_unicode_markdown_queries_highlights_from_source_chunks(cx: &mut TestAppContext) {
        let text = "> **The reconstructed Functionally Equivalent Scene（功能等价场景）can be directly imported into ROS（机器人操作系统）to support interactive simulation（交互式仿真）and long-horizon robot task execution（长时序机器人任务执行）.**\n";
        let buffer = cx.new(|_| {
            Buffer::scratch(text.to_owned(), BufferConfig::default()).expect("应创建 Buffer")
        });
        let language_buffer =
            cx.new(|cx| LanguageBuffer::new(buffer.clone(), Some(PathBuf::from("README.md")), cx));
        cx.run_until_parked();
        let snapshot = cx.read_entity(&buffer, |buffer, _| buffer.snapshot());
        let syntax = cx.read_entity(&language_buffer, |buffer, _| buffer.syntax_snapshot());
        assert!(syntax.has_language());

        let window = cx.add_window(|_, _| Empty);
        window
            .update(cx, |_, window, cx| {
                let text_system = window.text_system().clone();
                let font = window.text_style().font();
                let font_size = window.text_style().font_size.to_pixels(window.rem_size());
                let mut map = DisplayMap::new(snapshot.clone());
                map.set_syntax_snapshot(syntax);

                // 选择一个“片段长度不是行首 UTF-8 边界”的续行。旧实现把这个长度
                // 拼到行首上查询高亮，正好会制造落在中文编码内部的 capture 端点。
                let mut offending_row = None;
                for width in [px(320.), px(400.), px(480.), px(560.), px(640.)] {
                    map.set_wrap_width(Some(width), font.clone(), font_size, &text_system);
                    let display = map.snapshot();
                    let viewport = display
                        .slice_viewport(DisplayRow::ZERO, display.line_count())
                        .expect("应读取完整视口");
                    offending_row = viewport.rows().iter().find_map(|row| {
                        let WrapViewportRowKind::Text {
                            text, byte_range, ..
                        } = row.kind();
                        (!text.is_char_boundary(byte_range.len())).then_some(row.index())
                    });
                    if offending_row.is_some() {
                        break;
                    }
                }
                let offending_row = offending_row.expect("测试文本应产生目标 UTF-8 续行");
                let display = map.snapshot();
                let layout = layout_visible_lines(
                    display,
                    None,
                    EditorPresentation::new(&snapshot, None),
                    None,
                    VisibleLineLayoutParams {
                        geometry: EditorGeometry {
                            text_bounds: Bounds::new(
                                point(px(0.), px(0.)),
                                size(px(700.), px(80.)),
                            ),
                            text_clip_bounds: Bounds::new(
                                point(px(0.), px(0.)),
                                size(px(700.), px(80.)),
                            ),
                            gutter: None,
                        },
                        active_lines: &BTreeSet::new(),
                        foldable_lines: &BTreeSet::new(),
                        fold_anchor_lines: &BTreeSet::new(),
                        start_row: offending_row,
                        scroll_offset: point(px(0.), px(0.)),
                        line_height: px(20.),
                        diff_rows: &[],
                    },
                    window,
                    cx,
                );
                assert_eq!(
                    layout.lines.first().map(|line| line.row),
                    Some(offending_row)
                );
                assert!(!layout.lines[0].shaped.text.is_empty());
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
                    None,
                    presentation,
                    None,
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
                        fold_anchor_lines: &BTreeSet::new(),
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
                    None,
                    EditorPresentation::new(&snapshot, None),
                    None,
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
                        fold_anchor_lines: &BTreeSet::new(),
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
                map.fold_range(
                    TextRange::new(ByteOffset::new(6), ByteOffset::new(28))
                        .expect("折叠范围应合法"),
                )
                .expect("折叠应成功");
                let layout = layout_visible_lines(
                    map.snapshot(),
                    None,
                    EditorPresentation::new(&snapshot, None),
                    None,
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
                        fold_anchor_lines: &BTreeSet::new(),
                        start_row: DisplayRow::ZERO,
                        scroll_offset: point(px(0.), px(0.)),
                        line_height: px(20.),
                        diff_rows: &[],
                    },
                    window,
                    cx,
                );

                assert_eq!(layout.lines.len(), 2);
                // 折叠合并行：anchor 文本 + 占位符拼成同一显示行。
                assert_eq!(layout.lines[0].shaped.text.as_ref(), "anchor…");
                assert_eq!(layout.lines[1].shaped.text.as_ref(), "after");
            })
            .expect("测试窗口应保持可用");
    }

    #[gpui::test]
    fn multi_line_selection_uses_one_rounded_contour_with_inner_turns(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Empty);
        window
            .update(cx, |_, window, cx| {
                let snapshot =
                    Buffer::scratch("abcdef\nx\nabcde".to_owned(), BufferConfig::default())
                        .expect("测试 Buffer 应能创建")
                        .snapshot();
                let layout = layout_visible_lines(
                    DisplayMap::new(snapshot.clone()).snapshot(),
                    None,
                    EditorPresentation::new(&snapshot, None),
                    None,
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
                        fold_anchor_lines: &BTreeSet::new(),
                        start_row: DisplayRow::ZERO,
                        scroll_offset: point(px(0.), px(0.)),
                        line_height: px(20.),
                        diff_rows: &[],
                    },
                    window,
                    cx,
                );
                let selections = SelectionSet::new(vec![crate::selection::Selection::new(
                    ByteOffset::new(2),
                    ByteOffset::new(12),
                )]);
                let (segments, _) = layout_selections(&selections, &layout, px(20.), cx);
                let segments = segments
                    .into_iter()
                    .filter(|line| !line.is_empty())
                    .map(|mut line| line.remove(0))
                    .collect::<Vec<_>>();

                assert_eq!(segments.len(), 3, "连续多行选区应生成三条行片段");
                // 首行携带顶角圆角，末行携带底角圆角，宽度封顶于圆角半径（20 × 0.15）。
                assert_eq!(segments[0].corners[TOP_LEFT].style, CornerStyle::Round);
                assert_eq!(segments[0].corners[TOP_LEFT].width, px(3.));
                assert_eq!(segments[2].corners[BOTTOM_RIGHT].style, CornerStyle::Round);
                assert!(
                    segments[0].start_x > segments[1].start_x,
                    "首行左边界转入后续行时应形成内凹倒圆角"
                );
                assert!(
                    segments[0].end_x > segments[1].end_x && segments[2].end_x > segments[1].end_x,
                    "相邻行宽度收缩与扩张应形成两种圆角转折"
                );
                assert_eq!(
                    segments[0].end_x,
                    layout.lines[0].origin.x + layout.lines[0].shaped.width + px(6.),
                    "非末行应按 Zed 语义延伸两个圆角半径"
                );
            })
            .expect("测试窗口应保持可用");
    }

    #[gpui::test]
    fn selected_spaces_render_one_dot_each_without_treating_tabs_as_spaces(
        cx: &mut TestAppContext,
    ) {
        let window = cx.add_window(|_, _| Empty);
        window
            .update(cx, |_, window, cx| {
                let snapshot = Buffer::scratch("a  b\tc".to_owned(), BufferConfig::default())
                    .expect("测试 Buffer 应能创建")
                    .snapshot();
                let layout = layout_visible_lines(
                    DisplayMap::new(snapshot.clone()).snapshot(),
                    None,
                    EditorPresentation::new(&snapshot, None),
                    None,
                    VisibleLineLayoutParams {
                        geometry: EditorGeometry {
                            text_bounds: Bounds::new(
                                point(px(0.), px(0.)),
                                size(px(400.), px(40.)),
                            ),
                            text_clip_bounds: Bounds::new(
                                point(px(0.), px(0.)),
                                size(px(400.), px(40.)),
                            ),
                            gutter: None,
                        },
                        active_lines: &BTreeSet::new(),
                        foldable_lines: &BTreeSet::new(),
                        fold_anchor_lines: &BTreeSet::new(),
                        start_row: DisplayRow::ZERO,
                        scroll_offset: point(px(0.), px(0.)),
                        line_height: px(20.),
                        diff_rows: &[],
                    },
                    window,
                    cx,
                );

                assert_eq!(layout.lines[0].whitespaces.len(), 2);
                let selected_spaces = SelectionSet::new(vec![crate::selection::Selection::new(
                    ByteOffset::new(1),
                    ByteOffset::new(3),
                )]);
                let markers =
                    layout_selected_whitespace(&selected_spaces, &layout, px(20.), window, cx)
                        .expect("选中的两个空格都应生成圆点标记");
                assert_eq!(markers.symbol.text.as_ref(), "•");
                assert_eq!(markers.origins.len(), 2);
                assert!(markers.origins[0].x < markers.origins[1].x);

                let selected_tab = SelectionSet::new(vec![crate::selection::Selection::new(
                    ByteOffset::new(4),
                    ByteOffset::new(5),
                )]);
                assert!(
                    layout_selected_whitespace(&selected_tab, &layout, px(20.), window, cx,)
                        .is_none(),
                    "tab 展开的空格不能被误画成多个圆点"
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
                    None,
                    presentation,
                    None,
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
                        fold_anchor_lines: &BTreeSet::new(),
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
    fn multibuffer_excerpt_uses_the_same_text_selection_geometry_as_a_single_buffer(
        cx: &mut TestAppContext,
    ) {
        let text = "abcdef\nx\nabcde\n";
        let source_buffer = cx.new(|_| {
            Buffer::scratch(text.to_owned(), BufferConfig::default()).expect("应创建源 Buffer")
        });
        let source = cx.new(move |cx| {
            LanguageBuffer::new(source_buffer, Some(PathBuf::from("src/example.rs")), cx)
        });
        let source = cx.new(move |cx| MultiBuffer::singleton(source, cx));
        let combined = cx.new(MultiBuffer::empty);
        cx.update_entity(&combined, |combined, cx| {
            combined.set_excerpts(vec![MultiBufferExcerpt::line_range(source, 0..3, cx)], cx)
        });
        cx.run_until_parked();

        let multi_snapshot = cx.read_entity(&combined, |combined, cx| combined.snapshot(cx));
        let multi_text = multi_snapshot.text().clone();
        let single_text =
            Buffer::scratch(text.to_owned(), BufferConfig::default()).expect("应创建单文件 Buffer");
        let single_snapshot = single_text.snapshot();
        let selection = SelectionSet::new(vec![crate::selection::Selection::new(
            ByteOffset::new(2),
            ByteOffset::new(12),
        )]);

        let window = cx.add_window(|_, _| Empty);
        window
            .update(cx, |_, window, cx| {
                let geometry = EditorGeometry {
                    text_bounds: Bounds::new(point(px(0.), px(0.)), size(px(400.), px(160.))),
                    text_clip_bounds: Bounds::new(point(px(0.), px(0.)), size(px(400.), px(160.))),
                    gutter: None,
                };
                let active_lines = BTreeSet::new();
                let foldable_lines = BTreeSet::new();
                let fold_anchor_lines = BTreeSet::new();
                let params = |geometry| VisibleLineLayoutParams {
                    geometry,
                    active_lines: &active_lines,
                    foldable_lines: &foldable_lines,
                    fold_anchor_lines: &fold_anchor_lines,
                    start_row: DisplayRow::ZERO,
                    scroll_offset: point(px(0.), px(0.)),
                    line_height: px(20.),
                    diff_rows: &[],
                };
                let single_layout = layout_visible_lines(
                    DisplayMap::new(single_snapshot.clone()).snapshot(),
                    None,
                    EditorPresentation::new(&single_snapshot, None),
                    None,
                    params(geometry),
                    window,
                    cx,
                );
                let multi_layout = layout_visible_lines(
                    DisplayMap::new(multi_snapshot).snapshot(),
                    None,
                    EditorPresentation::new(&multi_text, None),
                    None,
                    params(geometry),
                    window,
                    cx,
                );

                let (single_segments, single_carets) =
                    layout_selections(&selection, &single_layout, px(20.), cx);
                let (multi_segments, multi_carets) =
                    layout_selections(&selection, &multi_layout, px(20.), cx);
                let single_fragments =
                    layout_background_fragments(&single_layout, &single_segments, cx);
                let multi_fragments =
                    layout_background_fragments(&multi_layout, &multi_segments, cx);

                assert_eq!(multi_fragments.len(), single_fragments.len());
                for (multi_line, single_line) in multi_fragments.iter().zip(&single_fragments) {
                    assert_eq!(
                        multi_line
                            .iter()
                            .map(|fragment| (fragment.start_x, fragment.end_x))
                            .collect::<Vec<_>>(),
                        single_line
                            .iter()
                            .map(|fragment| (fragment.start_x, fragment.end_x))
                            .collect::<Vec<_>>()
                    );
                }
                // 角样式也应与单文件一致（合成管线与缓冲形态无关）。
                assert_eq!(
                    multi_fragments
                        .iter()
                        .flatten()
                        .map(|fragment| fragment.corners)
                        .collect::<Vec<_>>(),
                    single_fragments
                        .iter()
                        .flatten()
                        .map(|fragment| fragment.corners)
                        .collect::<Vec<_>>()
                );
                assert_eq!(multi_carets.len(), single_carets.len());
                let colors = color::current(cx);
                let selection_fragment = single_fragments
                    .iter()
                    .flatten()
                    .find(|fragment| fragment.selection)
                    .expect("应有选区片段");
                assert_eq!(
                    selection_fragment.color,
                    colors
                        .editor_background
                        .blend(colors.editor_selection_background)
                );
                assert_eq!(
                    selection_fragment.color.a, 1.0,
                    "选区颜色应在 Editor 背景上展平"
                );
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
        map.set_inserted(InsertedLines::from([(
            Line::new(1),
            vec![
                StyledLine::plain(std::sync::Arc::from("old 1")),
                StyledLine::plain(std::sync::Arc::from("old 2")),
            ],
        )]));
        let snapshot = map.snapshot();
        let hunk = DiffHunk {
            range: 1..1,
            old_range: 1..3,
            kind: DiffHunkKind::Deleted,
        };
        // 未展开：行内无标记（删除点由 gutter 红色三角提示）。
        assert_eq!(
            diff_hunk_rows(&snapshot, std::slice::from_ref(&hunk), &[], &[]),
            vec![]
        );
        // 展开：背景覆盖被删除的合成行（锚定行 1 之后，显示行 2..4）。
        assert_eq!(
            diff_hunk_rows(
                &snapshot,
                std::slice::from_ref(&hunk),
                std::slice::from_ref(&(1..3)),
                &[]
            ),
            vec![(2..4, DiffHunkKind::Deleted)]
        );
        // 点击区域：折叠态在锚定行（分界线在锚定行底部），展开态覆盖合成行。
        assert_eq!(
            hunk_hit_regions(&snapshot, std::slice::from_ref(&hunk), &[], &[]),
            vec![(1..2, 1..3, DiffHunkKind::Deleted)]
        );
        assert_eq!(
            hunk_hit_regions(&snapshot, &[hunk], std::slice::from_ref(&(1..3)), &[]),
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
        map.set_inserted(InsertedLines::from([(
            Line::ZERO, // 修改块锚定 range.start - 1（旧行在修改行上方）。
            vec![StyledLine::plain(std::sync::Arc::from("old 1"))],
        )]));
        let snapshot = map.snapshot();
        let hunk = DiffHunk {
            range: 1..2,
            old_range: 1..2,
            kind: DiffHunkKind::Modified,
        };
        // 未展开：修改行（显示行 2..3）Modified 色。
        assert_eq!(
            diff_hunk_rows(&snapshot, std::slice::from_ref(&hunk), &[], &[]),
            vec![(2..3, DiffHunkKind::Modified)]
        );
        // 展开：旧行合成行（显示行 1..2）删除色 + 修改行（显示行 2..3）新增色。
        assert_eq!(
            diff_hunk_rows(
                &snapshot,
                std::slice::from_ref(&hunk),
                &[],
                std::slice::from_ref(&(1..2))
            ),
            vec![(1..2, DiffHunkKind::Deleted), (2..3, DiffHunkKind::Added),]
        );
        // 点击区域：未展开 = 修改行；展开 = 旧行合成行（都在修改行附近）。
        assert_eq!(
            hunk_hit_regions(&snapshot, std::slice::from_ref(&hunk), &[], &[]),
            vec![(2..3, 1..2, DiffHunkKind::Modified)]
        );
        // 展开的点击区域覆盖整个 hunk（旧行 + 修改行，与竖条一致）。
        assert_eq!(
            hunk_hit_regions(&snapshot, &[hunk], &[], std::slice::from_ref(&(1..2))),
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
        map.set_inserted(InsertedLines::from([(
            Line::ZERO,
            vec![StyledLine::plain(std::sync::Arc::from("old 1"))],
        )]));
        let snapshot = map.snapshot();
        let hunk = DiffHunk {
            range: 1..2,
            old_range: 1..2,
            kind: DiffHunkKind::Modified,
        };
        // 未展开：竖条覆盖修改行（显示行 2..3），黄色。
        assert_eq!(
            hunk_strip_rows(&snapshot, std::slice::from_ref(&hunk), &[], &[]),
            vec![(2..3, DiffHunkKind::Modified)]
        );
        // 展开：竖条仍黄，覆盖旧行 + 修改行（显示行 1..3）。
        assert_eq!(
            hunk_strip_rows(&snapshot, &[hunk], &[], std::slice::from_ref(&(1..2))),
            vec![(1..3, DiffHunkKind::Modified)]
        );
    }
}

/// 拖拽选择时的视口自动滚动量（对齐 Zed `mouse_dragged`）：
/// 鼠标在文本视口边缘外时按超出距离的 1.2 次方缩放，垂直上限 3 像素/事件，保证平滑。
fn drag_autoscroll_delta(
    position: Point<Pixels>,
    text_bounds: Bounds<Pixels>,
    line_height: Pixels,
) -> Point<Pixels> {
    let mut delta = point(Pixels::ZERO, Pixels::ZERO);
    let vertical_margin = line_height.min(text_bounds.size.height / 3.0);
    let top = text_bounds.origin.y + vertical_margin;
    let bottom = text_bounds.bottom_left().y - vertical_margin;
    if position.y < top {
        delta.y = scale_drag_autoscroll(top - position.y);
    } else if position.y > bottom {
        delta.y = -scale_drag_autoscroll(position.y - bottom);
    }
    // 水平边距近似 4 个 em（行高约 1.618em），与 Zed `horizontal_scroll_margin` 默认一致。
    let horizontal_margin = 2.5 * line_height;
    let left = text_bounds.origin.x + horizontal_margin;
    let right = text_bounds.top_right().x - horizontal_margin;
    if position.x < left {
        delta.x = -scale_drag_autoscroll_horizontal(left - position.x);
    } else if position.x > right {
        delta.x = scale_drag_autoscroll_horizontal(position.x - right);
    }
    delta
}

fn scale_drag_autoscroll(distance: Pixels) -> Pixels {
    px((f32::from(distance).powf(1.2) / 100.0).min(3.0))
}

fn scale_drag_autoscroll_horizontal(distance: Pixels) -> Pixels {
    px(f32::from(distance).powf(1.2) / 300.0)
}

#[cfg(test)]
mod autoscroll_tests {
    use super::*;

    fn text_bounds(origin_y: f32, height: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(0.), px(origin_y)),
            size: size(px(800.), px(height)),
        }
    }

    #[test]
    fn drag_autoscroll_only_scrolls_when_cursor_passes_viewport_edge() {
        let line_height = px(20.);
        let bounds = text_bounds(0., 200.);

        // 视口内：不滚动。
        assert_eq!(
            drag_autoscroll_delta(point(px(100.), px(100.)), bounds, line_height),
            point(Pixels::ZERO, Pixels::ZERO)
        );
        // 上边缘外：向上回看（正 y）。
        let delta = drag_autoscroll_delta(point(px(100.), px(-100.)), bounds, line_height);
        assert!(delta.y > Pixels::ZERO);
        // 下边缘外：向下查看新内容（负 y），滚动量有上限保证平滑。
        let delta = drag_autoscroll_delta(point(px(100.), px(300.)), bounds, line_height);
        assert!(delta.y < Pixels::ZERO);
        assert!(f32::from(delta.y).abs() <= 3.0);
        // 右边缘外：水平滚动（正 x）。
        let delta = drag_autoscroll_delta(point(px(900.), px(100.)), bounds, line_height);
        assert!(delta.x > Pixels::ZERO);
    }
}
