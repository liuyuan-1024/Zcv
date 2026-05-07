//! M6 光标、选区与多光标模型。
//!
//! M6 起，`SelectionSet` 是编辑引擎里的主选区模型；不再把 M3 的
//! `SelectionSnapshot` 作为兼容层继续传播。

mod composition;
mod cursor;
mod movement;
mod selection;
mod selection_set;

pub use composition::{CompositionSelection, CompositionState};
pub use cursor::Cursor;
pub use movement::{MovementDirection, MovementUnit};
pub use selection::Selection;
pub use selection_set::{SelectionMergePolicy, SelectionSet};
