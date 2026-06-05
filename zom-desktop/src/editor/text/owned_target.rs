//! 自持文本目标：小输入框自己的 buffer 与 selection。

use zom_command::EditTarget;
use zom_engine::{Buffer, BufferConfig, ByteOffset, Selection, SelectionSet};

use crate::editor::text::snapshot::build_snapshot;
use crate::editor::text::{EditorSnapshot, EditorSnapshotRequest, ImeQueryTarget, ImeTarget};

/// 一个独立的文本编辑目标：自持 buffer 与选区。
pub(crate) struct OwnedEditorTarget {
    buffer: Buffer,
    /// 视图侧的权威选区；编辑命令读它、写回它（与主编辑区 view/buffer
    /// 双选区模型一致）。
    selection: SelectionSet,
}

impl OwnedEditorTarget {
    /// 新建空编辑目标。空 `Buffer` 构造不涉及 IO，不会失败。
    pub(crate) fn new() -> Self {
        let buffer = Buffer::new(BufferConfig::default()).expect("空 Buffer 构造不会失败");
        Self {
            buffer,
            selection: SelectionSet::default(),
        }
    }

    /// 预填一段文本并选中全部内容。适合覆盖式输入：用户继续敲键会直接替换原文本。
    pub(crate) fn with_text_all_selected(text: &str) -> Self {
        let buffer = Buffer::from_text(text.to_string(), BufferConfig::default())
            .expect("自持输入框文本构造不会失败");
        let len = buffer.len_bytes();
        let selection = if len == ByteOffset::ZERO {
            SelectionSet::default()
        } else {
            SelectionSet::new(vec![Selection::new(ByteOffset::ZERO, len)])
        };
        Self { buffer, selection }
    }

    /// 预填一段文本，并把光标放到文本末尾。
    pub(crate) fn with_text_caret_at_end(text: &str) -> Self {
        let mut target = Self::with_text_all_selected(text);
        let len = target.buffer.len_bytes();
        target.selection = SelectionSet::caret(len);
        target
    }

    /// 当前完整文本内容（owned 拷贝）。
    ///
    /// 自持目标（搜索框 / 文件名输入 / 项目选择器）才用它——它们的 buffer
    /// 永远小，整段读取是 O(buffer size)。渲染仍走 [`Self::snapshot`] 的统一
    /// 视口切片入口。
    pub(crate) fn text(&self) -> String {
        self.buffer
            .slice_byte_range(ByteOffset::ZERO, self.buffer.len_bytes())
            .expect("自持输入框文本范围来自自身长度")
            .into_text()
            .into_owned()
    }

    pub(crate) fn snapshot(&self, request: EditorSnapshotRequest) -> EditorSnapshot {
        build_snapshot(&self.buffer, &self.selection, request)
    }

    fn sync_buffer_selection(&mut self) {
        self.buffer
            .set_selection(self.selection.clone())
            .expect("自持输入框 selection 来自自身 buffer，必须合法");
    }

    /// 把自身暴露成一次编辑命令的作用目标。
    pub(crate) fn as_edit_target(&mut self) -> EditTarget<'_> {
        self.sync_buffer_selection();
        EditTarget {
            buffer: &mut self.buffer,
            selection: &mut self.selection,
            wrap_map: None,
            visual_caret: None,
            goal_column: None,
        }
    }

    /// 把自身暴露成 IME 作用目标。
    pub(crate) fn as_ime_target(&mut self) -> ImeTarget<'_> {
        self.sync_buffer_selection();
        ImeTarget::new(&mut self.buffer, &mut self.selection)
    }

    /// 把自身暴露成 IME 查询目标。
    pub(crate) fn as_ime_query_target(&self) -> ImeQueryTarget<'_> {
        ImeQueryTarget::new(&self.buffer, &self.selection)
    }
}

impl Default for OwnedEditorTarget {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target_from(text: &str) -> OwnedEditorTarget {
        OwnedEditorTarget {
            buffer: Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap(),
            selection: SelectionSet::default(),
        }
    }

    #[test]
    fn owned_target_snapshot_should_use_requested_viewport() {
        let target = target_from("alpha\nbeta");

        let snapshot = target.snapshot(EditorSnapshotRequest::viewport(1, 1));

        assert_eq!(snapshot.top_line, 1);
        assert_eq!(snapshot.lines.len(), 1);
        assert_eq!(snapshot.lines[0].text, "beta");
    }

    #[test]
    fn with_text_caret_at_end_should_prefill_text_without_selecting_it() {
        let target = OwnedEditorTarget::with_text_caret_at_end("a.txt");

        let snapshot = target.snapshot(EditorSnapshotRequest::single_line());

        assert_eq!(target.text(), "a.txt");
        assert_eq!(snapshot.cursor_byte, "a.txt".len());
        assert!(snapshot.selection.primary().is_caret());
    }

    #[test]
    fn edit_target_should_sync_prefilled_selection_into_buffer_before_ime_commit() {
        let mut target = OwnedEditorTarget::with_text_caret_at_end("a.txt");
        let edit_target = target.as_edit_target();

        assert_eq!(edit_target.buffer.selection().primary().head().get(), 5);
    }
}
