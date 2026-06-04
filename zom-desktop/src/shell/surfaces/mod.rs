//! Workbench surface 框架（手册 21、布局模型 7）。
//!
//! Manager 作为 GPUI `Entity<SurfaceManager>` 挂在 `ShellView` 上，命令系统
//! 只通过 `HostEffect` 请求打开 surface；内容、行为与焦点由打开方决定。

mod anchor_registry;
mod manager;
mod shell;

use gpui::{AnyElement, Corner, ElementId, FocusHandle, Pixels, Point};
use std::rc::Rc;

pub(crate) use anchor_registry::{SurfaceAnchorRegistry, track_surface_anchor};
pub(crate) use manager::{ActiveSurface, SurfaceManager};
pub(crate) use shell::SurfaceShell;

// `SurfaceId` 的定义在 [`crate::ui_id`]，这里 re-export 让
// `crate::shell::surfaces::SurfaceId` 这一历史路径继续可用。
pub(crate) use crate::ui_id::SurfaceId;

/// Surface 的定位依据。当前只用召唤它的入口元素。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SurfaceAnchor {
    /// 锚到召唤该 surface 的已渲染 element 矩形上。
    Invoker(ElementId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SurfaceInvokerPoint {
    TopLeft,
    BottomLeft,
}

#[derive(Clone, Debug)]
pub(crate) struct SurfacePlacement {
    pub(crate) invoker_point: SurfaceInvokerPoint,
    pub(crate) corner: Corner,
    pub(crate) offset: Point<Pixels>,
    pub(crate) fallback_position: Point<Pixels>,
}

#[derive(Clone)]
pub(crate) struct SurfaceRequest {
    pub(crate) id: SurfaceId,
    pub(crate) anchor: SurfaceAnchor,
    pub(crate) placement: SurfacePlacement,
    pub(crate) focus_on_open: Option<FocusHandle>,
    pub(crate) render: Rc<dyn Fn() -> AnyElement>,
}
