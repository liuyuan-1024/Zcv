//! KeyboardShortcuts —— L3 panel 组件。

use std::rc::Rc;

use gpui::{
    Context, Div, FocusHandle, IntoElement, ListAlignment, ListState, div, list, prelude::*, px,
};

use crate::shell::shared::scroll;
use crate::shell::shared::theme::{color, space, typography};
use crate::shell::workbench::docks::render_focus_host;
use crate::shell::{CommandCatalogItem, CommandCatalogLookup, KeyRequest, ShortcutLookup};

#[derive(Clone)]
pub(crate) struct KeyboardShortcutsRuntime {
    focus: FocusHandle,
    list_state: ListState,
}

impl KeyboardShortcutsRuntime {
    pub(crate) fn new<T>(cx: &mut Context<T>) -> Self {
        Self {
            focus: cx.focus_handle(),
            list_state: ListState::new(0, ListAlignment::Top, px(64.0)).measure_all(),
        }
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub(crate) fn render(
        &self,
        key_request: &KeyRequest,
        shortcuts: &ShortcutLookup,
        command_catalog: &CommandCatalogLookup,
    ) -> Div {
        let rows = shortcut_rows(shortcuts, command_catalog);
        render_focus_host(
            &self.focus,
            key_request,
            shortcuts_panel(rows, &self.list_state).into_any_element(),
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ShortcutRow {
    title: String,
    shortcut: String,
    description: String,
}

fn shortcut_rows(
    shortcuts: &ShortcutLookup,
    command_catalog: &CommandCatalogLookup,
) -> Vec<ShortcutRow> {
    command_catalog()
        .into_iter()
        .filter(|command| command.visible_in_shortcuts)
        .filter_map(|command| row_from_command(command, shortcuts))
        .collect()
}

fn row_from_command(
    command: CommandCatalogItem,
    shortcuts: &ShortcutLookup,
) -> Option<ShortcutRow> {
    Some(ShortcutRow {
        shortcut: shortcuts(&command.command_id)?,
        description: command.description?,
        title: command.title,
    })
}

fn shortcuts_panel(rows: Vec<ShortcutRow>, list_state: &ListState) -> Div {
    let body = if rows.is_empty() {
        empty_message("暂无快捷键").into_any_element()
    } else {
        shortcuts_list(rows, list_state).into_any_element()
    };

    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(color::gray::s02())
        .text_size(typography::ui())
        .line_height(typography::ui_line())
        .text_color(color::gray::s09())
        .child(div().flex_1().overflow_hidden().child(body))
}

fn shortcuts_list(rows: Vec<ShortcutRow>, list_state: &ListState) -> impl IntoElement {
    let rows = Rc::new(rows);
    let count = rows.len();
    if list_state.item_count() != count {
        list_state.reset(count);
    }

    div()
        .relative()
        .size_full()
        .overflow_hidden()
        .child(
            list(list_state.clone(), move |index, _, _| {
                rows.get(index)
                    .cloned()
                    .map(render_row)
                    .unwrap_or_else(|| div())
                    .into_any_element()
            })
            .w_full()
            .h_full(),
        )
        .child(scroll::list_scrollbar(list_state))
}

fn render_row(row: ShortcutRow) -> Div {
    div()
        .flex()
        .flex_col()
        .w_full()
        .gap(space::s6())
        .border_b_1()
        .border_color(color::gray::s05())
        .p(space::s6())
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .w_full()
                .gap(space::s6())
                .child(
                    div()
                        .flex_1()
                        .overflow_hidden()
                        .truncate()
                        .text_color(color::gray::a09())
                        .child(row.title),
                )
                .child(shortcut_badge(row.shortcut)),
        )
        .child(
            div()
                .w_full()
                .overflow_hidden()
                .text_color(color::gray::s08())
                .line_height(typography::ui_line())
                .child(row.description),
        )
}

fn shortcut_badge(shortcut: String) -> Div {
    div()
        .flex_shrink_0()
        .whitespace_nowrap()
        .text_color(color::gray::a09())
        .child(shortcut)
}

fn empty_message(hint: &'static str) -> Div {
    div().flex().flex_col().size_full().child(
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_size(typography::ui())
            .text_color(color::gray::s08())
            .child(hint),
    )
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;

    #[test]
    fn shortcut_rows_should_filter_and_join_command_metadata() {
        let shortcuts: ShortcutLookup = Rc::new(|command_id| match command_id {
            "editor.save" => Some("⌘ S".to_string()),
            "editor.undo" => Some("⌘ Z".to_string()),
            _ => None,
        });
        let command_catalog: CommandCatalogLookup = Rc::new(|| {
            vec![
                CommandCatalogItem {
                    command_id: "editor.save".to_string(),
                    title: "保存".to_string(),
                    description: Some("保存当前打开的文件。".to_string()),
                    visible_in_shortcuts: true,
                },
                CommandCatalogItem {
                    command_id: "editor.undo".to_string(),
                    title: "撤销".to_string(),
                    description: None,
                    visible_in_shortcuts: true,
                },
                CommandCatalogItem {
                    command_id: "editor.redo".to_string(),
                    title: "重做".to_string(),
                    description: Some("重做上一次被撤销的编辑。".to_string()),
                    visible_in_shortcuts: false,
                },
                CommandCatalogItem {
                    command_id: "editor.close_tab".to_string(),
                    title: "关闭标签".to_string(),
                    description: Some("关闭当前编辑器标签。".to_string()),
                    visible_in_shortcuts: true,
                },
            ]
        });

        let rows = shortcut_rows(&shortcuts, &command_catalog);

        assert_eq!(
            rows,
            vec![ShortcutRow {
                title: "保存".to_string(),
                shortcut: "⌘ S".to_string(),
                description: "保存当前打开的文件。".to_string(),
            }]
        );
    }
}
