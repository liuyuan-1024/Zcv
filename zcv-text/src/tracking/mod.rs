//! 文本位置与区间跟随能力。
//!
//! Anchor / Mark 表达单点跟随，TrackedRange 表达由两个 Anchor 组成的区间跟随。

mod anchor;
mod policy;
mod tracked_range;
mod update;

pub use anchor::Anchor;
pub use policy::{
    TrackedRangeCollapsePolicy, TrackedRangeInvalidationPolicy, TrackedRangeUpdatePolicy,
};
pub use tracked_range::TrackedRange;
pub use update::TrackedRangeUpdate;
