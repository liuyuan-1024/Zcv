//! 可嵌入编辑器渲染图元 —— 唯一的编辑器实现（借鉴 Zed 的 `EditorElement`）。
//!
//! 单行与多行不是两种编辑器，而是同一个编辑器的两个 [`EditorKind`]：行号列、
//! 纵向滚动、高度模式全部由「行数」派生，调用方只选 kind，配不出
//! 「带行号的单行」这种无意义组合。
//!
//! 文本与光标分层绘制：每帧把各行 shape 成 [`ShapedLine`]，文本只随内容变化，
//! 光标只是一个独立填充矩形 —— 移动光标不触发文本重排，这是「不闪烁」的根因。
//!
//! 滚动有两条独立路径，共存于 [`Self::prepaint`]：
//!
//! - **reveal 路径**：响应外部 [`zom_view::RevealRequest`]（搜索 / goto-* 等
//!   命令调 `view.request_reveal(...)` 投递）。按 [`RevealKind`] 翻译成具体
//!   摆位策略；每个 seq 只触发一次。
//! - **edge-scroll 路径**：caret 跟随。永远跑，作为兜底。reveal 摆完位置后，
//!   edge-scroll 仍会跑，保证 caret 真的可见 —— 哪怕 reveal 把 reveal byte
//!   摆到上 1/3 但 caret（=match end）跨了多行被推到视区外，edge-scroll
//!   会把它拉回来。
//!
//! 两条路径共用一份跨帧滚动偏移（[`EditorScroll`]），存于 GPUI 元素状态。

use std::panic::Location;

use gpui::{
    App, Bounds, ContentMask, Element, ElementId, FocusHandle, GlobalElementId, Hsla,
    InspectorElementId, IntoElement, LayoutId, Pixels, Point, ShapedLine, SharedString, Style,
    TextRun, Window, fill, point, px, relative, size,
};

use zom_engine::{SelectionSet, TextRange};
use zom_view::RevealKind;

use crate::shell::shared::theme::{color, radius};

use super::blink::CaretClock;
use super::core::RevealHint;
use super::embed::{EditorInputHook, EditorPaintInfo};
use super::highlight::{LineMetric, paint_range_backgrounds};
use super::input::CaretLayout;

/// 行号列宽（24）+ 与正文的间距（12）。
const GUTTER_WIDTH: f32 = 36.0;
/// 光标竖条宽度。2px 与 VS Code / Zed 默认一致——1px 在高分屏上偏细。
const CARET_WIDTH: f32 = 2.0;

/// 光标竖条颜色 —— 编辑器自持的视觉角色，不随嵌入处而变。
fn caret_color() -> Hsla {
    color::blue::s07().into()
}

/// 行号文字颜色 —— 次级信息。
fn gutter_color() -> Hsla {
    color::gray::s08().into()
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
///
/// 光标闪烁可见性不在字段里 —— 每帧 paint 时从 [`CaretClock`] 全局读，整窗
/// 共享同一相位。
pub(crate) struct EditorElement {
    kind: EditorKind,
    text: SharedString,
    /// 完整选区集合。每个 selection 的 head 各画一个 caret（阶段 5）；
    /// 非空 selection 的 range 进阶段 2 范围背景；reveal / edge-scroll 只看
    /// primary。`SelectionSet::Clone` 是 O(1)（内部 Arc），元素按帧重建。
    selection: SelectionSet,
    focus: FocusHandle,
    input_handler_hook: EditorInputHook,
    /// 跨帧滚动偏移的状态键。每个编辑器实例都应给一个稳定 id。
    element_id: Option<ElementId>,
    /// 外部 reveal 请求；按 seq 触发一次 reveal 路径。
    reveal: Option<RevealHint>,
}

impl EditorElement {
    pub(crate) fn new(
        kind: EditorKind,
        text: impl Into<SharedString>,
        selection: SelectionSet,
        focus: FocusHandle,
        input_handler_hook: EditorInputHook,
    ) -> Self {
        Self {
            kind,
            text: text.into(),
            selection,
            focus,
            input_handler_hook,
            element_id: None,
            reveal: None,
        }
    }

    /// 赋予稳定的元素 id —— 据此跨帧保留滚动偏移。
    pub(crate) fn element_id(mut self, id: impl Into<ElementId>) -> Self {
        self.element_id = Some(id.into());
        self
    }

    pub(crate) fn reveal(mut self, hint: RevealHint) -> Self {
        self.reveal = Some(hint);
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

/// 单行的 prepaint 产物 —— shape 结果 + 字节坐标对齐信息。
///
/// 字节坐标信息是阶段 2 范围背景的硬性输入（[`LineMetric`]）：哪一段 selection
/// 跨过哪一行、行内 x_start / x_end 在哪，都依赖 `line_start_byte` / `line_len`
/// 把整段文本的字节区间投影到每行内。
pub(crate) struct PrepaintedLine {
    /// 本行起始字节相对整段 text 的偏移。
    line_start_byte: usize,
    /// 本行内容字节长度（不含 `\n`）。
    line_len: usize,
    shaped: ShapedLine,
}

/// `prepaint` 阶段 shape 出的、供 `paint` 直接绘制的结果。
pub(crate) struct EditorPrepaint {
    /// 每行的 shape + 字节坐标信息，下标即视觉行号。
    lines: Vec<PrepaintedLine>,
    /// 每行行号的 shaped 结果；无行号列时为空。
    gutter: Vec<ShapedLine>,
    line_height: Pixels,
    /// 正文起点相对 `bounds.origin.x` 的偏移（有行号列时为 [`GUTTER_WIDTH`]）。
    gutter_offset: Pixels,
    /// 所有 selection 的 head 位置 `(视觉行号, 行内像素 x)`，下标与
    /// `EditorElement::selection.as_slice()` 一一对应。
    ///
    /// reveal / edge-scroll 只看 primary（在 prepaint 内部就消化掉了，paint
    /// 拿到的就是"全部 caret 都要画"的扁平集合）。
    carets: Vec<(usize, Pixels)>,
    /// 非空 selection 的字节区间，已按 start 升序、互不重叠（SelectionSet 契约）。
    selection_ranges: Vec<TextRange>,
    /// 当前滚动偏移；正文与光标按它整体平移。
    scroll: Point<Pixels>,
}

/// 跨帧保留的滚动偏移，存于 GPUI 元素状态。
#[derive(Default)]
struct EditorScroll {
    offset: Point<Pixels>,
    /// 最近一次已处理的 reveal seq。
    /// `None` 表示没见过任何 reveal。第一次见到任何 seq 都视作新请求。
    last_applied_reveal_seq: Option<u64>,
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
        let text_len = self.text.len();
        let has_gutter = self.has_gutter();

        // SelectionSet 经过引擎归一化，as_slice() 已按 start 排序、互不重叠。
        // primary 在归一化前后由 primary_index 跟踪 —— 这里 carets 的下标与
        // as_slice 同步，下文用 primary_index 直接定位 primary caret。
        let selections = self.selection.as_slice();
        let primary_index = self
            .selection
            .primary_index()
            .min(selections.len().saturating_sub(1));
        // 每个 head 的 (视觉行, 行内 x)；用 None 占位，按 line 扫描时填充。
        let mut carets_pos: Vec<Option<(usize, Pixels)>> = vec![None; selections.len()];
        let mut lines = Vec::new();
        let mut gutter = Vec::new();
        let reveal_byte = self.reveal.map(|hint| hint.byte.min(text_len));
        // 同步记 reveal byte 的(行, 行内 x)：reveal 路径要把 X 也照顾到，
        // 否则 match 在视区左边时 edge-scroll 会把 caret(=match end) 拉到左边缘，
        // 反而把 match 本体切到视区左侧外。
        let mut reveal_pos: Option<(usize, Pixels)> = None;
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
            // 每个 selection.head 落在哪一行就在哪一行算 x。head 出现在行末
            // (`head == line_end`) 与行首 (`head == offset` for next line) 是
            // 同一字节位置——head 优先归到「等于行尾」的那一行（与旧 cursor_byte
            // 行为一致：保留尾随空格后的光标位）。
            for (i, sel) in selections.iter().enumerate() {
                if carets_pos[i].is_some() {
                    continue;
                }
                let head = sel.head().get().min(text_len);
                if head >= offset && head <= line_end {
                    carets_pos[i] = Some((index, shaped.x_for_index(head - offset)));
                }
            }
            if let Some(rb) = reveal_byte
                && reveal_pos.is_none()
                && rb >= offset
                && rb <= line_end
            {
                reveal_pos = Some((index, shaped.x_for_index(rb - offset)));
            }
            lines.push(PrepaintedLine {
                line_start_byte: offset,
                line_len: raw.len(),
                shaped,
            });

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

        // 任何 head 没匹配上（极端：选区指向超出当前 text 的字节，stale 快照）
        // 都退化到文首 caret，避免出现"看不到的 caret"。primary 同样保护。
        let carets: Vec<(usize, Pixels)> = carets_pos
            .into_iter()
            .map(|p| p.unwrap_or((0, px(0.))))
            .collect();
        // selection_ranges 只收非空区间；caret-only 的 selection 不画背景。
        // 顺序与 selections 同步，因此天然保持 start 升序、互不重叠的契约。
        let selection_ranges: Vec<TextRange> = selections
            .iter()
            .filter(|s| !s.is_caret())
            .map(|s| s.range())
            .collect();
        let primary_caret = carets.get(primary_index).copied().unwrap_or((0, px(0.)));

        let gutter_offset = if has_gutter { px(GUTTER_WIDTH) } else { px(0.) };

        // autoscroll：让光标保持在视口内。需要稳定 element id 来跨帧存偏移。
        let scroll = match id {
            Some(global_id) => {
                let content_height = line_height * lines.len().max(1) as f32;
                let content_width = lines.iter().fold(px(0.), |max, line| {
                    if line.shaped.width > max {
                        line.shaped.width
                    } else {
                        max
                    }
                });
                let viewport_h = bounds.size.height;
                let viewport_w =
                    clamp_px(bounds.size.width - gutter_offset, px(0.), bounds.size.width);
                let reveal = self.reveal;
                window.with_element_state::<EditorScroll, _>(global_id, |state, _window| {
                    let mut state = state.unwrap_or_default();
                    let mut off = state.offset;

                    // === reveal 路径 ===
                    // 外部 request_reveal 投递的请求；按 seq 触发一次。
                    // kind 决定触发条件与摆位 —— 集中在这里翻译，调用方只表达"意图"。
                    //
                    // 两个轴独立判断可见性 / 独立摆位：搜索 match 在视区"左边"
                    // 但同一行内时，只需要滚 X；在不同行但同一列时，只需要滚 Y。
                    // 一起判断会把"X 已经可见"也当成需要滚，反而抖。
                    if let Some(req) = reveal
                        && Some(req.seq) != state.last_applied_reveal_seq
                    {
                        if let Some((row, x)) = reveal_pos {
                            let target_top = line_height * row as f32;
                            let target_bottom = target_top + line_height;
                            let visible_y =
                                target_top >= off.y && target_bottom <= off.y + viewport_h;
                            let target_right = x + px(CARET_WIDTH);
                            let visible_x = x >= off.x && target_right <= off.x + viewport_w;

                            // (placement_factor 即"1/3"那个比例；force_scroll 区分
                            // IfOffscreen / Always 语义)
                            let (placement_factor, force_scroll) = match req.kind {
                                RevealKind::Match => (1.0 / 3.0, false),
                                RevealKind::Jump => (1.0 / 3.0, true),
                            };
                            if force_scroll || !visible_y {
                                off.y = target_top - viewport_h * placement_factor;
                            }
                            if force_scroll || !visible_x {
                                off.x = x - viewport_w * placement_factor;
                            }
                        }
                        // 即使本帧没拿到行号也要登记 seq。
                        // 不然下一帧文本仍然能命中行时会"补"一次滚动，对用户来说像延迟反应。
                        state.last_applied_reveal_seq = Some(req.seq);
                    }

                    // === edge-scroll 路径（兜底）===
                    // 即便 reveal 把 reveal byte 摆到上 1/3，caret（=match end）
                    // 在多行 match 时也可能跨到视区外；edge-scroll 把 caret 拉回来，宁可让 reveal byte 飘出视区，也不能让 caret 消失。
                    // 多光标场景只让 primary caret 决定滚动 —— 多个 caret 同时驱动
                    // 滚动会互相拉扯。primary 是用户最近交互的 caret，跟它走最自然。
                    let (caret_line, caret_x) = primary_caret;
                    let caret_top = line_height * caret_line as f32;
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
                    let caret_right = caret_x + px(CARET_WIDTH);
                    if caret_x < off.x {
                        off.x = caret_x;
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
            carets,
            selection_ranges,
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
        // 选区色与焦点态解耦：caret 是否闪烁已足够传达"哪个 view 活动"，
        // 选区无论焦点都用同一颜色（与 Zed 一致）。FocusHandle 仅决定 caret 可见。
        let caret_visible = self.focus.is_focused(window) && CaretClock::is_visible(cx);
        // 选区色：blue.a04（手册 §4.2 第 04 档 ui-active 的 alpha 形态）。
        let selection_color: Hsla = color::blue::a04().into();
        let selection_quads: Vec<(TextRange, Hsla)> = prepaint
            .selection_ranges
            .iter()
            .map(|range| (*range, selection_color))
            .collect();
        // LineMetric 借用 ShapedLine：放在 with_content_mask 闭包外构造，
        // 让 paint_range_backgrounds 与文本绘制共用一份借用切片。
        let line_metrics: Vec<LineMetric<'_>> = prepaint
            .lines
            .iter()
            .map(|line| LineMetric {
                line_start_byte: line.line_start_byte,
                line_len: line.line_len,
                shaped: &line.shaped,
            })
            .collect();

        window.with_content_mask(Some(ContentMask { bounds: text_area }), |window| {
            // 阶段 2 范围背景：画在文本之下，半透明色块让 syntax 字色透过来。
            paint_range_backgrounds(
                &selection_quads,
                &line_metrics,
                text_left,
                top,
                line_height,
                text_area,
                window,
            );

            // 阶段 3 文本。
            for (index, line) in prepaint.lines.iter().enumerate() {
                let y = top + line_height * index as f32;
                let _ = line
                    .shaped
                    .paint(point(text_left, y), line_height, window, cx);
            }

            // 阶段 5 caret：每个 selection.head 各画一根竖条，
            // 叠在文本与选区色块之上；blink 全局共享一只时钟（CaretClock）。
            if caret_visible {
                for (caret_line, caret_x) in &prepaint.carets {
                    let caret_bounds = Bounds {
                        origin: point(text_left + *caret_x, top + line_height * *caret_line as f32),
                        size: size(px(CARET_WIDTH), line_height),
                    };
                    window.paint_quad(fill(caret_bounds, caret_color()).corner_radii(radius::r2()));
                }
            }
        });

        // 行号列：只随正文纵向滚动，横向固定在 bounds 左缘。
        for (index, gutter_line) in prepaint.gutter.iter().enumerate() {
            let y = top + line_height * index as f32;
            let _ = gutter_line.paint(point(bounds.origin.x, y), line_height, window, cx);
        }

        // 把 primary caret 在 element 内的相对坐标打包给 input hook。系统 IME
        // 的 `bounds_for_range` 会借此把候选窗放到 caret 正下方；没有 caret 时
        // 传 None，让 IME 走默认（候选窗会落在窗口角，明显比"贴在编辑区左上
        // 角"易辨认是哪里有问题）。
        let caret_layout = (!prepaint.carets.is_empty()).then(|| {
            let primary_idx = self
                .selection
                .primary_index()
                .min(prepaint.carets.len() - 1);
            let (caret_line, caret_x) = prepaint.carets[primary_idx];
            CaretLayout {
                relative: point(
                    prepaint.gutter_offset + caret_x - scroll.x,
                    line_height * caret_line as f32 - scroll.y,
                ),
                line_height,
            }
        });
        (self.input_handler_hook)(
            EditorPaintInfo {
                bounds,
                caret_layout,
            },
            window,
            cx,
        );
    }
}
