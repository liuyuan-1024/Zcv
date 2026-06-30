//! VersionControl 面板的行渲染。
//!
//! 输入：[`VersionControlState`] 已是 DFS 扁平化后的可见行序列，本文件负责"画出来"。
//! 缩进连线与图标复用文件树的渲染模式。

use std::path::PathBuf;
use std::rc::Rc;

use gpui::{Div, MouseButton, Window, div, prelude::*, uniform_list};

use crate::shell::shared::scroll;
use crate::shell::shared::tree::{self};
use crate::theme::{color, space, typography};

use super::{VersionControlRow, VersionControlState};

/// 渲染整个变更文件列表。
pub(super) fn render_list(
    state: &VersionControlState,
    selected: Option<PathBuf>,
    on_click: impl Fn(PathBuf, &mut Window, &mut gpui::App) + 'static,
) -> Div {
    let rows = state.rows.clone();
    let on_click = Rc::new(on_click);
    let selected = Rc::new(selected);

    let scroll_handle = scroll::ScrollHandle::new();

    div()
        .relative()
        .size_full()
        .overflow_hidden()
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
                        let selected = Rc::clone(&selected);
                        let path = row.path.clone();
                        let is_selected = selected
                            .as_ref()
                            .as_ref()
                            .map(|p| p == &row.path)
                            .unwrap_or(false);
                        render_row(row, is_selected, move |window, cx| {
                            on_click(path.clone(), window, cx)
                        })
                        .into_any_element()
                    })
                    .collect()
            })
            .size_full()
            .track_scroll(scroll_handle.inner()),
        )
        .child(scroll::scrollbar(&scroll_handle))
}

/// 渲染单行。
fn render_row<F>(row: &VersionControlRow, is_selected: bool, on_click: F) -> Div
where
    F: Fn(&mut Window, &mut gpui::App) + 'static,
{
    let mut row_div = tree::row_skeleton(row.depth)
        .child(tree::guide_lines(row.depth))
        .child(tree::icon(row.is_dir, row.expanded))
        .child(tree::label(&row.name));

    if let Some(kind) = row.git_color {
        row_div = row_div.text_color(color::git_status(kind));
    }

    row_div = row_div.hover(|style| style.bg(color::current().gray.s04));

    if is_selected {
        row_div = row_div.child(tree::selection_overlay());
    }

    row_div
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            on_click(window, cx);
        })
}
