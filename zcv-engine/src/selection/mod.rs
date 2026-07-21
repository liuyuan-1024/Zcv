//! 光标、选区与多光标模型。
//!
//! `SelectionSet` 是编辑引擎里唯一的选区模型，承载多光标、IME composition 与移动语义。

mod composition;
mod core;
mod cursor;
mod movement;
mod selection_set;

pub use composition::{CompositionSelection, CompositionState};
pub use core::Selection;
pub use cursor::Cursor;
pub use movement::{Motion, MovementDirection, MovementUnit};
pub use selection_set::{SelectionMergePolicy, SelectionSet};
