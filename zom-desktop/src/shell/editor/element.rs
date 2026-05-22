//! 可嵌入编辑器渲染图元 —— 唯一的编辑器实现（借鉴 Zed 的 `EditorElement`）。
//!
//! 单行与多行不是两种编辑器，而是同一个编辑器的两个 [`EditorKind`]：行号列、
//! 纵向滚动、高度模式全部由「行数」派生，调用方只选 kind，配不出
//! 「带行号的单行」这种无意义组合。
//!
//! 文本与光标分层绘制：每帧把各行 shape 成 [`ShapedLine`]，文本只随内容变化，
//! 光标只是一个独立填充矩形 —— 移动光标不触发文本重排，这是「不闪烁」的根因。
//!
//! 滚动：维护一个跨帧滚动偏移（存于 GPUI 元素状态），每帧做 autoscroll 让
//! 光标保持在视口内，正文据此整体平移。

use std::panic::Location;

use gpui::{
    App, Bounds, ContentMask, Element, ElementId, GlobalElementId, Hsla, InspectorElementId,
    IntoElement, LayoutId, Pixels, Point, ShapedLine, SharedString, Style, TextRun, Window, fill,
    point, px, relative, size,
};

use crate::shell::InputHandlerHook;
use crate::shell::shared::theme::color;

/// 行号列宽（24）+ 与正文的间距（12）。
const GUTTER_WIDTH: f32 = 36.0;
/// 光标竖条宽度。
const CARET_WIDTH: f32 = 1.0;

/// 光标竖条颜色 —— 编辑器自持的视觉角色，不随嵌入处而变。
fn caret_color() -> Hsla {
    color::focus::border().into()
}

/// 行号文字颜色 —— 次级信息。
fn gutter_color() -> Hsla {
    color::gray::g60().into()
}

/// 编辑器的唯一区分轴：行数上限。其余差异（行号、纵向滚动、高度）皆由它派生。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditorKind {
    /// 单行：无行号、高度恰为一行、不发生纵向滚动。
    SingleLine,
    /// 多行：带行号、撑满容器、可纵向滚动。
    MultiLine,
}

/// 一个独立文本编辑单元的渲染图元。
///
/// 文本样式（字体 / 字号 / 行高 / 前景色）从父级 div 继承 —— 嵌入处决定，
/// 编辑器一律「继承」。光标色、行号色是编辑器自持的视觉角色。
pub(crate) struct EditorElement {
    kind: EditorKind,
    text: SharedString,
    cursor_byte: usize,
    input_handler_hook: InputHandlerHook,
    /// 光标当前是否可见（闪烁状态，由调用方注入）。
    caret_visible: bool,
    /// 跨帧滚动偏移的状态键。每个编辑器实例都应给一个稳定 id。
    element_id: Option<ElementId>,
}

impl EditorElement {
    pub(crate) fn new(
        kind: EditorKind,
        text: impl Into<SharedString>,
        cursor_byte: usize,
        input_handler_hook: InputHandlerHook,
    ) -> Self {
        Self {
            kind,
            text: text.into(),
            cursor_byte,
            input_handler_hook,
            caret_visible: true,
            element_id: None,
        }
    }

    /// 设置光标可见性（闪烁灭相时为 `false`，不绘制光标）。
    pub(crate) fn caret_visible(mut self, visible: bool) -> Self {
        self.caret_visible = visible;
        self
    }

    /// 赋予稳定的元素 id —— 据此跨帧保留滚动偏移。
    pub(crate) fn element_id(mut self, id: impl Into<ElementId>) -> Self {
        self.element_id = Some(id.into());
        self
    }

    /// 是否渲染行号列：仅多行编辑器。
    fn has_gutter(&self) -> bool {
        matches!(self.kind, EditorKind::MultiLine)
    }

    /// 是否撑满父容器：多行编辑器撑满并内部滚动，单行编辑器高度即一行。
    fn fills_viewport(&self) -> bool {
        matches!(self.kind, EditorKind::MultiLine)
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
    gutter_offset: Pixels,
    /// 光标位置：(视觉行号, 行内像素 x)。
    caret: (usize, Pixels),
    /// 当前滚动偏移；正文与光标按它整体平移。
    scroll: Point<Pixels>,
}

/// 跨帧保留的滚动偏移，存于 GPUI 元素状态。
#[derive(Default)]
struct EditorScroll {
    offset: Point<Pixels>,
}

/// 把 `value` 夹在 `[lo, hi]`（`Pixels` 无 `clamp`，本地实现）。
fn clamp_px(value: Pixels, lo: Pixels, hi: Pixels) -> Pixels {
    if value < lo {
        lo
    } else if value > hi {
        hi
    } else {
        value
    }
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
        self.element_id.clone()
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

        let mut style = Style::default();
        style.size.width = relative(1.).into();
        if self.fills_viewport() {
            // 多行编辑器：撑满视口，内容溢出靠内部滚动偏移消化。
            style.size.height = relative(1.).into();
        } else {
            // 单行编辑器：高度恰为一行。
            style.size.height = line_height.into();
        }
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
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut EditorLayout,
        window: &mut Window,
        _cx: &mut App,
    ) -> EditorPrepaint {
        let text_style = window.text_style();
        let font_size = layout.font_size;
        let line_height = layout.line_height;
        let cursor_byte = self.cursor_byte.min(self.text.len());
        let has_gutter = self.has_gutter();

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

            if has_gutter {
                let label = (index + 1).to_string();
                let run = TextRun {
                    len: label.len(),
                    font: text_style.font(),
                    color: gutter_color(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                gutter.push(
                    window
                        .text_system()
                        .shape_line(label.into(), font_size, &[run], None),
                );
            }

            offset = line_end + 1;
        }

        let gutter_offset = if has_gutter { px(GUTTER_WIDTH) } else { px(0.) };

        // autoscroll：让光标保持在视口内。需要稳定 element id 来跨帧存偏移。
        let scroll = match id {
            Some(global_id) => {
                let content_height = line_height * lines.len().max(1) as f32;
                let content_width =
                    lines.iter().fold(
                        px(0.),
                        |max, line| if line.width > max { line.width } else { max },
                    );
                let viewport_h = bounds.size.height;
                let viewport_w =
                    clamp_px(bounds.size.width - gutter_offset, px(0.), bounds.size.width);
                window.with_element_state::<EditorScroll, _>(global_id, |state, _window| {
                    let mut state = state.unwrap_or_default();
                    let mut off = state.offset;

                    // 纵向：光标所在行需完整可见。
                    let caret_top = line_height * caret.0 as f32;
                    let caret_bottom = caret_top + line_height;
                    if caret_top < off.y {
                        off.y = caret_top;
                    } else if caret_bottom > off.y + viewport_h {
                        off.y = caret_bottom - viewport_h;
                    }
                    off.y = clamp_px(
                        off.y,
                        px(0.),
                        clamp_px(content_height - viewport_h, px(0.), content_height),
                    );

                    // 横向：光标列需可见。行尾光标位于最后一个字形「之后」，
                    // 故可滚动宽度要在最宽行的基础上额外留出一个光标宽度 ——
                    // 否则滚到行尾时光标正好压在正文区裁剪边界上、被切掉。
                    let scrollable_w = content_width + px(CARET_WIDTH);
                    let caret_right = caret.1 + px(CARET_WIDTH);
                    if caret.1 < off.x {
                        off.x = caret.1;
                    } else if caret_right > off.x + viewport_w {
                        off.x = caret_right - viewport_w;
                    }
                    off.x = clamp_px(
                        off.x,
                        px(0.),
                        clamp_px(scrollable_w - viewport_w, px(0.), scrollable_w),
                    );

                    state.offset = off;
                    (off, state)
                })
            }
            None => Point::default(),
        };

        EditorPrepaint {
            lines,
            gutter,
            line_height,
            gutter_offset,
            caret,
            scroll,
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
        let scroll = prepaint.scroll;
        // 纵向滚动作用于所有内容（行号随正文一起上下移）。
        let top = bounds.origin.y - scroll.y;
        // 横向滚动只作用于正文与光标；行号列固定不动。
        let text_left = bounds.origin.x + prepaint.gutter_offset - scroll.x;

        // 正文与光标裁剪到「正文区」：横向滚动时不溢出到行号列。
        let text_area = Bounds {
            origin: point(bounds.origin.x + prepaint.gutter_offset, bounds.origin.y),
            size: size(
                clamp_px(
                    bounds.size.width - prepaint.gutter_offset,
                    px(0.),
                    bounds.size.width,
                ),
                bounds.size.height,
            ),
        };
        let caret_visible = self.caret_visible;
        let (caret_line, caret_x) = prepaint.caret;

        window.with_content_mask(Some(ContentMask { bounds: text_area }), |window| {
            for (index, shaped) in prepaint.lines.iter().enumerate() {
                let y = top + line_height * index as f32;
                let _ = shaped.paint(point(text_left, y), line_height, window, cx);
            }

            // 光标是独立绘制层：一个填充矩形叠在文本之上，移动它不触碰任何字形。
            if caret_visible {
                let caret_bounds = Bounds {
                    origin: point(text_left + caret_x, top + line_height * caret_line as f32),
                    size: size(px(CARET_WIDTH), line_height),
                };
                window.paint_quad(fill(caret_bounds, caret_color()));
            }
        });

        // 行号列：只随正文纵向滚动，横向固定在 bounds 左缘。
        for (index, gutter_line) in prepaint.gutter.iter().enumerate() {
            let y = top + line_height * index as f32;
            let _ = gutter_line.paint(point(bounds.origin.x, y), line_height, window, cx);
        }

        // 在 paint 阶段把编辑器输入宿主注册为系统输入法接收端；bounds 供候选窗定位。
        (self.input_handler_hook)(bounds, window, cx);
    }
}
