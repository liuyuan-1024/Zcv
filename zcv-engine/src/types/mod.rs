//! 领域强类型：集中定义编辑引擎的坐标、位置、范围、版本和 Buffer 身份。
//!
//! 本模块是 public API 的语义地基，只表达类型不变量和轻量运算，不绑定 Buffer、存储或历史实现。

mod buffer_identity;
mod line_endings;
mod offsets;
mod positions;
mod ranges;
mod versions;

pub use buffer_identity::{BufferOrigin, BufferState, OriginKind};
pub use line_endings::LineEndingStyle;
pub use offsets::{ByteOffset, CharOffset, Utf16Offset};
pub use positions::{DisplayColumn, Line, LogicalColumn, Position, Utf16Position};
pub use ranges::{LineRange, TextRange};
pub(crate) use versions::BufferId;
pub use versions::{BufferVersion, TransactionId};
