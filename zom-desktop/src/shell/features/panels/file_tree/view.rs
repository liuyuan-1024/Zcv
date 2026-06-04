//! 文件树的具体行渲染。
//!
//! 输入：[`FileTreeState`] 已是 flatten 后的可见行序列（已应用展开/折叠 +
//! 排序），本文件负责"画出来"+"接受键盘焦点并把按键转给 App"。
//!
//! 焦点宿主（track_focus + on_key_down 的那个外层 div）在任何状态下都得
//! 在树里——包括"尚未打开项目"占位——否则在打开项目的瞬间 `window.focus`
//! 找不到挂载点，焦点请求就会丢失。

use std::collections::BTreeSet;
use std::rc::Rc;

use gpui::{AnyElement, Div, Svg, div, prelude::*, svg, uniform_list};

use crate::shell::editor::TextEditorSlot;
use crate::shell::normalized_chord;
use crate::shell::shared::scroll;
use crate::shell::shared::theme::{color, radius, space, typography};
use crate::shell::workbench::PanelContext;
use zom_workspace::EntryKind;

use super::{FileTreeRow, FileTreeState, PendingNewEntry, PendingRename};

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
        render_list(
            panel.state,
            panel.is_focused,
            panel.new_entry_slot,
            panel.rename_slot,
            panel.scroll,
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
) -> Div {
    let items = logical_items(state);
    let selected_item = selected_item_index(&items, state);
    // pending / rename 激活时让"导航焦点"指示器（行的蓝框）让位给输入行的蓝框。
    // 同色规则同时生效会出现两个蓝框；视觉上焦点只能有一个。
    let selected = if state.pending.is_some() || state.pending_rename.is_some() {
        None
    } else {
        state.selected.clone()
    };
    // 选区与"焦点边框"是两套视觉：pending 名称输入时焦点边框让位（见上），
    // 但已经被用户累加的选区不应该静默丢失，所以这里照实传，不随 pending 收起。
    let selection = state.selection.clone();
    // 剪切待粘贴的行做半透明提示；仅 Cut 模式下非空（Copy 模式无视觉标记）。
    let cut_paths = state.cut_paths.clone();
    let active = state.active.clone();
    let new_entry_slot = Rc::clone(new_entry_slot);
    let rename_slot = Rc::clone(rename_slot);
    if let Some(index) = selected_item.filter(|index| *index < items.len()) {
        scroll_handle.reveal_item_if_changed(index);
    }

    div()
        .relative()
        .size_full()
        .overflow_hidden()
        .p(space::s4())
        .child(
            uniform_list("file-tree-list", items.len(), move |range, _, _| {
                range
                    .filter_map(|index| items.get(index))
                    .map(|item| match item {
                        FileTreeItem::Row(row) => render_row(
                            row,
                            selected.as_ref(),
                            &selection,
                            &cut_paths,
                            active.as_ref(),
                            is_focused,
                        )
                        .into_any_element(),
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
}

#[derive(Clone)]
enum FileTreeItem {
    Row(FileTreeRow),
    Pending(PendingNewEntry),
    Rename(PendingRename),
}

fn logical_items(state: &FileTreeState) -> Vec<FileTreeItem> {
    let mut items = Vec::new();
    for row in &state.rows {
        // 重命名：把目标行替换成输入行，不再画原名（避免「输入框旁边还有旧名」二义）。
        if let Some(rename) = &state.pending_rename
            && rename.path == row.path
        {
            items.push(FileTreeItem::Rename(rename.clone()));
            continue;
        }
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
    let icon = match kind {
        EntryKind::Directory => FOLDER_OPEN_ICON,
        EntryKind::File => FILE_ICON,
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .gap(space::s4())
        .overflow_hidden()
        .rounded(radius::r2())
        .border_1()
        .border_color(color::blue::s07())
        .pl(indent_unit() * (depth as f32) + space::s4())
        .text_size(typography::ui())
        .text_color(color::gray::s09())
        .child(
            div().flex_shrink_0().size(typography::ui_line()).child(
                svg()
                    .path(icon)
                    .size(typography::ui_line())
                    .text_color(color::gray::s09()),
            ),
        )
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
            .text_color(color::gray::s09())
            .child(hint),
    )
}

fn render_row(
    row: &FileTreeRow,
    selected: Option<&std::path::PathBuf>,
    selection: &BTreeSet<std::path::PathBuf>,
    cut_paths: &BTreeSet<std::path::PathBuf>,
    active: Option<&std::path::PathBuf>,
    is_focused: bool,
) -> Div {
    let is_selected = selected.map(|p| p == &row.path).unwrap_or(false);
    let is_in_selection = selection.contains(&row.path);
    let is_cut = cut_paths.contains(&row.path);
    let is_active = active.map(|p| p == &row.path).unwrap_or(false);

    // 边框始终占 1px，保证选中态切换时行高不抖。
    // 失焦时直接染透明，让选中标记瞬时消失；获焦后再画上。
    let border_color = if is_selected && is_focused {
        color::blue::s07()
    } else {
        gpui::rgba(0)
    };
    // 背景三态：多选选区 > 活动文件 > 透明。
    // 选区色用蓝 a04（theme 注释里就是"选区色块"），与活动文件的灰底视觉上一眼可分。
    let bg_color = if is_in_selection {
        color::blue::a04()
    } else if is_active {
        color::gray::s04()
    } else {
        gpui::rgba(0)
    };
    // 与 top bar Glyph 同基线：常态 g75，活动项 g95。
    let text_color = if is_active {
        color::gray::s09()
    } else {
        color::gray::s09()
    };

    let mut row_div = div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .gap(space::s4())
        .overflow_hidden()
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
        );
    // 剪切待粘贴的行：整行降透明度，向用户提示"它将被移走"。
    // 粘贴成功后 model 清空剪贴板、cut_paths 也清空，该效果随之消失。
    if is_cut {
        row_div = row_div.opacity(0.5);
    }
    row_div
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
        color::gray::s09()
    } else {
        color::gray::s09()
    };
    svg()
        .path(path)
        .size(typography::ui_line())
        .text_color(tint)
}
