//! 终端面板行为测试：启动、标签与焦点。

use std::{path::Path, sync::Arc};

use gpui::{AppContext as _, TestAppContext};
use zcv_actions::CloseTab;
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
            vec![temporary_directory.path().to_path_buf()],
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
            vec![temporary_directory.path().to_path_buf()],
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
            vec![temporary_directory.path().to_path_buf()],
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

#[gpui::test]
fn closing_terminal_tabs_activates_the_remaining_tab_then_hides_the_panel(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let project = cx.new(|cx| {
        Project::empty_with_watcher(
            Arc::new(LanguageRegistry::new()),
            Arc::new(PassiveWatcher::new()),
            cx,
        )
    });
    let (workspace, cx) = cx.add_window_view({
        let project = project.clone();
        move |window, cx| Workspace::new_with_project(project, window, cx)
    });
    let panel = cx.new(|cx| TerminalPanel::new(project, workspace.downgrade(), cx));
    let pane = cx.read_entity(&panel, |panel, _| panel.pane.clone());
    let [first, second] = std::array::from_fn(|_| {
        let terminal = cx.new(|cx| display_only_terminal(&TerminalBuilder::new(), cx));
        cx.new(|cx| TerminalView::new(terminal, cx))
    });
    let first_id = first.entity_id();

    workspace.update_in(cx, |workspace, window, cx| {
        pane.update(cx, |pane, cx| {
            pane.open_item(Box::new(first), false, window, cx);
            pane.open_item(Box::new(second), false, window, cx);
        });
        workspace.register_panel(panel, DockPosition::Bottom, window, cx);
        assert!(workspace.open_panel("terminal", window, cx));
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    cx.dispatch_action(CloseTab);
    cx.run_until_parked();

    cx.update(|window, cx| {
        let pane = pane.read(cx);
        assert_eq!(pane.tabs().len(), 1);
        let active = pane.active_item().expect("应激活剩余终端");
        assert_eq!(active.item_id(), first_id);
        assert!(active.item_focus_handle(cx).is_focused(window));
        assert!(
            workspace
                .read(cx)
                .dock(DockPosition::Bottom)
                .read(cx)
                .is_open()
        );
    });

    cx.dispatch_action(CloseTab);
    cx.run_until_parked();

    assert!(cx.read_entity(&pane, |pane, _| pane.tabs().is_empty()));
    assert!(cx.read_entity(&workspace, |workspace, cx| {
        !workspace.dock(DockPosition::Bottom).read(cx).is_open()
    }));
}
