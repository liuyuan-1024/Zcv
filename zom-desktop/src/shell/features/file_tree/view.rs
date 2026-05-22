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

use crate::shell::editor::{EditorElement, EditorKind};
use crate::shell::shared::theme::{color, radius, space, typography};
use crate::shell::workbench::PanelContext;
use crate::shell::{InputHandlerHook, normalized_chord};
use zom_workspace::EntryKind;

use super::{FileTreeRow, FileTreeState, PendingNewEntry};

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
        render_list(
            panel.state,
            panel.is_focused,
            panel.input_handler_hook,
            panel.caret_visible,
        )
        .into_any_element()
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
    input_handler_hook: &InputHandlerHook,
    caret_visible: bool,
) -> impl IntoElement + use<> {
    let mut list = div()
        .id("file-tree-list")
        .flex()
        .flex_col()
        .size_full()
        .overflow_y_scroll();
    for row in &state.rows {
        list = list.child(render_row(row, state, is_focused));
        // 新建条目的输入行紧跟在其父目录行之后。
        if let Some(pending) = &state.pending {
            if pending.parent == row.path {
                list =
                    list.child(render_input_row(pending, input_handler_hook, caret_visible));
            }
        }
    }
    list
}

/// 新建态的内联输入行：父目录行下方，带文件/目录图标、已键入名称与光标。
///
/// 名称输入框直接嵌入单行 [`EditorElement`] —— 与主编辑区是同一个编辑器，
/// 只是 `EditorKind::SingleLine`。本行（边框 / 图标 / 缩进）是它的外壳。
fn render_input_row(
    pending: &PendingNewEntry,
    input_handler_hook: &InputHandlerHook,
    caret_visible: bool,
) -> Div {
    let icon = match pending.kind {
        EntryKind::Directory => FOLDER_OPEN_ICON,
        EntryKind::File => FILE_ICON,
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
                .child(
                    EditorElement::new(
                        EditorKind::SingleLine,
                        pending.editor.text.clone(),
                        pending.editor.cursor_byte,
                        input_handler_hook.clone(),
                    )
                    .caret_visible(caret_visible)
                    .element_id("zom-editor-file-tree-pending"),
                ),
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
