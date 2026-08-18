//! Tooltip —— 悬停提示视图。
//!
//! 单一实现，供 Glyph、Checkbox 等组件复用。
//! 快捷键的查询与显示也是 Tooltip 的职责：消费方只需提供 action 名称。
//! 悬停延迟与触发由 gpui 的 `div.tooltip()` 机制承担，这里只负责气泡视觉与快捷键查询。

use gpui::{AnyView, App, Context, Render, Window, div, prelude::*, rems};
use zcv_keymap::KeyBindings;
use zcv_theme::{color, space, typography};

/// 构造提示气泡视图（label + 可选快捷键两段式，与 Glyph 原有提示一致）。
pub fn tooltip_view(cx: &mut App, label: Option<String>, shortcut: Option<String>) -> AnyView {
    cx.new(|_| TooltipView { label, shortcut }).into()
}

/// Tooltip 规格：label 与可选快捷键文本。
///
/// 组件持有规格（而非视图），悬停时才构建气泡 Entity；
/// 快捷键文本由构建方预先从 keymap 查好，保证不依赖悬停时机。
#[derive(Clone, Default)]
pub struct TooltipSpec {
    label: Option<String>,
    shortcut: Option<String>,
}

impl TooltipSpec {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: Some(label.into()),
            shortcut: None,
        }
    }

    /// 设置快捷键显示文本。
    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// 从当前 keymap 中查询 action 的快捷键并设为提示（Glyph/Checkbox 等共用）。
    pub fn with_action(mut self, action: &dyn gpui::Action, cx: &App) -> Self {
        if let Some(s) = cx
            .try_global::<KeyBindings>()
            .and_then(|kb| kb.display_shortcut(action))
        {
            self.shortcut = Some(s);
        }
        self
    }

    /// 是否包含任何提示内容（无内容时不挂 tooltip）。
    pub fn has_content(&self) -> bool {
        self.label.is_some() || self.shortcut.is_some()
    }

    /// 构造悬停气泡视图闭包；无内容时返回 None。
    pub fn build(&self) -> Option<impl Fn(&mut Window, &mut App) -> AnyView + 'static> {
        if !self.has_content() {
            return None;
        }
        let label = self.label.clone();
        let shortcut = self.shortcut.clone();
        Some(move |_: &mut Window, cx: &mut App| tooltip_view(cx, label.clone(), shortcut.clone()))
    }
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
            // 浮动层挂在 window 层，不在根元素树内：
            // 字号经 window rem 基准自动正确；字体需显式设置；行高 = 1rem。
            .font(typography::ui_font())
            .line_height(rems(1.0))
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
