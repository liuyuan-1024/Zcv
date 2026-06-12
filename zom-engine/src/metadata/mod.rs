//! MetadataLayers：按 `MetadataLayerKind` 索引若干 `VersionedRangeSet<T>`。
//!
//! 本模块不再定义独立的 layer / range / spec / update 容器——它们都直接复用 `versioned` 提供的 `VersionedRangeSet` 系列。
//! Kind 是宿主侧业务分类键，仅作为 set 的索引维度。

mod kind;
mod layers;

pub use kind::MetadataLayerKind;
pub use layers::MetadataLayers;
