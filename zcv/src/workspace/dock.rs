//! 布局系统 —— Dock + 单一中心 Pane。
//!
//! 布局控制分两类区域：
//!
//! - **Dock**（左/右/底）：可折叠，同一时间一个 panel 可见，用 PanelStack 切换。
//!   每个 Dock 是独立 Entity，参考 Zed `crates/workspace/src/dock.rs`。
//! - **中心编辑区**：单一 Pane。

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    Context, Entity, FocusHandle, Focusable, MouseButton, Pixels, Point, Render, Subscription,
    WeakEntity, Window, actions, div, prelude::*, px,
};

use super::Pane;
use super::panel::PanelHandle;
use zcv_theme::{color, space};

// ═══ Panel 通用 action ═══════════════════════════════════════════

actions!(
    dock,
    [
        ToggleProjectTree,
        ToggleVersionControl,
        ToggleOutline,
        ToggleLanguageServer,
        ToggleDiagnostics,
        ToggleProjectSearch,
        ToggleTerminal,
        ToggleDebug,
        ToggleKeyboardShortcuts,
    ]
);

// ═══ 类型定义 ═══════════════════════════════════════════════════

/// Dock 位置，对应 Zed `DockPosition`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DockPosition {
    Left,
    Bottom,
    Right,
}

impl DockPosition {
    /// 未恢复持久化状态时采用的 Dock 单一默认尺寸。
    pub(crate) fn default_size(self) -> Pixels {
        match self {
            Self::Left | Self::Right => px(240.0),
            Self::Bottom => px(200.0),
        }
    }
}

/// dock 和编辑区的最小尺寸，防止 dock 拖拽完全挤占编辑区。
const MIN_SIZE: Pixels = space::S16;

/// 正在进行的拖拽状态（Dock 内部使用）。
#[derive(Debug, Clone, Copy)]
struct DragState {
    start_cursor: gpui::Point<Pixels>,
    start_size: Pixels,
}

// ═══ Dock Entity ═════════════════════════════════════════════════

/// Dock 容器：相邻窗口边缘，可折叠，同一时间只显示一个 panel。
///
/// 参考 Zed `crates/workspace/src/dock.rs` 中的 Dock 设计。
pub(crate) struct Dock {
    pub position: DockPosition,
    pub is_open: bool,
    size: Pixels,
    active_panel_index: Option<usize>,
    pub panels: Vec<Arc<dyn PanelHandle>>,
    pub focus: FocusHandle,
    /// 左右 dock 互为 sibling，拖拽时协调尺寸。
    sibling: Option<WeakEntity<Dock>>,
    /// 拖拽进行中的状态。
    drag_state: Option<DragState>,
    /// 通知 Workspace 哪个 dock 正在被拖拽。
    pub drag_notify: Rc<Cell<Option<DockPosition>>>,
    /// 生命周期相关订阅。
    pub _subscriptions: Vec<Subscription>,
}

impl Dock {
    /// 创建一个新 Dock，默认折叠。
    pub(crate) fn new(
        position: DockPosition,
        panels: Vec<Arc<dyn PanelHandle>>,
        initial_size: Pixels,
        drag_notify: Rc<Cell<Option<DockPosition>>>,
        cx: &mut Context<Self>,
    ) -> Self {
        // 激活第一个面板
        let active_panel_index = if panels.is_empty() { None } else { Some(0) };
        Self {
            position,
            is_open: false,
            size: initial_size,
            active_panel_index,
            panels,
            focus: cx.focus_handle(),
            sibling: None,
            drag_state: None,
            drag_notify,
            _subscriptions: Vec::new(),
        }
    }

    /// 设置 sibling dock（左右耦合）。
    pub(crate) fn set_sibling(&mut self, sibling: WeakEntity<Dock>) {
        self.sibling = Some(sibling);
    }

    // ── 状态查询 ─────────────────────────────────────────────────

    /// Dock 是否展开且含有激活面板。
    pub(crate) fn is_open(&self) -> bool {
        self.is_open && self.active_panel_index.is_some()
    }

    /// 当前可见面板。
    pub(crate) fn visible_panel(&self) -> Option<&Arc<dyn PanelHandle>> {
        if self.is_open() {
            self.active_panel_index.and_then(|i| self.panels.get(i))
        } else {
            None
        }
    }

    /// 按 action_name 查找面板在 dock 内的 index。
    pub(crate) fn panel_index_by_action(&self, action_name: &str) -> Option<usize> {
        self.panels
            .iter()
            .position(|h| h.action_name() == action_name)
    }

    /// 当前激活面板的 index。
    pub(crate) fn active_panel_index(&self) -> Option<usize> {
        self.active_panel_index
    }

    /// 指定 index 的面板是否激活（展开且为当前面板）。
    pub(crate) fn is_panel_active(&self, panel_index: usize) -> bool {
        self.is_open && Some(panel_index) == self.active_panel_index
    }

    // ── 面板切换 ─────────────────────────────────────────────────

    /// 切换面板的展开/折叠，并在切换前后通知面板的 `set_active`。
    pub(crate) fn toggle_panel(
        &mut self,
        panel_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(handle) = self.panels.get(panel_index) else {
            return;
        };
        let is_active = self.is_open && Some(panel_index) == self.active_panel_index;

        if is_active {
            // 隐藏 dock
            self.is_open = false;
            handle.set_active(false, window, cx);
        } else {
            // 停用旧面板（如果切换面板）
            if let Some(old_idx) = self.active_panel_index
                && old_idx != panel_index
                && let Some(old_handle) = self.panels.get(old_idx)
            {
                old_handle.set_active(false, window, cx);
            }
            self.active_panel_index = Some(panel_index);
            self.is_open = true;
            handle.set_active(true, window, cx);
        }
        cx.notify();
    }

    // ── 拖拽调整大小 ─────────────────────────────────────────────

    /// 开始拖拽调整大小。
    pub(crate) fn start_resize(&mut self, cursor: Point<Pixels>) {
        self.drag_state = Some(DragState {
            start_cursor: cursor,
            start_size: self.size,
        });
    }

    /// 拖拽到指定光标位置，更新 dock 尺寸。
    pub(crate) fn resize_to(
        &mut self,
        cursor: Point<Pixels>,
        window_size: gpui::Size<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = &self.drag_state else {
            return;
        };
        let delta = match self.position {
            DockPosition::Left => cursor.x - state.start_cursor.x,
            DockPosition::Right => state.start_cursor.x - cursor.x,
            DockPosition::Bottom => state.start_cursor.y - cursor.y,
        };
        let raw = state.start_size + delta;
        let max_size = match self.position {
            DockPosition::Left | DockPosition::Right => window_size.width - MIN_SIZE - MIN_SIZE,
            DockPosition::Bottom => window_size.height - MIN_SIZE,
        };
        let new_size = raw.clamp(MIN_SIZE, max_size);
        self.size = new_size;

        // 左右 dock 耦合：调整 sibling 的尺寸
        if (self.position == DockPosition::Left || self.position == DockPosition::Right)
            && let Some(sibling) = self.sibling.as_ref().and_then(|s| s.upgrade())
        {
            sibling.update(cx, |sib, _| {
                let other_max = window_size.width - new_size - MIN_SIZE;
                if sib.size > other_max {
                    sib.size = other_max.max(MIN_SIZE);
                }
            });
        }

        cx.notify();
    }

    /// 结束拖拽。
    pub(crate) fn end_resize(&mut self, _cx: &mut Context<Self>) {
        self.drag_state = None;
    }

    /// 是否正在拖拽。
    pub(crate) fn is_dragging(&self) -> bool {
        self.drag_state.is_some()
    }

    /// 重置为默认尺寸。
    pub(crate) fn reset_size(&mut self, window_size: gpui::Size<Pixels>, cx: &mut Context<Self>) {
        let default = self.position.default_size();
        let max_size = match self.position {
            DockPosition::Left | DockPosition::Right => window_size.width - MIN_SIZE,
            DockPosition::Bottom => window_size.height - MIN_SIZE,
        };
        self.size = default.clamp(MIN_SIZE, max_size);
        cx.notify();
    }
}

impl Render for Dock {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let panel_view: gpui::AnyElement = match self.visible_panel() {
            Some(handle) => handle.to_any().into_any_element(),
            None => placeholder_div().into_any_element(),
        };

        let frame = div()
            .relative()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .overflow_hidden()
            .bg(color::current().panel_background)
            .text_color(color::current().text);

        let frame = match self.position {
            DockPosition::Left => frame
                .w(self.size)
                .h_full()
                .border_r_1()
                .border_color(color::current().border_variant),
            DockPosition::Right => frame
                .w(self.size)
                .h_full()
                .border_l_1()
                .border_color(color::current().border_variant),
            DockPosition::Bottom => frame
                .h(self.size)
                .w_full()
                .border_t_1()
                .border_color(color::current().border_variant),
        };

        let frame = frame.child(div().size_full().child(panel_view));

        // 拖拽调整大小的热区
        const HIT: Pixels = px(6.0);
        let dock_entity = cx.entity().clone();
        let notify = self.drag_notify.clone();
        let area = self.position;

        let handle = div()
            .absolute()
            .on_mouse_down(MouseButton::Left, {
                let dock_entity = dock_entity.clone();
                move |event, window, cx| {
                    dock_entity.update(cx, |d, _| d.start_resize(event.position));
                    notify.set(Some(area));
                    window.refresh();
                }
            })
            .on_mouse_up(MouseButton::Left, move |event, window, cx| {
                if event.click_count >= 2 {
                    let win_size = window.bounds().size;
                    dock_entity.update(cx, |d, cx| d.reset_size(win_size, cx));
                    window.refresh();
                }
            });

        let handle = match self.position {
            DockPosition::Left => handle
                .right(Pixels::ZERO)
                .w(HIT)
                .h_full()
                .cursor_col_resize(),
            DockPosition::Right => handle
                .left(Pixels::ZERO)
                .w(HIT)
                .h_full()
                .cursor_col_resize(),
            DockPosition::Bottom => handle.top(Pixels::ZERO).w_full().h(HIT).cursor_row_resize(),
        };

        frame.child(handle)
    }
}

impl Focusable for Dock {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

fn placeholder_div() -> gpui::Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(color::current().text_placeholder)
        .child("")
}

// ═══ 布局渲染 ═════════════════════════════════════════════════

/// 渲染 workbench 主体（不包含顶栏和底栏）。
///
/// 三个 Dock 各自是独立 Entity，由调用方检查 `is_open()` 后决定是否传入。
pub(crate) fn render_body(
    center: &Entity<Pane>,
    left_dock: Option<Entity<Dock>>,
    right_dock: Option<Entity<Dock>>,
    bottom_dock: Option<Entity<Dock>>,
) -> gpui::Div {
    let mut row = div()
        .flex_1()
        .flex()
        .flex_row()
        .size_full()
        .overflow_hidden()
        .relative();

    if let Some(dock) = left_dock {
        row = row.child(dock);
    }

    let mut center_col = div()
        .flex_1()
        .flex()
        .flex_col()
        .size_full()
        .overflow_hidden()
        .relative()
        .min_w(space::S16);
    center_col = center_col.child(div().flex_1().min_h(space::S16).child(center.clone()));

    if let Some(dock) = bottom_dock {
        center_col = center_col.child(dock);
    }
    row = row.child(center_col);

    if let Some(dock) = right_dock {
        row = row.child(dock);
    }
    row
}
