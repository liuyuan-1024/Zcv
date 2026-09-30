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

/// 结构刻度的基准是固定设计基准：用户改字号时 token 必须随之同比缩放，不能重新锚定。
#[test]
fn structural_tokens_scale_with_rem_size() {
    let at_default = scale::to_pixels_at(scale::S6, gpui::px(scale::DEFAULT_UI_SIZE));
    let doubled = scale::to_pixels_at(scale::S6, gpui::px(scale::DEFAULT_UI_SIZE * 2.0));
    assert!(
        (f32::from(at_default) - 6.0).abs() < 0.001,
        "在设计基准字号下 S6 应为 6px，实际 {at_default:?}"
    );
    assert!(
        (f32::from(doubled) - 12.0).abs() < 0.001,
        "字号翻倍时 S6 应同比翻倍，实际 {doubled:?}"
    );
}
