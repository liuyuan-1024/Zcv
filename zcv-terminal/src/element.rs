//! 终端渲染元素：每帧布局批处理 + 绘制。
//!
//! 渲染只读 `Terminal::last_content` 快照，不触碰模拟器锁。
//! 布局阶段把同一行的相邻同风格单元格合并为文本段（LineRun），背景色合并为色块（LayoutRect），绘制阶段逐段 shape + paint。

use gpui::{
    App, Bounds, ContentMask, Element, ElementId, Font, GlobalElementId, HitboxBehavior, Hsla,
    InspectorElementId, IntoElement, LayoutId, Pixels, Point, Rgba, Style, TextRun, Window, px,
    relative, size,
};
use zcv_theme::{color, typography};

use crate::{Cell, Content, CursorShape, IndexedCell, TerminalBounds, palette};

/// 同一行的渲染数据：文本段与起点。
struct LineRun {
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
    background: Rgba,
    hitbox: gpui::Hitbox,
}

pub(super) struct TerminalElement {
    view: gpui::Entity<crate::TerminalView>,
}

impl TerminalElement {
    pub(super) fn new(view: gpui::Entity<crate::TerminalView>) -> Self {
        TerminalElement { view }
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
        let font = typography::editor_font();
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
        let focused = self.view.read(cx).focused;
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
            background: color::current(cx).editor_background,
            hitbox: window.insert_hitbox(bounds, HitboxBehavior::Normal),
        };

        if let Some(content) = content {
            layout_grid(&content, &mut layout, window, cx);
            let show_cursor = self.view.read(cx).should_show_cursor(focused, cx);
            layout.cursor = layout_cursor(&content, &layout, window, cx, show_cursor);
            // IME 候选窗位置：光标像素 bounds 独立于闪烁可见性计算。
            self.view.update(cx, |view, _| {
                view.set_ime_cursor_bounds(cursor_pixel_bounds(&content, &layout));
            });
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
        // 内容整体对齐到 device pixel，避免字形在帧间抖动（对齐 Zed）。
        let scale_factor = window.scale_factor();
        let snap =
            |value: Pixels| Pixels::from((f32::from(value) * scale_factor).floor() / scale_factor);
        let offset = Point::new(
            snap(layout.origin.x) - layout.origin.x,
            snap(layout.origin.y) - layout.origin.y,
        );

        // 输入法组合预览：在光标处绘制 marked 文本（下划线 + 选区背景）。
        let ime_marked_text = self.view.read(cx).marked_text().map(str::to_string);
        let ime_cursor_bounds = layout.cursor.as_ref().map(|cursor| cursor.bounds);

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            window.paint_quad(gpui::fill(bounds, layout.background));
            for rect in &layout.rects {
                window.paint_quad(gpui::fill(shifted(rect.bounds, offset), rect.color));
            }
            for run in &layout.text_runs {
                let shaped = window.text_system().shape_line(
                    run.text.clone().into(),
                    layout.font_size,
                    &run.runs,
                    None,
                );
                if let Err(error) =
                    shaped.paint(run.origin + offset, layout.line_height, window, cx)
                {
                    log::error!("终端文本绘制失败：{error}");
                }
            }
            if let Some(marked) = &ime_marked_text
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
                    marked.clone().into(),
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
                    log::error!("终端组合文本绘制失败：{error}");
                }
            }
            if let Some(cursor) = &layout.cursor {
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
                        log::error!("终端光标字符绘制失败：{error}");
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
    view: gpui::Entity<crate::TerminalView>,
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
        if scroll_lines == 0 {
            return;
        }
        let point = crate::mappings::mouse::grid_point(
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
        let (point, side) = crate::mappings::mouse::grid_point_and_side(
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
        if phase != gpui::DispatchPhase::Bubble || !move_hitbox.is_hovered(window) {
            return;
        }
        let (point, side) = crate::mappings::mouse::grid_point_and_side(
            event.position,
            origin,
            cell_width,
            line_height,
            display_offset,
            screen_lines,
            columns,
        );
        move_view.update(cx, |view, cx| {
            view.handle_mouse_move(event, point, side, cx);
        });
    });

    let up_hitbox = hitbox.clone();
    let up_view = view;
    window.on_mouse_event(move |event: &gpui::MouseUpEvent, phase, window, cx| {
        if phase != gpui::DispatchPhase::Bubble || !up_hitbox.is_hovered(window) {
            return;
        }
        let point = crate::mappings::mouse::grid_point(
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
fn layout_grid(content: &Content, layout: &mut TerminalLayout, window: &mut Window, cx: &mut App) {
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
    let default_background = color::current(cx).editor_background;

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

        // 当前文本段：起始列、样式与段内文本；列不连续或样式变化时收束。
        let mut run_style: Option<(usize, CellStyle)> = None;
        let mut run_text = String::new();
        let mut rect_start: Option<(usize, Rgba)> = None;

        for indexed in cells {
            let point = indexed.point;
            let column = point.column;
            let cell = &indexed.cell;

            // 背景：默认背景不画；逆显、有底色或选中时画块（同色连续列合并）。
            let in_selection = selection.map(|s| s.contains(point)).unwrap_or(false);
            let bg = background_for(cell, window, cx);
            let needs_rect = in_selection || bg != default_background;
            let rect_color = if in_selection { selection_color } else { bg };
            match (needs_rect, rect_start.take()) {
                (true, None) => rect_start = Some((column, rect_color)),
                (true, Some((start, color))) if color == rect_color => {
                    rect_start = Some((start, color));
                }
                (true, Some((start, color))) => {
                    push_rect(
                        rects,
                        start,
                        column - 1,
                        row_origin,
                        *cell_width,
                        *line_height,
                        color,
                    );
                    rect_start = Some((column, rect_color));
                }
                (false, Some((start, color))) => {
                    push_rect(
                        rects,
                        start,
                        column - 1,
                        row_origin,
                        *cell_width,
                        *line_height,
                        color,
                    );
                }
                (false, None) => {}
            }

            // 文本：跳过空单元格与宽字符占位格（不产生字符，段被自然打断）。
            if cell.is_empty() || cell.is_wide_char_spacer() {
                push_run_segment(
                    text_runs,
                    &mut run_text,
                    run_style.take(),
                    row_origin,
                    *cell_width,
                    font,
                );
                continue;
            }
            let fg = foreground_for(cell, window, cx);
            let style = CellStyle {
                fg,
                bg,
                underline: cell.has_underline(),
                strikeout: cell.has_strikeout(),
                bold: cell.is_bold(),
                italic: cell.is_italic(),
            };
            match run_style {
                None => run_style = Some((column, style)),
                Some((run_start, current)) if run_start + 1 != column || current != style => {
                    push_run_segment(
                        text_runs,
                        &mut run_text,
                        Some((run_start, current)),
                        row_origin,
                        *cell_width,
                        font,
                    );
                    run_style = Some((column, style));
                }
                _ => {}
            }
            run_text.push(cell.character());
            if let Some(zerowidth) = cell.zerowidth() {
                for ch in zerowidth {
                    run_text.push(*ch);
                }
            }
        }

        // 行尾收束背景块与文本段。
        if let Some((start, color)) = rect_start.take() {
            push_rect(
                rects,
                start,
                last_column(cells),
                row_origin,
                *cell_width,
                *line_height,
                color,
            );
        }
        push_run_segment(
            text_runs,
            &mut run_text,
            run_style.take(),
            row_origin,
            *cell_width,
            font,
        );

        line_start = line_end;
    }
}

/// 平移矩形（Bounds 无 translate 方法，手动构造）。
fn shifted(bounds: Bounds<Pixels>, offset: Point<Pixels>) -> Bounds<Pixels> {
    Bounds::new(bounds.origin + offset, bounds.size)
}

/// 单元格样式：颜色、修饰与字重（粗体/斜体）。
#[derive(Clone, Copy, Debug, PartialEq)]
struct CellStyle {
    fg: Rgba,
    bg: Rgba,
    underline: bool,
    strikeout: bool,
    bold: bool,
    italic: bool,
}

/// 收束当前文本段为独立 LineRun：起点按段首列定位，
/// 段间的空格/宽字符占位格不并入文本（空格列由位置保留视觉间距）。
fn push_run_segment(
    text_runs: &mut Vec<LineRun>,
    run_text: &mut String,
    segment: Option<(usize, CellStyle)>,
    row_origin: Point<Pixels>,
    cell_width: Pixels,
    font: &Font,
) {
    let Some((start_column, style)) = segment else {
        return;
    };
    if run_text.is_empty() {
        return;
    }
    let text = std::mem::take(run_text);
    let runs = vec![make_run(style, text.len(), font)];
    text_runs.push(LineRun {
        origin: Point::new(
            row_origin.x + start_column as f32 * cell_width,
            row_origin.y,
        ),
        text,
        runs,
    });
}

/// 构造文本段 TextRun（len 为段内 UTF-8 字节数；粗体/斜体按单元格样式）。
fn make_run(style: CellStyle, len: usize, font: &Font) -> TextRun {
    let CellStyle {
        fg,
        bg,
        underline,
        strikeout,
        bold,
        italic,
    } = style;
    TextRun {
        len,
        font: Font {
            weight: if bold {
                gpui::FontWeight::BOLD
            } else {
                font.weight
            },
            style: if italic {
                gpui::FontStyle::Italic
            } else {
                font.style
            },
            ..font.clone()
        },
        color: fg.into(),
        background_color: Some(bg.into()),
        underline: underline.then_some(gpui::UnderlineStyle {
            thickness: px(1.),
            color: Some(fg.into()),
            wavy: false,
        }),
        strikethrough: strikeout.then_some(gpui::StrikethroughStyle {
            thickness: px(1.),
            color: Some(fg.into()),
        }),
    }
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

/// 光标格的像素 bounds（元素相对坐标），独立于可见性（IME 候选窗定位用）。
/// 注意：layout.origin 已含元素在窗口中的偏移，这里只算元素内相对位置，
/// 窗口坐标由 `bounds_for_range` 加上元素偏移得出。
fn cursor_pixel_bounds(content: &Content, layout: &TerminalLayout) -> Bounds<Pixels> {
    let cursor = &content.cursor;
    let row = (cursor.point.line + content.display_offset as i32)
        .clamp(0, content.screen_lines as i32 - 1) as usize;
    let col = cursor.point.column.min(content.columns.saturating_sub(1));
    let origin = Point::new(
        col as f32 * layout.cell_width,
        row as f32 * layout.line_height,
    );
    Bounds::new(origin, size(layout.cell_width, layout.line_height))
}

/// 光标渲染：Block 整格 + 字符；Beam/Underline 细条；隐藏模式省略。
fn layout_cursor(
    content: &Content,
    layout: &TerminalLayout,
    _window: &mut Window,
    cx: &mut App,
    show_cursor: bool,
) -> Option<CursorLayout> {
    let cursor = &content.cursor;
    if matches!(cursor.shape, CursorShape::Hidden) {
        return None;
    }
    if !show_cursor {
        return None;
    }
    let row = (cursor.point.line + content.display_offset as i32)
        .clamp(0, content.screen_lines as i32 - 1) as usize;
    let col = cursor.point.column.min(content.columns.saturating_sub(1));
    let origin = Point::new(
        layout.origin.x + col as f32 * layout.cell_width,
        layout.origin.y + row as f32 * layout.line_height,
    );
    let color = color::current(cx).editor_cursor;
    match cursor.shape {
        CursorShape::Block => {
            let bounds = Bounds::new(origin, size(layout.cell_width, layout.line_height));
            // 光标格内字符用终端背景色绘制，保证对比。
            let text = (content.cursor_char != ' ')
                .then(|| (content.cursor_char.to_string(), layout.background));
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

/// 单元格前景色：逆显时交换为背景色；dim 时降低亮度。
fn foreground_for(cell: &Cell, window: &mut Window, cx: &mut App) -> Rgba {
    let mut color = if cell.is_inverse() {
        palette::color_to_rgba(&cell.background(), window, cx)
    } else {
        palette::color_to_rgba(&cell.foreground(), window, cx)
    };
    if cell.is_dim() {
        color.a *= 0.5;
    }
    color
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
