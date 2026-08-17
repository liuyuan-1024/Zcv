//! 语言注册、tree-sitter 语法树与高亮查询。
//! 此文件是 `zcv-language` crate 的公共入口。

mod available_languages;
mod highlighting;
mod registry;

pub(crate) use registry as language;
mod language_buffer;
mod structure_queries;
mod syntax_map;
mod tree_sitter_utils;

pub use highlighting::HighlightSpan;
pub use language_buffer::{LanguageBuffer, ParseStatus};
pub use registry::Language;
pub use structure_queries::{
    BracketPair, FoldRange, IndentRange, NewlineIndent, OutlineItem, TextObjectRange,
};
pub use syntax_map::{SyntaxLayerInfo, SyntaxSnapshot};
