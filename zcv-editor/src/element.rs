//! Editor 的逐帧文本布局、绘制与像素命中测试。

use std::collections::BTreeSet;
use std::ops::Range;
use std::sync::Arc;

use gpui::{
    AnyElement, App, AvailableSpace, Bounds, ContentMask, Context, DispatchPhase, Element,
    ElementId, ElementInputHandler, Entity, GlobalElementId, HitboxBehavior, InspectorElementId,
    InteractiveElement, IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PaintQuad, Pixels, Point, ScrollWheelEvent, ShapedLine, Style, TextRun, Window,
    fill, point, px, relative, size,
};
use zcv_actions::ToggleFold;
use zcv_git::DiffHunkKind;
use zcv_language::BracketPair;
use zcv_text::{ByteOffset, Line, LogicalColumn, SearchMatch, TextRange};
use zcv_theme::color;
use zcv_ui::Glyph;

use crate::SelectionSet;

use super::display_map::{
    BufferPoint, DisplayColumn, DisplayPoint, DisplayRow, DisplaySnapshot, FoldRowSegment,
    ProjectedRange, RowStyleInput, WrapRowInfo, WrapViewportRowKind, byte_for_display_column,
    render_viewport_row,
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
    /// 折叠合并行的段表（anchor 文本 + 占位符 + 闭合尾段；命中测试与占位符点击用）。
    fold_segments: Option<Vec<FoldRowSegment>>,
    /// 该显示行所属的 git diff 类型（内容背景用；wrap 续行同样标注）。
    git_diff: Option<DiffHunkKind>,
    /// placeholder 提示行：命中测试不映射到 placeholder buffer（空 buffer 唯一合法坐标是 0）。
    is_placeholder: bool,
}

struct EditorLayout {
    lines: Vec<LayoutLine>,
    gutter: Option<GutterLayout>,
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
    selections: Vec<PaintQuad>,
    carets: Vec<PaintQuad>,
    ime_caret_bounds: Option<Bounds<Pixels>>,
    hitbox: gpui::Hitbox,
    gutter_hitbox: Option<gpui::Hitbox>,
    /// hunk 色带 hitbox（起点行可见时插入；点击切换折叠/展开；类型 + 展开态标志）。
    deleted_hunk_hitboxes: Arc<Vec<HunkHitbox>>,
    /// crease 折叠开关（Glyph 组件，已按 gutter 绝对坐标布局；自带点击与 tooltip）。
    crease_toggles: Vec<Option<AnyElement>>,
    /// 折叠占位符点击 hitbox（合并行占位符段；点击展开）。
    placeholder_hitboxes: Arc<Vec<(gpui::Hitbox, Line)>>,
    /// hunk 竖条范围与状态色（竖条色不随展开变化；行背景按行状态另行绘制）。
    hunk_strips: Arc<Vec<(Range<usize>, DiffHunkKind)>>,
    /// 展开态行区间（内容区与 gutter 行背景只画这些行）。
    expanded_rows: Arc<Vec<Range<usize>>>,
    scrollbar: Option<ScrollbarLayout>,
}

/// hunk 色带 hitbox：命中区域 + 点击目标范围 + 类型 + 展开态标志。
type HunkHitbox = (gpui::Hitbox, Range<usize>, DiffHunkKind, bool);

/// 构建 crease 折叠开关：Glyph 组件（chevron 图标 + tooltip + 点击），按 gutter 绝对坐标 as_root 独立布局（对齐 Zed 的 prepaint_crease_toggles）。
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
        let mut toggle = Glyph::icon(("gutter_crease", line.get()), path)
            .label(if folded { "展开" } else { "折叠" })
            .shortcut(&ToggleFold, cx)
            .on_click(move |_event, _window, cx| {
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
        let wrap_width = match soft_wrap {
            SoftWrap::None => None,
            SoftWrap::EditorWidth => Some(text_bounds.size.width),
            SoftWrap::Bounded => {
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
            // 垂直自动滚动在布局前应用：光标行进出视口的锚点修正只依赖行与视口几何，
            // 提前消费后首遍布局即为最终布局，光标移动帧不再整帧重排。
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
        // git diff 渲染数据：行标记、竖条与点击区域单遍计算共用（只依赖 snapshot 与注入 hunks，
        // 与滚动位置无关，autoscroll 重排可复用）。
        let hunk_render = {
            let editor = self.editor.read(cx);
            hunk_rendering(
                &display_snapshot,
                editor.diff_hunks(cx),
                editor.expanded_deleted_hunks(),
                editor.expanded_modified_hunks(),
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
        // 折叠占位符点击 hitbox：合并行占位符段区域（点击展开；交互型直接调 Entity 方法）。
        let placeholder_hitboxes = {
            let mut hitboxes = Vec::new();
            for line in &layout.lines {
                let Some(segments) = line.fold_segments.as_ref() else {
                    continue;
                };
                let indent = line.wrap_info.map_or(0, |info| info.indent);
                // 占位符段 = 合并文本中 anchor 段之后；显示文本 = 假空格 + 合并文本。
                let start = indent + segments[0].merged_range.end;
                let end = start + segments[1].merged_range.len();
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
        let scrollbar = layout_scrollbar(
            &mode,
            scrollbar_bounds,
            &hunk_render,
            line_height,
            &self.editor,
            cx,
            window,
        );

        // 展开态行区间（内容区与 gutter 行背景共用；未展开的 hunk 只由竖条/三角提示）。
        let expanded_rows = Arc::new(hunk_render.expanded_rows.clone());
        PrepaintState {
            layout,
            selections,
            carets,
            ime_caret_bounds,
            hitbox: window.insert_hitbox(bounds, HitboxBehavior::Normal),
            gutter_hitbox,
            deleted_hunk_hitboxes,
            crease_toggles,
            placeholder_hitboxes,
            hunk_strips,
            expanded_rows,
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
        let placeholder_hitboxes = prepaint.placeholder_hitboxes.clone();
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
            // 折叠占位符点击：展开该行（交互型，直接调 Entity 方法；先于 gutter 行号选行）。
            // crease 箭头点击由 Glyph 组件自带的 on_click 处理。
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
            // 折叠占位符可点击：hover 时手型光标（crease 箭头由 Glyph 自带 cursor_pointer）。
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
            // crease 折叠开关：Glyph 组件（chevron 图标 + tooltip + 点击），
            // prepaint 已按 gutter 绝对坐标布局，这里在 gutter 区域内绘制。
            window.paint_layer(gutter.bounds, |window| {
                for toggle in prepaint.crease_toggles.iter_mut().flatten() {
                    toggle.paint(window, cx);
                }
            });
            for row in &gutter.rows {
                if let Err(error) =
                    row.shaped_line_number
                        .paint(row.origin, gutter.line_height, window, cx)
                {
                    // 单个字形绘制失败只跳过该行，不能让整个窗口崩溃（对齐 Zed 的降级策略）。
                    eprintln!("Editor gutter 行号绘制失败：{error}");
                    continue;
                }
            }
        }
        let show_cursor = self.editor.read(cx).show_cursor(window, cx);
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
                for selection in prepaint.selections.drain(..) {
                    window.paint_quad(selection);
                }
                // run 背景（搜索高亮、语法背景等）：gpui 原生 paint_background
                // 用 decoration_runs 精确绘制，覆盖在选区之上、文本之下（对齐 Zed）。
                for line in &prepaint.layout.lines {
                    if let Err(error) = line.shaped.paint_background(
                        line.origin,
                        prepaint.layout.line_height,
                        window,
                        cx,
                    ) {
                        eprintln!("Editor 行背景绘制失败：{error}");
                    }
                }
                for line in &prepaint.layout.lines {
                    if let Err(error) =
                        line.shaped
                            .paint(line.origin, prepaint.layout.line_height, window, cx)
                    {
                        // 单个字形绘制失败只跳过该行，不能让整个窗口崩溃（对齐 Zed 的降级策略）。
                        eprintln!("Editor 文本行绘制失败：{error}");
                        continue;
                    }
                }
                if show_cursor {
                    for caret in prepaint.carets.drain(..) {
                        window.paint_quad(caret);
                    }
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
    // placeholder 模式：行数据源替换为 placeholder 快照（折行/行高与真实文本同一管线，
    // 对齐 Zed 的行层替换）；无高亮/折叠的查询对 placeholder 快照自然返回空。
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
    let mut gutter_rows = Vec::with_capacity(end.saturating_sub(start));
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
    // 搜索高亮：独立背景覆盖层（对齐 Zed 的 background highlights）。
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
        // placeholder 行用提示色（对齐 Zed element.rs 的 placeholder_color）。
        color: if placeholder_mode {
            color::current(cx).text_placeholder.into()
        } else {
            text_style.color
        },
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
                         fold_segments: Option<Vec<FoldRowSegment>>,
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
            fold_segments,
            git_diff,
            is_placeholder: placeholder_mode,
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
            match row.kind() {
                WrapViewportRowKind::Text { .. } => {
                    // 对齐 Zed highlighted_chunks：行解构、四层快照链穿透、chunk 合成与 run 映射都在管线侧完成，这里只消费渲染结果。
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
                        cx,
                    );
                    push_line(
                        row.index().get(),
                        rendered.logical_line,
                        rendered.gutter_line,
                        &rendered.display_text,
                        rendered.utf16_start,
                        rendered.wrap_info,
                        rendered.fold_segments,
                        rendered.runs,
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
    use crate::display_map::{DisplayMap, StyledLine};
    use gpui::{AppContext, Empty, TestAppContext};
    use std::path::PathBuf;
    use zcv_git::DiffHunk;
    use zcv_language::{LanguageBuffer, SyntaxSnapshot};
    use zcv_text::{Buffer, BufferConfig, ByteOffset, Line};

    /// 行级标记的显示行区间（`hunk_rendering` 的薄包装，测试专用）。
    fn diff_hunk_rows(
        snapshot: &DisplaySnapshot,
        hunks: &[DiffHunk],
        expanded_deleted: &[Range<usize>],
        expanded_modified: &[Range<usize>],
    ) -> Vec<(Range<usize>, DiffHunkKind)> {
        hunk_rendering(snapshot, hunks, expanded_deleted, expanded_modified).diff_rows
    }

    /// hunk 竖条范围与状态色（`hunk_rendering` 的薄包装，测试专用）。
    fn hunk_strip_rows(
        snapshot: &DisplaySnapshot,
        hunks: &[DiffHunk],
        expanded_deleted: &[Range<usize>],
        expanded_modified: &[Range<usize>],
    ) -> Vec<(Range<usize>, DiffHunkKind)> {
        hunk_rendering(snapshot, hunks, expanded_deleted, expanded_modified).strips
    }

    /// 可点击的 hunk 色带区域（`hunk_rendering` 的薄包装，测试专用）。
    fn hunk_hit_regions(
        snapshot: &DisplaySnapshot,
        hunks: &[DiffHunk],
        expanded_deleted: &[Range<usize>],
        expanded_modified: &[Range<usize>],
    ) -> Vec<(Range<usize>, Range<usize>, DiffHunkKind)> {
        hunk_rendering(snapshot, hunks, expanded_deleted, expanded_modified).hit_regions
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
        map.set_inserted(crate::display_map::InsertedLines::from([(
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
        map.set_inserted(crate::display_map::InsertedLines::from([(
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
