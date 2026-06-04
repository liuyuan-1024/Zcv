//! 编辑器 gutter 列 —— 行号 + 装饰图标。
//!
//! 横向不随正文滚动，纵向随视口顶行平移。行号在数字列内**右对齐**：位数
//! 不同时低位对低位、高位对高位。图标走与行号同一条字体路径，颜色由
//! producer 在 shape 时通过 [`TextRun::color`] 烤进去，避免引入额外资产
//! 管线（同手册阶段 4 / 阶段 6 的"shape 内嵌颜色"约定）。
//!
//! ## 列宽自适应位数
//!
//! 数字列宽 = buffer **总行数**的字符数 × 单字符宽度。用总行数而非视口
//! 当前最大行号——否则滚动到 1000 行附近时列宽会突变、整段正文跟着抖。
//! 单字符宽度按当前 text_style 实测（shape 一段 "0" 取 `width`），自动跟
//! 着字体 / 字号变化；编辑器统一用等宽字体，digit 宽度对任一数字都成立。
//!
//! 与 [`super::element::EditorElement`] 的协作：
//! - `prepare` 在 prepaint 阶段被调用，shape 行号文本并测量列宽；
//! - `Prepaint::offset` 给出正文水平偏移，element 用它裁剪文本区；
//! - `paint` 在阶段 1～5 之后调用，gutter 列横向固定、不参与正文滚动。

use gpui::{Bounds, ContentMask, Hsla, Pixels, ShapedLine, TextRun, Window, point, px, size};

use crate::shell::shared::theme::color;

/// 数字列与正文之间的间距。
const GAP: f32 = 12.0;

/// 行号文字颜色 —— 次级信息。
fn text_color() -> Hsla {
    color::gray::s08().into()
}

/// gutter 装饰图标的一条记录。
///
/// `row` 为视觉行号；`shaped` 是图标字形已 shape 的结果。
/// 颜色随 `shaped` 自带（producer 在构造 [`TextRun`] 时填入），本结构不再带独立 color 字段。
///
/// 当前为空 Vec；breakpoint / git diff / 诊断 glyph / bookmark 等可在此接入。
pub(crate) struct IconQuad {
    pub row: usize,
    pub shaped: ShapedLine,
}

/// gutter 的 prepaint 产物。
///
/// `enabled=false` 时 [`Self::offset`] 返回 0、[`paint`] 直接 noop——单行
/// 输入框等无行号编辑器走这个分支。
///
/// `line_numbers` 与视觉行一一对应：
/// - `Some(shaped)`：该视觉行是某条逻辑行的**首段**，画行号；
/// - `None`：该视觉行是软换行后的**续行**，行号留空（同一逻辑行的下一段）。
pub(crate) struct Prepaint {
    enabled: bool,
    line_numbers: Vec<Option<ShapedLine>>,
    icons: Vec<IconQuad>,
    /// 行号数字列宽（不含 [`GAP`]）；由 buffer 总行数与字体度量决定。
    number_width: Pixels,
}

impl Prepaint {
    /// gutter 关闭时的占位。
    pub(crate) fn disabled() -> Self {
        Self {
            enabled: false,
            line_numbers: Vec::new(),
            icons: Vec::new(),
            number_width: px(0.),
        }
    }

    /// 正文起点相对 gutter 左缘的水平偏移：`number_width + GAP`，关闭时为 0。
    pub(crate) fn offset(&self) -> Pixels {
        if self.enabled {
            self.number_width + px(GAP)
        } else {
            px(0.)
        }
    }
}

/// 软换行场景下，按视觉行**预先测量**数字列宽。
///
/// 仅依赖 buffer 总行数与字体度量，与具体视口切片无关——用来在 prepaint
/// 早期、还没决定每条逻辑行要拆成多少视觉行时，先算出正文起点的 X 偏移
/// （= number_width + GAP），以便用「正文区宽度」推断软换行的断行宽度。
pub(crate) fn measure_offset(
    total_lines: u64,
    text_style: &gpui::TextStyle,
    font_size: Pixels,
    window: &mut Window,
) -> Pixels {
    measure_number_width(total_lines, text_style, font_size, window) + px(GAP)
}

fn measure_number_width(
    total_lines: u64,
    text_style: &gpui::TextStyle,
    font_size: Pixels,
    window: &mut Window,
) -> Pixels {
    let max_digits = total_lines.max(1).to_string().len();
    shape_digits(&"0".repeat(max_digits), text_style, font_size, window).width
}

/// 按视觉行 shape 行号文本，产出可直接绘制的 [`Prepaint`]。
///
/// `total_lines` 为 buffer 总行数（**不是**当前视口最大行号），决定数字列宽的位数——这是「列宽不随滚动抖动」的关键。
///
/// `line_numbers_per_row` 长度等于视觉行数：
/// - `Some(line_index)`：该视觉行是某条逻辑行的首段，要画行号；
/// - `None`：续行（软换行的非首段），行号留空。
///
/// 不开软换行时 caller 传 `Some(line_index)` 全数组即可——退化回旧行为。
pub(crate) fn prepare(
    line_numbers_per_row: &[Option<u64>],
    total_lines: u64,
    text_style: &gpui::TextStyle,
    font_size: Pixels,
    window: &mut Window,
) -> Prepaint {
    let number_width = measure_number_width(total_lines, text_style, font_size, window);

    let mut line_numbers = Vec::with_capacity(line_numbers_per_row.len());
    for entry in line_numbers_per_row {
        match entry {
            Some(line_index) => {
                let label = (line_index + 1).to_string();
                line_numbers.push(Some(shape_digits(&label, text_style, font_size, window)));
            }
            None => line_numbers.push(None),
        }
    }
    Prepaint {
        enabled: true,
        line_numbers,
        icons: Vec::new(),
        number_width,
    }
}

/// 用 gutter 文字色 shape 一段纯数字串——行号与列宽探测共用同一字体路径。
fn shape_digits(
    digits: &str,
    text_style: &gpui::TextStyle,
    font_size: Pixels,
    window: &mut Window,
) -> ShapedLine {
    let run = TextRun {
        len: digits.len(),
        font: text_style.font(),
        color: text_color(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(digits.to_string().into(), font_size, &[run], None)
}

/// 绘制 gutter 列。
///
/// 行号右对齐到数字列右缘（`bounds_x + number_width`）——每个字形宽度不同
/// 时低位对低位、高位对高位。图标按 row 索引贴 `bounds_x` 起绘。两类绘制都
/// 在 gutter 区域内（自建 ContentMask），不溢出到正文。
pub(crate) fn paint(
    prepaint: &Prepaint,
    bounds_x: Pixels,
    bounds_top: Pixels,
    top: Pixels,
    bounds_height: Pixels,
    line_height: Pixels,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    if !prepaint.enabled {
        return;
    }
    let total_width = prepaint.number_width + px(GAP);
    let gutter_area = Bounds {
        origin: point(bounds_x, bounds_top),
        size: size(total_width, bounds_height),
    };
    let number_right_edge_x = bounds_x + prepaint.number_width;
    window.with_content_mask(
        Some(ContentMask {
            bounds: gutter_area,
        }),
        |window| {
            for (index, slot) in prepaint.line_numbers.iter().enumerate() {
                let Some(line_number) = slot else {
                    // 续行：留空。
                    continue;
                };
                let y = top + line_height * index as f32;
                let x = number_right_edge_x - line_number.width;
                let _ = line_number.paint(point(x, y), line_height, window, cx);
            }
            for icon in &prepaint.icons {
                let y = top + line_height * icon.row as f32;
                let _ = icon
                    .shaped
                    .paint(point(bounds_x, y), line_height, window, cx);
            }
        },
    );
}
