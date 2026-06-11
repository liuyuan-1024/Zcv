//! 编辑后同步拍点与跨 feature 帧 pump。
//!
//! 活动 buffer 的 post_edit 扇出仍是内置入口；
//! 跨 feature 的"编辑后同步"与"每帧 drain"通过 [`PostEditObserver`] / [`FramePump`] 两个端口（在[`crate::ports`]）注册，让 BackgroundPumps 不必认识具体 feature。
//!
//! 语法高亮没有需要 drain 的中间产物 —— paint 阶段直接从共享 `BufferSyntaxTreeSlot` 现查 tree-sitter Query。

use crate::ports::{FramePump, PostEditObserver};
use crate::workspace_session::WorkspaceSession;
use zom_view::{ViewportEditAnchor, WrapMap};

#[derive(Default)]
pub(super) struct BackgroundPumps {
    post_edit_observers: Vec<Box<dyn PostEditObserver>>,
    frame_pumps: Vec<Box<dyn FramePump>>,
}

impl BackgroundPumps {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn install_post_edit_observer(&mut self, observer: Box<dyn PostEditObserver>) {
        self.post_edit_observers.push(observer);
    }

    pub(super) fn install_frame_pump(&mut self, pump: Box<dyn FramePump>) {
        self.frame_pumps.push(pump);
    }

    /// 编辑后扇出 + 通知所有注册的 [`PostEditObserver`]。
    /// 必须先于观察者跑 built-in 的活动 buffer post_edit，否则观察者读到的状态还停在旧版本。
    pub(super) fn after_text_edit(
        &self,
        session: &mut WorkspaceSession,
        soft_wrap: bool,
        viewport_anchor: Option<ViewportEditAnchor>,
    ) {
        Self::pump_active_buffer_post_edit(session, soft_wrap, viewport_anchor);
        for observer in &self.post_edit_observers {
            observer.after_text_edit(session);
        }
    }

    /// 跑所有注册的 [`FramePump`]——具体哪些 feature 在跑由 ShellRuntime 决定，BackgroundPumps 不认。
    pub(super) fn pump_frame_observers(&self, session: &mut WorkspaceSession) {
        for pump in &self.frame_pumps {
            pump.pump(session);
        }
    }

    fn pump_active_buffer_post_edit(
        session: &mut WorkspaceSession,
        soft_wrap: bool,
        viewport_anchor: Option<ViewportEditAnchor>,
    ) {
        let active_buffer_id = session.active_buffer_id();
        let post_edit = active_buffer_id
            .and_then(|id| session.workspace_mut().buffer_mut(id))
            .and_then(|wb| {
                let events = wb.pump_post_edit().ok()?;
                let line_count = wb.buffer().line_count() as u64;
                Some((events, line_count))
            });
        let Some((events, line_count)) = post_edit else {
            return;
        };
        if events.is_empty() {
            return;
        }
        let active_view_id = session.active_edit_view_id();
        let (workspace, views) = session.parts_mut();
        let Some(buffer) = active_buffer_id.and_then(|id| workspace.buffer(id)) else {
            return;
        };
        if let Some(view) = active_view_id.and_then(|id| views.edit_view_mut(id)) {
            let wrap_map = if soft_wrap {
                view.wrap_map()
                    .map(|wm| wm.preserve_after_edit_events(buffer.buffer(), &events))
            } else {
                Some(WrapMap::sparse(false, line_count, []))
            };
            view.track_viewport_anchor_after_edit(viewport_anchor, &events);
            view.set_wrap_map(wrap_map);
        }
    }
}
