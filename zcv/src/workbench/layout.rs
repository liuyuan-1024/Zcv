//! 布局系统 —— Dock + PaneGroup 分治。
//!
//! 布局控制分两类区域：
//!
//! - **Dock**（左/右/底）：可折叠，同一时间一个 panel 可见，用 PanelStack 切换。
//! - **中心编辑区**（PaneGroup）：递归分栏树，叶子是 Pane。

use std::cell::RefCell;
use std::rc::Weak;

use gpui::Entity as GpuiEntity;
use gpui::{Entity, MouseButton, Pixels, Point, Window, div, prelude::*, px};

use crate::theme::{color, space};
use crate::workbench::pane::{CloseTab, Pane};

// ═══ 类型定义 ═══════════════════════════════════════════════════

/// 面板标识（Dock 中的工具面板）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PanelId {
    ProjectTree,
    VersionControl,
    Outline,
    Terminal,
    Debug,
    KeyboardShortcuts,
}

impl PanelId {
    pub(crate) fn label(self) -> &'static str {
        match self {
            PanelId::ProjectTree => "项目树",
            PanelId::VersionControl => "版本控制",
            PanelId::Outline => "大纲",
            PanelId::Terminal => "终端",
            PanelId::Debug => "调试",
            PanelId::KeyboardShortcuts => "快捷键",
        }
    }
}

/// 编辑区 Pane 的稳定标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PaneId(pub(crate) u32);

/// 分栏节点的稳定标识（供 resize 定位）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SplitId(pub(crate) u32);

/// 视图标识（某个打开文档的编辑视图）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ViewId(pub u64);

/// 分栏轴向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Axis {
    Horizontal,
    Vertical,
}

/// 导航方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// Dock 区域。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DockArea {
    Left,
    Right,
    Bottom,
}

/// 当前焦点所在位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LayoutFocus {
    Panel(PanelId),
    Pane(PaneId),
}

/// Dock 运行时状态：同一时间只显示一个 panel。
#[derive(Debug, Clone)]
pub(crate) struct DockState {
    pub collapsed: bool,
    pub size: Pixels,
    pub active_panel: Option<PanelId>,
    pub panels: Vec<PanelId>,
}

impl DockState {
    pub(crate) fn new(panels: Vec<PanelId>, default_size: Pixels) -> Self {
        Self {
            collapsed: true,
            size: default_size,
            active_panel: panels.first().copied(),
            panels,
        }
    }

    pub(crate) fn is_visible(&self) -> bool {
        !self.collapsed && self.active_panel.is_some()
    }
    pub(crate) fn active_panel(&self) -> Option<PanelId> {
        self.active_panel
    }
    pub(crate) fn contains(&self, panel: PanelId) -> bool {
        self.panels.contains(&panel)
    }
}

/// 标签页项。
#[derive(Debug, Clone)]
pub(crate) struct TabItem {
    pub view_id: ViewId,
    pub title: String,
    pub dirty: bool,
}

impl TabItem {
    pub(crate) fn new(view_id: ViewId, title: impl Into<String>) -> Self {
        Self {
            view_id,
            title: title.into(),
            dirty: false,
        }
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

// ═══ PaneGroup ═══════════════════════════════════════════════════

/// PaneGroup 递归树 —— 编辑区的布局结构。
#[derive(Clone)]
pub(crate) enum PaneGroup {
    Pane(PaneId, GpuiEntity<Pane>),
    Split {
        id: SplitId,
        axis: Axis,
        ratio: f32,
        children: [Box<PaneGroup>; 2],
    },
}

impl PaneGroup {
    pub(crate) fn first_pane_id(&self) -> Option<PaneId> {
        match self {
            PaneGroup::Pane(id, _) => Some(*id),
            PaneGroup::Split { children, .. } => children[0].first_pane_id(),
        }
    }
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
    next_pane_id: u32,
    next_split_id: u32,
    drag_state: Option<DragState>,
}

impl LayoutController {
    pub(crate) fn with_initial_pane(pane: Entity<Pane>) -> Self {
        let pane_id = PaneId(1);
        Self {
            left_dock: DockState::new(
                vec![
                    PanelId::ProjectTree,
                    PanelId::VersionControl,
                    PanelId::Outline,
                ],
                px(240.0),
            ),
            right_dock: DockState::new(vec![PanelId::KeyboardShortcuts], px(240.0)),
            bottom_dock: DockState::new(vec![PanelId::Terminal, PanelId::Debug], px(200.0)),
            center: PaneGroup::Pane(pane_id, pane.clone()),
            focus_pane: Some(pane),
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

    pub(crate) fn toggle_panel(&mut self, panel: PanelId) {
        let Some(dock) = self.dock_for_panel_mut(panel) else {
            return;
        };
        if dock.active_panel == Some(panel) && !dock.collapsed {
            dock.collapsed = true;
        } else {
            dock.active_panel = Some(panel);
            dock.collapsed = false;
        }
    }

    pub(crate) fn hide_panel(&mut self, panel: PanelId) {
        let Some(dock) = self.dock_for_panel_mut(panel) else {
            return;
        };
        if dock.active_panel == Some(panel) {
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

    pub(crate) fn is_panel_active(&self, panel: PanelId) -> bool {
        for dock in [&self.left_dock, &self.right_dock, &self.bottom_dock] {
            if dock.contains(panel) {
                return dock.active_panel == Some(panel) && !dock.collapsed;
            }
        }
        false
    }

    // ── 内部辅助 ─────────────────────────────────────────────────────

    fn dock_for_panel_mut(&mut self, panel: PanelId) -> Option<&mut DockState> {
        [
            &mut self.left_dock,
            &mut self.right_dock,
            &mut self.bottom_dock,
        ]
        .into_iter()
        .find(|dock| dock.contains(panel))
    }
}

impl Default for LayoutController {
    fn default() -> Self {
        panic!("LayoutController 需要 cx 来创建初始 Pane Entity，请使用 with_initial_pane");
    }
}

/// 关闭当前焦点所在的 tab。
pub(crate) fn handle_close_tab(_: &CloseTab, window: &mut Window, cx: &mut gpui::App) {
    if let Some(layout_ref) = cx.try_global::<LayoutRef>()
        && let Some(ctrl) = layout_ref.0.upgrade()
    {
        let pane_entity = ctrl.borrow().focus_pane.clone();
        if let Some(entity) = pane_entity {
            if let Some(view_id) = entity.read(cx).active {
                entity.update(cx, |pane, _| pane.close_tab(view_id));
                window.refresh();
            }
        }
    }
}

// ═══ 布局渲染 ═══════════════════════════════════════════════════

/// 面板内容提供者：布局不感知具体 panel 类型，通过此回调获取内容。
pub(crate) type PanelContentFn<'a> = dyn Fn(PanelId) -> Option<gpui::Div> + 'a;

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
        None => {
            let label = state.active_panel.map(|p| p.label()).unwrap_or("");
            render_placeholder(label)
        }
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
