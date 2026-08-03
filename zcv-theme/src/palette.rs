//! 调色板：两轴正交（色相 × 语义档），切换主题时原子替换。
//!
//! | 索引 | 语义 |
//! |---|---|
//! | 0 | app background |
//! | 1 | surface |
//! | 2 | ui-bg |
//! | 3 | ui-active |
//! | 4 | border-subtle |
//! | 5 | border-focus |
//! | 6 | solid 强调 |
//! | 7 | text-muted |
//! | 8 | text |
//!
//! **色值数据源**：`assets/themes/*.toml` 的 `[ui]` 段（与语法色同一主题文件，主题自包含）。
//! 代码内不持有任何色值；alpha 阶梯由各色相强调色 `s[6]`叠加统一透明度序列派生，不再逐值书写。
//!
//! 本模块只做「TOML → 调色板」的纯解析，不持有运行期状态；
//! 语义色快照的缓存与切换由 [`crate::color`] 承担。

use gpui::{Rgba, rgba};

use crate::Theme;

/// 单个色相的 solid + alpha 双阶梯。索引 0–8 对应语义 01–09。
#[derive(Clone, Copy, Debug)]
pub(crate) struct HuePalette {
    pub(crate) s: [Rgba; 9],
    pub(crate) a: [Rgba; 9],
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Palette {
    pub(crate) gray: HuePalette,
    pub(crate) blue: HuePalette,
    pub(crate) green: HuePalette,
    pub(crate) yellow: HuePalette,
    pub(crate) red: HuePalette,
}

const ONE_DARK_TOML: &str = include_str!("../assets/themes/onedark.toml");
const ONE_LIGHT_TOML: &str = include_str!("../assets/themes/onelight.toml");

impl Palette {
    /// 解析指定主题的调色板；解析失败返回 `None`（内嵌数据错误，调用方决定降级策略）。
    pub(crate) fn for_theme(theme: Theme) -> Option<Self> {
        let source = match theme {
            Theme::OneDark => ONE_DARK_TOML,
            Theme::OneLight => ONE_LIGHT_TOML,
            // System 未解析时按深色默认。
            Theme::System => ONE_DARK_TOML,
        };
        Self::from_toml(source)
    }

    /// 从主题文件的 `[ui]` 段解析调色板。
    ///
    /// 每个色相的 `s` 数组是 9 级 solid 色值；
    /// alpha 阶梯由该色相强调色 `s[6]` 叠加 `alpha -steps` 透明度序列派生。
    fn from_toml(source: &str) -> Option<Self> {
        let root: toml::Table = toml::from_str(source).ok()?;
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

        Some(Self {
            gray: hue("gray")?,
            blue: hue("blue")?,
            green: hue("green")?,
            yellow: hue("yellow")?,
            red: hue("red")?,
        })
    }
}

/// 解析 `#RRGGBB`（或 `#RRGGBBAA`）为 0xRRGGBB（8 位时丢弃 alpha）的整数。
fn parse_hex_u32(s: &str) -> Option<u32> {
    let body = s.strip_prefix('#')?;
    let value = u32::from_str_radix(body, 16).ok()?;
    Some(value >> if body.len() == 8 { 8 } else { 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(hex: u32) -> Rgba {
        gpui::rgba(hex)
    }

    #[test]
    fn dark_palette_matches_historical_values() {
        let dark = Palette::from_toml(ONE_DARK_TOML).expect("onedark [ui] 段应可解析");
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
    fn light_palette_matches_historical_values() {
        let light = Palette::from_toml(ONE_LIGHT_TOML).expect("onelight [ui] 段应可解析");
        assert_eq!(light.gray.s[0], rgba(0xfafafaff));
        assert_eq!(light.gray.s[8], rgba(0x1e1e1eff));
        assert_eq!(light.blue.s[6], rgba(0x2563ebff));
        assert_eq!(light.green.s[6], rgba(0x16a34aff));
        assert_eq!(light.yellow.s[6], rgba(0xca8a04ff));
        assert_eq!(light.red.s[6], rgba(0xdc2626ff));
        // light 主题的选区背景同样与历史一致：blue.a[2] = s[6] + 0x3d。
        assert_eq!(light.blue.a[2], rgba(0x2563eb3d));
    }
}
