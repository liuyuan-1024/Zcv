//! 引擎配置与策略系统：集中承载 Buffer 行为中可配置、可替换但不属于文本事实本身的策略。
//!
//! 本模块只定义纯数据策略及其默认值，不读取文本、不提交编辑，也不绑定具体宿主 UI。

mod buffer;
mod encoding;
mod large_file;
mod line_endings;
mod word;

pub use buffer::{BufferConfig, TabConfig};
pub use encoding::EncodingConfig;
pub use large_file::{LargeFilePolicy, LargeTransactionPolicy};
pub use line_endings::{LineEndingConfig, PositionEncodingConfig};
pub(crate) use word::WordBoundaryClassifier;
pub use word::WordBoundaryPolicy;
