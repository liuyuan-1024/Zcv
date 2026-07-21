//! LayoutController —— 布局状态的唯一控制入口。

use std::cell::RefCell;
use std::rc::Weak;

use gpui::{Entity, Pixels, Point, Window, px};

use crate::theme::space;

use super::pane::CloseTab;

use super::pane::Pane;
use super::types::{Axis, DockArea, DockState, DragState, DragTarget, PaneId, PanelId, SplitId};

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
                    let center = window_size.width - new_left - self.right_dock.size;
                    if center < MIN_SIZE {
                        self.right_dock.size =
                            (window_size.width - new_left - MIN_SIZE).max(MIN_SIZE);
                    }
                }
                self.left_dock.size = new_left;
            }
            DockArea::Right => {
                let new_right = size.clamp(MIN_SIZE, window_size.width - MIN_SIZE - MIN_SIZE);
                if self.left_dock.is_visible() {
                    let center = window_size.width - new_right - self.left_dock.size;
                    if center < MIN_SIZE {
                        self.left_dock.size =
                            (window_size.width - new_right - MIN_SIZE).max(MIN_SIZE);
                    }
                }
                self.right_dock.size = new_right;
            }
            DockArea::Bottom => {
                let max = (window_size.height - MIN_SIZE).max(MIN_SIZE);
                self.bottom_dock.size = size.clamp(MIN_SIZE, max);
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

    pub(crate) fn reset_dock_size(&mut self, area: DockArea, window_size: gpui::Size<Pixels>) {
        let default = match area {
            DockArea::Left => px(240.0),
            DockArea::Right => px(240.0),
            DockArea::Bottom => px(200.0),
        };
        self.resize_dock(area, default, window_size);
    }

    pub(crate) fn is_dragging(&self) -> bool {
        self.drag_state.is_some()
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

// ── PaneGroup 树定义与操作 ──────────────────────────────────────────

use gpui::Entity as GpuiEntity;

/// PaneGroup 递归树 —— 编辑区的布局结构。
#[derive(Clone)]
pub(crate) enum PaneGroup {
    /// 叶子节点：一个 Pane Entity。
    Pane(PaneId, GpuiEntity<Pane>),
    /// 分栏：将区域沿 axis 按 ratio 比例分割。
    Split {
        id: SplitId,
        axis: Axis,
        ratio: f32,
        children: [Box<PaneGroup>; 2],
    },
}

impl PaneGroup {
    /// 子树中的第一个 PaneId。
    pub(crate) fn first_pane_id(&self) -> Option<PaneId> {
        match self {
            PaneGroup::Pane(id, _) => Some(*id),
            PaneGroup::Split { children, .. } => children[0].first_pane_id(),
        }
    }
}

// ── 布局快照 ────────────────────────────────────────────────────────

/// 渲染期只读布局快照。
#[derive(Clone)]
pub(crate) struct LayoutSnapshot {
    pub left_dock: DockState,
    pub right_dock: DockState,
    pub bottom_dock: DockState,
    pub center: PaneGroup,
}
