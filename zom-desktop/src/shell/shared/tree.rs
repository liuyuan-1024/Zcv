//! 树形组件——行骨架、缩进竖线、图标、选中框。

use gpui::{div, prelude::*, px, svg};

use super::scroll;
use crate::theme::{color, radius, space, typography};

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

// ── 行完整渲染 ──

/// 树行完整渲染：行骨架 + 缩进竖线 + 图标 + 名称。
///
/// 消费方在此基础上继续链式调用 `.bg()`、`.hover()` 等来定制行外观。
pub(crate) fn render_row_base(depth: usize, is_dir: bool, expanded: bool, name: &str) -> gpui::Div {
    row_skeleton(depth)
        .child(guide_lines(depth))
        .child(icon(is_dir, expanded))
        .child(label(name))
}

// ── 选中框 ──

/// 容器层选中浮层——渲染在 uniform_list 外部，避免列表裁切。
///
/// `selected_index` 是选中行在 flat list 中的索引，`None` 时无选中。
pub(crate) fn list_selection_overlay(
    selected_index: Option<usize>,
    scroll_handle: &scroll::ScrollHandle,
) -> Option<gpui::Div> {
    selected_index.map(|idx| {
        let top =
            space::s4() + typography::ui_line() * (idx as f32) - scroll_handle.scroll_offset_y();
        selection_overlay().top(top)
    })
}

/// 选中行浮层边框——absolute 覆盖整行，不参与布局。
fn selection_overlay() -> gpui::Div {
    div()
        .absolute()
        .top(px(0.))
        .left(px(0.))
        .right(px(0.))
        .h(typography::ui_line())
        .rounded(radius::r2())
        .border_1()
        .border_color(color::current().blue.s07)
}
