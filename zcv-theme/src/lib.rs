//! 视觉 token：色彩、间距、字号、圆角。
//!
//! [`ThemeChoice`] 是主题配置入口：`System` 或注册表中的主题 id。
//! 主题数据（调色板 + 语法高亮）由 [`theme_data`] 注册表统一持有，新增主题只需添加 TOML 文件并在注册表登记，无需改动本模块逻辑。

use std::sync::atomic::{AtomicU16, Ordering};

use gpui::{App, Font, FontFallbacks, Pixels, Window, font, px};

pub mod color;
mod palette;
pub mod syntax;
mod theme_data;

use theme_data::{ThemeData, theme_by_id, themes};

/// 主题配置：跟随系统外观，或显式指定注册表中的主题。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeChoice {
    System,
    /// 注册表中的主题 id（如 `"one-dark"`）。
    Named(&'static str),
}

impl ThemeChoice {
    /// 从设置字符串解析；未知 id 回退到 `System`。
    pub fn from_config(s: &str) -> Self {
        match s {
            "system" => Self::System,
            _ => theme_by_id(s).map_or(Self::System, |theme| Self::Named(theme.id)),
        }
    }

    /// 写回设置文件的字符串。
    pub fn as_config(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Named(id) => id,
        }
    }

    pub fn label(self) -> String {
        match self {
            Self::System => "跟随系统".to_string(),
            Self::Named(id) => {
                theme_by_id(id).map_or_else(|| id.to_string(), |theme| theme.label.to_string())
            }
        }
    }

    /// 解析为具体主题：`System` 按窗口外观选择匹配的主题，无窗口时默认深色。
    pub(crate) fn effective(self, window: Option<&Window>) -> &'static ThemeData {
        match self {
            Self::Named(id) => theme_by_id(id).unwrap_or_else(|| first_theme()),
            Self::System => window
                .map(|w| w.appearance())
                .and_then(|appearance| themes().iter().find(|theme| theme.appearance == appearance))
                .unwrap_or_else(first_theme),
        }
    }

    pub fn apply(self, cx: &mut App, window: Option<&Window>) {
        let theme = self.effective(window);
        color::set_theme(theme, cx);
        syntax::set_theme(theme);
    }
}

/// 无窗口且注册表为空时的兜底（内置深色主题）。
fn first_theme() -> &'static ThemeData {
    themes()
        .first()
        .expect("主题注册表不应为空（至少包含内置主题）")
}

// ── 间距 ───────────────────────────────────────────────────────────

pub mod space {
    use gpui::{Pixels, px};
    pub const S2: Pixels = px(2.0);
    pub const S6: Pixels = px(6.0);
    pub const S8: Pixels = px(8.0);
    pub const S10: Pixels = px(10.0);
    pub const S12: Pixels = px(12.0);
    pub const S16: Pixels = px(16.0);
}

// ── 字号 ───────────────────────────────────────────────────────────

pub mod typography {
    use super::*;

    static UI_FONT_SIZE: AtomicU16 = AtomicU16::new(13);
    static EDITOR_FONT_SIZE: AtomicU16 = AtomicU16::new(16);

    pub fn set_sizes(ui: u16, editor: u16) {
        UI_FONT_SIZE.store(ui, Ordering::Relaxed);
        EDITOR_FONT_SIZE.store(editor, Ordering::Relaxed);
    }

    fn ui_size() -> f32 {
        UI_FONT_SIZE.load(Ordering::Relaxed) as f32
    }
    fn editor_size() -> f32 {
        EDITOR_FONT_SIZE.load(Ordering::Relaxed) as f32
    }

    pub fn ui_font() -> Font {
        mono_font()
    }

    pub fn editor_font() -> Font {
        mono_font()
    }

    /// 编辑器与 UI 共用同一等宽字体（含 CJK 回退）。
    fn mono_font() -> Font {
        let mut font = font("JetBrains Mono");
        font.fallbacks = Some(cjk_fallback());
        font
    }

    fn cjk_fallback() -> FontFallbacks {
        static FALLBACKS: std::sync::OnceLock<FontFallbacks> = std::sync::OnceLock::new();
        FALLBACKS
            .get_or_init(|| FontFallbacks::from_fonts(vec!["Sarasa Mono SC".to_string()]))
            .clone()
    }

    pub fn ui() -> Pixels {
        px(ui_size())
    }
    /// UI 行高（黄金比例），用于搜索框等单行输入场景
    pub fn ui_line() -> Pixels {
        px((ui_size() * 1.618_034).round())
    }
    pub fn editor() -> Pixels {
        px(editor_size())
    }
    pub fn editor_font_size() -> f32 {
        editor_size()
    }
    /// 编辑器行高（黄金比例），与 Zed 一致：`round(font_size * 1.618034)`
    pub fn editor_line() -> Pixels {
        px((editor_size() * 1.618_034).round())
    }
}
