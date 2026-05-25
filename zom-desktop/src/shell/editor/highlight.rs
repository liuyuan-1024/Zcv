//! 阶段 2 范围背景渲染原语 —— 在文本之下、caret 之上之外的中间层画色块。
//!
//! 手册 19.4 把编辑区绘制切成 6 个阶段；本模块负责其中**阶段 2「范围背景」**：
//! 接受若干 `(TextRange, Hsla)`，按行拆成 quad，画在文本之下。caller 在构造
//! ranges 时直接调相应色相的 alpha 阶梯（如 `color::blue::a04()` 给选区、
//! `color::blue::a05()` 给搜索普通命中、`color::orange::a05()` 给当前命中）；
//! 颜色解析不在本模块。
//!
//! **承载范围**：selection / search match / current search match / 未来 AI 提案
//! 区间，全部走这个原语；颜色从相应色相的 alpha 阶梯取，半透明叠加保留 syntax 字色。
//!
//! **不做防御性 dedupe**：上游 `SelectionSet` 已经归一化（非重叠、按 start
//! 排序）。重叠是引擎层 bug，应被 alpha 叠加自然显现而不是被静默吞掉。
//! debug 构建里加 `debug_assert!` 作为契约护栏，release 编译消掉。
//!
//! **零宽区间不画色块**：caret-only 的 selection 落在阶段 5；本阶段直接跳过
//! `is_empty()` 的区间，避免 1px 色条瑕疵。

use gpui::{Bounds, Hsla, Pixels, ShapedLine, Window, fill, point, px, size};

use zom_engine::TextRange;

/// 单行 byte→x 映射所需的最小信息。由 prepaint 阶段构造、paint 阶段消费。
///
/// `line_start_byte` / `line_len` 与 [`super::element::EditorElement`] 的
/// prepaint 里 `offset` / `raw.len()` 的语义一致；`line_len` **不含 `\n`**——
/// 选区跨行时的 EOL 视觉延伸由本模块的 [`EOL_EXTENSION`] 统一处理。
///
/// 这个结构体存在的理由：把 prepaint 算出的"行起始偏移 + 行长 + ShapedLine"
/// 三元组打成一个带语义命名的类型，避免裸 `(usize, usize, &ShapedLine)` 让
/// caller 猜两个 usize 的含义；同时让 `&ShapedLine` 借用有个生命周期挂靠点。
pub(crate) struct LineMetric<'a> {
    pub line_start_byte: usize,
    pub line_len: usize,
    pub shaped: &'a ShapedLine,
}

/// 跨行选区在行尾的视觉延伸（像素）：让"换行被选中"显式可见，也避免多行
/// 选区在换行处看上去断成几段。第一版固定常量，未来如需主题化再升级。
const EOL_EXTENSION: f32 = 4.0;

/// 阶段 2 渲染原语入口。
///
/// 调用方需保证：
/// - `ranges` 按 `TextRange::start` 升序、互不重叠（debug 断言）
/// - `lines` 的下标即视觉行号，顺序覆盖 `[0, n)` 行——传入序列必须与 prepaint
///   产出的 `lines` 同序、同长
/// - `text_left` / `top` 已吸收 scroll 偏移；`text_area` 是已扣去 gutter 的正文
///   裁剪盒，用于行级粗剔除（盒内裁剪由 caller 的 `with_content_mask` 兜底）
///
/// 本函数纯绘制、不持状态；对每个 range 独立调一次 [`paint_one_range`]。
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_range_backgrounds(
    ranges: &[(TextRange, Hsla)],
    lines: &[LineMetric<'_>],
    text_left: Pixels,
    top: Pixels,
    line_height: Pixels,
    text_area: Bounds<Pixels>,
    window: &mut Window,
) {
    #[cfg(debug_assertions)]
    {
        let mut prev_end: Option<usize> = None;
        for (range, _) in ranges {
            if let Some(p) = prev_end {
                debug_assert!(
                    range.start().get() >= p,
                    "paint_range_backgrounds ranges 必须按 start 升序、互不重叠（caller 责任）"
                );
            }
            prev_end = Some(range.end().get());
        }
    }

    for (range, color) in ranges {
        if range.is_empty() {
            continue;
        }
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

/// 把单个区间在它跨过的每一行上各画一个 quad。
///
/// 行内裁剪规则：
/// - 区间结束于行首之前（`r_end <= line_start`）→ 跳过
/// - 区间开始于行末之后（`r_start > line_end`）→ 跳过
/// - 区间跨过行末换行（`r_end > line_end`）→ x_end 取 `shaped.width + EOL_EXTENSION`，
///   显式表达"换行被选中"
#[allow(clippy::too_many_arguments)]
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
        window.paint_quad(fill(quad, color));
    }
}
