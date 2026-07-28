//! 光标、选区与多光标模型。
//!
//! `SelectionSet` 是 engine 提供的唯一选区数据模型；当前视图选区由宿主持有，
//! engine 只提供校验、映射、编辑和移动语义。

mod core;
mod cursor;
mod movement;
mod selection_set;

pub use core::Selection;
pub use cursor::Cursor;
pub use movement::{MovementDirection, MovementUnit};
pub use selection_set::{SelectionMergePolicy, SelectionSet};
