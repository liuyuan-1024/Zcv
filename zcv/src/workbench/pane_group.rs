//! PaneGroup —— 编辑区分栏树。
//!
//! 叶子是 Pane，分支是 Split（水平/垂直分屏）。

use gpui::Entity as GpuiEntity;

use crate::workbench::pane::Pane;

// ═══ 类型定义 ═══════════════════════════════════════════════════

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
