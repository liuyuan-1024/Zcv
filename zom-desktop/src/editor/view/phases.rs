//! EditorView 渲染管线 —— 6 阶段绘制契约的实现。
//!
//! 手册 19.4 把编辑区绘制切成 6 个固定顺序的阶段；本模块为每个阶段提供一个
//! 独立的 `paint_phase_N_*` 函数，由 [`super::element::EditorElement::paint`]
//! 按顺序调用。阶段间是绘制顺序（后画的覆盖前画的）；阶段内部不同 layer 来源之间才谈 z-order。
//!
//! | 阶段 | 内容 | 当前状态 |
//! |---|---|---|
//! | 1 | 行背景（active line / diff hunk 等整段） | 槽位留空 |
//! | 2 | 范围背景（selection / search） | selection |
//! | 3 | 字符层（字色 / 字重 / 下划线 / 波浪线） | 仅字色 |
//! | 4 | 字符叠加（inline ghost text / inlay hint） | 槽位留空 |
//! | 5 | caret + IME composition underline | caret，underline 槽位留空 |
//!
//! 阶段 6（gutter：行号 + 装饰图标）独立到 [`super::gutter`] 模块——它横向
//! 不随正文滚动，几何与本模块的范围 / 字符 / caret 阶段相互独立，没必要共用
//! 这里的 `LineMetric` 与 EOL 视觉延伸约定。
//!
//! 「槽位留空」= prepaint 给出空 `Vec`，paint 函数照常调用、for 循环零次迭代；
//! 接入新装饰来源时**不动 paint 主干**，只在 prepaint 入口处往对应 Vec 推条目。
//!
//! ## 颜色解析的位置
//!
//! 所有装饰来源遵循「语义键 + theme 解析」契约，但**解析步骤前置到 prepaint**：
//! - prepaint 把 `(语义, 主题)` 解析成 `(几何, Hsla)` 推进对应槽位
//! - paint 阶段只看 Hsla，不再 if 出"这条 range 是 selection 还是 search"
//!
//! 这与"prepaint 只算几何、paint 配色"的纯几何分层不同——多来源（selection /
//! search current vs normal hit 等需求下，paint 里 if 路由会迅速
//! 退化成 LayerKind dispatch，违反"阶段顺序固定"的契约。
//!
//! ## 输入契约（调试断言）
//!
//! 阶段 2 的 `ranges` 必须按 `TextRange::start` 升序、互不重叠——是**单个 producer
//! 内部的契约**（SelectionSet 已归一化、search 命中天然不重叠）。合并多来源后
//! 整体不再要求互不重叠：半透明 alpha 叠加是正确语义。
//!
//! 因此 [`paint_phase_2_range_backgrounds`] 不做合并后的全局重叠检查；只在
//! 单 producer 注入时由 producer 自己 `debug_assert!`。

use gpui::{Bounds, Hsla, Pixels, ShapedLine, Window, fill, point, px, size};

use zom_engine::TextRange;

use crate::theme::radius;

// ============================================================================
// 各阶段消费的数据类型（当前留空的槽位也都用这些类型）
// ============================================================================

/// 阶段 1 行背景的一条记录。`row` 为视觉行号（0-based）。
///
/// 当前为空 Vec；active line（编辑器自持色）、diff hunk（语法 / 版本控制
/// 来源）等整段背景。颜色已在 prepaint 解析；本阶段只看 Hsla。
pub(crate) struct LineBackgroundQuad {
    pub row: usize,
    pub color: Hsla,
}

/// 阶段 4 字符叠加的一条记录。
///
/// `at_byte` 为整段 text 内的字节偏移；ghost text / inlay hint 都是「在某字节
/// 位置插入一段非文档文本」。`shaped` 已 shape 完成——**颜色由 producer 在
/// shape 时通过 [`gpui::TextRun::color`] 烤进去**，本结构不再带独立 color 字段
/// （和阶段 6 行号同一路径，避免双源真相）。
///
/// 当前为空 Vec；inlay hint 可在此接入。
pub(crate) struct GlyphOverlay {
    pub at_byte: usize,
    pub shaped: ShapedLine,
}

/// 单行 byte→x 映射所需的最小信息，由 prepaint 构造、paint 消费。
///
/// `line_len` **不含换行符**；跨行选区的行尾视觉延伸由 [`EOL_EXTENSION`] 统一处理。
pub(crate) struct LineMetric<'a> {
    pub line_start_byte: usize,
    pub line_len: usize,
    pub shaped: &'a ShapedLine,
}

/// 跨行选区在行尾的视觉延伸（像素）：让"换行被选中"显式可见，也避免多行选区在换行处看上去断成几段。
/// 当前固定常量，未来如需主题化再升级。
pub(crate) const EOL_EXTENSION: f32 = 4.0;

// ============================================================================
// 阶段 1：行背景
// ============================================================================

/// 阶段 1：整段行背景。
///
/// 画在所有内容之下；与文本层共用纵向滚动（`top` 已吸收 scroll.y）。
/// 调用方传 `&[]` 时函数无操作。
pub(crate) fn paint_phase_1_line_backgrounds(
    quads: &[LineBackgroundQuad],
    text_area: Bounds<Pixels>,
    text_left: Pixels,
    top: Pixels,
    line_height: Pixels,
    window: &mut Window,
) {
    for quad in quads {
        let row_top = top + line_height * quad.row as f32;
        // 行级粗剔除：完全在视区外的行不画。
        let row_bottom = row_top + line_height;
        if row_bottom < text_area.origin.y || row_top > text_area.origin.y + text_area.size.height {
            continue;
        }
        // 当前行背景与文本区等宽。
        // 未来若需要 active-line 全宽（含 gutter），拆出 phase_1b 在 mask 外画即可，不改本函数。
        let bounds = Bounds {
            origin: point(text_left, row_top),
            size: size(text_area.size.width, line_height),
        };
        window.paint_quad(fill(bounds, quad.color));
    }
}

// ============================================================================
// 阶段 2：范围背景
// ============================================================================

/// 阶段 2：跨字节区间的半透明色块（selection / search 等）。
///
/// 各 producer 内部保证 ranges 按 `start` 升序、互不重叠（调试断言）；
/// 多 producer 合并后整体不要求互不重叠，alpha 叠加表达层叠语义。
///
/// 调用方需保证：
/// - `lines` 的下标即视觉行号，顺序覆盖 `[0, n)` 行，与 prepaint 产出同序同长
/// - `text_left` / `top` 已吸收 scroll 偏移
/// - `text_area` 是已扣去 gutter 的正文裁剪盒
pub(crate) fn paint_phase_2_range_backgrounds(
    ranges: &[(TextRange, Hsla)],
    lines: &[LineMetric<'_>],
    text_left: Pixels,
    top: Pixels,
    line_height: Pixels,
    text_area: Bounds<Pixels>,
    window: &mut Window,
) {
    for (range, color) in ranges {
        if range.is_empty() {
            // caret-only 区间在阶段 5 画；本阶段跳过避免 1px 色条瑕疵。
            continue;
        }
        // 优先尝试流体多边形渲染；失败时回退到逐行 quad。
        if !super::fluid_selection::paint_fluid_range(
            *range,
            *color,
            lines,
            text_left,
            top,
            line_height,
            text_area,
            window,
        ) {
            paint_one_range(
                *range,
                *color,
                lines,
                text_left,
                top,
                line_height,
                text_area,
                window,
            );
        }
    }
}

/// 把单个区间在它跨过的每一行上各画一个 quad。
///
/// 保留为流体多边形渲染的回退路径：当 PathBuilder 三角化失败时使用。
///
/// 行内裁剪规则：
/// - 区间结束于行首之前（`r_end <= line_start`）→ 跳过
/// - 区间开始于行末之后（`r_start > line_end`）→ 跳过
/// - 区间跨过行末换行（`r_end > line_end`）→ x_end 取 `shaped.width + EOL_EXTENSION`，
///   显式表达"换行被选中"
#[allow(dead_code)]
fn paint_one_range(
    range: TextRange,
    color: Hsla,
    lines: &[LineMetric<'_>],
    text_left: Pixels,
    top: Pixels,
    line_height: Pixels,
    text_area: Bounds<Pixels>,
    window: &mut Window,
) {
    let r_start = range.start().get();
    let r_end = range.end().get();

    let viewport_top = text_area.origin.y;
    let viewport_bottom = viewport_top + text_area.size.height;

    for (index, line) in lines.iter().enumerate() {
        let line_start = line.line_start_byte;
        let line_end = line_start + line.line_len;

        if r_end <= line_start || r_start > line_end {
            continue;
        }

        let row_top = top + line_height * index as f32;
        let row_bottom = row_top + line_height;
        if row_bottom < viewport_top || row_top > viewport_bottom {
            continue;
        }

        let start_in_line = r_start.saturating_sub(line_start).min(line.line_len);
        let x_start = line.shaped.x_for_index(start_in_line);

        let crosses_eol = r_end > line_end;
        let end_in_line = if crosses_eol {
            line.line_len
        } else {
            r_end - line_start
        };
        let mut x_end = line.shaped.x_for_index(end_in_line);
        if crosses_eol {
            x_end += px(EOL_EXTENSION);
        }

        if x_end <= x_start {
            continue;
        }

        let quad = Bounds {
            origin: point(text_left + x_start, row_top),
            size: size(x_end - x_start, line_height),
        };
        // 2px 圆角让色块边角更柔和（与 Zed / VS Code 习惯一致）。
        // 跨行选区的行间会有 2px 的「凹口」——可忽略级别。
        // 若要彻底消掉，需按「段顶 / 段底只圆外侧 corner」处理更复杂的外侧圆角规则。
        window.paint_quad(fill(quad, color).corner_radii(radius::r2()));
    }
}

// ============================================================================
// 阶段 3：字符层
// ============================================================================

/// 阶段 3：逐行绘制 shape 好的字符层。
///
/// 当前仅字色（在 shape 时已确定）；syntax 由 prepaint 拆成多段 TextRun，
/// paint 不变。下划线 / 波浪线（诊断、拼写）也由 shape 配置承载，不在此独立画。
pub(crate) fn paint_phase_3_glyphs(
    lines: &[super::element::PrepaintedLine],
    text_left: Pixels,
    top: Pixels,
    line_height: Pixels,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    for (index, line) in lines.iter().enumerate() {
        let y = top + line_height * index as f32;
        let _ = line
            .shaped()
            .paint(point(text_left, y), line_height, window, cx);
    }
}

// ============================================================================
// 阶段 4：字符叠加
// ============================================================================

/// 阶段 4：在字节位置上叠加非文档字形（ghost text / inlay hint）。
///
/// 调用方传 `&[]` 时函数无操作。inlay hint 接入时，
/// 由 prepaint 把字节位置算出对应的 `(行, 行内 x)`，shape 出叠加文本，
/// 推进 `glyph_overlays` Vec。
///
/// 「字节位置 → (行, x)」的换算与阶段 2 范围背景的入行逻辑一致，复用
/// [`LineMetric`]。
pub(crate) fn paint_phase_4_glyph_overlays(
    overlays: &[GlyphOverlay],
    lines: &[LineMetric<'_>],
    text_left: Pixels,
    top: Pixels,
    line_height: Pixels,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    for overlay in overlays {
        for (index, line) in lines.iter().enumerate() {
            let line_start = line.line_start_byte;
            let line_end = line_start + line.line_len;
            if overlay.at_byte < line_start || overlay.at_byte > line_end {
                continue;
            }
            let x = line.shaped.x_for_index(overlay.at_byte - line_start);
            let y = top + line_height * index as f32;
            let _ = overlay
                .shaped
                .paint(point(text_left + x, y), line_height, window, cx);
            break;
        }
    }
}

// ============================================================================
// 阶段 5：caret + IME composition underline
// ============================================================================

/// 阶段 5：每个 selection.head 一根 caret 竖条 + IME composition 下划线。
///
/// `caret_visible` 由 caller（focus + CaretClock 相位）决定；不可见时不画 caret，
/// 但 composition underline 仍要画（IME 调起时编辑器常被认为聚焦状态）。
///
/// `composition_underlines` 当前为空 Vec；IME marked text 作为阶段 2 / 5 的
/// 第二个消费者接入时复用本阶段。
pub(crate) fn paint_phase_5_carets_and_composition(
    carets: &[(usize, Pixels)],
    caret_visible: bool,
    caret_width: Pixels,
    caret_color: Hsla,
    composition_underlines: &[(TextRange, Hsla)],
    lines: &[LineMetric<'_>],
    text_left: Pixels,
    top: Pixels,
    line_height: Pixels,
    text_area: Bounds<Pixels>,
    window: &mut Window,
) {
    if caret_visible {
        for (caret_line, caret_x) in carets {
            let caret_bounds = Bounds {
                origin: point(text_left + *caret_x, top + line_height * *caret_line as f32),
                size: size(caret_width, line_height),
            };
            window.paint_quad(fill(caret_bounds, caret_color).corner_radii(radius::r2()));
        }
    }

    // IME 下划线视觉上与范围背景同形——一条横跨字节区间的细色带，画在文本底部。
    // 复用阶段 2 的单行裁剪逻辑（[`paint_one_range`] 是私有的，但 underline 形态与色块只差「画在底部 2px 而非整行高」）。
    // 这里保留接入点。
    if !composition_underlines.is_empty() {
        // IME marked text 接入时实现。
        let _ = (lines, text_area, composition_underlines);
    }
}

// 阶段 6（gutter）独立到 `super::gutter` 模块。
