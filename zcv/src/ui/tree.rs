//! 树行渲染辅助函数 —— 缩进、图标、名称、选中框。

use gpui::{Pixels, div, prelude::*, px};

use crate::ui::SvgIcon;
use zcv_theme::{color, radius, space, typography};

const FOLDER: &str = "icons/files/folder.svg";
const FOLDER_OPEN: &str = "icons/files/folder_open.svg";
const FILE: &str = "icons/files/file.svg";

/// 树行完整渲染：行骨架 + 缩进竖线 + 图标 + 名称。
pub(crate) fn render_row_base(depth: usize, is_dir: bool, expanded: bool, name: &str) -> gpui::Div {
    row_skeleton(depth)
        .children(guide_lines(depth))
        .child(icon(is_dir, expanded))
        .child(label(name))
}

/// 选中框——absolute 覆盖整行，不参与行布局。
pub(crate) fn selection_border() -> gpui::Div {
    let m = metrics();
    div()
        .absolute()
        .top(Pixels::ZERO)
        .left(Pixels::ZERO)
        .right(Pixels::ZERO)
        .h(m.row_height)
        .rounded(radius::R2)
        .border_1()
        .border_color(color::highlight())
}

// ── 私有辅助函数 ─────────────────────────────────────────────────────

/// 树行布局度量。
struct TreeMetrics {
    row_height: gpui::Pixels,
    indent: gpui::Pixels,
    padding: gpui::Pixels,
}

fn metrics() -> TreeMetrics {
    let padding = space::S6;
    let indent = typography::ui();
    TreeMetrics {
        row_height: indent + padding,
        indent,
        padding,
    }
}

impl TreeMetrics {
    fn indent_left(&self, depth: usize) -> gpui::Pixels {
        self.indent * (depth as f32) + self.padding
    }

    fn guide_x(&self, depth: usize) -> gpui::Pixels {
        self.indent * (depth as f32) + self.indent / 2.0 + self.padding
    }
}

/// 树行骨架：relative + flex-row + items_center + 缩进 + 字型。
fn row_skeleton(depth: usize) -> gpui::Div {
    let m = metrics();
    div()
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .h(m.row_height)
        .pl(m.indent_left(depth))
        .rounded(radius::R2)
}

/// 渲染缩进竖线——每条线直接 absolute 定位在行上。
fn guide_lines(depth: usize) -> Vec<gpui::Div> {
    let m = metrics();
    let line_color = color::current().gray.s[4];
    let line_w = px(1.0);

    (0..depth)
        .map(|k| {
            let x_center = m.guide_x(k);
            div()
                .absolute()
                .left(x_center - line_w / 2.0)
                .top(Pixels::ZERO)
                .w(line_w)
                .h_full()
                .bg(line_color)
        })
        .collect()
}

/// 根据条目类型和展开/折叠状态返回对应的图标元素。
fn icon(is_dir: bool, expanded: bool) -> impl IntoElement {
    let path = if is_dir {
        if expanded { FOLDER_OPEN } else { FOLDER }
    } else {
        FILE
    };
    div().flex_shrink_0().child(SvgIcon::new(path))
}

/// 条目名称文本，尾部溢出截断。
fn label(name: &str) -> gpui::Div {
    div()
        .flex_1()
        .overflow_hidden()
        .truncate()
        .child(name.to_string())
}
