//! 规范 highlight name 词汇表。
//!
//! 树标 capture name 与 LSP semantic tokens **都**映射为这套名字——主题只对这一套名字配色。
//! 名字采用 tree-sitter capture name 命名空间（点分前缀层级），这是现存的主题约定。
//!
//! `color_for` 的点分前缀回退提供二级安全网：若主题没有 `variable.parameter`，退到 `variable`，再退到 `default_fg`。
//!
//! ## 来源
//!
//! 词汇表 = 所有已支持语言 tree-sitter crate 内嵌 `HIGHLIGHTS_QUERY` capture 的并集。

// =============================================================================
// 规范性：单层名（点分前缀回退的"根"）
// =============================================================================

pub const ATTRIBUTE: &str = "attribute";
pub const BOOLEAN: &str = "boolean";
pub const CHARSET: &str = "charset";
pub const COMMENT: &str = "comment";
pub const CONSTANT: &str = "constant";
pub const CONSTRUCTOR: &str = "constructor";
pub const EMBEDDED: &str = "embedded";
pub const ESCAPE: &str = "escape";
pub const FUNCTION: &str = "function";
pub const IMPORT: &str = "import";
pub const KEYFRAMES: &str = "keyframes";
pub const KEYWORD: &str = "keyword";
pub const LABEL: &str = "label";
pub const MEDIA: &str = "media";
pub const NAMESPACE: &str = "namespace";
pub const NUMBER: &str = "number";
pub const OPERATOR: &str = "operator";
pub const PROPERTY: &str = "property";
pub const PUNCTUATION: &str = "punctuation";
pub const STRING: &str = "string";
pub const SUPPORTS: &str = "supports";
pub const TAG: &str = "tag";
pub const TYPE: &str = "type";
pub const VARIABLE: &str = "variable";

// =============================================================================
// 子层级名 —— 按父名分组
// =============================================================================

// --- comment.* ---
pub const COMMENT_DOCUMENTATION: &str = "comment.documentation";

// --- constant.* ---
pub const CONSTANT_BUILTIN: &str = "constant.builtin";

// --- function.* ---
pub const FUNCTION_BUILTIN: &str = "function.builtin";
pub const FUNCTION_MACRO: &str = "function.macro";
pub const FUNCTION_METHOD: &str = "function.method";

// --- punctuation.* ---
pub const PUNCTUATION_BRACKET: &str = "punctuation.bracket";
pub const PUNCTUATION_DELIMITER: &str = "punctuation.delimiter";
pub const PUNCTUATION_SPECIAL: &str = "punctuation.special";

// --- string.* ---
pub const STRING_ESCAPE: &str = "string.escape";
pub const STRING_SPECIAL: &str = "string.special";
pub const STRING_SPECIAL_KEY: &str = "string.special.key";

// --- tag.* ---
pub const TAG_ERROR: &str = "tag.error";

// --- type.* ---
pub const TYPE_BUILTIN: &str = "type.builtin";

// --- variable.* ---
pub const VARIABLE_BUILTIN: &str = "variable.builtin";
pub const VARIABLE_PARAMETER: &str = "variable.parameter";

// =============================================================================
// 完整词汇表（测试/校验用）
// =============================================================================

/// 所有规范 highlight name 的完整集合。
///
/// 新增 tree-sitter 语言时应在末尾追加其特有 capture 名。
pub const ALL_HIGHLIGHT_NAMES: &[&str] = &[
    // 单层名
    ATTRIBUTE,
    BOOLEAN,
    CHARSET,
    COMMENT,
    CONSTANT,
    CONSTRUCTOR,
    EMBEDDED,
    ESCAPE,
    FUNCTION,
    IMPORT,
    KEYFRAMES,
    KEYWORD,
    LABEL,
    MEDIA,
    NAMESPACE,
    NUMBER,
    OPERATOR,
    PROPERTY,
    PUNCTUATION,
    STRING,
    SUPPORTS,
    TAG,
    TYPE,
    VARIABLE,
    // 子层级名
    COMMENT_DOCUMENTATION,
    CONSTANT_BUILTIN,
    FUNCTION_BUILTIN,
    FUNCTION_MACRO,
    FUNCTION_METHOD,
    PUNCTUATION_BRACKET,
    PUNCTUATION_DELIMITER,
    PUNCTUATION_SPECIAL,
    STRING_ESCAPE,
    STRING_SPECIAL,
    STRING_SPECIAL_KEY,
    TAG_ERROR,
    TYPE_BUILTIN,
    VARIABLE_BUILTIN,
    VARIABLE_PARAMETER,
];

// =============================================================================
// 测试
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn no_empty_name() {
        for name in ALL_HIGHLIGHT_NAMES {
            assert!(!name.is_empty());
        }
    }

    #[test]
    fn all_lowercase_or_underscore() {
        for name in ALL_HIGHLIGHT_NAMES {
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_' || c == '.'),
                "非法字符: {name}"
            );
        }
    }

    #[test]
    fn no_trailing_dot() {
        for name in ALL_HIGHLIGHT_NAMES {
            assert!(!name.ends_with('.'), "尾随点: {name}");
            assert!(!name.starts_with('.'), "前导点: {name}");
        }
    }

    #[test]
    fn all_names_unique() {
        let mut seen = HashSet::new();
        for name in ALL_HIGHLIGHT_NAMES {
            assert!(seen.insert(*name), "重复: {name}");
        }
    }

    #[test]
    fn no_empty_dot_segments() {
        for name in ALL_HIGHLIGHT_NAMES {
            for seg in name.split('.') {
                assert!(!seg.is_empty(), "空段在: {name}");
            }
        }
    }

    #[test]
    fn dot_parent_names_exist_for_all_dotted_children() {
        let set: HashSet<&str> = ALL_HIGHLIGHT_NAMES.iter().copied().collect();
        for name in &set {
            let mut current = *name;
            while let Some(dot) = current.rfind('.') {
                let parent = &current[..dot];
                assert!(
                    set.contains(parent),
                    "缺少父名 '{parent}'（子名 '{name}' 需要）"
                );
                current = parent;
            }
        }
    }
}
