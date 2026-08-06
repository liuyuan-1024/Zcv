//! WorkbenchFrame —— 窗口顶层装配。

mod dock;
mod item;
mod pane;
mod panel;
mod panel_buttons;
mod status_bar;
mod tab_bar;
mod toolbar;
mod top_bar;
mod window_controls;

use dock::DockPosition;
pub(crate) use dock::{
    Dock, ToggleDebug, ToggleDiagnostics, ToggleKeyboardShortcuts, ToggleLanguageServer,
    ToggleOutline, ToggleProjectSearch, ToggleProjectTree, ToggleTerminal, ToggleVersionControl,
};
pub(crate) use item::{ItemEvent, ItemHandle};
pub(crate) use pane::Pane;
pub(crate) use panel::Panel;
use panel::{
    DebugPanel, KeyboardShortcutsPanel, OutlinePanel, PanelHandle, TerminalPanel,
    VersionControlPanel,
};
use panel_buttons::PanelButtons;
use status_bar::StatusBar;
pub(crate) use status_bar::StatusItemView;
pub(crate) use toolbar::{ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView};
use top_bar::TopBar;
use window_controls::{handle_minimize, handle_quit, handle_toggle_maximize};

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    Action, Context, Div, Entity, FocusHandle, MouseButton, Render, Subscription, Window, actions,
    div, prelude::*,
};

use self::dock::render_body as render_layout_body;
use crate::active_buffer_language::ActiveBufferLanguage;
use crate::cursor_position::CursorPosition;
use crate::diagnostics::DiagnosticsButton;
use crate::keymap;
use crate::language_tools::LspButton;
use crate::open_project_window;
use crate::project::{GitOperationKind, Project, ProjectEvent};
use crate::project_search::ProjectSearchButton;
use crate::project_tree::{OnCreate, OnOpenFile, OnRename, OnTrash, ProjectTree};
use crate::recent_projects::{OnProjectSelected, ToggleProjectPicker};
use crate::settings::SettingsStore;
use zcv_editor::Editor;
use zcv_theme::{color, typography};

actions!(workspace, [Save]);

/// 面板句柄与其停靠位置的配对列表。
type PanelPairs = Vec<(Arc<dyn PanelHandle>, DockPosition)>;

pub(crate) struct Workspace {
    pub(crate) focus: FocusHandle,
    pub(crate) pane: Entity<Pane>,
    top_bar: Entity<TopBar>,
    status_bar: Entity<StatusBar>,
    project: Entity<Project>,
    project_tree: Entity<ProjectTree>,
    /// 独立 Entity 管理的三个 Dock 区域。
    pub(crate) left_dock: Entity<Dock>,
    pub(crate) right_dock: Entity<Dock>,
    pub(crate) bottom_dock: Entity<Dock>,
    /// toggle action → (Dock Entity, panel_index_in_dock) 的查找表。
    /// 条目由各面板自身的 `toggle_action` 派生，快捷键、按钮与分派共用同一来源。
    panel_actions: Vec<(Box<dyn Action>, Entity<Dock>, usize)>,
    /// 拖拽协调：Dock 通过此 Cell 通知 Workspace 哪个 dock 正在被拖拽。
    drag_notify: Rc<Cell<Option<DockPosition>>>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    /// 创建所有面板 Entity，返回 (panel_handles, panel_pairs)。
    /// `project_tree` 由调用方先创建并传入，确保只有一个实体。
    fn make_panels(
        project_tree: &Entity<ProjectTree>,
        cx: &mut Context<Self>,
    ) -> (Vec<Arc<dyn PanelHandle>>, PanelPairs) {
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

    pub(crate) fn new(root: PathBuf, window: &Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let weak_self: gpui::WeakEntity<Self> = cx.weak_entity();
        let weak_project_switcher = weak_self.clone();
        let weak_open_file = weak_self.clone();
        let weak_rename = weak_self.clone();
        let weak_create = weak_self.clone();
        let weak_trash = weak_self.clone();

        let keybindings = keymap::load(cx).expect("内置 keymap 应完整有效");
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
        top_bar.update(cx, |bar, cx| {
            let label = root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            bar.project_picker.update(cx, |picker, _| {
                picker.set_current_label(label);
            });
        });

        let pane = cx.new(Pane::new);

        // 先创建 ProjectTree（唯一实体），再创建面板
        let project = cx.new(|cx| Project::new(root.clone(), cx));
        let root_for_tree = root.clone();
        let project_tree: Entity<ProjectTree> = cx.new(|cx| {
            let mut tree = ProjectTree::new(root_for_tree, project.clone(), cx);
            let on_open_file: OnOpenFile = Rc::new(
                move |path: PathBuf,
                      focus_opened_item: bool,
                      window: &mut Window,
                      cx: &mut gpui::App| {
                    if let Some(ws) = weak_open_file.upgrade() {
                        ws.update(cx, |ws, cx| {
                            ws.open_path(path, focus_opened_item, window, cx);
                        });
                    }
                },
            );
            tree.set_on_open_file(on_open_file);
            let on_rename: OnRename = Rc::new(move |from, to, cx| {
                let Some(workspace) = weak_rename.upgrade() else {
                    anyhow::bail!("工作区已关闭");
                };
                workspace.update(cx, |workspace, cx| workspace.rename_path(&from, &to, cx))
            });
            tree.set_on_rename(on_rename);
            let on_create: OnCreate = Rc::new(move |path, is_dir, cx| {
                let Some(workspace) = weak_create.upgrade() else {
                    anyhow::bail!("工作区已关闭");
                };
                workspace.update(cx, |workspace, cx| workspace.create_path(&path, is_dir, cx))
            });
            tree.set_on_create(on_create);
            let on_trash: OnTrash = Rc::new(move |path, window, cx| {
                let Some(workspace) = weak_trash.upgrade() else {
                    anyhow::bail!("工作区已关闭");
                };
                workspace.update(cx, |workspace, cx| workspace.trash_path(&path, window, cx))
            });
            tree.set_on_trash(on_trash);
            tree
        });

        let (_all_handles, panel_pairs) = Self::make_panels(&project_tree, cx);

        // ═══ 创建 Dock Entities ═══════════════════════════════════

        let drag_notify: Rc<Cell<Option<DockPosition>>> = Rc::new(Cell::new(None));

        // 按 DockPosition 分组
        let mut left_handles: Vec<Arc<dyn PanelHandle>> = Vec::new();
        let mut bottom_handles: Vec<Arc<dyn PanelHandle>> = Vec::new();
        let mut right_handles: Vec<Arc<dyn PanelHandle>> = Vec::new();

        for (handle, area) in &panel_pairs {
            match area {
                DockPosition::Left => left_handles.push(handle.clone()),
                DockPosition::Bottom => bottom_handles.push(handle.clone()),
                DockPosition::Right => right_handles.push(handle.clone()),
            }
        }

        let left_dock = cx.new(|cx| {
            Dock::new(
                DockPosition::Left,
                left_handles,
                DockPosition::Left.default_size(),
                drag_notify.clone(),
                cx,
            )
        });
        let right_dock = cx.new(|cx| {
            Dock::new(
                DockPosition::Right,
                right_handles,
                DockPosition::Right.default_size(),
                drag_notify.clone(),
                cx,
            )
        });
        let bottom_dock = cx.new(|cx| {
            Dock::new(
                DockPosition::Bottom,
                bottom_handles,
                DockPosition::Bottom.default_size(),
                drag_notify.clone(),
                cx,
            )
        });

        // 左右 dock 耦合
        left_dock.update(cx, |d, _| d.set_sibling(right_dock.downgrade()));
        right_dock.update(cx, |d, _| d.set_sibling(left_dock.downgrade()));

        // 构建 toggle action → (Dock, local_index) 查找表，action 由面板自身派生。
        let mut panel_actions: Vec<(Box<dyn Action>, Entity<Dock>, usize)> = Vec::new();
        for dock in [&left_dock, &bottom_dock, &right_dock] {
            for (idx, handle) in dock.read(cx).panels.iter().enumerate() {
                panel_actions.push((handle.toggle_action(cx), dock.clone(), idx));
            }
        }

        // ═══ StatusBar ═══════════════════════════════════════════

        let status_bar = cx.new(|cx| StatusBar::new(pane.clone(), cx));
        status_bar.update(cx, |bar, cx| {
            bar.add_left_item(cx.new(|cx| PanelButtons::new(left_dock.clone(), cx)), cx);
            bar.add_left_item(cx.new(|_| LspButton::new()), cx);
            bar.add_left_item(cx.new(|_| DiagnosticsButton::new()), cx);
            bar.add_left_item(cx.new(|_| ProjectSearchButton::new()), cx);
            bar.add_right_item(cx.new(|_| CursorPosition::new()), cx);
            bar.add_right_item(cx.new(|_| ActiveBufferLanguage::new()), cx);
            bar.add_right_item(cx.new(|cx| PanelButtons::new(bottom_dock.clone(), cx)), cx);
            bar.add_right_item(cx.new(|cx| PanelButtons::new(right_dock.clone(), cx)), cx);
        });

        // 项目根只在「项目根被外部重命名」时变化（切换项目 = 新窗口，不再动态换根）。
        let project_subscription =
            cx.subscribe(&project, |workspace, _project, event, cx| match event {
                ProjectEvent::RootChanged(root) => {
                    workspace.project_tree.update(cx, |tree, cx| {
                        tree.set_root(root.clone(), cx);
                    });
                }
                ProjectEvent::EntriesChanged => {
                    workspace
                        .project_tree
                        .update(cx, |tree, cx| tree.refresh(cx));
                }
            });

        // git 分支显示：GitStore 事件（首次扫描、外部 checkout、拉取操作）到达时刷新。
        let git_store = project.read(cx).git_store();
        let git_subscription = cx.subscribe(&git_store, |workspace, store, _event, cx| {
            let branch = store.read(cx).current_branch().map(str::to_string);
            let has_repositories = store.read(cx).has_repositories();
            let remote_operation_state = store.read(cx).remote_operation_state();
            workspace.top_bar.update(cx, |bar, cx| {
                bar.set_branch(branch);
                bar.set_has_repositories(has_repositories);
                bar.set_remote_operation_state(remote_operation_state);
                cx.notify();
            });
            // hunks 查询完成（Statuses 事件）后补推给打开的编辑器；缺失路径按需请求。
            workspace.push_diff_hunks(cx);
        });

        let pane_subscription = cx.subscribe(&pane, |workspace, pane, event, cx| {
            if matches!(
                event,
                pane::PaneEvent::Activate { .. } | pane::PaneEvent::Removed { .. }
            ) {
                let active_path = pane.read(cx).active_path(cx);
                // 活动仓库跟随焦点文件（最长前缀匹配）：打开/切换子项目文件后，分支显示与 fetch/pull/push 自动指向其所属仓库。
                if let Some(path) = &active_path {
                    workspace.project.update(cx, |project, cx| {
                        project.git_store().update(cx, |store, cx| {
                            store.set_active_repository_for_path(path, cx);
                        });
                    });
                }
                workspace.project_tree.update(cx, |tree, cx| {
                    tree.reveal_active_path(active_path, cx);
                });
            }
            // 打开/激活编辑器时推送 git diff hunks（打开即有快照里的现成数据）。
            workspace.push_diff_hunks(cx);
        });

        let settings_subscription =
            cx.observe_global_in::<SettingsStore>(window, |workspace, window, cx| {
                let settings = SettingsStore::get(cx);
                settings.theme.apply(cx, Some(window));
                workspace.pane.update(cx, |pane, cx| {
                    pane.set_soft_wrap(settings.soft_wrap, settings.preferred_line_length, cx)
                });
                // 扫描排除名单变化时重建项目树行模型。
                workspace
                    .project_tree
                    .update(cx, |tree, cx| tree.refresh(cx));
                cx.notify();
            });

        // 系统外观（深色/浅色）变化时重新应用主题。
        let appearance_subscription = window.observe_window_appearance(|window, cx| {
            let settings = SettingsStore::get(cx);
            settings.theme.apply(cx, Some(window));
            window.refresh();
        });

        Self {
            focus,
            pane,
            top_bar,
            status_bar,
            project,
            project_tree,
            left_dock,
            right_dock,
            bottom_dock,
            panel_actions,
            drag_notify,
            _subscriptions: vec![
                project_subscription,
                git_subscription,
                pane_subscription,
                settings_subscription,
                appearance_subscription,
            ],
        }
    }

    /// 由项目树回调调用的文件打开逻辑。
    fn open_path(
        &mut self,
        path: PathBuf,
        focus_opened_item: bool,
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
        let (project_root, buffer) = self.project.update(cx, |project, cx| {
            (project.root().to_path_buf(), project.open_buffer(&path, cx))
        });
        let buffer = match buffer {
            Ok(buffer) => buffer,
            Err(error) => {
                eprintln!("打开文件失败：{}：{error}", path.display());
                return;
            }
        };
        let pane = self.pane.clone();
        let focus = pane.update(cx, |pane, cx| {
            pane.open_file(path, project_root, buffer, window, cx)
        });
        let settings = SettingsStore::get(cx);
        pane.update(cx, |pane, cx| {
            pane.set_soft_wrap(settings.soft_wrap, settings.preferred_line_length, cx)
        });
        if focus_opened_item {
            window.focus(&focus);
        }
        window.refresh();
    }

    /// 后台执行 git 操作（fetch/pull/push），完成后 git_store 自动重新扫描。
    fn run_git_operation(&mut self, operation: GitOperationKind, cx: &mut Context<Self>) {
        self.project.update(cx, |project, cx| {
            project.git_store().update(cx, |store, cx| {
                store.run_operation(operation, cx);
            });
        });
    }

    fn handle_git_fetch(&mut self, _: &top_bar::GitFetch, _: &mut Window, cx: &mut Context<Self>) {
        self.run_git_operation(GitOperationKind::Fetch, cx);
    }

    /// 在当前窗口切换到指定项目。
    /// 切换项目：以新窗口打开目标项目并关闭当前窗口（替换语义，不保留旧项目窗口）。
    ///
    /// 先开新窗口、成功后关旧窗口：打开失败时当前窗口原样保留。
    /// 项目与窗口生命周期绑定，旧项目随窗口释放，无需在当前窗口内换根。
    pub(crate) fn switch_project(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = PathBuf::from(path);
        if let Err(error) = open_project_window(root, cx) {
            eprintln!("打开项目失败（{path}）：{error}");
            return;
        }
        window.remove_window();
    }

    fn handle_git_pull(&mut self, _: &top_bar::GitPull, _: &mut Window, cx: &mut Context<Self>) {
        self.run_git_operation(GitOperationKind::Pull, cx);
    }

    fn handle_git_push(&mut self, _: &top_bar::GitPush, _: &mut Window, cx: &mut Context<Self>) {
        self.run_git_operation(GitOperationKind::Push, cx);
    }

    fn handle_open_settings(
        &mut self,
        _: &top_bar::OpenSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match crate::settings::ensure_user_settings_file() {
            Ok(path) => self.open_path(path.to_path_buf(), true, window, cx),
            Err(error) => eprintln!("无法打开设置文件：{error}"),
        }
    }

    /// 把 GitStore 快照中的行级 diff hunks 推送给打开的 Editor。
    ///
    /// 路径尚未查询（打开文件后首次、或全量扫描后）时按需发起后台查询，完成后经 GitStoreEvent::Statuses 回到本函数补齐（无死循环：查询完成即 Some）。
    fn push_diff_hunks(&mut self, cx: &mut Context<Self>) {
        let store = self.project.read(cx).git_store();
        // 先收集打开的编辑器 (editor, path)，避免持 pane 借用时再可变借用 cx。
        let opened: Vec<(Entity<Editor>, PathBuf)> = self
            .pane
            .read(cx)
            .tabs
            .iter()
            .filter_map(|item| {
                let editor = item.downcast::<Editor>()?;
                let path = item.file_path(cx)?;
                Some((editor, path))
            })
            .collect();
        let mut missing = Vec::new();
        for (editor, path) in opened {
            let Some(hunks) = store.read(cx).hunks_for_path(&path) else {
                // 尚未查询：按需请求，事件回来再补。
                missing.push(path);
                continue;
            };
            let hunks: Vec<zcv_editor::DiffHunk> = hunks.to_vec();
            editor.update(cx, |editor, cx| editor.set_diff_hunks(hunks, cx));
        }
        if !missing.is_empty() {
            store.update(cx, |store, cx| store.request_hunks(&missing, cx));
        }
    }

    fn handle_save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.pane.clone();
        let (buffer, path) = {
            let pane = pane.read(cx);
            let Some(item) = pane.active_item() else {
                return;
            };
            let Some(buffer) = item.buffer(cx) else {
                return;
            };
            let Some(path) = pane.active_path(cx) else {
                return;
            };
            (buffer, path)
        };

        let result = self
            .project
            .update(cx, |project, cx| project.save_buffer(&buffer, &path, cx));
        if let Err(error) = result {
            eprintln!("保存文件失败（{}）：{error}", path.display());
        }
    }

    fn rename_path(
        &mut self,
        from: &Path,
        to: &Path,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        self.project
            .update(cx, |project, cx| project.rename_path(from, to, cx))?;
        self.pane
            .update(cx, |pane, cx| pane.rename_path(from, to, cx));
        Ok(())
    }

    fn create_path(
        &mut self,
        path: &Path,
        is_dir: bool,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        self.project
            .update(cx, |project, cx| project.create_path(path, is_dir, cx))
    }

    /// 将文件或目录移到系统废纸篓，并关闭打开它的标签页。
    fn trash_path(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        self.project
            .update(cx, |project, cx| project.trash_path(path, cx))?;
        self.pane
            .update(cx, |pane, cx| pane.remove_path(path, window, cx));
        Ok(())
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
        let pane = self.pane.read(cx);
        if let Some(item) = pane.active_item() {
            window.focus(&item.item_focus_handle(cx));
        } else {
            window.focus(&pane.focus);
        }
    }

    fn handle_close_tab(
        &mut self,
        _: &pane::CloseTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane_entity = self.pane.clone();
        let pane_focus = pane_entity.read(cx).focus.clone();
        if let Some(item_id) = pane_entity.read(cx).active {
            pane_entity.update(cx, |pane, cx| {
                pane.close_tab(item_id, window, cx);
            });
            if let Some(item) = pane_entity.read(cx).active_item() {
                window.focus(&item.item_focus_handle(cx));
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
        let Some((_, dock, panel_idx)) = self
            .panel_actions
            .iter()
            .find(|(action, _, _)| action.partial_eq(&ToggleProjectTree))
        else {
            return;
        };
        self.toggle_panel_focus_for_dock(dock.clone(), *panel_idx, &focus, window, cx);
    }

    /// 通用面板 toggle handler：把面板自身的 toggle action 路由到对应 Dock。
    fn handle_toggle_panel<A: Action>(
        &mut self,
        action: &A,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, dock, panel_idx)) = self
            .panel_actions
            .iter()
            .find(|(candidate, _, _)| candidate.partial_eq(action))
        else {
            return;
        };
        let (dock, panel_idx) = (dock.clone(), *panel_idx);

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
}

// ═══ 渲染 ═════════════════════════════════════════════════════════

/// 工作台顶层框架组装（简化版：直接接收 body Div）。
fn render_frame(
    top_bar: &Entity<TopBar>,
    status_bar: &Entity<StatusBar>,
    body: gpui::Div,
    cx: &gpui::App,
) -> Div {
    div()
        .relative()
        .flex()
        .flex_col()
        .size_full()
        .overflow_hidden()
        .bg(color::current(cx).surface_background)
        .font(typography::ui_font())
        .text_size(typography::ui())
        .line_height(typography::ui())
        .text_color(color::current(cx).text)
        .child(top_bar.clone())
        .child(body)
        .child(status_bar.clone())
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = self.pane.clone();

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
                render_layout_body(&pane, left_dock, right_dock, bottom_dock),
                cx,
            ))
            .on_action(handle_quit)
            .on_action(handle_minimize)
            .on_action(handle_toggle_maximize)
            .on_action(cx.listener(Self::handle_close_tab))
            .on_action(cx.listener(Self::handle_git_fetch))
            .on_action(cx.listener(Self::handle_git_pull))
            .on_action(cx.listener(Self::handle_git_push))
            .on_action(cx.listener(Self::handle_open_settings))
            .on_action(cx.listener(Self::handle_save))
            .on_action(cx.listener(Self::handle_toggle_project_tree))
            .on_action(cx.listener(Self::handle_toggle_panel::<ToggleVersionControl>))
            .on_action(cx.listener(Self::handle_toggle_panel::<ToggleOutline>))
            .on_action(cx.listener(Self::handle_toggle_panel::<ToggleTerminal>))
            .on_action(cx.listener(Self::handle_toggle_panel::<ToggleDebug>))
            .on_action(cx.listener(Self::handle_toggle_panel::<ToggleKeyboardShortcuts>))
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
                    dock.update(cx, |d, _| d.end_resize());
                }
                drag_notify_up.set(None);
                window.refresh();
            })
    }
}
