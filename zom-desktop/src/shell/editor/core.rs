//! 可嵌入编辑器本体：buffer、selection 与命令作用目标。

use zom_command::EditTarget;
use zom_engine::{Buffer, BufferConfig, SelectionSet};

use super::ime::{ImeQueryTarget, ImeTarget};

/// 一个独立的文本编辑单元：自持 buffer 与选区。
pub(crate) struct Editor {
    buffer: Buffer,
    /// 视图侧的权威选区；编辑命令读它、写回它（与主编辑区 view/buffer
    /// 双选区模型一致）。
    selection: SelectionSet,
}

/// 编辑器的 owned 渲染快照。
#[derive(Clone, Debug, Default)]
pub(crate) struct EditorSnapshot {
    pub(crate) text: String,
    pub(crate) cursor_byte: usize,
}

impl Editor {
    /// 新建空编辑器。空 `Buffer` 构造不涉及 IO，不会失败。
    pub(crate) fn new() -> Self {
        let buffer = Buffer::new(BufferConfig::default()).expect("空 Buffer 构造不会失败");
        Self {
            buffer,
            selection: SelectionSet::default(),
        }
    }

    /// 当前文本内容（owned 拷贝）。
    pub(crate) fn text(&self) -> String {
        self.buffer.text().into_owned()
    }

    /// 主光标的字节偏移（选区 head）。
    pub(crate) fn cursor_byte(&self) -> usize {
        self.selection.primary().head().get()
    }

    pub(crate) fn snapshot(&self) -> EditorSnapshot {
        EditorSnapshot {
            text: self.text(),
            cursor_byte: self.cursor_byte(),
        }
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

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}
