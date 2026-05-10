//! M10 MetadataLayer：把外部 metadata 绑定到可跟随文本变化的区间上。
//!
//! 本模块不定义 diagnostics、高亮、断点等业务 payload，只负责承载泛型
//! metadata、绑定 BufferVersion、复用 TrackedRange 的范围追踪语义，并提供基础查询。

mod id;
mod kind;
mod layer;
mod layers;
mod line_window;
pub(crate) mod query;
mod range;
mod range_spec;
mod update;

pub use id::MetadataRangeId;
pub use kind::MetadataLayerKind;
pub use layer::MetadataLayer;
pub use layers::MetadataLayers;
pub use line_window::MetadataLineWindow;
pub use range::MetadataRange;
pub use range_spec::MetadataRangeSpec;
pub use update::MetadataRangeUpdate;
