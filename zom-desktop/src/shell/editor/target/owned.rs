//! 自持文本目标：小输入框自己的 buffer 与 selection。

use zom_command::EditTarget;
use zom_engine::{Buffer, BufferConfig, ByteOffset, Selection, SelectionSet};

use crate::shell::editor::input::{ImeQueryTarget, ImeTarget};
use crate::shell::editor::snapshot::{EditorSnapshot, EditorSnapshotRequest, build_snapshot};

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

    /// 预填一段文本并选中全部内容。重命名输入框走这条路径——按下 mod-r 时名称已被全选，用户继续敲键直接覆盖；按 ← / → 可移动光标后微调。
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

    /// 把自身暴露成一次编辑命令的作用目标。
    pub(crate) fn as_edit_target(&mut self) -> EditTarget<'_> {
        EditTarget {
            buffer: &mut self.buffer,
            selection: &mut self.selection,
        }
    }

    /// 把自身暴露成 IME 作用目标。
    pub(crate) fn as_ime_target(&mut self) -> ImeTarget<'_> {
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

        assert_eq!(snapshot.viewport_start_line, 1);
        assert_eq!(snapshot.lines.len(), 1);
        assert_eq!(snapshot.lines[0].text, "beta");
    }
}
