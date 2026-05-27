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

use crate::shell::editor::input::CaretLayout;
use crate::shell::editor::kernel::EditorKernel;
use crate::shell::editor::snapshot::{RevealHint, SnapshotLine};

use super::blink::CaretClock;
use super::input_host::{EditorInputHook, EditorPaintInfo, EditorViewportSyncHook};
use super::phases::{
    GlyphOverlay, GutterIconQuad, LineBackgroundQuad, LineMetric, paint_phase_1_line_backgrounds,
    paint_phase_2_range_backgrounds, paint_phase_3_glyphs, paint_phase_4_glyph_overlays,
    paint_phase_5_carets_and_composition, paint_phase_6_gutter,
};

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
    /// 整 buffer 的逻辑行总数；用于算 `content_height`、scroll clamp 与回写
    /// 视口边界。
    total_lines: u64,
    /// snapshot 切片对应的顶行（0-based 逻辑行号）；首条 `lines[0].line_index`
    /// 等于该值，但 lines 为空时仍需要它，独立带一份。
    viewport_start_line: u64,
    /// primary head 的 (行号, 列号)，0-based，绝对逻辑坐标——edge-scroll y 轴
    /// 与 caret 视口判定靠它，与 lines 是否覆盖该行无关。
    cursor_position: (u64, u64),
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
    /// 阶段 2 范围背景的第二个 producer：BufferSearch 命中。空 Vec 表示无搜索
    /// （单行嵌入输入框或无 query 的主编辑器）。
    search_hits: Vec<TextRange>,
    /// current hit 的 range——与 `search_hits` 中某一项相等（若有）。prepaint
    /// 根据它把对应那一项的颜色升级为强调色，其余 hit 用普通色。
    search_current: Option<TextRange>,
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
        cursor_position: (u64, u64),
        selection: SelectionSet,
        focus: FocusHandle,
        input_handler_hook: EditorInputHook,
    ) -> Self {
        Self {
            kernel,
            lines,
            total_lines,
            viewport_start_line,
            cursor_position,
            selection,
            focus,
            input_handler_hook,
            element_id: None,
            reveal: None,
            search_hits: Vec::new(),
            search_current: None,
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

    /// 装载本编辑器要在阶段 2 显示的搜索命中。
    ///
    /// `hits` 应按 ordinal 升序、互不重叠（BufferSearch 来源天然保证）；`current`
    /// 若 `Some`，应与 `hits` 中某一项 `==`。空 Vec + `None` 表示不画搜索高亮。
    pub(crate) fn search_overlay(
        mut self,
        hits: Vec<TextRange>,
        current: Option<TextRange>,
    ) -> Self {
        self.search_hits = hits;
        self.search_current = current;
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
/// `TextRange` 也是绝对 byte，二者天然对齐。
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
/// 字段按手册 19.4 的 6 阶段分组：每个阶段消费一个 `Vec<...>` 槽位。v1 不接入
/// 的槽位（阶段 1 行背景、阶段 4 字符叠加、阶段 5 IME underline、阶段 6 装饰
/// 图标）固定为空 Vec；接入新装饰来源时只需在 prepaint 出口处往对应 Vec 推
/// 条目，paint 主干不变。
///
/// 颜色解析的位置：所有装饰来源遵循「语义键 + theme 解析」契约，但**解析步骤
/// 前置到 prepaint**——prepaint 在推条目时就配好 `Hsla`，paint 阶段只看几何
/// 与颜色，不再 if 路由"这条 range 来自 selection 还是 search"。详见 phases.rs
/// 模块注释。
pub(crate) struct EditorPrepaint {
    // ── 阶段 1：行背景 ─────────────────────────────────────────────────────
    /// v1 = 空 Vec；P3+ 接 active line / diff hunk 等整段背景。
    line_backgrounds: Vec<LineBackgroundQuad>,

    // ── 阶段 2：范围背景 ───────────────────────────────────────────────────
    /// selection / search / AI 区间等半透明色块。v1 仅 selection；同一 Vec 内
    /// 各 producer 自行保证按 `start` 升序、互不重叠，合并后整体不要求互不
    /// 重叠（alpha 叠加表达层叠语义）。
    range_backgrounds: Vec<(TextRange, Hsla)>,

    // ── 阶段 3：字符层 ─────────────────────────────────────────────────────
    /// 每行的 shape + 字节坐标信息，下标即视觉行号（= line_index - viewport_start_line）。
    lines: Vec<PrepaintedLine>,

    // ── 阶段 4：字符叠加 ───────────────────────────────────────────────────
    /// v1 = 空 Vec；P3 inlay hint / P4 AI ghost text 接入点。
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
    /// v1 = 空 Vec；P2 IME marked text 接入点。
    composition_underlines: Vec<(TextRange, Hsla)>,

    // ── 阶段 6：gutter（行号 + 装饰图标）──────────────────────────────────
    /// 每行行号的 shaped 结果；无行号列时为空。
    gutter_line_numbers: Vec<ShapedLine>,
    /// v1 = 空 Vec；P3+ breakpoint / git diff / 诊断 glyph / bookmark 接入点。
    gutter_icons: Vec<GutterIconQuad>,

    // ── 共享几何 ───────────────────────────────────────────────────────────
    line_height: Pixels,
    /// 正文起点相对 `bounds.origin.x` 的偏移（有行号列时为 [`GUTTER_WIDTH`]）。
    gutter_offset: Pixels,
    /// 当前滚动偏移；正文与光标按它整体平移。
    scroll: Point<Pixels>,
    /// `top` 已经吸收了 `viewport_start_line × line_height` 的修正：phases 用
    /// `top + row_index × line_height` 即可拿到正确像素 y，无需知道视口起点。
    top_adjusted: Pixels,
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
        let viewport_start_line = self.viewport_start_line;
        let total_lines = self.total_lines.max(1);

        // SelectionSet 经过引擎归一化，as_slice() 已按 start 排序、互不重叠。
        // primary 在归一化前后由 primary_index 跟踪 —— 这里 carets 的位置由
        // primary_caret 单独记录，下标与 selection.as_slice 已不再 1:1。
        let selections = self.selection.as_slice();
        let primary_sel_index = self
            .selection
            .primary_index()
            .min(selections.len().saturating_sub(1));

        // visible_row -> primary caret 的视口内 (row, x) 占位；None 表示视口外
        // / 暂未匹配上。primary 的"视口外但要 reveal/edge-scroll"路径走
        // primary_caret_logical / primary_caret_x 兜底。
        let mut carets_pos: Vec<Option<(usize, Pixels)>> = vec![None; selections.len()];
        let mut prepainted_lines: Vec<PrepaintedLine> = Vec::with_capacity(self.lines.len());
        let mut gutter_line_numbers: Vec<ShapedLine> = Vec::with_capacity(self.lines.len());

        // reveal 是否生效完全看快照里有没有；调用方不需要 reveal 时
        // 自然就不会在 owner.snapshot() 里填这个字段。
        let active_reveal = self.reveal;
        let reveal_byte = active_reveal.map(|hint| hint.byte);
        let reveal_logical_line = active_reveal.map(|hint| hint.line);
        // reveal 目标若在视口内，shape 后能算出行内 x（用于 reveal x 轴摆位）。
        let mut reveal_visible_row_x: Option<(usize, Pixels)> = None;

        for (visual_row, line) in self.lines.iter().enumerate() {
            let raw = line.text.as_str();
            let line_start = line.start_byte;
            let line_end = line_start + raw.len();

            // 文本 shape 的输入只有「内容 + 字体」，与光标无关 —— 同一行内容
            // 每帧 shape 命中 GPUI 行布局缓存，光标移动不会触发重排。
            let runs = if raw.is_empty() {
                Vec::new()
            } else {
                vec![text_style.to_run(raw.len())]
            };
            let shaped = window.text_system().shape_line(
                SharedString::from(raw.to_string()),
                font_size,
                &runs,
                None,
            );

            // 每个 selection.head 落在视口内时算其行内 x。head 出现在行末
            // (`head == line_end`) 与行首 (`head == 下一行 line_start`) 是
            // 同一字节位置——head 优先归到「等于行尾」的那一行（与旧 cursor_byte
            // 行为一致：保留尾随空格后的光标位）。
            for (i, sel) in selections.iter().enumerate() {
                if carets_pos[i].is_some() {
                    continue;
                }
                let head = sel.head().get();
                if head >= line_start && head <= line_end {
                    carets_pos[i] = Some((visual_row, shaped.x_for_index(head - line_start)));
                }
            }
            if let Some(rb) = reveal_byte
                && reveal_visible_row_x.is_none()
                && rb >= line_start
                && rb <= line_end
            {
                reveal_visible_row_x = Some((visual_row, shaped.x_for_index(rb - line_start)));
            }
            prepainted_lines.push(PrepaintedLine {
                line_start_byte: line_start,
                line_len: raw.len(),
                shaped,
            });

            if has_gutter {
                // 行号 1-based，叠加视口起点偏移得到真正的逻辑行号。
                let label = (line.line_index + 1).to_string();
                let run = TextRun {
                    len: label.len(),
                    font: text_style.font(),
                    color: gutter_color(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                gutter_line_numbers.push(window.text_system().shape_line(
                    label.into(),
                    font_size,
                    &[run],
                    None,
                ));
            }
        }

        // 视口外的 caret 直接丢掉——避免被 unwrap_or((0, 0)) 摆到视区左上角形成
        // 残影；多行主编辑区滚到不见 primary 时也只剩边缘指示。
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

        // 阶段 2 范围背景：颜色在此处按语义键解析为 Hsla，paint 阶段只看几何
        // 与颜色。两个 producer：
        //
        // 1. **search**（BufferSearch 命中）—— 先入栈，作为底层视觉。normal hit
        //    用 `orange.a03`（淡暖色，与 selection 的 blue 系不冲突）；current
        //    hit 用 `orange.a05`（手册标注的"当前搜索命中"色，更强）。
        // 2. **selection** —— 后入栈，作为最上层，保证用户的活动选区始终视觉
        //    优先（与 search hit 重叠时 selection 颜色在上）。
        //
        // 各 producer 内部已按 start 升序、互不重叠；合并后整体不要求互不重叠
        // ——alpha 叠加表达层叠语义。
        let search_normal_color: Hsla = color::orange::a03().into();
        let search_current_color: Hsla = color::orange::a05().into();
        let selection_color: Hsla = color::blue::a04().into();
        // search 覆盖层数据驱动：没填 hits 自然不画 —— 单行 owner（文件树 /
        // 选择器 / 搜索面板自己的输入框）不会在 snapshot 里塞 hits。
        let mut range_backgrounds: Vec<(TextRange, Hsla)> =
            Vec::with_capacity(self.search_hits.len() + selections.len());
        for hit in &self.search_hits {
            let color = if Some(*hit) == self.search_current {
                search_current_color
            } else {
                search_normal_color
            };
            range_backgrounds.push((*hit, color));
        }
        for sel in selections.iter().filter(|s| !s.is_caret()) {
            range_backgrounds.push((sel.range(), selection_color));
        }

        let gutter_offset = if has_gutter { px(GUTTER_WIDTH) } else { px(0.) };

        // primary caret 的"逻辑行 / 行内 x"——用于 edge-scroll / reveal。logical
        // 行号永远可拿（cursor_position 在 snapshot 已经折算好）；x 只有 primary
        // 真的落在 shape 出来的视口行里才有。
        let primary_caret_logical_line = self.cursor_position.0;
        let primary_caret_x = primary_caret_in_view
            .map(|idx| carets[idx].1)
            .unwrap_or(px(0.));

        // autoscroll：让光标保持在视口内。需要稳定 element id 来跨帧存偏移。
        let (scroll, measured_top_line, measured_visible_lines) = match id {
            Some(global_id) => {
                let content_height = line_height * total_lines as f32;
                let content_width = prepainted_lines.iter().fold(px(0.), |max, line| {
                    if line.shaped.width > max {
                        line.shaped.width
                    } else {
                        max
                    }
                });
                let viewport_h = bounds.size.height;
                let viewport_w =
                    clamp_px(bounds.size.width - gutter_offset, px(0.), bounds.size.width);
                let reveal = active_reveal;
                let result =
                    window.with_element_state::<EditorScroll, _>(global_id, |state, _window| {
                        let mut state = state.unwrap_or_default();
                        let mut off = state.offset;

                        // === reveal 路径 ===
                        // 外部 request_reveal 投递的请求；按 seq 触发一次。
                        // kind 决定触发条件与摆位 —— 集中在这里翻译，调用方只
                        // 表达"意图"。
                        //
                        // 两个轴独立判断可见性 / 独立摆位：搜索 match 在视区"左
                        // 边"但同一行内时，只需要滚 X；在不同行但同一列时，
                        // 只需要滚 Y。一起判断会把"X 已经可见"也当成需要滚，
                        // 反而抖。
                        if let Some(req) = reveal
                            && Some(req.seq) != state.last_applied_reveal_seq
                        {
                            // y 轴用逻辑行号，离视口多远都能滚回来——不必依赖
                            // reveal byte 是否在 shape 出来的视口行里。
                            let row = reveal_logical_line.unwrap_or(req.line);
                            let target_top = line_height * row as f32;
                            let target_bottom = target_top + line_height;
                            let visible_y =
                                target_top >= off.y && target_bottom <= off.y + viewport_h;

                            // x 轴只在 reveal 目标行确实进入视口、shape 出 x 时
                            // 有意义；否则保留当前 off.x，等到目标行被 y 轴滚
                            // 进视口的下一帧再校准。
                            let visible_x_opt = reveal_visible_row_x.map(|(_, x)| {
                                let target_right = x + px(CARET_WIDTH);
                                (x, x >= off.x && target_right <= off.x + viewport_w)
                            });

                            let (placement_factor, force_scroll) = match req.kind {
                                RevealKind::Match => (1.0 / 3.0, false),
                                RevealKind::Jump => (1.0 / 3.0, true),
                            };
                            if force_scroll || !visible_y {
                                off.y = target_top - viewport_h * placement_factor;
                            }
                            if let Some((x, visible_x)) = visible_x_opt
                                && (force_scroll || !visible_x)
                            {
                                off.x = x - viewport_w * placement_factor;
                            }
                            // 即使本帧没拿到行号也要登记 seq——不然下一帧文本
                            // 仍然能命中行时会"补"一次滚动，对用户来说像延迟反应。
                            state.last_applied_reveal_seq = Some(req.seq);
                        }

                        // === edge-scroll 路径（兜底）===
                        // 即便 reveal 把 reveal byte 摆到上 1/3，caret（=match
                        // end）在多行 match 时也可能跨到视区外；edge-scroll 把
                        // caret 拉回来，宁可让 reveal byte 飘出视区，也不能让
                        // caret 消失。多光标场景只让 primary caret 决定滚动
                        // —— 多个 caret 同时驱动滚动会互相拉扯。primary 是用户
                        // 最近交互的 caret，跟它走最自然。
                        if allows_vertical_scroll {
                            let caret_top = line_height * primary_caret_logical_line as f32;
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
                        } else {
                            off.y = px(0.);
                        }

                        // 横向：光标列需可见。行尾光标位于最后一个字形「之
                        // 后」，故可滚动宽度要在最宽行的基础上额外留出一个光
                        // 标宽度——否则滚到行尾时光标正好压在正文区裁剪边界
                        // 上、被切掉。primary caret 在视口外时 x = 0，不主动
                        // 触发 x 滚动（再下一帧 caret 进视口，shape 出 x 后再
                        // 校准）。
                        let scrollable_w = content_width + px(CARET_WIDTH);
                        if primary_caret_in_view.is_some() {
                            let caret_right = primary_caret_x + px(CARET_WIDTH);
                            if primary_caret_x < off.x {
                                off.x = primary_caret_x;
                            } else if caret_right > off.x + viewport_w {
                                off.x = caret_right - viewport_w;
                            }
                        }
                        off.x = clamp_px(
                            off.x,
                            px(0.),
                            clamp_px(scrollable_w - viewport_w, px(0.), scrollable_w),
                        );

                        state.offset = off;

                        // 反算视口边界：top_line = floor(off.y / line_height)；
                        // visible_line_count = ceil(viewport_h / line_height)
                        // ——加一行裕度让"行半被裁"也可以画出来。
                        let lh: f32 = f32::from(line_height).max(1.0);
                        let off_y_f: f32 = off.y.into();
                        let viewport_h_f: f32 = viewport_h.into();
                        let (top_line_clamped, visible) = if allows_vertical_scroll {
                            let top_line = (off_y_f / lh).floor().max(0.0) as u64;
                            let visible = ((viewport_h_f / lh).ceil() as u64).saturating_add(1);
                            (top_line.min(total_lines.saturating_sub(1)), visible)
                        } else {
                            (0, 1)
                        };
                        ((off, top_line_clamped, visible), state)
                    });
                result
            }
            None => (
                Point::default(),
                viewport_start_line,
                self.lines.len() as u64,
            ),
        };

        // 调用视口写回钩子，让下一帧的 snapshot 切对 buffer 视口段。一帧的
        // 旧值不会更糟——snapshot 用的是上一帧反算结果，element 当帧仍按 lines
        // 渲染；首帧的 200 行兜底已经够覆盖任何屏幕高度。
        if let Some(sync) = self.viewport_sync.as_ref() {
            sync(measured_top_line, measured_visible_lines, cx);
        }

        // top 已吸收 viewport_start_line × line_height 的修正：visual_row 0 在
        // 物理 y 上对应逻辑行 viewport_start_line，phases 用 row_index 算 y 即
        // 可，**不需要再知道视口起点**。
        let top_adjusted = bounds.origin.y - scroll.y + line_height * viewport_start_line as f32;

        EditorPrepaint {
            // v1 空槽位：行背景。
            line_backgrounds: Vec::new(),
            // v1 唯一 producer：selection（已在上方解析为 (range, color)）。
            range_backgrounds,
            lines: prepainted_lines,
            // v1 空槽位：字符叠加（ghost text / inlay hint）。
            glyph_overlays: Vec::new(),
            carets,
            primary_caret: primary_caret_in_view,
            // v1 空槽位：IME composition underline（marked text 待接入）。
            composition_underlines: Vec::new(),
            gutter_line_numbers,
            // v1 空槽位：gutter 装饰图标（breakpoint / git diff / 诊断 / bookmark）。
            gutter_icons: Vec::new(),
            line_height,
            gutter_offset,
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
        // 纵向滚动 + 视口顶行修正都已吸收进 prepaint.top_adjusted；这里 phases
        // 用 row_index × line_height 算 y 即可。
        let top = prepaint.top_adjusted;
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
        // caret 可见性与焦点态绑定：FocusHandle 决定本 view 是否"活动"；CaretClock
        // 决定闪烁相位。范围背景（含 selection）颜色已在 prepaint 解析，paint 不
        // 再 if 路由 producer 来源。
        let caret_visible = self.focus.is_focused(window) && CaretClock::is_visible(cx);
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
            // 阶段 1：行背景（v1 空 Vec → no-op；P3+ active line / diff hunk）。
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

            // 阶段 4：字符叠加（v1 空 Vec → no-op；P3 inlay hint / P4 ghost text）。
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
        paint_phase_6_gutter(
            &prepaint.gutter_line_numbers,
            &prepaint.gutter_icons,
            bounds.origin.x,
            prepaint.gutter_offset,
            bounds.origin.y,
            top,
            bounds.size.height,
            line_height,
            window,
            cx,
        );

        // 把 primary caret 在 element 内的相对坐标打包给 input hook。系统 IME
        // 的 `bounds_for_range` 会借此把候选窗放到 caret 正下方；primary 不在
        // 视口内时传 None，让 IME 走默认（候选窗落在窗口角，明显比"贴在
        // 编辑区左上角"易辨认是哪里有问题）。
        let caret_layout = prepaint.primary_caret.and_then(|idx| {
            let (caret_row, caret_x) = *prepaint.carets.get(idx)?;
            Some(CaretLayout {
                relative: point(
                    prepaint.gutter_offset + caret_x - scroll.x,
                    // 阶段 5 caret 画在 `top_adjusted + row × line_height`；
                    // 这里换回 `bounds.origin.y` 相对坐标即 `top - bounds.y +
                    // row × line_height`。
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
