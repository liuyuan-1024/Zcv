//! L1 视觉 token —— 命名常量。
//! 尺寸、字号、圆角、icon 不参与主题切换（手册 5.1 / 6.x）。
//!
//! [`Theme`] 是颜色主题的唯一真相源：配置字符串 ↔ 枚举 ↔ 调色板/语法高亮 的映射集中在这里。
//! 新增主题只需改这个文件。

use std::sync::OnceLock;

use gpui::{Font, FontFallbacks, Pixels, Window, WindowAppearance, font, px};

pub mod color;
pub mod syntax;

/// 用户可选的主题（含"跟随系统"）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Theme {
    System,
    OneDark,
    OneLight,
}

/// 落地后的具体主题（dark / light 之一）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConcreteTheme {
    Dark,
    Light,
}

impl Theme {
    /// 配置字符串 → 枚举。非法值兜底 System。
    pub(crate) fn from_config(s: &str) -> Self {
        match s {
            "one-dark" => Self::OneDark,
            "one-light" => Self::OneLight,
            _ => Self::System,
        }
    }

    /// 枚举 → 配置字符串。
    pub(crate) fn as_config(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::OneDark => "one-dark",
            Self::OneLight => "one-light",
        }
    }

    /// 设置面板展示用标签。
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::System => "跟随系统",
            Self::OneDark => "One Dark",
            Self::OneLight => "One Light",
        }
    }

    /// 轮转到下一个主题。
    pub(crate) fn next(self) -> Self {
        match self {
            Self::System => Self::OneDark,
            Self::OneDark => Self::OneLight,
            Self::OneLight => Self::System,
        }
    }

    /// 是否跟随系统外观。
    pub(crate) fn is_system(self) -> bool {
        self == Self::System
    }

    /// 解析为落地主题：system 读取窗口外观，固定主题直接映射。
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

    /// 一次性应用主题：更新调色板 + 语法高亮表。
    pub(crate) fn apply(self, window: Option<&Window>) {
        let concrete = self.effective(window);
        color::set_palette(concrete);
        syntax::set_theme(concrete);
    }
}

/// 距离类节拍尺（手册 6.1 / 6.2）。
pub mod space {
    use super::*;
    pub fn s2() -> Pixels {
        px(2.0)
    }
    pub fn s4() -> Pixels {
        px(4.0)
    }
    pub fn s6() -> Pixels {
        px(6.0)
    }
    pub fn s8() -> Pixels {
        px(8.0)
    }
    pub fn s12() -> Pixels {
        px(12.0)
    }
    pub fn s16() -> Pixels {
        px(16.0)
    }
    /// gutter 左缘 git diff 色条宽度。
    pub fn gutter_bar() -> Pixels {
        px(4.0)
    }
}

pub mod radius {
    use super::*;
    pub fn r2() -> Pixels {
        px(2.0)
    }
    pub fn r4() -> Pixels {
        px(4.0)
    }
    /// 圆点（control pip 等）专用，组件本地常量复用此 token。
    pub fn full() -> Pixels {
        px(999.0)
    }
}

/// 字号 + 行高（手册 6.3 / 6.4）。当前固定默认值；将来从 `cx.fonts()` 取。
///
/// 只有两套字号：`ui()` 给全部 UI chrome，`editor()` 给编辑区代码正文。
/// UI 层级靠颜色 / 字重区分，不靠字号。
pub mod typography {
    use super::*;
    use std::sync::RwLock;

    #[derive(Clone, Copy, Debug)]
    struct TypographyConfig {
        ui_font_size: f32,
        editor_font_size: f32,
    }

    impl Default for TypographyConfig {
        fn default() -> Self {
            Self {
                ui_font_size: 13.0,
                editor_font_size: 16.0,
            }
        }
    }

    pub(crate) fn set_sizes(ui_font_size: u16, editor_font_size: u16) {
        let lock = CONFIG.get_or_init(|| RwLock::new(TypographyConfig::default()));
        match lock.write() {
            Ok(mut config) => {
                config.ui_font_size = ui_font_size as f32;
                config.editor_font_size = editor_font_size as f32;
            }
            Err(error) => eprintln!("更新字号配置失败：{error}"),
        }
    }

    fn current() -> TypographyConfig {
        CONFIG
            .get_or_init(|| RwLock::new(TypographyConfig::default()))
            .read()
            .map(|config| *config)
            .unwrap_or_default()
    }

    static CONFIG: OnceLock<RwLock<TypographyConfig>> = OnceLock::new();

    /// 桌面 UI 字体：JetBrains Mono + Sarasa Mono SC（中文兜底）。
    /// 与编辑区共用一种字体——少一份资源注册，UI 与代码视觉一致。
    pub fn ui_font() -> Font {
        let mut font = font("JetBrains Mono");
        font.fallbacks = Some(cjk_font_fallbacks());
        font
    }

    /// 编辑区代码字体：JetBrains Mono + Sarasa Mono SC（中文兜底）。
    /// JetBrains 为代码场景做的等宽字体——大 x-height、字符区分度高，长时间盯不累。
    /// GSUB 自带 `calt`，但 GPUI 默认不启用编程连字。
    pub fn editor_font() -> Font {
        let mut font = font("JetBrains Mono");
        font.fallbacks = Some(cjk_font_fallbacks());
        font
    }

    fn cjk_font_fallbacks() -> FontFallbacks {
        static FALLBACKS: OnceLock<FontFallbacks> = OnceLock::new();
        FALLBACKS
            .get_or_init(|| FontFallbacks::from_fonts(vec!["Sarasa Mono SC".to_string()]))
            .clone()
    }

    /// UI 文字字号：文件树、顶栏、Dock、标签、浮层、tooltip 等全部 chrome。
    pub fn ui() -> Pixels {
        px(current().ui_font_size)
    }
    /// UI 行的标准尺寸：既是文字行高，也是 UI 图标尺寸
    /// —— 二者同一个值，让「图标 + 文字」一行等高、`items_center` 后盒子对齐。
    pub fn ui_line() -> Pixels {
        px((current().ui_font_size + 3.0).max(14.0))
    }

    /// 编辑区代码字号。后续做成用户可调。
    pub fn editor() -> Pixels {
        px(current().editor_font_size)
    }
    /// 编辑区字号的原始 f32 值，供 ratex 等非 GPUI 渲染使用。
    pub fn editor_font_size() -> f32 {
        current().editor_font_size
    }

    /// 编辑区行高（约 1.5×，照顾代码可读性）。
    pub fn editor_line() -> Pixels {
        px(current().editor_font_size * 1.5)
    }
}
