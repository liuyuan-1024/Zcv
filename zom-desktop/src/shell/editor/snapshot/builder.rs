//! 编辑器快照构建。

use zom_engine::{Buffer, Line, SelectionSet, Viewport};

use super::{EditorSnapshot, SnapshotLine};

/// 构造编辑器渲染快照时使用的视口请求。
///
/// 所有嵌入点都走同一种请求：单行输入框只是 `top_line = 0`、
/// `visible_line_count = 1`，主编辑区则由 `zom-view::ViewportState` 提供值。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EditorSnapshotRequest {
    pub(crate) top_line: u64,
    pub(crate) visible_line_count: u64,
}

impl EditorSnapshotRequest {
    pub(crate) fn single_line() -> Self {
        Self {
            top_line: 0,
            visible_line_count: 1,
        }
    }

    pub(crate) fn viewport(top_line: u64, visible_line_count: u64) -> Self {
        Self {
            top_line,
            visible_line_count,
        }
    }
}

/// 统一快照入口：所有可嵌入编辑器都从这里把 `Buffer + SelectionSet + 视口`
/// 投影成 owned [`EditorSnapshot`]。
pub(crate) fn build_snapshot(
    buffer: &Buffer,
    selection: &SelectionSet,
    request: EditorSnapshotRequest,
) -> EditorSnapshot {
    let (lines, total_lines, viewport_start_line, cursor_position, cursor_byte) =
        build_snapshot_view(
            buffer,
            selection,
            request.top_line,
            request.visible_line_count,
        );
    EditorSnapshot {
        lines,
        total_lines,
        viewport_start_line,
        cursor_byte,
        cursor_position,
        selection: selection.clone(),
        reveal: None,
        search_hits: Vec::new(),
        search_current: None,
    }
}

fn build_snapshot_view(
    buffer: &Buffer,
    selection: &SelectionSet,
    viewport_start_line: u64,
    viewport_line_count: u64,
) -> (Vec<SnapshotLine>, u64, u64, (u64, u64), usize) {
    let total_lines = buffer.line_count() as u64;
    let cursor_byte = selection.primary().head().get();
    let cursor_position = buffer
        .byte_to_position(selection.primary().head())
        .map(|pos| (pos.line().get() as u64, pos.column().get() as u64))
        .unwrap_or((0, 0));

    if total_lines == 0 || viewport_line_count == 0 {
        return (Vec::new(), total_lines, 0, cursor_position, cursor_byte);
    }

    let clamped_start = viewport_start_line.min(total_lines.saturating_sub(1));
    let viewport = Viewport::new(
        Line::new(clamped_start as usize),
        viewport_line_count as usize,
    );
    let lines = match buffer.slice_viewport(viewport) {
        Ok(slice) => slice
            .lines()
            .iter()
            .map(|visible| SnapshotLine {
                line_index: visible.line().get() as u64,
                start_byte: visible.full_range().start().get(),
                text: visible.as_str().to_string(),
            })
            .collect::<Vec<_>>(),
        Err(_) => Vec::new(),
    };
    (
        lines,
        total_lines,
        clamped_start,
        cursor_position,
        cursor_byte,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use zom_engine::BufferConfig;

    fn buffer_from(text: &str) -> Buffer {
        Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
    }

    #[test]
    fn build_snapshot_should_slice_requested_viewport() {
        let buffer = buffer_from("alpha\nbeta\ngamma");
        let snapshot = build_snapshot(
            &buffer,
            &SelectionSet::default(),
            EditorSnapshotRequest::viewport(1, 1),
        );

        assert_eq!(snapshot.total_lines, 3);
        assert_eq!(snapshot.viewport_start_line, 1);
        assert_eq!(snapshot.lines.len(), 1);
        assert_eq!(snapshot.lines[0].line_index, 1);
        assert_eq!(snapshot.lines[0].text, "beta");
    }
}
