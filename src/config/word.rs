//! 词边界分类策略：为 M6B movement 提供 identifier 与 symbol 的字符判定。
//!
//! 本文件不实现移动算法，只提供可配置的字符类别规则。

use super::display::is_combining_mark;

/// M6B 词边界策略。
///
/// 引擎层只定义纯文本移动语义，不绑定具体 UI 快捷键。不同宿主可以把
/// Option/Alt/Ctrl + Left/Right 映射到 Word / Identifier / Subword / Symbol。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WordBoundaryPolicy {
    /// `_` 是否视为 identifier 的一部分。
    ///
    /// 默认开启，适配 `snake_case`、Rust / C / JS 常见标识符。
    pub underscore_is_identifier: bool,
    /// `$` 是否视为 identifier 的一部分。
    ///
    /// 默认开启，适配 JS / shell / 部分模板语言常见标识符。
    pub dollar_is_identifier: bool,
    /// ASCII apostrophe 是否允许出现在自然语言 word 内。
    ///
    /// 当前 M6B 的 Unicode word movement 主要依赖 `unicode-segmentation`，该字段
    /// 保留给后续更细的自然语言策略；identifier / subword / symbol 不使用它。
    pub apostrophe_is_word: bool,
}

impl WordBoundaryPolicy {
    pub const fn new(
        underscore_is_identifier: bool,
        dollar_is_identifier: bool,
        apostrophe_is_word: bool,
    ) -> Self {
        Self {
            underscore_is_identifier,
            dollar_is_identifier,
            apostrophe_is_word,
        }
    }

    pub(crate) fn is_identifier_continue(self, ch: char) -> bool {
        ch.is_alphanumeric()
            || is_combining_mark(ch)
            || (self.underscore_is_identifier && ch == '_')
            || (self.dollar_is_identifier && ch == '$')
    }

    pub(crate) fn is_symbol_char(self, ch: char) -> bool {
        !ch.is_whitespace() && !self.is_identifier_continue(ch)
    }
}

impl Default for WordBoundaryPolicy {
    fn default() -> Self {
        Self {
            underscore_is_identifier: true,
            dollar_is_identifier: true,
            apostrophe_is_word: false,
        }
    }
}
