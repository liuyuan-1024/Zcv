//! 每窗口 overlay 状态管理。

use gpui::{Context, FocusHandle};

use super::{OverlayAnchor, OverlayKind};

/// 当前活跃 overlay 的完整运行时状态。
#[derive(Clone)]
pub(crate) struct ActiveOverlay {
    kind: OverlayKind,
    anchor: OverlayAnchor,
    focus_to_restore: FocusHandle,
}

impl ActiveOverlay {
    pub(crate) fn kind(&self) -> OverlayKind {
        self.kind
    }

    pub(crate) fn anchor(&self) -> &OverlayAnchor {
        &self.anchor
    }
}

/// 每窗口 overlay 管理器。第一版同时最多一个 active overlay。
#[derive(Default)]
pub(crate) struct OverlayManager {
    active: Option<ActiveOverlay>,
}

impl OverlayManager {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn active(&self) -> Option<&ActiveOverlay> {
        self.active.as_ref()
    }

    pub(crate) fn is_active(&self, kind: OverlayKind) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.kind == kind)
    }

    pub(crate) fn open(
        &mut self,
        kind: OverlayKind,
        anchor: OverlayAnchor,
        focus_to_restore: FocusHandle,
        cx: &mut Context<Self>,
    ) {
        self.active = Some(ActiveOverlay {
            kind,
            anchor,
            focus_to_restore,
        });
        cx.notify();
    }

    /// 关闭当前 overlay。返回打开时记录的焦点句柄供调用方恢复焦点。
    pub(crate) fn dismiss(&mut self, cx: &mut Context<Self>) -> Option<FocusHandle> {
        let focus = self.active.take().map(|active| active.focus_to_restore);
        if focus.is_some() {
            cx.notify();
        }
        focus
    }
}
