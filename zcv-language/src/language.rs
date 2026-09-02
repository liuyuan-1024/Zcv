//! 语言注册、tree-sitter 语法树与高亮查询。
//! 此文件是 `zcv-language` crate 的公共入口。

mod available_languages;
mod highlighting;
mod registry;

mod language_buffer;
mod structure_queries;
mod syntax_map;
mod tree_sitter_utils;

pub use highlighting::HighlightSpan;
pub use language_buffer::{LanguageBuffer, LanguageBufferEvent};
pub use registry::{Language, language_for_file};
pub use structure_queries::{BracketPair, FoldRange, NewlineIndent};
pub use syntax_map::SyntaxSnapshot;

/// 输入级自动闭合配对。
///
/// 决定键入 `start` 时编辑器是否自动补全 `end`、选中文本时是否用该对包裹；
/// 与语法级 `brackets.scm` 查询（折叠、括号跳转）互不相干。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutoClosePair {
    pub start: &'static str,
    pub end: &'static str,
    /// 键入 `start` 时自动补全 `end`。
    pub close: bool,
    /// 选中文本时键入 `start` 用该对包裹选区。
    pub surround: bool,
    /// 光标处于该对之间时按回车额外补一个空行（闭合符前回退到基准缩进）。
    pub newline: bool,
}
