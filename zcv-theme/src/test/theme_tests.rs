use super::*;
use gpui::{Context, IntoElement, Render, TestAppContext, div};

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

/// 供窗口测试挂载的最小 Render 视图（无内容，只为拿到 window）。
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
