//! 词边界分类策略：为文本移动提供 word / identifier / subword / symbol 的字符判定。
//!
//! 本文件不实现移动扫描，只集中维护可配置的字符类别和边界规则。

use crate::selection::MovementUnit;
use unicode_width::UnicodeWidthChar;

/// 零宽字符（组合音标、ZWJ、变体选择符等）判定。
///
/// 这类字符不占显示列宽，并应与其前后字符保持为一个词。
/// 控制字符宽度未定义（返回 `None`），不属于零宽字符，由调用方按控制字符处理。
fn is_zero_width(ch: char) -> bool {
    UnicodeWidthChar::width(ch) == Some(0)
}

/// 词边界策略。
///
/// 引擎层只定义纯文本移动语义，不绑定具体 UI 快捷键。
/// 不同宿主可以把 Option/Alt/Ctrl + Left/Right 映射到 Word / Identifier / Subword / Symbol。
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WordBoundaryClassifier {
    policy: WordBoundaryPolicy,
    unit: MovementUnit,
}

impl WordBoundaryPolicy {
    pub const fn new(underscore_is_identifier: bool, dollar_is_identifier: bool) -> Self {
        Self {
            underscore_is_identifier,
            dollar_is_identifier,
        }
    }

    pub(crate) fn is_identifier_continue(self, ch: char) -> bool {
        ch.is_alphanumeric()
            || is_zero_width(ch)
            || (self.underscore_is_identifier && ch == '_')
            || (self.dollar_is_identifier && ch == '$')
    }

    pub(crate) fn is_symbol_char(self, ch: char) -> bool {
        !ch.is_whitespace() && !self.is_identifier_continue(ch)
    }

    pub(crate) fn classifier(self, unit: MovementUnit) -> Option<WordBoundaryClassifier> {
        match unit {
            MovementUnit::Word
            | MovementUnit::Identifier
            | MovementUnit::Subword
            | MovementUnit::Symbol => Some(WordBoundaryClassifier { policy: self, unit }),
            MovementUnit::Grapheme | MovementUnit::LineEdge => None,
        }
    }
}

impl WordBoundaryClassifier {
    pub(crate) fn is_subword(self) -> bool {
        self.unit == MovementUnit::Subword
    }

    pub(crate) fn is_body(self, ch: char) -> bool {
        match self.unit {
            MovementUnit::Word | MovementUnit::Subword => is_natural_word_body(ch),
            MovementUnit::Identifier => self.policy.is_identifier_continue(ch),
            MovementUnit::Symbol => self.policy.is_symbol_char(ch),
            MovementUnit::Grapheme | MovementUnit::LineEdge => unreachable!("non-word classifier"),
        }
    }

    pub(crate) fn should_start_new_subword(
        self,
        previous: char,
        current: char,
        next: Option<char>,
    ) -> bool {
        debug_assert!(self.is_subword());

        if is_zero_width(current) || is_zero_width(previous) {
            return false;
        }

        (previous.is_lowercase() && current.is_uppercase())
            || (previous.is_alphabetic() && current.is_numeric())
            || (previous.is_numeric() && current.is_alphabetic())
            || (previous.is_uppercase()
                && current.is_uppercase()
                && next.is_some_and(char::is_lowercase))
    }
}

fn is_natural_word_body(ch: char) -> bool {
    ch.is_alphanumeric() || is_zero_width(ch)
}

impl Default for WordBoundaryPolicy {
    fn default() -> Self {
        Self {
            underscore_is_identifier: true,
            dollar_is_identifier: true,
        }
    }
}
