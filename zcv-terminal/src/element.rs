//! 终端渲染元素：每帧布局批处理 + 绘制。
//!
//! 渲染只读 `Terminal::last_content` 快照，不触碰模拟器锁。
//! 布局阶段把同一行的相邻同风格单元格合并为文本段（LineRun），背景色合并为色块（LayoutRect），绘制阶段逐段 shape + paint。

use std::collections::HashMap;
use std::sync::Arc;

use alacritty_terminal::vte::ansi::Color;
use gpui::{
    App, Bounds, ContentMask, Element, ElementId, Font, GlobalElementId, HighlightStyle,
    HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, Point, Rgba,
    ShapedLine, Style, TextRun, Window, px, relative, size,
};
use unicode_width::UnicodeWidthChar;
use zcv_theme::{color, typography};

use crate::mappings::mouse::{grid_point, grid_point_and_side};
use crate::{Cell, Content, IndexedCell, TerminalBounds, TerminalView, palette};
use alacritty_terminal::vte::ansi::CursorShape;

/// 同一行的渲染数据：文本段、起点与起始网格列。
struct LineRun {
    /// 段起始网格列：宽字符占 2 列，段起点可能跳过列。
    /// 渲染按列号 × 格宽定位（force_width 1 格 + 列号间距）。
    start_column: usize,
    origin: Point<Pixels>,
    text: String,
    runs: Vec<TextRun>,
}

/// 背景色块（含选择高亮）。
struct LayoutRect {
    bounds: Bounds<Pixels>,
    color: Rgba,
}

/// 光标渲染数据。
struct CursorLayout {
    bounds: Bounds<Pixels>,
    color: Rgba,
    /// Block 光标格内绘制的字符与其颜色。
    text: Option<(String, Rgba)>,
}

#[derive(Debug, Clone, PartialEq)]
struct TerminalStyleSpan {
    range: std::ops::Range<usize>,
    style: HighlightStyle,
}

pub(super) struct TerminalLayout {
    origin: Point<Pixels>,
    line_height: Pixels,
    cell_width: Pixels,
    font: Font,
    font_size: Pixels,
    /// 渲染快照的显示偏移与行列数（鼠标坐标换算用）。
    display_offset: usize,
    screen_lines: usize,
    columns: usize,
    text_runs: Vec<LineRun>,
    rects: Vec<LayoutRect>,
    cursor: Option<CursorLayout>,
    ime_marked_text: Option<String>,
    background: Rgba,
    hitbox: gpui::Hitbox,
}

pub(super) struct TerminalElement {
    view: gpui::Entity<TerminalView>,
    /// 文本段 shape 缓存：滚动等重绘帧直接命中，避免每帧全量字体 shaping。
    /// 键为文本与样式投影（TextRun 本身无 Hash）；
    /// 容量超限时整体清空（视口内行数有限，重建成本低，滚动场景命中率不受影响）。
    shaped_runs: HashMap<ShapedRunKey, ShapedLine>,
    /// 行转换缓存：内容指纹 → 转换结果（文本/样式段/背景区间）。
    /// 滚动只移动视口，行内容不变则指纹不变直接命中，跳过每格的主题颜色解析。
    row_cache: HashMap<u64, CachedRow>,
}

/// 一行网格的转换结果：文本、行内样式段（含起始网格列）与非默认背景区间。
struct StyledRow {
    text: String,
    spans: Arc<[TerminalStyleSpan]>,
    /// 每个样式段起始的网格列，与 spans 一一对应（渲染层按列号定位）。
    span_columns: Arc<[usize]>,
    bg_ranges: Vec<(usize, usize, u32)>,
}

/// 行转换结果：背景区间供绘制层逐帧像素化（选择高亮另行叠加）。
struct CachedRow {
    text: String,
    spans: Arc<[TerminalStyleSpan]>,
    span_columns: Arc<[usize]>,
    /// 需要绘制背景块的列区间（闭区间，含颜色）。
    bg_ranges: Vec<(usize, usize, u32)>,
}

/// 行缓存容量上限；超出即清空（终端历史行数有限，滚动窗口内命中率不受影响）。
const ROW_CACHE_LIMIT: usize = 4096;

/// shape 缓存键：行文本 + 样式段投影（font 省略——同一布局内恒同）。
type ShapedRunKey = (String, Vec<RunStyle>);

/// TextRun 的可哈希样式投影。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct RunStyle {
    len: usize,
    color: u32,
    background_color: Option<u32>,
    underline: Option<gpui::UnderlineStyle>,
    strikethrough: Option<gpui::StrikethroughStyle>,
}

impl From<&TextRun> for RunStyle {
    fn from(run: &TextRun) -> Self {
        Self {
            len: run.len,
            color: u32::from(Rgba::from(run.color)),
            background_color: run
                .background_color
                .map(|color| u32::from(Rgba::from(color))),
            underline: run.underline,
            strikethrough: run.strikethrough,
        }
    }
}

/// shape 缓存容量上限；超出即清空，避免终端输出流无限增长。
const SHAPED_RUN_CACHE_LIMIT: usize = 4096;

impl TerminalElement {
    pub(super) fn new(view: gpui::Entity<TerminalView>) -> Self {
        TerminalElement {
            view,
            shaped_runs: HashMap::new(),
            row_cache: HashMap::new(),
        }
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = TerminalLayout;

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
        // 尺寸度量：格宽取 'm' 字形宽度，行高取字体大小 × 行高倍率。
        let font = typography::content_font();
        let font_size = self.view.read(cx).font_size(cx);
        let cell_width = window
            .text_system()
            .shape_line(
                "m".into(),
                font_size,
                &[TextRun {
                    len: 1,
                    font: font.clone(),
                    color: Hsla::white(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            )
            .width
            .max(Pixels::from(1.));
        let line_height = self.view.read(cx).line_height(cx);

        // 通知终端新尺寸并排空事件队列，刷新渲染快照。
        let terminal_bounds = TerminalBounds::new(cell_width, line_height, bounds.size);
        self.view.update(cx, |view, cx| {
            view.terminal
                .update(cx, |terminal, cx| terminal.set_size(terminal_bounds, cx));
            view.sync(window, cx);
        });

        let content = self.view.read(cx).terminal.read(cx).last_content().cloned();
        let focused = self.view.read(cx).is_focused(window);
        let ime_marked_text = self.view.read(cx).marked_text().map(str::to_owned);
        let (display_offset, screen_lines, columns) =
            content.as_ref().map_or((0, 1, 2), |content| {
                (
                    content.display_offset,
                    content.screen_lines,
                    content.columns,
                )
            });
        let mut layout = TerminalLayout {
            origin: bounds.origin,
            line_height,
            cell_width,
            font,
            font_size,
            display_offset,
            screen_lines,
            columns,
            text_runs: Vec::new(),
            rects: Vec::new(),
            cursor: None,
            ime_marked_text,
            background: color::current(cx).editor_background,
            hitbox: window.insert_hitbox(bounds, HitboxBehavior::Normal),
        };

        if let Some(content) = content {
            let ime_marked_text = layout.ime_marked_text.clone();
            layout_grid(
                &content,
                &mut layout,
                &mut self.row_cache,
                ime_marked_text.as_deref(),
                window,
                cx,
            );
            let show_cursor = self.view.read(cx).should_show_cursor(focused, cx);
            layout.cursor = layout_cursor(&content, &layout, window, cx, show_cursor);
            // IME 候选窗位置：光标像素 bounds 独立于闪烁可见性计算。
            if let Some(bounds) = cursor_pixel_bounds(&content, &layout) {
                self.view.update(cx, |view, _| {
                    view.set_ime_cursor_bounds(bounds);
                });
            }
        }
        layout
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        layout: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // 内容整体对齐到 device pixel，避免字形在帧间抖动。
        let scale_factor = window.scale_factor();
        let snap =
            |value: Pixels| Pixels::from((f32::from(value) * scale_factor).floor() / scale_factor);
        let offset = Point::new(
            snap(layout.origin.x) - layout.origin.x,
            snap(layout.origin.y) - layout.origin.y,
        );

        // 输入法组合预览：在光标处绘制 marked 文本（下划线 + 选区背景）。
        let ime_marked_text = layout.ime_marked_text.as_deref();
        let ime_cursor_bounds = layout.cursor.as_ref().map(|cursor| cursor.bounds);

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            window.paint_quad(gpui::fill(bounds, layout.background));
            for rect in &layout.rects {
                window.paint_quad(gpui::fill(shifted(rect.bounds, offset), rect.color));
            }
            for run in &layout.text_runs {
                // 滚动等重绘帧行内容不变，shape 结果直接命中缓存。
                let key = (
                    run.text.clone(),
                    run.runs.iter().map(RunStyle::from).collect(),
                );
                let shaped = self.shaped_runs.entry(key).or_insert_with(|| {
                    // 强制每字形 1 格宽：CJK 字形 advance 与格宽一致，
                    // 宽字符占 2 格的间距由 run 的起始列号定位补足。
                    window.text_system().shape_line(
                        run.text.clone().into(),
                        layout.font_size,
                        &run.runs,
                        Some(layout.cell_width),
                    )
                });
                let origin = Point::new(
                    run.origin.x
                        + Pixels::from(run.start_column as f32 * f32::from(layout.cell_width)),
                    run.origin.y,
                );
                if let Err(error) = shaped.paint(origin + offset, layout.line_height, window, cx) {
                    eprintln!("终端文本绘制失败：{error}");
                }
            }
            if self.shaped_runs.len() > SHAPED_RUN_CACHE_LIMIT {
                self.shaped_runs.clear();
            }
            if let Some(marked) = ime_marked_text
                && let Some(cursor_bounds) = &ime_cursor_bounds
            {
                let run = TextRun {
                    len: marked.len(),
                    font: layout.font.clone(),
                    color: color::current(cx).text.into(),
                    background_color: Some(color::current(cx).editor_selection_background.into()),
                    underline: Some(gpui::UnderlineStyle {
                        thickness: px(1.),
                        color: None,
                        wavy: false,
                    }),
                    strikethrough: None,
                };
                let shaped = window.text_system().shape_line(
                    marked.to_owned().into(),
                    layout.font_size,
                    &[run],
                    None,
                );
                if let Err(error) = shaped.paint(
                    shifted(*cursor_bounds, offset).origin,
                    layout.line_height,
                    window,
                    cx,
                ) {
                    eprintln!("终端组合文本绘制失败：{error}");
                }
            }
            // 组合输入（marked 文本非空）时隐藏光标：Block 光标会盖住 marked 首字符。
            if ime_marked_text.is_none()
                && let Some(cursor) = &layout.cursor
            {
                window.paint_quad(gpui::fill(shifted(cursor.bounds, offset), cursor.color));
                if let Some((text, color)) = &cursor.text {
                    let run = TextRun {
                        len: text.len(),
                        font: layout.font.clone(),
                        color: (*color).into(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let shaped = window.text_system().shape_line(
                        text.clone().into(),
                        layout.font_size,
                        &[run],
                        None,
                    );
                    if let Err(error) =
                        shaped.paint(cursor.bounds.origin, layout.line_height, window, cx)
                    {
                        eprintln!("终端光标字符绘制失败：{error}");
                    }
                }
            }
        });
        register_mouse_listeners(layout, self.view.clone(), window, cx);

        // 注册 IME 输入处理器（中文输入法等组合输入）。
        let focus = self.view.read(cx).focus_handle();
        window.handle_input(
            &focus,
            gpui::ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );

        window.set_cursor_style(gpui::CursorStyle::IBeam, &layout.hitbox);
    }
}

/// 注册滚轮与鼠标监听（每帧重新注册，闭包捕获当帧的坐标度量与快照状态）。
fn register_mouse_listeners(
    layout: &TerminalLayout,
    view: gpui::Entity<TerminalView>,
    window: &mut Window,
    _cx: &mut App,
) {
    let hitbox = layout.hitbox.clone();
    let origin = layout.origin;
    let cell_width = layout.cell_width;
    let line_height = layout.line_height;
    let display_offset = layout.display_offset;
    let screen_lines = layout.screen_lines;
    let columns = layout.columns;

    let wheel_hitbox = hitbox.clone();
    let wheel_view = view.clone();
    window.on_mouse_event(move |event: &gpui::ScrollWheelEvent, phase, window, cx| {
        if phase != gpui::DispatchPhase::Bubble || !wheel_hitbox.is_hovered(window) {
            return;
        }
        let delta = event.delta.pixel_delta(line_height);
        let scroll_lines = (f32::from(delta.y) / f32::from(line_height)).trunc() as i32;
        // 慢滚（单事件增量小于行高）不提前丢弃：像素累积在滚动状态机内跨事件进行。
        let point = grid_point(
            event.position,
            origin,
            cell_width,
            line_height,
            display_offset,
            screen_lines,
            columns,
        );
        wheel_view.update(cx, |view, cx| {
            view.handle_scroll_wheel(event, Some(point), scroll_lines, window, cx);
        });
    });

    let down_hitbox = hitbox.clone();
    let down_view = view.clone();
    window.on_mouse_event(move |event: &gpui::MouseDownEvent, phase, window, cx| {
        if phase != gpui::DispatchPhase::Bubble || !down_hitbox.is_hovered(window) {
            return;
        }
        // 点击聚焦终端：光标显隐与键盘输入都绑定焦点。
        let focus = down_view.read(cx).focus_handle();
        window.focus(&focus);
        let (point, side) = grid_point_and_side(
            event.position,
            origin,
            cell_width,
            line_height,
            display_offset,
            screen_lines,
            columns,
        );
        down_view.update(cx, |view, cx| {
            view.handle_mouse_down(event, point, side, window, cx);
        });
    });

    let move_hitbox = hitbox.clone();
    let move_view = view.clone();
    window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, window, cx| {
        // 指针手势由视图拥有时，鼠标移出终端视口仍须由原视图完成。
        if phase != gpui::DispatchPhase::Bubble
            || (!move_hitbox.is_hovered(window) && !move_view.read(cx).owns_pointer_gesture())
        {
            return;
        }
        let (point, side) = grid_point_and_side(
            event.position,
            origin,
            cell_width,
            line_height,
            display_offset,
            screen_lines,
            columns,
        );
        // 拖拽选择时鼠标在视口边缘外：按距离缩放的量滚动视口（正 = 回看历史）。
        let autoscroll = if event.dragging() {
            drag_autoscroll_delta(event.position, origin, line_height, screen_lines)
        } else {
            Pixels::ZERO
        };
        move_view.update(cx, |view, cx| {
            view.handle_mouse_move(event, point, side, autoscroll, cx);
        });
    });

    let up_view = view;
    window.on_mouse_event(move |event: &gpui::MouseUpEvent, phase, _window, cx| {
        // 释放事件必须交给拥有按下手势的视图，即使指针已在视口外。
        if phase != gpui::DispatchPhase::Bubble {
            return;
        }
        let point = grid_point(
            event.position,
            origin,
            cell_width,
            line_height,
            display_offset,
            screen_lines,
            columns,
        );
        up_view.update(cx, |view, cx| {
            view.handle_mouse_up(event, point, cx);
        });
    });
}

/// 逐行批处理：合并相邻同风格单元格为文本段，收集背景色块与选择高亮。
fn layout_grid(
    content: &Content,
    layout: &mut TerminalLayout,
    row_cache: &mut HashMap<u64, CachedRow>,
    ime_marked_text: Option<&str>,
    window: &mut Window,
    cx: &mut App,
) {
    let TerminalLayout {
        origin,
        line_height,
        cell_width,
        font,
        text_runs,
        rects,
        ..
    } = layout;
    let display_offset = content.display_offset;
    let screen_lines = content.screen_lines;
    let selection = content.selection;
    let selection_color = color::current(cx).editor_selection_background;
    let ime_shift_columns = ime_marked_text.map(ime_text_width).unwrap_or(0);

    // 快照按行序排列，按行分组处理。
    let mut line_start = 0;
    while line_start < content.cells.len() {
        let line = content.cells[line_start].point.line;
        let mut line_end = line_start;
        while line_end < content.cells.len() && content.cells[line_end].point.line == line {
            line_end += 1;
        }
        // 视口行号：视口顶部绝对坐标为 -display_offset。
        let row = (line + display_offset as i32).clamp(0, screen_lines as i32 - 1) as usize;
        let row_origin = Point::new(origin.x, origin.y + *line_height * row as f32);
        let cells = &content.cells[line_start..line_end];

        let parts = ime_row_parts(
            cells,
            row as i32,
            content.cursor.point.line,
            content.cursor.point.column,
            ime_shift_columns,
        );

        for (part_cells, column_shift) in parts.into_iter().flatten() {
            // 行转换：内容指纹作缓存键——滚动只移动视口，行内容不变则直接命中，
            // 跳过每格主题颜色解析（滚动帧的 palette 开销降为 0）。
            let fingerprint = row_fingerprint(part_cells);
            let (text, spans, span_columns, bg_ranges) = {
                let entry = row_cache.entry(fingerprint).or_insert_with(|| {
                    let row = row_to_styled_line(part_cells, window, cx);
                    CachedRow {
                        text: row.text,
                        spans: row.spans,
                        span_columns: row.span_columns,
                        bg_ranges: row.bg_ranges,
                    }
                });
                (
                    entry.text.clone(),
                    entry.spans.clone(),
                    entry.span_columns.clone(),
                    entry.bg_ranges.clone(),
                )
            };
            if row_cache.len() > ROW_CACHE_LIMIT {
                row_cache.clear();
            }

            // 背景块：组合文本插入的空隙不能覆盖原有背景，因此后半行整体平移。
            for &(start, end, color) in &bg_ranges {
                push_rect(
                    rects,
                    start + column_shift,
                    end + column_shift,
                    row_origin,
                    *cell_width,
                    *line_height,
                    gpui::rgba(color),
                );
            }
            if let Some(sel) = selection {
                let mut sel_start: Option<usize> = None;
                for indexed in part_cells {
                    let column = indexed.point.column + column_shift;
                    let in_sel = sel.contains(indexed.point);
                    match (in_sel, sel_start) {
                        (true, None) => sel_start = Some(column),
                        (true, Some(_)) => {}
                        (false, Some(start)) => {
                            push_rect(
                                rects,
                                start,
                                column - 1,
                                row_origin,
                                *cell_width,
                                *line_height,
                                selection_color,
                            );
                            sel_start = None;
                        }
                        (false, None) => {}
                    }
                }
                if let Some(start) = sel_start {
                    push_rect(
                        rects,
                        start,
                        last_column(part_cells) + column_shift,
                        row_origin,
                        *cell_width,
                        *line_height,
                        selection_color,
                    );
                }
            }

            // 文本：终端网格已完成 tab 展开，按样式段直接产出 runs。
            // 宽字符占 2 列，段起点按列号 × 格宽定位（force_width 强制单字符 1 格，2 格间距由列号补足）。
            for (span, &start_column) in spans.iter().zip(span_columns.iter()) {
                let segment = &text[span.range.clone()];
                if segment.is_empty() {
                    continue;
                }
                let run = styled_text_run(
                    TextRun {
                        len: segment.len(),
                        font: font.clone(),
                        color: color::current(cx).text.into(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    },
                    span.style,
                );
                text_runs.push(LineRun {
                    start_column: start_column + column_shift,
                    origin: row_origin,
                    text: segment.to_string(),
                    runs: vec![run],
                });
            }
        }

        line_start = line_end;
    }
}

fn styled_text_run(mut run: TextRun, style: HighlightStyle) -> TextRun {
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
    run
}

/// 行的内容指纹：单元格列号、字符、颜色与标志的混合（不查主题，仅用于缓存失效判断）。
fn row_fingerprint(cells: &[IndexedCell]) -> u64 {
    let mut fp = 0u64;
    for indexed in cells {
        let cell = &indexed.cell;
        fp = fp.rotate_left(7)
            ^ (indexed.point.column as u64).rotate_left(3)
            ^ cell.character() as u64
            ^ color_fingerprint(&cell.foreground()).rotate_left(13)
            ^ color_fingerprint(&cell.background()).rotate_left(29)
            ^ cell_style_flags(cell).rotate_left(41);
    }
    fp
}

/// 单元格样式标志的稳定组合（指纹用；与渲染样式同源）。
fn cell_style_flags(cell: &Cell) -> u64 {
    (cell.is_bold() as u64)
        | (cell.is_italic() as u64) << 1
        | (cell.has_underline() as u64) << 2
        | (cell.has_strikeout() as u64) << 3
        | (cell.is_inverse() as u64) << 4
        | (cell.is_dim() as u64) << 5
        | (cell.is_wide_char() as u64) << 6
}

/// 颜色的稳定标识（不依赖主题解析，仅用于指纹）。
fn color_fingerprint(color: &Color) -> u64 {
    match color {
        Color::Named(named) => 1 << 8 | palette::named_index(*named) as u64,
        Color::Indexed(index) => 2 << 8 | *index as u64,
        Color::Spec(rgb) => 3 << 8 | (rgb.r as u64) << 16 | (rgb.g as u64) << 8 | rgb.b as u64,
    }
}

fn ime_text_width(text: &str) -> usize {
    text.chars()
        .map(|character| UnicodeWidthChar::width(character).unwrap_or(0))
        .sum()
}

fn ime_row_parts(
    cells: &[IndexedCell],
    row: i32,
    cursor_row: i32,
    cursor_column: usize,
    shift_columns: usize,
) -> [Option<(&[IndexedCell], usize)>; 2] {
    if shift_columns == 0 || row != cursor_row {
        return [Some((cells, 0)), None];
    }

    let split_at = cells.partition_point(|indexed| indexed.point.column < cursor_column);
    let mut parts = [None, None];
    if split_at > 0 {
        parts[0] = Some((&cells[..split_at], 0));
    }
    if split_at < cells.len() {
        parts[1] = Some((&cells[split_at..], shift_columns));
    }
    parts
}

/// 一行网格单元格 → 行文本 + 行内样式段 + 非默认背景区间。
/// 样式段按同样式连续格合并，尾随空格裁剪；背景区间按同色连续列合并。
fn row_to_styled_line(cells: &[IndexedCell], window: &mut Window, cx: &mut App) -> StyledRow {
    let mut text = String::with_capacity(cells.len());
    let mut spans: Vec<TerminalStyleSpan> = Vec::new();
    let mut span_columns: Vec<usize> = Vec::new();
    let mut span_start = 0usize;
    let mut span_style: Option<HighlightStyle> = None;
    let mut last_column: Option<usize> = None;
    let mut content_end = 0usize;
    let default_background = color::current(cx).editor_background;
    let mut bg_ranges: Vec<(usize, usize, u32)> = Vec::new();
    let mut bg_start: Option<(usize, u32)> = None;

    for indexed in cells {
        let column = indexed.point.column;
        let cell = &indexed.cell;
        // 宽字符占位格不产生文本：宽字符自身占两格宽；
        // 背景仍按格绘制（宽字符第二格背景不丢失）。
        if !cell.is_wide_char_spacer() {
            let ch = cell.character();
            let start = text.len();
            text.push(ch);
            if let Some(zerowidth) = cell.zerowidth() {
                for zch in zerowidth {
                    text.push(*zch);
                }
            }
            let end = text.len();
            if ch != ' ' {
                content_end = end;
            }
            let style = cell_highlight_style(cell, window, cx);
            // 样式变化或网格列不连续（宽字符跨 2 列）时切段；
            // 段起点列号供渲染层按列定位（force_width 1 格 + 列号间距）。
            let col_contiguous = last_column.is_none_or(|last| column == last + 1);
            match span_style {
                Some(prev) if prev == style && col_contiguous => {}
                Some(prev) => {
                    spans.push(TerminalStyleSpan {
                        range: span_start..start,
                        style: prev,
                    });
                    span_start = start;
                    span_columns.push(column);
                    span_style = Some(style);
                }
                None => {
                    span_start = start;
                    span_columns.push(column);
                    span_style = Some(style);
                }
            }
            last_column = Some(column);
        }
        // 背景：非默认背景画块（同色连续列合并）。
        let bg = background_for(cell, window, cx);
        let needs_rect = bg != default_background;
        let bg_color = u32::from(bg);
        match (needs_rect, bg_start) {
            (true, None) => bg_start = Some((column, bg_color)),
            (true, Some((_, color))) if color == bg_color => {}
            (true, Some((bg_col, color))) => {
                bg_ranges.push((bg_col, column - 1, color));
                bg_start = Some((column, bg_color));
            }
            (false, Some((bg_col, color))) => {
                bg_ranges.push((bg_col, column - 1, color));
                bg_start = None;
            }
            (false, None) => {}
        }
    }
    if let Some(style) = span_style {
        spans.push(TerminalStyleSpan {
            range: span_start..text.len(),
            style,
        });
    }
    if let Some((bg_col, color)) = bg_start {
        bg_ranges.push((bg_col, cells.len() - 1, color));
    }
    // 裁剪尾随空格：终端行常以空格 padding 结尾，不参与 shaping。
    text.truncate(content_end);
    let mut pairs: Vec<(TerminalStyleSpan, usize)> = spans.into_iter().zip(span_columns).collect();
    pairs.retain(|(span, _)| span.range.start < content_end);
    for (span, _) in &mut pairs {
        span.range.end = span.range.end.min(content_end);
    }
    StyledRow {
        text,
        spans: Arc::from(
            pairs
                .iter()
                .map(|(span, _)| span.clone())
                .collect::<Vec<_>>(),
        ),
        span_columns: Arc::from(pairs.iter().map(|(_, column)| *column).collect::<Vec<_>>()),
        bg_ranges,
    }
}

/// 单元格 → 行内样式：逆显交换前景/背景，dim 降亮度，粗体/斜体/下划线/删除线映射。
fn cell_highlight_style(cell: &Cell, window: &mut Window, cx: &mut App) -> HighlightStyle {
    let mut fg = if cell.is_inverse() {
        palette::color_to_rgba(&cell.background(), window, cx)
    } else {
        palette::color_to_rgba(&cell.foreground(), window, cx)
    };
    if cell.is_dim() {
        fg.a *= 0.5;
    }
    let bg = if cell.is_inverse() {
        palette::color_to_rgba(&cell.foreground(), window, cx)
    } else {
        palette::color_to_rgba(&cell.background(), window, cx)
    };
    HighlightStyle {
        color: Some(fg.into()),
        background_color: Some(bg.into()),
        font_weight: cell.is_bold().then_some(gpui::FontWeight::BOLD),
        font_style: cell.is_italic().then_some(gpui::FontStyle::Italic),
        underline: cell.has_underline().then_some(gpui::UnderlineStyle {
            thickness: px(1.),
            color: Some(fg.into()),
            wavy: false,
        }),
        strikethrough: cell.has_strikeout().then_some(gpui::StrikethroughStyle {
            thickness: px(1.),
            color: Some(fg.into()),
        }),
        ..Default::default()
    }
}

/// 平移矩形（Bounds 无 translate 方法，手动构造）。
fn shifted(bounds: Bounds<Pixels>, offset: Point<Pixels>) -> Bounds<Pixels> {
    Bounds::new(bounds.origin + offset, bounds.size)
}

/// 收束背景块为矩形（同色连续列）。
fn push_rect(
    rects: &mut Vec<LayoutRect>,
    start: usize,
    end: usize,
    origin: Point<Pixels>,
    cell_width: Pixels,
    line_height: Pixels,
    color: Rgba,
) {
    if end < start {
        return;
    }
    let width = (end + 1 - start) as f32 * f32::from(cell_width);
    let rect_origin = Point::new(origin.x + start as f32 * cell_width, origin.y);
    rects.push(LayoutRect {
        bounds: Bounds::new(rect_origin, size(Pixels::from(width), line_height)),
        color,
    });
}

/// 光标视口行：快照中的光标行已是视口相对坐标（构建时已换算滚动偏移）。
/// 光标滚出视口时返回 None（不绘制，视口外光标隐藏而非钳制悬浮）。
fn cursor_row(content: &Content) -> Option<usize> {
    let row = content.cursor.point.line;
    (row >= 0 && row < content.screen_lines as i32).then_some(row as usize)
}

/// 光标格的元素内坐标（不含 layout.origin 偏移），行越界返回 None。
fn cursor_cell_origin(content: &Content, layout: &TerminalLayout) -> Option<Point<Pixels>> {
    let row = cursor_row(content)?;
    let col = content
        .cursor
        .point
        .column
        .min(content.columns.saturating_sub(1));
    Some(Point::new(
        col as f32 * layout.cell_width,
        row as f32 * layout.line_height,
    ))
}

/// 光标格的像素 bounds（元素相对坐标），独立于可见性（IME 候选窗定位用）。
/// 注意：layout.origin 已含元素在窗口中的偏移，这里只算元素内相对位置，窗口坐标由 `bounds_for_range` 加上元素偏移得出。
fn cursor_pixel_bounds(content: &Content, layout: &TerminalLayout) -> Option<Bounds<Pixels>> {
    let origin = cursor_cell_origin(content, layout)?;
    Some(Bounds::new(
        origin,
        size(layout.cell_width, layout.line_height),
    ))
}

/// 光标渲染：Block 整格 + 字符；Beam/Underline 细条；隐藏模式省略。
fn layout_cursor(
    content: &Content,
    layout: &TerminalLayout,
    window: &mut Window,
    cx: &mut App,
    show_cursor: bool,
) -> Option<CursorLayout> {
    let cursor = &content.cursor;
    if !show_cursor {
        return None;
    }
    let cell_origin = cursor_cell_origin(content, layout)?;
    let origin = Point::new(
        layout.origin.x + cell_origin.x,
        layout.origin.y + cell_origin.y,
    );
    let color = color::current(cx).editor_cursor;
    match cursor.shape {
        CursorShape::Block => {
            // 光标宽度取光标字符的 shaped 宽度（至少 1 格）：宽字符（中文等）光标
            // 覆盖整个字符，避免只盖半格造成错位感。
            let cursor_char = content.cursor_cell.character();
            let cursor_width = {
                // 宽字符（CJK 等）光标直接取 2 格：字形 advance 按 1 格计算，
                // 但网格占 2 格，光标必须覆盖完整字符（与行文本按列号定位一致）。
                if content.cursor_cell.is_wide_char() {
                    layout.cell_width * 2.0
                } else {
                    let shaped = window.text_system().shape_line(
                        cursor_char.to_string().into(),
                        layout.font_size,
                        &[TextRun {
                            len: cursor_char.len_utf8(),
                            font: layout.font.clone(),
                            color: color::current(cx).text.into(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        }],
                        None,
                    );
                    shaped.width.max(layout.cell_width)
                }
            };
            let bounds = Bounds::new(origin, size(cursor_width, layout.line_height));
            // 光标格内字符用终端背景色绘制，保证对比。
            let text = (cursor_char != ' ').then(|| (cursor_char.to_string(), layout.background));
            Some(CursorLayout {
                bounds,
                color,
                text,
            })
        }
        CursorShape::Beam => {
            let bounds = Bounds::new(origin, size(Pixels::from(2.), layout.line_height));
            Some(CursorLayout {
                bounds,
                color,
                text: None,
            })
        }
        CursorShape::Underline => {
            let bounds = Bounds::new(origin, size(layout.cell_width, Pixels::from(2.)));
            Some(CursorLayout {
                bounds,
                color,
                text: None,
            })
        }
        CursorShape::HollowBlock => {
            let bounds = Bounds::new(origin, size(layout.cell_width, layout.line_height));
            Some(CursorLayout {
                bounds,
                color,
                text: None,
            })
        }
        CursorShape::Hidden => None,
    }
}

/// 单元格背景色：逆显时交换为前景色。
fn background_for(cell: &Cell, window: &mut Window, cx: &mut App) -> Rgba {
    if cell.is_inverse() {
        palette::color_to_rgba(&cell.foreground(), window, cx)
    } else {
        palette::color_to_rgba(&cell.background(), window, cx)
    }
}

/// 行内最后一列（背景块收束用）。
fn last_column(cells: &[IndexedCell]) -> usize {
    cells.last().map(|cell| cell.point.column).unwrap_or(0)
}

/// 拖拽选择时的视口自动滚动量（像素，正 = 向上回看历史）：
/// 滚动量 = 超出视口边缘的距离 × 0.3，单事件上限视口高 1/16（与编辑器 `drag_autoscroll_delta` 同款）；
/// 滚动频率由 view 层限频（≈60Hz）。
fn drag_autoscroll_delta(
    position: Point<Pixels>,
    origin: Point<Pixels>,
    line_height: Pixels,
    screen_lines: usize,
) -> Pixels {
    let top = origin.y;
    let bottom = origin.y + line_height * screen_lines as f32;
    let margin = line_height.min((bottom - top) / 3.0);
    let max_delta = (bottom - top) / 16.0;
    if position.y < top + margin {
        ((top + margin - position.y) * 0.3).min(max_delta)
    } else if position.y > bottom - margin {
        -((position.y - (bottom - margin)) * 0.3).min(max_delta)
    } else {
        Pixels::ZERO
    }
}

#[cfg(test)]
mod cursor_tests {
    use super::*;
    use crate::{Cursor, Modes, Point, alacritty::AlacrittyCell};

    /// 光标行是视口相对行：滚动（display_offset 增大）时光标随内容上移，而不是停留在原屏幕位置。
    #[test]
    fn cursor_row_is_viewport_relative_and_clamped() {
        let content = |display_offset: usize, line: i32| Content {
            cells: Vec::new(),
            mode: Modes::NONE,
            total_lines: 100,
            display_offset,
            columns: 80,
            screen_lines: 30,
            selection_text: None,
            selection: None,
            cursor: Cursor {
                shape: CursorShape::Block,
                point: Point { line, column: 3 },
            },
            cursor_cell: Cell::new(AlacrittyCell {
                c: 'x',
                ..Default::default()
            }),
            terminal_bounds: TerminalBounds::default(),
            scrolled_to_top: false,
            scrolled_to_bottom: false,
            bottom_row_occupied: false,
        };
        // 滚动 10 行后视口行 5 的光标仍定位在第 5 行（若错误叠加偏移会得到 15）。
        assert_eq!(cursor_row(&content(10, 5)), Some(5));
        // 光标滚出视口（行号越界）时隐藏而非钳制悬浮。
        assert_eq!(cursor_row(&content(0, 50)), None);
        assert_eq!(cursor_row(&content(0, -3)), None);
    }
}

#[cfg(test)]
mod autoscroll_tests {
    use super::*;

    #[test]
    fn drag_autoscroll_only_scrolls_when_cursor_passes_viewport_edge() {
        let origin = Point::new(px(0.), px(0.));
        let line_height = px(20.);

        // 视口内：不滚动。
        assert_eq!(
            drag_autoscroll_delta(Point::new(px(100.), px(100.)), origin, line_height, 10),
            Pixels::ZERO
        );
        // 上边缘外：回看历史（正）。
        assert!(
            drag_autoscroll_delta(Point::new(px(100.), px(-100.)), origin, line_height, 10)
                > Pixels::ZERO
        );
        // 下边缘外：查看新内容（负）。
        // 超出 120 × 0.3 = 36，被单事件上限（视口高 200 / 16 = 12.5）钳制。
        let delta = drag_autoscroll_delta(Point::new(px(100.), px(300.)), origin, line_height, 10);
        assert_eq!(f32::from(delta), -12.5);
    }
}

#[cfg(test)]
mod ime_layout_tests {
    use super::*;
    use crate::{Point as TerminalPoint, alacritty::AlacrittyCell};

    fn cells(text: &str) -> Vec<IndexedCell> {
        text.chars()
            .enumerate()
            .map(|(column, character)| IndexedCell {
                point: TerminalPoint { line: 0, column },
                cell: Cell::new(AlacrittyCell {
                    c: character,
                    ..Default::default()
                }),
            })
            .collect()
    }

    #[test]
    fn ime_preview_inserts_columns_at_cursor() {
        let cells = cells("abcd");
        assert_eq!(
            ime_row_parts(&cells, 0, 0, 0, ime_text_width("kaifazhe"))
                .into_iter()
                .flatten()
                .map(|(cells, shift)| (cells.first().map(|cell| cell.point.column), shift))
                .collect::<Vec<_>>(),
            vec![(Some(0), 8)]
        );
        assert_eq!(
            ime_row_parts(&cells, 0, 0, 2, ime_text_width("kaifazhe"))
                .into_iter()
                .flatten()
                .map(|(cells, shift)| (cells.first().map(|cell| cell.point.column), shift))
                .collect::<Vec<_>>(),
            vec![(Some(0), 0), (Some(2), 8)]
        );
    }

    #[test]
    fn ime_preview_uses_terminal_width_for_wide_characters() {
        assert_eq!(ime_text_width("kaifazhe"), 8);
        assert_eq!(ime_text_width("中"), 2);
        assert_eq!(ime_text_width("e\u{301}"), 1);
    }
}

#[cfg(test)]
mod styled_line_tests {
    use super::*;
    use crate::{Point as TerminalPoint, alacritty::AlacrittyCell};
    use alacritty_terminal::{
        term::cell::Flags,
        vte::ansi::{Color, NamedColor, Rgb},
    };
    use gpui::{Context, div};

    #[derive(Default)]
    struct EmptyView;

    impl gpui::Render for EmptyView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    fn cell(ch: char, fg: Color, bg: Color) -> IndexedCell {
        IndexedCell {
            point: TerminalPoint { line: 0, column: 0 },
            cell: Cell::new(AlacrittyCell {
                c: ch,
                fg,
                bg,
                flags: Flags::empty(),
                ..Default::default()
            }),
        }
    }

    fn cell_at(ch: char, column: usize, fg: Color, bg: Color) -> IndexedCell {
        IndexedCell {
            point: TerminalPoint { line: 0, column },
            cell: Cell::new(AlacrittyCell {
                c: ch,
                fg,
                bg,
                flags: Flags::empty(),
                ..Default::default()
            }),
        }
    }

    #[test]
    fn terminal_style_maps_directly_to_text_run() {
        let foreground: Hsla = gpui::red();
        let background: Hsla = gpui::blue();
        let underline = gpui::UnderlineStyle {
            color: Some(foreground),
            thickness: px(1.),
            wavy: false,
        };
        let strikethrough = gpui::StrikethroughStyle {
            color: Some(foreground),
            thickness: px(1.),
        };
        let run = styled_text_run(
            TextRun {
                len: 3,
                font: gpui::font(".SystemUIFont"),
                color: Default::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            },
            HighlightStyle {
                color: Some(foreground),
                background_color: Some(background),
                font_weight: Some(gpui::FontWeight::BOLD),
                font_style: Some(gpui::FontStyle::Italic),
                underline: Some(underline),
                strikethrough: Some(strikethrough),
                ..Default::default()
            },
        );

        assert_eq!(run.len, 3);
        assert_eq!(run.color, foreground);
        assert_eq!(run.background_color, Some(background));
        assert_eq!(run.font.weight, gpui::FontWeight::BOLD);
        assert_eq!(run.font.style, gpui::FontStyle::Italic);
        assert_eq!(run.underline, Some(underline));
        assert_eq!(run.strikethrough, Some(strikethrough));
    }

    /// 宽字符渲染：force_width 强制每字形 1 格宽（CJK 字形 advance 恰好 1 格），宽字符占 2 格的间距由段起始列号 × 格宽定位补足（不补空格）。
    /// "中"（列 0）"文"（列 2）"a"（列 4）三段渲染总宽应等于 5 格。
    #[gpui::test]
    fn wide_char_force_width_aligns_render_width(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            zcv_assets::Assets.load_fonts(cx).expect("内置字体应能加载");
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            zcv_theme::ThemeChoice::System.apply(cx, Some(window));
            EmptyView
        });
        cx.update(|window, cx| {
            let font = typography::content_font();
            let font_size = typography::content_size();
            let run = |ch: char| TextRun {
                len: ch.len_utf8(),
                font: font.clone(),
                color: color::current(cx).text.into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let cell_width = window
                .text_system()
                .shape_line("m".into(), font_size, &[run('m')], None)
                .width;
            // 前提：CJK 字形 advance 恰好 1 格（0.6em = 格宽），宽字符第二格由列号定位承担。
            let zhong = window
                .text_system()
                .shape_line("中".into(), font_size, &[run('中')], None)
                .width;
            assert!(
                (f32::from(zhong) - f32::from(cell_width)).abs() < 0.1,
                "CJK 字形应恰好 1 格：实际 {zhong:?}，格宽 {cell_width:?}"
            );
            // force_width 下每字形 1 格："中文" 2 字形 = 2 格。
            let forced = window
                .text_system()
                .shape_line(
                    "中文".into(),
                    font_size,
                    &[run('中'), run('文')],
                    Some(cell_width),
                )
                .width;
            assert!(
                (f32::from(forced) - f32::from(cell_width) * 2.0).abs() < 0.1,
                "force_width 下 '中文' 应等于 2 格：实际 {forced:?}，期望 {:?}",
                cell_width * 2.0
            );
            // 列号定位：段起点 [0, 2, 4]，各段 1 格宽 → 渲染总宽 5 格（网格 中+文+a = 5 列）。
            let segments = ["中", "文", "a"]
                .iter()
                .map(|ch| {
                    window
                        .text_system()
                        .shape_line(
                            (*ch).into(),
                            font_size,
                            &[run(ch.chars().next().unwrap())],
                            Some(cell_width),
                        )
                        .width
                })
                .collect::<Vec<_>>();
            let starts = [0usize, 2, 4];
            let total = starts
                .iter()
                .zip(segments)
                .map(|(col, width)| *col as f32 * f32::from(cell_width) + f32::from(width))
                .fold(0.0f32, f32::max);
            assert!(
                (total - f32::from(cell_width) * 5.0).abs() < 0.1,
                "列号定位后宽字符行渲染宽度应等于 5 格：实际 {total:?}，期望 {:?}",
                cell_width * 5.0
            );
        });
    }

    /// 指纹对相同内容稳定、对内容变化敏感（行缓存失效判定的正确性）。
    #[test]
    fn row_fingerprint_is_stable_and_sensitive() {
        let red = Color::Spec(Rgb { r: 255, g: 0, b: 0 });
        let bg = Color::Named(NamedColor::Background);
        let same = || vec![cell('a', red, bg), cell('b', red, bg)];
        let changed = || vec![cell('a', red, bg), cell('c', red, bg)];
        assert_eq!(
            row_fingerprint(&same()),
            row_fingerprint(&same()),
            "相同内容指纹应一致"
        );
        assert_ne!(
            row_fingerprint(&same()),
            row_fingerprint(&changed()),
            "内容变化指纹应不同"
        );
    }

    /// 宽字符：占位格不产生文本；跨列切段并记录段起始列号（渲染按列定位），背景仍按格绘制。
    #[gpui::test]
    fn wide_char_spacer_skips_text_not_background(cx: &mut gpui::TestAppContext) {
        let (_, cx) = cx.add_window_view(|window, cx| {
            zcv_theme::ThemeChoice::System.apply(cx, Some(window));
            EmptyView
        });
        let red = Color::Spec(Rgb { r: 255, g: 0, b: 0 });
        let bg = Color::Named(NamedColor::Background);
        let wide = |ch: char, column: usize| IndexedCell {
            point: TerminalPoint { line: 0, column },
            cell: Cell::new(AlacrittyCell {
                c: ch,
                fg: red,
                bg,
                flags: Flags::WIDE_CHAR,
                ..Default::default()
            }),
        };
        let spacer = |column: usize| IndexedCell {
            point: TerminalPoint { line: 0, column },
            cell: Cell::new(AlacrittyCell {
                c: ' ',
                fg: red,
                bg,
                flags: Flags::WIDE_CHAR_SPACER,
                ..Default::default()
            }),
        };
        let row = cx.update(|window, cx| {
            let cells = vec![wide('中', 0), spacer(1), wide('文', 2), spacer(3)];
            row_to_styled_line(&cells, window, cx)
        });
        assert_eq!(row.text, "中文", "宽字符占位格不产生文本");
        assert_eq!(row.spans.len(), 2, "宽字符跨 2 列，相邻段按列切分");
        assert_eq!(row.spans[0].range, 0..3);
        assert_eq!(row.spans[1].range, 3..6);
        assert_eq!(
            row.span_columns.as_ref(),
            [0, 2],
            "段起始列号反映宽字符占 2 列（渲染按列号定位）"
        );
    }

    /// 相邻同样式格合并为一段；样式变化分段；尾随空格不参与 shaping。
    #[gpui::test]
    fn row_to_styled_line_merges_same_style_and_trims_trailing_spaces(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_, cx) = cx.add_window_view(|window, cx| {
            zcv_theme::ThemeChoice::System.apply(cx, Some(window));
            EmptyView
        });
        let red = Color::Spec(Rgb { r: 255, g: 0, b: 0 });
        let green = Color::Spec(Rgb { r: 0, g: 255, b: 0 });
        let bg = Color::Named(NamedColor::Background);
        let row = cx.update(|window, cx| {
            let cells = vec![
                cell_at('a', 0, red, bg),
                cell_at('b', 1, red, bg),
                cell_at('c', 2, green, bg),
                cell_at(' ', 3, red, bg),
            ];
            row_to_styled_line(&cells, window, cx)
        });
        assert_eq!(row.text, "abc", "尾随空格应被裁剪");
        assert_eq!(row.spans.len(), 2, "相邻同样式格应合并为一段");
        assert_eq!(row.spans[0].range, 0..2);
        assert_eq!(row.spans[1].range, 2..3);
        assert_eq!(row.span_columns.as_ref(), [0, 2], "段起始列号按格连续推进");
        // 逆显格交换前景与背景。
        let inverse_row = cx.update(|window, cx| {
            // 手工构造逆显格。
            let inverse = AlacrittyCell {
                c: 'y',
                fg: red,
                bg,
                flags: Flags::INVERSE,
                ..Default::default()
            };
            let cells = vec![
                cell('x', red, bg),
                IndexedCell {
                    point: TerminalPoint { line: 0, column: 1 },
                    cell: Cell::new(inverse),
                },
            ];
            row_to_styled_line(&cells, window, cx)
        });
        assert_eq!(inverse_row.spans.len(), 2, "逆显格样式不同应分段");
    }
}
