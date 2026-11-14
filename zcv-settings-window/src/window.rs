//! 可视化设置页与窗口导航。

use std::{num::NonZeroUsize, time::Duration};

use gpui::{
    AnyElement, App, Context, Div, ElementId, Entity, FocusHandle, Focusable, IntoElement, Render,
    ScrollHandle, Subscription, Window, div, prelude::*, px,
};
use zcv_actions::OpenSettingsFile;
use zcv_settings::{
    CursorShape, IndentGuideSettingsContent, SettingsStore, SoftWrapMode, TerminalCursorShape,
    UserSettingsContent, update_user_settings,
};
use zcv_theme::{ThemeChoice, color, scale, typography};
use zcv_ui::{Button, ButtonStyle, Checkbox, SvgIcon};
use zcv_workspace::{TitleBar, ToastKind, ToastLayer};

type WindowAction = Box<dyn Fn(&mut SettingsWindow, &mut Context<SettingsWindow>)>;

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
    /// 手工编辑 settings.jsonc 后由设置 global 通知窗口重新渲染。
    _settings_subscription: Subscription,
    _typography_subscription: Subscription,
}

impl SettingsWindow {
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

    fn render_body(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_1()
            .min_h_0()
            .child(self.render_sidebar(cx))
            .child(self.render_settings_page(cx))
            .into_any_element()
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = *color::current(cx);
        let settings_window = cx.entity();
        let mut nav = div().flex().flex_col().gap(scale::S2).p(scale::S6);

        for page in SettingsPage::ALL {
            let selected = self.active_page == page;
            let settings_window = settings_window.clone();
            nav = nav.child(
                div()
                    .id(ElementId::Name(
                        format!("settings-nav-{}", page.key()).into(),
                    ))
                    .flex()
                    .items_center()
                    .gap(scale::S6)
                    .px(scale::S6)
                    .py(scale::S4)
                    .rounded_sm()
                    .cursor_pointer()
                    .when(selected, |element| element.bg(colors.element_selected))
                    .hover(|style| style.bg(colors.ghost_element_hover))
                    .on_click(move |_, _window, cx| {
                        settings_window.update(cx, |settings_window, cx| {
                            settings_window.set_active_page(page, cx)
                        });
                    })
                    .child(SvgIcon::new(page.icon()).size(px(14.0)).color(if selected {
                        colors.icon_accent
                    } else {
                        colors.icon_muted
                    }))
                    .child(
                        div()
                            .text_color(if selected {
                                colors.text
                            } else {
                                colors.text_muted
                            })
                            .child(page.label()),
                    ),
            );
        }

        div()
            .w(px(220.0))
            .flex_shrink_0()
            .h_full()
            .border_r_1()
            .border_color(colors.border)
            .bg(colors.surface_background)
            .child(nav)
            .into_any_element()
    }

    fn render_settings_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let settings = SettingsStore::get(cx);
        let overrides = SettingsStore::user_settings_content(cx);
        let colors = *color::current(cx);
        let ui_size = typography::current(cx).ui_size();
        let theme = settings.theme;
        let ui_font_size_setting = settings.ui_font_size;
        let content_font_size = settings.content_font_size;
        let content_line_height = settings.content_line_height;
        let soft_wrap = settings.soft_wrap;
        let preferred_line_length = settings.preferred_line_length;
        let tab_size = settings.tab.tab_size();
        let insert_spaces = settings.tab.insert_spaces;
        let indent_guides_enabled = settings.indent_guides.enabled;
        let indent_guide_width = settings.indent_guides.line_width;
        let cursor_shape = settings.cursor_shape;
        let cursor_blink = settings.cursor_blink;
        let terminal_font_size = settings.terminal_font_size;
        let terminal_line_height = settings.terminal_line_height;
        let terminal_cursor_shape = settings.terminal_cursor_shape;

        let page = match self.active_page {
            SettingsPage::Appearance => render_section(
                "外观",
                vec![
                    render_setting_row(
                        "theme",
                        "主题",
                        "跟随系统外观，或固定使用深色、浅色主题。",
                        render_theme_segments(theme, cx),
                        ui_size,
                        cx,
                    ),
                    render_number_row(
                        "ui-font-size",
                        "界面字号",
                        "窗口、标签与菜单等界面文字的字号（像素）。",
                        Stepper {
                            value: format!("{:.0}", ui_font_size_setting),
                            can_decrease: ui_font_size_setting > 11.0,
                            can_increase: ui_font_size_setting < 24.0,
                            decrease: Box::new(move |settings_window, cx| {
                                let value = stepped(ui_font_size_setting, -1.0, 11.0, 24.0);
                                settings_window.update_setting(cx, "界面字号", |content| {
                                    content.ui_font_size = Some(value);
                                });
                            }),
                            increase: Box::new(move |settings_window, cx| {
                                let value = stepped(ui_font_size_setting, 1.0, 11.0, 24.0);
                                settings_window.update_setting(cx, "界面字号", |content| {
                                    content.ui_font_size = Some(value);
                                });
                            }),
                        },
                        ui_size,
                        cx,
                    ),
                    render_number_row(
                        "content-font-size",
                        "内容字号",
                        "编辑器与预览共用的正文字号（像素）。",
                        Stepper {
                            value: format!("{:.0}", content_font_size),
                            can_decrease: content_font_size > 10.0,
                            can_increase: content_font_size < 32.0,
                            decrease: Box::new(move |settings_window, cx| {
                                let value = stepped(content_font_size, -1.0, 10.0, 32.0);
                                settings_window.update_setting(cx, "内容字号", |content| {
                                    content.content_font_size = Some(value);
                                });
                            }),
                            increase: Box::new(move |settings_window, cx| {
                                let value = stepped(content_font_size, 1.0, 10.0, 32.0);
                                settings_window.update_setting(cx, "内容字号", |content| {
                                    content.content_font_size = Some(value);
                                });
                            }),
                        },
                        ui_size,
                        cx,
                    ),
                    render_number_row(
                        "content-line-height",
                        "内容行高",
                        "正文行高相对字号的倍数。",
                        Stepper {
                            value: format!("{:.1}", content_line_height),
                            can_decrease: content_line_height > 1.0,
                            can_increase: content_line_height < 3.0,
                            decrease: Box::new(move |settings_window, cx| {
                                let value = stepped(content_line_height, -0.1, 1.0, 3.0);
                                settings_window.update_setting(cx, "内容行高", |content| {
                                    content.content_line_height = Some(value);
                                });
                            }),
                            increase: Box::new(move |settings_window, cx| {
                                let value = stepped(content_line_height, 0.1, 1.0, 3.0);
                                settings_window.update_setting(cx, "内容行高", |content| {
                                    content.content_line_height = Some(value);
                                });
                            }),
                        },
                        ui_size,
                        cx,
                    ),
                ],
                ui_size,
                cx,
            ),

            SettingsPage::Editor => render_section(
                "编辑器",
                vec![
                    render_setting_row(
                        "soft-wrap",
                        "软换行",
                        "超长行是水平滚动，还是按编辑器宽度或目标行宽换行。",
                        render_soft_wrap_segments(soft_wrap, cx),
                        ui_size,
                        cx,
                    ),
                    render_number_row(
                        "preferred-line-length",
                        "目标行宽",
                        "soft_wrap 为“固定行宽”时使用的目标列数。",
                        Stepper {
                            value: preferred_line_length.to_string(),
                            can_decrease: preferred_line_length > 20,
                            can_increase: preferred_line_length < 300,
                            decrease: Box::new(move |settings_window, cx| {
                                let value = preferred_line_length.saturating_sub(10).max(20);
                                settings_window.update_setting(cx, "目标行宽", |content| {
                                    content.preferred_line_length = Some(value);
                                });
                            }),
                            increase: Box::new(move |settings_window, cx| {
                                let value = preferred_line_length.saturating_add(10).min(300);
                                settings_window.update_setting(cx, "目标行宽", |content| {
                                    content.preferred_line_length = Some(value);
                                });
                            }),
                        },
                        ui_size,
                        cx,
                    ),
                    render_number_row(
                        "tab-size",
                        "Tab 宽度",
                        "制表符的视觉列宽，也是一级缩进宽度。",
                        Stepper {
                            value: tab_size.to_string(),
                            can_decrease: tab_size > 1,
                            can_increase: tab_size < 8,
                            decrease: Box::new(move |settings_window, cx| {
                                let value = tab_size.saturating_sub(1).max(1);
                                settings_window.update_setting(cx, "Tab 宽度", |content| {
                                    content.tab_size = NonZeroUsize::new(value);
                                });
                            }),
                            increase: Box::new(move |settings_window, cx| {
                                let value = tab_size.saturating_add(1).min(8);
                                settings_window.update_setting(cx, "Tab 宽度", |content| {
                                    content.tab_size = NonZeroUsize::new(value);
                                });
                            }),
                        },
                        ui_size,
                        cx,
                    ),
                    render_setting_row(
                        "insert-spaces",
                        "空格缩进",
                        "使用空格代替真实的 Tab 字符进行缩进。",
                        render_checkbox(
                            "insert-spaces",
                            insert_spaces,
                            cx,
                            move |settings_window, cx| {
                                let value = !insert_spaces;
                                settings_window.update_setting(cx, "空格缩进", |content| {
                                    content.insert_spaces = Some(value);
                                });
                            },
                        ),
                        ui_size,
                        cx,
                    ),
                    render_setting_row(
                        "indent-guides",
                        "缩进参考线",
                        "显示编辑器中的缩进层级参考线。",
                        render_checkbox(
                            "indent-guides",
                            indent_guides_enabled,
                            cx,
                            move |settings_window, cx| {
                                let enabled = !indent_guides_enabled;
                                settings_window.update_setting(cx, "缩进参考线", |content| {
                                    update_indent_guides(content, Some(enabled), None);
                                });
                            },
                        ),
                        ui_size,
                        cx,
                    ),
                    render_number_row(
                        "indent-guide-width",
                        "参考线宽度",
                        "缩进参考线的像素宽度（1 到 10）。",
                        Stepper {
                            value: indent_guide_width.to_string(),
                            can_decrease: indent_guide_width > 1,
                            can_increase: indent_guide_width < 10,
                            decrease: Box::new(move |settings_window, cx| {
                                let value = indent_guide_width.saturating_sub(1).max(1);
                                settings_window.update_setting(cx, "参考线宽度", |content| {
                                    update_indent_guides(content, None, Some(value));
                                });
                            }),
                            increase: Box::new(move |settings_window, cx| {
                                let value = indent_guide_width.saturating_add(1).min(10);
                                settings_window.update_setting(cx, "参考线宽度", |content| {
                                    update_indent_guides(content, None, Some(value));
                                });
                            }),
                        },
                        ui_size,
                        cx,
                    ),
                    render_setting_row(
                        "cursor-shape",
                        "光标形状",
                        "编辑器中文本光标的形状。",
                        render_cursor_segments(cursor_shape, cx),
                        ui_size,
                        cx,
                    ),
                    render_setting_row(
                        "cursor-blink",
                        "光标闪烁",
                        "编辑器聚焦时是否闪烁光标。",
                        render_checkbox(
                            "cursor-blink",
                            cursor_blink,
                            cx,
                            move |settings_window, cx| {
                                let value = !cursor_blink;
                                settings_window.update_setting(cx, "光标闪烁", |content| {
                                    content.cursor_blink = Some(value);
                                });
                            },
                        ),
                        ui_size,
                        cx,
                    ),
                ],
                ui_size,
                cx,
            ),

            SettingsPage::Terminal => render_section(
                "终端",
                vec![
                    render_number_row(
                        "terminal-font-size",
                        "终端字号",
                        "终端正文的字号（像素）。",
                        Stepper {
                            value: format!("{:.0}", terminal_font_size),
                            can_decrease: terminal_font_size > 10.0,
                            can_increase: terminal_font_size < 32.0,
                            decrease: Box::new(move |settings_window, cx| {
                                let value = stepped(terminal_font_size, -1.0, 10.0, 32.0);
                                settings_window.update_setting(cx, "终端字号", |content| {
                                    content.terminal_font_size = Some(value);
                                });
                            }),
                            increase: Box::new(move |settings_window, cx| {
                                let value = stepped(terminal_font_size, 1.0, 10.0, 32.0);
                                settings_window.update_setting(cx, "终端字号", |content| {
                                    content.terminal_font_size = Some(value);
                                });
                            }),
                        },
                        ui_size,
                        cx,
                    ),
                    render_number_row(
                        "terminal-line-height",
                        "终端行高",
                        "终端行高相对字号的倍数。",
                        Stepper {
                            value: format!("{:.1}", terminal_line_height),
                            can_decrease: terminal_line_height > 1.0,
                            can_increase: terminal_line_height < 3.0,
                            decrease: Box::new(move |settings_window, cx| {
                                let value = stepped(terminal_line_height, -0.1, 1.0, 3.0);
                                settings_window.update_setting(cx, "终端行高", |content| {
                                    content.terminal_line_height = Some(value);
                                });
                            }),
                            increase: Box::new(move |settings_window, cx| {
                                let value = stepped(terminal_line_height, 0.1, 1.0, 3.0);
                                settings_window.update_setting(cx, "终端行高", |content| {
                                    content.terminal_line_height = Some(value);
                                });
                            }),
                        },
                        ui_size,
                        cx,
                    ),
                    render_setting_row(
                        "terminal-cursor-shape",
                        "终端光标形状",
                        "终端光标使用的形状。",
                        render_terminal_cursor_segments(terminal_cursor_shape, cx),
                        ui_size,
                        cx,
                    ),
                ],
                ui_size,
                cx,
            ),
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
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll_handle)
            .p(scale::S8)
            .flex()
            .flex_col()
            .gap(scale::S16)
            .child(override_note);
        body.child(page).into_any_element()
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
            .bg(colors.panel_background)
            .child(self.render_top_bar(window, cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_body(cx))
                    .child(self.toast_layer.clone()),
            )
    }
}

fn render_section(
    title: &'static str,
    rows: Vec<Div>,
    ui_size: gpui::Pixels,
    cx: &mut Context<SettingsWindow>,
) -> Div {
    let colors = *color::current(cx);
    div()
        .flex()
        .flex_col()
        .gap(scale::S4)
        .child(
            div()
                .px(scale::S6)
                .text_size(ui_size * 0.85)
                .text_color(colors.text_muted)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(title),
        )
        .children(rows)
}

fn render_setting_row(
    key: &'static str,
    title: &'static str,
    description: &'static str,
    control: AnyElement,
    ui_size: gpui::Pixels,
    cx: &mut Context<SettingsWindow>,
) -> Div {
    let colors = *color::current(cx);
    div()
        .debug_selector(move || format!("settings-row-{key}"))
        .flex()
        .flex_col()
        .gap(scale::S4)
        .px(scale::S6)
        .py(scale::S4)
        .child(
            div()
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
        .child(control)
}

struct Stepper {
    value: String,
    can_decrease: bool,
    can_increase: bool,
    decrease: WindowAction,
    increase: WindowAction,
}

fn render_number_row(
    key: &'static str,
    title: &'static str,
    description: &'static str,
    stepper: Stepper,
    ui_size: gpui::Pixels,
    cx: &mut Context<SettingsWindow>,
) -> Div {
    render_setting_row(
        key,
        title,
        description,
        render_stepper(key, stepper, cx),
        ui_size,
        cx,
    )
}

fn render_stepper(
    key: &'static str,
    stepper: Stepper,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    let Stepper {
        value,
        can_decrease,
        can_increase,
        decrease,
        increase,
    } = stepper;
    let colors = *color::current(cx);
    let decrease_entity = cx.entity();
    let increase_entity = cx.entity();
    div()
        .flex()
        .items_center()
        .gap(scale::S2)
        .child(
            Button::icon(
                ElementId::Name(format!("settings-{key}-decrease").into()),
                "icons/dash.svg",
            )
            .disabled(!can_decrease)
            .label("减小")
            .on_click(move |_, _, cx| {
                decrease_entity.update(cx, |settings_window, cx| decrease(settings_window, cx));
            }),
        )
        .child(
            div()
                .min_w(scale::S12)
                .text_center()
                .text_color(colors.text)
                .child(value),
        )
        .child(
            Button::icon(
                ElementId::Name(format!("settings-{key}-increase").into()),
                "icons/plus.svg",
            )
            .disabled(!can_increase)
            .label("增大")
            .on_click(move |_, _, cx| {
                increase_entity.update(cx, |settings_window, cx| increase(settings_window, cx));
            }),
        )
        .into_any_element()
}

fn render_checkbox(
    key: &'static str,
    checked: bool,
    cx: &mut Context<SettingsWindow>,
    change: impl Fn(&mut SettingsWindow, &mut Context<SettingsWindow>) + 'static,
) -> AnyElement {
    let entity = cx.entity();
    div()
        .flex()
        .items_center()
        .child(
            Checkbox::new(
                ElementId::Name(format!("settings-{key}-checkbox").into()),
                checked,
            )
            .tooltip(if checked {
                "已开启，点击关闭"
            } else {
                "已关闭，点击开启"
            })
            .on_click(move |_window, cx| {
                entity.update(cx, |settings_window, cx| change(settings_window, cx));
            }),
        )
        .into_any_element()
}

fn render_theme_segments(selected: ThemeChoice, cx: &mut Context<SettingsWindow>) -> AnyElement {
    let segments = ThemeChoice::available()
        .map(|choice| {
            let label = theme_label(choice);
            let selected = selected == choice;
            let action: WindowAction = Box::new(move |settings_window, cx| {
                settings_window.update_setting(cx, "主题", |content| {
                    content.theme = Some(choice);
                });
            });
            (format!("theme-{}", choice.id()), label, selected, action)
        })
        .collect();
    render_segments("theme", segments, cx)
}

fn render_soft_wrap_segments(
    selected: SoftWrapMode,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    render_segments(
        "soft-wrap",
        vec![
            (
                "none".to_string(),
                "不换行",
                selected == SoftWrapMode::None,
                Box::new(
                    move |settings_window: &mut SettingsWindow,
                          cx: &mut Context<SettingsWindow>| {
                        settings_window.update_setting(cx, "软换行", |content| {
                            content.soft_wrap = Some(SoftWrapMode::None);
                        });
                    },
                ),
            ),
            (
                "editor-width".to_string(),
                "编辑器宽度",
                selected == SoftWrapMode::EditorWidth,
                Box::new(
                    move |settings_window: &mut SettingsWindow,
                          cx: &mut Context<SettingsWindow>| {
                        settings_window.update_setting(cx, "软换行", |content| {
                            content.soft_wrap = Some(SoftWrapMode::EditorWidth);
                        });
                    },
                ),
            ),
            (
                "bounded".to_string(),
                "固定行宽",
                selected == SoftWrapMode::Bounded,
                Box::new(
                    move |settings_window: &mut SettingsWindow,
                          cx: &mut Context<SettingsWindow>| {
                        settings_window.update_setting(cx, "软换行", |content| {
                            content.soft_wrap = Some(SoftWrapMode::Bounded);
                        });
                    },
                ),
            ),
        ],
        cx,
    )
}

fn render_cursor_segments(selected: CursorShape, cx: &mut Context<SettingsWindow>) -> AnyElement {
    render_segments(
        "cursor-shape",
        vec![
            (
                "bar".to_string(),
                "竖线",
                selected == CursorShape::Bar,
                Box::new(
                    move |settings_window: &mut SettingsWindow,
                          cx: &mut Context<SettingsWindow>| {
                        settings_window.update_setting(cx, "光标形状", |content| {
                            content.cursor_shape = Some(CursorShape::Bar);
                        });
                    },
                ),
            ),
            (
                "block".to_string(),
                "方块",
                selected == CursorShape::Block,
                Box::new(
                    move |settings_window: &mut SettingsWindow,
                          cx: &mut Context<SettingsWindow>| {
                        settings_window.update_setting(cx, "光标形状", |content| {
                            content.cursor_shape = Some(CursorShape::Block);
                        });
                    },
                ),
            ),
            (
                "underline".to_string(),
                "下划线",
                selected == CursorShape::Underline,
                Box::new(
                    move |settings_window: &mut SettingsWindow,
                          cx: &mut Context<SettingsWindow>| {
                        settings_window.update_setting(cx, "光标形状", |content| {
                            content.cursor_shape = Some(CursorShape::Underline);
                        });
                    },
                ),
            ),
            (
                "hollow".to_string(),
                "空心方块",
                selected == CursorShape::Hollow,
                Box::new(
                    move |settings_window: &mut SettingsWindow,
                          cx: &mut Context<SettingsWindow>| {
                        settings_window.update_setting(cx, "光标形状", |content| {
                            content.cursor_shape = Some(CursorShape::Hollow);
                        });
                    },
                ),
            ),
        ],
        cx,
    )
}

fn render_terminal_cursor_segments(
    selected: TerminalCursorShape,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    render_segments(
        "terminal-cursor-shape",
        vec![
            (
                "block".to_string(),
                "方块",
                selected == TerminalCursorShape::Block,
                Box::new(
                    move |settings_window: &mut SettingsWindow,
                          cx: &mut Context<SettingsWindow>| {
                        settings_window.update_setting(cx, "终端光标形状", |content| {
                            content.terminal_cursor_shape = Some(TerminalCursorShape::Block);
                        });
                    },
                ),
            ),
            (
                "underline".to_string(),
                "下划线",
                selected == TerminalCursorShape::Underline,
                Box::new(
                    move |settings_window: &mut SettingsWindow,
                          cx: &mut Context<SettingsWindow>| {
                        settings_window.update_setting(cx, "终端光标形状", |content| {
                            content.terminal_cursor_shape = Some(TerminalCursorShape::Underline);
                        });
                    },
                ),
            ),
            (
                "bar".to_string(),
                "竖线",
                selected == TerminalCursorShape::Bar,
                Box::new(
                    move |settings_window: &mut SettingsWindow,
                          cx: &mut Context<SettingsWindow>| {
                        settings_window.update_setting(cx, "终端光标形状", |content| {
                            content.terminal_cursor_shape = Some(TerminalCursorShape::Bar);
                        });
                    },
                ),
            ),
            (
                "hollow".to_string(),
                "空心方块",
                selected == TerminalCursorShape::Hollow,
                Box::new(
                    move |settings_window: &mut SettingsWindow,
                          cx: &mut Context<SettingsWindow>| {
                        settings_window.update_setting(cx, "终端光标形状", |content| {
                            content.terminal_cursor_shape = Some(TerminalCursorShape::Hollow);
                        });
                    },
                ),
            ),
        ],
        cx,
    )
}

fn render_segments(
    key: &'static str,
    segments: Vec<(String, &'static str, bool, WindowAction)>,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    let colors = *color::current(cx);
    let entity = cx.entity();
    let mut row = div()
        .id(ElementId::Name(format!("settings-{key}-segments").into()))
        .flex()
        .flex_wrap()
        .gap(scale::S2);
    for (id, label, selected, action) in segments {
        let entity = entity.clone();
        row = row.child(
            Button::text(
                ElementId::Name(format!("settings-{key}-{id}").into()),
                label,
            )
            .style(if selected {
                ButtonStyle::Solid
            } else {
                ButtonStyle::Ghost
            })
            .color(if selected {
                colors.icon_accent
            } else {
                colors.text
            })
            .on_click(move |_, _, cx| {
                entity.update(cx, |settings_window, cx| action(settings_window, cx));
            }),
        );
    }
    row.into_any_element()
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
