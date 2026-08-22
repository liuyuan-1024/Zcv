//! Workspace —— 工作区实体：Pane/Dock/StatusBar 的装配与命令分发。
//!
//! 对齐 Zed：Workspace 只管理工作区框架与通用命令，
//! 面板、顶栏、状态项与项目相关订阅由宿主（binary 装配层）注入。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Action, AnyView, App, AsyncApp, Context, Div, DragMoveEvent, Entity, FocusHandle, Focusable,
    Render, SharedString, Subscription, Task, WeakEntity, Window, div, prelude::*, rems,
};
use zcv_actions::{
    FocusOrHidePanel, OpenSettings, QuitWindow, Save, ToggleBottomDock, ToggleLeftDock,
    ToggleRightDock,
};
use zcv_project::Project;
use zcv_theme::{color, typography};

use crate::ItemHandle;
use crate::dock::{Dock, DockEvent, DockPosition, DockStructure, DraggedDock, render_body};
use crate::item_provider::item_provider_for_path;
use crate::layout_state::{self, PanelState, SerializedPane, WorkspaceLayout};
use crate::pane::{Pane, PaneEvent};
use crate::panel::PanelHandle;
use crate::preview::provider_for;
use crate::status_bar::StatusBar;
use crate::toast::{ToastAction, ToastKind, ToastLayer};
use crate::window_bounds;
use crate::window_controls::{handle_minimize, handle_toggle_maximize};

const LAYOUT_SAVE_THROTTLE: Duration = Duration::from_millis(200);

/// 支持预览的文件需要等待双击判定，避免双击源码前短暂显示预览。
const FILE_SINGLE_CLICK_DELAY: Duration = Duration::from_millis(300);

/// 打开设置文件的路径提供者：宿主注入，返回设置文件路径。
pub type OpenSettingsPathProvider = Box<dyn Fn(&mut App) -> Option<PathBuf> + Send + Sync>;

type WorkspaceAction =
    Box<dyn Fn(gpui::Stateful<gpui::Div>, &mut Context<Workspace>) -> gpui::Stateful<gpui::Div>>;

pub struct Workspace {
    pub focus: FocusHandle,
    pub pane: Entity<Pane>,
    status_bar: Entity<StatusBar>,
    toast_layer: Entity<ToastLayer>,
    project: Entity<Project>,
    pub left_dock: Entity<Dock>,
    pub right_dock: Entity<Dock>,
    pub bottom_dock: Entity<Dock>,
    /// 统一取消来自项目树、变更树等入口的待处理单击打开。
    file_click_generation: u64,
    /// 顶栏视图（对齐 Zed titlebar_item），由宿主注入。
    titlebar: Option<AnyView>,
    /// 打开设置文件的路径提供者（设置文件属于宿主配置，需注入）。
    open_settings_path_provider: Option<OpenSettingsPathProvider>,
    /// 宿主装配的订阅（git/settings/面板等）。
    _subscriptions: Vec<Subscription>,
    /// 经 register_action 注册的 action handler（render 时挂到根节点，焦点链全局可达）。
    /// 对齐 Zed `workspace_actions`：组件创建时注册自己的命令 handler。
    workspace_actions: Vec<WorkspaceAction>,
    layout_path: PathBuf,
    _layout_save_task: Option<Task<()>>,
}

impl Workspace {
    /// 登记宿主装配的订阅（git/settings/面板等）。
    pub fn add_subscription(&mut self, sub: Subscription) {
        self._subscriptions.push(sub);
    }

    pub fn new(root: PathBuf, window: &Window, cx: &mut Context<Self>) -> Self {
        let project = cx.new(|cx| Project::new(root, cx));
        Self::build(project, window, cx)
    }

    /// 创建不绑定项目目录的工作区。
    pub fn new_empty(window: &Window, cx: &mut Context<Self>) -> Self {
        let project = cx.new(Project::empty);
        Self::build(project, window, cx)
    }

    fn build(project: Entity<Project>, window: &Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();

        let keybindings = zcv_keymap::load(cx).expect("内置 keymap 应完整有效");
        cx.bind_keys(keybindings.bindings.clone());
        cx.set_global(keybindings);

        let pane = cx.new(Pane::new);
        let layout_path = layout_state::path_for_workspace(project.read(cx).root());
        let restored_layout = layout_state::load(&layout_path).unwrap_or_default();
        let restored_pane = restored_layout.pane.clone();
        let restored_panels = restored_layout.panels.clone();
        // 标签变化时节流保存布局（订阅带 window：调度保存需要 window）。
        let pane_for_layout = pane.clone();
        let layout_subscription =
            cx.subscribe_in(&pane_for_layout, window, |this, _, event, window, cx| {
                if matches!(
                    event,
                    PaneEvent::AddItem { .. }
                        | PaneEvent::ActivateItem { .. }
                        | PaneEvent::RemovedItem { .. }
                ) {
                    this.schedule_layout_save(window, cx);
                }
            });
        // 恢复标签与面板状态延后到首帧后：此时面板装配与 Item Provider 已就绪。
        let restored_for_defer = (restored_pane.clone(), restored_panels.clone());
        window.defer(cx, move |window, cx| {
            let Some(Some(workspace)) = window.root::<Workspace>() else {
                return;
            };
            workspace.update(cx, |workspace, cx| {
                workspace.restore_pane(&restored_for_defer.0, window, cx);
                workspace.restore_panels(&restored_for_defer.1, window, cx);
            });
        });
        // 三个空 Dock；面板由宿主经 register_panel 注册。
        let left_dock = cx.new(|cx| {
            Dock::new(
                DockPosition::Left,
                Vec::new(),
                DockPosition::Left.default_size(),
                Some(restored_layout.docks.left.clone()),
                cx,
            )
        });
        let right_dock = cx.new(|cx| {
            Dock::new(
                DockPosition::Right,
                Vec::new(),
                DockPosition::Right.default_size(),
                Some(restored_layout.docks.right.clone()),
                cx,
            )
        });
        let bottom_dock = cx.new(|cx| {
            Dock::new(
                DockPosition::Bottom,
                Vec::new(),
                DockPosition::Bottom.default_size(),
                Some(restored_layout.docks.bottom.clone()),
                cx,
            )
        });
        left_dock.update(cx, |d, _| d.set_sibling(right_dock.downgrade()));
        right_dock.update(cx, |d, _| d.set_sibling(left_dock.downgrade()));

        // dock 开合变化时节流保存布局（覆盖所有打开/折叠路径）。
        let dock_layout_subscriptions: Vec<Subscription> = [&left_dock, &right_dock, &bottom_dock]
            .iter()
            .map(|dock| {
                cx.subscribe_in(dock, window, |this, _, _: &DockEvent, window, cx| {
                    this.schedule_layout_save(window, cx);
                })
            })
            .collect();

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
            file_click_generation: 0,
            titlebar: None,
            open_settings_path_provider: None,
            _subscriptions: std::iter::once(layout_subscription)
                .chain(dock_layout_subscriptions)
                .collect(),
            workspace_actions: Vec::new(),
            layout_path,
            _layout_save_task: None,
        }
    }

    /// 展示一条全局提示（成功/错误）；`action` 提供可点击的操作按钮（如"重试"）。
    /// 宿主（装配层）经它呈现 git 操作等产品级反馈。
    pub fn show_toast(
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

    /// 捕获三个 Dock 的当前布局状态。
    fn capture_dock_state(&self, cx: &App) -> DockStructure {
        DockStructure {
            left: self.left_dock.read(cx).capture_state(),
            right: self.right_dock.read(cx).capture_state(),
            bottom: self.bottom_dock.read(cx).capture_state(),
        }
    }

    /// 节流调度一次布局保存（宿主装配层响应面板事件时调用）。
    pub fn schedule_layout_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self._layout_save_task.is_some() {
            return;
        }
        self._layout_save_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(LAYOUT_SAVE_THROTTLE).await;
            this.update_in(cx, |this, _window, cx| {
                this._layout_save_task.take();
                let layout = this.capture_layout(cx);
                if let Err(error) = layout_state::save(&this.layout_path, &layout) {
                    log::warn!("保存工作区布局失败：{error:#}");
                }
            })
            .ok();
        }));
    }

    /// 立即落盘布局状态；供宿主在替换工作区根之前冲刷节流中的保存（节流任务随旧根销毁而丢失）。
    pub fn flush_layout(&mut self, cx: &App) {
        self._layout_save_task.take();
        let layout = self.capture_layout(cx);
        if let Err(error) = layout_state::save(&self.layout_path, &layout) {
            log::warn!("保存工作区布局失败：{error:#}");
        }
    }

    /// 捕获完整布局快照：dock 状态 + 中心 Pane 标签 + 各面板自持状态。
    fn capture_layout(&self, cx: &App) -> WorkspaceLayout {
        let panels = [&self.left_dock, &self.right_dock, &self.bottom_dock]
            .iter()
            .flat_map(|dock| dock.read(cx).panels())
            .filter_map(|panel| {
                panel.serialized_state(cx).map(|data| PanelState {
                    name: panel.persistent_name().to_string(),
                    data,
                })
            })
            .collect();
        WorkspaceLayout {
            version: layout_state::LAYOUT_VERSION,
            docks: self.capture_dock_state(cx),
            pane: self.pane.read(cx).serialized(cx),
            panels,
        }
    }

    /// 注册面板（对齐 Zed register_panel）。
    pub fn register_panel(
        &mut self,
        handle: Arc<dyn PanelHandle>,
        position: DockPosition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dock = match position {
            DockPosition::Left => &self.left_dock,
            DockPosition::Right => &self.right_dock,
            DockPosition::Bottom => &self.bottom_dock,
        };
        dock.update(cx, |dock, cx| dock.add_panel(handle, window, cx));
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

    /// 恢复持久化的标签：按保存顺序固定打开，无路径/失效路径跳过，最后激活保存的位置。
    /// 与 open_path 的单击语义无关：固定打开不走临时标签流程。
    fn restore_pane(
        &mut self,
        state: &SerializedPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = self.project.clone();
        let pane = self.pane.clone();
        let tasks: Vec<gpui::Task<anyhow::Result<Box<dyn ItemHandle>>>> = state
            .items
            .iter()
            .filter_map(|path| {
                let path = path.canonicalize().ok()?;
                let provider = item_provider_for_path(&path, cx)?;
                Some(provider.open_item(path, project.clone(), cx))
            })
            .collect();
        let active_index = state.active_item;
        cx.spawn_in(window, async move |_workspace, cx| {
            for task in tasks {
                if let Ok(item) = task.await {
                    _workspace
                        .update_in(cx, |_workspace, window, cx| {
                            pane.update(cx, |pane, cx| pane.open_item(item, false, window, cx));
                        })
                        .ok();
                }
            }
            // 恢复活动标签（打开顺序与保存顺序一致，索引可直接映射）。
            if let Some(index) = active_index {
                _workspace
                    .update_in(cx, |_workspace, window, cx| {
                        let Some(item) = pane.read(cx).tabs().get(index) else {
                            return;
                        };
                        let item_id = item.item_id();
                        pane.update(cx, |pane, cx| {
                            pane.activate_tab(item_id, window, cx);
                        });
                    })
                    .ok();
            }
        })
        .detach();
    }

    /// 按持久化标识向各面板分发自持状态（找不到对应面板时跳过）。
    fn restore_panels(
        &mut self,
        states: &[PanelState],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let all_panels: Vec<Arc<dyn PanelHandle>> =
            [&self.left_dock, &self.right_dock, &self.bottom_dock]
                .iter()
                .flat_map(|dock| dock.read(cx).panels().iter().cloned())
                .collect();
        for state in states {
            if let Some(panel) = all_panels
                .iter()
                .find(|p| p.persistent_name() == state.name)
            {
                panel.restore_state(state.data.clone(), window, cx);
            }
        }
    }

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
            window.focus(&pane.focus_handle());
        }
    }

    /// Panel 键盘命令：已聚焦时关闭；可见未聚焦时聚焦；隐藏时显示并聚焦。
    fn handle_panel_keyboard_action(
        &mut self,
        action: &FocusOrHidePanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = [&self.left_dock, &self.bottom_dock, &self.right_dock]
            .into_iter()
            .find_map(|dock| {
                dock.read(cx)
                    .panel_index_for_persistent_name(&action.panel)
                    .map(|panel_index| (dock.clone(), panel_index))
            });
        let Some((dock, panel_idx)) = target else {
            return;
        };
        let focus = dock.read(cx).panel_focus_handle(panel_idx, cx).unwrap();

        if focus.contains_focused(window, cx) {
            dock.update(cx, |d, cx| d.toggle_panel_visibility(panel_idx, window, cx));
            self.focus_center_pane(window, cx);
        } else {
            if !dock.read(cx).is_panel_active(panel_idx) {
                dock.update(cx, |d, cx| d.toggle_panel_visibility(panel_idx, window, cx));
            }
            window.focus(&focus);
        }
        window.refresh();
    }

    /// 处理 Glyph 的鼠标意图：只切换指定 Panel 的可见性。
    pub(crate) fn toggle_panel_visibility_from_button(
        &mut self,
        position: DockPosition,
        panel_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dock = match position {
            DockPosition::Left => self.left_dock.clone(),
            DockPosition::Bottom => self.bottom_dock.clone(),
            DockPosition::Right => self.right_dock.clone(),
        };
        let was_visible = dock.read(cx).is_panel_active(panel_index);
        let panel_focus = dock.read(cx).panel_focus_handle(panel_index, cx);
        if panel_focus.is_none() {
            return;
        }

        dock.update(cx, |dock, cx| {
            dock.toggle_panel_visibility(panel_index, window, cx)
        });
        if was_visible {
            self.focus_center_pane(window, cx);
        } else if let Some(panel_focus) = panel_focus {
            window.focus(&panel_focus);
        }
        window.refresh();
    }

    fn toggle_dock(&mut self, position: DockPosition, window: &mut Window, cx: &mut Context<Self>) {
        let dock = match position {
            DockPosition::Left => self.left_dock.clone(),
            DockPosition::Bottom => self.bottom_dock.clone(),
            DockPosition::Right => self.right_dock.clone(),
        };
        let was_open = dock.read(cx).is_open();
        let panel_focus = dock
            .read(cx)
            .active_panel()
            .map(|panel| panel.focus_handle(cx));
        let focus_center = was_open
            && (dock.read(cx).focus_handle(cx).contains_focused(window, cx)
                || panel_focus
                    .as_ref()
                    .is_some_and(|focus| focus.contains_focused(window, cx)));

        dock.update(cx, |dock, cx| dock.set_open(!was_open, window, cx));
        if focus_center {
            self.focus_center_pane(window, cx);
        } else if !was_open && let Some(focus) = panel_focus {
            window.focus(&focus);
        }
        window.refresh();
    }

    fn handle_quit(&mut self, _: &QuitWindow, window: &mut Window, cx: &mut Context<Self>) {
        window_bounds::save_window_bounds(window, cx);
        self.flush_layout(cx);
        cx.quit();
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
}

// ═══ 渲染 ═════════════════════════════════════════════════════════

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = self.pane.clone();

        let left_dock = self.left_dock.clone();
        let right_dock = self.right_dock.clone();
        let bottom_dock = self.bottom_dock.clone();

        let left_dock_entity = self.left_dock.clone();
        let right_dock_entity = self.right_dock.clone();
        let bottom_dock_entity = self.bottom_dock.clone();
        let workspace_after_resize = cx.entity().downgrade();

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
            &self.toast_layer,
            cx,
        ))
        .on_action(cx.listener(Self::handle_quit))
        .on_action(handle_minimize)
        .on_action(handle_toggle_maximize)
        .on_action(cx.listener(Self::handle_open_settings))
        .on_action(cx.listener(Self::handle_save))
        .on_action(cx.listener(|this, _: &ToggleLeftDock, window, cx| {
            this.toggle_dock(DockPosition::Left, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ToggleBottomDock, window, cx| {
            this.toggle_dock(DockPosition::Bottom, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ToggleRightDock, window, cx| {
            this.toggle_dock(DockPosition::Right, window, cx)
        }))
        .on_action(cx.listener(Self::handle_panel_keyboard_action))
        // dock 尺寸拖拽：手势由 Dock 的 resize handle 发起（on_drag 承载 DraggedDock），这里在根节点接收拖动事件并按位置驱动对应 dock 尺寸；布局保存走既有节流。
        .on_drag_move(move |event: &DragMoveEvent<DraggedDock>, window, cx| {
            let area = event.drag(cx).0;
            let dock = match area {
                DockPosition::Left => &left_dock_entity,
                DockPosition::Right => &right_dock_entity,
                DockPosition::Bottom => &bottom_dock_entity,
            };
            dock.update(cx, |dock, cx| {
                dock.resize_to(event.event.position, event.bounds, cx);
            });
            workspace_after_resize
                .update(cx, |workspace, cx| {
                    workspace.schedule_layout_save(window, cx)
                })
                .ok();
            window.refresh();
        })
    }
}

/// 工作台顶层框架组装。
fn render_frame(
    titlebar: Option<&AnyView>,
    status_bar: &Entity<StatusBar>,
    body: gpui::Div,
    toast_layer: &Entity<ToastLayer>,
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
        // Toast 只覆盖主工作区，以主工作区底边为基准，始终位于状态栏上方。
        .child(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .child(body)
                .child(toast_layer.clone()),
        )
        .child(status_bar.clone())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui::{
        App, AppContext, Context, FocusHandle, Render, TestAppContext, Window, div, prelude::*,
    };

    use super::{DockPosition, LAYOUT_SAVE_THROTTLE, Workspace};
    use crate::DockData;
    use crate::panel::PanelEvent;
    use crate::{FocusOrHidePanel, Panel, PanelHandle};
    use gpui::EventEmitter;

    struct TestPanel {
        focus: FocusHandle,
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
    }

    impl Render for TestPanel {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().track_focus(&self.focus)
        }
    }

    #[gpui::test]
    fn empty_workspace_has_a_project_without_a_worktree(cx: &mut TestAppContext) {
        let (workspace, cx) = cx.add_window_view(|window, cx| Workspace::new_empty(window, cx));
        let project = cx.read_entity(&workspace, |workspace, _| workspace.project().clone());
        assert!(!cx.read_entity(&project, |project, _| project.has_worktree()));
    }

    /// 回归：序列化 visible=true 的 dock 随面板注册恢复打开（重启不展开问题）。
    #[gpui::test]
    fn workspace_restores_visible_dock(cx: &mut TestAppContext) {
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::new_empty(window, cx);
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
            });
            let handle: Arc<dyn PanelHandle> = Arc::new(panel);
            workspace.register_panel(handle, DockPosition::Bottom, window, cx);
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
        let directory = tempfile::tempdir().unwrap();
        let layout_path = directory.path().join("layout.json");
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::new_empty(window, cx);
            let panel = cx.new(|cx| TestPanel {
                focus: cx.focus_handle(),
            });
            let handle: Arc<dyn PanelHandle> = Arc::new(panel);
            workspace.register_panel(handle, DockPosition::Bottom, window, cx);
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

        let saved = crate::layout_state::load(&layout_path).expect("应保存布局快照");
        assert!(saved.docks.bottom.visible, "dock 开合应触发保存");
    }

    #[gpui::test]
    fn closed_dock_stays_in_action_lifecycle_and_can_reopen(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let layout_path = directory.path().join("layout.json");
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::new_empty(window, cx);
            let panel = cx.new(|cx| TestPanel {
                focus: cx.focus_handle(),
            });
            let handle: Arc<dyn PanelHandle> = Arc::new(panel);
            workspace.register_panel(handle, DockPosition::Left, window, cx);
            workspace
        });
        workspace.update(cx, |workspace, _| {
            workspace.layout_path = layout_path.clone();
        });
        let focus = cx.read_entity(&workspace, |workspace, _| workspace.focus.clone());
        cx.update(|window, _| window.focus(&focus));

        cx.dispatch_action(FocusOrHidePanel::new("test-panel"));
        assert!(cx.read_entity(&workspace, |workspace, cx| {
            workspace.left_dock.read(cx).is_open()
        }));
        cx.update(|window, cx| {
            let panel_focus = workspace
                .read(cx)
                .left_dock
                .read(cx)
                .active_panel()
                .unwrap()
                .focus_handle(cx);
            assert!(panel_focus.contains_focused(window, cx));
        });

        // 快捷键：panel 可见但未聚焦时，只聚焦，不隐藏。
        let center_focus = cx.read_entity(&workspace, |workspace, cx| {
            workspace.pane.read(cx).focus_handle()
        });
        cx.update(|window, _| window.focus(&center_focus));
        cx.dispatch_action(FocusOrHidePanel::new("test-panel"));
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
        assert!(cx.read_entity(&workspace, |workspace, cx| {
            workspace.left_dock.read(cx).is_open()
        }));

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_panel_visibility_from_button(DockPosition::Left, 0, window, cx);
        });
        assert!(!cx.read_entity(&workspace, |workspace, cx| {
            workspace.left_dock.read(cx).is_open()
        }));

        cx.dispatch_action(FocusOrHidePanel::new("test-panel"));
        assert!(cx.read_entity(&workspace, |workspace, cx| {
            workspace.left_dock.read(cx).is_open()
        }));

        cx.executor().advance_clock(LAYOUT_SAVE_THROTTLE);
        cx.run_until_parked();
        let saved = crate::layout_state::load(&layout_path).expect("应保存布局快照");
        assert!(saved.docks.left.visible);
        assert_eq!(saved.docks.left.active_panel.as_deref(), Some("test-panel"));
    }
}
