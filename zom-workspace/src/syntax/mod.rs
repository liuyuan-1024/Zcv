//! 语法高亮子系统：见 [`zom-desktop/docs/桌面端语法高亮.md`](../../../zom-desktop/docs/桌面端语法高亮.md)。
//!
//! 调度层、provider trait 与注册表都寄存于本模块；
//! 具体 Tier 1 provider 实例在 `providers/` 子模块下。

mod coordinator;
mod engine;
mod language;
mod payload;
mod provider;
mod tree;
mod worker;

pub mod providers;

pub use coordinator::{BufferSyntax, MAX_HIGHLIGHT_BYTES};
pub use engine::SyntaxEngine;
pub use language::{LanguageDetector, LanguageId, LanguageRegistry, ProviderFactory};
pub use payload::{HighlightName, HighlightSpan, TokenModifiers};
pub use provider::{BufferHandle, HighlightProvider};
pub use providers::install_builtin_providers;
pub use tree::{BufferSyntaxTree, BufferSyntaxTreeSlot, SyntaxQueryCursor};
pub use worker::SyntaxWorkerHandle;
