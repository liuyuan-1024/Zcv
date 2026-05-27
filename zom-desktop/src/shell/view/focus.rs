//! Shell 焦点投影。
//!
//! App 持有唯一语义焦点 [`AppFocus`]；GPUI 的 [`FocusHandle`] 只是它在
//! 窗口系统里的投影。本模块只做两件事：
//!
//! - App 请求焦点变化时，把 [`AppFocus`] 投影到对应 [`FocusHandle`]。
//! - GPUI 当前焦点变化时，把 [`FocusHandle`] 反查成一个粗粒度 [`AppFocus`]，
//!   再交给 App 根据运行态细化（例如文件树 pending 输入态）。
//!
//! 它不决定业务焦点，也不缓存当前焦点。

use gpui::{FocusHandle, Window};

use crate::focus::{
    AppFocus, FileTreeFocus, PanelFocus, ProjectPickerFocus, SearchField, SurfaceFocus,
};
use crate::shell::features::panels::file_tree::FileTreeRuntime;
use crate::shell::features::panels::{PanelId, PanelRuntimes, focus_panel_handle};

/// `AppFocus <-> FocusHandle` 的 shell-only 投影表。
#[derive(Clone, Default)]
pub(crate) struct FocusProjection {
    entries: Vec<(FocusHandle, AppFocus)>,
}

impl FocusProjection {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn register(&mut self, handle: FocusHandle, focus: AppFocus) {
        self.entries.push((handle, focus));
    }

    /// 当前 GPUI 焦点投影出的语义焦点。返回值是 shell 能从 handle 看出的
    /// 粗粒度焦点；App 会在 `request_focus_from_shell` 里按自身运行态细化。
    pub(crate) fn current_focus(&self, window: &Window) -> AppFocus {
        self.entries
            .iter()
            .find(|(handle, _)| handle.is_focused(window))
            .map(|(_, focus)| *focus)
            .unwrap_or(AppFocus::None)
    }

    /// 把语义焦点投影到 GPUI。某些语义 leaf 共用一个宿主 handle，例如文件树
    /// Navigate / NewEntryName / ConfirmDelete。
    pub(crate) fn apply(&self, focus: AppFocus, window: &mut Window) {
        let Some(handle) = self.handle_for(focus) else {
            return;
        };
        window.focus(&handle);
    }

    pub(crate) fn apply_panel(&self, panel: PanelId, window: &mut Window) {
        if let Some(handle) = self.handle_for(panel_default_focus(panel)) {
            // panel 可能刚被 show_panel 显示，本帧尚未布局，下一帧再聚一次。
            focus_panel_handle(handle, window, true);
        }
    }

    pub(crate) fn is_at(&self, focus: AppFocus, window: &Window) -> bool {
        self.handle_for(focus)
            .is_some_and(|handle| handle.is_focused(window))
    }

    pub(crate) fn is_at_panel(&self, panel: PanelId, window: &Window) -> bool {
        match panel {
            PanelId::FileTree => [
                AppFocus::file_tree(FileTreeFocus::Navigate),
                AppFocus::file_tree(FileTreeFocus::NewEntryName),
                AppFocus::file_tree(FileTreeFocus::ConfirmDelete),
            ]
            .into_iter()
            .any(|focus| self.is_at(focus, window)),
            PanelId::Search => [
                AppFocus::search(SearchField::Query),
                AppFocus::search(SearchField::Replacement),
            ]
            .into_iter()
            .any(|focus| self.is_at(focus, window)),
            other => self.is_at(panel_default_focus(other), window),
        }
    }

    fn handle_for(&self, focus: AppFocus) -> Option<FocusHandle> {
        self.entries
            .iter()
            .find(|(_, registered)| registered.same_projection(focus))
            .map(|(handle, _)| handle.clone())
    }
}

pub(crate) fn panel_default_focus(panel: PanelId) -> AppFocus {
    match panel {
        PanelId::FileTree => AppFocus::file_tree(FileTreeFocus::Navigate),
        PanelId::VersionControl => AppFocus::Panel(PanelFocus::VersionControl),
        PanelId::Outline => AppFocus::Panel(PanelFocus::Outline),
        PanelId::Search => AppFocus::search(SearchField::Query),
        PanelId::Terminal => AppFocus::Panel(PanelFocus::Terminal),
        PanelId::Debug => AppFocus::Panel(PanelFocus::Debug),
        PanelId::KeyboardShortcuts => AppFocus::Panel(PanelFocus::KeyboardShortcuts),
    }
}

pub(crate) fn projection_from_runtimes(
    editor: FocusHandle,
    panel_runtimes: &PanelRuntimes,
    file_tree: &FileTreeRuntime,
    project_picker: FocusHandle,
) -> FocusProjection {
    let mut projection = FocusProjection::new();
    projection.register(editor, AppFocus::editor());
    projection.register(
        file_tree.focus_handle(),
        AppFocus::file_tree(FileTreeFocus::Navigate),
    );
    projection.register(
        panel_runtimes.search_query_focus_handle(),
        AppFocus::search(SearchField::Query),
    );
    projection.register(
        panel_runtimes.search_replacement_focus_handle(),
        AppFocus::search(SearchField::Replacement),
    );
    projection.register(
        project_picker,
        AppFocus::Surface(SurfaceFocus::ProjectPicker(ProjectPickerFocus::Query)),
    );
    for panel in [
        PanelId::VersionControl,
        PanelId::Outline,
        PanelId::Terminal,
        PanelId::Debug,
        PanelId::KeyboardShortcuts,
    ] {
        if let Some(handle) = panel_runtimes.focus_handle(panel) {
            projection.register(handle, panel_default_focus(panel));
        }
    }
    projection
}
