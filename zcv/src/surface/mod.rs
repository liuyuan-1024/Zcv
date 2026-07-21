//! Surface —— 窗口级浮层面板系统。
//!
//! 基于 GPUI 的 `Anchored`、`deferred`、`occlude` 等能力搭建的一套轻量浮层方案。
//! 同时最多一个 active surface，点击遮罩外部自动关闭，关闭后恢复焦点。

mod anchor;
mod manager;
mod shell;

use std::rc::Rc;

use gpui::{AnyElement, BorrowAppContext, Corner, FocusHandle, Window};

pub(crate) use anchor::{AnchorRegistry, track_anchor};
pub(crate) use manager::SurfaceManager;
pub(crate) use shell::SurfaceShell;

/// Surface 标识符。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SurfaceId {
    ProjectPicker,
    BranchPicker,
}

/// Surface 定位方式。
#[derive(Clone, Debug)]
pub(crate) enum SurfaceAnchor {
    /// 锚定到窗口坐标系中的某个点。`offset` 在锚定方向上做微调。
    Position {
        /// 锚点位置（窗口坐标）。
        point: gpui::Point<gpui::Pixels>,
        /// 浮面的哪个角对准锚点。
        corner: Corner,
    },
    /// 窗口居中。
    Center,
}

/// 一个浮面的完整描述。
#[derive(Clone)]
pub(crate) struct SurfaceRequest {
    pub id: SurfaceId,
    pub anchor: SurfaceAnchor,
    pub focus_on_open: Option<FocusHandle>,
    pub render: Rc<dyn Fn() -> AnyElement>,
}

/// 打开一个 surface。
///
/// 保存当前窗口焦点供关闭后恢复，然后委托 `SurfaceManager::open`。
pub(crate) fn open_surface(request: SurfaceRequest, window: &mut Window, cx: &mut gpui::App) {
    let focus_to_restore = window.focused(cx);
    let focus_on_open = request.focus_on_open.clone();
    cx.update_global::<SurfaceManager, _>(|m, _| m.open(request, focus_to_restore));
    if let Some(focus) = &focus_on_open {
        window.focus(focus);
    }
    window.refresh();
}

/// 便捷函数：从锚定元素 bounds 构造 Position anchor。
///
/// 把 `corner` 理解为"浮面附着到触发元素的哪个角"，
/// 返回的 `SurfaceAnchor::Position` 将浮面的 `corner` 角对准触发元素对应角的坐标。
pub(crate) fn anchor_from_bounds(
    bounds: gpui::Bounds<gpui::Pixels>,
    corner: Corner,
) -> SurfaceAnchor {
    let point = match corner {
        Corner::TopLeft => bounds.bottom_left(),
        Corner::TopRight => bounds.bottom_right(),
        Corner::BottomLeft => bounds.origin,
        Corner::BottomRight => bounds.top_right(),
    };
    SurfaceAnchor::Position { point, corner }
}
