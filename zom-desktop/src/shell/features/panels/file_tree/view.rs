//! 文件树的具体行渲染。
//!
//! 输入：[`FileTreeState`] 已是 flatten 后的可见行序列（已应用展开/折叠 + 排序），本文件负责"画出来"+"接受键盘焦点并把按键转给 App"。
//!
//! 焦点宿主（track_focus + on_key_down 的那个外层 div）在任何状态下都得在树里——包括"尚未打开项目"占位——否则在打开项目的瞬间 `window.focus` 找不到挂载点，焦点请求就会丢失。

use std::collections::BTreeSet;
use std::rc::Rc;

use gpui::{AnyElement, Div, MouseButton, div, prelude::*, uniform_list};

use crate::editor::TextEditorSlot;
use crate::host_intent::FileTreeClickCallback;
use crate::shell::normalized_chord;
use crate::shell::shared::scroll;
use crate::shell::shared::tree::{self};
use crate::shell::workbench::PanelContext;
use crate::theme::{color, space, typography};
use zom_workspace::EntryKind;

use super::{FileTreeRow, FileTreeState};

pub(super) fn render(ctx: PanelContext<'_>) -> Div {
    let panel = ctx.file_tree;
    let key_request = Rc::clone(panel.key_request);

    let body: AnyElement = if !ctx.has_project {
        empty_message("尚未打开项目").into_any_element()
    } else if panel.state.rows.is_empty() {
        empty_message("项目目录为空").into_any_element()
    } else {
        render_list(
            panel.state,
            panel.is_focused,
            panel.new_entry_slot,
            panel.rename_slot,
            panel.scroll,
            panel.on_item_click,
        )
        .into_any_element()
    };

    div()
        .size_full()
        .text_size(typography::ui())
        .line_height(typography::ui_line())
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
    new_entry_slot: &Rc<TextEditorSlot>,
    rename_slot: &Rc<TextEditorSlot>,
    scroll_handle: &scroll::ScrollHandle,
    on_item_click: &FileTreeClickCallback,
) -> Div {
    let items = logical_items(state);
    let selected_item = selected_item_index(&items, state);
    let selection = state.selection.clone();
    // 剪切待粘贴的行做半透明提示；仅 Cut 模式下非空（Copy 模式无视觉标记）。
    let cut_paths = state.cut_paths.clone();
    let active = state.active.clone();
    let new_entry_slot = Rc::clone(new_entry_slot);
    let rename_slot = Rc::clone(rename_slot);
    let on_item_click = Rc::clone(on_item_click);
    if let Some(index) = selected_item.filter(|index| *index < items.len()) {
        scroll_handle.reveal_item_if_changed(index);
    }

    div()
        .relative()
        .size_full()
        .p(space::s4())
        .text_color(color::current().gray.s08)
        .child(
            uniform_list("file-tree-list", items.len(), move |range, _, _| {
                range
                    .filter_map(|index| items.get(index))
                    .map(|item| match item {
                        FileTreeItem::Row(row) => {
                            render_row(row, &selection, &cut_paths, active.as_ref(), &on_item_click)
                                .into_any_element()
                        }
                        FileTreeItem::Pending(pending) => {
                            render_input_row(pending.kind, pending.depth, &new_entry_slot)
                                .into_any_element()
                        }
                        FileTreeItem::Rename(rename) => {
                            render_input_row(rename.kind, rename.depth, &rename_slot)
                                .into_any_element()
                        }
                    })
                    .collect()
            })
            .size_full()
            .track_scroll(scroll_handle.inner()),
        )
        .child(scroll::scrollbar(scroll_handle))
        .when(is_focused, |el| {
            el.children(tree::list_selection_overlay(selected_item, scroll_handle))
        })
}

#[derive(Clone)]
enum FileTreeItem {
    Row(FileTreeRow),
    Pending(PendingItem),
    Rename(PendingItem),
}

#[derive(Clone)]
struct PendingItem {
    kind: EntryKind,
    depth: usize,
}

fn logical_items(state: &FileTreeState) -> Vec<FileTreeItem> {
    let mut items = Vec::new();
    for row in &state.rows {
        // 重命名：把目标行替换成输入行，不再画原名（避免「输入框旁边还有旧名」二义）。
        if let Some(rename) = &state.pending_rename
            && rename.path == row.path
        {
            items.push(FileTreeItem::Rename(PendingItem {
                kind: rename.kind,
                depth: rename.depth,
            }));
            continue;
        }
        items.push(FileTreeItem::Row(row.clone()));
        // 新建条目的输入行紧跟在其父目录行之后，是父目录的"最后一个子项"。
        if let Some(pending) = &state.pending
            && pending.parent == row.path
        {
            items.push(FileTreeItem::Pending(PendingItem {
                kind: pending.kind,
                depth: pending.depth,
            }));
        }
    }
    items
}

fn selected_item_index(items: &[FileTreeItem], state: &FileTreeState) -> Option<usize> {
    if state.pending_rename.is_some() {
        return items
            .iter()
            .position(|item| matches!(item, FileTreeItem::Rename(_)));
    }
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

/// 内联输入行：新建 / 重命名共用。图标按目标类型挑（重命名时取原条目类型），
/// 文本与光标由 slot.embed 内部从 router 拉。
fn render_input_row(kind: EntryKind, depth: usize, slot: &Rc<TextEditorSlot>) -> Div {
    let is_dir = matches!(kind, EntryKind::Directory);
    tree::row_skeleton(depth)
        .text_color(color::current().gray.s09)
        .child(tree::guide_lines(depth))
        .child(tree::icon(is_dir, true))
        .child(
            div()
                .flex_1()
                .flex()
                .items_center()
                .overflow_hidden()
                .line_height(typography::ui_line())
                .child(slot.embed()),
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
            .text_color(color::current().gray.s09)
            .child(hint),
    )
}

fn render_row(
    row: &FileTreeRow,
    selection: &BTreeSet<std::path::PathBuf>,
    cut_paths: &BTreeSet<std::path::PathBuf>,
    active: Option<&std::path::PathBuf>,
    on_item_click: &FileTreeClickCallback,
) -> Div {
    let is_in_selection = selection.contains(&row.path);
    let is_cut = cut_paths.contains(&row.path);
    let is_active = active.map(|p| p == &row.path).unwrap_or(false);

    // 背景三态：多选选区 > 活动文件 > 透明。
    let bg_color = if is_in_selection {
        color::current().blue.a04
    } else if is_active {
        color::current().gray.s04
    } else {
        gpui::rgba(0)
    };

    let mut row_div = tree::render_row_base(
        row.depth,
        matches!(row.kind, EntryKind::Directory),
        row.expanded,
        &row.name,
    )
    .bg(bg_color);

    if let Some(kind) = row.git_color {
        row_div = row_div.text_color(color::git_status(kind));
    }
    row_div = row_div.hover(|style| style.bg(color::current().gray.s04));
    let click_path = row.path.clone();
    let on_click = Rc::clone(on_item_click);
    row_div = row_div
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            on_click(click_path.clone(), window, cx)
        });
    if is_cut {
        row_div = row_div.opacity(0.5);
    }
    row_div
}
