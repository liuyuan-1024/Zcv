//! 设置 surface。

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    Context, Div, FocusHandle, InteractiveElement, IntoElement, ListAlignment, ListState,
    MouseButton, Window, div, list, prelude::*, px,
};
use zom_command::commands::settings as settings_commands;

use crate::config::{AppConfig, SettingsChange};
use crate::shell::shared::{CommandBinding, Glyph, scroll};
use crate::shell::surfaces::{SurfaceAnchor, SurfaceRequest, WindowPosition};
use crate::shell::{CommandTitleLookup, ShortcutLookup};
use crate::theme::{Theme, color, radius, space};
use crate::ui_id::SurfaceId;

#[derive(Clone, Debug, Default)]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SettingsIntent {
    OpenToml,
    Change(SettingsChange),
}

pub(crate) type SettingsIntentRequest = Rc<dyn Fn(SettingsIntent, &mut Window, &mut gpui::App)>;

#[derive(Clone)]
pub(crate) struct SettingsRuntime {
    focus: FocusHandle,
    list_state: ListState,
    intent_request: Rc<RefCell<Option<SettingsIntentRequest>>>,
    title_lookup: Rc<RefCell<Option<CommandTitleLookup>>>,
    shortcut_lookup: Rc<RefCell<Option<ShortcutLookup>>>,
    state: Rc<RefCell<SettingsPanelState>>,
}

impl SettingsRuntime {
    pub(crate) fn new<T>(cx: &mut Context<T>) -> Self {
        Self {
            focus: cx.focus_handle(),
            list_state: ListState::new(SETTINGS_SECTION_COUNT, ListAlignment::Top, px(48.0))
                .measure_all(),
            intent_request: Rc::new(RefCell::new(None)),
            title_lookup: Rc::new(RefCell::new(None)),
            shortcut_lookup: Rc::new(RefCell::new(None)),
            state: Rc::new(RefCell::new(SettingsPanelState::default())),
        }
    }

    pub(crate) fn set_intent_request(&self, intent_request: SettingsIntentRequest) {
        *self.intent_request.borrow_mut() = Some(intent_request);
    }

    pub(crate) fn set_title_lookup(&self, title_lookup: CommandTitleLookup) {
        *self.title_lookup.borrow_mut() = Some(title_lookup);
    }

    pub(crate) fn set_shortcut_lookup(&self, shortcut_lookup: ShortcutLookup) {
        *self.shortcut_lookup.borrow_mut() = Some(shortcut_lookup);
    }

    pub(crate) fn set_state(&self, state: SettingsPanelState) {
        *self.state.borrow_mut() = state;
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }
}

// ── 渲染上下文 ──
// 把三个运行时注入的 lookup 归拢到一个 struct 里，渲染函数只传 &ctx。

#[derive(Clone)]
struct RenderCtx {
    intent: Rc<RefCell<Option<SettingsIntentRequest>>>,
    title: Rc<RefCell<Option<CommandTitleLookup>>>,
    shortcut: Rc<RefCell<Option<ShortcutLookup>>>,
}

impl RenderCtx {
    fn from_runtime(runtime: &SettingsRuntime) -> Self {
        Self {
            intent: Rc::clone(&runtime.intent_request),
            title: Rc::clone(&runtime.title_lookup),
            shortcut: Rc::clone(&runtime.shortcut_lookup),
        }
    }

    /// 构造一条 [`CommandBinding`]：点击时执行 `intent`，悬浮时从 title/shortcut lookup 取 tooltip。
    fn binding(&self, command_id: &'static str, intent: SettingsIntent) -> CommandBinding {
        let intent_request = self.intent.borrow().clone();
        CommandBinding {
            id: command_id.to_string(),
            title: self
                .title
                .borrow()
                .clone()
                .unwrap_or_else(|| Rc::new(|_| None)),
            shortcut: self
                .shortcut
                .borrow()
                .clone()
                .unwrap_or_else(|| Rc::new(|_| None)),
            request: Rc::new(move |window, cx| {
                if let Some(f) = intent_request.as_ref() {
                    f(intent, window, cx);
                }
            }),
        }
    }

    /// 给非 Glyph 控件挂上 click 行为（pill / toggle）。
    fn clickable(&self, control: Div, intent: SettingsIntent) -> Div {
        let intent_request = self.intent.clone();
        control
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                if let Some(f) = intent_request.borrow().as_ref() {
                    f(intent, window, cx);
                }
                cx.stop_propagation();
            })
    }
}

// ── surface 入口 ──

pub(crate) fn request(runtime: SettingsRuntime) -> SurfaceRequest {
    let focus = runtime.focus.clone();
    let ctx = RenderCtx::from_runtime(&runtime);
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
                &ctx,
                runtime.state.borrow().clone(),
            )
            .into_any_element()
        }),
    }
}

fn render(
    focus: &FocusHandle,
    list_state: ListState,
    ctx: &RenderCtx,
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
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .child(header(&state, ctx))
        .child(body(list_state, ctx, state))
}

// ── 头部 ──

fn header(state: &SettingsPanelState, ctx: &RenderCtx) -> Div {
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
        .child(ctx.clickable(pill("打开 TOML".to_string()), SettingsIntent::OpenToml))
}

// ── 列表 body ──

const SETTINGS_SECTION_COUNT: usize = 3;

fn body(list_state: ListState, ctx: &RenderCtx, state: SettingsPanelState) -> impl IntoElement {
    if list_state.item_count() != SETTINGS_SECTION_COUNT {
        list_state.reset(SETTINGS_SECTION_COUNT);
    }

    let ctx = ctx.clone();
    div()
        .relative()
        .flex_1()
        .overflow_hidden()
        .p(space::s4())
        .child(
            list(list_state.clone(), move |index, _, _| {
                settings_section_item(index, &ctx, &state).into_any_element()
            })
            .w_full()
            .h_full(),
        )
        .child(scroll::list_scrollbar(&list_state))
}

fn settings_section_item(index: usize, ctx: &RenderCtx, state: &SettingsPanelState) -> Div {
    let content = match index {
        0 => section(
            "全局".to_string(),
            vec![select_row(
                "主题".to_string(),
                theme_label(&state.config.general.theme),
                "general.theme",
                ctx,
                SettingsIntent::Change(SettingsChange::CycleTheme),
            )],
        ),
        1 => section(
            "界面".to_string(),
            vec![stepper_row(
                "字号".to_string(),
                format!("{} px", state.config.ui.font_size),
                "ui.font_size",
                ctx,
                SettingsChange::AdjustUiFont(-1),
                SettingsChange::AdjustUiFont(1),
                settings_commands::DECREASE_UI_FONT_SIZE,
                settings_commands::INCREASE_UI_FONT_SIZE,
            )],
        ),
        2 => section(
            "编辑器".to_string(),
            vec![
                stepper_row(
                    "编辑字号".to_string(),
                    format!("{} px", state.config.editor.font_size),
                    "editor.font_size",
                    ctx,
                    SettingsChange::AdjustEditorFont(-1),
                    SettingsChange::AdjustEditorFont(1),
                    settings_commands::DECREASE_EDITOR_FONT_SIZE,
                    settings_commands::INCREASE_EDITOR_FONT_SIZE,
                ),
                toggle_row(
                    "软换行".to_string(),
                    state.config.editor.soft_wrap,
                    "editor.soft_wrap",
                    ctx,
                    SettingsIntent::Change(SettingsChange::ToggleEditorSoftWrap),
                ),
                select_row(
                    "Tab 宽度".to_string(),
                    state.config.editor.tab_size.to_string(),
                    "editor.tab_size",
                    ctx,
                    SettingsIntent::Change(SettingsChange::CycleEditorTabSize),
                ),
            ],
        ),
        _ => div(),
    };

    div().px(space::s12()).pb(space::s12()).child(content)
}

// ── 行控件 ──

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
    ctx: &RenderCtx,
    intent: SettingsIntent,
) -> Div {
    setting_row(label, key, ctx.clickable(pill(value), intent))
}

fn stepper_row(
    label: String,
    value: String,
    key: &'static str,
    ctx: &RenderCtx,
    decrement: SettingsChange,
    increment: SettingsChange,
    decrement_cmd: &'static str,
    increment_cmd: &'static str,
) -> Div {
    setting_row(
        label,
        key,
        stepper(
            value,
            ctx,
            decrement,
            increment,
            decrement_cmd,
            increment_cmd,
        ),
    )
}

fn toggle_row(
    label: String,
    enabled: bool,
    key: &'static str,
    ctx: &RenderCtx,
    intent: SettingsIntent,
) -> Div {
    setting_row(label, key, ctx.clickable(toggle(enabled), intent))
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

// ── 通用原子控件 ──

fn section_label(text: String) -> Div {
    div().text_color(color::current().gray.s08).child(text)
}

fn label_text(text: String) -> Div {
    div().text_color(color::current().gray.s09).child(text)
}

fn muted(text: impl Into<String>) -> Div {
    div()
        .text_color(color::current().gray.s08)
        .child(text.into())
}

fn pill(text: String) -> Div {
    div()
        .rounded(radius::r4())
        .border_1()
        .border_color(color::current().gray.s05)
        .px(space::s8())
        .py(space::s6())
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
        .text_color(color::current().gray.s09)
        .child(text)
}

// ── stepper ──

const MINUS_ICON: &str = "icons/actions/square_minus.svg";
const PLUS_ICON: &str = "icons/actions/square_plus.svg";

fn stepper(
    value: String,
    ctx: &RenderCtx,
    decrement: SettingsChange,
    increment: SettingsChange,
    decrement_cmd: &'static str,
    increment_cmd: &'static str,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(space::s4())
        .child(stepper_button(
            MINUS_ICON,
            ctx,
            SettingsIntent::Change(decrement),
            decrement_cmd,
        ))
        .child(value_box(value))
        .child(stepper_button(
            PLUS_ICON,
            ctx,
            SettingsIntent::Change(increment),
            increment_cmd,
        ))
}

/// stepper 按钮——纯 Glyph 渲染，点击与 tooltip 都走 [`CommandBinding`]。
fn stepper_button(
    icon_path: &'static str,
    ctx: &RenderCtx,
    intent: SettingsIntent,
    command_id: &'static str,
) -> impl IntoElement {
    Glyph::icon(command_id, icon_path)
        .command(ctx.binding(command_id, intent))
        .render()
}

fn theme_label(theme: &str) -> String {
    Theme::from_config(theme).label().to_string()
}
