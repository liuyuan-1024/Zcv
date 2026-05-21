//! L3 功能切片 —— UI、元数据与局部行为放在同一个功能目录。
//!
//! 第一版骨架里多数功能还是 panel 占位，但目录已经按功能归拢：接入真实数据源时，
//! 在各自目录内补充 `view.rs` / `state.rs` / `actions.rs` 等文件。
//!
//! Dock 归属由对应 dock 模块声明，本目录只提供功能本身。

use gpui::{AnyElement, Context, FocusHandle, IntoElement};

use crate::shell::KeyRequest;

pub(crate) mod debug;
pub(crate) mod diagnostics;
pub(crate) mod file_tree;
pub(crate) mod keyboard_shortcuts;
pub(crate) mod language_servers;
pub(crate) mod outline;
mod panel;
pub(crate) mod project_picker;
pub(crate) mod project_search;
pub(crate) mod settings;
pub(crate) mod terminal;
pub(crate) mod version_control;

pub(crate) use panel::{PanelId, focus_panel_handle};

#[derive(Clone)]
pub(crate) struct PanelRuntimes {
    version_control: version_control::VersionControlRuntime,
    outline: outline::OutlineRuntime,
    project_search: project_search::ProjectSearchRuntime,
    terminal: terminal::TerminalRuntime,
    debug: debug::DebugRuntime,
    keyboard_shortcuts: keyboard_shortcuts::KeyboardShortcutsRuntime,
}

impl PanelRuntimes {
    pub(crate) fn new<T>(cx: &mut Context<T>) -> Self {
        Self {
            version_control: version_control::VersionControlRuntime::new(cx),
            outline: outline::OutlineRuntime::new(cx),
            project_search: project_search::ProjectSearchRuntime::new(cx),
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
            PanelId::ProjectSearch => Some(self.project_search.focus_handle()),
            PanelId::Terminal => Some(self.terminal.focus_handle()),
            PanelId::Debug => Some(self.debug.focus_handle()),
            PanelId::KeyboardShortcuts => Some(self.keyboard_shortcuts.focus_handle()),
        }
    }

    pub(crate) fn render(&self, panel: PanelId, key_request: &KeyRequest) -> Option<AnyElement> {
        match panel {
            PanelId::FileTree => None,
            PanelId::VersionControl => {
                Some(self.version_control.render(key_request).into_any_element())
            }
            PanelId::Outline => Some(self.outline.render(key_request).into_any_element()),
            PanelId::ProjectSearch => {
                Some(self.project_search.render(key_request).into_any_element())
            }
            PanelId::Terminal => Some(self.terminal.render(key_request).into_any_element()),
            PanelId::Debug => Some(self.debug.render(key_request).into_any_element()),
            PanelId::KeyboardShortcuts => Some(
                self.keyboard_shortcuts
                    .render(key_request)
                    .into_any_element(),
            ),
        }
    }
}
