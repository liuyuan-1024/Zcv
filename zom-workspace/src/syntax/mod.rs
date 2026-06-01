//! 语法高亮子系统：见 [`zom-desktop/docs/桌面端语法高亮.md`](../../../zom-desktop/docs/桌面端语法高亮.md)。
//!
//! 调度层、provider trait 与注册表都寄存于本模块；具体 Tier 1 provider
//! 实例在 `providers/` 子模块下。

mod coordinator;
mod language;
mod payload;
mod provider;
mod sink;
mod worker;

pub mod providers;

pub use coordinator::{BufferSyntaxState, MAX_HIGHLIGHT_BYTES};
pub use language::{LanguageDetector, LanguageId, LanguageRegistry, ProviderFactory};
pub use payload::{HighlightName, HighlightSpan, TokenModifiers, syntax_layer_kind};
pub use provider::{BufferHandle, HighlightProvider};
pub use sink::{HighlightSink, SinkMessage};
pub use worker::SyntaxWorkerHandle;
