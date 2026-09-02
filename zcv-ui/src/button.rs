//! `Button` —— 通用交互原语：图标 / 文字 / 图标+文字按钮。
//!
//! 统一承载点击交互（on_click / disabled / hover / tooltip）与两种视觉样式：Ghost（幽灵，默认）与 Solid（实心）。
//! 图标部分委托 `SvgIcon` 渲染。

use std::rc::Rc;

use gpui::{
    Action, App, ClickEvent, Component, CursorStyle, ElementId, IntoElement, MouseButton, Pixels,
    RenderOnce, Window, div, prelude::*, rems,
};
use zcv_theme::{color, space, typography};

use crate::{SvgIcon, TooltipSpec};

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

fn cursor_for_state(disabled: bool, has_click_handler: bool) -> Option<CursorStyle> {
    if disabled {
        Some(CursorStyle::OperationNotAllowed)
    } else if has_click_handler {
        Some(CursorStyle::PointingHand)
    } else {
        None
    }
}

/// 按钮视觉样式。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonStyle {
    /// 幽灵样式：悬停才显示背景，用于工具栏/状态栏等高频轻量操作。
    Ghost,
    /// 实心样式：常驻边框与背景，用于突出主操作。
    Solid,
}

/// 按钮尺寸：内边距与圆角随尺寸档位缩放，高度 = UI 字号 + 上下内边距。
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonSize {
    /// 紧凑：小内边距、小圆角，用于工具栏/状态栏等高频轻量操作（默认）。
    #[default]
    Compact,
    /// 宽松：大内边距、大圆角，用于主操作按钮。
    Loose,
}

impl ButtonSize {
    /// 内边距：紧凑 S2，宽松 S6。
    fn padding(self) -> Pixels {
        match self {
            ButtonSize::Compact => space::S2,
            ButtonSize::Loose => space::S6,
        }
    }

    /// 整体高度 = UI 字号 + 上下内边距。
    fn height(self) -> Pixels {
        typography::ui_size() + self.padding() * 2.0
    }

    /// 按档位施加内边距与圆角（紧凑小圆角，宽松大圆角）。
    fn shell<D: Styled>(self, element: D) -> D {
        match self {
            ButtonSize::Compact => element.rounded_sm().p(self.padding()),
            ButtonSize::Loose => element.rounded_md().p(self.padding()),
        }
    }
}

#[derive(Clone)]
enum ButtonContent {
    Icon(&'static str),
    Text(String),
    IconText { icon: &'static str, text: String },
}

/// 可交互的 UI 原语。
pub struct Button {
    id: ElementId,
    content: ButtonContent,
    style: ButtonStyle,
    size: ButtonSize,
    color: Option<gpui::Rgba>,
    tooltip: TooltipSpec,
    on_click: Option<ClickHandler>,
    disabled: bool,
    /// hitbox 是否遮蔽下层元素（浮层内按钮关闭遮蔽，避免打断外层 hover 追踪）。
    occlude: bool,
}

impl Button {
    pub fn icon(id: impl Into<ElementId>, path: &'static str) -> Self {
        Self::new(id, ButtonContent::Icon(path))
    }

    pub fn text(id: impl Into<ElementId>, text: impl Into<String>) -> Self {
        Self::new(id, ButtonContent::Text(text.into()))
    }

    pub fn icon_text(
        id: impl Into<ElementId>,
        path: &'static str,
        text: impl Into<String>,
    ) -> Self {
        Self::new(
            id,
            ButtonContent::IconText {
                icon: path,
                text: text.into(),
            },
        )
    }

    fn new(id: impl Into<ElementId>, content: ButtonContent) -> Self {
        Self {
            id: id.into(),
            content,
            style: ButtonStyle::Ghost,
            size: ButtonSize::Compact,
            color: None,
            tooltip: TooltipSpec::default(),
            on_click: None,
            disabled: false,
            occlude: true,
        }
    }

    /// 关闭 hitbox 遮蔽：按钮位于浮层内时避免遮蔽外层元素的 hover 命中。
    pub fn no_occlude(mut self) -> Self {
        self.occlude = false;
        self
    }

    /// 设置视觉样式。
    pub fn style(mut self, style: ButtonStyle) -> Self {
        self.style = style;
        self
    }

    /// 设置尺寸（默认紧凑）。
    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    /// 设置 tooltip 标签文字。
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.tooltip = TooltipSpec::new(label);
        self
    }

    /// 从当前 keymap 中获取 action 对应的快捷键并设为提示。
    pub fn shortcut(mut self, action: &dyn Action, cx: &App) -> Self {
        self.tooltip = self.tooltip.with_action(action, cx);
        self
    }

    /// 设置颜色（覆盖默认色）。
    pub fn color(mut self, color: gpui::Rgba) -> Self {
        self.color = Some(color);
        self
    }

    /// 设置点击回调。
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    /// 禁用交互，但保留图形作为可见的操作反馈。
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl IntoElement for Button {
    type Element = Component<Self>;

    fn into_element(self) -> Self::Element {
        Component::new(self)
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = *color::current(cx);
        let disabled = self.disabled;
        let has_click = self.on_click.is_some();
        let color = if disabled {
            colors.text_disabled
        } else {
            self.color.unwrap_or(colors.text)
        };
        let icon_only = matches!(&self.content, ButtonContent::Icon(_));

        // 基础容器：尺寸档位决定高度、内边距与圆角。
        let height = self.size.height();
        let mut element = self.size.shell(
            div()
                .id(self.id)
                .h(height)
                .when(icon_only, |element| element.w(height))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .font(typography::ui_font())
                .text_size(typography::ui_size())
                .line_height(rems(1.0))
                .when(self.occlude, |element| element.occlude()),
        );
        if self.style == ButtonStyle::Solid {
            element = element
                .border_1()
                .border_color(colors.border_variant)
                .bg(colors.panel_background);
        }

        // 交互：禁用优先；有点击回调时拦截冒泡，避免触发外层点击。
        if let Some(cursor) = cursor_for_state(disabled, has_click) {
            element = element.cursor(cursor);
        }
        if !disabled {
            let hover_background = match self.style {
                ButtonStyle::Ghost => colors.ghost_element_hover,
                ButtonStyle::Solid => colors.element_hover,
            };
            element = element.hover(move |style| style.bg(hover_background));
        }
        if !disabled && let Some(handler) = self.on_click {
            element = element
                .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                    cx.stop_propagation()
                })
                .on_click(move |event, window, cx| {
                    handler(event, window, cx);
                    cx.stop_propagation();
                });
        }
        if let Some(build) = self.tooltip.build() {
            element = element.tooltip(build);
        }

        // 内容形态；图标统一经 SvgIcon 渲染。
        element.child(match self.content {
            ButtonContent::Icon(path) => SvgIcon::new(path)
                .size(typography::ui_size())
                .color(color)
                .into_any_element(),
            ButtonContent::Text(text) => div().text_color(color).child(text).into_any_element(),
            ButtonContent::IconText { icon: path, text } => div()
                .flex()
                .flex_row()
                .items_center()
                .gap(space::S2)
                .child(SvgIcon::new(path).size(typography::ui_size()).color(color))
                .child(div().text_color(color).child(text))
                .into_any_element(),
        })
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Context, Render, TestAppContext};

    use super::*;

    #[test]
    fn disabled_state_uses_not_allowed_cursor() {
        assert_eq!(
            cursor_for_state(true, true),
            Some(CursorStyle::OperationNotAllowed)
        );
        assert_eq!(
            cursor_for_state(true, false),
            Some(CursorStyle::OperationNotAllowed)
        );
    }

    #[test]
    fn enabled_state_preserves_existing_interaction() {
        assert_eq!(
            cursor_for_state(false, true),
            Some(CursorStyle::PointingHand)
        );
        assert_eq!(cursor_for_state(false, false), None);
    }

    struct ButtonHeightHost;

    impl Render for ButtonHeightHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .debug_selector(|| "icon-button".into())
                        .child(Button::icon("icon", "icons/close.svg")),
                )
                .child(
                    div()
                        .debug_selector(|| "text-button".into())
                        .child(Button::text("text", "关闭").style(ButtonStyle::Solid)),
                )
                .child(
                    div()
                        .debug_selector(|| "icon-text-button".into())
                        .child(Button::icon_text("icon-text", "icons/close.svg", "关闭")),
                )
        }
    }

    #[gpui::test]
    fn content_and_visual_style_share_the_default_height(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| ButtonHeightHost);
        let icon = cx.debug_bounds("icon-button").expect("图标按钮应参与布局");
        let text = cx.debug_bounds("text-button").expect("文字按钮应参与布局");
        let icon_text = cx
            .debug_bounds("icon-text-button")
            .expect("图文按钮应参与布局");

        assert_eq!(icon.size.height, text.size.height);
        assert_eq!(text.size.height, icon_text.size.height);
        assert_eq!(icon.size.height, ButtonSize::Compact.height());
    }

    struct LooseButtonHost;

    impl Render for LooseButtonHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex()
                .flex_col()
                .child(div().debug_selector(|| "loose-icon".into()).child(
                    Button::icon("loose-icon-btn", "icons/close.svg").size(ButtonSize::Loose),
                ))
                .child(
                    div().debug_selector(|| "loose-text".into()).child(
                        Button::text("loose-text-btn", "确定")
                            .size(ButtonSize::Loose)
                            .style(ButtonStyle::Solid),
                    ),
                )
        }
    }

    #[gpui::test]
    fn loose_size_scales_height_and_padding(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| LooseButtonHost);
        let icon = cx
            .debug_bounds("loose-icon")
            .expect("宽松图标按钮应参与布局");
        let text = cx
            .debug_bounds("loose-text")
            .expect("宽松文字按钮应参与布局");

        // 宽松高度 = 字号 + S6×2，与紧凑档位差 2×(S6−S2)。
        let expected = ButtonSize::Loose.height();
        assert_eq!(icon.size.height, expected);
        assert_eq!(text.size.height, expected);
        assert_eq!(
            icon.size.height,
            ButtonSize::Compact.height() + space::S6 * 2.0 - space::S2 * 2.0
        );
    }
}
