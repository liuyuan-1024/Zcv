//! 语法高亮：tree-sitter highlight name → 字色 的查询入口（手册《桌面端语法高亮》§十一）。
//!
//! **本模块只提供机制，不提供数据**。
//!
//! 手册 §三 / §十一 把分工写得很清楚：
//! - 调用机制（完整 name → 点分前缀 → 默认前景色 逐级回退）归我们；
//! - **具体 name → 色值映射归"主题"**——属外部主题文件（Helix / Zed 兼容口径），不在 zom 仓内手写。
//!
//! ## 当前实现
//!
//! 启动期 [`include_str!`] 一份 vendor 的 Helix `onedark.toml`，
//! OnceLock parse 成 `HashMap<&'static str, Hsla>`。
//! `color_for` 走完点分前缀回退链后命中表即返；
//! 任何未命中的 name 落到 [`default_fg`]。
//!
//! 当前只取每条规则的 `fg` 字段；`modifiers` / `underline` / `bg` 等暂未消费 —— 下游 prepaint 拆 TextRun 时只用字色，不动字重 / 下划线。
//! 多主题切换、用户主题加载路径和 modifiers / underline 的视觉消费归设置与主题系统后续接入，不在本查询入口里提前铺兼容分支。
//!
//! ## 为什么不自己定义色值
//!
//! 上一轮决策：手册 §三 / §十一 把 token name 词汇表与具体配色都划归"主题生态"，我们只负责机制。
//! 自创 26 行 `lookup_exact` 表是自创主题，违反这条边界——已剥除，改为 vendor 上游主题文件。
//! Helix `onedark.toml` 是 MPL-2.0 文件级 copyleft，vendor 一份文件不影响其他代码许可。

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use gpui::{Hsla, Rgba, rgb, rgba};

use super::color;
use crate::theme::ConcreteTheme;

/// vendor 的默认主题源（启动期 include 进二进制）。
const THEME_ONE_DARK_TOML: &str = include_str!("../../assets/themes/onedark.toml");
const THEME_ONE_LIGHT_TOML: &str = include_str!("../../assets/themes/onelight.toml");

/// 默认前景色——任何 name 都无前缀命中时落到这里。
pub fn default_fg() -> Hsla {
    color::current().gray.s09.into()
}

/// 按 highlight name 解析出字色；遵守点分前缀回退链。
///
/// 例如 `keyword.control.import` 未命中完整 name 时退到 `keyword.control`，
/// 再退到 `keyword`，最后退到 [`default_fg`]。
pub fn color_for(name: &str) -> Hsla {
    // 完整 name → 前缀逐段砍。strip 到空字符串就到底了。
    let mut current = name;
    loop {
        if let Some(hsla) = lookup_in_theme(current) {
            return hsla;
        }
        match current.rfind('.') {
            Some(dot) => current = &current[..dot],
            None => return default_fg(),
        }
    }
}

pub(crate) fn set_theme(theme: ConcreteTheme) {
    let source = match theme {
        ConcreteTheme::Dark => THEME_ONE_DARK_TOML,
        ConcreteTheme::Light => THEME_ONE_LIGHT_TOML,
    };
    let table = parse_helix_theme(source).unwrap_or_default();
    let lock = ACTIVE_THEME.get_or_init(|| RwLock::new(default_theme_table()));
    match lock.write() {
        Ok(mut active) => *active = table,
        Err(error) => eprintln!("更新语法主题失败：{error}"),
    }
}

/// 单条 name 的精确查询（不做前缀回退）。
fn lookup_in_theme(name: &str) -> Option<Hsla> {
    let lock = ACTIVE_THEME.get_or_init(|| RwLock::new(default_theme_table()));
    lock.read().ok().and_then(|theme| theme.get(name).copied())
}

/// 全进程共享的解析后主题表。
/// OnceLock 兜底，主题文件解析失败则空表，所有 name 落 default_fg——
/// 不会因为坏主题文件让 UI 整个不渲染。
static ACTIVE_THEME: OnceLock<RwLock<HashMap<&'static str, Hsla>>> = OnceLock::new();

fn default_theme_table() -> HashMap<&'static str, Hsla> {
    parse_helix_theme(THEME_ONE_DARK_TOML).unwrap_or_default()
}

/// 把一份 Helix 风格的 theme.toml 解析成 name → Hsla 表。
///
/// 支持的语法：
/// - `"foo" = "color"`            — fg 取 palette[color] 或 hex
/// - `"foo" = { fg = "color" }`   — 取 fg 字段
/// - 不带 `fg` 的条目跳过（如 `"ui.background" = { bg = "..." }`）
/// - `[palette]` 表下 `name = "#hex"`
///
/// 当前静默忽略（不算错）：
/// - `inherits` 字段（链式继承）
/// - `modifiers` / `underline` / `bg`（只取 fg）
/// - palette 名引用 palette 名（onedark 不用，不处理）
fn parse_helix_theme(src: &str) -> Option<HashMap<&'static str, Hsla>> {
    let root: toml::Table = toml::from_str(src).ok()?;

    // palette 先解：name → Rgba。
    // 后面查规则的 fg 字符串时，先按 palette 名查，命中再去 hex parse 兜底。
    let palette: HashMap<String, Rgba> = root
        .get("palette")
        .and_then(|v| v.as_table())
        .map(|tbl| {
            tbl.iter()
                .filter_map(|(k, v)| {
                    let hex = v.as_str()?;
                    let color = parse_hex(hex)?;
                    Some((k.clone(), color))
                })
                .collect()
        })
        .unwrap_or_default();

    let resolve = |color: &str| -> Option<Rgba> {
        if color.starts_with('#') {
            parse_hex(color)
        } else {
            palette.get(color).copied()
        }
    };

    let mut out: HashMap<&'static str, Hsla> = HashMap::new();
    for (key, value) in &root {
        if key == "palette" {
            continue;
        }
        // 只看 fg 来源——string 形式整体当 fg，table 形式取 fg 字段。
        let color_token: Option<&str> = match value {
            toml::Value::String(s) => Some(s.as_str()),
            toml::Value::Table(t) => t.get("fg").and_then(|v| v.as_str()),
            _ => None,
        };
        let Some(token) = color_token else { continue };
        let Some(rgba) = resolve(token) else { continue };
        // Box::leak 把 key 提升到 'static——主题表是进程级常量，泄漏一次即终生。
        let static_key: &'static str = Box::leak(key.clone().into_boxed_str());
        out.insert(static_key, rgba.into());
    }
    Some(out)
}

/// 解析 `#RRGGBB` 或 `#RRGGBBAA` 形式的 hex 字符串。
fn parse_hex(s: &str) -> Option<Rgba> {
    let body = s.strip_prefix('#')?;
    let value = u32::from_str_radix(body, 16).ok()?;
    Some(match body.len() {
        6 => rgb(value),
        8 => rgba(value),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_chain_terminates_at_default_fg() {
        // 整段无前缀关联：直接 default_fg（onedark 不覆盖 nope / totally.unknown）。
        assert_eq!(color_for("totally.unknown"), default_fg());
        assert_eq!(color_for("nope"), default_fg());
    }

    #[test]
    fn onedark_provides_color_for_common_rust_names() {
        // tree-sitter-rust 实际会产出的常见 name 都应被 onedark 命中或经回退链命中一个父前缀。
        // 这条断言挂着，未来换主题如果不覆盖这些基本名能立刻爆。
        for name in &["keyword", "string", "comment", "function", "type"] {
            let c = color_for(name);
            assert_ne!(
                c,
                default_fg(),
                "expected onedark to color `{name}` but it fell to default_fg"
            );
        }
    }

    #[test]
    fn dot_prefix_fallback_uses_parent_rule() {
        // tree-sitter-rust 会产出 @function.method；onedark 只列了 function。
        // function.method 应回退到 function 的色，与单独查 function 一致。
        assert_eq!(color_for("function.method"), color_for("function"));
        // 同理 type.builtin → type；comment.documentation → comment。
        assert_eq!(color_for("type.builtin"), color_for("type"));
        assert_eq!(color_for("comment.documentation"), color_for("comment"));
    }

    #[test]
    fn parser_resolves_palette_references() {
        let src = r##"
            "keyword" = { fg = "red" }
            "string" = "green"
            "comment" = "#abcdef"

            [palette]
            red = "#ff0000"
            green = "#00ff00"
        "##;
        let table = parse_helix_theme(src).expect("应能解析主题");
        let keyword = table.get("keyword").expect("keyword 应存在");
        let string = table.get("string").expect("string 应存在");
        let comment = table.get("comment").expect("comment 应存在");

        assert_eq!(*keyword, Hsla::from(rgb(0xff0000)));
        assert_eq!(*string, Hsla::from(rgb(0x00ff00)));
        assert_eq!(*comment, Hsla::from(rgb(0xabcdef)));
    }

    #[test]
    fn parser_skips_entries_without_fg() {
        // ui.background 只有 bg；diagnostic.unnecessary 只有 modifiers——都跳过。
        let src = r##"
            "ui.background" = { bg = "black" }
            "diagnostic.unnecessary" = { modifiers = ["dim"] }
            "keyword" = { fg = "red" }

            [palette]
            red = "#ff0000"
            black = "#000000"
        "##;
        let table = parse_helix_theme(src).expect("应能解析主题");
        assert!(!table.contains_key("ui.background"));
        assert!(!table.contains_key("diagnostic.unnecessary"));
        assert!(table.contains_key("keyword"));
    }

    #[test]
    fn parser_skips_unresolved_color_tokens() {
        // palette 没列的色名静默丢——不会把 None 当成异常打断整个 parse。
        let src = r##"
            "keyword" = "missing-color"
            "string" = "red"

            [palette]
            red = "#ff0000"
        "##;
        let table = parse_helix_theme(src).expect("应能解析主题");
        assert!(!table.contains_key("keyword"));
        assert!(table.contains_key("string"));
    }

    #[test]
    fn parser_handles_invalid_toml() {
        // 坏文件返 None；调用方（theme_table）落空表，所有 name 自然 default_fg。
        assert!(parse_helix_theme("this is not [valid toml").is_none());
    }

    // ----------------------------------------------------------------
    // LSP semantic token → highlight name → color 端到端
    // ----------------------------------------------------------------

    #[test]
    fn lsp_parameter_resolves_to_color_via_variable_parameter() {
        // onedark 有 `"variable.parameter" = { fg = "red" }`，所以
        // LSP "parameter" → "variable.parameter" 应命中实际颜色而非 default_fg。
        let c = color_for("variable.parameter");
        assert_ne!(
            c,
            default_fg(),
            "onedark 必须给 variable.parameter 上色（LSP parameter 产此名）"
        );
    }

    #[test]
    fn lsp_method_falls_back_to_function() {
        // LSP "method" → "function.method"；onedark 有 "function" 但无 "function.method"。
        // 点分回退应让 function.method 取 function 的颜色。
        assert_eq!(color_for("function.method"), color_for("function"));
    }

    #[test]
    fn lsp_enum_member_resolves_via_variable_other_member() {
        // onedark 有 `"variable.other.member" = { fg = "red" }`；
        // LSP enumMember → "variable.other.member" 应命中。
        let c = color_for("variable.other.member");
        assert_ne!(
            c,
            default_fg(),
            "onedark 必须给 variable.other.member 上色（LSP enumMember 产此名）"
        );
    }

    #[test]
    fn lsp_macro_resolves_via_function_dot_macro() {
        // onedark 有 `"function.macro" = { fg = "purple" }`；
        // LSP macro → "function.macro" 应命中。
        let c = color_for("function.macro");
        assert_ne!(
            c,
            default_fg(),
            "onedark 必须给 function.macro 上色（LSP macro 产此名）"
        );
    }
}
