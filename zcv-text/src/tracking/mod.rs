//! 文本位置与区间跟随能力。
//!
//! Anchor 表达随文本编辑推进的单点位置。

mod anchor;
mod edit_log;

pub use anchor::Anchor;
pub(crate) use edit_log::EditLog;
