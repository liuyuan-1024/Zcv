//! 编辑器渲染图元：文本与光标分层绘制（借鉴 Zed 的 `EditorElement`）。
//!
//! 每帧把各行 shape 成 [`ShapedLine`]，文本与光标坐标取自同一份 shaped
//! 结果：文本只随 buffer 内容变化，光标移动只改一个填充矩形的位置 —— 二者
//! 不再相互触发重排，这正是「移动光标不闪烁」的根因。旧实现把 `|` 直接插进
//! 文本字符串，每次移动光标都改变整行内容、强制重新 shape，故而闪烁。

use std::panic::Location;

use gpui::{
    App, Bounds, Element, ElementId, GlobalElementId, Hsla, InspectorElementId, IntoElement,
    LayoutId, Pixels, ShapedLine, SharedString, Style, TextRun, Window, fill, point, px, relative,
    size,
};

use crate::shell::InputHandlerHook;

/// 行号列宽（24）+ 与正文的间距（12）；与旧 div 布局口径一致。
const GUTTER_WIDTH: f32 = 36.0;
/// 光标竖条宽度，与内联编辑器旧 caret 一致。
const CARET_WIDTH: f32 = 1.0;

/// 一个独立文本编辑单元的渲染图元。
///
/// 文本样式（字体 / 字号 / 行高 / 前景色）从父级 div 继承，调用方只需额外
/// 给出光标色与可选的行号列。
pub(crate) struct EditorElement {
    text: SharedString,
    cursor_byte: usize,
    input_handler_hook: InputHandlerHook,
    caret_color: Hsla,
    /// `Some(色)` 渲染左侧行号列；`None` 表示无行号（内联编辑器）。
    gutter_color: Option<Hsla>,
}

impl EditorElement {
    pub(crate) fn new(
        text: impl Into<SharedString>,
        cursor_byte: usize,
        input_handler_hook: InputHandlerHook,
    ) -> Self {
        Self {
            text: text.into(),
            cursor_byte,
            input_handler_hook,
            caret_color: Hsla::default(),
            gutter_color: None,
        }
    }

    /// 设置光标竖条颜色。
    pub(crate) fn caret_color(mut self, color: impl Into<Hsla>) -> Self {
        self.caret_color = color.into();
        self
    }

    /// 启用左侧行号列，并指定行号文字颜色。
    pub(crate) fn with_gutter(mut self, color: impl Into<Hsla>) -> Self {
        self.gutter_color = Some(color.into());
        self
    }
}

/// `request_layout` 阶段算出、供后续阶段复用的度量。
pub(crate) struct EditorLayout {
    line_height: Pixels,
    font_size: Pixels,
}

/// `prepaint` 阶段 shape 出的、供 `paint` 直接绘制的结果。
pub(crate) struct EditorPrepaint {
    /// 每行正文的 shaped 结果，下标即视觉行号。
    lines: Vec<ShapedLine>,
    /// 每行行号的 shaped 结果；无行号列时为空。
    gutter: Vec<ShapedLine>,
    line_height: Pixels,
    /// 正文起点相对 `bounds.origin.x` 的偏移（有行号列时为 [`GUTTER_WIDTH`]）。
    text_x: Pixels,
    /// 光标位置：(视觉行号, 行内像素 x)。
    caret: (usize, Pixels),
}

impl IntoElement for EditorElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for EditorElement {
    type RequestLayoutState = EditorLayout;
    type PrepaintState = EditorPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, EditorLayout) {
        let text_style = window.text_style();
        let rem_size = window.rem_size();
        let line_height = text_style.line_height_in_pixels(rem_size);
        let font_size = text_style.font_size.to_pixels(rem_size);
        // 行数靠 '\n' 切分；空文本也算一行（光标停在空行）。
        let line_count = self.text.split('\n').count().max(1);

        let mut style = Style::default();
        // 宽度撑满父级；高度按内容行数定 —— 父级负责裁剪溢出。
        style.size.width = relative(1.).into();
        style.size.height = (line_height * line_count as f32).into();
        let layout_id = window.request_layout(style, [], cx);

        (
            layout_id,
            EditorLayout {
                line_height,
                font_size,
            },
        )
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        layout: &mut EditorLayout,
        window: &mut Window,
        _cx: &mut App,
    ) -> EditorPrepaint {
        let text_style = window.text_style();
        let font_size = layout.font_size;
        let cursor_byte = self.cursor_byte.min(self.text.len());

        let mut lines = Vec::new();
        let mut gutter = Vec::new();
        let mut caret = (0usize, px(0.));
        // 当前行起点在整段文本里的字节偏移。
        let mut offset = 0usize;

        for (index, raw) in self.text.split('\n').enumerate() {
            // 文本 shape 的输入只有「内容 + 字体」，与光标无关 —— 同一行内容
            // 每帧 shape 命中 GPUI 行布局缓存，光标移动不会触发重排。
            let runs = if raw.is_empty() {
                Vec::new()
            } else {
                vec![text_style.to_run(raw.len())]
            };
            let shaped =
                window
                    .text_system()
                    .shape_line(raw.to_string().into(), font_size, &runs, None);

            let line_end = offset + raw.len();
            if cursor_byte >= offset && cursor_byte <= line_end {
                caret = (index, shaped.x_for_index(cursor_byte - offset));
            }
            lines.push(shaped);

            if let Some(gutter_color) = self.gutter_color {
                let label = (index + 1).to_string();
                let run = TextRun {
                    len: label.len(),
                    font: text_style.font(),
                    color: gutter_color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                gutter.push(window.text_system().shape_line(
                    label.into(),
                    font_size,
                    &[run],
                    None,
                ));
            }

            offset = line_end + 1;
        }

        let text_x = if self.gutter_color.is_some() {
            px(GUTTER_WIDTH)
        } else {
            px(0.)
        };

        EditorPrepaint {
            lines,
            gutter,
            line_height: layout.line_height,
            text_x,
            caret,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _layout: &mut EditorLayout,
        prepaint: &mut EditorPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let line_height = prepaint.line_height;
        let text_x = bounds.origin.x + prepaint.text_x;

        for (index, shaped) in prepaint.lines.iter().enumerate() {
            let y = bounds.origin.y + line_height * index as f32;
            if let Some(gutter_line) = prepaint.gutter.get(index) {
                let _ = gutter_line.paint(point(bounds.origin.x, y), line_height, window, cx);
            }
            let _ = shaped.paint(point(text_x, y), line_height, window, cx);
        }

        // 光标是独立绘制层：一个填充矩形叠在文本之上，移动它不触碰任何字形。
        let (caret_line, caret_x) = prepaint.caret;
        let caret_bounds = Bounds {
            origin: point(
                text_x + caret_x,
                bounds.origin.y + line_height * caret_line as f32,
            ),
            size: size(px(CARET_WIDTH), line_height),
        };
        window.paint_quad(fill(caret_bounds, self.caret_color));

        // 在 paint 阶段把编辑器输入宿主注册为系统输入法接收端；bounds 供候选窗定位。
        (self.input_handler_hook)(bounds, window, cx);
    }
}
