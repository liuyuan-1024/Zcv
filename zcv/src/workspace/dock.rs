//! 布局系统 —— Dock + PaneGroup 分治。
//!
//! 布局控制分两类区域：
//!
//! - **Dock**（左/右/底）：可折叠，同一时间一个 panel 可见，用 PanelStack 切换。
//! - **中心编辑区**（PaneGroup）：递归分栏树，叶子是 Pane。
//!
//! 面板身份由 `LayoutController.panels` 中的 index 标识。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{App, Entity, MouseButton, Pixels, Point, Window, actions, div, prelude::*, px};

use super::pane::Pane;
use super::pane_group::{Axis, PaneGroup, PaneId};
use super::panel::PanelHandle;
use crate::theme::{color, space};

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

/// Dock 区域。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DockArea {
    Left,
    Right,
    Bottom,
}

/// Dock 运行时状态：同一时间只显示一个 panel。
///
/// 面板身份由 `LayoutController.panels` 中的 index 标识。
#[derive(Debug, Clone)]
pub(crate) struct DockState {
    pub collapsed: bool,
    pub size: Pixels,
    /// 当前激活面板在 panels 中的 index。
    pub active_panel: Option<usize>,
}

impl DockState {
    pub(crate) fn is_visible(&self) -> bool {
        !self.collapsed && self.active_panel.is_some()
    }
}

/// 拖拽目标（当前仅支持 dock 分隔线）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DragTarget {
    DockDivider(DockArea),
}

/// 正在进行的拖拽状态。
#[derive(Debug, Clone, Copy)]
pub(crate) struct DragState {
    pub target: DragTarget,
    pub start_cursor: gpui::Point<Pixels>,
    pub start_size: Pixels,
}

// ═══ LayoutSnapshot ═══════════════════════════════════════════

/// 渲染期只读布局快照。
#[derive(Clone)]
pub(crate) struct LayoutSnapshot {
    pub left_dock: DockState,
    pub right_dock: DockState,
    pub bottom_dock: DockState,
    pub center: PaneGroup,
}

// ═══ LayoutController ═════════════════════════════════════════

/// dock 和编辑区的最小尺寸，防止 dock 拖拽完全挤占编辑区。
const MIN_SIZE: Pixels = space::S16;

/// 布局控制器：持有所有布局状态，提供唯一变更入口。
/// 面板身份为 `panels` vec 中的 index，通过 `DockState.active_panel` 引用。
pub(crate) struct LayoutController {
    left_dock: DockState,
    right_dock: DockState,
    bottom_dock: DockState,
    center: PaneGroup,
    pub(crate) panels: Vec<(Arc<dyn PanelHandle>, DockArea)>,
    next_pane_id: u32,
    next_split_id: u32,
    drag_state: Option<DragState>,
}

impl LayoutController {
    pub(crate) fn with_initial_pane(
        pane: Entity<Pane>,
        panels: Vec<(Arc<dyn PanelHandle>, DockArea)>,
    ) -> Self {
        // 初始化时激活注册表中每个 dock area 的第一个面板
        let first_index =
            |area: DockArea| -> Option<usize> { panels.iter().position(|(_, a)| *a == area) };
        Self {
            left_dock: DockState {
                collapsed: true,
                size: px(240.0),
                active_panel: first_index(DockArea::Left),
            },
            right_dock: DockState {
                collapsed: true,
                size: px(240.0),
                active_panel: first_index(DockArea::Right),
            },
            bottom_dock: DockState {
                collapsed: true,
                size: px(200.0),
                active_panel: first_index(DockArea::Bottom),
            },
            center: PaneGroup::Pane(PaneId(1), pane),
            panels,
            next_pane_id: 2,
            next_split_id: 1,
            drag_state: None,
        }
    }

    pub(crate) fn next_pane_id(&mut self) -> PaneId {
        let id = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        id
    }

    pub(crate) fn snapshot(&self) -> LayoutSnapshot {
        LayoutSnapshot {
            left_dock: self.left_dock.clone(),
            right_dock: self.right_dock.clone(),
            bottom_dock: self.bottom_dock.clone(),
            center: self.center.clone(),
        }
    }

    // ── Dock 操作 ─────────────────────────────────────────────

    /// 切换面板的展开/折叠，并在切换前后通知面板的 `set_active`。
    pub(crate) fn toggle_panel(&mut self, index: usize, window: &mut Window, cx: &mut App) {
        let (handle, area) = match self.panels.get(index) {
            Some((h, a)) => (h.clone(), *a),
            None => return,
        };
        let was_active = {
            let dock = self.dock(area);
            dock.active_panel == Some(index) && !dock.collapsed
        };

        if was_active {
            self.dock_mut(area).collapsed = true;
            handle.set_active(false, window, cx);
        } else {
            // 停用旧面板
            let old_handle = {
                let dock = self.dock(area);
                dock.active_panel
                    .and_then(|old| self.panels.get(old).map(|(h, _)| h.clone()))
            };
            if let Some(ref old) = old_handle {
                old.set_active(false, window, cx);
            }
            self.dock_mut(area).active_panel = Some(index);
            self.dock_mut(area).collapsed = false;
            handle.set_active(true, window, cx);
        }
    }

    pub(crate) fn resize_dock(
        &mut self,
        area: DockArea,
        size: Pixels,
        window_size: gpui::Size<Pixels>,
    ) {
        match area {
            DockArea::Left => {
                let new_left = size.clamp(MIN_SIZE, window_size.width - MIN_SIZE - MIN_SIZE);
                if self.right_dock.is_visible() {
                    self.right_dock.size = (window_size.width - new_left - MIN_SIZE).max(MIN_SIZE);
                }
                self.left_dock.size = new_left;
            }
            DockArea::Right => {
                let new_right = size.clamp(MIN_SIZE, window_size.width - MIN_SIZE - MIN_SIZE);
                if self.left_dock.is_visible() {
                    self.left_dock.size = (window_size.width - new_right - MIN_SIZE).max(MIN_SIZE);
                }
                self.right_dock.size = new_right;
            }
            DockArea::Bottom => {
                self.bottom_dock.size =
                    size.clamp(MIN_SIZE, (window_size.height - MIN_SIZE).max(MIN_SIZE));
            }
        }
    }

    // ── 拖拽操作 ──────────────────────────────────────────────

    pub(crate) fn start_dock_drag(&mut self, area: DockArea, cursor: Point<Pixels>) {
        let size = match area {
            DockArea::Left => self.left_dock.size,
            DockArea::Right => self.right_dock.size,
            DockArea::Bottom => self.bottom_dock.size,
        };
        self.drag_state = Some(DragState {
            target: DragTarget::DockDivider(area),
            start_cursor: cursor,
            start_size: size,
        });
    }

    pub(crate) fn drag_to(&mut self, cursor: Point<Pixels>, window_size: gpui::Size<Pixels>) {
        let Some(state) = &self.drag_state else {
            return;
        };
        let DragTarget::DockDivider(area) = state.target;
        let delta = Point::new(
            cursor.x - state.start_cursor.x,
            cursor.y - state.start_cursor.y,
        );
        let new_size = match area {
            DockArea::Left => state.start_size + delta.x,
            DockArea::Right => state.start_size - delta.x,
            DockArea::Bottom => state.start_size - delta.y,
        };
        self.resize_dock(area, new_size, window_size);
    }

    pub(crate) fn end_drag(&mut self) {
        self.drag_state = None;
    }
    pub(crate) fn is_dragging(&self) -> bool {
        self.drag_state.is_some()
    }

    pub(crate) fn reset_dock_size(&mut self, area: DockArea, window_size: gpui::Size<Pixels>) {
        let default = match area {
            DockArea::Left => px(240.0),
            DockArea::Right => px(240.0),
            DockArea::Bottom => px(200.0),
        };
        self.resize_dock(area, default, window_size);
    }

    pub(crate) fn is_panel_active(&self, index: usize) -> bool {
        let Some((_, area)) = self.panels.get(index) else {
            return false;
        };
        let dock = self.dock(*area);
        dock.active_panel == Some(index) && !dock.collapsed
    }

    // ── 内部辅助 ───────────────────────────────────────────────

    fn dock(&self, area: DockArea) -> &DockState {
        match area {
            DockArea::Left => &self.left_dock,
            DockArea::Right => &self.right_dock,
            DockArea::Bottom => &self.bottom_dock,
        }
    }

    fn dock_mut(&mut self, area: DockArea) -> &mut DockState {
        match area {
            DockArea::Left => &mut self.left_dock,
            DockArea::Right => &mut self.right_dock,
            DockArea::Bottom => &mut self.bottom_dock,
        }
    }
}

impl Default for LayoutController {
    fn default() -> Self {
        panic!("LayoutController 需要 cx 来创建初始 Pane Entity，请使用 with_initial_pane");
    }
}

// ═══ 布局渲染 ═════════════════════════════════════════════════

/// 渲染 workbench 主体（不包含顶栏和底栏）。
pub(crate) fn render_body(
    layout: &LayoutSnapshot,
    panels: &[(Arc<dyn PanelHandle>, DockArea)],
    layout_ctrl: Rc<RefCell<LayoutController>>,
) -> gpui::Div {
    let mut row = div()
        .flex_1()
        .flex()
        .flex_row()
        .size_full()
        .overflow_hidden()
        .relative();

    if layout.left_dock.is_visible() {
        row = row.child(render_dock(
            DockArea::Left,
            &layout.left_dock,
            panels,
            Rc::clone(&layout_ctrl),
        ));
    }

    let mut center_col = div()
        .flex_1()
        .flex()
        .flex_col()
        .size_full()
        .overflow_hidden()
        .relative()
        .min_w(space::S16);
    center_col = center_col.child(render_pane_group(&layout.center));

    if layout.bottom_dock.is_visible() {
        center_col = center_col.child(render_dock(
            DockArea::Bottom,
            &layout.bottom_dock,
            panels,
            Rc::clone(&layout_ctrl),
        ));
    }
    row = row.child(center_col);

    if layout.right_dock.is_visible() {
        row = row.child(render_dock(
            DockArea::Right,
            &layout.right_dock,
            panels,
            layout_ctrl,
        ));
    }
    row
}

// ── Dock 渲染 ─────────────────────────────────────────────────

fn render_dock(
    area: DockArea,
    state: &DockState,
    panels: &[(Arc<dyn PanelHandle>, DockArea)],
    layout_ctrl: Rc<RefCell<LayoutController>>,
) -> gpui::Div {
    let frame = div()
        .relative()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .overflow_hidden()
        .bg(color::current().gray.s[1])
        .text_color(color::current().gray.s[8]);

    let frame = match area {
        DockArea::Left => frame
            .w(state.size)
            .h_full()
            .border_r_1()
            .border_color(color::current().gray.s[4]),
        DockArea::Right => frame
            .w(state.size)
            .h_full()
            .border_l_1()
            .border_color(color::current().gray.s[4]),
        DockArea::Bottom => frame
            .h(state.size)
            .w_full()
            .border_t_1()
            .border_color(color::current().gray.s[4]),
    };

    let body: gpui::Div = match state
        .active_panel
        .and_then(|i| panels.get(i).map(|(h, _)| h))
    {
        Some(panel) => div().size_full().child(panel.to_any_view()),
        None => placeholder_div(),
    };
    let frame = frame.child(body);

    const HIT: Pixels = space::S6;
    let zone = dock_drag_zone(area, layout_ctrl);
    match area {
        DockArea::Left => frame.child(zone.right(px(0.0)).w(HIT)),
        DockArea::Right => frame.child(zone.left(px(0.0)).w(HIT)),
        DockArea::Bottom => frame.child(zone.top(px(0.0)).h(HIT)),
    }
}

fn placeholder_div() -> gpui::Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(color::current().gray.s[5])
        .child("")
}

fn dock_drag_zone(area: DockArea, layout: Rc<RefCell<LayoutController>>) -> gpui::Div {
    let layout2 = Rc::clone(&layout);
    let base = div()
        .absolute()
        .on_mouse_down(MouseButton::Left, move |event, window, _cx| {
            layout.borrow_mut().start_dock_drag(area, event.position);
            window.refresh();
        })
        .on_mouse_up(MouseButton::Left, move |event, window, _cx| {
            if event.click_count >= 2 {
                layout2
                    .borrow_mut()
                    .reset_dock_size(area, window.bounds().size);
                window.refresh();
            }
        });

    match area {
        DockArea::Left | DockArea::Right => base.h_full().cursor_col_resize(),
        DockArea::Bottom => base.w_full().cursor_row_resize(),
    }
}

// ── 中心编辑区渲染 ─────────────────────────────────────────────

fn render_pane_group(group: &PaneGroup) -> gpui::Div {
    match group {
        PaneGroup::Pane(_, entity) => div().flex_1().min_h(space::S16).child(entity.clone()),
        PaneGroup::Split { axis, children, .. } => render_split(*axis, children),
    }
}

fn render_split(axis: Axis, children: &[Box<PaneGroup>; 2]) -> gpui::Div {
    let child_a = render_pane_group(&children[0]);
    let child_b = render_pane_group(&children[1]);

    match axis {
        Axis::Horizontal => div()
            .flex()
            .flex_row()
            .size_full()
            .overflow_hidden()
            .child(div().flex_1().min_w_0().child(child_a))
            .child(
                div()
                    .flex_shrink_0()
                    .w(px(1.0))
                    .h_full()
                    .bg(color::current().gray.s[4]),
            )
            .child(div().flex_1().min_w_0().child(child_b)),
        Axis::Vertical => div()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .child(div().flex_1().min_h_0().child(child_a))
            .child(
                div()
                    .flex_shrink_0()
                    .h(px(1.0))
                    .w_full()
                    .bg(color::current().gray.s[4]),
            )
            .child(div().flex_1().min_h_0().child(child_b)),
    }
}
