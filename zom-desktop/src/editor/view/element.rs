//! 可嵌入编辑器渲染图元 —— 唯一的编辑器实现（借鉴 Zed 的 `EditorElement`）。
//!
//! 单行输入框与主编辑区不是两种编辑器，而是同一个[`EditorKernel`](crate::editor::EditorKernel) 按能力开关创建出的两个形态；本文件只费内核传下来的能力开关并完成 GPUI 绘制。
//!
//! 文本与光标分层绘制：每帧把视口内每行 shape 成 [`ShapedLine`]，文本只随内容变化，光标只是一个独立填充矩形 —— 移动光标不触发文本重排，这是「不闪烁」的根因。
//!
//! 视口切片：snapshot 已经按 view 当前 `(top_line, visible_logical_lines)` 给出一段 `SnapshotLine[]`，element 只 shape 这一段；
//! `total_lines` 决定`content_height`，从而支持 GB 级文件不爆显存。
//! prepaint 在 shape 出本帧 `breaks_per_line` 之后立刻调 [`EditorViewportSyncHook`]：
//! 把可见行数和新 wrap_map 写回 view、随即跑一次 settle，再用 settle 结果决定本帧 `top_visual_row`。
//! 这样「插入新行 / 触发新 sub-row」在同一帧里就能被 edge-scroll 拉回视区，不留一帧滞后。
//! 首帧由 main_editor 的 `DEFAULT_VISIBLE_LINES` 兜底。
//!
//! 软换行：开关由 [`EditorKernel::soft_wrap`] 控制，运行时可切换。
//! 开启后每条逻辑行可能被拆成多条「视觉行」（sub-row），prepaint 阶段在 shape 完一次全行后用 [`zom_workspace::view::compute_segments`] 按视口宽度算出断点字节列表，再为每段 sub-row 重 shape。
//! 分行规则与 CJK 边界判定都内聚在 `view::wrap` 模块——这里只负责测量与绘制。
//! 下游 phases / gutter 一律按视觉行索引消费；`PrepaintedLine` 的 `line_start_byte` / `line_len` 直接描述当前视觉段的字节边界。
//! 软换行打开时禁用横向滚动，viewport_sync 回写的不是视觉行数而是「下一帧切多少条逻辑行就够铺满视口」。
//!
//! 滚动有两条独立路径，共存于 [`Self::prepaint`]：
//!
//! - **reveal 路径**：
//! 响应外部 [`zom_workspace::view::RevealRequest`]（搜索 / goto-* 等命令调 `view.request_reveal(...)` 投递）。
//! 按 [`RevealKind`] 翻译成具体摆位策略；每个 seq 只触发一次。
//! - **edge-scroll 路径**：
//! caret 跟随。永远跑，作为兜底。reveal 摆完位置后，edge-scroll 仍会跑，保证 caret 真的可见 —— 哪怕 reveal 把 reveal byte 摆到上 1/3 但 caret（=match end）跨了多行被推到视区外，edge-scroll 会把它拉回来。
//!
//! 两条路径共用一份跨帧滚动偏移（[`EditorScroll`]），存于 GPUI 元素状态。

use std::panic::Location;
use std::rc::Rc;

use gpui::{
    App, Bounds, ContentMask, CursorStyle, Element, ElementId, FocusHandle, GlobalElementId,
    Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, Point,
    ShapedLine, SharedString, Style, TextRun, Window, point, px, relative, size,
};

use zom_engine::{SelectionSet, TextRange};
use zom_workspace::view::{RevealKind, VisualPosition, WrapMap};

use crate::editor::highlight::Decoration;
use crate::editor::text::snapshot::{RevealHint, SnapshotLine};
use crate::theme::color;

use crate::editor::highlight::compose::{Composition, compose};
use crate::editor::input::CaretLayout;
use crate::editor::kernel::EditorKernel;
use crate::editor::pointer::{
    PointerHitLine, PointerHitTest, PointerScrollHook, PointerSelectionHook,
    PointerSelectionSession, install_scroll_handler, install_selection_handlers,
};

use super::blink::CaretClock;
use super::gutter;
use super::input_host::{
    EditorInputHook, EditorPaintInfo, EditorViewportMeasurement, EditorViewportSyncHook,
    SettledViewportTop,
};
use super::phases::{
    GlyphOverlay, LineBackgroundQuad, LineMetric, paint_phase_1_line_backgrounds,
    paint_phase_2_range_backgrounds, paint_phase_3_glyphs, paint_phase_4_glyph_overlays,
    paint_phase_5_carets_and_composition,
};

/// 光标竖条宽度。2px 与 VS Code / Zed 默认一致——1px 在高分屏上偏细。
const CARET_WIDTH: f32 = 2.0;

/// 光标竖条颜色 —— 编辑器自持的视觉角色，不随嵌入处而变。
fn caret_color() -> Hsla {
    color::current().blue.s07.into()
}

/// 一个独立文本编辑单元的渲染图元。
///
/// 文本样式（字体 / 字号 / 行高 / 前景色）从父级 div 继承 —— 嵌入处决定，
/// 编辑器一律「继承」。光标色、行号色是编辑器自持的视觉角色。
///
/// 光标闪烁可见性不在字段里 —— 每帧 paint 时从 [`CaretClock`] 全局读，整窗
/// 共享同一相位。
pub(crate) struct EditorElement {
    kernel: EditorKernel,
    /// 当前视口可见的逻辑行（绝对 line / byte 坐标）。
    lines: Vec<SnapshotLine>,
    /// buffer 总行数——gutter 按它算列宽，避免滚动时行号列宽度抖动。
    total_lines: u64,
    /// view 落定的视口顶行（0-based）；
    /// 与 snapshot 切片起点不同，这是用户真正看到的顶部逻辑行。
    top_line: u64,
    /// `top_line` 内的软换行视觉段序号（0-based）。不开软换行时为 0。
    top_subrow: u64,
    /// 完整选区集合。
    /// 每个 selection 的 head 各画一个 caret（阶段 5）；
    /// reveal / edge-scroll 只看 primary。`SelectionSet::Clone` 是 O(1)（内部 Arc），元素按帧重建。
    ///
    /// **非空 selection 的 range 已作为 `Background` Decoration 由 snapshot 构造端推入 [`Self::decorations`]**（手册架构 §三 把选区列为独立 producer），本字段只剩 caret 几何用途；
    /// 范围背景由 composer 与 syntax / search 等一起合成。
    selection: SelectionSet,
    /// primary caret 的视觉投影，用来区分软换行边界处同一个 byte 的两个显示位置。
    visual_caret: Option<VisualPosition>,
    focus: FocusHandle,
    input_handler_hook: EditorInputHook,
    scroll_hook: Option<PointerScrollHook>,
    selection_hook: Option<PointerSelectionHook>,
    pointer_session: Option<PointerSelectionSession>,
    /// 跨帧滚动偏移的状态键。每个编辑器实例都应给一个稳定 id。
    element_id: Option<ElementId>,
    /// 外部 reveal 请求；按 seq 触发一次 reveal 路径。
    reveal: Option<RevealHint>,
    /// 高亮装饰集合——syntax / selection / search 等 producer 的统一产物（手册《桌面端高亮架构》§四）。
    /// prepaint 调用 [`highlight::compose`] 切分为前景 / 背景，分别喂给阶段 3 / 阶段 2。
    decorations: Vec<Decoration>,
    /// prepaint 末尾调用，把当前帧测得的 viewport 写回 view 的视口测量值；只主编辑区装。
    viewport_sync: Option<EditorViewportSyncHook>,
}

impl EditorElement {
    pub(crate) fn new(
        kernel: EditorKernel,
        lines: Vec<SnapshotLine>,
        total_lines: u64,
        top_line: u64,
        top_subrow: u64,
        selection: SelectionSet,
        visual_caret: Option<VisualPosition>,
        focus: FocusHandle,
        input_handler_hook: EditorInputHook,
    ) -> Self {
        Self {
            kernel,
            lines,
            total_lines,
            top_line,
            top_subrow,
            selection,
            visual_caret,
            focus,
            input_handler_hook,
            scroll_hook: None,
            selection_hook: None,
            pointer_session: None,
            element_id: None,
            reveal: None,
            decorations: Vec::new(),
            viewport_sync: None,
        }
    }

    /// 赋予稳定的元素 id —— 据此跨帧保留滚动偏移。
    pub(crate) fn element_id(mut self, id: impl Into<ElementId>) -> Self {
        self.element_id = Some(id.into());
        self
    }

    pub(crate) fn reveal_if_some(mut self, hint: Option<RevealHint>) -> Self {
        self.reveal = hint;
        self
    }

    /// 装载本编辑器的高亮装饰集合（syntax / selection / search 等）。
    ///
    /// 输入要求：每个 producer 自身保证内部 range 不重叠；
    /// 跨 producer 允许 Background 重叠（alpha 叠加表达层叠语义）。
    /// 空 Vec = 无装饰，前景退到继承的 text_style 单 run，背景全无。
    pub(crate) fn decorations(mut self, decorations: Vec<Decoration>) -> Self {
        self.decorations = decorations;
        self
    }

    pub(crate) fn viewport_sync(mut self, hook: EditorViewportSyncHook) -> Self {
        self.viewport_sync = Some(hook);
        self
    }

    pub(crate) fn scroll_hook(mut self, hook: PointerScrollHook) -> Self {
        self.scroll_hook = Some(hook);
        self
    }

    pub(crate) fn selection_hook(mut self, hook: PointerSelectionHook) -> Self {
        self.selection_hook = Some(hook);
        self
    }

    pub(crate) fn pointer_session(mut self, session: PointerSelectionSession) -> Self {
        self.pointer_session = Some(session);
        self
    }

    /// 是否渲染行号列：由内核能力决定，当前只有主编辑区开启。
    fn has_gutter(&self) -> bool {
        self.kernel.has_gutter()
    }

    /// 是否撑满父容器：主编辑区撑满并内部滚动，单行输入框高度即一行。
    fn fills_viewport(&self) -> bool {
        self.kernel.fills_viewport()
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
///
/// 注意：从视口切片接入起，`line_start_byte` 是该行在整 buffer 中的**绝对**字节偏移，而非 element 内某段文本的局部偏移——selection / search 命中的 `TextRange` 也是绝对 byte，二者天然对齐。
/// 开启软换行后，本结构表示**视觉行**（一条逻辑行可拆成多个 sub-row），`line_start_byte` / `line_len` 仍指当前 sub-row 在 buffer 中的绝对字节区间。
pub(crate) struct PrepaintedLine {
    /// 本行起始字节在整 buffer 中的绝对偏移。
    line_start_byte: usize,
    /// 本行内容字节长度（不含 `\n`）。
    line_len: usize,
    shaped: ShapedLine,
}

impl PrepaintedLine {
    /// 暴露给阶段 3 绘制的 shape 结果。其余阶段需要按行 byte→x 映射的，请走 [`LineMetric`]（在 paint 入口处由 prepaint.lines 构造）。
    pub(crate) fn shaped(&self) -> &ShapedLine {
        &self.shaped
    }
}

#[derive(Clone, Copy, Debug)]
struct VisualRow {
    line_index: u64,
    subrow: u64,
}

/// `prepaint` 阶段 shape 出的、供 `paint` 直接绘制的结果。
///
/// 字段按手册 19.4 的 6 阶段分组：每个阶段消费一个 `Vec<...>` 槽位。
/// v1 不接入的槽位（阶段 1 行背景、阶段 4 字符叠加、阶段 5 IME underline、阶段 6 装饰图标）固定为空 Vec；
/// 接入新装饰来源时只需在 prepaint 出口处往对应 Vec 推条目，paint 主干不变。
///
/// 颜色解析的位置：所有装饰来源遵循「语义键 + theme 解析」契约，**解析在[`highlight::compose`] 内一次性完成**；
/// prepaint 拿到的就是 `(range, Hsla)` 列表，paint 阶段只看几何与颜色，不再 if 路由"这条 range 来自 selection 还是 search"。
/// 详见 phases.rs 模块注释与 [`highlight`] 模块。
pub(crate) struct EditorPrepaint {
    // ── 阶段 1：行背景 ─────────────────────────────────────────────────────
    /// 当前为空 Vec；active line / diff hunk 等整段背景可接入这里。
    line_backgrounds: Vec<LineBackgroundQuad>,

    // ── 阶段 2：范围背景 ───────────────────────────────────────────────────
    /// composer 已切分、已按 priority 升序排好的 `Background` 装饰（selection / search / 未来 diagnostics / AI 提案等）。
    /// paint 顺序就是 Vec 顺序，低优先级先画、高优先级后画，alpha 叠加。
    range_backgrounds: Vec<(TextRange, Hsla)>,

    // ── 阶段 3：字符层 ─────────────────────────────────────────────────────
    /// 每行的 shape + 字节坐标信息，下标即 snapshot 内的视觉行号。
    lines: Vec<PrepaintedLine>,

    // ── 阶段 4：字符叠加 ───────────────────────────────────────────────────
    /// 当前为空 Vec；inlay hint / AI ghost text 接入点。
    glyph_overlays: Vec<GlyphOverlay>,

    // ── 阶段 5：caret + IME composition underline ─────────────────────────
    /// 落在视口内的 caret 的 `(视觉行号, 行内像素 x)`。下标与
    /// `EditorElement::selection.as_slice()` **不再一一对应**——视口外的 caret
    /// 已被剔除，避免在视区左上角画出"看不到的 caret 残影"。
    ///
    /// reveal / edge-scroll 只看 primary（在 prepaint 内部就消化掉了，paint
    /// 拿到的就是"全部 caret 都要画"的扁平集合）。
    carets: Vec<(usize, Pixels)>,
    /// primary caret 在 `carets` 中的下标，仅当 primary 在视口内时为 Some。
    /// 单行嵌入式编辑器永远在视口内——`None` 只可能出现在多行主编辑器，且
    /// 当 primary head 落在视口外。
    primary_caret: Option<usize>,
    /// 当前为空 Vec；IME marked text 接入点。
    composition_underlines: Vec<(TextRange, Hsla)>,

    // ── 阶段 6：gutter（行号 + 装饰图标）──────────────────────────────────
    /// gutter 列的 prepaint 产物——无 gutter 时为 [`gutter::Prepaint::disabled`]。
    /// 正文水平偏移由 `gutter.offset()` 返回，避免 element 重复持有几何字段。
    gutter: gutter::Prepaint,

    // ── 共享几何 ───────────────────────────────────────────────────────────
    line_height: Pixels,
    /// 当前滚动偏移；正文与光标按它整体平移。
    scroll: Point<Pixels>,
    /// `top` 已经吸收了 snapshot 上方视觉行 padding 的修正：phases 用
    /// `top + row_index × line_height` 即可拿到正确像素 y，无需知道视口起点。
    top_adjusted: Pixels,
    /// 编辑器鼠标命中区。
    mouse_hitbox: Hitbox,
    /// 当前内核是否允许纵向滚动。
    allows_vertical_scroll: bool,
}

/// 跨帧保留的 X 轴滚动偏移，存于 GPUI 元素状态。
/// Y 轴 top_line 由 view 层托管（见 `View::settle_viewport_y`），不再走元素状态。
#[derive(Default)]
struct EditorScroll {
    offset_x: Pixels,
    /// element 侧对 reveal 的 X 轴 dedupe；与 view 的 `last_applied_reveal_seq`
    /// 各管自己的轴，互不干扰。
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

fn resolve_visual_top(rows: &[VisualRow], top_line: u64, top_subrow: u64) -> usize {
    rows.iter()
        .position(|row| row.line_index == top_line && row.subrow == top_subrow)
        .or_else(|| {
            if top_subrow > 0 {
                rows.iter().rposition(|row| row.line_index == top_line)
            } else {
                None
            }
        })
        .or_else(|| rows.iter().position(|row| row.line_index >= top_line))
        .unwrap_or(0)
}

fn viewport_measurement_from_visual_top(
    rows: &[VisualRow],
    top_visual_row: usize,
    visible_visual_rows: u64,
) -> EditorViewportMeasurement {
    let visible_visual_rows = visible_visual_rows.max(1);
    if rows.get(top_visual_row).is_none() {
        return EditorViewportMeasurement {
            visible_visual_rows,
            visible_logical_lines: 1,
        };
    };

    let end = top_visual_row
        .saturating_add(visible_visual_rows as usize)
        .min(rows.len());
    let mut visible_logical_lines = 0_u64;
    let mut last_line = None;
    for row in &rows[top_visual_row..end] {
        if last_line != Some(row.line_index) {
            visible_logical_lines += 1;
            last_line = Some(row.line_index);
        }
    }

    EditorViewportMeasurement {
        visible_visual_rows,
        visible_logical_lines: visible_logical_lines.max(1),
    }
}

/// 把 byte 解析成 caret 在视口内的 `(row_index, x)`。
///
/// 默认规则：byte 同时落在两条视觉行（软换行边界）时，优先选择「下一段行首」—— 与 `WrapMap::resolve(None)` 一致。
/// `hint`（来自 view 的 visual_caret 缓存）若与某条视觉行的 (logical_line, subrow) 精确对得上，则尊重 hint，
/// 从而支持「LineEnd 的 caret 留在上一段末尾」这种连续上下移动语义。
fn caret_render_position(
    lines: &[PrepaintedLine],
    visual_rows: &[VisualRow],
    byte: usize,
    hint: Option<&VisualPosition>,
) -> Option<(usize, Pixels)> {
    let mut first_match: Option<(usize, Pixels)> = None;
    let mut preferred_start: Option<(usize, Pixels)> = None;
    let mut hint_match: Option<(usize, Pixels)> = None;

    for (index, line) in lines.iter().enumerate() {
        if byte < line.line_start_byte || byte > line.line_start_byte + line.line_len {
            continue;
        }
        let row = visual_rows[index];
        let offset = byte - line.line_start_byte;
        let x = px(f32::from(line.shaped.x_for_index(offset)));

        if let Some(h) = hint
            && h.byte.get() == byte
            && h.logical_line == row.line_index
            && h.subrow as u64 == row.subrow
        {
            hint_match = Some((index, x));
            break;
        }

        if first_match.is_none() {
            first_match = Some((index, x));
        }
        // 同 byte 出现在第二条（下段行首）：优先它。
        if byte == line.line_start_byte && row.subrow > 0 {
            preferred_start = Some((index, x));
        }
    }
    hint_match.or(preferred_start).or(first_match)
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
            // 主编辑区：撑满视口，内容溢出靠内部滚动偏移消化。
            style.size.height = relative(1.).into();
        } else {
            // 紧凑输入框：高度恰为一行。
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
        cx: &mut App,
    ) -> EditorPrepaint {
        let text_style = window.text_style();
        let font_size = layout.font_size;
        let line_height = layout.line_height;
        let has_gutter = self.has_gutter();
        let allows_vertical_scroll = self.kernel.allows_vertical_scroll();
        let soft_wrap = self.kernel.soft_wrap();
        // composer 把 Decoration 切分为 (foreground, background) 两条已解析色的列表（手册架构 §五）。
        // Decoration 集合 element 自己用完即弃，take 出来交给 composer 即可。
        let Composition {
            foreground: highlight_runs,
            background: range_backgrounds,
        } = compose(std::mem::take(&mut self.decorations));

        // SelectionSet 经过引擎归一化，as_slice() 已按 start 排序、互不重叠。
        // primary 在归一化前后由 primary_index 跟踪。
        // 这里 carets 的位置由 primary_caret 单独记录，下标与 selection.as_slice 已不再 1:1。
        let selections = self.selection.as_slice();
        let primary_sel_index = self
            .selection
            .primary_index()
            .min(selections.len().saturating_sub(1));

        // 软换行需要在 shape 之前知道正文区宽度（= bounds.width - gutter_offset）。
        // 这里先预算 gutter 偏移；真正的行号 shape 仍在拿到 sub-row 列表后再做（measure_offset 与 prepare 走同一 number_width 路径，数值上等价）。
        let gutter_offset_estimate = if has_gutter {
            gutter::measure_offset(self.total_lines, &text_style, font_size, window)
        } else {
            px(0.)
        };
        let text_viewport_w = clamp_px(
            bounds.size.width - gutter_offset_estimate,
            px(0.),
            bounds.size.width,
        );

        // 视觉行 -> 每个 selection 的 (row, x) 占位；None 表示视口外 / 暂未匹配上。
        // primary 的"视口外但要 reveal/edge-scroll"路径走 primary_caret_x 兜底。
        let mut carets_pos: Vec<Option<(usize, Pixels)>> = vec![None; selections.len()];
        let mut prepainted_lines: Vec<PrepaintedLine> = Vec::with_capacity(self.lines.len());
        let mut visual_rows: Vec<VisualRow> = Vec::with_capacity(self.lines.len());
        // 视觉模型：本帧测到的「逻辑行 → 行内相对字节断点」。
        // 未渲染过的行由 WrapMap 按 1 个 subrow 处理，避免每帧按整篇文档行数分配。
        // 命令层走 [`WrapMap::resolve`] 在文本域查询；未填充的行天然退化为「按逻辑行移动」。
        let mut breaks_per_line: Vec<(u64, Vec<u32>)> = Vec::with_capacity(self.lines.len());
        // 每条视觉行对应的「逻辑行号」：首段填 Some(line_index)，软换行的续段填 None。
        // 长度与 prepainted_lines 一一对应；不开软换行时全数组都是 Some(...)。
        let mut gutter_rows: Vec<Option<u64>> = Vec::with_capacity(self.lines.len());
        let mut primary_caret_visual_row: Option<usize> = None;

        // reveal 是否生效完全看快照里有没有；调用方不需要 reveal 时自然就不会在 owner.snapshot() 里填这个字段。
        let active_reveal = self.reveal;
        let reveal_byte = active_reveal.map(|hint| hint.byte);
        // reveal 目标若在视口内，shape 后能算出行内 x（用于 reveal x 轴摆位）。
        let mut reveal_visible_row_x: Option<(usize, Pixels)> = None;

        // 视口能装下多少视觉像素行——软换行下回写「逻辑行数」的判据。
        // 用 floor：只算「完整可见」的行。露半截的尾行不算入 edge-scroll 分母，
        // 否则光标走到那条半行时，settle 仍认为它在视口内而不滚，视觉上就被裁掉。
        let lh_safe: f32 = f32::from(line_height).max(1.0);
        let viewport_h_f: f32 = bounds.size.height.into();
        let visible_pixel_rows = if allows_vertical_scroll {
            ((viewport_h_f / lh_safe).floor() as usize).max(1)
        } else {
            1
        };

        for line in self.lines.iter() {
            let raw = line.text.as_str();
            let line_start = line.start_byte;
            let line_end = line_start + raw.len();

            // 每条逻辑行的 TextRun 切分——sub-row 都要参考、本行只算一次。
            //
            // 阶段 3 字符层：把行内字节区间按 composer 输出的前景 runs 切多段 TextRun。
            // 前景为空 → 兜底单 run（继承 text_style 字色）。
            let full_runs =
                build_text_runs_for_line(raw, line_start, line_end, &highlight_runs, &text_style);

            // 计算 sub-row 字节切分。
            // soft_wrap 关 → 单段 (0, len)。
            // soft_wrap 开 → 先 shape 一次全行用来测断点；
            // shape 结果只用于测量，sub-row 再单独 re-shape（GPUI 行布局缓存：同内容 + 同样式不付二次成本）。
            // 分行规则与 CJK 边界判定都内聚在 [`zom_workspace::view::compute_segments`]，渲染端只把测量结果（x_for_index）以闭包形式喂进去。
            let segments: Vec<(usize, usize)> = if soft_wrap {
                let measure_shaped = window.text_system().shape_line(
                    SharedString::from(raw.to_string()),
                    font_size,
                    &full_runs,
                    None,
                );
                zom_workspace::view::compute_segments(
                    raw,
                    f32::from(measure_shaped.width),
                    f32::from(text_viewport_w),
                    |byte| f32::from(measure_shaped.x_for_index(byte)),
                )
            } else {
                vec![(0, raw.len())]
            };

            // 收集本条逻辑行的视觉断点（行内相对字节，不含 0 与 line_len）。
            if soft_wrap {
                let breaks = segments
                    .iter()
                    .skip(1)
                    .map(|&(start, _)| start as u32)
                    .collect::<Vec<_>>();
                breaks_per_line.push((line.line_index, breaks));
            }

            for (seg_i, &(seg_start, seg_end)) in segments.iter().enumerate() {
                let abs_start = line_start + seg_start;
                let abs_end = line_start + seg_end;

                // 单段覆盖全行时复用 full_runs；多段时按 sub-row 切 runs
                // （build_text_runs_for_line 把 highlight_runs 夹到 sub-row 范围）。
                let (seg_text, sub_runs) = if seg_start == 0 && seg_end == raw.len() {
                    (raw, full_runs.clone())
                } else {
                    let st = &raw[seg_start..seg_end];
                    let rs = build_text_runs_for_line(
                        st,
                        abs_start,
                        abs_end,
                        &highlight_runs,
                        &text_style,
                    );
                    (st, rs)
                };
                let shaped = window.text_system().shape_line(
                    SharedString::from(seg_text.to_string()),
                    font_size,
                    &sub_runs,
                    None,
                );

                prepainted_lines.push(PrepaintedLine {
                    line_start_byte: abs_start,
                    line_len: seg_end - seg_start,
                    shaped,
                });
                visual_rows.push(VisualRow {
                    line_index: line.line_index,
                    subrow: seg_i as u64,
                });
                gutter_rows.push(if seg_i == 0 {
                    Some(line.line_index)
                } else {
                    None
                });
            }
        }

        for (i, sel) in selections.iter().enumerate() {
            let hint = (i == primary_sel_index)
                .then_some(self.visual_caret.as_ref())
                .flatten();
            if let Some((row, x)) =
                caret_render_position(&prepainted_lines, &visual_rows, sel.head().get(), hint)
            {
                carets_pos[i] = Some((row, x));
                if i == primary_sel_index {
                    primary_caret_visual_row = Some(row);
                }
            }
        }
        if let Some(rb) = reveal_byte
            && let Some((row, x)) = caret_render_position(&prepainted_lines, &visual_rows, rb, None)
        {
            reveal_visible_row_x = Some((row, x));
        }

        // gutter：按视觉行 shape 行号——续行槽位为 None，paint 时跳过。
        // 列宽仍由 buffer 总行数决定（避免滚动时列宽抖动）。
        let gutter_prepaint = if has_gutter {
            gutter::prepare(
                &gutter_rows,
                self.total_lines,
                &text_style,
                font_size,
                window,
            )
        } else {
            gutter::Prepaint::disabled()
        };
        let gutter_offset = gutter_prepaint.offset();

        // primary caret 的"行内 x"——X 轴 edge-scroll / reveal 用。
        // x 只有 primary 真的落在 shape 出来的视口行里才有。
        let primary_caret_x = carets_pos
            .get(primary_sel_index)
            .and_then(|slot| slot.map(|(_, x)| x))
            .unwrap_or(px(0.));

        let viewport_h = bounds.size.height;
        let viewport_w = clamp_px(bounds.size.width - gutter_offset, px(0.), bounds.size.width);
        let visible_visual_rows = if allows_vertical_scroll {
            (visible_pixel_rows as u64).max(1)
        } else {
            1
        };

        // 先用 view 当前 top（可能是上一帧的旧值）解析一个临时位置，
        // 仅用于构造 measurement——sync 钩子会立刻用本帧新 wrap_map 跑 settle 修正。
        let provisional_top_visual_row = resolve_visual_top(
            &visual_rows,
            self.top_line,
            if soft_wrap { self.top_subrow } else { 0 },
        );
        let measured_viewport = viewport_measurement_from_visual_top(
            &visual_rows,
            provisional_top_visual_row,
            visible_visual_rows,
        );

        // 同帧 settle：把本帧测出的 wrap_map / 可见行数推回 view，让 settle 用「新」wrap_map 而不是上一帧的旧 map 决定 top。
        // 返回值取代 self.top_line/top_subrow——这样「插入新行 / 触发新 sub-row」当帧 edge-scroll 就生效，不留一帧滞后。
        // 无 viewport_sync 的内核（单行嵌入框）或无活动 view 时退回 self 持有的 top。
        let settled_top = self.viewport_sync.as_ref().and_then(|sync| {
            let wrap_map = allows_vertical_scroll
                .then(|| WrapMap::sparse(soft_wrap, self.total_lines, breaks_per_line));
            sync(measured_viewport, wrap_map, cx)
        });
        let (effective_top_line, effective_top_subrow) = match settled_top {
            Some(SettledViewportTop {
                top_line,
                top_subrow,
            }) => (top_line, top_subrow),
            None => (self.top_line, self.top_subrow),
        };
        let top_visual_row = resolve_visual_top(
            &visual_rows,
            effective_top_line,
            if soft_wrap { effective_top_subrow } else { 0 },
        );

        // element 仅负责 X 轴：软换行下 X 恒 0（横向不溢出），关掉软换行才有 X reveal / edge-scroll。
        let scroll = match id {
            Some(global_id) => {
                let content_width = prepainted_lines.iter().fold(px(0.), |max, line| {
                    if line.shaped.width > max {
                        line.shaped.width
                    } else {
                        max
                    }
                });
                let reveal = active_reveal;
                let off_x =
                    window.with_element_state::<EditorScroll, _>(global_id, |state, _window| {
                        let mut state = state.unwrap_or_default();
                        if soft_wrap {
                            state.offset_x = px(0.);
                            return (px(0.), state);
                        }
                        let mut off = state.offset_x;

                        // === X 轴 reveal ===
                        // Y 轴 reveal 由 view.settle_viewport_y 吸收；这里只管横向。
                        // view 的 reveal 字段不会自动清空，element 仍按 seq 在本地 dedupe 防止反复摆位。
                        if let Some(req) = reveal
                            && Some(req.seq) != state.last_applied_reveal_seq
                        {
                            // reveal 目标行确实在视口里 shape 出了 x 时才能摆位；
                            // 否则保留当前 off，等下一帧该行进视口再校准。
                            if let Some((_, x)) = reveal_visible_row_x {
                                let target_right = x + px(CARET_WIDTH);
                                let visible_x = x >= off && target_right <= off + viewport_w;
                                let (placement_factor, force_scroll) = match req.kind {
                                    RevealKind::Match => (1.0 / 3.0, false),
                                    RevealKind::Jump => (1.0 / 3.0, true),
                                };
                                if force_scroll || !visible_x {
                                    off = x - viewport_w * placement_factor;
                                }
                            }
                            state.last_applied_reveal_seq = Some(req.seq);
                        }

                        // === X 轴 edge-scroll ===
                        // 光标列需可见。行尾光标位于最后一个字形「之后」。
                        // 可滚动宽度要在最宽行的基础上额外留出一个光标宽度，否则滚到行尾时光标正好压在正文裁剪边界上被切掉。
                        // primary caret 在视口外时 x = 0，不主动触发 x 滚动（再下一帧 caret 进视口、shape 出 x 后再校准）。
                        let scrollable_w = content_width + px(CARET_WIDTH);
                        let primary_caret_in_view = primary_caret_visual_row.is_some_and(|row| {
                            row >= top_visual_row
                                && row < top_visual_row.saturating_add(visible_pixel_rows)
                        });
                        if primary_caret_in_view {
                            let caret_right = primary_caret_x + px(CARET_WIDTH);
                            if primary_caret_x < off {
                                off = primary_caret_x;
                            } else if caret_right > off + viewport_w {
                                off = caret_right - viewport_w;
                            }
                        }
                        off = clamp_px(
                            off,
                            px(0.),
                            clamp_px(scrollable_w - viewport_w, px(0.), scrollable_w),
                        );

                        state.offset_x = off;
                        (off, state)
                    });
                Point::new(off_x, px(0.))
            }
            None => Point::new(px(0.), px(0.)),
        };

        // 仅留作上文 viewport_h_f 的语义来源，避免 unused-binding 警告。
        let _ = viewport_h;

        // top 已吸收 snapshot 上方 padding 的真实视觉行数修正。
        // phases 用 row_index 算 y 即可，**不需要再知道视口起点**。
        let top_adjusted = bounds.origin.y - line_height * top_visual_row as f32;

        let visible_start = top_visual_row;
        let visible_end = visible_start.saturating_add(visible_visual_rows as usize);
        let mut carets: Vec<(usize, Pixels)> = Vec::with_capacity(carets_pos.len());
        let mut primary_caret_in_view: Option<usize> = None;
        for (i, slot) in carets_pos.into_iter().enumerate() {
            if let Some((row, x)) = slot
                && row >= visible_start
                && row < visible_end
            {
                if i == primary_sel_index {
                    primary_caret_in_view = Some(carets.len());
                }
                carets.push((row, x));
            }
        }
        let mouse_hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);

        EditorPrepaint {
            // v1 空槽位：行背景。
            line_backgrounds: Vec::new(),
            // composer 已按 priority 排好；paint 顺序 = Vec 顺序，低优先级先画。
            range_backgrounds,
            lines: prepainted_lines,
            // v1 空槽位:字符叠加（ghost text / inlay hint）。
            glyph_overlays: Vec::new(),
            carets,
            primary_caret: primary_caret_in_view,
            // v1 空槽位：IME composition underline（marked text 待接入）。
            composition_underlines: Vec::new(),
            gutter: gutter_prepaint,
            line_height,
            scroll,
            top_adjusted,
            mouse_hitbox,
            allows_vertical_scroll,
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
        window.set_cursor_style(CursorStyle::IBeam, &prepaint.mouse_hitbox);
        let line_height = prepaint.line_height;
        let scroll = prepaint.scroll;
        if let (Some(selection_hook), Some(pointer_session)) =
            (self.selection_hook.clone(), self.pointer_session.clone())
        {
            let hitbox = prepaint.mouse_hitbox.clone();
            let hit_test = Rc::new(PointerHitTest::new(
                prepaint
                    .lines
                    .iter()
                    .map(|line| {
                        PointerHitLine::new(
                            line.line_start_byte,
                            line.line_len,
                            line.shaped.clone(),
                        )
                    })
                    .collect(),
                prepaint.gutter.offset(),
                prepaint.scroll,
                prepaint.line_height,
                prepaint.top_adjusted,
            ));
            install_selection_handlers(
                hitbox,
                bounds,
                hit_test,
                pointer_session,
                selection_hook,
                window,
            );
        }
        if prepaint.allows_vertical_scroll
            && let Some(scroll_hook) = self.scroll_hook.clone()
        {
            let hitbox = prepaint.mouse_hitbox.clone();
            install_scroll_handler(hitbox, line_height, scroll_hook, window);
        }
        // 纵向滚动 + 视口顶行修正都已吸收进 prepaint.top_adjusted；这里 phases 用 row_index × line_height 算 y 即可。
        let top = prepaint.top_adjusted;
        let gutter_offset = prepaint.gutter.offset();
        // 横向滚动只作用于正文与光标；行号列固定不动。
        let text_left = bounds.origin.x + gutter_offset - scroll.x;

        // 正文与光标裁剪到「正文区」：横向滚动时不溢出到行号列。
        let text_area = Bounds {
            origin: point(bounds.origin.x + gutter_offset, bounds.origin.y),
            size: size(
                clamp_px(bounds.size.width - gutter_offset, px(0.), bounds.size.width),
                bounds.size.height,
            ),
        };
        // caret 可见性与焦点态绑定：FocusHandle 决定本 view 是否"活动"；CaretClock决定闪烁相位；
        // 窗口失活时整窗不显光标，与系统其它编辑器一致。
        let caret_visible = self.focus.is_focused(window)
            && window.is_window_active()
            && CaretClock::is_visible(cx);
        // LineMetric 借用 ShapedLine：在 with_content_mask 闭包外构造一次，
        // 让阶段 2 / 4 / 5（IME underline 接入后）共用一份借用切片。
        let line_metrics: Vec<LineMetric<'_>> = prepaint
            .lines
            .iter()
            .map(|line| LineMetric {
                line_start_byte: line.line_start_byte,
                line_len: line.line_len,
                shaped: line.shaped(),
            })
            .collect();

        // ── 阶段 1～5：正文区（横向滚动会作用于此区域内的内容）─────────────
        window.with_content_mask(Some(ContentMask { bounds: text_area }), |window| {
            // 阶段 1：行背景（当前空 Vec 时无操作；active line / diff hunk 接入点）。
            paint_phase_1_line_backgrounds(
                &prepaint.line_backgrounds,
                text_area,
                text_left,
                top,
                line_height,
                window,
            );

            // 阶段 2：范围背景（selection / 未来 search / AI 区间）。
            paint_phase_2_range_backgrounds(
                &prepaint.range_backgrounds,
                &line_metrics,
                text_left,
                top,
                line_height,
                text_area,
                window,
            );

            // 阶段 3：字符层。
            paint_phase_3_glyphs(&prepaint.lines, text_left, top, line_height, window, cx);

            // 阶段 4：字符叠加（当前空 Vec 时无操作；inlay hint / ghost text 接入点）。
            paint_phase_4_glyph_overlays(
                &prepaint.glyph_overlays,
                &line_metrics,
                text_left,
                top,
                line_height,
                window,
                cx,
            );

            // 阶段 5：caret + IME composition underline。
            paint_phase_5_carets_and_composition(
                &prepaint.carets,
                caret_visible,
                px(CARET_WIDTH),
                caret_color(),
                &prepaint.composition_underlines,
                &line_metrics,
                text_left,
                top,
                line_height,
                text_area,
                window,
            );
        });

        // ── 阶段 6：gutter 列（横向固定在 bounds 左缘，纵向随正文滚动）─────
        gutter::paint(
            &prepaint.gutter,
            bounds.origin.x,
            bounds.origin.y,
            top,
            bounds.size.height,
            line_height,
            window,
            cx,
        );

        // 把 primary caret 在 element 内的相对坐标打包给 input hook。
        // 系统 IME 的 `bounds_for_range` 会借此把候选窗放到 caret 正下方。
        // primary 不在视口内时传 None，让 IME 走默认（候选窗落在窗口角，明显比"贴在编辑区左上角"易辨认是哪里有问题）。
        let caret_layout = prepaint.primary_caret.and_then(|idx| {
            let (caret_row, caret_x) = *prepaint.carets.get(idx)?;
            Some(CaretLayout {
                relative: point(
                    gutter_offset + caret_x - scroll.x,
                    // 阶段 5 caret 画在 `top_adjusted + row × line_height`。
                    // 这里换回 `bounds.origin.y` 相对坐标即 `top - bounds.y + row × line_height`。
                    prepaint.top_adjusted - bounds.origin.y + line_height * caret_row as f32,
                ),
                line_height,
            })
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

/// 把一行文本按前景 runs 切成多段 [`TextRun`]。
///
/// `raw` 是该行不含换行的文本；`line_start` / `line_end` 是该行在整 buffer
/// 中的绝对字节区间。`highlight_runs` 是 composer 输出的前景列（已解析为
/// `Hsla`，按 start 升序、互不重叠——v1 唯一前景 producer 是 syntax）。
///
/// 输出契约：所有 TextRun 的 `len` 之和必须严格等于 `raw.len()`，否则 GPUI
/// `shape_line` 会少绘 / 越界。函数内部用 cursor 推进保证不漏不重。
///
/// 边界处理：
/// - `raw` 为空 → 返回 `Vec::new()`（shape_line 接受空 runs）。
/// - `highlight_runs` 为空 → 兜底单 run（继承 text_style 字色）。
/// - 与本行不相交的 run → 跳过。
/// - 字节偏移到 UTF-8 边界由上游保证（tree-sitter 产物天然字节对齐；行边界
///   是换行符之后的字节，不会切到 char 中间）。
fn build_text_runs_for_line(
    raw: &str,
    line_start: usize,
    line_end: usize,
    highlight_runs: &[(TextRange, Hsla)],
    text_style: &gpui::TextStyle,
) -> Vec<TextRun> {
    if raw.is_empty() {
        return Vec::new();
    }
    if highlight_runs.is_empty() {
        return vec![text_style.to_run(raw.len())];
    }

    let mut runs: Vec<TextRun> = Vec::new();
    // cursor 用整 buffer 的绝对字节坐标推进；每次写 run 时换算成行内长度。
    let mut cursor = line_start;

    for (range, color) in highlight_runs {
        let r_start = range.start().get();
        let r_end = range.end().get();
        // 完全在本行之前 → 跳过；完全在本行之后 → 后续都跳过。
        if r_end <= line_start {
            continue;
        }
        if r_start >= line_end {
            break;
        }
        // 与本行有交集——夹到行内。
        let span_start = r_start.max(cursor).max(line_start);
        let span_end = r_end.min(line_end);
        if span_end <= span_start {
            continue;
        }
        let Some((span_start, span_end)) =
            clamp_span_to_char_boundaries(raw, line_start, span_start, span_end)
        else {
            continue;
        };
        // 行内未着色段：cursor..span_start。
        if span_start > cursor {
            runs.push(text_style.to_run(span_start - cursor));
        }
        // 着色段：用 highlight 色。font / 行高继承 text_style，仅替换 color。
        runs.push(TextRun {
            len: span_end - span_start,
            font: text_style.font(),
            color: *color,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
        cursor = span_end;
    }

    // 行尾未着色段。
    if cursor < line_end {
        runs.push(text_style.to_run(line_end - cursor));
    }

    runs
}

fn clamp_span_to_char_boundaries(
    raw: &str,
    line_start: usize,
    span_start: usize,
    span_end: usize,
) -> Option<(usize, usize)> {
    let rel_start = span_start.checked_sub(line_start)?;
    let rel_end = span_end.checked_sub(line_start)?;
    let aligned_start = next_char_boundary(raw, rel_start)?;
    let aligned_end = previous_char_boundary(raw, rel_end)?;

    if aligned_end <= aligned_start {
        return None;
    }

    Some((line_start + aligned_start, line_start + aligned_end))
}

fn next_char_boundary(text: &str, offset: usize) -> Option<usize> {
    if offset > text.len() {
        return None;
    }
    if text.is_char_boundary(offset) {
        return Some(offset);
    }
    ((offset + 1)..=text.len()).find(|&candidate| text.is_char_boundary(candidate))
}

fn previous_char_boundary(text: &str, offset: usize) -> Option<usize> {
    if offset > text.len() {
        return None;
    }
    if text.is_char_boundary(offset) {
        return Some(offset);
    }
    (0..offset)
        .rev()
        .find(|&candidate| text.is_char_boundary(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TextStyle, white};

    fn style() -> TextStyle {
        TextStyle {
            color: white(),
            ..Default::default()
        }
    }

    fn rng(a: usize, b: usize) -> TextRange {
        TextRange::new(
            zom_engine::ByteOffset::new(a),
            zom_engine::ByteOffset::new(b),
        )
        .unwrap()
    }

    fn color() -> Hsla {
        gpui::red()
    }

    fn lens(runs: &[TextRun]) -> Vec<usize> {
        runs.iter().map(|r| r.len).collect()
    }

    #[test]
    fn empty_line_returns_no_runs() {
        let runs = build_text_runs_for_line("", 0, 0, &[], &style());
        assert!(runs.is_empty());
    }

    #[test]
    fn no_highlights_falls_back_to_single_run() {
        let runs = build_text_runs_for_line("hello", 0, 5, &[], &style());
        assert_eq!(lens(&runs), vec![5]);
    }

    #[test]
    fn highlight_covers_entire_line() {
        let runs = build_text_runs_for_line("hello", 0, 5, &[(rng(0, 5), color())], &style());
        assert_eq!(lens(&runs), vec![5]);
        assert_eq!(runs[0].color, color());
    }

    #[test]
    fn highlight_inside_line_splits_three_runs() {
        // "hello" 中 `ell` 被染色。
        let runs = build_text_runs_for_line("hello", 0, 5, &[(rng(1, 4), color())], &style());
        assert_eq!(lens(&runs), vec![1, 3, 1]);
        assert_eq!(runs[1].color, color());
    }

    #[test]
    fn highlight_inside_multibyte_character_should_be_dropped_before_gpui_shape() {
        let runs = build_text_runs_for_line("语言", 0, 6, &[(rng(0, 2), color())], &style());

        assert_eq!(lens(&runs), vec![6]);
        assert_eq!(runs[0].color, style().color);
    }

    #[test]
    fn highlight_on_multibyte_character_boundary_should_keep_byte_lengths() {
        let runs = build_text_runs_for_line("语言", 0, 6, &[(rng(0, 3), color())], &style());

        assert_eq!(lens(&runs), vec![3, 3]);
        assert_eq!(runs[0].color, color());
    }

    #[test]
    fn soft_wrap_subrow_should_rebase_absolute_highlight_ranges_to_segment_text() {
        let line = "- **语言**：中文是第一语言";
        let segment = &line[4..];
        let runs = build_text_runs_for_line(
            segment,
            4,
            line.len(),
            &[
                (rng(0, 2), color()),
                (rng(2, 3), color()),
                (rng(3, 4), color()),
                (rng(4, 10), color()),
                (rng(10, 11), color()),
                (rng(11, 12), color()),
            ],
            &style(),
        );

        assert_eq!(lens(&runs)[0], "语言".len());
        let total: usize = runs.iter().map(|run| run.len).sum();
        assert_eq!(total, segment.len());
    }

    #[test]
    fn highlights_outside_line_are_skipped() {
        let runs = build_text_runs_for_line(
            "hello",
            10,
            15,
            &[(rng(0, 5), color()), (rng(20, 25), color())],
            &style(),
        );
        assert_eq!(lens(&runs), vec![5]);
    }

    #[test]
    fn run_lengths_sum_to_line_length() {
        let runs = build_text_runs_for_line(
            "abcdefghij",
            0,
            10,
            &[
                (rng(1, 3), color()),
                (rng(5, 7), color()),
                (rng(9, 10), color()),
            ],
            &style(),
        );
        let total: usize = runs.iter().map(|r| r.len).sum();
        assert_eq!(total, 10);
    }

    #[test]
    fn visual_top_resolves_soft_wrap_subrow() {
        let rows = [
            VisualRow {
                line_index: 10,
                subrow: 0,
            },
            VisualRow {
                line_index: 10,
                subrow: 1,
            },
            VisualRow {
                line_index: 11,
                subrow: 0,
            },
        ];

        assert_eq!(resolve_visual_top(&rows, 10, 1), 1);
    }

    #[test]
    fn viewport_measurement_counts_logical_lines_from_visual_top() {
        let rows = [
            VisualRow {
                line_index: 10,
                subrow: 0,
            },
            VisualRow {
                line_index: 10,
                subrow: 1,
            },
            VisualRow {
                line_index: 10,
                subrow: 2,
            },
            VisualRow {
                line_index: 11,
                subrow: 0,
            },
        ];

        let measurement = viewport_measurement_from_visual_top(&rows, 1, 2);

        assert_eq!(measurement.visible_visual_rows, 2);
        assert_eq!(measurement.visible_logical_lines, 1);
    }

    // 软换行边界处 caret 选段、affinity 翻转等语义由 zom-engine `WrapMap` 与
    // zom-command `visual_movement` 直接覆盖；element 侧的 [`caret_render_position`]
    // 要构造真实的 `ShapedLine`，依赖运行时 Window，因此放到集成层验证。
}
