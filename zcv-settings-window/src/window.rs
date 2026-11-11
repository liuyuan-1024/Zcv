//! 独立设置窗口：设置页面与内嵌 settings.json 编辑器。
//!
//! 状态所有权：窗口不保存可写设置副本。
//! 显示值来自 `SettingsStore` 的运行时快照，每个控件把一次字段修改交给 `zcv_settings::update_user_settings`；
//! 设置层负责读取 settings.json、解析、校验、序列化、落盘并更新 `SettingsStore`。
//! 手工编辑文件时由设置监听走同一 `SettingsStore::set_user_settings` 入口，窗口通过全局观察刷新，因此两条路径共享同一权威状态。

use std::{num::NonZeroUsize, sync::Arc};

use gpui::{
    AnyElement, App, Context, Div, ElementId, Entity, FocusHandle, Focusable, IntoElement, Render,
    ScrollHandle, SharedString, Subscription, Window, WindowControlArea, div, prelude::*, px,
};
use zcv_actions::Save;
use zcv_editor::{Editor, EditorEvent};
use zcv_language::{LanguageBuffer, LanguageRegistry};
use zcv_multi_buffer::MultiBuffer;
use zcv_settings::{
    CursorShape, IndentGuideSettingsContent, SettingsStore, SoftWrapMode, TerminalCursorShape,
    UserSettingsContent, update_user_settings,
};
use zcv_text::{Buffer, BufferConfig};
use zcv_theme::{ThemeChoice, color, scale, typography};
use zcv_ui::{Button, ButtonStyle, Checkbox, SvgIcon};
use zcv_workspace::window_controls::{self, WindowControls};

type WindowAction = Box<dyn Fn(&mut SettingsWindow, &mut Context<SettingsWindow>)>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsView {
    Page,
    File,
}

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
    languages: Arc<LanguageRegistry>,
    scroll_handle: ScrollHandle,
    settings_editor: Option<Entity<Editor>>,
    active_view: SettingsView,
    active_page: SettingsPage,
    error: Option<SharedString>,
    window_controls: WindowControls,
    /// 手工编辑 settings.json 后由设置 global 通知窗口重新渲染。
    _settings_subscription: Subscription,
    _appearance_subscription: Subscription,
    _editor_subscription: Option<Subscription>,
}

impl SettingsWindow {
    pub fn new(
        languages: Arc<LanguageRegistry>,
        show_file: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = SettingsStore::get(cx);
        typography::set_base_typography(
            cx,
            Some(settings.content_font_size),
            Some(settings.ui_font_size),
            Some(settings.content_line_height),
        );
        window.set_rem_size(typography::current(cx).ui_size());
        settings.theme.apply(cx, Some(window));

        let settings_subscription =
            cx.observe_global_in::<SettingsStore>(window, |_, window, cx| {
                let settings = SettingsStore::get(cx);
                typography::set_base_typography(
                    cx,
                    Some(settings.content_font_size),
                    Some(settings.ui_font_size),
                    Some(settings.content_line_height),
                );
                window.set_rem_size(typography::current(cx).ui_size());
                settings.theme.apply(cx, Some(window));
                cx.notify();
            });
        let appearance_subscription = window.observe_window_appearance(|window, cx| {
            SettingsStore::get(cx).theme.apply(cx, Some(window));
            window.refresh();
        });

        let mut settings_window = Self {
            focus: cx.focus_handle(),
            languages,
            scroll_handle: ScrollHandle::new(),
            settings_editor: None,
            active_view: SettingsView::Page,
            active_page: SettingsPage::Appearance,
            error: None,
            window_controls: WindowControls::new(),
            _settings_subscription: settings_subscription,
            _appearance_subscription: appearance_subscription,
            _editor_subscription: None,
        };
        if show_file {
            settings_window.open_settings_file(window, cx);
        }
        settings_window
    }

    pub fn show_view(&mut self, show_file: bool, window: &mut Window, cx: &mut Context<Self>) {
        if show_file {
            self.open_settings_file(window, cx);
        } else {
            self.show_settings_page(window, cx);
        }
    }

    fn update_setting(
        &mut self,
        cx: &mut Context<Self>,
        title: &'static str,
        change: impl FnOnce(&mut UserSettingsContent),
    ) {
        match update_user_settings(cx, change) {
            Ok(_) => {
                self.error = None;
                cx.notify();
            }
            Err(error) => {
                self.error = Some(format!("保存设置失败（{title}）：{error:#}").into());
                cx.notify();
            }
        }
    }

    fn open_settings_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = &self.settings_editor {
            self.active_view = SettingsView::File;
            let focus = editor.read(cx).focus_handle();
            window.focus(&focus, cx);
            cx.notify();
            return;
        }

        match self.create_settings_editor(window, cx) {
            Ok(editor) => {
                self._editor_subscription =
                    Some(
                        cx.subscribe_in(&editor, window, |_, _, event: &EditorEvent, _, cx| {
                            if matches!(event, EditorEvent::Edited { .. }) {
                                cx.notify();
                            }
                        }),
                    );
                self.settings_editor = Some(editor);
                self.active_view = SettingsView::File;
                self.error = None;
                cx.notify();
            }
            Err(error) => {
                self.error = Some(format!("打开设置文件失败：{error:#}").into());
                cx.notify();
            }
        }
    }

    fn create_settings_editor(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Entity<Editor>> {
        zcv_settings::ensure_user_settings_file()?;
        let path = zcv_settings::settings_file_path().to_path_buf();
        let text = zcv_settings::read_user_settings_text()?;
        let buffer = Buffer::from_text(text, BufferConfig::default())?;
        let languages = Arc::clone(&self.languages);
        let language_buffer = cx.new(|cx| LanguageBuffer::new(buffer, Some(path), languages, cx));
        let multi_buffer = cx.new(|cx| MultiBuffer::singleton(language_buffer, cx));
        let editor = cx.new(|cx| Editor::for_multi_buffer(multi_buffer, cx));
        let focus = editor.read(cx).focus_handle();
        window.focus(&focus, cx);
        Ok(editor)
    }

    fn show_settings_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.active_view = SettingsView::Page;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn set_active_page(&mut self, page: SettingsPage, cx: &mut Context<Self>) {
        self.active_page = page;
        self.active_view = SettingsView::Page;
        cx.notify();
    }

    fn save_settings_file(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.settings_editor.clone() else {
            return;
        };
        let text = editor.read(cx).text(cx);
        match zcv_settings::write_user_settings_text(&text) {
            Ok(()) => {
                let multi_buffer = editor.read(cx).multi_buffer();
                if let Some(source) = multi_buffer.read(cx).singleton_source() {
                    let version = source.read(cx).version();
                    source.update(cx, |source, cx| source.did_save(version, cx));
                }
                self.error = None;
                cx.notify();
            }
            Err(error) => {
                self.error = Some(format!("保存设置文件失败：{error:#}").into());
                cx.notify();
            }
        }
    }

    fn handle_save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_view == SettingsView::File {
            self.save_settings_file(window, cx);
        }
    }

    fn render_top_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let height = window_controls::top_bar_height(window, cx);
        self.window_controls.align(window, height);
        let colors = *color::current(cx);
        let settings_window = cx.entity();
        let mut actions: Vec<AnyElement> = Vec::new();

        match self.active_view {
            SettingsView::Page => {
                let settings_window = settings_window.clone();
                actions.push(
                    Button::icon(
                        ElementId::Name("settings-open-file".into()),
                        "icons/file_text_outlined.svg",
                    )
                    .label("打开设置文件")
                    .on_click(move |_, window, cx| {
                        settings_window.update(cx, |settings_window, cx| {
                            settings_window.open_settings_file(window, cx)
                        });
                    })
                    .into_any_element(),
                );
            }
            SettingsView::File => {
                let dirty = self
                    .settings_editor
                    .as_ref()
                    .is_some_and(|editor| editor.read(cx).is_dirty(cx));
                let save_button =
                    Button::icon(ElementId::Name("settings-save".into()), "icons/check.svg")
                        .label(if dirty {
                            "保存设置文件（有未保存修改）"
                        } else {
                            "保存设置文件"
                        })
                        .color(if dirty {
                            colors.icon_accent
                        } else {
                            colors.text_muted
                        })
                        .on_click(move |_, window, cx| {
                            settings_window.update(cx, |settings_window, cx| {
                                settings_window.save_settings_file(window, cx)
                            });
                        });
                actions.push(
                    div()
                        .relative()
                        .child(save_button)
                        .when(dirty, |element| {
                            element.child(
                                div()
                                    .absolute()
                                    .top(px(1.0))
                                    .right(px(1.0))
                                    .size(px(6.0))
                                    .rounded_full()
                                    .bg(colors.icon_accent),
                            )
                        })
                        .into_any_element(),
                );
            }
        }

        window_controls::top_bar_interaction(
            div()
                .id("settings-top_bar")
                .window_control_area(WindowControlArea::Drag)
                .relative()
                .flex()
                .flex_row()
                .items_center()
                .w_full()
                .h(height)
                .bg(colors.title_bar_background)
                .text_color(colors.text),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .flex_1()
                .min_w_0()
                .py(scale::S6)
                .pr(scale::S6)
                .pl(window_controls::leading_inset(window))
                .gap(scale::S6)
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("设置"))
                .child(div().flex_1())
                .children(actions),
        )
        .when_some(window_controls::trailing(window, cx), |bar, controls| {
            bar.child(controls)
        })
        .child(
            div()
                .absolute()
                .inset_0()
                .border_b_1()
                .border_color(colors.border),
        )
        .into_any_element()
    }

    fn render_body(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let content = if self.active_view == SettingsView::File {
            match &self.settings_editor {
                Some(editor) => div()
                    .flex_1()
                    .min_h_0()
                    .child(editor.clone())
                    .into_any_element(),
                None => div().flex_1().into_any_element(),
            }
        } else {
            self.render_settings_page(window, cx)
        };

        div()
            .flex()
            .flex_1()
            .min_h_0()
            .child(self.render_sidebar(cx))
            .child(content)
            .into_any_element()
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = *color::current(cx);
        let settings_window = cx.entity();
        let mut nav = div().flex().flex_col().gap(scale::S2).p(scale::S6);

        for page in SettingsPage::ALL {
            let selected = self.active_page == page && self.active_view == SettingsView::Page;
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

    fn render_settings_page(&self, _window: &Window, cx: &mut Context<Self>) -> AnyElement {
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

        let appearance = render_section(
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
        );

        let editor = render_section(
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
        );

        let terminal = render_section(
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
        );

        let override_note = div()
            .px(scale::S6)
            .text_size(ui_size * 0.85)
            .text_color(colors.text_muted)
            .child(format!(
                "settings.json 已显式覆盖 {} 项；未覆盖的字段使用内置默认值。",
                explicit_override_count(&overrides)
            ));

        let mut body = div()
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
        if let Some(error) = &self.error {
            body = body.child(
                div()
                    .px(scale::S8)
                    .py(scale::S6)
                    .rounded_sm()
                    .bg(colors.surface_background)
                    .border_1()
                    .border_color(colors.status_error)
                    .text_color(colors.status_error)
                    .child(error.clone()),
            );
        }
        let page = match self.active_page {
            SettingsPage::Appearance => appearance,
            SettingsPage::Editor => editor,
            SettingsPage::Terminal => terminal,
        };
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
            .on_action(cx.listener(Self::handle_save))
            .bg(colors.panel_background)
            .child(self.render_top_bar(window, cx))
            .child(self.render_body(window, cx))
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
        .rounded_sm()
        .hover(|style| style.bg(colors.element_hover))
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
mod tests {
    use super::*;

    #[test]
    fn stepped_values_stay_on_the_advertised_precision() {
        assert_eq!(stepped(1.5, 0.1, 1.0, 3.0), 1.6);
        assert_eq!(stepped(1.0, -0.1, 1.0, 3.0), 1.0);
        assert_eq!(stepped(32.0, 1.0, 10.0, 32.0), 32.0);
    }

    #[test]
    fn indent_guide_updates_preserve_other_explicit_fields() {
        let mut content = UserSettingsContent {
            indent_guides: Some(IndentGuideSettingsContent {
                enabled: Some(true),
                line_width: std::num::NonZeroU32::new(3),
            }),
            ..UserSettingsContent::default()
        };

        update_indent_guides(&mut content, None, Some(5));
        assert_eq!(
            content.indent_guides,
            Some(IndentGuideSettingsContent {
                enabled: Some(true),
                line_width: std::num::NonZeroU32::new(5),
            })
        );

        update_indent_guides(&mut content, Some(false), None);
        assert_eq!(
            content.indent_guides,
            Some(IndentGuideSettingsContent {
                enabled: Some(false),
                line_width: std::num::NonZeroU32::new(5),
            })
        );
    }

    #[test]
    fn override_count_counts_top_level_explicit_fields() {
        let content = UserSettingsContent {
            theme: Some(ThemeChoice::Named("dark")),
            content_font_size: Some(18.0),
            indent_guides: Some(IndentGuideSettingsContent::default()),
            ..UserSettingsContent::default()
        };
        assert_eq!(explicit_override_count(&content), 3);
        assert_eq!(explicit_override_count(&UserSettingsContent::default()), 0);
    }

    #[test]
    fn every_registered_theme_has_a_stable_window_label() {
        let labels: Vec<_> = ThemeChoice::available().map(theme_label).collect();
        assert_eq!(labels, ["跟随系统", "深色", "浅色"]);
    }
}
