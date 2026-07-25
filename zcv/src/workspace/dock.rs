//! 布局系统 —— Dock + PaneGroup 分治。
//!
//! 布局控制分两类区域：
//!
//! - **Dock**（左/右/底）：可折叠，同一时间一个 panel 可见，用 PanelStack 切换。
//! - **中心编辑区**（PaneGroup）：递归分栏树，叶子是 Pane。

use std::cell::RefCell;
use std::rc::Weak;

use gpui::{App, Entity, MouseButton, Pixels, Point, Window, actions, div, prelude::*, px};

use super::pane_group::{Axis, PaneGroup, PaneId};
use crate::theme::{color, space};
use crate::workspace::pane::{CloseTab, Pane};

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

// ═══ PanelEntry 注册表项 ════════════════════════════════════════

/// 单个面板的注册信息，供 PanelButtons 遍历生成按钮。
pub(crate) struct PanelEntry {
    pub dock_area: DockArea,
    pub icon: &'static str,
    pub label: &'static str,
    /// 快捷键查找名（如 "dock::ToggleProjectTree"）。
    pub action_name: &'static str,
    pub requires_active_color: bool,
    pub dispatch: fn(&mut Window, &mut App),
}

/// 默认面板注册表。面板在注册表中的 index 即为面板身份标识。
pub(crate) fn default_panels() -> Vec<PanelEntry> {
    vec![
        PanelEntry {
            dock_area: DockArea::Left,
            icon: "icons/panels/project_tree.svg",
            label: "项目树",
            action_name: "dock::ToggleProjectTree",
            requires_active_color: true,
            dispatch: |w: &mut Window, cx: &mut App| {
                w.dispatch_action(Box::new(ToggleProjectTree), cx)
            },
        },
        PanelEntry {
            dock_area: DockArea::Left,
            icon: "icons/panels/version_control.svg",
            label: "版本控制",
            action_name: "dock::ToggleVersionControl",
            requires_active_color: true,
            dispatch: |w: &mut Window, cx: &mut App| {
                w.dispatch_action(Box::new(ToggleVersionControl), cx)
            },
        },
        PanelEntry {
            dock_area: DockArea::Left,
            icon: "icons/panels/outline.svg",
            label: "大纲",
            action_name: "dock::ToggleOutline",
            requires_active_color: true,
            dispatch: |w: &mut Window, cx: &mut App| w.dispatch_action(Box::new(ToggleOutline), cx),
        },
        PanelEntry {
            dock_area: DockArea::Bottom,
            icon: "icons/panels/terminal.svg",
            label: "终端",
            action_name: "dock::ToggleTerminal",
            requires_active_color: true,
            dispatch: |w: &mut Window, cx: &mut App| {
                w.dispatch_action(Box::new(ToggleTerminal), cx)
            },
        },
        PanelEntry {
            dock_area: DockArea::Bottom,
            icon: "icons/panels/debug.svg",
            label: "调试",
            action_name: "dock::ToggleDebug",
            requires_active_color: true,
            dispatch: |w: &mut Window, cx: &mut App| w.dispatch_action(Box::new(ToggleDebug), cx),
        },
        PanelEntry {
            dock_area: DockArea::Right,
            icon: "icons/panels/keyboard_shortcuts.svg",
            label: "快捷键",
            action_name: "dock::ToggleKeyboardShortcuts",
            requires_active_color: true,
            dispatch: |w: &mut Window, cx: &mut App| {
                w.dispatch_action(Box::new(ToggleKeyboardShortcuts), cx)
            },
        },
    ]
}

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
/// 面板身份由 `LayoutController.panel_registry` 中的 index 标识。
#[derive(Debug, Clone)]
pub(crate) struct DockState {
    pub collapsed: bool,
    pub size: Pixels,
    /// 当前激活面板在 panel_registry 中的 index。
    pub active_panel: Option<usize>,
}

impl DockState {
    pub(crate) fn new(default_size: Pixels) -> Self {
        Self {
            collapsed: true,
            size: default_size,
            active_panel: None,
        }
    }

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

// ═══ LayoutSnapshot ════════════════════════════════════════════════

/// 渲染期只读布局快照。
#[derive(Clone)]
pub(crate) struct LayoutSnapshot {
    pub left_dock: DockState,
    pub right_dock: DockState,
    pub bottom_dock: DockState,
    pub center: PaneGroup,
}

// ═══ LayoutController ═══════════════════════════════════════════

/// dock 和编辑区的最小尺寸，防止 dock 拖拽完全挤占编辑区。
const MIN_SIZE: Pixels = space::S16;

/// 全局弱引用包装，供 `on_action` 自由函数访问布局控制器。
pub(crate) struct LayoutRef(pub(crate) Weak<RefCell<LayoutController>>);

impl gpui::Global for LayoutRef {}

/// 布局控制器：持有所有布局状态，提供唯一变更入口。
pub(crate) struct LayoutController {
    left_dock: DockState,
    right_dock: DockState,
    bottom_dock: DockState,
    center: PaneGroup,
    pub(crate) focus_pane: Option<Entity<Pane>>,
    pub(crate) panel_registry: Vec<PanelEntry>,
    next_pane_id: u32,
    next_split_id: u32,
    drag_state: Option<DragState>,
}

impl LayoutController {
    pub(crate) fn with_initial_pane(pane: Entity<Pane>) -> Self {
        let registry = default_panels();
        let pane_id = PaneId(1);
        // 初始化时激活注册表中每个 dock area 的第一个面板
        let first_index =
            |area: DockArea| -> Option<usize> { registry.iter().position(|p| p.dock_area == area) };
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
            center: PaneGroup::Pane(pane_id, pane.clone()),
            focus_pane: Some(pane),
            panel_registry: registry,
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

    fn next_split_id(&mut self) -> SplitId {
        let id = SplitId(self.next_split_id);
        self.next_split_id += 1;
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

    pub(crate) fn focus_pane_entity(&self) -> Option<&Entity<Pane>> {
        self.focus_pane.as_ref()
    }
    pub(crate) fn set_focus_pane(&mut self, entity: &Entity<Pane>) {
        self.focus_pane = Some(entity.clone());
    }

    // ── Dock 操作 ────────────────────────────────────────────────────

    pub(crate) fn toggle_panel(&mut self, index: usize) {
        let Some(entry) = self.panel_registry.get(index) else {
            return;
        };
        let dock = self.dock_mut(entry.dock_area);
        if dock.active_panel == Some(index) && !dock.collapsed {
            dock.collapsed = true;
        } else {
            dock.active_panel = Some(index);
            dock.collapsed = false;
        }
    }

    pub(crate) fn hide_panel(&mut self, index: usize) {
        let Some(entry) = self.panel_registry.get(index) else {
            return;
        };
        let dock = self.dock_mut(entry.dock_area);
        if dock.active_panel == Some(index) {
            dock.collapsed = true;
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

    // ── 拖拽操作 ────────────────────────────────────────────────────

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

    pub(crate) fn panels_for_area(&self, area: DockArea) -> Vec<(usize, &PanelEntry)> {
        self.panel_registry
            .iter()
            .enumerate()
            .filter(|(_, p)| p.dock_area == area)
            .map(|(i, p)| (i, p))
            .collect()
    }

    pub(crate) fn is_panel_active(&self, index: usize) -> bool {
        let Some(entry) = self.panel_registry.get(index) else {
            return false;
        };
        let dock = self.dock(entry.dock_area);
        dock.active_panel == Some(index) && !dock.collapsed
    }

    // ── 内部辅助 ─────────────────────────────────────────────────────

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

use super::pane_group::SplitId;

/// 关闭当前焦点所在的 tab。
pub(crate) fn handle_close_tab(_: &CloseTab, window: &mut Window, cx: &mut gpui::App) {
    if let Some(layout_ref) = cx.try_global::<LayoutRef>()
        && let Some(ctrl) = layout_ref.0.upgrade()
    {
        let pane_entity = ctrl.borrow().focus_pane.clone();
        if let Some(entity) = pane_entity {
            if let Some(view_id) = entity.read(cx).active {
                let editor = entity.update(cx, |pane, cx| {
                    pane.close_tab(view_id);
                    pane.active_editor(cx)
                });
                if let Some(editor) = editor {
                    window.focus(&editor.read(cx).focus_handle());
                }
                window.refresh();
            }
        }
    }
}

// ═══ 布局渲染 ═══════════════════════════════════════════════════

/// 面板内容提供者：布局不感知具体 panel 类型，通过此回调获取内容。
pub(crate) type PanelContentFn<'a> = dyn Fn(usize) -> Option<gpui::Div> + 'a;

/// 渲染 workbench 主体（不包含顶栏和底栏）。
pub(crate) fn render_body(layout: &LayoutSnapshot, panel_content: &PanelContentFn) -> gpui::Div {
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
            panel_content,
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
            panel_content,
        ));
    }
    row = row.child(center_col);

    if layout.right_dock.is_visible() {
        row = row.child(render_dock(
            DockArea::Right,
            &layout.right_dock,
            panel_content,
        ));
    }
    row
}

// ── Dock 渲染 ────────────────────────────────────────────────────────

fn render_dock(area: DockArea, state: &DockState, panel_content: &PanelContentFn) -> gpui::Div {
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

    let body: gpui::Div = match state.active_panel.and_then(panel_content) {
        Some(content) => div().size_full().child(content),
        None => render_placeholder(""),
    };
    let frame = frame.child(body);

    const HIT: Pixels = space::S6;
    match area {
        DockArea::Left => frame.child(dock_drag_zone(DockArea::Left).right(px(0.0)).w(HIT)),
        DockArea::Right => frame.child(dock_drag_zone(DockArea::Right).left(px(0.0)).w(HIT)),
        DockArea::Bottom => frame.child(dock_drag_zone(DockArea::Bottom).top(px(0.0)).h(HIT)),
    }
}

fn render_placeholder(label: &str) -> gpui::Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(color::current().gray.s[5])
        .child(label.to_string())
}

fn dock_drag_zone(area: DockArea) -> gpui::Div {
    let base = div()
        .absolute()
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            if let Some(layout_ref) = cx.try_global::<LayoutRef>()
                && let Some(ctrl) = layout_ref.0.upgrade()
            {
                ctrl.borrow_mut().start_dock_drag(area, event.position);
                window.refresh();
            }
        })
        .on_mouse_up(MouseButton::Left, move |event, window, cx| {
            if event.click_count >= 2
                && let Some(layout_ref) = cx.try_global::<LayoutRef>()
                && let Some(ctrl) = layout_ref.0.upgrade()
            {
                ctrl.borrow_mut()
                    .reset_dock_size(area, window.bounds().size);
                window.refresh();
            }
        });

    match area {
        DockArea::Left | DockArea::Right => base.h_full().cursor_col_resize(),
        DockArea::Bottom => base.w_full().cursor_row_resize(),
    }
}

// ── 中心编辑区渲染 ──────────────────────────────────────────────────

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
