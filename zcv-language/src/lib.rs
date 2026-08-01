//! 语言注册、tree-sitter 语法树与高亮查询。

mod language;
mod language_buffer;
mod syntax_map;

pub use language::Language;
pub use language_buffer::{LanguageBuffer, ParseStatus};
pub use syntax_map::{HighlightSpan, SyntaxMap, SyntaxSnapshot};
