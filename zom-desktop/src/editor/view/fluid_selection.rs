//! 流体选区渲染 —— 把跨行选区的矩形合并为统一多边形外轮廓，
//! 对凸角 / 凹角分别做智能倒角，通过 PathBuilder 构建路径后一次性 GPU 填充。
//!
//! ## 算法概要
//!
//! 1. 收集 range 在可见行上的矩形参数（collect_rects）
//! 2. 构建顺时针多边形外轮廓顶点（build_vertices）
//! 3. 对每个顶点判断凸/凹（detect_corners）
//! 4. 凸角用 8 段 line_to 近似圆弧，凹角用二次贝塞尔曲线填充
//! 5. PathBuilder::build() 三角化 → paint_path 渲染

use gpui::{Bounds, Hsla, PathBuilder, Pixels, Point, Window, point, px};
use zom_engine::TextRange;

use crate::theme::radius;

use super::phases::{EOL_EXTENSION, LineMetric};

// ============================================================================
// 工具函数：Pixels → f32（gpui 的 Pixels.0 是 crate-private）
// ============================================================================

/// gpui 的 `Pixels.0` 字段对外部 crate 不可见，通过 `From<Pixels> for f32` 转换。
#[inline(always)]
fn pf(p: Pixels) -> f32 {
    f32::from(p)
}

// ============================================================================
// 数据结构
// ============================================================================

/// 选区在单行上的矩形几何参数（已吸收 text_left 偏移）。
struct RowRect {
    y_top: Pixels,
    y_bottom: Pixels,
    x_left: Pixels,
    x_right: Pixels,
}

/// 多边形顶点处的角类型。
#[derive(Clone, Copy, PartialEq)]
enum CornerKind {
    /// 凸角：多边形外突，用圆弧切角。
    Convex,
    /// 凹角：多边形内凹（notch），用贝塞尔曲线填充。
    Concave,
}

/// 圆弧近似的分段数。
const ARC_SEGMENTS: usize = 8;

// ============================================================================
// 公开入口
// ============================================================================

/// 用流体多边形渲染单个 range 的选区背景。
///
/// 对单行选区（0 或 1 个可见行）不做多边形渲染，返回 `false` 让调用方用逐行 quad 渲染。
/// PathBuilder 三角化失败时也返回 `false`。
pub(crate) fn paint_fluid_range(
    range: TextRange,
    color: Hsla,
    lines: &[LineMetric<'_>],
    text_left: Pixels,
    top: Pixels,
    line_height: Pixels,
    text_area: Bounds<Pixels>,
    window: &mut Window,
) -> bool {
    let rects = collect_rects(range, lines, text_left, top, line_height, text_area);
    if rects.len() < 2 {
        return false;
    }

    let vertices = build_vertices(&rects);
    if vertices.len() < 3 {
        return false;
    }

    let corners = detect_corners(&vertices);
    let r = f32::from(radius::r2());

    let path = match build_fluid_path(&vertices, &corners, r) {
        Some(p) => p,
        None => return false,
    };

    window.paint_path(path, color);
    true
}

// ============================================================================
// 步骤 1：收集行矩形
// ============================================================================

fn collect_rects(
    range: TextRange,
    lines: &[LineMetric<'_>],
    text_left: Pixels,
    top: Pixels,
    line_height: Pixels,
    text_area: Bounds<Pixels>,
) -> Vec<RowRect> {
    let r_start = range.start().get();
    let r_end = range.end().get();
    let viewport_top = text_area.origin.y;
    let viewport_bottom = viewport_top + text_area.size.height;

    let mut rects = Vec::new();

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

        rects.push(RowRect {
            y_top: row_top,
            y_bottom: row_bottom,
            x_left: text_left + x_start,
            x_right: text_left + x_end,
        });
    }

    rects
}

// ============================================================================
// 步骤 2：构建顺时针多边形外轮廓顶点
// ============================================================================

/// 从行矩形列表构建顺时针多边形顶点序列。
///
/// 轮廓路径：首行左上角 → 顶边 → 右边（逐行下行，含 notch 过渡）→ 底边 → 左边（逐行上行，含 notch 过渡）→ 闭合。
///
/// 连续重复顶点（同行宽时）会被去重；共线中间顶点被剔除。
fn build_vertices(rects: &[RowRect]) -> Vec<Point<Pixels>> {
    let n = rects.len();
    let mut verts: Vec<Point<Pixels>> = Vec::with_capacity(n * 2 + 4);

    // 顶边：首行左上 → 右上
    verts.push(point(rects[0].x_left, rects[0].y_top));
    push_vert(&mut verts, point(rects[0].x_right, rects[0].y_top));

    // 右边，逐行下行
    for i in 0..n {
        push_vert(&mut verts, point(rects[i].x_right, rects[i].y_bottom));
        if i + 1 < n {
            push_vert(&mut verts, point(rects[i + 1].x_right, rects[i + 1].y_top));
        }
    }

    // 底边：末行右下 → 左下
    push_vert(
        &mut verts,
        point(rects[n - 1].x_left, rects[n - 1].y_bottom),
    );

    // 左边，逐行上行
    for i in (0..n).rev() {
        push_vert(&mut verts, point(rects[i].x_left, rects[i].y_top));
        if i > 0 {
            push_vert(
                &mut verts,
                point(rects[i - 1].x_left, rects[i - 1].y_bottom),
            );
        }
    }

    // 去除与首顶点重复的末顶点
    if verts.len() > 1 && verts.first() == verts.last() {
        verts.pop();
    }

    remove_collinear(verts)
}

/// 仅当新顶点与当前末顶点不同时才推入。
fn push_vert(verts: &mut Vec<Point<Pixels>>, p: Point<Pixels>) {
    if verts.last() != Some(&p) {
        verts.push(p);
    }
}

/// 去除连续共线顶点中的中间顶点，避免零转角处误判凸/凹。
fn remove_collinear(verts: Vec<Point<Pixels>>) -> Vec<Point<Pixels>> {
    if verts.len() < 3 {
        return verts;
    }
    let n = verts.len();
    let mut result: Vec<Point<Pixels>> = Vec::with_capacity(n);

    for i in 0..n {
        let prev = verts[(i + n - 1) % n];
        let curr = verts[i];
        let next = verts[(i + 1) % n];

        let d1 = (pf(curr.x) - pf(prev.x), pf(curr.y) - pf(prev.y));
        let d2 = (pf(next.x) - pf(curr.x), pf(next.y) - pf(curr.y));
        let cross = d1.0 * d2.1 - d1.1 * d2.0;

        let dot = d1.0 * d2.0 + d1.1 * d2.1;
        let len1 = (d1.0 * d1.0 + d1.1 * d1.1).sqrt();
        let len2 = (d2.0 * d2.0 + d2.1 * d2.1).sqrt();
        let is_collinear = cross.abs() < 0.001 && dot > 0.0 && len1 > 0.001 && len2 > 0.001;

        if !is_collinear {
            result.push(curr);
        }
    }

    if result.len() < 3 {
        return verts;
    }
    result
}

// ============================================================================
// 步骤 3：凸/凹角检测
// ============================================================================

fn detect_corners(vertices: &[Point<Pixels>]) -> Vec<CornerKind> {
    let n = vertices.len();
    let mut corners = Vec::with_capacity(n);

    for i in 0..n {
        let prev = vertices[(i + n - 1) % n];
        let curr = vertices[i];
        let next = vertices[(i + 1) % n];

        let d1 = (pf(curr.x) - pf(prev.x), pf(curr.y) - pf(prev.y));
        let d2 = (pf(next.x) - pf(curr.x), pf(next.y) - pf(curr.y));

        let len1 = (d1.0 * d1.0 + d1.1 * d1.1).sqrt();
        let len2 = (d2.0 * d2.0 + d2.1 * d2.1).sqrt();

        if len1 < 0.001 || len2 < 0.001 {
            corners.push(CornerKind::Convex);
            continue;
        }

        let d1n = (d1.0 / len1, d1.1 / len1);
        let d2n = (d2.0 / len2, d2.1 / len2);
        let cross = d1n.0 * d2n.1 - d1n.1 * d2n.0;

        // 顺时针多边形：cross > 0 → 凸角，cross < 0 → 凹角
        if cross > 0.001 {
            corners.push(CornerKind::Convex);
        } else if cross < -0.001 {
            corners.push(CornerKind::Concave);
        } else {
            corners.push(CornerKind::Convex);
        }
    }

    corners
}

// ============================================================================
// 步骤 4：构建流体路径
// ============================================================================

fn build_fluid_path(
    vertices: &[Point<Pixels>],
    corners: &[CornerKind],
    r: f32,
) -> Option<gpui::Path<Pixels>> {
    let n = vertices.len();
    let mut pb = PathBuilder::fill();

    // 确定路径起点：若末顶点为凹角，从顶点本身开始；凸角从出弧切点开始
    let last_v = vertices[n - 1];
    if corners[n - 1] == CornerKind::Concave {
        pb.move_to(last_v);
    } else {
        let start_prev = vertices[n - 2];
        let start_next = vertices[0];
        let (_, start_tout) = tangent_points(start_prev, last_v, start_next, r);
        pb.move_to(start_tout);
    }

    for i in 0..n {
        let prev = vertices[(i + n - 1) % n];
        let curr = vertices[i];
        let next = vertices[(i + 1) % n];

        let (tin, tout) = tangent_points(prev, curr, next, r);

        match corners[i] {
            CornerKind::Convex => {
                pb.line_to(tin);
                emit_convex_arc(&mut pb, tin, tout, curr, prev, r);
            }
            CornerKind::Concave => {
                // 凹角不做切点偏移，直接走到顶点。
                // 原因：贝塞尔曲线在此处的切线与邻边不连续，
                // 切角三角形缺角会透过半透明背景形成可见黑点。
                pb.line_to(curr);
            }
        }
    }

    pb.close();
    pb.build().ok()
}

// ============================================================================
// 辅助：切点计算
// ============================================================================

/// 计算顶点 V 处的入弧切点和出弧切点。
///
/// T_in = V - r * d1（入边距角 r 处）
/// T_out = V + r * d2（出边距角 r 处）
fn tangent_points(
    prev: Point<Pixels>,
    curr: Point<Pixels>,
    next: Point<Pixels>,
    r: f32,
) -> (Point<Pixels>, Point<Pixels>) {
    let d1 = (pf(curr.x) - pf(prev.x), pf(curr.y) - pf(prev.y));
    let d2 = (pf(next.x) - pf(curr.x), pf(next.y) - pf(curr.y));

    let len1 = (d1.0 * d1.0 + d1.1 * d1.1).sqrt().max(0.001);
    let len2 = (d2.0 * d2.0 + d2.1 * d2.1).sqrt().max(0.001);

    let d1n = (d1.0 / len1, d1.1 / len1);
    let d2n = (d2.0 / len2, d2.1 / len2);

    let cx = pf(curr.x);
    let cy = pf(curr.y);
    let tin = point(px(cx - r * d1n.0), px(cy - r * d1n.1));
    let tout = point(px(cx + r * d2n.0), px(cy + r * d2n.1));

    (tin, tout)
}

// ============================================================================
// 辅助：凸角圆弧（8 段 line_to 近似）
// ============================================================================

/// 凸角：在 polygon 内部做圆弧切角。
///
/// 圆心 `C = V + r * (n1 - d1)`，其中 `n1 = (-d1.y, d1.x)` 是入边内法线。
fn emit_convex_arc(
    pb: &mut PathBuilder,
    t1: Point<Pixels>,
    t2: Point<Pixels>,
    v: Point<Pixels>,
    prev: Point<Pixels>,
    r: f32,
) {
    let _ = t1;

    let d1 = (pf(v.x) - pf(prev.x), pf(v.y) - pf(prev.y));
    let len1 = (d1.0 * d1.0 + d1.1 * d1.1).sqrt().max(0.001);
    let d1n = (d1.0 / len1, d1.1 / len1);
    // 入边内法线（顺时针多边形）：(-dy, dx)
    let n1 = (-d1n.1, d1n.0);

    let cx = pf(v.x) + r * (n1.0 - d1n.0);
    let cy = pf(v.y) + r * (n1.1 - d1n.1);

    emit_arc_segments(pb, t1, t2, cx, cy, r);
}

// ============================================================================
// 辅助：圆弧线段近似
// ============================================================================

/// 用 `ARC_SEGMENTS` 段 line_to 近似从 T1 到 T2 沿圆心的圆弧。
///
/// 自动选择短弧方向（使 sweep 绝对值 ≤ π）。
fn emit_arc_segments(
    pb: &mut PathBuilder,
    t1: Point<Pixels>,
    t2: Point<Pixels>,
    cx: f32,
    cy: f32,
    r: f32,
) {
    let a1 = f32::atan2(pf(t1.y) - cy, pf(t1.x) - cx);
    let a2 = f32::atan2(pf(t2.y) - cy, pf(t2.x) - cx);

    // 选择短弧方向
    let mut sweep = a2 - a1;
    if sweep > std::f32::consts::PI {
        sweep -= 2.0 * std::f32::consts::PI;
    } else if sweep < -std::f32::consts::PI {
        sweep += 2.0 * std::f32::consts::PI;
    }

    for i in 1..=ARC_SEGMENTS {
        let a = a1 + sweep * (i as f32 / ARC_SEGMENTS as f32);
        pb.line_to(point(px(cx + r * a.cos()), px(cy + r * a.sin())));
    }
}
