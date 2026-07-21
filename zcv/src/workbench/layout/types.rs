//! 布局系统的所有类型定义。

use gpui::Pixels;

// ── ID 类型 ──────────────────────────────────────────────────────────

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
    /// 用于占位展示的简短标签。
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

// ── 枚举 ─────────────────────────────────────────────────────────────

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

// ── Dock ─────────────────────────────────────────────────────────────

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

    /// 该 dock 是否承载某个 panel。
    pub(crate) fn contains(&self, panel: PanelId) -> bool {
        self.panels.contains(&panel)
    }
}

// ── 编辑区 Pane ──────────────────────────────────────────────────────

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

/// 一个 Pane = 一组 tab + 当前激活的 view。
#[derive(Debug, Clone)]
pub(crate) struct Pane {
    pub id: PaneId,
    pub tabs: Vec<TabItem>,
    pub active: Option<ViewId>,
}

impl Pane {
    pub(crate) fn new(id: PaneId) -> Self {
        Self {
            id,
            tabs: Vec::new(),
            active: None,
        }
    }

    /// 无内容且无 tab 的 pane 视为空，compact 时将被折叠。
    pub(crate) fn is_empty(&self) -> bool {
        self.tabs.is_empty() && self.active.is_none()
    }
}

/// PaneGroup 递归树 —— 编辑区的布局结构。
#[derive(Debug, Clone)]
pub(crate) enum PaneGroup {
    /// 叶子节点：一组 tab + 当前编辑视图。
    Pane(Pane),
    /// 分栏：将区域沿 axis 按 ratio 比例分割。
    Split {
        id: SplitId,
        axis: Axis,
        ratio: f32,
        children: [Box<PaneGroup>; 2],
    },
}

impl PaneGroup {
    pub(crate) fn pane_count(&self) -> usize {
        match self {
            PaneGroup::Pane(_) => 1,
            PaneGroup::Split { children, .. } => {
                children[0].pane_count() + children[1].pane_count()
            }
        }
    }

    pub(crate) fn all_panes(&self) -> Vec<PaneId> {
        match self {
            PaneGroup::Pane(pane) => vec![pane.id],
            PaneGroup::Split { children, .. } => {
                let mut ids = children[0].all_panes();
                ids.extend(children[1].all_panes());
                ids
            }
        }
    }

    pub(crate) fn find_pane(&self, id: PaneId) -> Option<&Pane> {
        match self {
            PaneGroup::Pane(pane) if pane.id == id => Some(pane),
            PaneGroup::Split { children, .. } => children[0]
                .find_pane(id)
                .or_else(|| children[1].find_pane(id)),
            _ => None,
        }
    }

    pub(crate) fn contains_pane(&self, id: PaneId) -> bool {
        self.find_pane(id).is_some()
    }

    /// 递归遍历找到第一个叶子 Pane。
    pub(crate) fn first_pane_id(&self) -> Option<PaneId> {
        match self {
            PaneGroup::Pane(pane) => Some(pane.id),
            PaneGroup::Split { children, .. } => children[0].first_pane_id(),
        }
    }
}

// ── 拖拽状态 ─────────────────────────────────────────────────────

/// 拖拽目标（当前仅支持 dock 分隔线）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DragTarget {
    DockDivider(DockArea),
    // 后续可扩展：SplitDivider(SplitId),
}

/// 正在进行的拖拽状态。
#[derive(Debug, Clone, Copy)]
pub(crate) struct DragState {
    pub target: DragTarget,
    /// 开始拖拽时的鼠标窗口坐标。
    pub start_cursor: gpui::Point<Pixels>,
    /// 拖拽目标的起始尺寸（dock 的 size 像素值）。
    pub start_size: Pixels,
}

// ── 布局快照（只读，给 render 用） ──────────────────────────────────

/// 渲染期只读布局快照。
#[derive(Debug, Clone)]
pub(crate) struct LayoutSnapshot {
    pub left_dock: DockState,
    pub right_dock: DockState,
    pub bottom_dock: DockState,
    pub center: PaneGroup,
    pub focus: LayoutFocus,
}
