//! 文件树的具体行渲染。
//!
//! 输入：[`FileTreeState`] 已是 flatten 后的可见行序列（已应用展开/折叠 +
//! 排序），本文件负责"画出来"+"接受键盘焦点并把按键转给 App"。
//!
//! 焦点宿主（track_focus + on_key_down 的那个外层 div）在任何状态下都得
//! 在树里——包括"尚未打开项目"占位——否则在打开项目的瞬间 `window.focus`
//! 找不到挂载点，焦点请求就会丢失。

use std::rc::Rc;

use gpui::{AnyElement, Div, Svg, div, prelude::*, svg, uniform_list};

use crate::shell::editor::{EditorKind, TextEditorSlot};
use crate::shell::normalized_chord;
use crate::shell::shared::scroll;
use crate::shell::shared::theme::{color, radius, space, typography};
use crate::shell::workbench::PanelContext;
use zom_workspace::EntryKind;

use super::{FileTreeRow, FileTreeState, PendingNewEntry};

const FOLDER_ICON: &str = "icons/files/folder.svg";
const FOLDER_OPEN_ICON: &str = "icons/files/folder_open.svg";
const FILE_ICON: &str = "icons/files/file.svg";

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
        render_list(panel.state, panel.is_focused, panel.slot, panel.scroll).into_any_element()
    };

    div()
        .size_full()
        .track_focus(panel.focus)
        .tab_index(0)
        .on_key_down(move |event, window, cx| {
            if key_request(normalized_chord(&event.keystroke), window, cx) {
                cx.stop_propagation();
            }
        })
        .child(body)
}

fn render_list(
    state: &FileTreeState,
    is_focused: bool,
    slot: &Rc<TextEditorSlot>,
    scroll_handle: &scroll::ScrollHandle,
) -> Div {
    let items = logical_items(state);
    let selected_item = selected_item_index(&items, state);
    let selected = state.selected.clone();
    let active = state.active.clone();
    let slot = Rc::clone(slot);
    if let Some(index) = selected_item.filter(|index| *index < items.len()) {
        scroll_handle.reveal_item_if_changed(index);
    }

    div()
        .relative()
        .size_full()
        .overflow_hidden()
        .child(
            uniform_list("file-tree-list", items.len(), move |range, _, _| {
                range
                    .filter_map(|index| items.get(index))
                    .map(|item| match item {
                        FileTreeItem::Row(row) => {
                            render_row(row, selected.as_ref(), active.as_ref(), is_focused)
                                .into_any_element()
                        }
                        FileTreeItem::Pending(pending) => {
                            render_input_row(pending, &slot).into_any_element()
                        }
                    })
                    .collect()
            })
            .size_full()
            .track_scroll(scroll_handle.inner()),
        )
        .child(scroll::scrollbar(scroll_handle))
}

#[derive(Clone)]
enum FileTreeItem {
    Row(FileTreeRow),
    Pending(PendingNewEntry),
}

fn logical_items(state: &FileTreeState) -> Vec<FileTreeItem> {
    let mut items = Vec::new();
    for row in &state.rows {
        items.push(FileTreeItem::Row(row.clone()));
        // 新建条目的输入行紧跟在其父目录行之后。
        if let Some(pending) = &state.pending
            && pending.parent == row.path
        {
            items.push(FileTreeItem::Pending(pending.clone()));
        }
    }
    items
}

fn selected_item_index(items: &[FileTreeItem], state: &FileTreeState) -> Option<usize> {
    if state.pending.is_some() {
        return items
            .iter()
            .position(|item| matches!(item, FileTreeItem::Pending(_)));
    }
    state.selected.as_ref().and_then(|selected| {
        items
            .iter()
            .position(|item| matches!(item, FileTreeItem::Row(row) if &row.path == selected))
    })
}

/// 新建态的内联输入行：父目录行下方，带文件/目录图标、已键入名称与光标。
///
/// 名称输入框直接嵌入文件树 pending 名称 [`TextEditorSlot`] —— 与主编辑区
/// 是同一个编辑器，只是 `EditorKind::SingleLine`。本行（边框 / 图标 / 缩进）
/// 是它的外壳。
fn render_input_row(pending: &PendingNewEntry, slot: &Rc<TextEditorSlot>) -> Div {
    let icon = match pending.kind {
        EntryKind::Directory => FOLDER_OPEN_ICON,
        EntryKind::File => FILE_ICON,
    };
    // 文本与光标位由 slot.embed 内部从 router 拉，pending.editor 这里不再需要。
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(space::s4())
        .overflow_hidden()
        .px(space::s4())
        .rounded(radius::r2())
        .border_1()
        .border_color(color::focus::border())
        .pl(indent_unit() * (pending.depth as f32) + space::s4())
        .text_size(typography::ui())
        .text_color(color::gray::g95())
        .child(
            div().flex_shrink_0().size(typography::ui_line()).child(
                svg()
                    .path(icon)
                    .size(typography::ui_line())
                    .text_color(color::gray::g95()),
            ),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .items_center()
                .overflow_hidden()
                .line_height(typography::ui_line())
                .child(slot.embed(EditorKind::SingleLine)),
        )
}

fn empty_message(hint: &'static str) -> Div {
    div().flex().flex_col().size_full().child(
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_size(typography::ui())
            .text_color(color::gray::g75())
            .child(hint),
    )
}

fn render_row(
    row: &FileTreeRow,
    selected: Option<&std::path::PathBuf>,
    active: Option<&std::path::PathBuf>,
    is_focused: bool,
) -> Div {
    let is_selected = selected.map(|p| p == &row.path).unwrap_or(false);
    let is_active = active.map(|p| p == &row.path).unwrap_or(false);

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
    // 与 top bar Glyph 同基线：常态 g75，活动项 g95。
    let text_color = if is_active {
        color::gray::g95()
    } else {
        color::gray::g75()
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
        .text_size(typography::ui())
        .line_height(typography::ui_line())
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
        .size(typography::ui_line())
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
    svg()
        .path(path)
        .size(typography::ui_line())
        .text_color(tint)
}
