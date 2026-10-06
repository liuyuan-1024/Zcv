//! 语言注册、tree-sitter 语法树与高亮查询。
//! 此文件是 `zcv-language` crate 的公共入口。

mod available_languages;
mod highlighting;
mod registry;
mod snippet;

mod highlight_cache;
mod language_buffer;
mod language_settings;
mod structure;
mod syntax_map;
mod tree_sitter_utils;

#[cfg(test)]
mod test;

pub use highlight_cache::HighlightCache;
pub use highlighting::HighlightSpan;
pub use language_buffer::{
    EditedLanguageBufferSnapshot, LanguageBuffer, LanguageBufferEvent, LanguageBufferSnapshot,
};
pub use language_settings::{IndentGuideSettings, LanguageSettings};
pub use registry::{InputScope, Language, LanguageRegistry};
pub use snippet::{
    SnippetHighlightCancellation, SnippetHighlights, highlight_snippet_with_cancellation,
};
pub use structure::{BracketPair, FoldRange, LocalBinding, NewlineIndent, OutlineItem, SyntaxNode};
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
    /// 当前语法作用域包含其中任一名称时禁用该配对。
    pub not_in: &'static [&'static str],
}

/// JSX/TSX 标签自动闭合的语法节点配置。
///
/// 与通用括号配对分离：它描述的是标签结构节点的名称与命名子节点，供输入 `>` 后判断开放标签与已闭合状态使用。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JsxTagAutoCloseConfig {
    /// 开放标签节点种类，例如 `jsx_opening_element`。
    pub open_tag_node_name: &'static str,
    /// 闭合标签节点种类，例如 `jsx_closing_element`。
    pub close_tag_node_name: &'static str,
    /// 同时包含开闭标签的完整元素节点种类，例如 `jsx_element`。
    pub jsx_element_node_name: &'static str,
    /// 描述标签名的命名子节点种类，例如 `identifier`。
    pub tag_name_node_name: &'static str,
    /// 标签名节点的替代种类，例如 TSX 的成员表达式 `member_expression`。
    pub tag_name_node_alternates: &'static [&'static str],
}

/// 语言配置中的块注释续行格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockCommentConfig {
    pub start: &'static str,
    pub prefix: &'static str,
    pub end: &'static str,
    pub tab_size: usize,
}

/// 语言配置中的有序列表标记格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrderedListConfig {
    pub pattern: &'static str,
    pub format: &'static str,
}

/// 语言配置中的任务列表续行格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskListConfig {
    pub prefixes: &'static [&'static str],
    pub continuation: &'static str,
}

/// 语言层拥有的换行、注释和列表输入政策。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LanguageInputConfig {
    pub line_comments: &'static [&'static str],
    pub block_comment: Option<BlockCommentConfig>,
    pub element_block_comment: Option<BlockCommentConfig>,
    pub documentation_comment: Option<BlockCommentConfig>,
    pub unordered_list: &'static [&'static str],
    pub ordered_list: &'static [OrderedListConfig],
    pub task_list: Option<TaskListConfig>,
}

impl LanguageInputConfig {
    pub const fn empty() -> Self {
        Self {
            line_comments: &[],
            block_comment: None,
            element_block_comment: None,
            documentation_comment: None,
            unordered_list: &[],
            ordered_list: &[],
            task_list: None,
        }
    }
}
