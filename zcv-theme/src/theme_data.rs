//! 主题数据：单一解析器与主题注册表。
//!
//! 对齐 Zed 的 `ThemeRegistry`：一个主题文件一次解析出调色板与语法高亮表，palette / syntax / color 各模块只消费解析结果，不再各自解析 TOML。
//!
//! 新增主题：把主题 TOML 放入 `assets/themes/`，并在下方 `THEMES` 注册表加一行（`id`/`label`/`appearance` 由描述符声明），其余代码零改动。

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use gpui::{
    FontStyle, FontWeight, HighlightStyle, Hsla, Rgba, StrikethroughStyle, UnderlineStyle,
    WindowAppearance, px, rgb, rgba,
};

use crate::palette::{HuePalette, Palette};

/// 单个已解析主题：元数据 + 调色板 + 语法高亮表。
pub(crate) struct ThemeData {
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    /// 主题明暗（`System` 选择时按窗口外观匹配）。
    pub(crate) appearance: WindowAppearance,
    pub(crate) palette: Palette,
    pub(crate) syntax_table: Arc<BTreeMap<&'static str, HighlightStyle>>,
}

const ONE_DARK_TOML: &str = include_str!("../assets/themes/onedark.toml");
const ONE_LIGHT_TOML: &str = include_str!("../assets/themes/onelight.toml");

/// 主题注册表（编译期内嵌）：新增主题在此登记一行。
static THEMES: OnceLock<Vec<ThemeData>> = OnceLock::new();

pub(crate) fn themes() -> &'static [ThemeData] {
    THEMES.get_or_init(|| {
        vec![
            build_theme(
                "one-dark",
                "One Dark",
                WindowAppearance::Dark,
                ONE_DARK_TOML,
            ),
            build_theme(
                "one-light",
                "One Light",
                WindowAppearance::Light,
                ONE_LIGHT_TOML,
            ),
        ]
    })
}

/// 按 id 查询主题；未知 id 返回 `None`。
pub(crate) fn theme_by_id(id: &str) -> Option<&'static ThemeData> {
    themes().iter().find(|theme| theme.id == id)
}

fn build_theme(
    id: &'static str,
    label: &'static str,
    appearance: WindowAppearance,
    source: &'static str,
) -> ThemeData {
    // 内嵌数据错误属于构建期缺陷：直接 panic 暴露，而不是运行时降级掩盖。
    parse_theme(id, label, appearance, source).expect("内嵌主题文件应可解析")
}

/// 单一解析器：一次 TOML 解析产出主题的全部数据。
fn parse_theme(
    id: &'static str,
    label: &'static str,
    appearance: WindowAppearance,
    source: &str,
) -> Option<ThemeData> {
    let root: toml::Table = toml::from_str(source).ok()?;
    Some(ThemeData {
        id,
        label,
        appearance,
        palette: parse_palette(&root)?,
        syntax_table: Arc::new(parse_syntax_table(&root)?),
    })
}

/// 解析 `[ui]` 段：每个色相的 9 级 solid 色值 + 由强调色派生的 alpha 阶梯。
fn parse_palette(root: &toml::Table) -> Option<Palette> {
    let ui = root.get("ui")?.as_table()?;
    let alpha_steps = ui.get("alpha-steps")?.as_array()?;
    let alpha_steps: [u8; 9] = alpha_steps
        .iter()
        .map(|value| {
            let hex = value.as_str()?;
            let body = hex.strip_prefix("0x").unwrap_or(hex);
            u8::from_str_radix(body, 16).ok()
        })
        .collect::<Option<Vec<_>>>()?
        .try_into()
        .ok()?;

    let hue = |name: &str| -> Option<HuePalette> {
        let table = ui.get(name)?.as_table()?;
        let s = table.get("s")?.as_array()?;
        // 先解析为 0xRRGGBB 整数：alpha 派生在整数域组合，避免 f32 回转换丢精度。
        let s: [u32; 9] = s
            .iter()
            .map(|value| parse_hex_u32(value.as_str()?))
            .collect::<Option<Vec<_>>>()?
            .try_into()
            .ok()?;
        let a = alpha_steps.map(|step| rgba((s[6] << 8) | step as u32));
        let s = s.map(gpui::rgb);
        Some(HuePalette { s, a })
    };

    Some(Palette {
        gray: hue("gray")?,
        blue: hue("blue")?,
        green: hue("green")?,
        yellow: hue("yellow")?,
        red: hue("red")?,
    })
}

/// 解析 `[palette]` 颜色名引用与各高亮规则段（Helix 风格主题）。
///
/// 支持 `"name" = "color"` 和 `"name" = { fg = "color", modifiers = [...] }` 两种格式；
/// 不含 `fg` 的条目跳过，颜色名引用解析自 `[palette]` 段。
fn parse_syntax_table(root: &toml::Table) -> Option<BTreeMap<&'static str, HighlightStyle>> {
    let palette: BTreeMap<String, Rgba> = root
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

    let mut out: BTreeMap<&'static str, HighlightStyle> = BTreeMap::new();
    for (key, value) in root {
        if key == "palette" {
            continue;
        }
        let (color_token, modifiers): (Option<&str>, &[toml::Value]) = match value {
            toml::Value::String(s) => (Some(s.as_str()), &[]),
            toml::Value::Table(t) => (
                t.get("fg").and_then(|v| v.as_str()),
                t.get("modifiers")
                    .and_then(|v| v.as_array())
                    .map_or(&[], Vec::as_slice),
            ),
            _ => (None, &[]),
        };
        let color = color_token.and_then(resolve).map(Hsla::from);
        if color_token.is_some() && color.is_none() {
            continue;
        }
        let has_modifier = |name: &str| modifiers.iter().any(|value| value.as_str() == Some(name));
        let style = HighlightStyle {
            color,
            font_weight: has_modifier("bold").then_some(FontWeight::BOLD),
            font_style: has_modifier("italic").then_some(FontStyle::Italic),
            underline: has_modifier("underlined").then_some(UnderlineStyle {
                thickness: px(1.),
                color,
                wavy: false,
            }),
            strikethrough: has_modifier("crossed_out").then_some(StrikethroughStyle {
                thickness: px(1.),
                color,
            }),
            ..HighlightStyle::default()
        };
        if style == HighlightStyle::default() {
            continue;
        }
        let static_key: &'static str = Box::leak(key.clone().into_boxed_str());
        out.insert(static_key, style);
    }
    Some(out)
}

/// 解析 `#RRGGBB`（或 `#RRGGBBAA`）为 0xRRGGBB（8 位时丢弃 alpha）的整数。
fn parse_hex_u32(s: &str) -> Option<u32> {
    let body = s.strip_prefix('#')?;
    let value = u32::from_str_radix(body, 16).ok()?;
    Some(value >> if body.len() == 8 { 8 } else { 0 })
}

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

    fn rgba(hex: u32) -> Rgba {
        gpui::rgba(hex)
    }

    #[test]
    fn dark_theme_matches_historical_palette_values() {
        let theme = theme_by_id("one-dark").expect("内置 onedark 主题应存在");
        let dark = theme.palette;
        // solid 阶梯与历史硬编码逐字节一致（抽查每色相边界与强调色）。
        assert_eq!(dark.gray.s[0], rgba(0x0d0f12ff));
        assert_eq!(dark.gray.s[8], rgba(0xa8b0c0ff));
        assert_eq!(dark.blue.s[0], rgba(0x0e1a2eff));
        assert_eq!(dark.blue.s[6], rgba(0x74ade8ff));
        assert_eq!(dark.green.s[6], rgba(0x3ddc84ff));
        assert_eq!(dark.yellow.s[6], rgba(0xe8cf74ff));
        assert_eq!(dark.red.s[6], rgba(0xff6b6bff));
        // 被消费的 alpha 值（选区背景）与历史逐字节一致：blue.a[2] = s[6] + 0x3d。
        assert_eq!(dark.blue.a[2], rgba(0x74ade83d));
    }

    #[test]
    fn light_theme_matches_historical_palette_values() {
        let theme = theme_by_id("one-light").expect("内置 onelight 主题应存在");
        let light = theme.palette;
        assert_eq!(light.gray.s[0], rgba(0xfafafaff));
        assert_eq!(light.gray.s[8], rgba(0x1e1e1eff));
        assert_eq!(light.blue.s[6], rgba(0x2563ebff));
        assert_eq!(light.green.s[6], rgba(0x16a34aff));
        assert_eq!(light.yellow.s[6], rgba(0xca8a04ff));
        assert_eq!(light.red.s[6], rgba(0xdc2626ff));
        // light 主题的选区背景同样与历史一致：blue.a[2] = s[6] + 0x3d。
        assert_eq!(light.blue.a[2], rgba(0x2563eb3d));
    }

    #[test]
    fn theme_registry_has_expected_entries() {
        let themes = themes();
        assert_eq!(themes.len(), 2);
        assert_eq!(themes[0].id, "one-dark");
        assert_eq!(themes[1].id, "one-light");
        assert!(theme_by_id("unknown").is_none());
    }

    #[test]
    fn syntax_table_resolves_palette_references() {
        let src = r##"
            "keyword" = { fg = "red" }
            "string" = "green"
            "comment" = "#abcdef"
            [palette]
            red = "#ff0000"
            green = "#00ff00"
        "##;
        let table = parse_syntax_table(&toml::from_str(src).expect("应能解析")).expect("应能解析");
        assert_eq!(
            table.get("keyword").unwrap().color,
            Some(Hsla::from(rgb(0xff0000)))
        );
        assert_eq!(
            table.get("string").unwrap().color,
            Some(Hsla::from(rgb(0x00ff00)))
        );
        assert_eq!(
            table.get("comment").unwrap().color,
            Some(Hsla::from(rgb(0xabcdef)))
        );
    }

    #[test]
    fn syntax_table_skips_entries_without_fg() {
        let src = r##"
            "ui.background" = { bg = "black" }
            "diagnostic.unnecessary" = { modifiers = ["dim"] }
            "keyword" = { fg = "red" }
            [palette]
            red = "#ff0000"
            black = "#000000"
        "##;
        let table = parse_syntax_table(&toml::from_str(src).expect("应能解析")).expect("应能解析");
        assert!(!table.contains_key("ui.background"));
        assert!(!table.contains_key("diagnostic.unnecessary"));
        assert!(table.contains_key("keyword"));
    }

    #[test]
    fn syntax_table_skips_unresolved_color_tokens() {
        let src = r##"
            "keyword" = "missing-color"
            "string" = "red"
            [palette]
            red = "#ff0000"
        "##;
        let table = parse_syntax_table(&toml::from_str(src).expect("应能解析")).expect("应能解析");
        assert!(!table.contains_key("keyword"));
        assert!(table.contains_key("string"));
    }
}
