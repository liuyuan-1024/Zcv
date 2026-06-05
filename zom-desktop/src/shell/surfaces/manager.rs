//! 每窗口 surface 状态管理。

use gpui::{Context, FocusHandle};

use crate::ui_id::SurfaceId;

use super::SurfaceRequest;

/// 当前活跃 surface 的完整运行时状态。
#[derive(Clone)]
pub(crate) struct ActiveSurface {
    request: SurfaceRequest,
    focus_to_restore: FocusHandle,
}

impl ActiveSurface {
    pub(crate) fn request(&self) -> &SurfaceRequest {
        &self.request
    }
}

/// 每窗口 surface 管理器。当前同时最多一个 active surface。
#[derive(Default)]
pub(crate) struct SurfaceManager {
    active: Option<ActiveSurface>,
}

impl SurfaceManager {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn active(&self) -> Option<&ActiveSurface> {
        self.active.as_ref()
    }

    pub(crate) fn is_active(&self, id: SurfaceId) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.request.id == id)
    }

    pub(crate) fn open(
        &mut self,
        request: SurfaceRequest,
        focus_to_restore: FocusHandle,
        cx: &mut Context<Self>,
    ) {
        self.active = Some(ActiveSurface {
            request,
            focus_to_restore,
        });
        cx.notify();
    }

    /// 关闭当前 surface。返回打开时记录的焦点句柄供调用方恢复焦点。
    pub(crate) fn dismiss(&mut self, cx: &mut Context<Self>) -> Option<FocusHandle> {
        let focus = self.active.take().map(|active| active.focus_to_restore);
        if focus.is_some() {
            cx.notify();
        }
        focus
    }
}
