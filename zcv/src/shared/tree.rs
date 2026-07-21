use gpui::{div, prelude::*, px, svg};

use crate::theme::{color, radius, space, typography};

const FOLDER: &str = "icons/files/folder.svg";
const FOLDER_OPEN: &str = "icons/files/folder_open.svg";
const FILE: &str = "icons/files/file.svg";

fn indent_unit() -> gpui::Pixels {
    typography::ui()
}

/// 树行骨架：relative + flex-row + items_center + 缩进 + 字型。
fn row_skeleton(depth: usize) -> gpui::Div {
    div()
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .h(typography::ui())
        .gap(space::S4)
        .rounded(radius::R2)
        .pl(indent_unit() * (depth as f32))
}

/// 渲染缩进竖线——只画祖辈级，本级留给子行去画。
fn guide_lines(depth: usize) -> impl IntoElement {
    let line_color = color::current().gray.s[4];
    let line_w = px(1.0);
    let row_h = typography::ui();

    let mut container = div().absolute().top(px(0.)).left(px(0.)).w_full().h(row_h);
    let icon_half = typography::ui() / 2.0;

    for k in 0..depth {
        let x_center = indent_unit() * (k as f32) + icon_half;
        container = container.child(
            div()
                .absolute()
                .left(x_center - line_w / 2.0)
                .top(px(0.))
                .w(line_w)
                .h(row_h)
                .bg(line_color),
        );
    }
    container
}

/// 根据条目类型和展开/折叠状态返回对应的图标元素。
fn icon(is_dir: bool, expanded: bool) -> impl IntoElement {
    let path = if is_dir {
        if expanded { FOLDER_OPEN } else { FOLDER }
    } else {
        FILE
    };
    div().flex_shrink_0().size(typography::ui()).child(
        svg()
            .path(path)
            .size(typography::ui())
            .text_color(color::current().gray.s[7]),
    )
}

/// 条目名称文本，尾部溢出截断。
fn label(name: &str) -> gpui::Div {
    div()
        .flex_1()
        .overflow_hidden()
        .truncate()
        .child(name.to_string())
}

/// 树行完整渲染：行骨架 + 缩进竖线 + 图标 + 名称。
pub(crate) fn render_row_base(depth: usize, is_dir: bool, expanded: bool, name: &str) -> gpui::Div {
    row_skeleton(depth)
        .child(guide_lines(depth))
        .child(icon(is_dir, expanded))
        .child(label(name))
}

/// 选中行蓝框——absolute 覆盖整行，不参与行布局。
/// `.when(is_selected, |el| el.child(tree::selection_border()))`
pub(crate) fn selection_border() -> gpui::Div {
    div()
        .absolute()
        .top(px(0.))
        .left(px(0.))
        .right(px(0.))
        .h(typography::ui())
        .rounded(radius::R2)
        .border_1()
        .border_color(color::current().blue.s[6])
}
