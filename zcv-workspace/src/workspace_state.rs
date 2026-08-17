//! Workspace —— 工作区实体：Pane/Dock/StatusBar 的装配与命令分发。
//!
//! 对齐 Zed：Workspace 只管理工作区框架与通用命令，
//! 面板、顶栏、状态项与项目相关订阅由宿主（binary 装配层）注入。

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Action, AnyView, App, AsyncApp, Context, Div, Entity, FocusHandle, MouseButton, Render,
    SharedString, Subscription, WeakEntity, Window, div, prelude::*, rems,
};
use zcv_actions::{CloseTab, GitFetch, GitPull, GitPush, OpenSettings, Save};
use zcv_project::{GitOperationKind, Project};
use zcv_theme::{color, typography};

use crate::dock::{Dock, DockPosition, render_body};
use crate::item_provider::item_provider_for_path;
use crate::pane::Pane;
use crate::panel::PanelHandle;
use crate::preview::provider_for;
use crate::status_bar::StatusBar;
use crate::toast::{ToastAction, ToastKind, ToastLayer};
use crate::window_controls::{handle_minimize, handle_quit, handle_toggle_maximize};

/// 支持预览的文件需要等待双击判定，避免双击源码前短暂显示预览。
const FILE_SINGLE_CLICK_DELAY: Duration = Duration::from_millis(300);

/// 打开设置文件的路径提供者：宿主注入，返回设置文件路径。
pub type OpenSettingsPathProvider = Box<dyn Fn(&mut App) -> Option<PathBuf> + Send + Sync>;

pub struct Workspace {
    pub focus: FocusHandle,
    pub pane: Entity<Pane>,
    status_bar: Entity<StatusBar>,
    toast_layer: Entity<ToastLayer>,
    project: Entity<Project>,
    pub left_dock: Entity<Dock>,
    pub right_dock: Entity<Dock>,
    pub bottom_dock: Entity<Dock>,
    /// toggle action → (Dock Entity, panel_index_in_dock) 的查找表。
    /// 条目由各面板自身的 `toggle_action` 派生，快捷键、按钮与分派共用同一来源。
    panel_actions: Vec<(Box<dyn Action>, Entity<Dock>, usize)>,
    /// 拖拽协调：Dock 通过此 Cell 通知 Workspace 哪个 dock 正在被拖拽。
    pub drag_notify: Rc<Cell<Option<DockPosition>>>,
    /// 统一取消来自项目树、变更树等入口的待处理单击打开。
    file_click_generation: u64,
    /// 顶栏视图（对齐 Zed titlebar_item），由宿主注入。
    titlebar: Option<AnyView>,
    /// 打开设置文件的路径提供者（设置文件属于宿主配置，需注入）。
    open_settings_path_provider: Option<OpenSettingsPathProvider>,
    /// 宿主装配的订阅（git/settings/面板等）。
    pub _subscriptions: Vec<Subscription>,
    /// 经 register_action 注册的 action handler（render 时挂到根节点，焦点链全局可达）。
    /// 对齐 Zed `workspace_actions`：组件创建时注册自己的命令 handler。
    workspace_actions: Vec<
        Box<dyn Fn(gpui::Stateful<gpui::Div>, &mut Context<Self>) -> gpui::Stateful<gpui::Div>>,
    >,
}

impl Workspace {
    pub fn new(root: PathBuf, _window: &Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();

        let keybindings = zcv_keymap::load(cx).expect("内置 keymap 应完整有效");
        cx.bind_keys(keybindings.bindings.clone());
        cx.set_global(keybindings);

        let pane = cx.new(Pane::new);
        let project = cx.new(|cx| Project::new(root.clone(), cx));

        // 三个空 Dock；面板由宿主经 register_panel 注册。
        let drag_notify: Rc<Cell<Option<DockPosition>>> = Rc::new(Cell::new(None));
        let left_dock = cx.new(|cx| {
            Dock::new(
                DockPosition::Left,
                Vec::new(),
                DockPosition::Left.default_size(),
                drag_notify.clone(),
                cx,
            )
        });
        let right_dock = cx.new(|cx| {
            Dock::new(
                DockPosition::Right,
                Vec::new(),
                DockPosition::Right.default_size(),
                drag_notify.clone(),
                cx,
            )
        });
        let bottom_dock = cx.new(|cx| {
            Dock::new(
                DockPosition::Bottom,
                Vec::new(),
                DockPosition::Bottom.default_size(),
                drag_notify.clone(),
                cx,
            )
        });
        left_dock.update(cx, |d, _| d.set_sibling(right_dock.downgrade()));
        right_dock.update(cx, |d, _| d.set_sibling(left_dock.downgrade()));

        let status_bar = cx.new(|cx| StatusBar::new(pane.clone(), cx));
        let toast_layer = cx.new(|_| ToastLayer::new());

        Self {
            focus,
            pane,
            status_bar,
            toast_layer,
            project,
            left_dock,
            right_dock,
            bottom_dock,
            panel_actions: Vec::new(),
            drag_notify,
            file_click_generation: 0,
            titlebar: None,
            open_settings_path_provider: None,
            _subscriptions: Vec::new(),
            workspace_actions: Vec::new(),
        }
    }

    /// 展示一条全局提示（成功/错误）；`action` 提供可点击的操作按钮（如"重试"）。
    pub(crate) fn show_toast(
        &self,
        kind: ToastKind,
        message: impl Into<SharedString>,
        action: Option<ToastAction>,
        dismiss_after: Option<Duration>,
        cx: &mut App,
    ) {
        self.toast_layer.update(cx, |toast, cx| {
            toast.show(kind, message, action, dismiss_after, cx)
        });
    }

    // ═══ 访问器与宿主注入 ═════════════════════════════════════════

    pub fn pane(&self) -> &Entity<Pane> {
        &self.pane
    }

    pub fn project(&self) -> &Entity<Project> {
        &self.project
    }

    pub fn status_bar(&self) -> &Entity<StatusBar> {
        &self.status_bar
    }

    /// 注册面板并建立 toggle action 查找表（对齐 Zed register_panel）。
    pub fn register_panel(
        &mut self,
        handle: Arc<dyn PanelHandle>,
        position: DockPosition,
        cx: &mut Context<Self>,
    ) {
        let dock = match position {
            DockPosition::Left => &self.left_dock,
            DockPosition::Right => &self.right_dock,
            DockPosition::Bottom => &self.bottom_dock,
        };
        let dock = dock.clone();
        let idx = dock.read(cx).panels.len();
        dock.update(cx, |dock, cx| dock.add_panel(handle, cx));
        self.panel_actions
            .push((dock.read(cx).panels[idx].toggle_action(cx), dock, idx));
        cx.notify();
    }

    /// 注入顶栏视图（对齐 Zed titlebar_item）。
    pub fn set_titlebar(&mut self, view: AnyView, cx: &mut Context<Self>) {
        self.titlebar = Some(view);
        cx.notify();
    }

    /// 注入打开设置文件的路径提供者。
    pub fn set_open_settings_provider(&mut self, provider: OpenSettingsPathProvider) {
        self.open_settings_path_provider = Some(provider);
    }

    /// 注册 action handler 到 Workspace 根节点（焦点链顶端，快捷键全局可达）。
    ///
    /// 对齐 Zed `Workspace::register_action`：不在主焦点链上的组件（如顶栏选择器）创建时把命令 handler 注册到工作区，由工作区统一派发。
    pub fn register_action<A: Action>(
        &mut self,
        callback: impl Fn(&mut Self, &A, &mut Window, &mut Context<Self>) + 'static,
    ) -> &mut Self {
        let callback = std::sync::Arc::new(callback);
        self.workspace_actions.push(Box::new(move |div, cx| {
            let callback = callback.clone();
            div.on_action(cx.listener(move |workspace, event, window, cx| {
                (callback)(workspace, event, window, cx)
            }))
        }));
        self
    }

    // ═══ 文件打开 ═════════════════════════════════════════════════

    /// 所有文件树入口共享的单击/双击协调逻辑。
    pub fn open_path(
        &mut self,
        path: PathBuf,
        focus_opened_item: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.file_click_generation = self.file_click_generation.wrapping_add(1);
        let generation = self.file_click_generation;

        if focus_opened_item || provider_for(&path, cx).is_none() {
            self.open_path_now(path, focus_opened_item, window, cx);
            return;
        }

        cx.spawn_in(window, async move |workspace, cx| {
            cx.background_executor()
                .timer(FILE_SINGLE_CLICK_DELAY)
                .await;
            workspace
                .update_in(cx, |workspace, window, cx| {
                    if workspace.file_click_generation == generation {
                        workspace.open_path_now(path, false, window, cx);
                    }
                })
                .ok();
        })
        .detach();
    }

    /// 已完成点击判定后的实际文件打开流程：经 ItemProvider 注册表创建 Item。
    fn open_path_now(
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
        let Some(provider) = item_provider_for_path(&path, cx) else {
            eprintln!(
                "打开文件失败：{}：没有支持该类型的 Item Provider",
                path.display()
            );
            return;
        };
        let project = self.project.clone();
        let pane = self.pane.clone();
        let task = provider.open_item(path.clone(), project, cx);
        cx.spawn_in(window, async move |workspace, cx| {
            let item = match task.await {
                Ok(item) => item,
                Err(error) => {
                    eprintln!("打开文件失败：{}：{error}", path.display());
                    return;
                }
            };
            workspace
                .update_in(cx, |_workspace, window, cx| {
                    let focus = pane.update(cx, |pane, cx| {
                        pane.open_item(item, !focus_opened_item, window, cx)
                    });
                    if focus_opened_item {
                        window.focus(&focus);
                    }
                    window.refresh();
                })
                .ok();
        })
        .detach();
    }

    // ═══ 文件与路径操作 ═══════════════════════════════════════════

    pub fn rename_path(
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

    pub fn create_path(
        &mut self,
        path: &Path,
        is_dir: bool,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        self.project
            .update(cx, |project, cx| project.create_path(path, is_dir, cx))
    }

    /// 将文件或目录移到系统废纸篓，并关闭打开它的标签页。
    pub fn trash_path(
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

    // ═══ 命令分发 ═════════════════════════════════════════════════

    /// 聚焦回编辑区。
    fn focus_center_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.pane.read(cx);
        if let Some(item) = pane.active_item() {
            window.focus(&item.item_focus_handle(cx));
        } else {
            window.focus(&pane.focus);
        }
    }

    fn handle_close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
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

    /// 通用面板 toggle：已聚焦时再按关闭并回到编辑区；可见未聚焦时先聚焦（Zed 语义）。
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
        let focus = dock.read(cx).panels[panel_idx].focus_handle(cx);

        if focus.contains_focused(window, cx) {
            dock.update(cx, |d, cx| d.toggle_panel(panel_idx, window, cx));
            self.focus_center_pane(window, cx);
        } else {
            if !dock.read(cx).is_panel_active(panel_idx) {
                dock.update(cx, |d, cx| d.toggle_panel(panel_idx, window, cx));
            }
            window.focus(&focus);
        }
        window.refresh();
    }

    fn handle_save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.pane.clone();
        let Some(item) = pane.read(cx).active_item() else {
            return;
        };
        if !item.can_save(cx) {
            return;
        }
        let project = self.project.clone();
        let task = item.boxed_clone().save(project, window, cx);
        cx.spawn(|this: WeakEntity<Self>, asynccx: &mut AsyncApp| {
            let mut cx = asynccx.clone();
            async move {
                if let Err(error) = task.await {
                    eprintln!("保存文件失败：{error}");
                    if let Some(this) = this.upgrade() {
                        this.update(&mut cx, |workspace, cx| {
                            workspace.show_toast(
                                ToastKind::Error,
                                format!("保存文件失败：{error}"),
                                None,
                                Some(Duration::from_secs(5)),
                                cx,
                            );
                        })
                        .ok();
                    }
                }
            }
        })
        .detach();
    }

    fn handle_open_settings(
        &mut self,
        _: &OpenSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(provider) = &self.open_settings_path_provider else {
            return;
        };
        let Some(path) = provider(cx) else {
            return;
        };
        self.open_path(path, true, window, cx);
    }

    /// 后台执行 git 操作（fetch/pull/push）：等待结果后直接弹提示（成功/失败+错误详情）。
    fn run_git_operation(&mut self, operation: GitOperationKind, cx: &mut Context<Self>) {
        let git_store = self.project.read(cx).git_store();
        let task = git_store.update(cx, |store, cx| store.run_operation(operation, cx));
        let name = match operation {
            GitOperationKind::Fetch => "拉取",
            GitOperationKind::Pull => "合并拉取",
            GitOperationKind::Push => "推送",
        };
        cx.spawn(move |this: WeakEntity<Self>, asynccx: &mut AsyncApp| {
            let mut cx = asynccx.clone();
            async move {
                let (kind, message, action) = match task.await {
                    Ok(()) => (ToastKind::Success, format!("{name}完成"), None),
                    Err(error) => {
                        // 失败提示带重试按钮：点击重新执行同一操作（弱引用，不持有 Workspace）。
                        let weak = this.clone();
                        (
                            ToastKind::Error,
                            format!("{name}失败：{error:#}"),
                            Some(ToastAction::new("重试", move |_window, cx| {
                                if let Some(workspace) = weak.upgrade() {
                                    // App 上下文的 Entity::update 直接返回闭包结果（实体经 upgrade 已确认存在），无 Result 包装。
                                    workspace.update(cx, |workspace, cx| {
                                        workspace.run_git_operation(operation, cx);
                                    });
                                }
                            })),
                        )
                    }
                };
                if let Some(this) = this.upgrade() {
                    this.update(&mut cx, |workspace, cx| {
                        workspace.show_toast(
                            kind,
                            message,
                            action,
                            Some(Duration::from_secs(5)),
                            cx,
                        );
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    fn handle_git_fetch(&mut self, _: &GitFetch, _: &mut Window, cx: &mut Context<Self>) {
        self.run_git_operation(GitOperationKind::Fetch, cx);
    }

    fn handle_git_pull(&mut self, _: &GitPull, _: &mut Window, cx: &mut Context<Self>) {
        self.run_git_operation(GitOperationKind::Pull, cx);
    }

    fn handle_git_push(&mut self, _: &GitPush, _: &mut Window, cx: &mut Context<Self>) {
        self.run_git_operation(GitOperationKind::Push, cx);
    }
}

// ═══ 渲染 ═════════════════════════════════════════════════════════

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

        let titlebar = self.titlebar.as_ref();

        let mut root = div()
            .id("app-view")
            // 全局字号经 window rem 基准设置（open_window 时 set_rem_size）；
            // 字体在此设置，行高 = 1rem（与字号同源，文字块上下 padding 与左右完全对称）；
            // 全树（含挂载在根下的 toast）继承，子元素不再重复设置（需要其他字号时显式覆盖）。
            .font(typography::ui_font())
            .line_height(rems(1.0))
            .track_focus(&self.focus)
            .key_context("Workspace")
            .size_full()
            .relative();
        for action in &self.workspace_actions {
            root = action(root, cx);
        }
        root.child(render_frame(
            titlebar,
            &self.status_bar,
            render_body(&pane, left_dock, right_dock, bottom_dock),
            cx,
        ))
        .child(self.toast_layer.clone())
        .on_action(handle_quit)
        .on_action(handle_minimize)
        .on_action(handle_toggle_maximize)
        .on_action(cx.listener(Self::handle_close_tab))
        .on_action(cx.listener(Self::handle_git_fetch))
        .on_action(cx.listener(Self::handle_git_pull))
        .on_action(cx.listener(Self::handle_git_push))
        .on_action(cx.listener(Self::handle_open_settings))
        .on_action(cx.listener(Self::handle_save))
        .on_action(cx.listener(Self::handle_toggle_panel::<crate::dock::ToggleProjectTree>))
        .on_action(cx.listener(Self::handle_toggle_panel::<crate::dock::ToggleVersionControl>))
        .on_action(cx.listener(Self::handle_toggle_panel::<crate::dock::ToggleOutline>))
        .on_action(cx.listener(Self::handle_toggle_panel::<crate::dock::ToggleTerminal>))
        .on_action(cx.listener(Self::handle_toggle_panel::<crate::dock::ToggleDebug>))
        .on_action(cx.listener(Self::handle_toggle_panel::<crate::dock::ToggleKeyboardShortcuts>))
        .on_action(cx.listener(Self::handle_toggle_panel::<crate::dock::ToggleLanguageServer>))
        .on_action(cx.listener(Self::handle_toggle_panel::<crate::dock::ToggleDiagnostics>))
        .on_action(cx.listener(Self::handle_toggle_panel::<crate::dock::ToggleProjectSearch>))
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

/// 工作台顶层框架组装。
fn render_frame(
    titlebar: Option<&AnyView>,
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
        .text_color(color::current(cx).text)
        .child(
            titlebar
                .map(|view| view.clone().into_any_element())
                .unwrap_or_else(|| gpui::div().into_any_element()),
        )
        .child(body)
        .child(status_bar.clone())
}
