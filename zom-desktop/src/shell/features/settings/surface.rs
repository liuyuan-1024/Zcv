//! 设置 surface。

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    Context, Div, Entity, FocusHandle, InteractiveElement, ListAlignment, ListState, MouseButton,
    Window, div, list, prelude::*, px,
};

use crate::config::{AppConfig, SettingsChange};
use crate::shell::KeyRequest;
use crate::shell::normalized_chord;
use crate::shell::shared::scroll;
use crate::shell::surfaces::{SurfaceAnchor, SurfaceManager, SurfaceRequest, WindowPosition};
use crate::theme::{color, radius, space, typography};
use crate::ui_id::SurfaceId;

#[derive(Clone, Debug)]
pub(crate) struct SettingsPanelState {
    config: AppConfig,
    path: Option<PathBuf>,
}

impl SettingsPanelState {
    pub(crate) fn new(config: AppConfig, path: Option<PathBuf>) -> Self {
        Self { config, path }
    }

    fn path_label(&self) -> String {
        self.path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| "memory".to_string())
    }
}

impl Default for SettingsPanelState {
    fn default() -> Self {
        Self {
            config: AppConfig::default(),
            path: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SettingsAction {
    OpenToml,
    Change(SettingsChange),
}

pub(crate) type SettingsActionRequest = Rc<dyn Fn(SettingsAction, &mut Window, &mut gpui::App)>;

#[derive(Clone)]
pub(crate) struct SettingsRuntime {
    focus: FocusHandle,
    list_state: ListState,
    key_request: Rc<RefCell<Option<KeyRequest>>>,
    action_request: Rc<RefCell<Option<SettingsActionRequest>>>,
    state: Rc<RefCell<SettingsPanelState>>,
}

impl SettingsRuntime {
    pub(crate) fn new<T>(cx: &mut Context<T>) -> Self {
        Self {
            focus: cx.focus_handle(),
            list_state: ListState::new(SETTINGS_SECTION_COUNT, ListAlignment::Top, px(48.0))
                .measure_all(),
            key_request: Rc::new(RefCell::new(None)),
            action_request: Rc::new(RefCell::new(None)),
            state: Rc::new(RefCell::new(SettingsPanelState::default())),
        }
    }

    pub(crate) fn set_key_request(&self, key_request: KeyRequest) {
        *self.key_request.borrow_mut() = Some(key_request);
    }

    pub(crate) fn set_action_request(&self, action_request: SettingsActionRequest) {
        *self.action_request.borrow_mut() = Some(action_request);
    }

    pub(crate) fn set_state(&self, state: SettingsPanelState) {
        *self.state.borrow_mut() = state;
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub(crate) fn install_listeners<T: 'static>(
        &self,
        surfaces: Entity<SurfaceManager>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let focus = self.focus.clone();
        cx.on_blur(&focus, window, move |_, _, cx| {
            surfaces.update(cx, |surfaces, cx| {
                if surfaces.is_active(SurfaceId::Settings) {
                    surfaces.dismiss(cx);
                }
            });
            cx.notify();
        })
        .detach();
    }
}

pub(crate) fn request(runtime: SettingsRuntime) -> SurfaceRequest {
    let focus = runtime.focus.clone();
    SurfaceRequest {
        id: SurfaceId::Settings,
        anchor: SurfaceAnchor::Window {
            position: WindowPosition::Center,
        },
        focus_on_open: Some(focus),
        render: Rc::new(move || {
            render(
                &runtime.focus,
                runtime.list_state.clone(),
                Rc::clone(&runtime.key_request),
                Rc::clone(&runtime.action_request),
                runtime.state.borrow().clone(),
            )
            .into_any_element()
        }),
    }
}

fn render(
    focus: &FocusHandle,
    list_state: ListState,
    key_request: Rc<RefCell<Option<KeyRequest>>>,
    action_request: Rc<RefCell<Option<SettingsActionRequest>>>,
    state: SettingsPanelState,
) -> Div {
    div()
        .w(px(558.0))
        .h(px(500.0))
        .flex()
        .flex_col()
        .rounded(radius::r4())
        .border_1()
        .border_color(color::current().gray.s05)
        .bg(color::current().gray.s03)
        .overflow_hidden()
        .track_focus(focus)
        .tab_index(0)
        .on_key_down(move |event, window, cx| {
            let Some(key_request) = key_request.borrow().clone() else {
                return;
            };
            if key_request(normalized_chord(&event.keystroke), window, cx) {
                cx.stop_propagation();
            }
        })
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .child(header(&state, Rc::clone(&action_request)))
        .child(body(list_state, action_request, state))
}

fn header(
    state: &SettingsPanelState,
    action_request: Rc<RefCell<Option<SettingsActionRequest>>>,
) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(space::s12())
        .px(space::s12())
        .py(space::s8())
        .border_b_1()
        .border_color(color::current().gray.s05)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::s4())
                .child(muted(state.path_label())),
        )
        .child(clickable(
            pill("打开 TOML".to_string()),
            action_request,
            SettingsAction::OpenToml,
        ))
}

const SETTINGS_SECTION_COUNT: usize = 3;

fn body(
    list_state: ListState,
    action_request: Rc<RefCell<Option<SettingsActionRequest>>>,
    state: SettingsPanelState,
) -> impl IntoElement {
    if list_state.item_count() != SETTINGS_SECTION_COUNT {
        list_state.reset(SETTINGS_SECTION_COUNT);
    }

    div()
        .relative()
        .flex_1()
        .overflow_hidden()
        .p(space::s4())
        .child(
            list(list_state.clone(), move |index, _, _| {
                settings_section_item(index, Rc::clone(&action_request), &state).into_any_element()
            })
            .w_full()
            .h_full(),
        )
        .child(scroll::list_scrollbar(&list_state))
}

fn settings_section_item(
    index: usize,
    action_request: Rc<RefCell<Option<SettingsActionRequest>>>,
    state: &SettingsPanelState,
) -> Div {
    let content = match index {
        0 => section(
            "全局".to_string(),
            vec![select_row(
                "主题".to_string(),
                theme_label(&state.config.general.theme),
                "general.theme",
                Rc::clone(&action_request),
                SettingsAction::Change(SettingsChange::CycleTheme),
            )],
        ),
        1 => section(
            "界面".to_string(),
            vec![stepper_row(
                "字号".to_string(),
                format!("{} px", state.config.ui.font_size),
                "ui.font_size",
                Rc::clone(&action_request),
                SettingsChange::AdjustUiFont(-1),
                SettingsChange::AdjustUiFont(1),
            )],
        ),
        2 => section(
            "编辑器".to_string(),
            vec![
                stepper_row(
                    "编辑字号".to_string(),
                    format!("{} px", state.config.editor.font_size),
                    "editor.font_size",
                    Rc::clone(&action_request),
                    SettingsChange::AdjustEditorFont(-1),
                    SettingsChange::AdjustEditorFont(1),
                ),
                toggle_row(
                    "软换行".to_string(),
                    state.config.editor.soft_wrap,
                    "editor.soft_wrap",
                    Rc::clone(&action_request),
                    SettingsAction::Change(SettingsChange::ToggleEditorSoftWrap),
                ),
                select_row(
                    "Tab 宽度".to_string(),
                    state.config.editor.tab_size.to_string(),
                    "editor.tab_size",
                    Rc::clone(&action_request),
                    SettingsAction::Change(SettingsChange::CycleEditorTabSize),
                ),
            ],
        ),
        _ => div(),
    };

    div().px(space::s12()).pb(space::s12()).child(content)
}

fn section(label: String, rows: Vec<Div>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(space::s6())
        .child(section_label(label))
        .child(
            div()
                .flex()
                .flex_col()
                .rounded(radius::r4())
                .border_1()
                .border_color(color::current().gray.s05)
                .overflow_hidden()
                .children(rows),
        )
}

fn select_row(
    label: String,
    value: String,
    key: &'static str,
    action_request: Rc<RefCell<Option<SettingsActionRequest>>>,
    action: SettingsAction,
) -> Div {
    setting_row(label, key, clickable(pill(value), action_request, action))
}

fn stepper_row(
    label: String,
    value: String,
    key: &'static str,
    action_request: Rc<RefCell<Option<SettingsActionRequest>>>,
    decrement: SettingsChange,
    increment: SettingsChange,
) -> Div {
    setting_row(
        label,
        key,
        stepper(value, action_request, decrement, increment),
    )
}

fn toggle_row(
    label: String,
    enabled: bool,
    key: &'static str,
    action_request: Rc<RefCell<Option<SettingsActionRequest>>>,
    action: SettingsAction,
) -> Div {
    setting_row(
        label,
        key,
        clickable(toggle(enabled), action_request, action),
    )
}

fn setting_row(label: String, key: &'static str, control: Div) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(space::s12())
        .px(space::s12())
        .py(space::s8())
        .border_b_1()
        .border_color(color::current().gray.s05)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::s4())
                .child(label_text(label))
                .child(muted(key)),
        )
        .child(control)
}

fn section_label(text: String) -> Div {
    div()
        .text_size(typography::ui())
        .line_height(typography::ui_line())
        .text_color(color::current().gray.s08)
        .child(text)
}

fn label_text(text: String) -> Div {
    div()
        .text_size(typography::ui())
        .line_height(typography::ui_line())
        .text_color(color::current().gray.s09)
        .child(text)
}

fn muted(text: impl Into<String>) -> Div {
    div()
        .text_size(typography::ui())
        .line_height(typography::ui_line())
        .text_color(color::current().gray.s08)
        .child(text.into())
}

fn clickable(
    control: Div,
    action_request: Rc<RefCell<Option<SettingsActionRequest>>>,
    action: SettingsAction,
) -> Div {
    control
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            let Some(action_request) = action_request.borrow().clone() else {
                return;
            };
            action_request(action, window, cx);
            cx.stop_propagation();
        })
}

fn pill(text: String) -> Div {
    div()
        .rounded(radius::r4())
        .border_1()
        .border_color(color::current().gray.s05)
        .px(space::s8())
        .py(space::s6())
        .text_size(typography::ui())
        .line_height(typography::ui_line())
        .text_color(color::current().gray.s09)
        .child(text)
}

fn stepper(
    value: String,
    action_request: Rc<RefCell<Option<SettingsActionRequest>>>,
    decrement: SettingsChange,
    increment: SettingsChange,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(space::s4())
        .child(stepper_button(
            "-",
            Rc::clone(&action_request),
            SettingsAction::Change(decrement),
        ))
        .child(value_box(value))
        .child(stepper_button(
            "+",
            action_request,
            SettingsAction::Change(increment),
        ))
}

fn stepper_button(
    text: &'static str,
    action_request: Rc<RefCell<Option<SettingsActionRequest>>>,
    action: SettingsAction,
) -> Div {
    div()
        .w(px(24.0))
        .h(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::r4())
        .border_1()
        .border_color(color::current().gray.s05)
        .cursor_pointer()
        .text_size(typography::ui())
        .line_height(typography::ui_line())
        .text_color(color::current().gray.s09)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            let Some(action_request) = action_request.borrow().clone() else {
                return;
            };
            action_request(action, window, cx);
            cx.stop_propagation();
        })
        .child(text)
}

fn value_box(text: String) -> Div {
    div()
        .min_w(px(64.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::r4())
        .border_1()
        .border_color(color::current().gray.s05)
        .px(space::s8())
        .py(space::s6())
        .text_size(typography::ui())
        .line_height(typography::ui_line())
        .text_color(color::current().gray.s09)
        .child(text)
}

fn toggle(enabled: bool) -> Div {
    let knob_x = if enabled { space::s16() } else { space::s4() };
    div()
        .relative()
        .w(px(36.0))
        .h(px(20.0))
        .rounded(radius::full())
        .bg(if enabled {
            color::current().blue.s05
        } else {
            color::current().gray.s04
        })
        .child(
            div()
                .absolute()
                .left(knob_x)
                .top(px(4.0))
                .w(px(12.0))
                .h(px(12.0))
                .rounded(radius::full())
                .bg(color::current().gray.a09),
        )
}

fn theme_label(theme: &str) -> String {
    match theme {
        "one-dark" => "One Dark",
        "one-light" => "One Light",
        _ => "One Dark",
    }
    .to_string()
}
