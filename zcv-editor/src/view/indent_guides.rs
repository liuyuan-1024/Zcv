use std::ops::Range;
use std::time::Duration;

use gpui::{App, AppContext as _, Context, Task, Window};
use zcv_actions::ToggleIndentGuides;
use zcv_multi_buffer::IndentGuide;
use zcv_text::{Line, LineIndent};

use super::{Editor, EditorMode};

#[derive(Default)]
pub(super) struct ActiveIndentGuidesState {
    display_version: u64,
    cursor_row: Option<Line>,
    enclosing: Option<(Range<Line>, LineIndent)>,
    pending_refresh: Option<Task<()>>,
    dirty: bool,
}

impl Editor {
    pub(crate) fn indent_guides(&self, rows: Range<Line>, cx: &App) -> Vec<IndentGuide> {
        if self.mode == EditorMode::SingleLine {
            return Vec::new();
        }
        let snapshot = self.display_snapshot(cx);
        let buffer = snapshot.buffer_snapshot();
        if self.show_indent_guides == Some(false)
            || (self.show_indent_guides.is_none()
                && buffer.is_singleton()
                && !buffer.language_settings().indent_guides.enabled)
        {
            return Vec::new();
        }
        buffer
            .indent_guides_in_range(rows, self.show_indent_guides == Some(true))
            .into_iter()
            .filter(|guide| {
                if self.is_buffer_folded(guide.buffer_id, cx) {
                    return false;
                }
                let start = buffer
                    .line_start_byte(guide.start_row)
                    .expect("引导线起始行必须存在");
                let end = buffer
                    .line_start_byte(guide.end_row)
                    .expect("引导线结束行必须存在");
                !snapshot
                    .fold_range_covering_offset(start)
                    .is_some_and(|(fold_start, fold_end)| fold_start < start && fold_end >= end)
            })
            .collect()
    }

    pub(crate) fn active_indent_guide_indices(
        &mut self,
        guides: &[IndentGuide],
        cx: &mut Context<Self>,
    ) -> Vec<usize> {
        if guides.is_empty() {
            return Vec::new();
        }
        let snapshot = self.display_snapshot(cx);
        let cursor = self.resolved_selections(cx).primary().head();
        let Some(cursor_row) = snapshot.buffer_snapshot().byte_to_line(cursor).ok() else {
            return Vec::new();
        };
        let state = &mut self.active_indent_guides;
        if state.display_version != snapshot.version() || state.cursor_row != Some(cursor_row) {
            state.display_version = snapshot.version();
            state.cursor_row = Some(cursor_row);
            state.enclosing = None;
            state.dirty = true;
        }
        if state.dirty && state.pending_refresh.is_none() {
            state.dirty = false;
            let version = snapshot.version();
            let task = cx.background_spawn(async move {
                snapshot.buffer_snapshot().enclosing_indent(cursor_row)
            });
            match cx
                .foreground_executor()
                .block_with_timeout(Duration::from_micros(200), task)
            {
                Ok(enclosing) => state.enclosing = enclosing,
                Err(task) => {
                    state.pending_refresh = Some(cx.spawn(async move |editor, cx| {
                        let enclosing = task.await;
                        editor
                            .update(cx, |editor, cx| {
                                let state = &mut editor.active_indent_guides;
                                state.pending_refresh = None;
                                if state.display_version == version
                                    && state.cursor_row == Some(cursor_row)
                                {
                                    state.enclosing = enclosing;
                                } else {
                                    state.dirty = true;
                                }
                                cx.notify();
                            })
                            .ok();
                    }));
                    return Vec::new();
                }
            }
        }
        let enclosing = state.enclosing.clone();
        let Some((range, indent)) = enclosing else {
            return Vec::new();
        };
        guides
            .iter()
            .enumerate()
            .filter(|(_, guide)| {
                guide.indent_level() == indent.len(guide.tab_size)
                    && range.start <= guide.end_row
                    && guide.start_row <= range.end
            })
            .map(|(index, _)| index)
            .collect()
    }

    pub(crate) fn toggle_indent_guides(
        &mut self,
        _: &ToggleIndentGuides,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let currently_enabled = self.show_indent_guides.unwrap_or_else(|| {
            let snapshot = self.display_snapshot(cx);
            !snapshot.buffer_snapshot().is_singleton()
                || snapshot
                    .buffer_snapshot()
                    .language_settings()
                    .indent_guides
                    .enabled
        });
        self.show_indent_guides = Some(!currently_enabled);
        cx.notify();
    }
}
