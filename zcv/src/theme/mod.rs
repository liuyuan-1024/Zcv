//! 视觉 token：色彩、间距、字号、圆角。
//!
//! [`Theme`] 是主题的唯一入口，配置字符串 ↔ 枚举 ↔ 调色板/语法高亮 在此映射。

use std::sync::atomic::{AtomicU16, Ordering};

use gpui::{Font, FontFallbacks, Pixels, Window, WindowAppearance, font, px};

pub mod color;
pub mod syntax;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Theme {
    System,
    OneDark,
    OneLight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConcreteTheme {
    Dark,
    Light,
}

impl Theme {
    pub(crate) fn from_config(s: &str) -> Self {
        match s {
            "one-dark" => Self::OneDark,
            "one-light" => Self::OneLight,
            _ => Self::System,
        }
    }

    pub(crate) fn as_config(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::OneDark => "one-dark",
            Self::OneLight => "one-light",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::System => "跟随系统",
            Self::OneDark => "One Dark",
            Self::OneLight => "One Light",
        }
    }

    pub(crate) fn next(self) -> Self {
        match self {
            Self::System => Self::OneDark,
            Self::OneDark => Self::OneLight,
            Self::OneLight => Self::System,
        }
    }

    pub(crate) fn is_system(self) -> bool {
        self == Self::System
    }

    pub(crate) fn effective(self, window: Option<&Window>) -> ConcreteTheme {
        match self {
            Self::System => match window.map(|w| w.appearance()) {
                Some(WindowAppearance::Dark | WindowAppearance::VibrantDark) => ConcreteTheme::Dark,
                Some(WindowAppearance::Light | WindowAppearance::VibrantLight) => {
                    ConcreteTheme::Light
                }
                None => ConcreteTheme::Dark,
            },
            Self::OneDark => ConcreteTheme::Dark,
            Self::OneLight => ConcreteTheme::Light,
        }
    }

    pub(crate) fn apply(self, window: Option<&Window>) {
        let concrete = self.effective(window);
        color::set_palette(concrete);
        syntax::set_theme(concrete);
    }
}

// ── 间距 ───────────────────────────────────────────────────────────

pub mod space {
    use gpui::{Pixels, px};
    pub const S2: Pixels = px(2.0);
    pub const S6: Pixels = px(6.0);
    pub const S8: Pixels = px(8.0);
    pub const S16: Pixels = px(16.0);
    pub const GUTTER_BAR: Pixels = px(4.0);
}

// ── 圆角 ───────────────────────────────────────────────────────────

pub mod radius {
    use gpui::{Pixels, px};
    pub const R2: Pixels = px(2.0);
    pub const R4: Pixels = px(4.0);
    pub const FULL: Pixels = px(999.0);
}

// ── 字号 ───────────────────────────────────────────────────────────

pub mod typography {
    use super::*;

    static UI_FONT_SIZE: AtomicU16 = AtomicU16::new(13);
    static EDITOR_FONT_SIZE: AtomicU16 = AtomicU16::new(16);

    pub(crate) fn set_sizes(ui: u16, editor: u16) {
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
        let mut font = font("JetBrains Mono");
        font.fallbacks = Some(cjk_fallback());
        font
    }

    pub fn editor_font() -> Font {
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
