//! 布局系统 —— Dock + 单一中心 Pane。
//!
//! 布局控制分两类区域：
//!
//! - **Dock**（左/右/底）：可折叠，同一时间一个 panel 可见，用 PanelStack 切换。
//!   每个 Dock 是独立 Entity，参考 Zed `crates/workspace/src/dock.rs`。
//! - **中心编辑区**：单一 Pane。

use std::sync::Arc;

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton, Pixels, Point, Render,
    Subscription, WeakEntity, Window, deferred, div, prelude::*, px,
};
use serde::{Deserialize, Serialize};
use zcv_theme::{color, space};

use crate::pane::Pane;
use crate::panel::PanelHandle;

/// Dock 对外发出的事件。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockEvent {
    /// 开合状态实际变化（折叠/展开）。
    OpenChanged,
    /// 尺寸被重置/调整（双击手柄重置、程序化调整）。
    SizeChanged,
}

impl EventEmitter<DockEvent> for Dock {}

// ═══ 类型定义 ═══════════════════════════════════════════════════

/// Dock 位置，对应 Zed `DockPosition`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockPosition {
    Left,
    Bottom,
    Right,
}

/// 可持久化的单个 Dock 状态。panel 使用稳定名称，不依赖注册顺序。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DockData {
    pub visible: bool,
    pub active_panel: Option<String>,
    pub size: Option<f32>,
}

/// 工作区三个 Dock 的持久化快照。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DockStructure {
    pub left: DockData,
    pub right: DockData,
    pub bottom: DockData,
}

/// 未恢复持久化状态时采用的 Dock 默认尺寸：侧向按宽度、底部按高度。
const DEFAULT_SIDE_SIZE: Pixels = px(240.0);
const DEFAULT_BOTTOM_SIZE: Pixels = px(227.0);

impl DockPosition {
    /// 未恢复持久化状态时采用的 Dock 单一默认尺寸。
    pub fn default_size(self) -> Pixels {
        match self {
            Self::Left | Self::Right => DEFAULT_SIDE_SIZE,
            Self::Bottom => DEFAULT_BOTTOM_SIZE,
        }
    }
}

/// dock 和编辑区的最小尺寸，防止 dock 拖拽完全挤占编辑区。
const MIN_SIZE: Pixels = space::S16;

/// 拖拽调整尺寸的浮层实体；gpui 拖拽系统经它携带 dock 位置信息，
/// Workspace 根节点在 `on_drag_move` 中按位置驱动对应 dock 的尺寸。
pub(crate) struct DraggedDock(pub(crate) DockPosition);

impl Render for DraggedDock {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

// ═══ Dock Entity ═════════════════════════════════════════════════

/// Dock 容器：相邻窗口边缘，可折叠，同一时间只显示一个 panel。
///
/// 参考 Zed `crates/workspace/src/dock.rs` 中的 Dock 设计。
pub struct Dock {
    position: DockPosition,
    is_open: bool,
    size: Pixels,
    active_panel_index: Option<usize>,
    panels: Vec<Arc<dyn PanelHandle>>,
    /// 允许布局先于具体 panel 注册载入；每次 add_panel 后重试恢复。
    serialized_dock: Option<DockData>,
    focus: FocusHandle,
    /// 左右 dock 互为 sibling，拖拽时协调尺寸。
    sibling: Option<WeakEntity<Dock>>,
    /// 生命周期相关订阅。
    subscriptions: Vec<Subscription>,
}

impl Dock {
    /// 创建一个新 Dock，默认折叠。
    pub fn new(
        position: DockPosition,
        panels: Vec<Arc<dyn PanelHandle>>,
        initial_size: Pixels,
        serialized_dock: Option<DockData>,
        cx: &mut Context<Self>,
    ) -> Self {
        let active_panel_index = if panels.is_empty() { None } else { Some(0) };
        Self {
            position,
            is_open: false,
            size: initial_size,
            active_panel_index,
            panels,
            serialized_dock,
            focus: cx.focus_handle(),
            sibling: None,
            subscriptions: Vec::new(),
        }
    }

    /// 设置 sibling dock（左右耦合）。
    pub fn set_sibling(&mut self, sibling: WeakEntity<Dock>) {
        self.sibling = Some(sibling);
    }

    /// 追加面板；空 dock 自动激活第一个面板。
    pub fn add_panel(
        &mut self,
        handle: Arc<dyn PanelHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_panel_index.is_none() {
            self.active_panel_index = Some(0);
        }
        self.panels.push(handle);
        self.restore_state(window, cx);
        cx.notify();
    }

    /// 登记生命周期订阅（宿主装配层使用）。
    pub fn add_subscription(&mut self, sub: Subscription) {
        self.subscriptions.push(sub);
    }

    // ── 状态查询 ─────────────────────────────────────────────────

    /// Dock 是否展开且含有激活面板。
    /// Dock 所在位置。
    pub fn position(&self) -> DockPosition {
        self.position
    }

    /// 面板总数。
    pub fn panel_count(&self) -> usize {
        self.panels.len()
    }

    /// 按持久化名称查找面板 index。
    pub fn panel_index_for_persistent_name(&self, name: &str) -> Option<usize> {
        self.panels
            .iter()
            .position(|panel| panel.persistent_name() == name)
    }

    /// 指定 index 面板的焦点句柄。
    pub fn panel_focus_handle(&self, panel_index: usize, cx: &App) -> Option<FocusHandle> {
        self.panels
            .get(panel_index)
            .map(|panel| panel.focus_handle(cx))
    }

    /// 全部面板（只读）。
    pub fn panels(&self) -> &[Arc<dyn PanelHandle>] {
        &self.panels
    }

    pub fn is_open(&self) -> bool {
        self.is_open && self.active_panel_index.is_some()
    }

    /// 当前可见面板。
    pub fn visible_panel(&self) -> Option<&Arc<dyn PanelHandle>> {
        if self.is_open() {
            self.active_panel_index.and_then(|i| self.panels.get(i))
        } else {
            None
        }
    }

    /// 当前选中的 panel；Dock 关闭时仍返回，供恢复与 action 使用。
    pub fn active_panel(&self) -> Option<&Arc<dyn PanelHandle>> {
        self.active_panel_index.and_then(|i| self.panels.get(i))
    }

    /// 当前激活面板的 index。
    pub fn active_panel_index(&self) -> Option<usize> {
        self.active_panel_index
    }

    /// 指定 index 的面板是否激活（展开且为当前面板）。
    pub fn is_panel_active(&self, panel_index: usize) -> bool {
        self.is_open && Some(panel_index) == self.active_panel_index
    }

    /// 设置开关状态，并统一触发 panel 生命周期回调；状态实际变化时发出事件（宿主据此保存布局）。
    pub fn set_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_open == open {
            return;
        }
        self.is_open = open;
        if let Some(panel) = self.active_panel().cloned() {
            panel.set_active(open, window, cx);
        }
        // 折叠时若焦点在 Dock 内（面板或面板内 item），回落到 Dock 自身句柄（根容器始终挂载，焦点链与 Dock 快捷键保持有效）。
        if !open && self.has_focus(window, cx) {
            window.focus(&self.focus);
        }
        cx.emit(DockEvent::OpenChanged);
        cx.notify();
    }

    /// Dock 自身或其激活面板是否持有焦点（决定折叠时是否归还焦点）。
    fn has_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus.is_focused(window)
            || self
                .active_panel()
                .is_some_and(|panel| panel.focus_handle(cx).contains_focused(window, cx))
    }

    /// 切换当前 panel，并统一停用旧 panel、激活新 panel。
    pub fn activate_panel(
        &mut self,
        panel_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if panel_index >= self.panels.len() || self.active_panel_index == Some(panel_index) {
            return false;
        }
        if let Some(old_panel) = self.active_panel().cloned() {
            old_panel.set_active(false, window, cx);
        }
        self.active_panel_index = Some(panel_index);
        if let Some(new_panel) = self.active_panel().cloned() {
            new_panel.set_active(true, window, cx);
        }
        cx.notify();
        true
    }

    /// 注入待恢复状态；panel 尚未注册时保留快照，后续 add_panel 会继续恢复。
    pub fn set_serialized_state(
        &mut self,
        state: DockData,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.serialized_dock = Some(state);
        self.restore_state(window, cx);
    }

    /// 根据当前已注册 panel 尝试恢复状态。
    pub fn restore_state(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(state) = self.serialized_dock.clone() else {
            return false;
        };
        if let Some(size) = state.size.filter(|size| size.is_finite() && *size > 0.0) {
            let window_size = window.bounds().size;
            let max_size = match self.position {
                DockPosition::Left | DockPosition::Right => window_size.width - MIN_SIZE,
                DockPosition::Bottom => window_size.height - MIN_SIZE,
            }
            .max(MIN_SIZE);
            self.size = px(size).clamp(MIN_SIZE, max_size);
        }

        let restored_index = state.active_panel.as_deref().and_then(|name| {
            self.panels
                .iter()
                .position(|panel| panel.persistent_name() == name)
        });
        if let Some(index) = restored_index {
            self.activate_panel(index, window, cx);
        }

        let active_panel_available = state.active_panel.is_none() || restored_index.is_some();
        self.set_open(state.visible && active_panel_available, window, cx);
        active_panel_available
    }

    /// 捕获当前 Dock 的可持久化快照。
    pub fn capture_state(&self) -> DockData {
        DockData {
            visible: self.is_open(),
            active_panel: self
                .active_panel()
                .map(|panel| panel.persistent_name().to_owned()),
            size: Some(f32::from(self.size)),
        }
    }

    // ── 面板切换 ─────────────────────────────────────────────────

    /// 切换面板的展开/折叠，并在切换前后通知面板的 `set_active`。
    pub fn toggle_panel_visibility(
        &mut self,
        panel_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if panel_index >= self.panels.len() {
            return;
        }
        let is_active = self.is_open && Some(panel_index) == self.active_panel_index;

        if is_active {
            self.set_open(false, window, cx);
        } else {
            self.activate_panel(panel_index, window, cx);
            self.set_open(true, window, cx);
        }
    }

    // ── 拖拽调整大小 ─────────────────────────────────────────────

    /// 拖拽到指定光标位置，更新 dock 尺寸。
    ///
    /// 采用绝对坐标模型：直接按光标相对参考区域边缘的距离计算新尺寸，不依赖拖拽起点——避免 delta 模型在跨坐标系事件（handle 本地坐标 vs 窗口坐标）下的基准漂移。
    pub fn resize_to(
        &mut self,
        cursor: Point<Pixels>,
        bounds: gpui::Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let raw = match self.position {
            DockPosition::Left => cursor.x - bounds.left(),
            DockPosition::Right => bounds.right() - cursor.x,
            DockPosition::Bottom => bounds.bottom() - cursor.y,
        };
        let max_size = match self.position {
            DockPosition::Left | DockPosition::Right => bounds.size.width - MIN_SIZE - MIN_SIZE,
            DockPosition::Bottom => bounds.size.height - MIN_SIZE,
        };
        let new_size = raw.clamp(MIN_SIZE, max_size);
        self.size = new_size;

        // 左右 dock 耦合：调整 sibling 的尺寸
        if (self.position == DockPosition::Left || self.position == DockPosition::Right)
            && let Some(sibling) = self.sibling.as_ref().and_then(|s| s.upgrade())
        {
            sibling.update(cx, |sib, _| {
                let other_max = bounds.size.width - new_size - MIN_SIZE;
                if sib.size > other_max {
                    sib.size = other_max.max(MIN_SIZE);
                }
            });
        }

        cx.emit(DockEvent::SizeChanged);
        cx.notify();
    }

    /// 重置为默认尺寸。
    pub fn reset_size(&mut self, window_size: gpui::Size<Pixels>, cx: &mut Context<Self>) {
        let default = self.position.default_size();
        let max_size = match self.position {
            DockPosition::Left | DockPosition::Right => window_size.width - MIN_SIZE,
            DockPosition::Bottom => window_size.height - MIN_SIZE,
        };
        self.size = default.clamp(MIN_SIZE, max_size);
        cx.emit(DockEvent::SizeChanged);
        cx.notify();
    }
}

impl Render for Dock {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_open = self.is_open();
        let panel_view: gpui::AnyElement = match self.visible_panel() {
            Some(handle) => handle.to_any().into_any_element(),
            None => placeholder_div(cx).into_any_element(),
        };

        let frame = div()
            .relative()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .overflow_hidden()
            .bg(color::current(cx).panel_background)
            .text_color(color::current(cx).text)
            .track_focus(&self.focus)
            .when_some(self.active_panel(), |this, panel| {
                this.key_context(panel.persistent_name())
            });

        let frame = match self.position {
            DockPosition::Left => frame
                .w(if is_open { self.size } else { Pixels::ZERO })
                .h_full()
                .when(is_open, |this| this.border_r_1())
                .border_color(color::current(cx).border),
            DockPosition::Right => frame
                .w(if is_open { self.size } else { Pixels::ZERO })
                .h_full()
                .when(is_open, |this| this.border_l_1())
                .border_color(color::current(cx).border),
            DockPosition::Bottom => frame
                .h(if is_open { self.size } else { Pixels::ZERO })
                .w_full()
                .when(is_open, |this| this.border_t_1())
                .border_color(color::current(cx).border),
        };

        let frame = frame.child(div().size_full().child(panel_view));

        // 拖拽调整大小的热区；手势由 gpui 拖拽系统承载，位置信息随 DraggedDock 传递，Workspace 根节点经 on_drag_move 驱动尺寸。
        // 手柄中心压在 dock 边界线上，两侧各占一半宽度，从 dock 内侧或编辑区一侧都能拖拽；deferred 提升渲染层，避免负偏移部分被 frame 溢出裁剪。
        const HIT: Pixels = px(6.0);
        let dock_entity = cx.entity().clone();
        let area = self.position;

        let handle = div()
            .id("resize-handle")
            .absolute()
            .on_drag(DraggedDock(area), move |_, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| DraggedDock(area))
            })
            // 按住手柄时阻断事件穿透，编辑区内容不响应落在手柄外伸区域的点击。
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                cx.stop_propagation();
            })
            .on_mouse_up(MouseButton::Left, {
                let dock_entity = dock_entity.clone();
                move |event, window, cx| {
                    if event.click_count >= 2 {
                        let win_size = window.bounds().size;
                        dock_entity.update(cx, |d, cx| d.reset_size(win_size, cx));
                        window.refresh();
                    }
                    cx.stop_propagation();
                }
            })
            .occlude();

        let handle = match self.position {
            DockPosition::Left => {
                deferred(handle.right(HIT * -0.5).w(HIT).h_full().cursor_col_resize())
            }
            DockPosition::Right => {
                deferred(handle.left(HIT * -0.5).w(HIT).h_full().cursor_col_resize())
            }
            DockPosition::Bottom => {
                deferred(handle.top(HIT * -0.5).w_full().h(HIT).cursor_row_resize())
            }
        };

        frame.child(handle)
    }
}

impl Focusable for Dock {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

fn placeholder_div(cx: &gpui::App) -> gpui::Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(color::current(cx).text_placeholder)
        .child("")
}

// ═══ 布局渲染 ═════════════════════════════════════════════════

/// 渲染 workbench 主体（不包含顶栏和底栏）。
///
/// 三个 Dock 始终挂载；关闭时由 Dock 自身收缩为零，确保 focus/action 生命周期稳定。
pub(crate) fn render_body(
    center: &Entity<Pane>,
    left_dock: Entity<Dock>,
    right_dock: Entity<Dock>,
    bottom_dock: Entity<Dock>,
) -> gpui::Div {
    let mut row = div()
        .flex_1()
        .flex()
        .flex_row()
        .size_full()
        .overflow_hidden()
        .relative();

    row = row.child(left_dock);

    let mut center_col = div()
        .flex_1()
        .flex()
        .flex_col()
        .size_full()
        .overflow_hidden()
        .relative()
        .min_w(space::S16);
    center_col = center_col.child(div().flex_1().min_h(space::S16).child(center.clone()));

    center_col = center_col.child(bottom_dock);
    row = row.child(center_col);

    row = row.child(right_dock);
    row
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui::{App, Context, FocusHandle, Render, TestAppContext, Window, div, prelude::*, px};

    use super::*;
    use crate::{Panel, PanelEvent, PanelHandle};

    macro_rules! test_panel {
        ($name:ident, $persistent_name:literal) => {
            struct $name {
                focus: FocusHandle,
            }

            impl EventEmitter<PanelEvent> for $name {}

            impl Panel for $name {
                fn icon() -> &'static str {
                    "icons/list_tree.svg"
                }

                fn label() -> &'static str {
                    $persistent_name
                }

                fn persistent_name() -> &'static str {
                    $persistent_name
                }

                fn focus_handle(&self, _: &App) -> FocusHandle {
                    self.focus.clone()
                }
            }

            impl Render for $name {
                fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                    div().track_focus(&self.focus)
                }
            }
        };
    }

    test_panel!(FirstPanel, "first");
    test_panel!(SecondPanel, "second");

    #[gpui::test]
    fn restores_active_panel_after_late_registration(cx: &mut TestAppContext) {
        let first = cx.new(|cx| FirstPanel {
            focus: cx.focus_handle(),
        });
        let second = cx.new(|cx| SecondPanel {
            focus: cx.focus_handle(),
        });
        let first_handle: Arc<dyn PanelHandle> = Arc::new(first);
        let second_handle: Arc<dyn PanelHandle> = Arc::new(second);
        let state = DockData {
            visible: true,
            active_panel: Some("second".into()),
            size: Some(333.0),
        };
        let (dock, cx) = cx.add_window_view(move |window, cx| {
            let mut dock = Dock::new(
                DockPosition::Left,
                Vec::new(),
                DockPosition::Left.default_size(),
                Some(state),
                cx,
            );
            dock.add_panel(first_handle, window, cx);
            assert!(!dock.is_open());
            dock.add_panel(second_handle, window, cx);
            dock
        });

        cx.read_entity(&dock, |dock, _| {
            assert!(dock.is_open());
            assert_eq!(dock.active_panel_index(), Some(1));
            assert_eq!(dock.capture_state().active_panel.as_deref(), Some("second"));
            assert_eq!(dock.capture_state().size, Some(333.0));
        });
    }

    /// 序列化 visible=true 的 dock 应随面板注册恢复打开（回归：终端面板重启不展开）。
    #[gpui::test]
    fn serialized_visible_dock_opens_on_panel_registration(cx: &mut TestAppContext) {
        let panel = cx.new(|cx| FirstPanel {
            focus: cx.focus_handle(),
        });
        let handle: Arc<dyn PanelHandle> = Arc::new(panel);
        let serialized = DockData {
            visible: true,
            active_panel: Some("first".into()),
            size: Some(200.0),
        };
        let (dock, cx) = cx.add_window_view(move |window, cx| {
            let mut dock = Dock::new(
                DockPosition::Bottom,
                Vec::new(),
                px(200.0),
                Some(serialized),
                cx,
            );
            dock.add_panel(handle, window, cx);
            dock
        });

        cx.read_entity(&dock, |dock, _| {
            assert!(dock.is_open(), "可见的 dock 应随面板注册恢复打开");
        });
    }

    #[gpui::test]
    fn capture_uses_stable_panel_name_not_index(cx: &mut TestAppContext) {
        let panel = cx.new(|cx| SecondPanel {
            focus: cx.focus_handle(),
        });
        let handle: Arc<dyn PanelHandle> = Arc::new(panel);
        let (dock, cx) = cx.add_window_view(move |window, cx| {
            let mut dock = Dock::new(DockPosition::Bottom, Vec::new(), px(200.0), None, cx);
            dock.add_panel(handle, window, cx);
            dock.set_open(true, window, cx);
            dock
        });

        cx.read_entity(&dock, |dock, _| {
            assert_eq!(dock.capture_state().active_panel.as_deref(), Some("second"));
        });
    }
}
