//! Panel 功能身份与元数据。
//!
//! 这里维护 desktop 当前固定 panel 列表。具体 UI 仍在各功能目录内，`PanelId`
//! 负责把命令、布局与功能模块连接起来。承载小件（焦点宿主、占位面板）属于
//! workbench 的 panel 框架，见 `workbench::docks`。

use gpui::{AnyElement, Context, FocusHandle, IntoElement, Window};

use crate::shell::editor::TextEditorSlot;
use crate::shell::{CommandCatalogLookup, CommandTitleLookup, KeyRequest, ShortcutLookup};

pub(crate) mod debug;
pub(crate) mod file_tree;
pub(crate) mod keyboard_shortcuts;
pub(crate) mod outline;
pub(crate) mod search;
pub(crate) mod terminal;
pub(crate) mod version_control;

/// 桌面端当前固定的 panel 列表（手册 20.10）。
///
/// 不抽 `PanelProvider` trait（手册 20.2）；新增 panel 直接在此 enum 加变体。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum PanelId {
    FileTree,
    VersionControl,
    Outline,
    Search,
    Terminal,
    Debug,
    KeyboardShortcuts,
}

impl PanelId {
    /// 该 panel 在 bar 上代表的图标资源路径（embedded assets 里的相对路径）。
    pub(crate) fn icon_path(self) -> &'static str {
        match self {
            PanelId::FileTree => "icons/panels/file_tree.svg",
            PanelId::VersionControl => "icons/panels/version_control.svg",
            PanelId::Outline => "icons/panels/outline.svg",
            PanelId::Search => "icons/panels/search.svg",
            PanelId::Terminal => "icons/panels/terminal.svg",
            PanelId::Debug => "icons/panels/debug.svg",
            PanelId::KeyboardShortcuts => "icons/panels/keyboard_shortcuts.svg",
        }
    }

    /// 切换本 panel 显隐的完整命令 id。常量本体在各自
    /// `zom_command::commands::<feature>` 模块，这里只做枚举 → 常量 的映射，
    /// 供 bar glyph 等 UI 标注。
    pub(crate) fn toggle_command_id(self) -> &'static str {
        use zom_command::commands::{
            debug, file_tree, keyboard_shortcuts, outline, search, terminal, version_control,
        };
        match self {
            PanelId::FileTree => file_tree::TOGGLE_PANEL,
            PanelId::VersionControl => version_control::TOGGLE_PANEL,
            PanelId::Outline => outline::TOGGLE_PANEL,
            PanelId::Search => search::TOGGLE_PANEL,
            PanelId::Terminal => terminal::TOGGLE_PANEL,
            PanelId::Debug => debug::TOGGLE_PANEL,
            PanelId::KeyboardShortcuts => keyboard_shortcuts::TOGGLE_PANEL,
        }
    }

    /// 短字符串 id —— 与 [`zom_command::HostEffect::TogglePanel`] 里
    /// String 字段对应。专门给 effect ↔ enum 之间架桥用。
    pub(crate) fn command_str_id(self) -> &'static str {
        match self {
            PanelId::FileTree => "file_tree",
            PanelId::VersionControl => "version_control",
            PanelId::Outline => "outline",
            PanelId::Search => "search",
            PanelId::Terminal => "terminal",
            PanelId::Debug => "debug",
            PanelId::KeyboardShortcuts => "keyboard_shortcuts",
        }
    }

    /// 反向解析：把 `HostEffect::TogglePanel(s)` 里的字符串还原成枚举。
    pub(crate) fn from_command_str_id(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|panel| panel.command_str_id() == value)
    }

    /// 枚举全部 panel —— 命令注册和 bar 渲染共用。
    pub(crate) const ALL: &'static [PanelId] = &[
        PanelId::FileTree,
        PanelId::VersionControl,
        PanelId::Outline,
        PanelId::Search,
        PanelId::Terminal,
        PanelId::Debug,
        PanelId::KeyboardShortcuts,
    ];
}

pub(crate) fn focus_panel_handle(focus: FocusHandle, window: &mut Window, on_next_frame: bool) {
    window.focus(&focus);
    if on_next_frame {
        window.on_next_frame(move |window, _| {
            window.focus(&focus);
        });
    }
}

#[derive(Clone)]
pub(crate) struct PanelRuntimes {
    version_control: version_control::VersionControlRuntime,
    outline: outline::OutlineRuntime,
    search: search::SearchRuntime,
    terminal: terminal::TerminalRuntime,
    debug: debug::DebugRuntime,
    keyboard_shortcuts: keyboard_shortcuts::KeyboardShortcutsRuntime,
}

impl PanelRuntimes {
    pub(crate) fn new<T>(cx: &mut Context<T>) -> Self {
        Self {
            version_control: version_control::VersionControlRuntime::new(cx),
            outline: outline::OutlineRuntime::new(cx),
            search: search::SearchRuntime::new(cx),
            terminal: terminal::TerminalRuntime::new(cx),
            debug: debug::DebugRuntime::new(cx),
            keyboard_shortcuts: keyboard_shortcuts::KeyboardShortcutsRuntime::new(cx),
        }
    }

    pub(crate) fn focus_handle(&self, panel: PanelId) -> Option<FocusHandle> {
        match panel {
            PanelId::FileTree => None,
            PanelId::VersionControl => Some(self.version_control.focus_handle()),
            PanelId::Outline => Some(self.outline.focus_handle()),
            PanelId::Search => Some(self.search.focus_handle()),
            PanelId::Terminal => Some(self.terminal.focus_handle()),
            PanelId::Debug => Some(self.debug.focus_handle()),
            PanelId::KeyboardShortcuts => Some(self.keyboard_shortcuts.focus_handle()),
        }
    }

    pub(crate) fn render(
        &self,
        panel: PanelId,
        key_request: &KeyRequest,
        search_state: &search::SearchState,
        search_query_slot: &std::rc::Rc<TextEditorSlot>,
        search_replacement_slot: &std::rc::Rc<TextEditorSlot>,
        shortcuts: &ShortcutLookup,
        titles: &CommandTitleLookup,
        command_catalog: &CommandCatalogLookup,
    ) -> Option<AnyElement> {
        match panel {
            PanelId::FileTree => None,
            PanelId::VersionControl => Some(
                self.version_control
                    .render(key_request, titles)
                    .into_any_element(),
            ),
            PanelId::Outline => Some(self.outline.render(key_request, titles).into_any_element()),
            PanelId::Search => Some(
                self.search
                    .render(
                        search_state,
                        key_request,
                        search_query_slot,
                        search_replacement_slot,
                        shortcuts,
                        titles,
                    )
                    .into_any_element(),
            ),
            PanelId::Terminal => Some(self.terminal.render(key_request, titles).into_any_element()),
            PanelId::Debug => Some(self.debug.render(key_request, titles).into_any_element()),
            PanelId::KeyboardShortcuts => Some(
                self.keyboard_shortcuts
                    .render(key_request, shortcuts, command_catalog)
                    .into_any_element(),
            ),
        }
    }

    pub(crate) fn search_query_focus_handle(&self) -> FocusHandle {
        self.search.query_focus_handle()
    }

    pub(crate) fn search_replacement_focus_handle(&self) -> FocusHandle {
        self.search.replacement_focus_handle()
    }

    pub(crate) fn install_listeners<T: 'static>(
        &self,
        app: std::rc::Rc<std::cell::RefCell<crate::app::App>>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        self.search.install_listeners(app, window, cx);
    }
}
