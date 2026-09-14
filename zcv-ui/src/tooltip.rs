//! Tooltip —— 悬停提示视图。
//!
//! 单一实现，供 Button、SvgIcon、Checkbox 等组件复用。
//! 快捷键的查询与显示也是 Tooltip 的职责：消费方只需提供 action 名称。
//! 悬停延迟与触发由 gpui 的 `div.tooltip()` 机制承担，这里只负责气泡视觉与快捷键查询。

use gpui::{AnyView, App, Context, Render, Window, div, prelude::*, px};
use zcv_keymap::KeyBindings;
use zcv_theme::{color, space, typography};

/// 构造提示气泡视图（多行内容 + 可选快捷键）。
fn tooltip_view(cx: &mut App, lines: Vec<String>, shortcut: Option<String>) -> AnyView {
    cx.new(|_| TooltipView { lines, shortcut }).into()
}

/// Tooltip 规格：多行内容与可选快捷键文本。
///
/// `from_lines` 是通用的多行构造入口；Tooltip 负责按内容宽度换行，行数由调用方决定。
///
/// 组件持有规格（而非视图），悬停时才构建气泡 Entity；
/// 快捷键文本由构建方预先从 keymap 查好，保证不依赖悬停时机。
#[derive(Clone, Default)]
pub struct TooltipSpec {
    lines: Vec<String>,
    shortcut: Option<String>,
}

impl TooltipSpec {
    pub fn from_lines<I, S>(lines: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            lines: lines.into_iter().map(Into::into).collect(),
            shortcut: None,
        }
    }

    /// 设置快捷键显示文本。
    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// 从当前 keymap 中查询 action 的快捷键并设为提示（Button/SvgIcon/Checkbox 等共用）。
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
        !self.lines.is_empty() || self.shortcut.is_some()
    }

    /// 构造悬停气泡视图闭包；无内容时返回 None。
    pub fn build(&self) -> Option<impl Fn(&mut Window, &mut App) -> AnyView + 'static> {
        if !self.has_content() {
            return None;
        }
        let lines = self.lines.clone();
        let shortcut = self.shortcut.clone();
        Some(move |_: &mut Window, cx: &mut App| tooltip_view(cx, lines.clone(), shortcut.clone()))
    }
}

/// 提示气泡。
struct TooltipView {
    lines: Vec<String>,
    shortcut: Option<String>,
}

impl Render for TooltipView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut content = div().flex().flex_col().items_start().gap(space::S2);
        for line in &self.lines {
            content = content.child(
                div()
                    // 限制气泡宽度，让长文本自然换行；
                    // 每行也可以自行包含换行符。
                    .max_w(px(480.0))
                    .text_color(color::current(cx).text)
                    .child(line.clone()),
            );
        }

        let mut popup = div()
            .flex()
            .items_center()
            .gap(space::S6)
            .p(space::S6)
            // 浮动层挂在 window 层，不在根元素树内：
            // 字号经 window rem 基准自动正确；字体需显式设置；行高 = ui_line()（墨迹高度，与根元素同源 token）。
            .font(typography::ui_font())
            .line_height(typography::ui_line_at(window.rem_size()))
            .bg(color::current(cx).elevated_surface_background)
            .border_1()
            .border_color(color::current(cx).border_variant)
            .rounded_sm()
            // test cfg 下注册 debug bounds，供 hover 测试断言气泡出现。
            .debug_selector(|| "tooltip-view".into());
        if !self.lines.is_empty() {
            popup = popup.child(content);
        }
        if let Some(shortcut) = &self.shortcut {
            popup = popup.child(
                div()
                    .text_color(color::current(cx).text_placeholder)
                    .child(shortcut.clone()),
            );
        }

        // 外层 div(.p) 提供与光标之间的间距，防止气泡被鼠标遮挡。
        div().p(space::S6).child(popup)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltip_lines_are_structured_without_embedded_separators() {
        let spec = TooltipSpec::from_lines(["完整值", "右键复制该列信息", "第三行提示"]);

        assert_eq!(
            spec.lines,
            vec![
                "完整值".to_string(),
                "右键复制该列信息".to_string(),
                "第三行提示".to_string(),
            ]
        );
    }
}
