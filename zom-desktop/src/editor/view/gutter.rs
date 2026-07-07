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

use gpui::{
    Bounds, ContentMask, Hsla, Path, Pixels, Rgba, ShapedLine, TextRun, Window, fill, point, px,
    size,
};

use super::element::VisualRow;
use crate::git_service::{DiffHunk, DiffHunkKind};
use crate::theme::{color, space};

/// 行号文字颜色 —— 次级信息。
fn text_color() -> Hsla {
    color::current().gray.s08.into()
}

/// gutter 装饰图标的一条记录。
///
/// `row` 为视觉行号；`shaped` 是图标字形已 shape 的结果。
/// 颜色随 `shaped` 自带（producer 在构造 [`TextRun`] 时填入），本结构不再带独立 color 字段。
///
/// breakpoint / 诊断 glyph / bookmark 等可在此接入。
pub(crate) struct IconQuad {
    pub row: usize,
    pub shaped: ShapedLine,
}

/// git diff 色条：gutter 左缘 3px 宽竖条，标记该行属于哪个 diff hunk。
struct GitBar {
    row: usize,
    color: Rgba,
}

/// 删除三角——用 GPU 原生三角形绘制。
struct DeletionMark {
    row: usize,
    /// true = ◣（左下角朝下的直角三角形），false = ◤（左上角朝上的直角三角形）。
    is_top: bool,
    color: Rgba,
}

/// gutter 的 prepaint 产物。
pub(crate) struct Prepaint {
    enabled: bool,
    line_numbers: Vec<Option<ShapedLine>>,
    icons: Vec<IconQuad>,
    git_bars: Vec<GitBar>,
    deletion_marks: Vec<DeletionMark>,
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
            git_bars: Vec::new(),
            deletion_marks: Vec::new(),
            number_width: px(0.),
        }
    }

    /// 正文起点相对 gutter 左缘的水平偏移：`左padding + 数字列 + 右padding`，关闭时为 0。
    /// 左右 padding 对称，色条浮在左 padding 上。
    pub(crate) fn offset(&self) -> Pixels {
        if self.enabled {
            space::s16() + self.number_width + space::s16()
        } else {
            px(0.)
        }
    }
}

/// 软换行场景下，按视觉行**预先测量**数字列宽。
///
/// 仅依赖 buffer 总行数与字体度量，与具体视口切片无关。
/// 用来在 prepaint 早期、还没决定每条逻辑行要拆成多少视觉行时，先算出正文起点的 X 偏移，以便用「正文区宽度」推断软换行的断行宽度。
pub(crate) fn measure_offset(
    total_lines: u64,
    text_style: &gpui::TextStyle,
    font_size: Pixels,
    window: &mut Window,
) -> Pixels {
    space::s16() + measure_number_width(total_lines, text_style, font_size, window) + space::s16()
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
/// `visual_rows` 长度等于视觉行数。每条视觉行都携带 `line_index`（属于哪个逻辑行），
/// 软换行的续行与首段同属一个逻辑行；`subrow == 0` 表示首段，需要画行号。
///
/// `diff_hunks` 是当前文件的 git diff hunk 列表，用于在行号左侧画添加/修改/删除色条。
pub(crate) fn prepare(
    visual_rows: &[VisualRow],
    total_lines: u64,
    text_style: &gpui::TextStyle,
    font_size: Pixels,
    diff_hunks: &[DiffHunk],
    window: &mut Window,
) -> Prepaint {
    let number_width = measure_number_width(total_lines, text_style, font_size, window);

    let mut line_numbers = Vec::with_capacity(visual_rows.len());
    let mut git_bars = Vec::new();
    let mut deletion_marks = Vec::new();

    // 行号 → 首段视觉行索引映射，用于删除标记定位。
    let mut first_visual_row: Vec<Option<usize>> = vec![None; total_lines as usize];
    for (visual_row, vr) in visual_rows.iter().enumerate() {
        if vr.subrow == 0 {
            let idx = vr.line_index as usize;
            if idx < first_visual_row.len() {
                first_visual_row[idx] = Some(visual_row);
            }
        }
    }

    // 缓存上一次 diff hunk 查表结果，避免同逻辑行的续行重复遍历 hunk 列表。
    let mut last_line_index: Option<u64> = None;
    let mut last_bar_color: Option<Rgba> = None;

    for (visual_row, vr) in visual_rows.iter().enumerate() {
        // 行号：仅首段视觉行（subrow == 0）需要画
        if vr.subrow == 0 {
            let label = (vr.line_index + 1).to_string();
            line_numbers.push(Some(shape_digits(&label, text_style, font_size, window)));
        } else {
            line_numbers.push(None);
        }

        // git diff 色条：逻辑行变化时才重新查 hunk，续行复用缓存结果
        if last_line_index != Some(vr.line_index) {
            last_line_index = Some(vr.line_index);
            last_bar_color = None;
            for hunk in diff_hunks {
                let start = hunk.new_start.saturating_sub(1) as u64; // 1-based → 0-based
                let end = start + hunk.new_lines as u64;
                if vr.line_index >= start && vr.line_index < end {
                    last_bar_color = Some(match hunk.kind {
                        DiffHunkKind::Added => {
                            color::git_status(crate::git_service::ColorKind::Added)
                        }
                        DiffHunkKind::Modified => {
                            color::git_status(crate::git_service::ColorKind::Modified)
                        }
                        DiffHunkKind::Deleted => {
                            color::git_status(crate::git_service::ColorKind::Deleted)
                        }
                    });
                    break; // 一个行只属于一个 hunk
                }
            }
        }
        if let Some(bar_color) = last_bar_color {
            git_bars.push(GitBar {
                row: visual_row,
                color: bar_color,
            });
        }
    }

    // 删除三角：上方行 ◣（左下填充），下方行 ◤（左上填充），共同指向删除间隙。
    let max_line = total_lines.saturating_sub(1);
    let del_color = color::git_status(crate::git_service::ColorKind::Deleted);
    for hunk in diff_hunks {
        if !matches!(hunk.kind, DiffHunkKind::Deleted) {
            continue;
        }
        // git diff -U0 在多 hunk 场景下 +new_start 比实际位置少 1，
        // 因此用 (new_start-1, new_start) 替代理论值 (new_start-2, new_start-1)。
        let before = hunk.new_start.saturating_sub(1) as u64; // 间隙上方行（0-based）
        let after = hunk.new_start as u64; // 间隙下方行（0-based）
        if before <= max_line {
            if let Some(Some(vr)) = first_visual_row.get(before as usize).copied() {
                deletion_marks.push(DeletionMark {
                    row: vr,
                    is_top: true,
                    color: del_color,
                });
            }
        }
        if after <= max_line {
            if let Some(Some(vr)) = first_visual_row.get(after as usize).copied() {
                deletion_marks.push(DeletionMark {
                    row: vr,
                    is_top: false,
                    color: del_color,
                });
            }
        }
    }

    Prepaint {
        enabled: true,
        line_numbers,
        icons: Vec::new(),
        git_bars,
        deletion_marks,
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
/// 行号右对齐到「左padding + 数字列」的右缘——每个字形宽度不同时低位对低位、高位对高位。
/// 图标按 row 索引贴 `bounds_x` 起绘。两类绘制都在 gutter 区域内（自建 ContentMask），不溢出到正文。
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
    let total_width = space::s16() + prepaint.number_width + space::s16();
    let gutter_area = Bounds {
        origin: point(bounds_x, bounds_top),
        size: size(total_width, bounds_height),
    };
    // 行号右对齐到「左padding + 数字列」的右缘，左 padding 留给色条浮动。
    let number_right_edge_x = bounds_x + space::s16() + prepaint.number_width;
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
                // 右对齐：个位对个位、十位对十位
                let x = number_right_edge_x - line_number.width;
                let _ = line_number.paint(point(x, y), line_height, window, cx);
            }
            for icon in &prepaint.icons {
                let y = top + line_height * icon.row as f32;
                let _ = icon
                    .shaped
                    .paint(point(bounds_x, y), line_height, window, cx);
            }
            // 删除三角：GPU 原生三角形，上 ◣ 下 ◤ 在间隙处无缝拼接。
            let tri_w = space::gutter_bar();
            let tri_h = line_height * 0.33;
            for mark in &prepaint.deletion_marks {
                let y = top + line_height * mark.row as f32;
                let path = if mark.is_top {
                    // ◣：左下角直角三角形，从 y+h-tri_h 到 y+h，与下方 ◤ 对称。
                    let tip_y = y + line_height - tri_h;
                    let mut p = Path::new(point(bounds_x, tip_y));
                    p.line_to(point(bounds_x + tri_w, y + line_height));
                    p.line_to(point(bounds_x, y + line_height));
                    p
                } else {
                    // ◤：左上角直角三角形，从 y 到 y+tri_h，与上方 ◣ 对称。
                    let mut p = Path::new(point(bounds_x, y));
                    p.line_to(point(bounds_x + tri_w, y));
                    p.line_to(point(bounds_x, y + tri_h));
                    p
                };
                window.paint_path(path, mark.color);
            }
            // git diff 色条：全高，贴 gutter 左缘
            for bar in &prepaint.git_bars {
                let y = top + line_height * bar.row as f32;
                let bar_rect = Bounds {
                    origin: point(bounds_x, y),
                    size: size(space::gutter_bar(), line_height),
                };
                window.paint_quad(fill(bar_rect, bar.color));
            }
        },
    );
}
