//! LayoutController —— 布局状态的唯一控制入口。

use std::cell::RefCell;
use std::rc::Weak;

use gpui::{Pixels, Point, px};

use crate::theme::space;

use super::types::{
    Axis, Direction, DockArea, DockState, DragState, DragTarget, LayoutFocus, LayoutSnapshot, Pane,
    PaneGroup, PaneId, PanelId, SplitId, TabItem, ViewId,
};

/// dock 和编辑区的最小尺寸，防止 dock 拖拽完全挤占编辑区。
const MIN_SIZE: Pixels = space::S16;

/// 全局弱引用包装，供 `on_action` 自由函数访问布局控制器。
pub(crate) struct LayoutRef(pub(crate) Weak<RefCell<LayoutController>>);

impl gpui::Global for LayoutRef {}

/// 布局控制器：持有所有布局状态，提供唯一变更入口。
///
/// 内部维护 dock 三区 + 中心 PaneGroup 递归树 + 焦点。
/// render 层通过 [`snapshot()`](LayoutController::snapshot) 获取只读快照。
pub(crate) struct LayoutController {
    left_dock: DockState,
    right_dock: DockState,
    bottom_dock: DockState,
    center: PaneGroup,
    focus: LayoutFocus,
    next_pane_id: u32,
    next_split_id: u32,
    /// 当前拖拽状态（分隔线拖拽进行中时非空）。
    drag_state: Option<DragState>,
}

impl LayoutController {
    pub(crate) fn new() -> Self {
        let initial_pane_id = PaneId(1);
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
            center: PaneGroup::Pane(Pane::new(initial_pane_id)),
            focus: LayoutFocus::Pane(initial_pane_id),
            next_pane_id: 2,
            next_split_id: 1,
            drag_state: None,
        }
    }

    // ── ID 生成 ──────────────────────────────────────────────────────

    fn next_pane_id(&mut self) -> PaneId {
        let id = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        id
    }

    fn next_split_id(&mut self) -> SplitId {
        let id = SplitId(self.next_split_id);
        self.next_split_id += 1;
        id
    }

    // ── 快照 ─────────────────────────────────────────────────────────

    pub(crate) fn snapshot(&self) -> LayoutSnapshot {
        LayoutSnapshot {
            left_dock: self.left_dock.clone(),
            right_dock: self.right_dock.clone(),
            bottom_dock: self.bottom_dock.clone(),
            center: self.center.clone(),
            focus: self.focus,
        }
    }

    // ── Dock 操作 ────────────────────────────────────────────────────

    /// 切换 panel 显示：已显示则折叠其 dock，否则展开并切换。
    pub(crate) fn toggle_panel(&mut self, panel: PanelId) {
        let Some(dock) = self.dock_for_panel_mut(panel) else {
            return;
        };
        if dock.active_panel == Some(panel) && !dock.collapsed {
            dock.collapsed = true;
        } else {
            dock.active_panel = Some(panel);
            dock.collapsed = false;
            self.focus = LayoutFocus::Panel(panel);
        }
    }

    /// 隐藏指定 panel 所在的 dock（如果它是 active panel）。
    pub(crate) fn hide_panel(&mut self, panel: PanelId) {
        let Some(dock) = self.dock_for_panel_mut(panel) else {
            return;
        };
        if dock.active_panel == Some(panel) {
            dock.collapsed = true;
        }
    }

    /// 调整 dock 尺寸（像素），自动限制到合法范围。
    ///
    /// 左右 dock 可互相挤压，但对方和编辑区至少保留 MIN_SIZE。
    /// 底 dock 最大尺寸受编辑区最小高度约束。
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
                    // 如果编辑区被压缩到小于 MIN_SIZE，则挤压右 dock
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
                    // 如果编辑区被压缩到小于 MIN_SIZE，则挤压左 dock
                    let center = window_size.width - new_right - self.left_dock.size;
                    if center < MIN_SIZE {
                        self.left_dock.size =
                            (window_size.width - new_right - MIN_SIZE).max(MIN_SIZE);
                    }
                }

                self.right_dock.size = new_right;
            }
            DockArea::Bottom => {
                // 底 dock 不能挤占编辑区
                let max = (window_size.height - MIN_SIZE).max(MIN_SIZE);
                self.bottom_dock.size = size.clamp(MIN_SIZE, max);
            }
        }
    }

    // ── 拖拽操作 ────────────────────────────────────────────────────

    /// 开始拖拽某个 dock 分隔线。
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

    /// 拖拽移动中：根据当前鼠标位置更新 dock 尺寸。
    /// `window_size` 传入窗口内容区尺寸，由 resize_dock 统一做 clamp。
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

    /// 结束拖拽。
    pub(crate) fn end_drag(&mut self) {
        self.drag_state = None;
    }

    /// 双击分隔线：恢复 dock 到默认尺寸。
    pub(crate) fn reset_dock_size(&mut self, area: DockArea, window_size: gpui::Size<Pixels>) {
        let default = match area {
            DockArea::Left => px(240.0),
            DockArea::Right => px(240.0),
            DockArea::Bottom => px(200.0),
        };
        self.resize_dock(area, default, window_size);
    }

    /// 当前是否正在拖拽。
    pub(crate) fn is_dragging(&self) -> bool {
        self.drag_state.is_some()
    }

    /// 指定 panel 是否当前可见且激活（用于决定底栏 glyph 的高亮态）。
    pub(crate) fn is_panel_active(&self, panel: PanelId) -> bool {
        for dock in [&self.left_dock, &self.right_dock, &self.bottom_dock] {
            if dock.contains(panel) {
                return dock.active_panel == Some(panel) && !dock.collapsed;
            }
        }
        false
    }

    // ── 中心编辑区操作 ──────────────────────────────────────────────

    /// 向当前活跃 pane 添加一个 View（打开文件）。
    pub(crate) fn open_file(&mut self, view_id: ViewId, title: &str) {
        let pane_id = match self.focus {
            LayoutFocus::Pane(id) => id,
            LayoutFocus::Panel(_) => {
                // 如果焦点在 panel，用第一个 pane
                self.center.first_pane_id().unwrap_or_else(|| {
                    let id = self.next_pane_id();
                    self.center = PaneGroup::Pane(Pane::new(id));
                    id
                })
            }
        };
        self.center.add_tab(pane_id, view_id, title);
        self.focus = LayoutFocus::Pane(pane_id);
    }

    /// 沿 axis 方向拆分指定 pane。
    /// 原 pane 留在左/上，新 pane 在右/下。
    /// 返回新 pane 的 id（可用于后续操作）。
    pub(crate) fn split_pane(&mut self, pane_id: PaneId, axis: Axis) -> Option<PaneId> {
        let new_id = self.next_pane_id();
        let split_id = self.next_split_id();
        if self.center.split_pane_at(pane_id, axis, new_id, split_id) {
            self.focus = LayoutFocus::Pane(new_id);
            Some(new_id)
        } else {
            None
        }
    }

    /// 关闭指定 pane。
    /// 如果是 split 中的节点，自动合并邻居；如果是根节点，保留为空的 Pane。
    pub(crate) fn close_pane(&mut self, pane_id: PaneId) -> bool {
        if !self.center.contains_pane(pane_id) {
            return false;
        }
        self.center.close_pane_at(pane_id);

        // 如果焦点 pane 被关闭了，移到第一个 pane
        let all_panes = self.center.all_panes();
        if let LayoutFocus::Pane(id) = self.focus
            && !all_panes.contains(&id)
        {
            self.focus = all_panes
                .first()
                .copied()
                .map(LayoutFocus::Pane)
                .unwrap_or(self.focus);
        }
        true
    }

    /// 从当前焦点 pane 向指定方向导航到相邻 pane。
    pub(crate) fn navigate(&mut self, dir: Direction) -> bool {
        let current = match self.focus {
            LayoutFocus::Pane(id) => id,
            _ => return false,
        };
        if let Some(neighbor) = self.center.find_neighbor(current, dir) {
            self.focus = LayoutFocus::Pane(neighbor);
            true
        } else {
            false
        }
    }

    /// 调整分栏比例。
    pub(crate) fn resize_split(&mut self, split_id: SplitId, ratio: f32) -> bool {
        self.center.resize_split_at(split_id, ratio)
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
        Self::new()
    }
}

// ── PaneGroup 树操作 ─────────────────────────────────────────────────

impl PaneGroup {
    /// 拆分叶子 pane：将 `PaneGroup::Pane` 替换为 `Split`。
    fn split_pane_at(
        &mut self,
        id: PaneId,
        axis: Axis,
        new_pane_id: PaneId,
        split_id: SplitId,
    ) -> bool {
        if matches!(self, PaneGroup::Pane(pane) if pane.id == id) {
            let dummy = PaneGroup::Pane(Pane::new(new_pane_id));
            let old = std::mem::replace(self, dummy);
            if let PaneGroup::Pane(pane) = old {
                *self = PaneGroup::Split {
                    id: split_id,
                    axis,
                    ratio: 0.5,
                    children: [
                        Box::new(PaneGroup::Pane(pane)),
                        Box::new(PaneGroup::Pane(Pane::new(new_pane_id))),
                    ],
                };
                return true;
            }
            unreachable!()
        }
        if let PaneGroup::Split { children, .. } = self {
            return children[0].split_pane_at(id, axis, new_pane_id, split_id)
                || children[1].split_pane_at(id, axis, new_pane_id, split_id);
        }
        false
    }

    /// 关闭叶子 pane。如果是 split 下的孩子，合并到邻居。
    /// 返回 true 表示找到了并做了修改。
    fn close_pane_at(&mut self, id: PaneId) -> bool {
        if matches!(self, PaneGroup::Pane(pane) if pane.id == id) {
            *self = PaneGroup::Pane(Pane::new(id));
            return true;
        }

        if let PaneGroup::Split { children, .. } = self {
            let changed = children[0].close_pane_at(id) || children[1].close_pane_at(id);
            if changed {
                self.compact();
            }
            return changed;
        }
        false
    }

    /// 合并空孩子：如果一个 split 的某个孩子是空 Pane，用另一个孩子替换自身。
    fn compact(&mut self) {
        let (empty_left, empty_right) = match self {
            PaneGroup::Split { children, .. } => (children[0].is_empty(), children[1].is_empty()),
            _ => return,
        };

        if !empty_left && !empty_right {
            return;
        }

        let dummy = PaneGroup::Pane(Pane::new(PaneId(0)));
        let old = std::mem::replace(self, dummy);
        if let PaneGroup::Split { children, .. } = old {
            let [left, right] = children;
            *self = if empty_left && empty_right {
                PaneGroup::Pane(Pane::new(PaneId(0)))
            } else if empty_left {
                *right
            } else {
                *left
            };
        }
    }

    /// 子树是否全空（所有叶子 pane 都没有 tab）。
    fn is_empty(&self) -> bool {
        match self {
            PaneGroup::Pane(pane) => pane.is_empty(),
            PaneGroup::Split { children, .. } => children[0].is_empty() && children[1].is_empty(),
        }
    }

    /// 寻找当前 pane 在指定方向的相邻 pane。
    fn find_neighbor(&self, id: PaneId, dir: Direction) -> Option<PaneId> {
        match self {
            PaneGroup::Pane(_) => None,
            PaneGroup::Split { axis, children, .. } => {
                let axis_matches = matches!(
                    (axis, dir),
                    (Axis::Horizontal, Direction::Left | Direction::Right)
                        | (Axis::Vertical, Direction::Up | Direction::Down)
                );

                if axis_matches {
                    if children[0].contains_pane(id)
                        && matches!(dir, Direction::Right | Direction::Down)
                    {
                        return children[1].first_pane_id();
                    }
                    if children[1].contains_pane(id)
                        && matches!(dir, Direction::Left | Direction::Up)
                    {
                        return children[0].first_pane_id();
                    }
                }

                let r1 = children[0].find_neighbor(id, dir);
                if r1.is_some() {
                    return r1;
                }
                children[1].find_neighbor(id, dir)
            }
        }
    }

    /// 调整分栏比例。
    fn resize_split_at(&mut self, split_id: SplitId, ratio: f32) -> bool {
        match self {
            PaneGroup::Split { id, ratio: r, .. } if *id == split_id => {
                *r = ratio.clamp(0.1f32, 0.9f32);
                true
            }
            PaneGroup::Split { children, .. } => {
                children[0].resize_split_at(split_id, ratio)
                    || children[1].resize_split_at(split_id, ratio)
            }
            _ => false,
        }
    }

    /// 向指定 pane 添加一个 tab（若已存在则激活）。
    fn add_tab(&mut self, pane_id: PaneId, view_id: ViewId, title: &str) {
        if let PaneGroup::Pane(pane) = self {
            if pane.id == pane_id {
                if let Some(_tab) = pane.tabs.iter_mut().find(|t| t.view_id == view_id) {
                    pane.active = Some(view_id);
                    return;
                }
                pane.tabs.push(TabItem::new(view_id, title));
                pane.active = Some(view_id);
                return;
            }
        }
        if let PaneGroup::Split { children, .. } = self {
            children[0].add_tab(pane_id, view_id, title);
            children[1].add_tab(pane_id, view_id, title);
        }
    }
}
