//! 文件树的具体行渲染。
//!
//! 输入：[`FileTreeState`] 已是 flatten 后的可见行序列（已应用展开/折叠 +
//! 排序），本文件负责"画出来"+"接受键盘焦点并把按键转给 App"。
//!
//! 焦点宿主（track_focus + on_key_down 的那个外层 div）在任何状态下都得
//! 在树里——包括"尚未打开项目"占位——否则在打开项目的瞬间 `window.focus`
//! 找不到挂载点，焦点请求就会丢失。

use std::rc::Rc;

use gpui::{AnyElement, Div, IntoElement, Svg, div, prelude::*, svg};

use crate::shell::normalized_chord;
use crate::shell::shared::theme::{color, icon, radius, space, typography};
use crate::shell::workbench::PanelContext;
use zom_workspace::EntryKind;

use super::{FileTreeRow, FileTreeState};

const FOLDER_ICON: &str = "icons/features/file_tree/folder.svg";
const FOLDER_OPEN_ICON: &str = "icons/features/file_tree/folder_open.svg";
const FILE_ICON: &str = "icons/features/file_tree/file.svg";

/// 单层缩进对应的像素宽度。
fn indent_unit() -> gpui::Pixels {
    space::s12()
}

pub(super) fn render(ctx: PanelContext<'_>) -> Div {
    let panel = ctx.file_tree;
    let key_request = Rc::clone(panel.key_request);

    let body: AnyElement = if !ctx.has_project {
        empty_message("尚未打开项目").into_any_element()
    } else if panel.state.rows.is_empty() {
        empty_message("项目目录为空").into_any_element()
    } else {
        render_list(panel.state, panel.is_focused).into_any_element()
    };

    div()
        .size_full()
        .track_focus(panel.focus)
        .tab_index(0)
        .on_key_down(move |event, window, cx| {
            let chord = normalized_chord(&event.keystroke);
            if key_request(chord, window, cx) {
                cx.stop_propagation();
            }
        })
        .child(body)
}

fn render_list(state: &FileTreeState, is_focused: bool) -> impl IntoElement + use<> {
    let list = div()
        .id("file-tree-list")
        .flex()
        .flex_col()
        .size_full()
        .overflow_y_scroll();
    state.rows.iter().fold(list, |list, row| {
        list.child(render_row(row, state, is_focused))
    })
}

fn empty_message(hint: &'static str) -> Div {
    div().flex().flex_col().size_full().child(
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_size(typography::body())
            .text_color(color::gray::g60())
            .child(hint),
    )
}

fn render_row(row: &FileTreeRow, state: &FileTreeState, is_focused: bool) -> Div {
    let is_selected = state
        .selected
        .as_ref()
        .map(|p| p == &row.path)
        .unwrap_or(false);
    let is_active = state
        .active
        .as_ref()
        .map(|p| p == &row.path)
        .unwrap_or(false);

    // 边框始终占 1px，保证选中态切换时行高不抖。失焦时直接染透明，让选中
    // 标记瞬时消失；获焦后再画上。
    let border_color = if is_selected && is_focused {
        color::focus::border()
    } else {
        gpui::rgba(0)
    };
    let bg_color = if is_active {
        color::gray::g20()
    } else {
        gpui::rgba(0)
    };
    let text_color = if is_active {
        color::gray::g95()
    } else {
        color::gray::g90()
    };

    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(space::s4())
        .overflow_hidden()
        .px(space::s4())
        .rounded(radius::r2())
        .border_1()
        .border_color(border_color)
        .bg(bg_color)
        .pl(indent_unit() * (row.depth as f32) + space::s4())
        .text_size(typography::body())
        .text_color(text_color)
        .child(icon_cell(row, is_active))
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .truncate()
                .child(row.name.clone()),
        )
}

fn icon_cell(row: &FileTreeRow, is_active: bool) -> Div {
    div()
        .flex_shrink_0()
        .size(icon::i16())
        .child(entry_icon(row, is_active))
}

fn entry_icon(row: &FileTreeRow, is_active: bool) -> Svg {
    let path = match row.kind {
        EntryKind::Directory if row.expanded => FOLDER_OPEN_ICON,
        EntryKind::Directory => FOLDER_ICON,
        EntryKind::File => FILE_ICON,
    };
    let tint = if is_active {
        color::gray::g95()
    } else {
        color::gray::g75()
    };
    svg().path(path).size(icon::i16()).text_color(tint)
}
