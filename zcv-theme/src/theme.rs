//! 视觉 token：色彩、间距、字号、圆角。
//! 此文件是 `zcv-theme` crate 的公共入口。
//!
//! [`ThemeChoice`] 是主题配置入口：`System` 或注册表中的主题 id。
//! 主题数据（语义色 + 语法高亮）由 `theme_data` 注册表统一持有，新增主题只需添加 TOML 文件并在注册表登记，无需改动本模块逻辑。

use std::sync::atomic::{AtomicU16, AtomicU32, Ordering};

use gpui::{App, Font, FontFallbacks, Pixels, Window, font, px};

/// 编译期嵌入的内置设置文件（与 zcv-assets 运行时嵌入为同一文件）：
/// 排版默认值（字号/行高）的唯一数据源，typography 不重复硬编码。
const INITIAL_SETTINGS: &str = include_str!("../../assets/settings/initial_user_settings.json");

pub mod color;
mod icon_theme;
pub mod syntax;
mod theme_data;

use theme_data::{ThemeData, theme_by_id, themes};

pub use icon_theme::FileIcons;

/// 主题配置：跟随系统外观，或显式指定注册表中的主题（id 取自主题文件名）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeChoice {
    System,
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

    // 运行时状态：启动时由 set_typography 注入设置值；0 表示尚未注入，读取时回退内置设置文件的默认值（单一数据源，见 INITIAL_SETTINGS）。
    static UI_FONT_SIZE: AtomicU16 = AtomicU16::new(0);
    static EDITOR_FONT_SIZE: AtomicU16 = AtomicU16::new(0);
    /// 编辑器行高倍数；f32 位模式存储，0 表示未注入。
    static EDITOR_LINE_HEIGHT: AtomicU32 = AtomicU32::new(0);

    /// 应用排版设置（编辑器/UI 字号与编辑器行高倍数）；启动与设置变更时调用，未配置的维度保持当前值不变。
    pub fn set_typography(editor: Option<f32>, ui: Option<f32>, line_height: Option<f32>) {
        if let Some(editor) = editor {
            EDITOR_FONT_SIZE.store(editor as u16, Ordering::Relaxed);
        }
        if let Some(ui) = ui {
            UI_FONT_SIZE.store(ui as u16, Ordering::Relaxed);
        }
        if let Some(line_height) = line_height {
            EDITOR_LINE_HEIGHT.store(line_height.to_bits(), Ordering::Relaxed);
        }
    }

    /// 内置设置文件的排版默认值（font_size / ui_font_size / line_height）。
    fn defaults() -> (f32, f32, f32) {
        static DEFAULTS: std::sync::OnceLock<(f32, f32, f32)> = std::sync::OnceLock::new();
        *DEFAULTS.get_or_init(|| {
            let value: serde_json::Value =
                serde_json::from_str(INITIAL_SETTINGS).expect("内置设置文件应合法");
            let get = |key: &str| value[key].as_f64().expect("内置默认应存在") as f32;
            (get("font_size"), get("ui_font_size"), get("line_height"))
        })
    }

    fn ui_size() -> f32 {
        let size = UI_FONT_SIZE.load(Ordering::Relaxed);
        if size == 0 { defaults().1 } else { size as f32 }
    }
    fn editor_size() -> f32 {
        let size = EDITOR_FONT_SIZE.load(Ordering::Relaxed);
        if size == 0 { defaults().0 } else { size as f32 }
    }
    /// 编辑器行高倍数；未注入时回退内置默认。
    fn editor_line_height() -> f32 {
        let bits = EDITOR_LINE_HEIGHT.load(Ordering::Relaxed);
        if bits == 0 {
            defaults().2
        } else {
            f32::from_bits(bits)
        }
    }

    /// UI 字体：gpui 系统字体（比例，界面文案更圆润），非 ASCII 回退到项目字体资源。
    pub fn ui_font() -> Font {
        let mut font = font(".SystemUIFont");
        font.fallbacks = Some(cjk_fallback());
        font
    }

    /// 编辑器字体：等宽（代码缩进/列对齐依赖等宽），含 CJK 回退。
    pub fn editor_font() -> Font {
        mono_font()
    }

    /// 编辑器等宽字体（含 CJK 回退）。
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
    /// 编辑器行高（相对字号的倍数，缺省内置默认），与 Zed 一致：`round(font_size * 倍数)`
    pub fn editor_line() -> Pixels {
        px((editor_size() * editor_line_height()).round())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 设置字符串解析：system 与主题文件名 id 各归其位，未知回退 System。
    #[test]
    fn theme_choice_from_config() {
        assert_eq!(ThemeChoice::from_config("system"), ThemeChoice::System);
        assert_eq!(ThemeChoice::from_config("dark"), ThemeChoice::Named("dark"));
        assert_eq!(
            ThemeChoice::from_config("light"),
            ThemeChoice::Named("light")
        );
        assert_eq!(ThemeChoice::from_config("unknown"), ThemeChoice::System);
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;
    use gpui::{Context, IntoElement, Render, TestAppContext, Window, div};

    #[derive(Default)]
    struct EmptyView;

    impl Render for EmptyView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    /// System 选择应返回与窗口外观匹配的主题。
    #[gpui::test]
    fn system_theme_matches_window_appearance(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_window, _cx| EmptyView);
        let (appearance, theme_appearance) = cx.update(|window, _| {
            let theme = ThemeChoice::System.effective(Some(window));
            (window.appearance(), theme.appearance)
        });
        assert_eq!(
            theme_appearance, appearance,
            "System 主题应匹配窗口外观 {appearance:?}"
        );
    }
}
