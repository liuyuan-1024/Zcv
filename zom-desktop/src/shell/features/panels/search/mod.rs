//! Search —— L3 panel 组件。
//!
//! 第一版 panel **只是输入控制条**：query / replacement 输入框 + 选项 toggle +
//! 上一/下一/替换按钮 + "3 / 27" 命中数标签。
//!
//! - 所有命中**直接在编辑器内高亮**（EditorView 阶段 2），panel 不显示结果列表
//! - 算法层由 `WorkspaceBuffer::BufferSearch` 提供（P3 待落地），panel 不持搜索状态
//! - 跨文件搜索 / 替换是 workspace 层另一笔账，与本面板无关
//!
//! P3 BufferSearch 落地后，本文件只动 [`render_query_row_actions`] 等少量位置去
//! 接 `hit_count`，不动 UI 结构。

mod effects;
mod model;

pub(crate) use effects::try_apply_effect;

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Context, Div, FocusHandle, IntoElement, MouseButton, Window, div, prelude::*, px};
use zom_command::commands::search;

use crate::app::App;
use crate::focus::{AppFocus, SearchField};
use crate::shell::editor::TextEditorSlot;
use crate::shell::normalized_chord;
use crate::shell::shared::glyph::Glyph;
use crate::shell::shared::theme::{color, radius, space, typography};
use crate::shell::workbench::docks::render_focus_host;
use crate::shell::{CommandTitleLookup, KeyRequest, ShortcutLookup};

pub(crate) use model::{HitCount, SearchModel, SearchState};

const CASE_SENSITIVE_ICON: &str = "icons/actions/case_sensitive.svg";
const WHOLE_WORD_ICON: &str = "icons/actions/whole_word.svg";
const REGEX_ICON: &str = "icons/actions/regex.svg";
const FIND_PREVIOUS_ICON: &str = "icons/navigation/chevron_left.svg";
const FIND_NEXT_ICON: &str = "icons/navigation/chevron_right.svg";
const REPLACE_NEXT_ICON: &str = "icons/actions/replace_next.svg";
const REPLACE_ALL_ICON: &str = "icons/actions/replace_all.svg";

#[derive(Clone)]
pub(crate) struct SearchRuntime {
    focus: FocusHandle,
    query_focus: FocusHandle,
    replacement_focus: FocusHandle,
}

impl SearchRuntime {
    pub(crate) fn new<T>(cx: &mut Context<T>) -> Self {
        Self {
            focus: cx.focus_handle(),
            query_focus: cx.focus_handle(),
            replacement_focus: cx.focus_handle(),
        }
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.query_focus.clone()
    }

    pub(crate) fn query_focus_handle(&self) -> FocusHandle {
        self.query_focus.clone()
    }

    pub(crate) fn replacement_focus_handle(&self) -> FocusHandle {
        self.replacement_focus.clone()
    }

    pub(crate) fn install_listeners<T: 'static>(
        &self,
        app: Rc<RefCell<App>>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        install_field_focus_listener(
            Rc::clone(&app),
            &self.query_focus,
            SearchField::Query,
            window,
            cx,
        );
        install_field_focus_listener(
            app,
            &self.replacement_focus,
            SearchField::Replacement,
            window,
            cx,
        );
    }

    pub(crate) fn render(
        &self,
        state: &SearchState,
        key_request: &KeyRequest,
        query_slot: &Rc<TextEditorSlot>,
        replacement_slot: &Rc<TextEditorSlot>,
        shortcuts: &ShortcutLookup,
        titles: &CommandTitleLookup,
    ) -> Div {
        render_focus_host(
            &self.focus,
            key_request,
            search_panel(
                state,
                key_request,
                &self.query_focus,
                &self.replacement_focus,
                query_slot,
                replacement_slot,
                shortcuts,
                titles,
            )
            .into_any_element(),
        )
    }
}

fn install_field_focus_listener<T: 'static>(
    app: Rc<RefCell<App>>,
    focus: &FocusHandle,
    field: SearchField,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    let app_on_focus = Rc::clone(&app);
    cx.on_focus(focus, window, move |_, _, cx| {
        app_on_focus
            .borrow_mut()
            .request_focus_from_shell(AppFocus::search(field));
        cx.notify();
    })
    .detach();
    cx.on_blur(focus, window, move |_, _, cx| {
        cx.notify();
    })
    .detach();
}

fn search_panel(
    state: &SearchState,
    key_request: &KeyRequest,
    query_focus: &FocusHandle,
    replacement_focus: &FocusHandle,
    query_slot: &Rc<TextEditorSlot>,
    replacement_slot: &Rc<TextEditorSlot>,
    shortcuts: &ShortcutLookup,
    titles: &CommandTitleLookup,
) -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(color::gray::s02())
        .text_size(typography::ui())
        .line_height(typography::ui_line())
        .text_color(color::gray::s09())
        .child(search_controls(
            state,
            key_request,
            query_focus,
            replacement_focus,
            query_slot,
            replacement_slot,
            shortcuts,
            titles,
        ))
}

fn search_controls(
    state: &SearchState,
    key_request: &KeyRequest,
    query_focus: &FocusHandle,
    replacement_focus: &FocusHandle,
    query_slot: &Rc<TextEditorSlot>,
    replacement_slot: &Rc<TextEditorSlot>,
    shortcuts: &ShortcutLookup,
    titles: &CommandTitleLookup,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(space::s8())
        .border_b_1()
        .border_color(color::gray::s05())
        .px(space::s4())
        .pt(space::s4())
        .pb(space::s8())
        .child(search_row(
            "查找",
            "搜索...",
            query_focus,
            key_request,
            query_slot,
            state.query.text().is_empty(),
            vec![
                hit_count_badge(state.hit_count),
                option_toggle(
                    "search-case-sensitive",
                    CASE_SENSITIVE_ICON,
                    search::TOGGLE_CASE_SENSITIVE,
                    state.options.case_sensitive,
                    shortcuts,
                    titles,
                ),
                option_toggle(
                    "search-whole-word",
                    WHOLE_WORD_ICON,
                    search::TOGGLE_WHOLE_WORD,
                    state.options.whole_word,
                    shortcuts,
                    titles,
                ),
                option_toggle(
                    "search-regex",
                    REGEX_ICON,
                    search::TOGGLE_REGEX,
                    state.options.regex,
                    shortcuts,
                    titles,
                ),
            ],
            vec![
                Glyph::icon(
                    "search-find-previous",
                    FIND_PREVIOUS_ICON,
                    titles(search::FIND_PREVIOUS)
                        .unwrap_or_else(|| search::FIND_PREVIOUS.to_string()),
                )
                .hint(shortcuts(search::FIND_PREVIOUS))
                .active(false)
                .render(),
                Glyph::icon(
                    "search-find-next",
                    FIND_NEXT_ICON,
                    titles(search::FIND_NEXT).unwrap_or_else(|| search::FIND_NEXT.to_string()),
                )
                .hint(shortcuts(search::FIND_NEXT))
                .active(false)
                .render(),
            ],
        ))
        .child(replace_row(
            "替换",
            "替换..",
            replacement_focus,
            key_request,
            replacement_slot,
            state.replacement.text().is_empty(),
            vec![
                Glyph::icon(
                    "search-replace-next",
                    REPLACE_NEXT_ICON,
                    titles(search::REPLACE_NEXT)
                        .unwrap_or_else(|| search::REPLACE_NEXT.to_string()),
                )
                .hint(shortcuts(search::REPLACE_NEXT))
                .active(false)
                .render(),
                Glyph::icon(
                    "search-replace-all",
                    REPLACE_ALL_ICON,
                    titles(search::REPLACE_ALL).unwrap_or_else(|| search::REPLACE_ALL.to_string()),
                )
                .hint(shortcuts(search::REPLACE_ALL))
                .active(false)
                .render(),
            ],
        ))
}

/// 输入行右侧的 "3 / 27" 命中数小标签。无命中时显示淡灰 "0 / 0"，避免布局抖动。
///
/// 第一版 `hit_count` 恒为 `None`（panel 不主动搜索）；P3 BufferSearch 落地后
/// 由 SearchModel::state() 从 active buffer 的 BufferSearch 读出真实数据。
fn hit_count_badge(hit_count: Option<HitCount>) -> gpui::AnyElement {
    let (text, color) = match hit_count {
        Some(HitCount { total: 0, .. }) | None => ("0 / 0".to_string(), color::gray::s07()),
        Some(HitCount { current, total }) => (format!("{current} / {total}"), color::gray::s09()),
    };
    div()
        .flex()
        .items_center()
        .px(space::s4())
        .text_color(color)
        .text_size(typography::ui())
        .child(text)
        .into_any_element()
}

fn option_toggle(
    id: &'static str,
    icon: &'static str,
    command_id: &'static str,
    active: bool,
    shortcuts: &ShortcutLookup,
    titles: &CommandTitleLookup,
) -> gpui::AnyElement {
    let title = titles(command_id).unwrap_or_else(|| command_id.to_string());

    div()
        .size(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::r4())
        .child(
            Glyph::icon(id, icon, title)
                .hint(shortcuts(command_id))
                .active(active)
                .icon_size(px(14.0))
                .render(),
        )
        .into_any_element()
}

fn search_row(
    label: &'static str,
    placeholder: &'static str,
    focus: &FocusHandle,
    key_request: &KeyRequest,
    slot: &Rc<TextEditorSlot>,
    show_placeholder: bool,
    input_actions: Vec<gpui::AnyElement>,
    actions: Vec<gpui::AnyElement>,
) -> Div {
    let mut action_group = div().flex().flex_row().items_center().gap(space::s4());
    for action in actions {
        action_group = action_group.child(action);
    }

    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(space::s4())
        .child(row_label(label))
        .child(search_input_with_actions(
            placeholder,
            focus,
            key_request,
            slot,
            show_placeholder,
            input_actions,
        ))
        .child(action_group)
}

fn replace_row(
    label: &'static str,
    placeholder: &'static str,
    focus: &FocusHandle,
    key_request: &KeyRequest,
    slot: &Rc<TextEditorSlot>,
    show_placeholder: bool,
    actions: Vec<gpui::AnyElement>,
) -> Div {
    let mut action_group = div().flex().flex_row().items_center().gap(space::s4());
    for action in actions {
        action_group = action_group.child(action);
    }

    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(space::s4())
        .child(row_label(label))
        .child(search_input(
            placeholder,
            focus,
            key_request,
            slot,
            show_placeholder,
        ))
        .child(action_group)
}

fn row_label(label: &'static str) -> Div {
    div()
        .flex_shrink_0()
        .text_color(color::gray::s08())
        .child(label)
}

fn search_input(
    placeholder: &'static str,
    focus: &FocusHandle,
    key_request: &KeyRequest,
    slot: &Rc<TextEditorSlot>,
    show_placeholder: bool,
) -> Div {
    search_input_base(placeholder, focus, key_request, slot, show_placeholder)
}

fn search_input_with_actions(
    placeholder: &'static str,
    focus: &FocusHandle,
    key_request: &KeyRequest,
    slot: &Rc<TextEditorSlot>,
    show_placeholder: bool,
    actions: Vec<gpui::AnyElement>,
) -> Div {
    let mut action_group = div().flex().flex_row().items_center().gap(space::s4());
    for action in actions {
        action_group = action_group.child(action);
    }

    search_input_base(placeholder, focus, key_request, slot, show_placeholder).child(action_group)
}

fn search_input_base(
    placeholder: &'static str,
    focus: &FocusHandle,
    key_request: &KeyRequest,
    slot: &Rc<TextEditorSlot>,
    show_placeholder: bool,
) -> Div {
    let focus_for_click = focus.clone();
    let key_request = Rc::clone(key_request);
    div()
        .h(px(28.0))
        .flex_1()
        .flex()
        .items_center()
        .justify_between()
        .gap(space::s4())
        .overflow_hidden()
        .rounded(radius::r4())
        .border_1()
        .border_color(color::gray::s05())
        .bg(color::gray::s01())
        .px(space::s4())
        .text_color(color::gray::s08())
        .track_focus(focus)
        .tab_index(0)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.focus(&focus_for_click);
            cx.stop_propagation();
        })
        .on_key_down(move |event, window, cx| {
            if key_request(normalized_chord(&event.keystroke), window, cx) {
                cx.stop_propagation();
            }
        })
        .child(
            div()
                .flex_1()
                .relative()
                .flex()
                .items_center()
                .overflow_hidden()
                .child(search_editor(slot, show_placeholder, placeholder)),
        )
}

fn search_editor(
    slot: &Rc<TextEditorSlot>,
    show_placeholder: bool,
    placeholder: &'static str,
) -> Div {
    let mut editor = div()
        .relative()
        .h(typography::ui_line())
        .flex_1()
        .overflow_hidden()
        .text_color(color::gray::s09());
    if show_placeholder {
        editor = editor.child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .text_color(color::gray::s08())
                .child(placeholder),
        );
    }
    editor.child(slot.embed())
}
