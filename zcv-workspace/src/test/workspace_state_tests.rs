use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
};

use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Modifiers, Render, Task,
    TestAppContext, Window, div, prelude::*, px,
};

use super::{DockPosition, LAYOUT_SAVE_THROTTLE, Workspace};
use crate::{Item, Panel, dock::DockData, layout_state, panel::PanelEvent};
use zcv_actions::{FocusOrHidePanel, Save};
use zcv_language::LanguageRegistry;
use zcv_project::{FileSaveConflictPolicy, Project};
use zcv_settings::{SettingsStore, UserSettings};
use zcv_theme::typography;

struct ConflictItem {
    focus: FocusHandle,
    path: PathBuf,
    conflicted: Rc<Cell<bool>>,
    saves: Rc<RefCell<Vec<FileSaveConflictPolicy>>>,
    restores: Rc<Cell<usize>>,
}

impl EventEmitter<()> for ConflictItem {}

impl gpui::Focusable for ConflictItem {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ConflictItem {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl Item for ConflictItem {
    type Event = ();

    fn tab_content_text(&self, _cx: &App) -> gpui::SharedString {
        "冲突文件".into()
    }

    fn item_path(&self, _cx: &App) -> Option<PathBuf> {
        Some(self.path.clone())
    }

    fn can_save(&self, _cx: &App) -> bool {
        true
    }

    fn conflicted_file_paths(&self, _cx: &App) -> Vec<PathBuf> {
        self.conflicted
            .get()
            .then(|| self.path.clone())
            .into_iter()
            .collect()
    }

    fn save(
        &mut self,
        _project: Entity<Project>,
        conflict_policy: FileSaveConflictPolicy,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        self.saves.borrow_mut().push(conflict_policy);
        self.conflicted.set(false);
        Task::ready(Ok(()))
    }

    fn reload_conflicted_files(
        &mut self,
        _project: Entity<Project>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        self.restores.set(self.restores.get() + 1);
        self.conflicted.set(false);
        Task::ready(Ok(()))
    }
}

#[gpui::test]
fn lost_window_focus_returns_to_the_active_pane(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    struct FocusLossHost {
        workspace: Entity<Workspace>,
        temporary: FocusHandle,
        show_temporary: bool,
    }

    impl Render for FocusLossHost {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(self.workspace.clone())
                .when(self.show_temporary, |root| {
                    root.child(div().track_focus(&self.temporary))
                })
        }
    }

    let (host, cx) = cx.add_window_view(|window, cx| FocusLossHost {
        workspace: cx.new(|cx| Workspace::new_empty(test_languages(), window, cx)),
        temporary: cx.focus_handle(),
        show_temporary: true,
    });
    let (workspace, temporary) = cx.read_entity(&host, |host, _| {
        (host.workspace.clone(), host.temporary.clone())
    });
    let pane_focus = cx.read_entity(&workspace, |workspace, cx| {
        workspace.pane.read(cx).focus_handle()
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.focus(&temporary, cx));
    host.update_in(cx, |host, _, cx| {
        host.show_temporary = false;
        cx.notify();
    });
    cx.run_until_parked();
    cx.update(|window, _| assert!(pane_focus.is_focused(window)));
}

#[gpui::test]
fn saving_a_conflicted_item_shows_one_persistent_choice_toast(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let path = directory.path().join("conflict-item.txt");
    let (workspace, cx) =
        cx.add_window_view(|window, cx| Workspace::new_empty(test_languages(), window, cx));
    cx.update(|_, cx| cx.set_reduce_motion(true));
    let conflicted = Rc::new(Cell::new(true));
    let saves = Rc::new(RefCell::new(Vec::new()));
    let restores = Rc::new(Cell::new(0));
    let item = cx.new(|cx| ConflictItem {
        focus: cx.focus_handle(),
        path,
        conflicted: conflicted.clone(),
        saves: saves.clone(),
        restores: restores.clone(),
    });
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.pane.update(cx, |pane, cx| {
            pane.open_item(Box::new(item), false, window, cx);
        });
        workspace.handle_save(&Save, window, cx);
        workspace.handle_save(&Save, window, cx);
    });
    cx.read_entity(&workspace, |workspace, cx| {
        assert_eq!(workspace.conflict_toasts.len(), 1);
        let id = *workspace.conflict_toasts.values().next().unwrap();
        assert!(workspace.toast_layer.read(cx).contains(id));
    });
    assert!(saves.borrow().is_empty());
    let overwrite = cx.debug_bounds("toast-action-0-0").expect("覆盖按钮应显示");
    let restore = cx.debug_bounds("toast-action-0-1").expect("还原按钮应显示");
    assert!(overwrite.left() < restore.left());
    cx.simulate_click(overwrite.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(*saves.borrow(), vec![FileSaveConflictPolicy::Overwrite]);
    assert_eq!(restores.get(), 0);
    assert!(!conflicted.get());

    conflicted.set(true);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.handle_save(&Save, window, cx);
    });
    let restore = cx
        .debug_bounds("toast-action-1-1")
        .expect("还原按钮应再次显示");
    cx.simulate_click(restore.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(restores.get(), 1);
    assert!(!conflicted.get());
    assert_eq!(*saves.borrow(), vec![FileSaveConflictPolicy::Overwrite]);
}

/// 测试用语言注册表；生产装配层创建应用级唯一实例，测试各自提供一份。
fn test_languages() -> Arc<LanguageRegistry> {
    Arc::new(LanguageRegistry::new())
}

struct TestPanel {
    focus: FocusHandle,
    activation_focus: FocusHandle,
}

impl EventEmitter<PanelEvent> for TestPanel {}

impl Panel for TestPanel {
    fn icon() -> &'static str {
        "icons/list_tree.svg"
    }

    fn label() -> &'static str {
        "测试面板"
    }

    fn persistent_name() -> &'static str {
        "test-panel"
    }

    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }

    fn activation_focus_handle(&self, _: &App) -> FocusHandle {
        self.activation_focus.clone()
    }
}

impl Render for TestPanel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus)
            .child(div().track_focus(&self.activation_focus))
    }
}

#[gpui::test]
fn empty_workspace_has_a_project_without_a_worktree(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let (workspace, cx) =
        cx.add_window_view(|window, cx| Workspace::new_empty(test_languages(), window, cx));
    let project = cx.read_entity(&workspace, |workspace, _| workspace.project().clone());
    assert!(!cx.read_entity(&project, |project, _| project.has_worktree()));
}

#[gpui::test]
fn typography_override_belongs_to_one_workspace(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let (first, cx) =
        cx.add_window_view(|window, cx| Workspace::new_empty(test_languages(), window, cx));
    let (second, cx) =
        cx.add_window_view(|window, cx| Workspace::new_empty(test_languages(), window, cx));
    let original = cx.read_entity(&second, |workspace, _| {
        f32::from(workspace.typography().content_size())
    });

    first.update(cx, |workspace, cx| {
        workspace.increase_content_font_size(1., cx);
    });

    let first_size = cx.read_entity(&first, |workspace, _| {
        f32::from(workspace.typography().content_size())
    });
    let second_size = cx.read_entity(&second, |workspace, _| {
        f32::from(workspace.typography().content_size())
    });
    assert_eq!(second_size, original);
    assert_eq!(first_size, original + 1.);
}

/// 回归：字号快捷键写工作区覆盖后，窗口级读取入口读取覆盖值，全局基准不受影响。
///
/// 版本控制图、终端、Markdown 预览等渲染统一经 typography_for_window 读取，
/// 因此本测试代表全部窗口级视图的读取路径。
#[gpui::test]
fn font_size_override_projects_through_window_typography(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let (workspace, cx) =
        cx.add_window_view(|window, cx| Workspace::new_empty(test_languages(), window, cx));
    let baseline = cx.update(|window, cx| crate::typography_for_window(window, cx).content_size());
    let global_before = cx.update(|_, cx| typography::content_size(cx));

    workspace.update(cx, |workspace, cx| {
        workspace.increase_content_font_size(2., cx);
    });

    let projected = cx.update(|window, cx| crate::typography_for_window(window, cx).content_size());
    let global_after = cx.update(|_, cx| typography::content_size(cx));
    assert_eq!(projected, baseline + px(2.0));
    assert_eq!(global_after, global_before, "工作区覆盖不应写回全局基准");
}

/// 回归：序列化 visible=true 的 dock 随面板注册恢复打开（重启不展开问题）。
#[gpui::test]
fn workspace_restores_visible_dock(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::new_empty(test_languages(), window, cx);
        workspace.bottom_dock.update(cx, |dock, cx| {
            dock.set_serialized_state(
                DockData {
                    visible: true,
                    active_panel: Some("test-panel".into()),
                    size: Some(200.0),
                },
                window,
                cx,
            );
        });
        let panel = cx.new(|cx| TestPanel {
            focus: cx.focus_handle(),
            activation_focus: cx.focus_handle(),
        });
        workspace.register_panel(panel, DockPosition::Bottom, window, cx);
        workspace
    });
    cx.run_until_parked();

    cx.read_entity(&workspace, |workspace, cx| {
        assert!(
            workspace.bottom_dock.read(cx).is_open(),
            "序列化可见的 dock 应随面板注册恢复打开"
        );
    });
}

/// 回归：dock 开合（set_open 直接调用，非 toggle 路径）应经 DockEvent 触发布局保存。
#[gpui::test]
fn dock_open_change_saves_layout(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let directory = tempfile::tempdir().unwrap();
    let layout_path = directory.path().join("layout.json");
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::new_empty(test_languages(), window, cx);
        let panel = cx.new(|cx| TestPanel {
            focus: cx.focus_handle(),
            activation_focus: cx.focus_handle(),
        });
        workspace.register_panel(panel, DockPosition::Bottom, window, cx);
        workspace
    });
    workspace.update(cx, |workspace, _| {
        workspace.layout_path = layout_path.clone();
    });

    // 直接 set_open(true)：cmd-t 打开终端面板等非 toggle 路径。
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.bottom_dock.update(cx, |dock, cx| {
            dock.set_open(true, window, cx);
        });
    });
    cx.executor().advance_clock(LAYOUT_SAVE_THROTTLE);
    cx.run_until_parked();

    let saved = layout_state::load(&layout_path).expect("应保存布局快照");
    assert!(saved.docks.bottom.visible, "dock 开合应触发保存");
}

/// 回归：dock 尺寸变化（拖拽 resize_to / 双击重置 reset_size）应经 DockEvent 触发布局保存。
#[gpui::test]
fn dock_resize_and_reset_save_layout(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let directory = tempfile::tempdir().unwrap();
    let layout_path = directory.path().join("layout.json");
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::new_empty(test_languages(), window, cx);
        let panel = cx.new(|cx| TestPanel {
            focus: cx.focus_handle(),
            activation_focus: cx.focus_handle(),
        });
        workspace.register_panel(panel, DockPosition::Bottom, window, cx);
        workspace
    });
    workspace.update(cx, |workspace, _| {
        workspace.layout_path = layout_path.clone();
    });

    // 拖拽调整：尺寸应落盘。
    workspace.update_in(cx, |workspace, window, cx| {
        let bounds = window.bounds();
        workspace.bottom_dock.update(cx, |dock, cx| {
            dock.resize_to(
                gpui::point(bounds.size.width / 2.0, px(0.0)),
                bounds,
                window,
                cx,
            );
        });
    });
    cx.executor().advance_clock(LAYOUT_SAVE_THROTTLE);
    cx.run_until_parked();
    let saved = layout_state::load(&layout_path).expect("应保存布局快照");
    let dragged = saved.docks.bottom.size.expect("拖拽尺寸应保存");
    assert_ne!(dragged, f32::from(DockPosition::Bottom.default_size()));

    // 双击重置：恢复默认尺寸并同样落盘。
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.bottom_dock.update(cx, |dock, cx| {
            dock.reset_size(window.bounds().size, window, cx);
        });
    });
    cx.executor().advance_clock(LAYOUT_SAVE_THROTTLE);
    cx.run_until_parked();
    let saved = layout_state::load(&layout_path).expect("应保存布局快照");
    assert_eq!(
        saved.docks.bottom.size,
        Some(f32::from(DockPosition::Bottom.default_size())),
        "双击重置的尺寸应触发保存"
    );
}

#[gpui::test]
fn closed_dock_stays_in_action_lifecycle_and_can_reopen(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let directory = tempfile::tempdir().unwrap();
    let layout_path = directory.path().join("layout.json");
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::new_empty(test_languages(), window, cx);
        let panel = cx.new(|cx| TestPanel {
            focus: cx.focus_handle(),
            activation_focus: cx.focus_handle(),
        });
        workspace.register_panel(panel, DockPosition::Left, window, cx);
        workspace
    });
    workspace.update(cx, |workspace, _| {
        workspace.layout_path = layout_path.clone();
    });
    let focus = cx.read_entity(&workspace, |workspace, _| workspace.focus.clone());
    cx.update(|window, cx| window.focus(&focus, cx));

    cx.dispatch_action(FocusOrHidePanel::new("test-panel"));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.read_entity(&workspace, |workspace, cx| {
        workspace.left_dock.read(cx).is_open()
    }));
    cx.update(|window, cx| {
        let activation_focus = workspace
            .read(cx)
            .left_dock
            .read(cx)
            .active_panel()
            .unwrap()
            .activation_focus_handle(cx);
        assert!(activation_focus.is_focused(window));
    });

    // 快捷键：panel 可见但未聚焦时，只聚焦，不隐藏。
    let center_focus = cx.read_entity(&workspace, |workspace, cx| {
        workspace.pane.read(cx).focus_handle()
    });
    cx.update(|window, cx| window.focus(&center_focus, cx));
    cx.dispatch_action(FocusOrHidePanel::new("test-panel"));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.read_entity(&workspace, |workspace, cx| {
        workspace.left_dock.read(cx).is_open()
    }));

    // 快捷键：panel 可见且已聚焦时隐藏。
    cx.dispatch_action(FocusOrHidePanel::new("test-panel"));
    assert!(!cx.read_entity(&workspace, |workspace, cx| {
        workspace.left_dock.read(cx).is_open()
    }));

    // 鼠标按钮路径直接切换可见性，不分派键盘 Action。
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.toggle_panel_visibility_from_button(DockPosition::Left, 0, window, cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.read_entity(&workspace, |workspace, cx| {
        workspace.left_dock.read(cx).is_open()
    }));
    cx.update(|window, cx| {
        let activation_focus = workspace
            .read(cx)
            .left_dock
            .read(cx)
            .active_panel()
            .unwrap()
            .activation_focus_handle(cx);
        assert!(activation_focus.is_focused(window));
    });

    workspace.update_in(cx, |workspace, window, cx| {
        workspace.toggle_panel_visibility_from_button(DockPosition::Left, 0, window, cx);
    });
    assert!(!cx.read_entity(&workspace, |workspace, cx| {
        workspace.left_dock.read(cx).is_open()
    }));
    cx.update(|window, _| assert!(center_focus.is_focused(window)));

    workspace.update_in(cx, |workspace, window, cx| {
        workspace.toggle_panel_visibility_from_button(DockPosition::Left, 0, window, cx);
    });
    cx.update(|window, cx| window.focus(&focus, cx));
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.toggle_panel_visibility_from_button(DockPosition::Left, 0, window, cx);
    });
    cx.update(|window, _| assert!(focus.is_focused(window)));

    cx.dispatch_action(FocusOrHidePanel::new("test-panel"));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.read_entity(&workspace, |workspace, cx| {
        workspace.left_dock.read(cx).is_open()
    }));

    cx.executor().advance_clock(LAYOUT_SAVE_THROTTLE);
    cx.run_until_parked();
    let saved = layout_state::load(&layout_path).expect("应保存布局快照");
    assert!(saved.docks.left.visible);
    assert_eq!(saved.docks.left.active_panel.as_deref(), Some("test-panel"));
}

#[gpui::test]
fn later_workspace_navigation_keeps_the_latest_focus(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::new_empty(test_languages(), window, cx);
        let panel = cx.new(|cx| TestPanel {
            focus: cx.focus_handle(),
            activation_focus: cx.focus_handle(),
        });
        workspace.register_panel(panel, DockPosition::Bottom, window, cx);
        workspace
    });
    let center_focus = cx.read_entity(&workspace, |workspace, cx| {
        workspace.pane.read(cx).focus_handle()
    });

    workspace.update_in(cx, |workspace, window, cx| {
        assert!(workspace.open_panel("test-panel", window, cx));
        workspace.focus_center_pane(window, cx);
    });
    cx.update(|window, _| assert!(center_focus.is_focused(window)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, _| assert!(center_focus.is_focused(window)));
}

#[gpui::test]
fn closing_focused_panel_returns_focus_to_the_center_pane(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let panel = cx.new(|cx| TestPanel {
        focus: cx.focus_handle(),
        activation_focus: cx.focus_handle(),
    });
    let panel_for_registration = panel.clone();
    let (workspace, cx) = cx.add_window_view(move |window, cx| {
        let mut workspace = Workspace::new_empty(test_languages(), window, cx);
        workspace.register_panel(panel_for_registration, DockPosition::Bottom, window, cx);
        workspace
    });
    let center_focus = cx.read_entity(&workspace, |workspace, cx| {
        workspace.pane.read(cx).focus_handle()
    });
    workspace.update_in(cx, |workspace, window, cx| {
        assert!(workspace.open_panel("test-panel", window, cx));
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    panel.update(cx, |_, cx| cx.emit(PanelEvent::Close));
    cx.run_until_parked();

    assert!(!cx.read_entity(&workspace, |workspace, cx| {
        workspace.bottom_dock.read(cx).is_open()
    }));
    cx.update(|window, _| assert!(center_focus.is_focused(window)));
}

#[gpui::test]
fn closing_inactive_panel_preserves_active_panel_focus(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let inactive = cx.new(|cx| TestPanel {
        focus: cx.focus_handle(),
        activation_focus: cx.focus_handle(),
    });
    let active = cx.new(|cx| TestPanel {
        focus: cx.focus_handle(),
        activation_focus: cx.focus_handle(),
    });
    let active_focus = cx.read_entity(&active, |panel, _| panel.activation_focus.clone());
    let inactive_for_registration = inactive.clone();
    let (workspace, cx) = cx.add_window_view(move |window, cx| {
        let mut workspace = Workspace::new_empty(test_languages(), window, cx);
        workspace.register_panel(inactive_for_registration, DockPosition::Bottom, window, cx);
        workspace.register_panel(active, DockPosition::Bottom, window, cx);
        workspace
    });

    workspace.update_in(cx, |workspace, window, cx| {
        workspace.toggle_panel_visibility_from_button(DockPosition::Bottom, 1, window, cx);
    });
    cx.update(|window, _| assert!(active_focus.is_focused(window)));

    inactive.update(cx, |_, cx| cx.emit(PanelEvent::Close));
    cx.run_until_parked();

    assert!(cx.read_entity(&workspace, |workspace, cx| {
        workspace.bottom_dock.read(cx).is_panel_active(1)
    }));
    cx.update(|window, _| assert!(active_focus.is_focused(window)));
}
