//! Editor —— 可嵌入文本编辑组件。

mod blink_manager;
mod display_map;
mod element;
mod gutter;
mod scroll;
mod scrollbar;
mod selection;
mod view;

pub use view::{
    Backspace, Copy, Cut, Delete, DeleteToBeginningOfLine, DeleteToEndOfLine, DeleteToNextWordEnd,
    DeleteToPreviousWordStart, DiffHunk, DiffHunkKind, Editor, EditorEvent, ExpandSelection,
    Indent, MoveDown, MoveLeft, MovePageDown, MovePageUp, MoveRight, MoveToBeginning,
    MoveToBeginningOfLine, MoveToEnd, MoveToEndOfLine, MoveToNextWord, MoveToPreviousWord, MoveUp,
    Newline, Outdent, Paste, Redo, SelectAll, SelectDown, SelectLeft, SelectPageDown, SelectPageUp,
    SelectRight, SelectToBeginning, SelectToBeginningOfLine, SelectToEnd, SelectToEndOfLine,
    SelectToNextWord, SelectToPreviousWord, SelectUp, SoftWrap, Undo,
};
