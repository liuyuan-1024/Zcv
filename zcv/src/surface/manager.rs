//! SurfaceManager —— 每窗口 surface 状态管理。
//!
//! 作为 GPUI Global 存储，所有 state 变更后调用方负责 `window.refresh()`。

use gpui::{FocusHandle, Global};

use super::{SurfaceId, SurfaceRequest};

/// 当前活跃 surface 的运行时状态。
#[derive(Clone)]
pub(crate) struct ActiveSurface {
    pub(crate) request: SurfaceRequest,
    /// 打开前窗口焦点，关闭后恢复。可能无焦点。
    pub(crate) focus_to_restore: Option<FocusHandle>,
}

impl ActiveSurface {
    pub fn request(&self) -> &SurfaceRequest {
        &self.request
    }
}

/// 每窗口 surface 管理器。全局单例，变更后调用方自己 `window.refresh()`。
#[derive(Default)]
pub(crate) struct SurfaceManager {
    active: Option<ActiveSurface>,
}

impl Global for SurfaceManager {}

impl SurfaceManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn active(&self) -> Option<&ActiveSurface> {
        self.active.as_ref()
    }

    pub fn is_active(&self, id: SurfaceId) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.request.id == id)
    }

    /// 打开 surface。如果已有 active surface 则替换。
    pub fn open(&mut self, request: SurfaceRequest, focus_to_restore: Option<FocusHandle>) {
        self.active = Some(ActiveSurface {
            request,
            focus_to_restore,
        });
    }

    /// 关闭当前 surface。返回焦点句柄供调用方恢复。
    pub fn dismiss(&mut self) -> Option<FocusHandle> {
        self.active
            .take()
            .and_then(|active| active.focus_to_restore)
    }
}
