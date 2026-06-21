//! LSP 语义 token 解码：uint32 delta 编码 → `Vec<(TextRange, HighlightSpan)>`。
//!
//! LSP `textDocument/semanticTokens/full` 返回的 `data` 是一个 `u32` 数组，每 5 个值一组：
//! `[deltaLine, deltaStartChar, length, tokenType, tokenModifiers]`
//!
//! 解码目标：把 utf16 坐标转为字节偏移，把 token type 索引映射为规范 highlight name。
//!
//! 映射规则表在 [`super::highlight_names`] 词汇表上选择最具体的点分规范名，
//! 下游 `color_for` 的点分前缀回退在主题未定义精确名时自然退到父名。

use lsp_types::SemanticTokensLegend;
use zom_engine::{Line, Snapshot, TextRange, Utf16Offset, Utf16Position};

use super::{HighlightName, HighlightSpan, TokenModifiers};

// =============================================================================
// LSP token type → 规范 highlight name 映射表
// =============================================================================

/// 一条 LSP 语义 token 映射规则。
///
/// `token_types` 列出匹配此规则的 LSP token 类型名；
/// `highlight_name` 是产出的规范 highlight name（tree-sitter capture name 约定）。
///
/// 选择"最具体"的点分规范名，让下游 `color_for` 的点分前缀回退
/// 在主题未定义该精确名时自然退到父名。
struct LspTokenRule {
    token_types: &'static [&'static str],
    highlight_name: &'static str,
}

const LSP_RULES: &[LspTokenRule] = &[
    LspTokenRule {
        token_types: &["comment"],
        highlight_name: "comment",
    },
    LspTokenRule {
        token_types: &["keyword", "modifier"],
        highlight_name: "keyword",
    },
    LspTokenRule {
        token_types: &["string", "regexp"],
        highlight_name: "string",
    },
    LspTokenRule {
        token_types: &["number"],
        highlight_name: "number",
    },
    LspTokenRule {
        token_types: &["function"],
        highlight_name: "function",
    },
    LspTokenRule {
        token_types: &["method"],
        highlight_name: "function.method",
    },
    LspTokenRule {
        token_types: &["variable"],
        highlight_name: "variable",
    },
    LspTokenRule {
        token_types: &["parameter"],
        highlight_name: "variable.parameter",
    },
    LspTokenRule {
        token_types: &["type", "class", "enum", "interface", "struct"],
        highlight_name: "type",
    },
    LspTokenRule {
        token_types: &["typeParameter"],
        highlight_name: "type",
    },
    LspTokenRule {
        token_types: &["property"],
        highlight_name: "property",
    },
    LspTokenRule {
        token_types: &["enumMember"],
        highlight_name: "variable.other.member",
    },
    LspTokenRule {
        token_types: &["macro"],
        highlight_name: "function.macro",
    },
    LspTokenRule {
        token_types: &["operator"],
        highlight_name: "operator",
    },
    LspTokenRule {
        token_types: &["namespace"],
        highlight_name: "namespace",
    },
    LspTokenRule {
        token_types: &["decorator"],
        highlight_name: "attribute",
    },
];

/// LSP token type → 规范 highlight name。
///
/// 在规则表里线性查——16 条规则，开销可忽略。
/// 未命中退到 `"variable"`。
fn token_type_to_highlight_name(token_type: &str) -> &'static str {
    for rule in LSP_RULES {
        if rule.token_types.contains(&token_type) {
            return rule.highlight_name;
        }
    }
    "variable"
}

// =============================================================================
// 修饰位解码
// =============================================================================

fn modifiers_to_token_modifiers(mods: &[&str], bits: u32) -> TokenModifiers {
    let mut result = TokenModifiers::EMPTY;
    for (i, name) in mods.iter().enumerate() {
        if (bits >> i) & 1 != 0 {
            match *name {
                "static" => result = result.union(TokenModifiers::STATIC),
                "readonly" => result = result.union(TokenModifiers::READONLY),
                "deprecated" => result = result.union(TokenModifiers::DEPRECATED),
                "async" => result = result.union(TokenModifiers::ASYNC),
                "abstract" => result = result.union(TokenModifiers::ABSTRACT),
                _ => {}
            }
        }
    }
    result
}

// =============================================================================
// 解码入口
// =============================================================================

/// 把 LSP semantic tokens `data` 数组解码为字节范围的 highlight spans。
///
/// `data` 是 `textDocument/semanticTokens/full` 响应中的 `data` 字段；
/// `legend` 从 server capabilities 获取。
pub fn decode_semantic_tokens(
    data: &[u32],
    legend: &SemanticTokensLegend,
    snapshot: &Snapshot,
) -> Vec<(TextRange, HighlightSpan)> {
    if data.is_empty() || data.len() % 5 != 0 {
        return Vec::new();
    }

    let token_count = data.len() / 5;
    let mut spans: Vec<(TextRange, HighlightSpan)> = Vec::with_capacity(token_count);
    let mut line: u32 = 0;
    let mut start_char: u32 = 0;

    for chunk in data.chunks_exact(5) {
        let delta_line = chunk[0];
        let delta_start_char = chunk[1];
        let length = chunk[2];
        let token_type_idx = chunk[3] as usize;
        let token_modifier_bits = chunk[4];

        if delta_line > 0 {
            line += delta_line;
            start_char = delta_start_char;
        } else {
            start_char += delta_start_char;
        }

        let highlight_name = legend
            .token_types
            .get(token_type_idx)
            .map(|t| token_type_to_highlight_name(t.as_str()))
            .unwrap_or("variable");

        let modifiers = modifiers_to_token_modifiers(
            &legend
                .token_modifiers
                .iter()
                .map(|m| m.as_str())
                .collect::<Vec<_>>(),
            token_modifier_bits,
        );

        let start_pos = Utf16Position::new(
            Line::new(line as usize),
            Utf16Offset::new(start_char as usize),
        );
        let end_pos = Utf16Position::new(
            Line::new(line as usize),
            Utf16Offset::new((start_char + length) as usize),
        );

        let Ok(start_byte) = snapshot.utf16_position_to_byte(start_pos) else {
            continue;
        };
        let Ok(end_byte) = snapshot.utf16_position_to_byte(end_pos) else {
            continue;
        };
        let Ok(range) = TextRange::new(start_byte, end_byte) else {
            continue;
        };

        spans.push((
            range,
            HighlightSpan::new(HighlightName::new(highlight_name), modifiers),
        ));
    }

    spans
}

// =============================================================================
// 测试
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ----------------------------------------------------------------
    // token_type_to_highlight_name
    // ----------------------------------------------------------------

    #[test]
    fn parameter_maps_to_variable_parameter() {
        assert_eq!(
            token_type_to_highlight_name("parameter"),
            "variable.parameter"
        );
    }

    #[test]
    fn variable_stays_flat() {
        assert_eq!(token_type_to_highlight_name("variable"), "variable");
    }

    #[test]
    fn method_maps_to_function_dot_method() {
        assert_eq!(token_type_to_highlight_name("method"), "function.method");
    }

    #[test]
    fn function_stays_flat() {
        assert_eq!(token_type_to_highlight_name("function"), "function");
    }

    #[test]
    fn macro_maps_to_function_dot_macro() {
        assert_eq!(token_type_to_highlight_name("macro"), "function.macro");
    }

    #[test]
    fn enum_member_maps_to_variable_other_member() {
        assert_eq!(
            token_type_to_highlight_name("enumMember"),
            "variable.other.member"
        );
    }

    #[test]
    fn unknown_token_falls_to_variable() {
        assert_eq!(token_type_to_highlight_name("__made_up__"), "variable");
    }

    #[test]
    fn type_parameter_maps_to_type() {
        assert_eq!(token_type_to_highlight_name("typeParameter"), "type");
    }

    #[test]
    fn all_known_lsp_types() {
        let cases = &[
            ("comment", "comment"),
            ("keyword", "keyword"),
            ("modifier", "keyword"),
            ("string", "string"),
            ("regexp", "string"),
            ("number", "number"),
            ("function", "function"),
            ("method", "function.method"),
            ("variable", "variable"),
            ("parameter", "variable.parameter"),
            ("type", "type"),
            ("class", "type"),
            ("enum", "type"),
            ("interface", "type"),
            ("struct", "type"),
            ("typeParameter", "type"),
            ("property", "property"),
            ("enumMember", "variable.other.member"),
            ("macro", "function.macro"),
            ("operator", "operator"),
            ("namespace", "namespace"),
            ("decorator", "attribute"),
        ];
        for (lsp_type, expected) in cases {
            let got = token_type_to_highlight_name(lsp_type);
            assert!(!got.is_empty(), "{lsp_type} → 空串");
            assert_eq!(got, *expected, "{lsp_type}: expected {expected}, got {got}");
        }
    }

    #[test]
    fn every_rule_token_type_is_unique() {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for rule in LSP_RULES {
            for tt in rule.token_types {
                assert!(seen.insert(*tt), "重复的 LSP token type: {tt}");
            }
        }
    }

    // ----------------------------------------------------------------
    // modifiers_to_token_modifiers
    // ----------------------------------------------------------------

    #[test]
    fn empty_modifiers_on_zero_bits() {
        let mods = &["static", "readonly", "deprecated"];
        assert_eq!(modifiers_to_token_modifiers(mods, 0), TokenModifiers::EMPTY);
    }

    #[test]
    fn static_bit_set() {
        let mods = &["static", "readonly"];
        let result = modifiers_to_token_modifiers(mods, 1);
        assert!(result.contains(TokenModifiers::STATIC));
        assert!(!result.contains(TokenModifiers::READONLY));
    }

    #[test]
    fn unknown_modifier_name_is_ignored() {
        let mods = &["unknown_mod", "async"];
        let result = modifiers_to_token_modifiers(mods, 1);
        assert_eq!(result, TokenModifiers::EMPTY);
        let result2 = modifiers_to_token_modifiers(mods, 2);
        assert!(result2.contains(TokenModifiers::ASYNC));
    }

    // ----------------------------------------------------------------
    // decode_semantic_tokens
    // ----------------------------------------------------------------

    #[test]
    fn decode_empty_data() {
        let legend = SemanticTokensLegend {
            token_types: vec!["keyword".into()],
            token_modifiers: vec![],
        };
        let result = decode_semantic_tokens(&[], &legend, &dummy_snapshot());
        assert!(result.is_empty());
    }

    #[test]
    fn decode_single_token() {
        let legend = SemanticTokensLegend {
            token_types: vec!["keyword".into()],
            token_modifiers: vec![],
        };
        let data = &[0u32, 0, 3, 0, 0];
        let snapshot = dummy_snapshot();
        let result = decode_semantic_tokens(data, &legend, &snapshot);
        assert_eq!(result.len(), 1);
        let (range, span) = &result[0];
        assert_eq!(range.start().get(), 0);
        assert_eq!(range.end().get(), 3);
        assert_eq!(span.name.as_str(), "keyword");
    }

    fn dummy_snapshot() -> Snapshot {
        use zom_engine::{Buffer, BufferConfig};
        let buf = Buffer::from_text(
            "fn main() {\n    let x = 1;\n}\n".to_string(),
            BufferConfig::default(),
        )
        .unwrap();
        buf.snapshot()
    }
}
