//! 可嵌入编辑器渲染图元 —— 唯一的编辑器实现（借鉴 Zed 的 `EditorElement`）。
//!
//! 单行输入框与主编辑区不是两种编辑器，而是同一个
//! [`EditorKernel`](crate::shell::editor::EditorKernel) 按能力开关创建出的两个形态；本文件只
//! 消费内核传下来的能力开关并完成 GPUI 绘制。
//!
//! 文本与光标分层绘制：每帧把视口内每行 shape 成 [`ShapedLine`]，文本只随
//! 内容变化，光标只是一个独立填充矩形 —— 移动光标不触发文本重排，这是
//! 「不闪烁」的根因。
//!
//! 视口切片：snapshot 已经按 view 当前 `(top_line, visible_line_count)` 给
//! 出一段 `SnapshotLine[]`，element 只 shape 这一段；`total_lines` 决定
//! `content_height`，从而支持 GB 级文件不爆显存。prepaint 末尾根据 bounds /
//! line_height 反算实际可见行数 + 顶行，回写 view 的 `ViewportState`，下一帧
//! 的 snapshot 据此切；首帧由 main_editor 的 `DEFAULT_VISIBLE_LINES` 兜底。
//!
//! 软换行：开关由 [`EditorKernel::soft_wrap`] 控制，运行时可切换。开启后每条
//! 逻辑行可能被拆成多条「视觉行」（sub-row），prepaint 阶段在 shape 完一次
//! 全行后用 [`compute_wrap_segments`] 按视口宽度算出断点字节列表，再为每
//! 段 sub-row 重 shape。下游 phases / gutter 一律按视觉行索引消费——只要
//! `PrepaintedLine` 的 `line_start_byte` / `line_len` 是 sub-row 级别的字节
//! 边界，原有路径自然兼容。软换行打开时禁用横向滚动，viewport_sync 回写
//! 的不是视觉行数而是「下一帧切多少条逻辑行就够铺满视口」。
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
    TextRun, Window, point, px, relative, size,
};

use zom_engine::{SelectionSet, TextRange};
use zom_view::RevealKind;

use crate::shell::shared::theme::color;

use crate::shell::editor::highlight::{self, Composition, Decoration};
use crate::shell::editor::input::CaretLayout;
use crate::shell::editor::kernel::EditorKernel;
use crate::shell::editor::snapshot::{RevealHint, SnapshotLine};

use super::blink::CaretClock;
use super::gutter;
use super::input_host::{EditorInputHook, EditorPaintInfo, EditorViewportSyncHook};
use super::phases::{
    GlyphOverlay, LineBackgroundQuad, LineMetric, paint_phase_1_line_backgrounds,
    paint_phase_2_range_backgrounds, paint_phase_3_glyphs, paint_phase_4_glyph_overlays,
    paint_phase_5_carets_and_composition,
};

/// 光标竖条宽度。2px 与 VS Code / Zed 默认一致——1px 在高分屏上偏细。
const CARET_WIDTH: f32 = 2.0;

/// 光标竖条颜色 —— 编辑器自持的视觉角色，不随嵌入处而变。
fn caret_color() -> Hsla {
    color::blue::s07().into()
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
    /// snapshot 切片对应的顶行（0-based 逻辑行号）；首条 `lines[0].line_index`
    /// 等于该值，但 lines 为空时仍需要它，独立带一份。
    viewport_start_line: u64,
    /// view 落定的视口顶行（0-based）；与 `viewport_start_line` 的区别见
    /// [`EditorSnapshot::top_line`]。element 用它直接算 `off.y`，不再反算。
    top_line: u64,
    /// 完整选区集合。每个 selection 的 head 各画一个 caret（阶段 5）；reveal /
    /// edge-scroll 只看 primary。`SelectionSet::Clone` 是 O(1)（内部 Arc），元素
    /// 按帧重建。
    ///
    /// **非空 selection 的 range 已作为 `Background` Decoration 由 snapshot 构造
    /// 端推入 [`Self::decorations`]**（手册架构 §三 把选区列为独立 producer），
    /// 本字段只剩 caret 几何用途；范围背景由 composer 与 syntax / search 等
    /// 一起合成。
    selection: SelectionSet,
    focus: FocusHandle,
    input_handler_hook: EditorInputHook,
    /// 跨帧滚动偏移的状态键。每个编辑器实例都应给一个稳定 id。
    element_id: Option<ElementId>,
    /// 外部 reveal 请求；按 seq 触发一次 reveal 路径。
    reveal: Option<RevealHint>,
    /// 高亮装饰集合——syntax / selection / search 等 producer 的统一产物
    /// （手册《桌面端高亮架构》§四）。prepaint 调
    /// [`highlight::compose`] 切分为前景 / 背景，分别喂给阶段 3 / 阶段 2。
    decorations: Vec<Decoration>,
    /// prepaint 末尾调用，把当前帧测得的 (top_line, visible_line_count) 写回
    /// view 的 ViewportState；只主编辑区装。
    viewport_sync: Option<EditorViewportSyncHook>,
}

impl EditorElement {
    pub(crate) fn new(
        kernel: EditorKernel,
        lines: Vec<SnapshotLine>,
        total_lines: u64,
        viewport_start_line: u64,
        top_line: u64,
        selection: SelectionSet,
        focus: FocusHandle,
        input_handler_hook: EditorInputHook,
    ) -> Self {
        Self {
            kernel,
            lines,
            total_lines,
            viewport_start_line,
            top_line,
            selection,
            focus,
            input_handler_hook,
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
    /// 输入要求：每个 producer 自身保证内部 range 不重叠；跨 producer 允许
    /// Background 重叠（alpha 叠加表达层叠语义）。空 Vec = 无装饰，前景退到
    /// 继承的 text_style 单 run，背景全无。
    pub(crate) fn decorations(mut self, decorations: Vec<Decoration>) -> Self {
        self.decorations = decorations;
        self
    }

    pub(crate) fn viewport_sync(mut self, hook: EditorViewportSyncHook) -> Self {
        self.viewport_sync = Some(hook);
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
/// 注意：从视口切片接入起，`line_start_byte` 是该行在整 buffer 中的**绝对**
/// 字节偏移，而非 element 内某段文本的局部偏移——selection / search 命中的
/// `TextRange` 也是绝对 byte，二者天然对齐。开启软换行后，本结构表示**视觉
/// 行**（一条逻辑行可拆成多个 sub-row），`line_start_byte` / `line_len`
/// 仍指当前 sub-row 在 buffer 中的绝对字节区间。
pub(crate) struct PrepaintedLine {
    /// 本行起始字节在整 buffer 中的绝对偏移。
    line_start_byte: usize,
    /// 本行内容字节长度（不含 `\n`）。
    line_len: usize,
    shaped: ShapedLine,
}

impl PrepaintedLine {
    /// 暴露给阶段 3 绘制的 shape 结果。其余阶段需要按行 byte→x 映射的，请走
    /// [`LineMetric`]（在 paint 入口处由 prepaint.lines 构造）。
    pub(crate) fn shaped(&self) -> &ShapedLine {
        &self.shaped
    }
}

/// `prepaint` 阶段 shape 出的、供 `paint` 直接绘制的结果。
///
/// 字段按手册 19.4 的 6 阶段分组：每个阶段消费一个 `Vec<...>` 槽位。
/// v1 不接入的槽位（阶段 1 行背景、阶段 4 字符叠加、阶段 5 IME underline、阶段 6 装饰图标）固定为空 Vec；
/// 接入新装饰来源时只需在 prepaint 出口处往对应 Vec 推条目，paint 主干不变。
///
/// 颜色解析的位置：所有装饰来源遵循「语义键 + theme 解析」契约，**解析在
/// [`highlight::compose`] 内一次性完成**；prepaint 拿到的就是 `(range, Hsla)`
/// 列表，paint 阶段只看几何与颜色，不再 if 路由"这条 range 来自 selection 还是
/// search"。详见 phases.rs 模块注释与 [`highlight`] 模块。
pub(crate) struct EditorPrepaint {
    // ── 阶段 1：行背景 ─────────────────────────────────────────────────────
    /// 当前为空 Vec；active line / diff hunk 等整段背景可接入这里。
    line_backgrounds: Vec<LineBackgroundQuad>,

    // ── 阶段 2：范围背景 ───────────────────────────────────────────────────
    /// composer 已切分、已按 priority 升序排好的 `Background` 装饰
    /// （selection / search / 未来 diagnostics / AI 提案等）。paint 顺序就是
    /// Vec 顺序，低优先级先画、高优先级后画，alpha 叠加。
    range_backgrounds: Vec<(TextRange, Hsla)>,

    // ── 阶段 3：字符层 ─────────────────────────────────────────────────────
    /// 每行的 shape + 字节坐标信息，下标即视觉行号（= line_index - viewport_start_line）。
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
    /// `top` 已经吸收了 `viewport_start_line × line_height` 的修正：phases 用
    /// `top + row_index × line_height` 即可拿到正确像素 y，无需知道视口起点。
    top_adjusted: Pixels,
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
        let viewport_start_line = self.viewport_start_line;

        // composer 把 Decoration 切分为 (foreground, background) 两条已解析色的列表（手册架构 §五）。
        // Decoration 集合 element 自己用完即弃，take 出来交给 composer 即可。
        let Composition {
            foreground: highlight_runs,
            background: range_backgrounds,
        } = highlight::compose(std::mem::take(&mut self.decorations));

        // SelectionSet 经过引擎归一化，as_slice() 已按 start 排序、互不重叠。
        // primary 在归一化前后由 primary_index 跟踪。
        // 这里 carets 的位置由 primary_caret 单独记录，下标与 selection.as_slice 已不再 1:1。
        let selections = self.selection.as_slice();
        let primary_sel_index = self
            .selection
            .primary_index()
            .min(selections.len().saturating_sub(1));

        // 软换行需要在 shape 之前知道正文区宽度（= bounds.width - gutter_offset）。
        // 这里先预算 gutter 偏移；真正的行号 shape 仍在拿到 sub-row 列表后再做
        // （measure_offset 与 prepare 走同一 number_width 路径，数值上等价）。
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
        // 每条视觉行对应的「逻辑行号」：首段填 Some(line_index)，软换行的续段填 None。
        // 长度与 prepainted_lines 一一对应；不开软换行时全数组都是 Some(...)。
        let mut gutter_rows: Vec<Option<u64>> = Vec::with_capacity(self.lines.len());

        // reveal 是否生效完全看快照里有没有；调用方不需要 reveal 时自然就不会在 owner.snapshot() 里填这个字段。
        let active_reveal = self.reveal;
        let reveal_byte = active_reveal.map(|hint| hint.byte);
        // reveal 目标若在视口内，shape 后能算出行内 x（用于 reveal x 轴摆位）。
        let mut reveal_visible_row_x: Option<(usize, Pixels)> = None;

        // 视口能装下多少视觉像素行——软换行下回写「逻辑行数」的判据。
        let lh_safe: f32 = f32::from(line_height).max(1.0);
        let viewport_h_f: f32 = bounds.size.height.into();
        let visible_pixel_rows = if allows_vertical_scroll {
            (viewport_h_f / lh_safe).ceil() as usize
        } else {
            1
        };
        // 累计已铺的视觉行数；首次 ≥ visible_pixel_rows 时停止累加。
        // 软换行下，该计数就是「下一帧切多少条逻辑行就够铺满视口」的下界。
        let mut logical_lines_to_cover: u64 = 0;
        let mut cover_satisfied = false;

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
            // soft_wrap 关 → 单段 (0, len)，与旧路径完全等价。
            // soft_wrap 开 → 先 shape 一次全行用来测断点；shape 结果只用于测量，
            //   sub-row 再单独 re-shape（GPUI 行布局缓存：同内容 + 同样式不付二次成本）。
            let segments: Vec<(usize, usize)> = if soft_wrap {
                let measure_shaped = window.text_system().shape_line(
                    SharedString::from(raw.to_string()),
                    font_size,
                    &full_runs,
                    None,
                );
                compute_wrap_segments(raw, &measure_shaped, text_viewport_w)
            } else {
                vec![(0, raw.len())]
            };

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

                let visual_row = prepainted_lines.len();

                // caret 命中规则与旧路径一致："head ≤ abs_end" 归到当前 sub-row。
                // 软换行边界 (head == abs_end == 下一 sub-row.abs_start)：当前
                // sub-row 先把 carets_pos[i] 写满，后续 sub-row 因 is_some() 短路。
                // caret 显示在 sub-row 末——与「行尾光标」一致。
                for (i, sel) in selections.iter().enumerate() {
                    if carets_pos[i].is_some() {
                        continue;
                    }
                    let head = sel.head().get();
                    if head >= abs_start && head <= abs_end {
                        carets_pos[i] = Some((visual_row, shaped.x_for_index(head - abs_start)));
                    }
                }
                if let Some(rb) = reveal_byte
                    && reveal_visible_row_x.is_none()
                    && rb >= abs_start
                    && rb <= abs_end
                {
                    reveal_visible_row_x = Some((visual_row, shaped.x_for_index(rb - abs_start)));
                }

                prepainted_lines.push(PrepaintedLine {
                    line_start_byte: abs_start,
                    line_len: seg_end - seg_start,
                    shaped,
                });
                gutter_rows.push(if seg_i == 0 {
                    Some(line.line_index)
                } else {
                    None
                });
            }

            // 处理完这条逻辑行：若视口尚未铺满，把它计入待覆盖数。
            if !cover_satisfied {
                logical_lines_to_cover += 1;
                if prepainted_lines.len() >= visible_pixel_rows {
                    cover_satisfied = true;
                }
            }
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

        // 视口外的 caret 直接丢掉，避免被 unwrap_or((0, 0)) 摆到视区左上角形成残影。
        // 多行主编辑区滚到不见 primary 时也只剩边缘指示。
        let mut carets: Vec<(usize, Pixels)> = Vec::with_capacity(carets_pos.len());
        let mut primary_caret_in_view: Option<usize> = None;
        for (i, slot) in carets_pos.into_iter().enumerate() {
            if let Some(entry) = slot {
                if i == primary_sel_index {
                    primary_caret_in_view = Some(carets.len());
                }
                carets.push(entry);
            }
        }

        // primary caret 的"行内 x"——X 轴 edge-scroll / reveal 用。
        // x 只有 primary 真的落在 shape 出来的视口行里才有。
        // 行号用于 Y 轴 edge-scroll，已搬到 view 层 `settle_viewport_y` 处理，本处不再需要。
        let primary_caret_x = primary_caret_in_view
            .map(|idx| carets[idx].1)
            .unwrap_or(px(0.));

        // === Y 轴：view 已在 settle_viewport_y 里落定好 top_line ===
        //
        // 不再在本帧反算 / 写回 top_line：view 是 Y 轴真源。
        // reveal Y 摆位与 edge-scroll 都在 `View::settle_viewport_y` 里跑过了（由 slot::embed 在 snapshot 之前触发）。
        // 本帧 self.top_line 就是要画的视口顶行。
        //
        // === X 轴：仍是像素级，element 内部 with_element_state 跨帧持 off.x ===
        //
        // X 轴依赖字体度量（shape 出来的字形宽度），无法在 view 层（按行号粒度）表达。
        // reveal X 摆位与 caret 列 edge-scroll 都留在这里。`off.y` 直接由 self.top_line 推导。
        //
        // 软换行打开时整条横向滚动路径都失效——内容已经按视口宽度断行，
        // off.x 强制 0、reveal / edge-scroll 直接跳过。
        let viewport_h = bounds.size.height;
        let viewport_w = clamp_px(bounds.size.width - gutter_offset, px(0.), bounds.size.width);
        let derived_off_y = if allows_vertical_scroll {
            line_height * self.top_line as f32
        } else {
            px(0.)
        };

        let (scroll, measured_visible_lines) = match id {
            Some(global_id) => {
                let content_width = prepainted_lines.iter().fold(px(0.), |max, line| {
                    if line.shaped.width > max {
                        line.shaped.width
                    } else {
                        max
                    }
                });
                let reveal = active_reveal;
                let result =
                    window.with_element_state::<EditorScroll, _>(global_id, |state, _window| {
                        let mut state = state.unwrap_or_default();
                        let mut off = if soft_wrap { px(0.) } else { state.offset_x };

                        if !soft_wrap {
                            // === X 轴 reveal ===
                            // 仅处理横向；Y 轴 reveal 由 view.settle_viewport_y 吸收。
                            // 仍然按 seq 在 element 侧 dedupe —— view 的 reveal 字段不会自动清空。
                            // element 自己记 last_applied_reveal_seq 防重。
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
                            if primary_caret_in_view.is_some() {
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
                        } else {
                            // 软换行下：把 reveal seq 视为已消费（避免开关切换瞬间的旧 seq 被
                            // 重新应用），但 off 保持 0 不动。
                            if let Some(req) = reveal {
                                state.last_applied_reveal_seq = Some(req.seq);
                            }
                        }

                        state.offset_x = off;

                        // 测量 visible_line_count：软换行下回写「逻辑行数」，否则回写视觉行数（= 像素行数）。
                        // 不开软换行时二者等价；开启后两者不一致，必须用逻辑行数让下一帧切片对齐。
                        let visible = if !allows_vertical_scroll {
                            1
                        } else if soft_wrap {
                            logical_lines_to_cover.max(1)
                        } else {
                            visible_pixel_rows as u64
                        };
                        ((off, visible), state)
                    });
                let (off_x, visible) = result;
                (Point::new(off_x, derived_off_y), visible)
            }
            None => (Point::new(px(0.), derived_off_y), self.lines.len() as u64),
        };

        let _ = viewport_h; // 仅留作上文 viewport_h_f 的语义来源，避免 unused-binding 警告。

        // 把测得的 visible_line_count 推回 view，让下一帧 snapshot 切片用更准的行数。
        // top_line 已经由 settle 落定，sync 不再写它。
        if let Some(sync) = self.viewport_sync.as_ref() {
            sync(measured_visible_lines, cx);
        }

        // top 已吸收 viewport_start_line × line_height 的修正：visual_row 0 在物理 y 上对应逻辑行 viewport_start_line。
        // phases 用 row_index 算 y 即可，**不需要再知道视口起点**。
        let top_adjusted = bounds.origin.y - scroll.y + line_height * viewport_start_line as f32;

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

/// 软换行 MVP 断行算法：
/// 按字节遍历 `raw`，用全行 [`ShapedLine`] 的 [`ShapedLine::x_for_index`] 取每个字符的累计像素位置；
/// 当 sub-row 宽度即将超过 `viewport_w` 时，回退到最近的「合法断点」断；找不到就在当前字符位置硬断。
///
/// **合法断点**有两种（参考 UAX #14 的实用子集）：
/// - **空白后**：任何空白字符之后的位置——空白会留在前一 sub-row 末尾。
/// - **CJK 边界**：CJK 字符与任何字符之间的位置（CJK-CJK / CJK-ASCII / ASCII-CJK）。
///   CJK 之间没有空白也要能断，否则一长串中文遇到行尾会被整段挤到下一行，
///   留出大块空白——视觉上「明明右边还有位置为啥换了」的来源。
///
/// 返回 sub-row 的字节区间列表 `[(start, end), ...]`，覆盖 `[0, raw.len())`，互不重叠且按顺序排列；
/// 不开软换行 / 整行能放下 / `viewport_w <= 0` 时直接返回 `[(0, raw.len())]`。
///
/// 不变量：
/// - 单个字符比 `viewport_w` 还宽时，仍把这个字符放入当前 sub-row（避免产生空段死循环）；下一个字符再触发断行。
/// - 边界（断点字节）总落在 UTF-8 字符边界——本函数只在字符边界处记录断点。
/// - `raw == ""` 返回 `[(0, 0)]`，与上游 build_text_runs_for_line 的空行约定一致。
fn compute_wrap_segments(
    raw: &str,
    full_shaped: &ShapedLine,
    viewport_w: Pixels,
) -> Vec<(usize, usize)> {
    if raw.is_empty() {
        return vec![(0, 0)];
    }
    if viewport_w <= px(0.) {
        return vec![(0, raw.len())];
    }
    if full_shaped.width <= viewport_w {
        return vec![(0, raw.len())];
    }

    let mut segments: Vec<(usize, usize)> = Vec::new();
    let mut current_start: usize = 0;
    let mut current_start_x = full_shaped.x_for_index(0);
    // 最近一个「合法断点」的字节位置——溢出时把它当成 sub-row 切点。
    // 空白后的合法断点 = 空白字符之后的字节；CJK 边界的合法断点 = 当前字节。
    let mut last_break: Option<usize> = None;
    // 前一字符是不是 CJK——用于判定「CJK / 非-CJK」切换处也是断点。
    let mut prev_was_cjk = false;

    let bytes = raw.as_bytes();
    let mut byte: usize = 0;
    while byte < raw.len() {
        // 推进到下一个 UTF-8 字符边界。
        let mut next = byte + 1;
        while next < raw.len() && (bytes[next] & 0xC0) == 0x80 {
            next += 1;
        }

        let ch = raw[byte..next].chars().next().unwrap_or(' ');
        let curr_is_cjk = is_cjk_break_candidate(ch);

        // CJK 边界断点：当前位置（byte）介于「前 CJK / 当前 CJK」之间，
        // 或「前非-CJK / 当前 CJK」、「前 CJK / 当前非-CJK」之间。
        // 任一侧是 CJK 就视为可断——位置 = byte（CJK 字符去新 sub-row）。
        if curr_is_cjk || prev_was_cjk {
            last_break = Some(byte);
        }

        let x_next = full_shaped.x_for_index(next);
        let segment_w = x_next - current_start_x;

        if segment_w > viewport_w && byte > current_start {
            // 当前 sub-row 加上下一个字符会溢出——需要断。
            // 优先 last_break；否则在当前位置硬断。
            let break_at = match last_break {
                Some(p) if p > current_start && p <= byte => p,
                _ => byte,
            };
            segments.push((current_start, break_at));
            current_start = break_at;
            current_start_x = full_shaped.x_for_index(break_at);
            last_break = None;
            prev_was_cjk = false;
            // 不推进 byte——本字符重新进入下一 sub-row。
            continue;
        }

        // 空白后的合法断点（位置 = next，空白留在前一 sub-row 末尾）。
        if ch.is_whitespace() {
            last_break = Some(next);
        }

        prev_was_cjk = curr_is_cjk;
        byte = next;
    }

    if current_start < raw.len() {
        segments.push((current_start, raw.len()));
    }
    if segments.is_empty() {
        segments.push((0, raw.len()));
    }
    segments
}

/// 是否是「东亚正文字符」——把它两侧都视为软换行的合法断点。
///
/// 覆盖：
/// - CJK 符号与标点（U+3000-303F）：含全角空格、句号、逗号等。
///   注意句末连续标点其实有 UAX #14 的「不可前断 / 不可后断」分类，MVP 一并归入可断；
///   极端情况下会出现「，」开头的续行，可接受。
/// - 平假名、片假名、CJK 偏旁、康熙部首、注音符号、CJK 基本与扩展 A、CJK 兼容、全角形式。
/// - 韩文音节（U+AC00-D7A3）。
/// - 扩展 B/C/D/E/F 落在 SMP，编辑器目前用 UTF-8 字节遍历，照样命中。
///
/// 不包含拉丁、阿拉伯数字、ASCII 标点——这些走「空白后」断点路径。
fn is_cjk_break_candidate(c: char) -> bool {
    let code = c as u32;
    matches!(
        code,
        0x2E80..=0x2EFF   // CJK 偏旁补充
        | 0x2F00..=0x2FDF // 康熙部首
        | 0x3000..=0x303F // CJK 符号与标点
        | 0x3040..=0x309F // 平假名
        | 0x30A0..=0x30FF // 片假名
        | 0x3100..=0x312F // 注音符号
        | 0x3130..=0x318F // 谚文兼容字母
        | 0x3190..=0x319F // 汉文训点
        | 0x31A0..=0x31BF // 注音符号扩展
        | 0x31C0..=0x31EF // CJK 笔画
        | 0x31F0..=0x31FF // 片假名拼音扩展
        | 0x3200..=0x32FF // 带圈 CJK 字母月
        | 0x3300..=0x33FF // CJK 兼容
        | 0x3400..=0x4DBF // CJK 扩展 A
        | 0x4E00..=0x9FFF // CJK 基础
        | 0xA000..=0xA48F // 彝文
        | 0xAC00..=0xD7A3 // 谚文音节
        | 0xF900..=0xFAFF // CJK 兼容表意文字
        | 0xFE30..=0xFE4F // CJK 兼容形式
        | 0xFF00..=0xFFEF // 全角 / 半角形式
        | 0x20000..=0x2EBEF // CJK 扩展 B/C/D/E/F
        | 0x2F800..=0x2FA1F // CJK 兼容表意文字补充
    )
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
}
