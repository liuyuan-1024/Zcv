//! BottomBar —— 窗口级底部外壳（Entity）。
//!
//! action handler 通过 GPUI 全局状态 `LayoutRef` 访问布局控制器。

use gpui::{
    AnyElement, App, Context, Div, Entity, Subscription, WeakEntity, Window, actions, div,
    prelude::*,
};

use crate::editor::editor::Editor;
use crate::theme::color;
use crate::ui::glyph::Glyph;
use crate::workbench::dock::LayoutRef;
use crate::workbench::dock::PanelId;
use crate::workbench::pane::Pane;

actions!(
    bottom_bar,
    [
        ToggleProjectTree,
        ToggleVersionControl,
        ToggleOutline,
        ToggleLanguageServer,
        ToggleDiagnostics,
        ToggleProjectSearch,
        ToggleTerminal,
        ToggleDebug,
        ToggleKeyboardShortcuts,
    ]
);

// ── BottomBar Entity ───────────────────────────────────────────────

pub(crate) struct BottomBar {
    cursor_text: String,
    language: String,
    file_path: Option<std::path::PathBuf>,
    active_editor: Option<WeakEntity<Editor>>,
    _pane_subscription: Option<Subscription>,
    _editor_subscription: Option<Subscription>,
}

impl BottomBar {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let mut bar = Self {
            cursor_text: String::new(),
            language: String::new(),
            file_path: None,
            active_editor: None,
            _pane_subscription: None,
            _editor_subscription: None,
        };
        bar.follow_active_editor(cx);
        bar
    }

    /// 通过 LayoutRef 找到焦点 Pane 和活跃 Editor，订阅变化。
    fn follow_active_editor(&mut self, cx: &mut Context<Self>) {
        let pane = cx
            .try_global::<LayoutRef>()
            .and_then(|r| r.0.upgrade())
            .and_then(|ctrl| ctrl.borrow().focus_pane.clone());

        // 订阅 Pane 切换 tab
        self._pane_subscription = pane.as_ref().map(|pane| {
            cx.observe(pane, |this, pane, cx| {
                this.on_pane_changed(pane, cx);
            })
        });

        if let Some(pane) = &pane {
            self.on_pane_changed(pane.clone(), cx);
        }
    }

    fn on_pane_changed(&mut self, pane: Entity<Pane>, cx: &mut Context<Self>) {
        let (editor, path) = {
            let pane = pane.read(cx);
            (pane.active_editor(), pane.active_file().map(|(_, p)| p))
        };
        self.file_path = path;
        self.watch_editor(editor.as_ref(), cx);
    }

    fn watch_editor(&mut self, editor: Option<&Entity<Editor>>, cx: &mut Context<Self>) {
        // 取消旧订阅
        self._editor_subscription = None;
        self.active_editor = editor.map(|e| e.downgrade());

        // 订阅新 Editor 的变更（选区移动、编辑等都会触发 notify）
        if let Some(editor) = editor {
            self._editor_subscription = Some(cx.observe(editor, |this, editor, cx| {
                this.sync_from_editor(Some(&editor), cx);
            }));
        }

        self.sync_from_editor(editor, cx);
    }

    fn sync_from_editor(&mut self, editor: Option<&Entity<Editor>>, cx: &mut Context<Self>) {
        match editor {
            Some(editor) => {
                let editor_ref = editor.read(cx);
                self.cursor_text = editor_ref.cursor_text();

                // 语言检测：优先用文件路径，path_suffixes 未命中时读 Buffer 首行查 shebang
                self.language = self
                    .file_path
                    .as_deref()
                    .and_then(|path| {
                        let first_line = editor_ref
                            .render_snapshot()
                            .slice_byte_range(
                                zcv_engine::ByteOffset::ZERO,
                                zcv_engine::ByteOffset::new(256.min(
                                    editor_ref.render_snapshot().len_bytes().get(),
                                )),
                            )
                            .ok()
                            .map(|s| {
                                s.as_str()
                                    .lines()
                                    .next()
                                    .unwrap_or("")
                                    .to_owned()
                            });
                        crate::available_languages::language_for_file(
                            path,
                            first_line.as_deref(),
                        )
                    })
                    .map(|name| name.to_owned())
                    .unwrap_or_default();
            }
            None => {
                self.cursor_text = String::new();
                self.language = String::new();
            }
        }
        cx.notify();
    }
}

fn bar_frame() -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .px(space::S8)
        .py(space::S6)
        .gap(space::S6)
        .bg(color::current().gray.s[2])
        .text_color(color::current().gray.s[8])
        .border_t_1()
        .border_color(color::current().gray.s[4])
}

fn bar_divider() -> Div {
    div()
        .w(gpui::px(1.0))
        .h_full()
        .bg(color::current().gray.s[4])
}

use crate::theme::space;

impl gpui::Render for BottomBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let is_active = |panel: PanelId| -> bool {
            cx.try_global::<LayoutRef>()
                .and_then(|r| r.0.upgrade())
                .map(|ctrl| ctrl.borrow().is_panel_active(panel))
                .unwrap_or(false)
        };
        bar_frame()
            .id("bottom-bar")
            .child(leading_region(&is_active, cx))
            .child(trailing_region(
                &is_active,
                cx,
                &self.cursor_text,
                &self.language,
            ))
    }
}

fn region(items: Vec<AnyElement>, justify_start: bool) -> Div {
    let wrapper = div().flex_1().flex().items_center().gap(space::S8);
    let wrapper = if justify_start {
        wrapper.justify_start()
    } else {
        wrapper.justify_end()
    };
    wrapper.children(items)
}

/// dispatch_action 封装：GPUI 要求 action 装箱。
macro_rules! dispatch {
    ($window:expr, $action:expr, $cx:expr) => {
        $window.dispatch_action(Box::new($action), $cx)
    };
}

fn leading_region(is_active: &dyn Fn(PanelId) -> bool, cx: &App) -> Div {
    region(leading_slots(is_active, cx), true)
}

fn trailing_region(
    is_active: &dyn Fn(PanelId) -> bool,
    cx: &App,
    cursor_text: &str,
    language: &str,
) -> Div {
    region(trailing_slots(is_active, cx, cursor_text, language), false)
}

fn leading_slots(is_active: &dyn Fn(PanelId) -> bool, cx: &App) -> Vec<AnyElement> {
    join_groups(vec![
        vec![
            Glyph::icon("bottom-bar.project-tree", "icons/panels/project_tree.svg")
                .label("项目树")
                .shortcut(&ToggleProjectTree, cx)
                .color(if is_active(PanelId::ProjectTree) {
                    color::highlight()
                } else {
                    color::default()
                })
                .on_click(|window, cx| dispatch!(window, ToggleProjectTree, cx))
                .into_any_element(),
            Glyph::icon(
                "bottom-bar.version-control",
                "icons/panels/version_control.svg",
            )
            .label("版本控制")
            .shortcut(&ToggleVersionControl, cx)
            .color(if is_active(PanelId::VersionControl) {
                color::highlight()
            } else {
                color::default()
            })
            .on_click(|window, cx| dispatch!(window, ToggleVersionControl, cx))
            .into_any_element(),
            Glyph::icon("bottom-bar.outline", "icons/panels/outline.svg")
                .label("大纲")
                .shortcut(&ToggleOutline, cx)
                .color(if is_active(PanelId::Outline) {
                    color::highlight()
                } else {
                    color::default()
                })
                .on_click(|window, cx| dispatch!(window, ToggleOutline, cx))
                .into_any_element(),
        ],
        vec![
            Glyph::icon(
                "bottom-bar.language-server",
                "icons/status/language_server.svg",
            )
            .label("语言服务器")
            .shortcut(&ToggleLanguageServer, cx)
            .on_click(|window, cx| dispatch!(window, ToggleLanguageServer, cx))
            .into_any_element(),
            Glyph::icon_text(
                "bottom-bar.diagnostics",
                "icons/status/diagnostics.svg",
                "0",
            )
            .label("诊断")
            .shortcut(&ToggleDiagnostics, cx)
            .on_click(|window, cx| dispatch!(window, ToggleDiagnostics, cx))
            .into_any_element(),
            Glyph::icon("bottom-bar.project-search", "icons/panels/search.svg")
                .label("项目搜索")
                .shortcut(&ToggleProjectSearch, cx)
                .on_click(|window, cx| dispatch!(window, ToggleProjectSearch, cx))
                .into_any_element(),
        ],
    ])
}

fn trailing_slots(
    is_active: &dyn Fn(PanelId) -> bool,
    cx: &App,
    cursor_text: &str,
    language: &str,
) -> Vec<AnyElement> {
    join_groups(vec![
        vec![
            Glyph::text("bottom-bar.cursor", cursor_text.to_owned())
                .label("光标位置")
                .into_any_element(),
            Glyph::text("bottom-bar.language", language.to_owned())
                .label("语言")
                .into_any_element(),
        ],
        vec![
            Glyph::icon("bottom-bar.terminal", "icons/panels/terminal.svg")
                .label("终端")
                .shortcut(&ToggleTerminal, cx)
                .color(if is_active(PanelId::Terminal) {
                    color::highlight()
                } else {
                    color::default()
                })
                .on_click(|window, cx| dispatch!(window, ToggleTerminal, cx))
                .into_any_element(),
            Glyph::icon("bottom-bar.debug", "icons/panels/debug.svg")
                .label("调试")
                .shortcut(&ToggleDebug, cx)
                .color(if is_active(PanelId::Debug) {
                    color::highlight()
                } else {
                    color::default()
                })
                .on_click(|window, cx| dispatch!(window, ToggleDebug, cx))
                .into_any_element(),
        ],
        vec![
            Glyph::icon(
                "bottom-bar.keyboard-shortcuts",
                "icons/panels/keyboard_shortcuts.svg",
            )
            .label("快捷键")
            .shortcut(&ToggleKeyboardShortcuts, cx)
            .color(if is_active(PanelId::KeyboardShortcuts) {
                color::highlight()
            } else {
                color::default()
            })
            .on_click(|window, cx| dispatch!(window, ToggleKeyboardShortcuts, cx))
            .into_any_element(),
        ],
    ])
}

fn join_groups(groups: Vec<Vec<AnyElement>>) -> Vec<AnyElement> {
    let mut out = Vec::new();
    for group in groups {
        if !out.is_empty() {
            out.push(bar_divider().into_any_element());
        }
        out.extend(group);
    }
    out
}
