//! Workbench surface 框架（手册 21、布局模型 7）。
//!
//! Manager 作为 GPUI `Entity<SurfaceManager>` 挂在 `ShellView` 上，命令系统
//! 只通过 `HostEffect` 请求打开 surface；内容、行为与焦点由打开方决定。

mod anchor_registry;
mod manager;
mod shell;

use gpui::{AnyElement, Corner, ElementId, FocusHandle, Pixels, Point};
use std::rc::Rc;

use crate::ui_id::SurfaceId;

pub(crate) use anchor_registry::{SurfaceAnchorRegistry, track_surface_anchor};
pub(crate) use manager::{ActiveSurface, SurfaceManager};
pub(crate) use shell::SurfaceShell;

/// Surface 的定位依据，每种变体自带定位参数。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SurfaceAnchor {
    /// 锚到召唤该 surface 的已渲染 element 矩形上。
    /// `attachment` 是浮面贴到入口的角，入口锚点取对角。
    Invoker {
        id: ElementId,
        attachment: Corner,
        fallback_position: Point<Pixels>,
    },
    /// 相对于窗口定位。
    Window { position: WindowPosition },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WindowPosition {
    Center,
}

#[derive(Clone)]
pub(crate) struct SurfaceRequest {
    pub(crate) id: SurfaceId,
    pub(crate) anchor: SurfaceAnchor,
    pub(crate) focus_on_open: Option<FocusHandle>,
    pub(crate) render: Rc<dyn Fn() -> AnyElement>,
}
