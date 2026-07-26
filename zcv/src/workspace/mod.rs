//! WorkbenchFrame —— 窗口顶层装配。

mod dock;
mod item;
mod pane;
mod pane_group;
mod panel;
mod panel_buttons;
mod status_bar;
mod tab_bar;
mod toolbar;
mod top_bar;
mod window_controls;

pub(crate) use dock::{
    Dock, DockPosition, ToggleDebug, ToggleDiagnostics, ToggleKeyboardShortcuts,
    ToggleLanguageServer, ToggleOutline, ToggleProjectSearch, ToggleProjectTree, ToggleTerminal,
    ToggleVersionControl,
};
pub(crate) use item::{ItemEvent, ItemHandle};
pub(crate) use pane::{CloseTab, NextTab, Pane, PaneEvent, PrevTab};
pub(crate) use pane_group::{PaneGroup, PaneId};
pub(crate) use panel::{
    DebugPanel, KeyboardShortcutsPanel, OutlinePanel, Panel, PanelHandle, TerminalPanel,
    VersionControlPanel,
};
pub(crate) use panel_buttons::{PanelButtons, PanelDispatch};
pub(crate) use status_bar::{StatusBar, StatusItemView};
pub(crate) use tab_bar::TabBar;
pub(crate) use toolbar::{Toolbar, ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView};
pub(crate) use top_bar::{GitFetch, GitPull, GitPush, OpenSettings, TopBar};
pub(crate) use window_controls::{
    MinimizeWindow, QuitWindow, ToggleMaximizeWindow, handle_minimize, handle_quit,
    handle_toggle_maximize, render,
};

use std::cell::Cell;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use gpui::{
    AsyncApp, Context, Div, Entity, FocusHandle, Global, MouseButton, Render, Subscription, Task,
    WeakEntity, Window, actions, div, prelude::*, px,
};
use zcv_engine::{Buffer, BufferSaveError};

use self::dock::render_body as render_layout_body;
use crate::diagnostics::DiagnosticsButton;
use crate::editor::BufferStore;
use crate::fs_watcher::{FsWatcher, PathEvent, PathEventKind, Watcher};
use crate::go_to_line::CursorPosition;
use crate::keymap;
use crate::language_selector::ActiveBufferLanguage;
use crate::language_tools::LspButton;
use crate::project_search::ProjectSearchButton;
use crate::project_tree::{OnOpenFile, ProjectTree};
use crate::recent_projects::{self, OnProjectSelected, ToggleProjectPicker};
use crate::theme::{color, typography};

/// 项目根目录全局，供 breadcrumbs 等组件读取相对路径。
#[derive(Clone)]
pub(crate) struct ProjectRoot(pub(crate) PathBuf);

impl Global for ProjectRoot {}

actions!(workspace, [Save]);

pub(crate) struct Workspace {
    pub(crate) focus: FocusHandle,
    pub(crate) center: PaneGroup,
    pub(crate) focus_pane: Option<Entity<Pane>>,
    top_bar: Entity<TopBar>,
    status_bar: Entity<StatusBar>,
    project_tree: Entity<ProjectTree>,
    /// 独立 Entity 管理的三个 Dock 区域。
    pub(crate) left_dock: Entity<Dock>,
    pub(crate) right_dock: Entity<Dock>,
    pub(crate) bottom_dock: Entity<Dock>,
    /// action_name → (Dock Entity, panel_index_in_dock) 的查找表。
    panel_action_map: Vec<(&'static str, Entity<Dock>, usize)>,
    /// 拖拽协调：Dock 通过此 Cell 通知 Workspace 哪个 dock 正在被拖拽。
    drag_notify: Rc<Cell<Option<DockPosition>>>,
    /// FsWatcher 实例。
    fs_watcher: Option<Arc<dyn Watcher>>,
    /// 待处理的 FS 事件缓冲区。
    pending_fs_events: Arc<Mutex<Vec<PathEvent>>>,
    /// FS 事件信号接收端（后台 task 持有 clone）。
    event_signal: async_channel::Receiver<()>,
    /// 前台 task（处理 FS 事件 → reload Buffer + 刷新 ProjectTree）。
    _fs_bg_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    /// 创建所有面板 Entity，返回 (panel_handles, panel_pairs)。
    /// `project_tree` 由调用方先创建并传入，确保只有一个实体。
    fn make_panels(
        project_tree: &Entity<ProjectTree>,
        cx: &mut Context<Self>,
    ) -> (
        Vec<Arc<dyn PanelHandle>>,
        Vec<(Arc<dyn PanelHandle>, DockPosition)>,
    ) {
        let version_control = cx.new(VersionControlPanel::new);
        let outline = cx.new(OutlinePanel::new);
        let terminal = cx.new(TerminalPanel::new);
        let debug = cx.new(DebugPanel::new);
        let keyboard_shortcuts = cx.new(KeyboardShortcutsPanel::new);

        let mut handles: Vec<Arc<dyn PanelHandle>> = Vec::new();
        let mut pairs: Vec<(Arc<dyn PanelHandle>, DockPosition)> = Vec::new();

        macro_rules! reg {
            ($entity:expr, $area:expr) => {{
                let handle: Arc<dyn PanelHandle> = Arc::new($entity);
                handles.push(handle.clone());
                pairs.push((handle, $area));
            }};
        }

        reg!(project_tree.clone(), DockPosition::Left);
        reg!(version_control, DockPosition::Left);
        reg!(outline, DockPosition::Left);
        reg!(terminal, DockPosition::Bottom);
        reg!(debug, DockPosition::Bottom);
        reg!(keyboard_shortcuts, DockPosition::Right);

        (handles, pairs)
    }

    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let weak_self: gpui::WeakEntity<Self> = cx.weak_entity();
        let weak_project_switcher = weak_self.clone();
        let weak_open_file = weak_self.clone();

        let keybindings = keymap::load();
        cx.bind_keys(keybindings.bindings.clone());
        cx.set_global(keybindings);

        let on_project_selected: OnProjectSelected = Rc::new(move |path, window, app| {
            if let Some(ws) = weak_project_switcher.upgrade() {
                ws.update(app, |workspace, cx| {
                    workspace.switch_project(&path, window, cx);
                });
            }
        });

        let top_bar = cx.new(|cx| TopBar::new(on_project_selected, cx));

        let initial_pane = cx.new(|cx| Pane::new(PaneId(1), cx));
        let status_pane = initial_pane.clone();

        // 先创建 ProjectTree（唯一实体），再创建面板
        let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        cx.set_global(ProjectRoot(root.clone()));
        let root_for_tree = root.clone();
        let project_tree: Entity<ProjectTree> = cx.new(|cx| {
            let mut tree = ProjectTree::new(root_for_tree, cx);
            let on_open_file: OnOpenFile = Rc::new(
                move |path: PathBuf, window: &mut Window, cx: &mut gpui::App| {
                    if let Some(ws) = weak_open_file.upgrade() {
                        ws.update(cx, |ws, cx| {
                            ws.open_path_in_active_pane(path, window, cx);
                        });
                    }
                },
            );
            tree.set_on_open_file(on_open_file);
            tree
        });

        let (_all_handles, panel_pairs) = Self::make_panels(&project_tree, cx);

        // ═══ 创建 Dock Entities ═══════════════════════════════════

        let drag_notify: Rc<Cell<Option<DockPosition>>> = Rc::new(Cell::new(None));

        // 按 DockPosition 分组并生成 dispatch 函数
        let make_dispatch = |action_name: &str| -> PanelDispatch {
            match action_name {
                "dock::ToggleProjectTree" => {
                    |w, cx| w.dispatch_action(Box::new(ToggleProjectTree), cx)
                }
                "dock::ToggleVersionControl" => {
                    |w, cx| w.dispatch_action(Box::new(ToggleVersionControl), cx)
                }
                "dock::ToggleOutline" => |w, cx| w.dispatch_action(Box::new(ToggleOutline), cx),
                "dock::ToggleTerminal" => |w, cx| w.dispatch_action(Box::new(ToggleTerminal), cx),
                "dock::ToggleDebug" => |w, cx| w.dispatch_action(Box::new(ToggleDebug), cx),
                "dock::ToggleKeyboardShortcuts" => {
                    |w, cx| w.dispatch_action(Box::new(ToggleKeyboardShortcuts), cx)
                }
                _ => unreachable!(),
            }
        };

        let mut left_handles: Vec<Arc<dyn PanelHandle>> = Vec::new();
        let mut left_dispatches: Vec<PanelDispatch> = Vec::new();
        let mut bottom_handles: Vec<Arc<dyn PanelHandle>> = Vec::new();
        let mut bottom_dispatches: Vec<PanelDispatch> = Vec::new();
        let mut right_handles: Vec<Arc<dyn PanelHandle>> = Vec::new();
        let mut right_dispatches: Vec<PanelDispatch> = Vec::new();

        for (handle, area) in &panel_pairs {
            let dispatch = make_dispatch(handle.action_name());
            match area {
                DockPosition::Left => {
                    left_handles.push(handle.clone());
                    left_dispatches.push(dispatch);
                }
                DockPosition::Bottom => {
                    bottom_handles.push(handle.clone());
                    bottom_dispatches.push(dispatch);
                }
                DockPosition::Right => {
                    right_handles.push(handle.clone());
                    right_dispatches.push(dispatch);
                }
            }
        }

        let left_dock = cx.new(|cx| {
            Dock::new(
                DockPosition::Left,
                left_handles,
                px(240.0),
                drag_notify.clone(),
                cx,
            )
        });
        let right_dock = cx.new(|cx| {
            Dock::new(
                DockPosition::Right,
                right_handles,
                px(240.0),
                drag_notify.clone(),
                cx,
            )
        });
        let bottom_dock = cx.new(|cx| {
            Dock::new(
                DockPosition::Bottom,
                bottom_handles,
                px(200.0),
                drag_notify.clone(),
                cx,
            )
        });

        // 左右 dock 耦合
        left_dock.update(cx, |d, _| d.set_sibling(right_dock.downgrade()));
        right_dock.update(cx, |d, _| d.set_sibling(left_dock.downgrade()));

        // 构建 action_name → (Dock, local_index) 查找表
        let mut panel_action_map: Vec<(&'static str, Entity<Dock>, usize)> = Vec::new();
        for (action_name, dock) in [
            ("dock::ToggleProjectTree", &left_dock),
            ("dock::ToggleVersionControl", &left_dock),
            ("dock::ToggleOutline", &left_dock),
            ("dock::ToggleTerminal", &bottom_dock),
            ("dock::ToggleDebug", &bottom_dock),
            ("dock::ToggleKeyboardShortcuts", &right_dock),
        ] {
            if let Some(local_idx) = dock.read(cx).panel_index_by_action(action_name) {
                panel_action_map.push((action_name, dock.clone(), local_idx));
            }
        }

        // ═══ 中心编辑区 ══════════════════════════════════════════

        let center = PaneGroup::Pane(PaneId(1), initial_pane.clone());

        // ═══ StatusBar ═══════════════════════════════════════════

        let status_bar = cx.new(|cx| StatusBar::new(status_pane, cx));
        status_bar.update(cx, |bar, cx| {
            bar.add_left_item(
                cx.new(|cx| PanelButtons::new(left_dock.clone(), left_dispatches, cx)),
                cx,
            );
            bar.add_left_item(cx.new(|_| LspButton::new()), cx);
            bar.add_left_item(cx.new(|_| DiagnosticsButton::new()), cx);
            bar.add_left_item(cx.new(|_| ProjectSearchButton::new()), cx);
            bar.add_right_item(cx.new(|_| CursorPosition::new()), cx);
            bar.add_right_item(cx.new(|_| ActiveBufferLanguage::new()), cx);
            bar.add_right_item(
                cx.new(|cx| PanelButtons::new(bottom_dock.clone(), bottom_dispatches, cx)),
                cx,
            );
            bar.add_right_item(
                cx.new(|cx| PanelButtons::new(right_dock.clone(), right_dispatches, cx)),
                cx,
            );
        });

        cx.set_global(BufferStore::new());

        // ═══ 文件系统监听（参考 Zed 架构：Workspace 统一管理） ═════

        let (fs_signal_tx, fs_signal_rx) = async_channel::unbounded::<()>();
        let fs_pending: Arc<Mutex<Vec<PathEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let fs_watcher: Arc<dyn Watcher> =
            Arc::new(FsWatcher::new(fs_signal_tx, fs_pending.clone()));

        if let Err(e) = fs_watcher.add(&root) {
            log::warn!("无法监听项目目录 {:?}：{e}", root);
        }

        // 前台 task：处理 FS 事件 → reload Buffer + 刷新 ProjectTree
        let bg_pending = fs_pending.clone();
        let bg_signal = fs_signal_rx.clone();
        let fs_bg_task: Task<()> =
            cx.spawn(|ws: WeakEntity<Workspace>, async_cx: &mut AsyncApp| {
                let mut cx = async_cx.clone();
                async move {
                    while let Ok(()) = bg_signal.recv().await {
                        let events = std::mem::take(&mut *bg_pending.lock().unwrap());

                        // 1. 外部修改文件 → 重新加载已打开的 Buffer
                        for event in &events {
                            if matches!(
                                event.kind,
                                Some(PathEventKind::Changed | PathEventKind::Created)
                            ) {
                                let path = event.path.clone();
                                let _ = cx.update(|app| {
                                    app.update_global::<BufferStore, _>(|store, app| {
                                        store.reload_buffer_for_path(&path, app);
                                    });
                                });
                            }
                        }

                        // 2. 通知 ProjectTree 刷新
                        if !events.is_empty() {
                            let _ = ws.update(&mut cx, |workspace, cx| {
                                workspace.project_tree.update(cx, |_, cx| cx.notify());
                            });
                        }
                    }
                }
            });

        Self {
            focus,
            center,
            focus_pane: Some(initial_pane),
            top_bar,
            status_bar,
            project_tree,
            left_dock,
            right_dock,
            bottom_dock,
            panel_action_map,
            drag_notify,
            fs_watcher: Some(fs_watcher),
            pending_fs_events: fs_pending,
            event_signal: fs_signal_rx,
            _fs_bg_task: Some(fs_bg_task),
            _subscriptions: Vec::new(),
        }
    }

    /// 由项目树回调调用的文件打开逻辑。
    fn open_path_in_active_pane(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = match path.canonicalize() {
            Ok(p) => p,
            Err(error) => {
                eprintln!("打开文件失败：{}：{error}", path.display());
                return;
            }
        };
        let Ok(buffer) =
            cx.update_global::<BufferStore, _>(|store, cx| store.open_buffer(&path, cx))
        else {
            return;
        };
        let Some(pane) = self.focus_pane.clone() else {
            return;
        };
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let focus = pane.update(cx, |pane, cx| {
            pane.open_file(path, file_name, buffer, window, cx)
        });
        window.focus(&focus);
        window.refresh();
    }

    /// 开发构建启动时，沿用正式项目切换链路打开 zcv 工作区。
    #[cfg(debug_assertions)]
    pub(crate) fn open_development_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let project_root = crate_dir.parent().unwrap_or(&crate_dir);
        self.switch_project(&project_root.to_string_lossy(), window, cx);
    }

    fn handle_git_fetch(_: &top_bar::GitFetch, _: &mut Window, _: &mut gpui::App) {
        println!("fetch");
    }

    fn handle_git_pull(_: &top_bar::GitPull, _: &mut Window, _: &mut gpui::App) {
        println!("pull");
    }

    fn handle_git_push(_: &top_bar::GitPush, _: &mut Window, _: &mut gpui::App) {
        println!("push");
    }

    fn handle_open_settings(_: &top_bar::OpenSettings, _: &mut Window, _: &mut gpui::App) {
        println!("设置");
    }

    fn handle_save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.focus_pane.clone() else {
            return;
        };
        let (editor, path) = {
            let pane = pane.read(cx);
            let Some(editor) = pane.active_editor(cx) else {
                return;
            };
            let Some(path) = pane.active_path(cx) else {
                return;
            };
            (editor, path)
        };
        let buffer = editor.read(cx).buffer();

        let result = buffer.update(cx, |buffer, cx| {
            let result = write_buffer_to_path(buffer, &path);
            if result.is_ok() {
                cx.notify();
            }
            result
        });
        if let Err(error) = result {
            eprintln!("保存文件失败（{}）：{error}", path.display());
        }
    }

    /// 切换面板焦点：通过 panel_action_map 找到对应的 Dock 进行操作。
    fn toggle_panel_focus_for_dock(
        &mut self,
        dock: Entity<Dock>,
        panel_idx: usize,
        focus_handle: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if focus_handle.contains_focused(window, cx) {
            dock.update(cx, |d, cx| d.toggle_panel(panel_idx, window, cx));
            self.focus_center_pane(window, cx);
        } else {
            if !dock.read(cx).is_panel_active(panel_idx) {
                dock.update(cx, |d, cx| d.toggle_panel(panel_idx, window, cx));
            }
            window.focus(focus_handle);
        }
        window.refresh();
    }

    /// 聚焦回编辑区。
    fn focus_center_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ref pane_entity) = self.focus_pane {
            let pane = pane_entity.read(cx);
            if let Some(editor) = pane.active_editor(cx) {
                window.focus(&editor.read(cx).focus_handle());
            } else {
                window.focus(&pane.focus);
            }
        }
    }

    /// 注册焦点监听：当指定 Pane 或其子元素获得焦点时更新 StatusBar。
    pub(crate) fn register_pane_focus_listener(
        &mut self,
        pane: &Entity<Pane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = pane.read(cx).focus.clone();
        let pane_entity = pane.clone();
        let sub = cx.on_focus_in(&focus, window, move |this, _window, cx| {
            this.handle_pane_focused(&pane_entity, cx);
        });
        self._subscriptions.push(sub);

        let sub = cx.subscribe_in(pane, window, |_this, _emitter, event, _window, _cx| {
            let _ = event;
        });
        self._subscriptions.push(sub);
    }

    /// 当 Pane 获得焦点时更新 Workspace 和 StatusBar 的焦点 Pane。
    fn handle_pane_focused(&mut self, pane: &Entity<Pane>, cx: &mut Context<Self>) {
        self.focus_pane = Some(pane.clone());
        self.status_bar.update(cx, |bar, cx| {
            bar.set_active_pane(pane, cx);
        });
    }

    fn handle_close_tab(
        &mut self,
        _: &pane::CloseTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane_entity) = self.focus_pane.clone() else {
            return;
        };
        let pane_focus = pane_entity.read(cx).focus.clone();
        if let Some(view_id) = pane_entity.read(cx).active {
            pane_entity.update(cx, |pane, cx| {
                pane.close_tab(view_id, window, cx);
                cx.emit(pane::PaneEvent::Removed { view_id });
                cx.notify();
            });
            if let Some(editor) = pane_entity.read(cx).active_editor(cx) {
                window.focus(&editor.read(cx).focus_handle());
            } else {
                window.focus(&pane_focus);
            }
            window.refresh();
        }
    }

    fn handle_toggle_project_tree(
        &mut self,
        _: &ToggleProjectTree,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = self.project_tree.read(cx).focus.clone();
        if let Some((_, dock, panel_idx)) = self
            .panel_action_map
            .iter()
            .find(|(name, _, _)| *name == "dock::ToggleProjectTree")
        {
            self.toggle_panel_focus_for_dock(dock.clone(), *panel_idx, &focus, window, cx);
        }
    }

    fn handle_toggle_panel(
        &mut self,
        action_name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entry = match self
            .panel_action_map
            .iter()
            .find(|(name, _, _)| *name == action_name)
        {
            Some(e) => (e.1.clone(), e.2),
            None => return,
        };
        let (dock, panel_idx) = entry;

        let was_active = dock.read(cx).is_panel_active(panel_idx);
        dock.update(cx, |d, cx| d.toggle_panel(panel_idx, window, cx));

        if was_active {
            self.focus_center_pane(window, cx);
        } else {
            let focus = dock.read(cx).panels[panel_idx].focus_handle(cx);
            window.focus(&focus);
        }
        window.refresh();
    }

    fn handle_toggle_project_picker(
        &mut self,
        _: &ToggleProjectPicker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.top_bar.update(cx, |bar, cx| {
            bar.project_picker.update(cx, |picker, cx| {
                picker.toggle(window, cx);
            });
        });
    }

    /// 切换到指定目录作为项目根目录，同时重启文件监听。
    fn switch_project(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let root = PathBuf::from(path);
        let label = root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        // 重启 FsWatcher：移除旧根 → 监听新根
        if let Some(watcher) = &self.fs_watcher {
            let _ = watcher.remove(self.project_tree.read(cx).root());
            if let Err(e) = watcher.add(&root) {
                log::warn!("无法监听项目目录 {:?}：{e}", root);
            }
        }

        self.project_tree.update(cx, |tree, cx| {
            tree.set_root(root.clone(), cx);
        });
        self.top_bar.update(cx, |bar, cx| {
            bar.project_picker.update(cx, |picker, _cx| {
                picker.set_current_label(label);
            });
        });
        recent_projects::add_to_recent(path);
        window.refresh();
    }
}

// ═══ 渲染 ═════════════════════════════════════════════════════════

/// 工作台顶层框架组装（简化版：直接接收 body Div）。
fn render_frame(top_bar: &Entity<TopBar>, status_bar: &Entity<StatusBar>, body: gpui::Div) -> Div {
    div()
        .relative()
        .flex()
        .flex_col()
        .size_full()
        .overflow_hidden()
        .bg(color::current().gray.s[1])
        .font(typography::ui_font())
        .text_size(typography::ui())
        .line_height(typography::ui())
        .text_color(color::current().gray.s[8])
        .child(top_bar.clone())
        .child(body)
        .child(status_bar.clone())
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let center = self.center.clone();

        let left_dock = if self.left_dock.read(cx).is_open() {
            Some(self.left_dock.clone())
        } else {
            None
        };
        let right_dock = if self.right_dock.read(cx).is_open() {
            Some(self.right_dock.clone())
        } else {
            None
        };
        let bottom_dock = if self.bottom_dock.read(cx).is_open() {
            Some(self.bottom_dock.clone())
        } else {
            None
        };

        let left_dock_entity = self.left_dock.clone();
        let right_dock_entity = self.right_dock.clone();
        let bottom_dock_entity = self.bottom_dock.clone();
        let drag_notify = self.drag_notify.clone();

        let left_dock_up = self.left_dock.clone();
        let right_dock_up = self.right_dock.clone();
        let bottom_dock_up = self.bottom_dock.clone();
        let drag_notify_up = self.drag_notify.clone();

        div()
            .id("app-view")
            .track_focus(&self.focus)
            .key_context("Workspace")
            .size_full()
            .relative()
            .child(render_frame(
                &self.top_bar,
                &self.status_bar,
                render_layout_body(&center, left_dock, right_dock, bottom_dock),
            ))
            .on_action(handle_quit)
            .on_action(handle_minimize)
            .on_action(handle_toggle_maximize)
            .on_action(cx.listener(Self::handle_close_tab))
            .on_action(Self::handle_git_fetch)
            .on_action(Self::handle_git_pull)
            .on_action(Self::handle_git_push)
            .on_action(Self::handle_open_settings)
            .on_action(cx.listener(Self::handle_save))
            .on_action(cx.listener(Self::handle_toggle_project_tree))
            .on_action(cx.listener(
                |this: &mut Workspace, _: &ToggleVersionControl, window, cx| {
                    this.handle_toggle_panel("dock::ToggleVersionControl", window, cx);
                },
            ))
            .on_action(
                cx.listener(|this: &mut Workspace, _: &ToggleOutline, window, cx| {
                    this.handle_toggle_panel("dock::ToggleOutline", window, cx);
                }),
            )
            .on_action(
                cx.listener(|this: &mut Workspace, _: &ToggleTerminal, window, cx| {
                    this.handle_toggle_panel("dock::ToggleTerminal", window, cx);
                }),
            )
            .on_action(
                cx.listener(|this: &mut Workspace, _: &ToggleDebug, window, cx| {
                    this.handle_toggle_panel("dock::ToggleDebug", window, cx);
                }),
            )
            .on_action(cx.listener(
                |this: &mut Workspace, _: &ToggleKeyboardShortcuts, window, cx| {
                    this.handle_toggle_panel("dock::ToggleKeyboardShortcuts", window, cx);
                },
            ))
            .on_action(cx.listener(Self::handle_toggle_project_picker))
            .on_mouse_move(move |event, window, cx| {
                if let Some(area) = drag_notify.get() {
                    let dock = match area {
                        DockPosition::Left => &left_dock_entity,
                        DockPosition::Right => &right_dock_entity,
                        DockPosition::Bottom => &bottom_dock_entity,
                    };
                    dock.update(cx, |dock, cx| {
                        if dock.is_dragging() {
                            dock.resize_to(event.position, window.bounds().size, cx);
                        }
                    });
                    window.refresh();
                }
            })
            .on_mouse_up(MouseButton::Left, move |_event, window, cx| {
                for dock in [&left_dock_up, &right_dock_up, &bottom_dock_up] {
                    dock.update(cx, |d, cx| d.end_resize(cx));
                }
                drag_notify_up.set(None);
                window.refresh();
            })
    }
}

fn write_buffer_to_path(buffer: &mut Buffer, path: &Path) -> Result<(), BufferSaveError> {
    let version = buffer.version();
    let mut file = File::create(path)?;
    buffer.write_to(version, &mut file)?;
    file.sync_all()?;
    buffer.mark_saved();
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use zcv_engine::{BufferConfig, ByteOffset};

    use super::*;

    #[test]
    fn saving_buffer_writes_current_version_and_marks_it_clean() {
        let path = test_file_path();
        let mut buffer =
            Buffer::scratch("旧内容".to_owned(), BufferConfig::default()).expect("应创建 Buffer");
        buffer
            .insert(buffer.len_bytes(), " + 新内容")
            .expect("测试编辑应成功");
        assert!(buffer.is_dirty());

        write_buffer_to_path(&mut buffer, &path).expect("保存应成功");

        assert_eq!(
            fs::read_to_string(&path).expect("应读回文件"),
            "旧内容 + 新内容"
        );
        assert!(!buffer.is_dirty());
        fs::remove_file(path).expect("测试文件应可删除");
    }

    #[test]
    fn failed_save_keeps_buffer_dirty() {
        let path = test_file_path().join("missing.txt");
        let mut buffer =
            Buffer::scratch("内容".to_owned(), BufferConfig::default()).expect("应创建 Buffer");
        buffer
            .insert(ByteOffset::ZERO, "未保存")
            .expect("测试编辑应成功");

        assert!(write_buffer_to_path(&mut buffer, &path).is_err());
        assert!(buffer.is_dirty());
    }

    fn test_file_path() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("系统时间应晚于 Unix Epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "zcv-workspace-save-{}-{nonce}.txt",
            std::process::id()
        ))
    }
}
