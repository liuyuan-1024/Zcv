//! 语言注册、tree-sitter 语法树与高亮查询。

mod available_languages;
mod highlighting;
mod language;
mod language_buffer;
mod structure_queries;
mod syntax_map;
mod tree_sitter_utils;

pub use highlighting::HighlightSpan;
pub use language::Language;
pub use language_buffer::{LanguageBuffer, ParseStatus};
pub use structure_queries::{BracketPair, FoldRange, IndentRange, OutlineItem, TextObjectRange};
pub use syntax_map::{SyntaxLayerInfo, SyntaxSnapshot};
