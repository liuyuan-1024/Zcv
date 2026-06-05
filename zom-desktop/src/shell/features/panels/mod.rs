//! Panel 功能身份与元数据。
//!
//! 这里维护 desktop 当前固定 panel 列表。具体 UI 仍在各功能目录内，`PanelId`
//! 负责把命令、布局与功能模块连接起来。承载小件（焦点宿主、占位面板）属于
//! workbench 的 panel 框架，见 `workbench::docks`。

use gpui::{AnyElement, Context, FocusHandle, IntoElement, Window};

use crate::editor::TextEditorSlot;
use crate::shell::{CommandCatalogLookup, CommandTitleLookup, KeyRequest, ShortcutLookup};
use crate::ui_id::PanelId;

pub(crate) mod debug;
pub(crate) mod file_tree;
pub(crate) mod keyboard_shortcuts;
pub(crate) mod outline;
pub(crate) mod search;
pub(crate) mod terminal;
pub(crate) mod version_control;

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

    pub(crate) fn search_runtime_handle(&self) -> search::SearchRuntimeHandle {
        self.search.runtime_handle()
    }

    /// SearchModel 的 [`TextTargetOwner`] 句柄——由 ShellRuntime 走通用 owner 注册路径装进 router。
    ///
    /// [`TextTargetOwner`]: crate::text_target::TextTargetOwner
    pub(crate) fn search_owner_handle(
        &self,
    ) -> std::rc::Rc<std::cell::RefCell<dyn crate::text_target::TextTargetOwner>> {
        self.search.owner_handle()
    }

    pub(crate) fn search_state(&self, workspace: &zom_workspace::Workspace) -> search::SearchState {
        self.search.runtime_handle().state(workspace)
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
