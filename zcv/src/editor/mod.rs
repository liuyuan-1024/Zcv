//! Editor —— 可嵌入文本编辑组件。

pub(crate) mod blink_manager;
pub(crate) mod display_map;
mod element;
mod scroll;
mod selection;
pub(crate) mod view;

pub(crate) use view::{
    Backspace, Copy, Cut, Delete, Editor, Indent, MoveDown, MoveLeft, MoveRight,
    MoveToBeginningOfLine, MoveToEndOfLine, MoveToNextWord, MoveToPreviousWord, MoveUp, Newline,
    Outdent, Paste, Redo, SelectAll, SelectDown, SelectLeft, SelectRight, SelectToBeginningOfLine,
    SelectToEndOfLine, SelectToNextWord, SelectToPreviousWord, SelectUp, Undo,
};
