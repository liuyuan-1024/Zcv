//! 文本位置与区间跟随能力。
//!
//! Anchor 表达随文本编辑推进的单点位置。

mod anchor;
mod coordinate_index;
mod edit_log;
mod insertion_index;
mod locator;

pub use anchor::Anchor;
pub(crate) use coordinate_index::CoordinateIndex;
pub(crate) use edit_log::EditLog;
pub(crate) use insertion_index::InsertionIndex;
