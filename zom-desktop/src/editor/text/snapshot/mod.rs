//! 编辑器快照层。

mod builder;
mod types;

pub(crate) use builder::{EditorSnapshotRequest, build_snapshot};
pub(crate) use types::{EditorSnapshot, RevealHint, SnapshotLine};
