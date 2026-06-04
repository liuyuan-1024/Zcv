//! Search —— L3 panel 组件。
//!
//! 当前 panel **只是输入控制条**：query / replacement 输入框 + 选项 toggle +
//! 上一/下一/替换按钮 + "3 / 27" 命中数标签。
//!
//! - 所有命中**直接在编辑器内高亮**（EditorView 阶段 2），panel 不显示结果列表
//! - 算法层由 `WorkspaceBuffer::BufferSearch` 提供，panel 不持搜索状态
//! - 跨文件搜索 / 替换是 workspace 层另一笔账，与本面板无关

pub(crate) mod coordinator;
mod effects;
mod model;

pub(crate) use effects::try_apply_effect;

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Context, Div, FocusHandle, IntoElement, MouseButton, Window, div, prelude::*};
use zom_command::{EditTarget, SearchOption, commands::search};
use zom_view::ViewSet;
use zom_workspace::Workspace;

use crate::app::App;
use crate::focus::{AppFocus, SearchField};
use crate::shell::editor::{TextEditorSlot, TextTargetOwner, TextTargetQuery};
use crate::shell::normalized_chord;
use crate::shell::shared::glyph::Glyph;
use crate::shell::shared::theme::{color, radius, space, typography};
use crate::shell::workbench::docks::render_focus_host;
use crate::shell::{CommandTitleLookup, KeyRequest, ShortcutLookup};

use model::SearchModel;
pub(crate) use model::{HitCount, SearchState};

const CASE_SENSITIVE_ICON: &str = "icons/actions/case_sensitive.svg";
const WHOLE_WORD_ICON: &str = "icons/actions/whole_word.svg";
const REGEX_ICON: &str = "icons/actions/regex.svg";
const FIND_PREVIOUS_ICON: &str = "icons/navigation/chevron_left.svg";
const FIND_NEXT_ICON: &str = "icons/navigation/chevron_right.svg";
const REPLACE_NEXT_ICON: &str = "icons/actions/replace_next.svg";
const REPLACE_ALL_ICON: &str = "icons/actions/replace_all.svg";

/// 搜索面板暴露给宿主的窄接口。
///
/// `SearchModel` 的双输入框 owner 拆分、panel 状态同步、命中导航都留在 search
/// feature 内；App 只在输入派发和 workspace/view 生命周期点调用这里的能力方法。
#[derive(Clone)]
pub(crate) struct SearchRuntimeHandle {
    model: Rc<RefCell<SearchModel>>,
}

impl SearchRuntimeHandle {
    fn new(model: Rc<RefCell<SearchModel>>) -> Self {
        Self { model }
    }

    pub(crate) fn accepts_focus(&self, focus: AppFocus) -> bool {
        matches!(focus, AppFocus::Panel(crate::focus::PanelFocus::Search(_)))
    }

    pub(crate) fn with_query_owners<R>(
        &self,
        f: impl FnOnce(&dyn TextTargetQuery, &dyn TextTargetQuery) -> R,
    ) -> R {
        let model = self.model.borrow();
        let query = model.query_owner();
        let replacement = model.replacement_owner();
        f(&query, &replacement)
    }

    pub(crate) fn with_active_owner<R>(
        &self,
        focus: AppFocus,
        f: impl FnOnce(&mut dyn TextTargetOwner) -> R,
    ) -> R {
        let mut model = self.model.borrow_mut();
        let mut owner = model.active_owner(focus);
        f(&mut owner)
    }

    pub(crate) fn with_edit_target_for_focus<R>(
        &self,
        focus: AppFocus,
        f: impl FnOnce(Option<EditTarget<'_>>) -> R,
    ) -> R {
        let mut model = self.model.borrow_mut();
        f(model.edit_target_for_focus(focus))
    }

    pub(crate) fn state(&self, workspace: &Workspace) -> SearchState {
        let mut state = self.model.borrow().state();
        state.hit_count = coordinator::current_hit_count(workspace);
        state
    }

    pub(crate) fn sync_active_buffer_search(&self, workspace: &mut Workspace, views: &mut ViewSet) {
        let mut model = self.model.borrow_mut();
        coordinator::sync_active_buffer_search(&mut model, workspace, views);
    }

    pub(crate) fn pump_active_buffer_search(workspace: &mut Workspace, views: &mut ViewSet) {
        coordinator::pump_active_buffer_search(workspace, views);
    }

    pub(crate) fn on_panel_opened(&self, workspace: &mut Workspace, views: &mut ViewSet) {
        let mut model = self.model.borrow_mut();
        coordinator::on_panel_opened(&mut model, workspace, views);
    }

    pub(crate) fn on_panel_closed(&self, workspace: &mut Workspace) {
        let mut model = self.model.borrow_mut();
        coordinator::on_panel_closed(&mut model, workspace);
    }

    pub(crate) fn toggle_option(
        &self,
        workspace: &mut Workspace,
        views: &mut ViewSet,
        option: SearchOption,
    ) {
        let mut model = self.model.borrow_mut();
        coordinator::toggle_option(&mut model, workspace, views, option);
    }

    pub(crate) fn find_next(&self, workspace: &mut Workspace, views: &mut ViewSet) {
        let mut model = self.model.borrow_mut();
        coordinator::find_next(&mut model, workspace, views);
    }

    pub(crate) fn find_previous(&self, workspace: &mut Workspace, views: &mut ViewSet) {
        let mut model = self.model.borrow_mut();
        coordinator::find_previous(&mut model, workspace, views);
    }

    pub(crate) fn replace_next(&self, workspace: &mut Workspace, views: &mut ViewSet) {
        let mut model = self.model.borrow_mut();
        coordinator::replace_next(&mut model, workspace, views);
    }

    pub(crate) fn replace_all(&self, workspace: &mut Workspace, views: &mut ViewSet) {
        let mut model = self.model.borrow_mut();
        coordinator::replace_all(&mut model, workspace, views);
    }
}

/// 搜索面板的 shell 端 runtime —— 焦点宿主 + `SearchModel` 的真正拥有者。
///
/// App 只保存 [`SearchRuntimeHandle`]，不直接认识 `SearchModel` 或 coordinator
/// 函数。这样搜索输入框的 owner 拆分仍能参与全局 editor router，同时业务动作
/// 留在 search feature 内部。
#[derive(Clone)]
pub(crate) struct SearchRuntime {
    focus: FocusHandle,
    query_focus: FocusHandle,
    replacement_focus: FocusHandle,
    model: Rc<RefCell<SearchModel>>,
}

impl SearchRuntime {
    pub(crate) fn new<T>(cx: &mut Context<T>) -> Self {
        Self {
            focus: cx.focus_handle(),
            query_focus: cx.focus_handle(),
            replacement_focus: cx.focus_handle(),
            model: Rc::new(RefCell::new(SearchModel::new())),
        }
    }

    pub(crate) fn runtime_handle(&self) -> SearchRuntimeHandle {
        SearchRuntimeHandle::new(self.model.clone())
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
        .gap(space::s6())
        .border_b_1()
        .border_color(color::gray::s05())
        .p(space::s6())
        .child(search_row(
            "查找...",
            query_focus,
            key_request,
            query_slot,
            state
                .query
                .lines
                .first()
                .map(|line| line.text.is_empty())
                .unwrap_or(true),
            vec![
                hit_count_badge(state.hit_count),
                Glyph::icon(
                    "search-case-sensitive",
                    CASE_SENSITIVE_ICON,
                    titles(search::TOGGLE_CASE_SENSITIVE)
                        .unwrap_or_else(|| search::TOGGLE_CASE_SENSITIVE.to_string()),
                )
                .hint(shortcuts(search::TOGGLE_CASE_SENSITIVE))
                .active(state.options.case_sensitive)
                .render(),
                Glyph::icon(
                    "search-whole-word",
                    WHOLE_WORD_ICON,
                    titles(search::TOGGLE_WHOLE_WORD)
                        .unwrap_or_else(|| search::TOGGLE_WHOLE_WORD.to_string()),
                )
                .hint(shortcuts(search::TOGGLE_WHOLE_WORD))
                .active(state.options.whole_word)
                .render(),
                Glyph::icon(
                    "search-regex",
                    REGEX_ICON,
                    titles(search::TOGGLE_REGEX)
                        .unwrap_or_else(|| search::TOGGLE_REGEX.to_string()),
                )
                .hint(shortcuts(search::TOGGLE_REGEX))
                .active(state.options.regex)
                .render(),
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
            "替换...",
            replacement_focus,
            key_request,
            replacement_slot,
            state
                .replacement
                .lines
                .first()
                .map(|line| line.text.is_empty())
                .unwrap_or(true),
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
/// `hit_count` 来自活动 buffer 的 `BufferSearch`；panel 自身不主动搜索。
/// 由 SearchModel::state() 从 active buffer 的 BufferSearch 读出真实数据。
fn hit_count_badge(hit_count: Option<HitCount>) -> gpui::AnyElement {
    let (text, color) = match hit_count {
        Some(HitCount { total: 0, .. }) | None => ("0/0".to_string(), color::gray::s07()),
        Some(HitCount { current, total }) => (format!("{current}/{total}"), color::gray::s09()),
    };
    div()
        .flex()
        .items_center()
        .text_color(color)
        .text_size(typography::ui())
        .child(text)
        .into_any_element()
}

fn search_row(
    placeholder: &'static str,
    focus: &FocusHandle,
    key_request: &KeyRequest,
    slot: &Rc<TextEditorSlot>,
    show_placeholder: bool,
    input_actions: Vec<gpui::AnyElement>,
    actions: Vec<gpui::AnyElement>,
) -> Div {
    let mut action_group = div().flex().flex_row().items_center().gap(space::s6());
    for action in actions {
        action_group = action_group.child(action);
    }

    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(space::s6())
        .child(input_box_with_actions(
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
    placeholder: &'static str,
    focus: &FocusHandle,
    key_request: &KeyRequest,
    slot: &Rc<TextEditorSlot>,
    show_placeholder: bool,
    actions: Vec<gpui::AnyElement>,
) -> Div {
    let mut action_group = div().flex().flex_row().items_center().gap(space::s6());
    for action in actions {
        action_group = action_group.child(action);
    }

    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(space::s6())
        .child(base_input_box(
            placeholder,
            focus,
            key_request,
            slot,
            show_placeholder,
        ))
        .child(action_group)
}

fn input_box_with_actions(
    placeholder: &'static str,
    focus: &FocusHandle,
    key_request: &KeyRequest,
    slot: &Rc<TextEditorSlot>,
    show_placeholder: bool,
    actions: Vec<gpui::AnyElement>,
) -> Div {
    let mut action_group = div().flex().flex_row().items_center().gap(space::s6());
    for action in actions {
        action_group = action_group.child(action);
    }

    base_input_box(placeholder, focus, key_request, slot, show_placeholder).child(action_group)
}

fn base_input_box(
    placeholder: &'static str,
    focus: &FocusHandle,
    key_request: &KeyRequest,
    slot: &Rc<TextEditorSlot>,
    show_placeholder: bool,
) -> Div {
    let focus_for_click = focus.clone();
    let key_request = Rc::clone(key_request);
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_between()
        .overflow_hidden()
        .p(space::s6())
        .border_1()
        .rounded(radius::r4())
        .border_color(color::gray::s05())
        .bg(color::gray::s01())
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
                .child(editor(slot, show_placeholder, placeholder)),
        )
}

fn editor(slot: &Rc<TextEditorSlot>, show_placeholder: bool, placeholder: &'static str) -> Div {
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
