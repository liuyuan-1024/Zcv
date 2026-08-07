//! Tooltip —— 悬停提示视图。
//!
//! 单一实现，供 Glyph、Checkbox 等组件复用。
//! 快捷键的查询与显示也是 Tooltip 的职责：消费方只需提供 action 名称。
//! 悬停延迟与触发由 gpui 的 `div.tooltip()` 机制承担，这里只负责气泡视觉与快捷键查询。

use gpui::{AnyView, App, Context, Render, Window, div, prelude::*};

use crate::keymap::KeyBindings;
use zcv_theme::{color, space, typography};

/// 构造提示气泡视图（label + 可选快捷键两段式，与 Glyph 原有提示一致）。
pub(crate) fn tooltip_view(
    cx: &mut App,
    label: Option<String>,
    shortcut: Option<String>,
) -> AnyView {
    cx.new(|_| TooltipView { label, shortcut }).into()
}

/// 构造带快捷键的提示气泡：快捷键从 keymap 按 action 名称查询并显示。
pub(crate) fn tooltip_for_action(
    text: impl Into<String>,
    action_name: &str,
    cx: &mut App,
) -> AnyView {
    let shortcut = cx
        .try_global::<KeyBindings>()
        .and_then(|bindings| bindings.display_shortcut(action_name));
    tooltip_view(cx, Some(text.into()), shortcut)
}

/// 提示气泡。
struct TooltipView {
    label: Option<String>,
    shortcut: Option<String>,
}

impl Render for TooltipView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let popup = div()
            .flex()
            .items_center()
            .gap(space::S6)
            .p(space::S6)
            .text_size(typography::ui())
            .line_height(typography::ui())
            .bg(color::current(cx).elevated_surface_background)
            .border_1()
            .border_color(color::current(cx).border_variant)
            .rounded_sm()
            // test cfg 下注册 debug bounds，供 hover 测试断言气泡出现。
            .debug_selector(|| "tooltip-view".into())
            .children(self.label.as_ref().map(|label| {
                div()
                    .text_color(color::current(cx).text)
                    .child(label.clone())
                    .into_any_element()
            }))
            .children(self.shortcut.as_ref().map(|shortcut| {
                div()
                    .text_color(color::current(cx).text_placeholder)
                    .child(shortcut.clone())
                    .into_any_element()
            }));

        // 外层 div(.p) 提供与光标之间的间距，防止气泡被鼠标遮挡。
        div().p(space::S6).child(popup)
    }
}
