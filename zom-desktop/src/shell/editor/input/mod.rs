//! 编辑器输入层。

mod handler;
mod ime;

pub(crate) use handler::{CaretLayout, EditorInput};
pub(crate) use ime::{ImeQueryTarget, ImeTarget};
