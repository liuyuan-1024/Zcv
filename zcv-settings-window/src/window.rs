//! 可视化设置页与窗口导航。

use std::{
    num::{NonZeroU64, NonZeroUsize},
    ops::RangeInclusive,
    time::Duration,
};

use gpui::{
    AnyElement, App, Bounds, Context, DisplayId, Div, Entity, FocusHandle, Focusable, IntoElement,
    Modifiers, Pixels, Render, ScrollHandle, Size, Subscription, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, size,
};
use zcv_actions::OpenSettingsFile;
use zcv_settings::{
    AutosaveSetting, CursorShape, IndentGuideSettingsContent, SettingsStore, SoftWrapMode,
    TerminalCursorShape, UserSettingsContent, update_user_settings,
};
use zcv_theme::{ThemeChoice, color, fixed, scale, typography};
use zcv_title_bar::TitleBar;
use zcv_ui::{
    Button, ContextMenuEntry, DropdownMenu, PopoverLayer, Stepper, SvgIcon, Switch, ToastKind,
    ToastLayer,
};

const DEFAULT_SIZE: Size<Pixels> = size(px(900.0), px(680.0));
const MIN_SIZE: Size<Pixels> = size(px(680.0), px(520.0));

const SIDEBAR_BASE_WIDTH: f32 = 160.0;
const SIDEBAR_MAX_WIDTH: f32 = 200.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsPage {
    Appearance,
    Editor,
    Terminal,
}

impl SettingsPage {
    const ALL: [Self; 3] = [Self::Appearance, Self::Editor, Self::Terminal];

    fn key(self) -> &'static str {
        match self {
            Self::Appearance => "appearance",
            Self::Editor => "editor",
            Self::Terminal => "terminal",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Appearance => "外观",
            Self::Editor => "编辑器",
            Self::Terminal => "终端",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Appearance => "icons/eye.svg",
            Self::Editor => "icons/text_snippet.svg",
            Self::Terminal => "icons/terminal.svg",
        }
    }
}

/// 独立设置窗口；设置值由 `SettingsStore` 持有，窗口只负责呈现与提交修改。
pub struct SettingsWindow {
    focus: FocusHandle,
    scroll_handle: ScrollHandle,
    active_page: SettingsPage,
    toast_layer: Entity<ToastLayer>,
    title_bar: TitleBar,
    popovers: PopoverLayer,
    /// 手工编辑 settings.jsonc 后由设置 global 通知窗口重新渲染。
    _settings_subscription: Subscription,
    _typography_subscription: Subscription,
}

impl SettingsWindow {
    pub fn options(display_id: Option<DisplayId>, cx: &App) -> WindowOptions {
        let bounds = Bounds::centered(display_id, DEFAULT_SIZE, cx);
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            display_id,
            titlebar: Some(TitlebarOptions {
                title: Some("Zcv — 设置".into()),
                appears_transparent: true,
                traffic_light_position: None,
            }),
            focus: true,
            show: true,
            window_min_size: Some(MIN_SIZE.min(&bounds.size)),
            ..Default::default()
        }
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        window.set_rem_size(typography::current(cx).ui_size());
        let settings_subscription = cx.observe_global::<SettingsStore>(|_, cx| cx.notify());
        let typography_subscription = typography::observe_in(window, cx, |_, window, cx| {
            window.set_rem_size(typography::current(cx).ui_size());
            cx.notify();
        });
        Self {
            focus: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
            active_page: SettingsPage::Appearance,
            toast_layer: cx.new(|_| ToastLayer::default()),
            title_bar: TitleBar::default(),
            popovers: PopoverLayer::default(),
            _settings_subscription: settings_subscription,
            _typography_subscription: typography_subscription,
        }
    }

    fn update_setting(
        &mut self,
        cx: &mut Context<Self>,
        title: &'static str,
        change: impl FnOnce(&mut UserSettingsContent),
    ) {
        if let Err(error) = update_user_settings(cx, change) {
            self.show_error(format!("保存设置失败（{title}）：{error:#}"), cx);
        }
    }

    pub fn show_error(&self, message: String, cx: &mut Context<Self>) {
        self.toast_layer.update(cx, |layer, cx| {
            layer.show(
                ToastKind::Error,
                message,
                vec![],
                Some(Duration::from_secs(8)),
                cx,
            );
        });
    }

    fn set_active_page(&mut self, page: SettingsPage, cx: &mut Context<Self>) {
        self.active_page = page;
        self.scroll_handle.set_offset(Default::default());
        cx.notify();
    }

    fn render_top_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let open_file = Button::icon("settings-open-file", "icons/file_text_outlined.svg")
            .label("打开设置文件")
            .on_click(|_, window, cx| {
                window.dispatch_action(Box::new(OpenSettingsFile), cx);
            });

        let content = div()
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("设置"))
            .child(div().flex_1())
            .child(open_file);
        self.title_bar
            .render("settings-top_bar", content, window, cx)
            .into_any_element()
    }

    fn render_body(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(self.render_sidebar(window, cx))
            .child(self.render_settings_page(cx))
    }

    fn render_sidebar(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let colors = *color::current(cx);
        let width = scale::to_pixels(scale::structural(SIDEBAR_BASE_WIDTH), window)
            .clamp(px(SIDEBAR_BASE_WIDTH), px(SIDEBAR_MAX_WIDTH));
        let rows = SettingsPage::ALL.map(|page| {
            let id = format!("settings-nav-{}", page.key());
            let selected = self.active_page == page;
            div()
                .id(id.clone())
                .debug_selector(move || id)
                .flex()
                .items_center()
                .gap(scale::S6)
                .px(scale::S6)
                .py(scale::S4)
                .rounded_sm()
                .cursor_pointer()
                .hover(move |row| row.bg(colors.element_highlight))
                .when(selected, |row| row.bg(colors.element_highlight))
                .child(
                    SvgIcon::new(page.icon())
                        .size(window.rem_size())
                        .color(if selected {
                            colors.icon_accent
                        } else {
                            colors.icon_muted
                        }),
                )
                .child(
                    div()
                        .text_color(if selected {
                            colors.text
                        } else {
                            colors.text_muted
                        })
                        .child(page.label()),
                )
                .on_click(cx.listener(move |settings, _, _, cx| settings.set_active_page(page, cx)))
        });

        div()
            .debug_selector(|| "settings-sidebar".into())
            .w(width)
            .flex_shrink_0()
            .h_full()
            .border_r_1()
            .border_color(colors.border)
            .bg(colors.surface_background)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(scale::S2)
                    .p(scale::S6)
                    .children(rows),
            )
    }

    fn render_settings_page(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let settings = SettingsStore::get(cx);
        let overrides = SettingsStore::user_settings_content(cx);
        let colors = *color::current(cx);
        let ui_size = typography::current(cx).ui_size();
        let theme = settings.theme;
        let ui_font_size_setting = settings.ui_font_size;
        let content_font_size = settings.content_font_size;
        let content_line_height = settings.content_line_height;
        let tab_size = settings.tab.tab_size();
        let insert_spaces = settings.tab.insert_spaces;
        let indent_guides_enabled = settings.indent_guides.enabled;
        let indent_guide_width = settings.indent_guides.line_width;
        let cursor_shape = settings.cursor_shape;
        let cursor_blink = settings.cursor_blink;
        let terminal_font_size = settings.terminal_font_size;
        let terminal_line_height = settings.terminal_line_height;
        let terminal_cursor_shape = settings.terminal_cursor_shape;

        let rows = match self.active_page {
            SettingsPage::Appearance => vec![
                render_setting_row(
                    "theme",
                    "主题",
                    "跟随系统外观，或固定使用深色、浅色主题。",
                    render_setting_dropdown(
                        "theme",
                        "主题",
                        theme,
                        ThemeChoice::available().map(|choice| (choice, theme_label(choice))),
                        |content, value| content.theme = Some(value),
                        &self.popovers,
                        cx,
                    ),
                    ui_size,
                    cx,
                ),
                render_number_row(
                    "ui-font-size",
                    "界面字号",
                    "窗口、标签与菜单等界面文字的字号。",
                    NumberAdjustment::stepped(
                        ui_font_size_setting,
                        format!("{ui_font_size_setting:.0} px"),
                        1.0,
                        11.0..=24.0,
                        |content, value| content.ui_font_size = Some(value),
                    ),
                    ui_size,
                    cx,
                ),
                render_number_row(
                    "content-font-size",
                    "内容字号",
                    "编辑器与预览共用的正文字号。",
                    NumberAdjustment::stepped(
                        content_font_size,
                        format!("{content_font_size:.0} px"),
                        1.0,
                        10.0..=32.0,
                        |content, value| content.content_font_size = Some(value),
                    ),
                    ui_size,
                    cx,
                ),
                render_number_row(
                    "content-line-height",
                    "内容行高",
                    "正文行高相对字号的倍数。",
                    NumberAdjustment::stepped(
                        content_line_height,
                        format!("{content_line_height:.1} ×"),
                        0.1,
                        1.0..=3.0,
                        |content, value| content.content_line_height = Some(value),
                    ),
                    ui_size,
                    cx,
                ),
            ],

            SettingsPage::Editor => self
                .render_autosave_group(settings.autosave, ui_size, cx)
                .into_iter()
                .chain(self.render_wrapping_group(
                    settings.soft_wrap,
                    settings.preferred_line_length,
                    ui_size,
                    cx,
                ))
                .chain([
                    render_number_row(
                        "tab-size",
                        "Tab 宽度",
                        "制表符的视觉列宽，也是一级缩进宽度。",
                        NumberAdjustment {
                            value: format!("{tab_size} 列"),
                            decrease: (tab_size > 1).then(|| tab_size.saturating_sub(1).max(1)),
                            increase: (tab_size < 8).then(|| tab_size.saturating_add(1).min(8)),
                            write: |content, value| content.tab_size = NonZeroUsize::new(value),
                        },
                        ui_size,
                        cx,
                    ),
                    render_setting_row(
                        "insert-spaces",
                        "空格缩进",
                        "使用空格代替真实的 Tab 字符进行缩进。",
                        render_switch(
                            "insert-spaces",
                            "空格缩进",
                            insert_spaces,
                            |content, enabled| content.insert_spaces = Some(enabled),
                            cx,
                        ),
                        ui_size,
                        cx,
                    ),
                    render_setting_row(
                        "indent-guides",
                        "缩进参考线",
                        "显示编辑器中的缩进层级参考线。",
                        render_switch(
                            "indent-guides",
                            "缩进参考线",
                            indent_guides_enabled,
                            |content, enabled| update_indent_guides(content, Some(enabled), None),
                            cx,
                        ),
                        ui_size,
                        cx,
                    ),
                    render_number_row(
                        "indent-guide-width",
                        "参考线宽度",
                        "缩进参考线的宽度（1 到 10 px）。",
                        NumberAdjustment {
                            value: format!("{indent_guide_width} px"),
                            decrease: (indent_guide_width > 1)
                                .then(|| indent_guide_width.saturating_sub(1).max(1)),
                            increase: (indent_guide_width < 10)
                                .then(|| indent_guide_width.saturating_add(1).min(10)),
                            write: |content, value| {
                                update_indent_guides(content, None, Some(value))
                            },
                        },
                        ui_size,
                        cx,
                    ),
                    render_setting_row(
                        "cursor-shape",
                        "光标形状",
                        "编辑器中文本光标的形状。",
                        render_setting_dropdown(
                            "cursor-shape",
                            "光标形状",
                            cursor_shape,
                            [
                                (CursorShape::Bar, "竖线"),
                                (CursorShape::Block, "实心方块"),
                                (CursorShape::Underline, "下划线"),
                                (CursorShape::Hollow, "空心方块"),
                            ],
                            |content, value| content.cursor_shape = Some(value),
                            &self.popovers,
                            cx,
                        ),
                        ui_size,
                        cx,
                    ),
                    render_setting_row(
                        "cursor-blink",
                        "光标闪烁",
                        "编辑器聚焦时是否闪烁光标。",
                        render_switch(
                            "cursor-blink",
                            "光标闪烁",
                            cursor_blink,
                            |content, enabled| content.cursor_blink = Some(enabled),
                            cx,
                        ),
                        ui_size,
                        cx,
                    ),
                ])
                .collect(),

            SettingsPage::Terminal => vec![
                render_number_row(
                    "terminal-font-size",
                    "终端字号",
                    "终端正文的字号。",
                    NumberAdjustment::stepped(
                        terminal_font_size,
                        format!("{terminal_font_size:.0} px"),
                        1.0,
                        10.0..=32.0,
                        |content, value| content.terminal_font_size = Some(value),
                    ),
                    ui_size,
                    cx,
                ),
                render_number_row(
                    "terminal-line-height",
                    "终端行高",
                    "终端行高相对字号的倍数。",
                    NumberAdjustment::stepped(
                        terminal_line_height,
                        format!("{terminal_line_height:.1} ×"),
                        0.1,
                        1.0..=3.0,
                        |content, value| content.terminal_line_height = Some(value),
                    ),
                    ui_size,
                    cx,
                ),
                render_setting_row(
                    "terminal-cursor-shape",
                    "终端光标形状",
                    "终端光标使用的形状。",
                    render_setting_dropdown(
                        "terminal-cursor-shape",
                        "终端光标形状",
                        terminal_cursor_shape,
                        [
                            (TerminalCursorShape::Bar, "竖线"),
                            (TerminalCursorShape::Block, "实心方块"),
                            (TerminalCursorShape::Underline, "下划线"),
                            (TerminalCursorShape::Hollow, "空心方块"),
                        ],
                        |content, value| content.terminal_cursor_shape = Some(value),
                        &self.popovers,
                        cx,
                    ),
                    ui_size,
                    cx,
                ),
            ],
        };

        let override_note = div()
            .px(scale::S6)
            .text_size(ui_size * 0.85)
            .text_color(colors.text_muted)
            .child(format!(
                "settings.jsonc 已显式覆盖 {} 项；未覆盖的字段使用内置默认值。",
                explicit_override_count(&overrides)
            ));

        let body = div()
            .id("settings-scroll")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll_handle)
            .p(scale::S8)
            .flex()
            .flex_col()
            .gap(scale::S16)
            .child(override_note);
        body.child(div().flex().flex_col().gap(scale::S4).children(rows))
    }

    fn render_autosave_group(
        &self,
        autosave: AutosaveSetting,
        ui_size: Pixels,
        cx: &mut Context<Self>,
    ) -> Vec<Div> {
        let delay = match autosave {
            AutosaveSetting::AfterDelay { milliseconds } => Some(milliseconds),
            _ => None,
        };
        let choices = [
            (AutosaveSetting::Off, "关闭"),
            (
                AutosaveSetting::AfterDelay {
                    milliseconds: delay
                        .unwrap_or(NonZeroU64::new(1000).expect("初始延迟必须大于零")),
                },
                "延迟保存",
            ),
            (AutosaveSetting::OnFocusChange, "离开文档时"),
            (AutosaveSetting::OnWindowChange, "离开窗口时"),
        ];
        let mut rows = vec![render_setting_row(
            "autosave",
            "自动保存",
            "选择何时保存未保存的文件。",
            render_setting_dropdown(
                "autosave",
                "自动保存",
                autosave,
                choices,
                |content, value| content.autosave = Some(value),
                &self.popovers,
                cx,
            ),
            ui_size,
            cx,
        )];

        if let Some(milliseconds) = delay {
            rows.push(render_dependent_row(
                render_setting_row(
                    "autosave-delay",
                    "保存延迟",
                    "从停止输入开始计时。",
                    render_stepper(
                        "autosave-delay",
                        "保存延迟",
                        autosave_delay_stepper(milliseconds),
                        cx,
                    ),
                    ui_size,
                    cx,
                ),
                cx,
            ));
        }
        rows
    }

    fn render_wrapping_group(
        &self,
        soft_wrap: SoftWrapMode,
        preferred_line_length: usize,
        ui_size: Pixels,
        cx: &mut Context<Self>,
    ) -> Vec<Div> {
        let mut rows = vec![render_setting_row(
            "soft-wrap",
            "软换行",
            "选择长行的显示方式。",
            render_setting_dropdown(
                "soft-wrap",
                "软换行",
                soft_wrap,
                [
                    (SoftWrapMode::None, "不换行"),
                    (SoftWrapMode::EditorWidth, "按编辑器宽度"),
                    (SoftWrapMode::Bounded, "按目标行宽"),
                ],
                |content, value| content.soft_wrap = Some(value),
                &self.popovers,
                cx,
            ),
            ui_size,
            cx,
        )];

        if soft_wrap == SoftWrapMode::Bounded {
            rows.push(render_dependent_row(
                render_setting_row(
                    "preferred-line-length",
                    "目标行宽",
                    "在目标行宽与编辑器宽度的较小值处换行。",
                    render_stepper(
                        "preferred-line-length",
                        "目标行宽",
                        preferred_line_length_stepper(preferred_line_length),
                        cx,
                    ),
                    ui_size,
                    cx,
                ),
                cx,
            ));
        }
        rows
    }
}

impl Focusable for SettingsWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = *color::current(cx);
        let typography = typography::current(cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .font(typography::ui_font())
            .line_height(typography.ui_line())
            .text_size(typography.ui_size())
            .track_focus(&self.focus)
            .on_key_down(|event, window, cx| {
                let stroke = &event.keystroke;
                if stroke.key == "tab"
                    && let Modifiers {
                        shift,
                        control: false,
                        alt: false,
                        platform: false,
                        function: false,
                    } = stroke.modifiers
                {
                    if shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                    window.prevent_default();
                    cx.stop_propagation();
                }
            })
            .bg(colors.panel_background)
            .child(self.render_top_bar(window, cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_body(window, cx))
                    .child(self.toast_layer.clone()),
            )
    }
}

fn render_dependent_row(row: Div, cx: &App) -> Div {
    row.relative().pl(scale::S12).child(
        div()
            .absolute()
            .left_0()
            .top(scale::S4)
            .bottom(scale::S4)
            .w(fixed::HAIRLINE)
            .bg(color::current(cx).border),
    )
}

fn render_setting_row(
    key: &'static str,
    title: &'static str,
    description: &'static str,
    control: AnyElement,
    ui_size: Pixels,
    cx: &App,
) -> Div {
    let colors = color::current(cx);
    div()
        .debug_selector(move || format!("settings-row-{key}"))
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x(scale::S8)
        .gap_y(scale::S4)
        .mx(scale::S6)
        .py(scale::S6)
        .child(
            div()
                .debug_selector(move || format!("settings-label-{key}"))
                .flex_1()
                .min_w(scale::structural(104.0))
                .flex()
                .flex_col()
                .gap(scale::S2)
                .child(div().text_color(colors.text).child(title))
                .child(
                    div()
                        .text_size(ui_size * 0.85)
                        .text_color(colors.text_muted)
                        .child(description),
                ),
        )
        .child(
            div()
                .debug_selector(move || format!("settings-control-{key}"))
                .ml_auto()
                .flex_none()
                .child(control),
        )
}

struct NumberAdjustment<T> {
    value: String,
    decrease: Option<T>,
    increase: Option<T>,
    write: fn(&mut UserSettingsContent, T),
}

impl NumberAdjustment<f32> {
    fn stepped(
        current: f32,
        value: String,
        step: f32,
        range: RangeInclusive<f32>,
        write: fn(&mut UserSettingsContent, f32),
    ) -> Self {
        let (min, max) = range.into_inner();
        Self {
            value,
            decrease: (current > min).then(|| stepped(current, -step, min, max)),
            increase: (current < max).then(|| stepped(current, step, min, max)),
            write,
        }
    }
}

fn render_number_row<T: Copy + 'static>(
    key: &'static str,
    title: &'static str,
    description: &'static str,
    stepper: NumberAdjustment<T>,
    ui_size: Pixels,
    cx: &Context<SettingsWindow>,
) -> Div {
    render_setting_row(
        key,
        title,
        description,
        render_stepper(key, title, stepper, cx),
        ui_size,
        cx,
    )
}

fn render_stepper<T: Copy + 'static>(
    key: &'static str,
    title: &'static str,
    adjustment: NumberAdjustment<T>,
    cx: &Context<SettingsWindow>,
) -> AnyElement {
    let NumberAdjustment {
        value,
        decrease,
        increase,
        write,
    } = adjustment;
    let window = cx.entity().downgrade();
    let update = move |value| {
        let window = window.clone();
        move |_: &mut Window, cx: &mut App| {
            window
                .update(cx, |settings, cx| {
                    settings.update_setting(cx, title, |content| write(content, value));
                })
                .ok();
        }
    };

    Stepper::new(key, title, value)
        .when_some(decrease, |stepper, value| {
            stepper.on_decrease(update(value))
        })
        .when_some(increase, |stepper, value| {
            stepper.on_increase(update(value))
        })
        .into_any_element()
}

fn render_switch(
    key: &'static str,
    title: &'static str,
    checked: bool,
    write: fn(&mut UserSettingsContent, bool),
    cx: &Context<SettingsWindow>,
) -> AnyElement {
    let window = cx.entity().downgrade();
    Switch::new(key, checked, move |enabled, _, cx| {
        window
            .update(cx, |settings, cx| {
                settings.update_setting(cx, title, |content| write(content, enabled));
            })
            .ok();
    })
    .into_any_element()
}

fn autosave_delay_stepper(milliseconds: NonZeroU64) -> NumberAdjustment<NonZeroU64> {
    let delay = milliseconds.get();
    NumberAdjustment {
        value: format!("{delay} ms"),
        decrease: (delay > 1)
            .then(|| NonZeroU64::new(delay.saturating_sub(250)).unwrap_or(NonZeroU64::MIN)),
        increase: (delay < u64::MAX).then(|| milliseconds.saturating_add(250)),
        write: |content, delay| {
            if let Some(AutosaveSetting::AfterDelay { milliseconds }) = &mut content.autosave {
                *milliseconds = delay;
            }
        },
    }
}

fn preferred_line_length_stepper(length: usize) -> NumberAdjustment<usize> {
    NumberAdjustment {
        value: format!("{length} 列"),
        decrease: (length > 20).then(|| length.saturating_sub(10).max(20)),
        increase: (length < 300).then(|| length.saturating_add(10).min(300)),
        write: |content, value| content.preferred_line_length = Some(value),
    }
}

fn render_setting_dropdown<T: Copy + PartialEq + 'static>(
    key: &'static str,
    title: &'static str,
    current: T,
    choices: impl IntoIterator<Item = (T, &'static str)>,
    write: fn(&mut UserSettingsContent, T),
    layer: &PopoverLayer,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    let entity = cx.entity().downgrade();
    let mut current_label = None;
    let entries = choices
        .into_iter()
        .map(|(value, label)| {
            let checked = value == current;
            if checked {
                current_label = Some(label);
            }
            let entity = entity.clone();
            ContextMenuEntry::new(label)
                .checked(checked)
                .handler(move |_, _, cx| {
                    if !checked {
                        entity
                            .update(cx, |settings, cx| {
                                settings.update_setting(cx, title, |content| write(content, value));
                            })
                            .ok();
                    }
                })
        })
        .collect();
    DropdownMenu::new(
        key,
        current_label.expect("下拉选项必须包含当前设置值"),
        entries,
        layer.clone(),
    )
    .into_any_element()
}

fn theme_label(choice: ThemeChoice) -> &'static str {
    match choice.id() {
        "system" => "跟随系统",
        "dark" => "深色",
        "light" => "浅色",
        other => other,
    }
}

fn explicit_override_count(content: &UserSettingsContent) -> usize {
    [
        content.theme.is_some(),
        content.content_font_size.is_some(),
        content.ui_font_size.is_some(),
        content.content_line_height.is_some(),
        content.tab_size.is_some(),
        content.insert_spaces.is_some(),
        content.indent_guides.is_some(),
        content.soft_wrap.is_some(),
        content.minimum_contrast_for_highlights.is_some(),
        content.cursor_shape.is_some(),
        content.cursor_blink.is_some(),
        content.cursor_animation.is_some(),
        content.preferred_line_length.is_some(),
        content.file_scan_exclusions.is_some(),
        content.use_autoclose.is_some(),
        content.use_auto_surround.is_some(),
        content.auto_indent.is_some(),
        content.autosave.is_some(),
        content.extend_comment_on_newline.is_some(),
        content.extend_list_on_newline.is_some(),
        content.remove_trailing_whitespace_on_save.is_some(),
        content.ensure_final_newline_on_save.is_some(),
        content.terminal_font_size.is_some(),
        content.terminal_line_height.is_some(),
        content.terminal_max_scroll_history_lines.is_some(),
        content.terminal_cursor_shape.is_some(),
        content.terminal_alternate_scroll.is_some(),
        content.terminal_option_as_meta.is_some(),
        content.terminal_shell.is_some(),
        content.languages.is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count()
}

fn stepped(value: f32, step: f32, min: f32, max: f32) -> f32 {
    let next = (value + step).clamp(min, max);
    (next * 10.0).round() / 10.0
}

fn update_indent_guides(
    content: &mut UserSettingsContent,
    enabled: Option<bool>,
    line_width: Option<u32>,
) {
    let current = content.indent_guides.unwrap_or_default();
    content.indent_guides = Some(IndentGuideSettingsContent {
        enabled: enabled.or(current.enabled),
        line_width: line_width
            .and_then(std::num::NonZeroU32::new)
            .or(current.line_width),
    });
}

#[cfg(test)]
#[path = "test/window_tests.rs"]
mod tests;
