//! 设置 surface。

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{
    Context, Corner, Div, Entity, FocusHandle, InteractiveElement, ListAlignment, ListState,
    MouseButton, Window, div, list, point, prelude::*, px,
};

use crate::config::{AppConfig, SettingsChange};
use crate::shell::KeyRequest;
use zom_workspace::syntax::SyntaxEngine;

use crate::shell::editor::{TextEditorSlot, TextTargetOwner};
use crate::shell::features::settings::toml_editor::SettingsTomlEditor;
use crate::shell::normalized_chord;
use crate::shell::shared::scroll;
use crate::shell::shared::theme::{color, radius, space, typography};
use crate::shell::surfaces::{
    SurfaceAnchor, SurfaceId, SurfaceInvokerPoint, SurfaceManager, SurfacePlacement, SurfaceRequest,
};

#[derive(Clone, Debug)]
pub(crate) struct SettingsPanelState {
    config: AppConfig,
    path: Option<PathBuf>,
    toml_open: bool,
}

impl SettingsPanelState {
    pub(crate) fn new(config: AppConfig, path: Option<PathBuf>, toml_open: bool) -> Self {
        Self {
            config,
            path,
            toml_open,
        }
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
            toml_open: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SettingsAction {
    OpenToml,
    ReturnSettings,
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
    toml_slot: Rc<RefCell<Option<Rc<TextEditorSlot>>>>,
    /// config.toml 视图的可嵌入编辑器。runtime 是唯一拥有者，构造时从 App 借 `SyntaxEngine` handle 自建；
    /// App 不再持任何 settings 字段。
    toml_editor: Rc<RefCell<SettingsTomlEditor>>,
}

impl SettingsRuntime {
    /// 用 App 借出的 `SyntaxEngine` handle 构造 runtime —— 内部直接 new 一个 `SettingsTomlEditor` 并装进 `Rc<RefCell<_>>`，自身做该 owner 的拥有者。
    ///
    /// shell 拿构造好的 runtime 后调 [`toml_owner_handle`](Self::toml_owner_handle) 把 owner 注册进 App 的 router；
    /// App 在命令派发期通过 registry 借出 owner 走 edit_target / after_text_changed，不再有任何"App 字段"形式的耦合。
    pub(crate) fn new<T>(engine: Rc<SyntaxEngine>, cx: &mut Context<T>) -> Self {
        Self {
            focus: cx.focus_handle(),
            list_state: ListState::new(SETTINGS_SECTION_COUNT, ListAlignment::Top, px(48.0))
                .measure_all(),
            key_request: Rc::new(RefCell::new(None)),
            action_request: Rc::new(RefCell::new(None)),
            state: Rc::new(RefCell::new(SettingsPanelState::default())),
            toml_slot: Rc::new(RefCell::new(None)),
            toml_editor: Rc::new(RefCell::new(SettingsTomlEditor::new(engine))),
        }
    }

    /// 把 TOML 编辑器当作 [`TextTargetOwner`] 暴露给 App 注册进 router。
    /// 与 runtime 内部共享同一份 `Rc<RefCell<_>>`，两端写入立刻互见。
    pub(crate) fn toml_owner_handle(&self) -> Rc<RefCell<dyn TextTargetOwner>> {
        self.toml_editor.clone()
    }

    /// 从磁盘读 config.toml，读失败兜底用调用方给的 in-memory `AppConfig`
    /// 序列化文本。把内部状态翻成 "已打开" + caret 在末尾的多行编辑态。
    pub(crate) fn open_toml_from_disk(&self, path: &Path, fallback: &AppConfig) {
        self.toml_editor.borrow_mut().open_from_disk(path, fallback);
    }

    /// 关闭 toml 视图并解析当前文本为 `AppConfig`。解析失败时保持打开态、
    /// 返回 `None`；caller 据此决定是否替换全局 config。
    pub(crate) fn close_toml_and_parse(&self) -> Option<AppConfig> {
        self.toml_editor.borrow_mut().close_and_parse()
    }

    pub(crate) fn is_toml_open(&self) -> bool {
        self.toml_editor.borrow().is_open()
    }

    /// 每帧 prepaint 由 [`ShellView::render`] 调一次，把后台 SyntaxWorker 已就绪的高亮产物落到 toml 文档的 MetadataLayers。
    ///
    /// 与 [`crate::app::App::pump_pending_highlights`] 平级 —— 主工作区与嵌入式 toml 编辑器各自独立 drain。
    ///
    /// [`ShellView::render`]: crate::shell::view::ShellView
    pub(crate) fn pump_pending_highlights(&self) {
        self.toml_editor.borrow_mut().pump_pending_highlights();
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

    pub(crate) fn set_toml_slot(&self, slot: Rc<TextEditorSlot>) {
        *self.toml_slot.borrow_mut() = Some(slot);
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
        anchor: SurfaceAnchor::Invoker(super::INVOKER_ID.into()),
        placement: SurfacePlacement {
            invoker_point: SurfaceInvokerPoint::TopLeft,
            corner: Corner::TopRight,
            offset: point(px(0.0), px(18.0)),
            fallback_position: point(px(520.0), px(28.0)),
        },
        focus_on_open: Some(focus),
        render: Rc::new(move || {
            render(
                &runtime.focus,
                runtime.list_state.clone(),
                Rc::clone(&runtime.key_request),
                Rc::clone(&runtime.action_request),
                runtime.toml_slot.borrow().clone(),
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
    toml_slot: Option<Rc<TextEditorSlot>>,
    state: SettingsPanelState,
) -> Div {
    div()
        .w(px(558.0))
        .h(px(500.0))
        .flex()
        .flex_col()
        .rounded(radius::r4())
        .border_1()
        .border_color(color::gray::s05())
        .bg(color::gray::s03())
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
        .child(if state.toml_open {
            toml_body(toml_slot).into_any_element()
        } else {
            body(list_state, action_request, state).into_any_element()
        })
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
        .border_color(color::gray::s05())
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::s4())
                .child(title(if state.toml_open {
                    "config.toml".to_string()
                } else {
                    "设置".to_string()
                }))
                .child(muted(state.path_label())),
        )
        .child(if state.toml_open {
            ghost_button(
                "返回设置".to_string(),
                action_request,
                SettingsAction::ReturnSettings,
            )
        } else {
            ghost_button(
                "打开 TOML".to_string(),
                action_request,
                SettingsAction::OpenToml,
            )
        })
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

fn toml_body(slot: Option<Rc<TextEditorSlot>>) -> Div {
    let Some(slot) = slot else {
        return div().flex_1().bg(color::gray::s01());
    };
    div()
        .flex_1()
        .overflow_hidden()
        .bg(color::gray::s01())
        .p(space::s6())
        .font(typography::editor_font())
        .line_height(typography::editor_line())
        .text_size(typography::editor())
        .text_color(color::gray::s09())
        .child(slot.embed())
}

fn settings_section_item(
    index: usize,
    action_request: Rc<RefCell<Option<SettingsActionRequest>>>,
    state: &SettingsPanelState,
) -> Div {
    let content = match index {
        0 => section(
            "全局".to_string(),
            vec![value_row(
                "主题".to_string(),
                theme_label(&state.config.general.theme),
                "general.theme",
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
                .border_color(color::gray::s05())
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

fn value_row(label: String, value: String, key: &'static str) -> Div {
    setting_row(label, key, value_box(value))
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
        .border_color(color::gray::s05())
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

fn title(text: String) -> Div {
    div()
        .text_size(typography::ui())
        .line_height(typography::ui_line())
        .text_color(color::gray::a09())
        .child(text)
}

fn section_label(text: String) -> Div {
    div()
        .text_size(typography::ui())
        .line_height(typography::ui_line())
        .text_color(color::gray::s08())
        .child(text)
}

fn label_text(text: String) -> Div {
    div()
        .text_size(typography::ui())
        .line_height(typography::ui_line())
        .text_color(color::gray::s09())
        .child(text)
}

fn muted(text: impl Into<String>) -> Div {
    div()
        .text_size(typography::ui())
        .line_height(typography::ui_line())
        .text_color(color::gray::s08())
        .child(text.into())
}

fn ghost_button(
    text: String,
    action_request: Rc<RefCell<Option<SettingsActionRequest>>>,
    action: SettingsAction,
) -> Div {
    div()
        .rounded(radius::r4())
        .border_1()
        .border_color(color::gray::s05())
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            let Some(action_request) = action_request.borrow().clone() else {
                return;
            };
            action_request(action, window, cx);
            cx.stop_propagation();
        })
        .px(space::s8())
        .py(space::s6())
        .text_size(typography::ui())
        .line_height(typography::ui_line())
        .text_color(color::gray::s09())
        .child(text)
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
        .bg(color::gray::s04())
        .px(space::s8())
        .py(space::s6())
        .text_size(typography::ui())
        .line_height(typography::ui_line())
        .text_color(color::gray::a09())
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
        .border_color(color::gray::s05())
        .cursor_pointer()
        .text_size(typography::ui())
        .line_height(typography::ui_line())
        .text_color(color::gray::s09())
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
        .border_color(color::gray::s05())
        .px(space::s8())
        .py(space::s6())
        .text_size(typography::ui())
        .line_height(typography::ui_line())
        .text_color(color::gray::s09())
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
            color::blue::s05()
        } else {
            color::gray::s04()
        })
        .child(
            div()
                .absolute()
                .left(knob_x)
                .top(px(4.0))
                .w(px(12.0))
                .h(px(12.0))
                .rounded(radius::full())
                .bg(color::gray::a09()),
        )
}

fn theme_label(theme: &str) -> String {
    match theme {
        "one-dark" => "One Dark",
        _ => "One Dark",
    }
    .to_string()
}
