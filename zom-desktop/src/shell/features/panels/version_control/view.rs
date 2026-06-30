//! VersionControl 面板的行渲染。
//!
//! 输入：[`VersionControlState`] 已是 DFS 扁平化后的可见行序列，本文件负责"画出来"。
//! 缩进连线与图标复用文件树的渲染模式。

use std::path::PathBuf;
use std::rc::Rc;

use gpui::{Div, MouseButton, Window, div, prelude::*, px, rgba, svg, uniform_list};

use crate::shell::shared::scroll;
use crate::shell::shared::tree::{self};
use crate::theme::{color, space, typography};

use super::{VersionControlRow, VersionControlState};

/// 渲染整个变更文件列表。
pub(super) fn render_list(
    state: &VersionControlState,
    selected: Option<PathBuf>,
    on_click: impl Fn(PathBuf, &mut Window, &mut gpui::App) + 'static,
    on_checkbox_click: impl Fn(PathBuf, &mut Window, &mut gpui::App) + 'static,
) -> Div {
    let rows = state.rows.clone();
    let on_click = Rc::new(on_click);
    let on_checkbox_click = Rc::new(on_checkbox_click);
    let selected = Rc::new(selected);

    let scroll_handle = scroll::ScrollHandle::new();

    let selected_index = selected
        .as_ref()
        .as_ref()
        .and_then(|sel| rows.iter().position(|r| &r.path == sel));

    div()
        .relative()
        .size_full()
        .p(space::s4())
        .text_size(typography::ui())
        .line_height(typography::ui_line())
        .text_color(color::current().gray.s08)
        .child(
            uniform_list("version-control-list", rows.len(), move |range, _, _| {
                range
                    .filter_map(|index| rows.get(index))
                    .map(|row| {
                        let on_click = Rc::clone(&on_click);
                        let on_checkbox_click = Rc::clone(&on_checkbox_click);
                        let path = row.path.clone();
                        render_row(row, move |window, cx| on_click(path.clone(), window, cx), {
                            let path = row.path.clone();
                            move |window, cx| on_checkbox_click(path.clone(), window, cx)
                        })
                        .into_any_element()
                    })
                    .collect()
            })
            .size_full()
            .track_scroll(scroll_handle.inner()),
        )
        .child(scroll::scrollbar(&scroll_handle))
        .children(tree::list_selection_overlay(selected_index, &scroll_handle))
}

/// 渲染单行。
fn render_row<F, G>(row: &VersionControlRow, on_click: F, on_checkbox_click: G) -> Div
where
    F: Fn(&mut Window, &mut gpui::App) + 'static,
    G: Fn(&mut Window, &mut gpui::App) + 'static,
{
    let mut row_div = tree::render_row_base(row.depth, row.is_dir, row.expanded, &row.name);

    // 文件行行尾渲染暂存复选框；目录行不渲染。
    if !row.is_dir {
        row_div = row_div.child(render_checkbox(row.staged, on_checkbox_click));
    }

    if let Some(kind) = row.git_color {
        row_div = row_div.text_color(color::git_status(kind));
    }

    row_div = row_div.hover(|style| style.bg(color::current().gray.s04));

    row_div
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            on_click(window, cx);
        })
}

/// 渲染暂存复选框——CSS 方形外框 + SVG 勾选标记（参照 Zed Checkbox）。
///
/// 点击时调用 `on_click` 并阻止事件冒泡，避免触发行激活（打开文件）。
fn render_checkbox(
    staged: bool,
    on_click: impl Fn(&mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let size = typography::ui_line() * 0.75;
    let rounded = px(3.0);

    let (bg, border) = if staged {
        (color::current().blue.s07, color::current().blue.s07)
    } else {
        (rgba(0), color::current().gray.s06)
    };

    div()
        .flex_shrink_0()
        .size(size)
        .rounded(rounded)
        .border_1()
        .border_color(border)
        .bg(bg)
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .when(staged, |el| {
            el.child(
                svg()
                    .path("icons/actions/check.svg")
                    .size(size - px(2.0))
                    .text_color(rgba(0xffffffff)),
            )
        })
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            on_click(window, cx);
        })
}
