//! 可嵌入编辑器本体：buffer、selection 与命令作用目标。

use zom_command::EditTarget;
use zom_engine::{Buffer, BufferConfig, SelectionSet};
use zom_view::RevealKind;

use super::ime::{ImeQueryTarget, ImeTarget};

/// 一个独立的文本编辑单元：自持 buffer 与选区。
pub(crate) struct Editor {
    buffer: Buffer,
    /// 视图侧的权威选区；编辑命令读它、写回它（与主编辑区 view/buffer
    /// 双选区模型一致）。
    selection: SelectionSet,
}

/// 编辑器的 owned 渲染快照。
///
/// `cursor_byte` 与 `selection` 是平行字段：前者是 primary head 的纯 byte 投影
/// （供 blink / 状态栏 / 测试这些"只关心活动光标在哪"的下游消费），
/// 后者是完整 SelectionSet（供 [`super::element::EditorElement`] 渲染多光标 caret + 选区背景）。
/// 两个字段必须从同一份 SelectionSet 派生、不允许漂移：构造快照的
/// 唯一入口在 [`Editor::snapshot`] 与 [`super::main_editor`]，那里保证一致。
#[derive(Clone, Debug, Default)]
pub(crate) struct EditorSnapshot {
    pub(crate) text: String,
    pub(crate) cursor_byte: usize,
    pub(crate) selection: SelectionSet,
    /// 外部 reveal 请求在快照里的表示。只在多行主编辑器有意义；
    /// 嵌入式单行编辑器（搜索框等）始终为 `None`。
    pub(crate) reveal: Option<RevealHint>,
}

/// [`zom_view::RevealRequest`] 的渲染端镜像 —— 把 `ByteOffset` 换成 `usize`，
/// 便于元素侧直接用，不再跨 crate 引 engine 类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RevealHint {
    pub(crate) byte: usize,
    pub(crate) kind: RevealKind,
    pub(crate) seq: u64,
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
            selection: self.selection.clone(),
            reveal: None,
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
