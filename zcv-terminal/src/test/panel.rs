//! 终端面板行为测试：项目工作目录与终端启动失败。

use std::{path::Path, sync::Arc};

use gpui::{AppContext as _, TestAppContext};
use zcv_fs_watch::{FsEventStream, FsWatcher, Watcher};
use zcv_language::LanguageRegistry;
use zcv_project::Project;
use zcv_workspace::{DockPosition, Workspace};

use super::{TerminalPanel, project_terminal_cwd};
use crate::{TerminalBuilder, TerminalView, test::support::display_only_terminal};
use zcv_settings::{SettingsStore, UserSettings};

/// 测试项目不注册真实文件系统监听，避免 OS 监听线程向 GPUI 测试调度器投递事件。
struct PassiveWatcher(FsWatcher);

impl PassiveWatcher {
    fn new() -> Self {
        Self(FsWatcher::new())
    }
}

impl Watcher for PassiveWatcher {
    fn add(&self, _path: &Path) -> anyhow::Result<()> {
        Ok(())
    }

    fn remove(&self, _path: &Path) -> anyhow::Result<()> {
        Ok(())
    }

    fn watch(&self, latency: std::time::Duration) -> FsEventStream {
        self.0.watch(latency)
    }
}

#[gpui::test]
fn new_terminal_uses_the_project_root_as_working_directory(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let temporary_directory = tempfile::tempdir().expect("应创建临时项目目录");
    let expected_root = temporary_directory
        .path()
        .canonicalize()
        .expect("临时项目根目录应可规范化");
    let project = cx.new(|cx| {
        Project::new_with_watcher(
            temporary_directory.path().to_path_buf(),
            Arc::new(PassiveWatcher::new()),
            Arc::new(LanguageRegistry::new()),
            cx,
        )
        .expect("测试项目根目录应可规范化")
    });
    let cwd = cx.read_entity(&project, |project, _| project_terminal_cwd(project));
    assert_eq!(
        cwd.as_deref(),
        Some(expected_root.as_path()),
        "新终端的工作目录应为所属项目的根"
    );
}

#[gpui::test]
fn terminal_creation_failure_keeps_panel_empty(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let temporary_directory = tempfile::tempdir().expect("应创建临时项目目录");
    let project = cx.new(|cx| {
        Project::new_with_watcher(
            temporary_directory.path().to_path_buf(),
            Arc::new(PassiveWatcher::new()),
            Arc::new(LanguageRegistry::new()),
            cx,
        )
        .expect("测试项目根目录应可规范化")
    });
    let (workspace, cx) = cx.add_window_view({
        let project = project.clone();
        move |window, cx| Workspace::new_with_project(project, window, cx)
    });
    let panel =
        cx.update(|_, cx| cx.new(|cx| TerminalPanel::new(project, workspace.downgrade(), cx)));
    std::fs::remove_dir(temporary_directory.path()).expect("应移除已创建的项目目录");

    cx.update(|window, cx| {
        assert!(!panel.update(cx, |panel, cx| panel.new_terminal(window, cx)));
        assert!(panel.read(cx).pane.read(cx).tabs().is_empty());
    });
}

#[gpui::test]
fn opening_hidden_terminal_panel_focuses_the_active_terminal(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let temporary_directory = tempfile::tempdir().expect("应创建临时项目目录");
    let project = cx.new(|cx| {
        Project::new_with_watcher(
            temporary_directory.path().to_path_buf(),
            Arc::new(PassiveWatcher::new()),
            Arc::new(LanguageRegistry::new()),
            cx,
        )
        .expect("测试项目根目录应可规范化")
    });
    let (workspace, cx) = cx.add_window_view({
        let project = project.clone();
        move |window, cx| Workspace::new_with_project(project, window, cx)
    });
    let panel = cx.new(|cx| {
        let workspace_handle = workspace.downgrade();
        TerminalPanel::new(project, workspace_handle, cx)
    });
    let terminal = cx.new(|cx| display_only_terminal(&TerminalBuilder::new(), cx));
    let view = cx.new(|cx| TerminalView::new(terminal.clone(), cx));
    let terminal_focus = cx.update(|_, cx| view.read(cx).focus_handle());

    workspace.update_in(cx, |workspace, window, cx| {
        panel.update(cx, |panel, cx| {
            panel.pane.update(cx, |pane, cx| {
                pane.open_item(Box::new(view), false, window, cx)
            });
        });
        workspace.register_panel(panel, DockPosition::Bottom, window, cx);
        assert!(workspace.open_panel("terminal", window, cx));
    });

    cx.update(|window, _| assert!(terminal_focus.is_focused(window)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, _| {
        assert!(
            terminal_focus.is_focused(window),
            "隐藏面板重新显示后应聚焦当前终端"
        );
    });
    assert!(cx.read_entity(&terminal, |terminal, _| terminal.term.lock().is_focused));
}
