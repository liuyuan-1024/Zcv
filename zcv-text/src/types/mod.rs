//! 领域强类型：集中定义文本内核的坐标、位置、范围、版本和 Buffer 身份。
//!
//! 本模块是 public API 的语义地基，只表达类型不变量和轻量运算，不绑定 Buffer、存储或历史实现。

mod line_endings;
mod offsets;
mod positions;
mod ranges;
mod versions;

pub use line_endings::LineEndingStyle;
pub use offsets::{ByteOffset, CharOffset, Utf16Offset};
pub use positions::{Line, LogicalColumn, Position, Utf16Position};
pub use ranges::{LineRange, TextRange};
pub use versions::{BufferVersion, TransactionId};
