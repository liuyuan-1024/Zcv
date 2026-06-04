//! 后台产物收割与编辑后同步拍点。
//!
//! 工作区自家的两条线（语法高亮 drain、活动 buffer 的 post_edit 扇出、
//! viewport hint 推送）保持为内置入口；其它跨 feature 的"编辑后同步"与
//! "每帧 drain"通过 [`PostEditObserver`] / [`FramePump`] 两个端口注册，
//! 让 BackgroundPumps 不必认识具体 feature。

use crate::workspace_session::WorkspaceSession;

/// 编辑后同步端口：每次活动 buffer 上产生编辑事件后被调一次。
///
/// 顺序保证：built-in 的 `pump_active_buffer_post_edit` 先跑（把 buffer 上的DeltaEvent 扇出给搜索 / 语法 provider），然后才轮到注册的观察者
/// ——后者通常依赖前者已经把事件推到自己关心的状态机里。
pub(crate) trait PostEditObserver {
    fn after_text_edit(&self, session: &mut WorkspaceSession);
}

/// 每帧端口：每帧 prepaint 起手按注册顺序被调一次。
///
/// 不保证调用线程之外的协调；实现内部如果有 RefCell，自己负责借用周期。
pub(crate) trait FramePump {
    fn pump(&self, session: &mut WorkspaceSession);
}

#[derive(Default)]
pub(crate) struct BackgroundPumps {
    post_edit_observers: Vec<Box<dyn PostEditObserver>>,
    frame_pumps: Vec<Box<dyn FramePump>>,
}

impl BackgroundPumps {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn install_post_edit_observer(&mut self, observer: Box<dyn PostEditObserver>) {
        self.post_edit_observers.push(observer);
    }

    pub(crate) fn install_frame_pump(&mut self, pump: Box<dyn FramePump>) {
        self.frame_pumps.push(pump);
    }

    /// 编辑后扇出 + 通知所有注册的 [`PostEditObserver`]。
    /// 必须先于观察者跑 built-in 的活动 buffer post_edit，否则观察者读到的状态
    /// 还停在旧版本。
    pub(crate) fn after_text_edit(&self, session: &mut WorkspaceSession) {
        Self::pump_active_buffer_post_edit(session);
        for observer in &self.post_edit_observers {
            observer.after_text_edit(session);
        }
    }

    pub(crate) fn pump_pending_highlights(&self, session: &mut WorkspaceSession) {
        session.workspace_mut().pump_pending_highlights();
    }

    /// 跑所有注册的 [`FramePump`]——具体哪些 feature 在跑由 ShellRuntime 决定，
    /// BackgroundPumps 不认。
    pub(crate) fn pump_frame_observers(&self, session: &mut WorkspaceSession) {
        for pump in &self.frame_pumps {
            pump.pump(session);
        }
    }

    pub(crate) fn pump_active_viewport_hint(&self, session: &mut WorkspaceSession) {
        let Some(view) = session.views().active_view() else {
            return;
        };
        let buffer_id = view.buffer();
        let viewport = view.viewport();
        let Some(wb) = session.workspace().buffer(buffer_id) else {
            return;
        };
        let snapshot = wb.buffer().snapshot();
        let total_lines = snapshot.line_count();
        if total_lines == 0 {
            return;
        }
        const PAD_LINES: u64 = 32;
        let start_line = viewport.top_line.saturating_sub(PAD_LINES);
        let raw_end = viewport
            .top_line
            .saturating_add(viewport.visible_logical_lines)
            .saturating_add(PAD_LINES);
        let end_line = raw_end.min(total_lines as u64);
        if start_line >= end_line {
            return;
        }
        let Ok(start_byte) = snapshot.line_start_byte(zom_engine::Line::new(start_line as usize))
        else {
            return;
        };
        let end_byte = if end_line >= total_lines as u64 {
            snapshot.len_bytes()
        } else {
            match snapshot.line_start_byte(zom_engine::Line::new(end_line as usize)) {
                Ok(b) => b,
                Err(_) => snapshot.len_bytes(),
            }
        };
        if start_byte >= end_byte {
            return;
        }
        let Ok(range) = zom_engine::TextRange::new(start_byte, end_byte) else {
            return;
        };
        session
            .workspace_mut()
            .set_buffer_viewport_hint(buffer_id, Some(range));
    }

    fn pump_active_buffer_post_edit(session: &mut WorkspaceSession) {
        if let Some(wb) = session.workspace_mut().active_buffer_mut() {
            let _ = wb.pump_post_edit();
        }
    }
}
