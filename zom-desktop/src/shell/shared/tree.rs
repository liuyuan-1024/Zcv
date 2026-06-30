//! 树形组件——行骨架、缩进竖线、图标、选中框。

use gpui::{div, prelude::*, px, svg};

use crate::theme::{color, radius, typography};

/// 文件夹图标
pub(crate) const FOLDER_ICON: &str = "icons/files/folder.svg";
/// 展开态文件夹图标
pub(crate) const FOLDER_OPEN_ICON: &str = "icons/files/folder_open.svg";
/// 文件图标
pub(crate) const FILE_ICON: &str = "icons/files/file.svg";

/// 单层缩进对应的像素宽度，等于图标尺寸。
pub(crate) fn indent_unit() -> gpui::Pixels {
    typography::ui_line()
}

// ── 行骨架 ──

/// 树行骨架：relative + flex-row + items_center + 缩进 + 字型。
/// 消费方在此基础上继续链式调用 `.child(…)`、`.hover(…)` 等来定制。
pub(crate) fn row_skeleton(depth: usize) -> gpui::Div {
    div()
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .gap(typography::ui_line() / 4.0) // ≈ 图标间距
        .rounded(radius::r2())
        .pl(indent_unit() * (depth as f32))
        .text_size(typography::ui())
        .line_height(typography::ui_line())
}

// ── 缩进竖线 ──

/// 渲染缩进竖线——只画祖辈级，本级留给子行去画。
///
/// 参照 Zed：竖线从父级图标中心向下延伸到子行。当前行不画自己这一级的竖线。
pub(crate) fn guide_lines(depth: usize) -> impl IntoElement {
    let line_color = crate::theme::color::current().gray.s05;
    let line_w = px(1.0);
    let row_h = typography::ui_line();

    let mut container = div().absolute().top(px(0.)).left(px(0.)).w_full().h(row_h);

    let icon_half = typography::ui_line() / 2.0;

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

// ── 图标 ──

/// 根据条目类型和展开/折叠状态返回对应的图标元素。
pub(crate) fn icon(is_dir: bool, expanded: bool) -> impl IntoElement {
    let path = if is_dir {
        if expanded {
            FOLDER_OPEN_ICON
        } else {
            FOLDER_ICON
        }
    } else {
        FILE_ICON
    };
    div().flex_shrink_0().size(typography::ui_line()).child(
        svg()
            .path(path)
            .size(typography::ui_line())
            .text_color(color::current().gray.s09),
    )
}

// ── 名称 ──

/// 条目名称文本，尾部溢出截断。
pub(crate) fn label(name: &str) -> gpui::Div {
    div()
        .flex_1()
        .overflow_hidden()
        .truncate()
        .child(name.to_string())
}

// ── 选中框 ──

/// 选中行浮层边框——absolute 覆盖整行，不参与布局，不影响竖线拼接。
pub(crate) fn selection_overlay() -> impl IntoElement {
    div()
        .absolute()
        .top(px(0.))
        .left(px(0.))
        .size_full()
        .rounded(radius::r2())
        .border_1()
        .border_color(color::current().blue.s07)
}
