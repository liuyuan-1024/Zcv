//! Editor —— 可嵌入文本编辑组件。

mod blink_manager;
mod display_map;
mod element;
mod scroll;
mod selection;
mod view;

pub use view::{
    Backspace, Copy, Cut, Delete, Editor, EditorEvent, Indent, MoveDown, MoveLeft, MoveRight,
    MoveToBeginning, MoveToBeginningOfLine, MoveToEnd, MoveToEndOfLine, MoveToNextWord,
    MoveToPreviousWord, MoveUp, Newline, Outdent, Paste, Redo, SelectAll, SelectDown, SelectLeft,
    SelectRight, SelectToBeginning, SelectToBeginningOfLine, SelectToEnd, SelectToEndOfLine,
    SelectToNextWord, SelectToPreviousWord, SelectUp, Undo,
};
