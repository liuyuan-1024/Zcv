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

/// 各 surface 的 active 态快照，在渲染帧起点一次性解析，下发给 glyph 查表。
///
/// 每个 surface entry 通过 [`Self::is_active`] 自行查询，不再需要调用方逐层透传 `bool` 参数。
#[derive(Clone, Copy, Debug)]
pub(crate) struct SurfaceStates {
    pub(crate) project_picker: bool,
    pub(crate) settings: bool,
    pub(crate) language_servers: bool,
    pub(crate) go_to_line: bool,
}

impl SurfaceStates {
    pub(crate) fn is_active(&self, id: SurfaceId) -> bool {
        match id {
            SurfaceId::ProjectPicker => self.project_picker,
            SurfaceId::Settings => self.settings,
            SurfaceId::LanguageServers => self.language_servers,
            SurfaceId::GoToLine => self.go_to_line,
        }
    }
}

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
