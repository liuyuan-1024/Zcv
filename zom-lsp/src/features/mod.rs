//! LSP 能力 feature 模块。
//!
//! 每个模块对应一组 LSP 能力，提供请求构造的类型安全封装。
//! 当前为骨架——按需实现。

pub mod completion;
pub mod diagnostics;
pub mod goto;
pub mod hover;
pub mod outline;
pub mod semantic_tokens;
