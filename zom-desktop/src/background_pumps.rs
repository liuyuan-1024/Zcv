//! 后台产物收割与编辑后同步拍点。

use crate::shell::features::panels::search::SearchRuntimeHandle;
use crate::text_target_hub::TextTargetHub;
use crate::workspace_session::WorkspaceSession;

#[derive(Default)]
pub(crate) struct BackgroundPumps;

impl BackgroundPumps {
    pub(crate) fn new() -> Self {
        Self
    }

    pub(crate) fn after_text_edit(
        &mut self,
        session: &mut WorkspaceSession,
        text_targets: &TextTargetHub,
    ) {
        Self::pump_active_buffer_post_edit(session);
        text_targets.sync_active_buffer_search(session);
    }

    pub(crate) fn pump_pending_highlights(&mut self, session: &mut WorkspaceSession) {
        session.workspace_mut().pump_pending_highlights();
    }

    pub(crate) fn pump_pending_search(&mut self, session: &mut WorkspaceSession) {
        let (workspace, views) = session.parts_mut();
        SearchRuntimeHandle::pump_active_buffer_search(workspace, views);
    }

    pub(crate) fn pump_active_viewport_hint(&mut self, session: &mut WorkspaceSession) {
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
