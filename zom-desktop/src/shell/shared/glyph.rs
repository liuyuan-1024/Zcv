//! `Glyph` —— shell 内复用的基础视觉标记。
//!
//! 它可以承载文字、图标、图标 + 文字，统一携带 tooltip 数据并在悬停时由
//! GPUI 的 tooltip 通道呈现。`Glyph` 只表达视觉，不知道命令系统、bar、
//! surface 或 invoker。

use gpui::{
    AnyElement, AnyView, App, Context, ElementId, IntoElement, Pixels, Render, Svg, Window, div,
    prelude::*, svg,
};

use crate::shell::shared::theme::{color, radius, space, typography};

#[derive(Clone)]
enum GlyphContent {
    Text(String),
    Icon(&'static str),
    IconText { icon: &'static str, text: String },
}

/// 一个基础视觉标记。
pub(crate) struct Glyph {
    id: ElementId,
    content: GlyphContent,
    tooltip: String,
    hint: Option<String>,
    active: bool,
    /// 图标尺寸；只作用于 Icon / IconText 内容，默认 `typography::ui_line()`。
    icon_size: Pixels,
}

impl Glyph {
    pub(crate) fn text(
        id: impl Into<ElementId>,
        text: impl Into<String>,
        tooltip: impl Into<String>,
    ) -> Self {
        Self::new(id, GlyphContent::Text(text.into()), tooltip)
    }

    pub(crate) fn icon(
        id: impl Into<ElementId>,
        path: &'static str,
        tooltip: impl Into<String>,
    ) -> Self {
        Self::new(id, GlyphContent::Icon(path), tooltip)
    }

    pub(crate) fn icon_text(
        id: impl Into<ElementId>,
        path: &'static str,
        text: impl Into<String>,
        tooltip: impl Into<String>,
    ) -> Self {
        Self::new(
            id,
            GlyphContent::IconText {
                icon: path,
                text: text.into(),
            },
            tooltip,
        )
    }

    fn new(id: impl Into<ElementId>, content: GlyphContent, tooltip: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            content,
            tooltip: tooltip.into(),
            hint: None,
            active: false,
            icon_size: typography::ui_line(),
        }
    }

    /// 设置 tooltip 右侧的辅助文本，例如快捷键。
    pub(crate) fn hint(mut self, hint: impl Into<Option<String>>) -> Self {
        self.hint = hint.into();
        self
    }

    pub(crate) fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// 覆盖图标尺寸（默认 `typography::ui_line()`）；对 Text 内容无效。
    pub(crate) fn icon_size(mut self, size: Pixels) -> Self {
        self.icon_size = size;
        self
    }

    pub(crate) fn render(self) -> AnyElement {
        let color_value = if self.active {
            color::gray::s09()
        } else {
            color::gray::s09()
        };
        let id = self.id.clone();
        let icon_size = self.icon_size;
        let tooltip = self.tooltip.clone();
        let hint = self.hint.clone();

        let build_tooltip = move |_window: &mut Window, cx: &mut App| -> AnyView {
            tooltip_view(cx, tooltip.clone(), hint.clone())
        };

        match self.content {
            GlyphContent::Text(text) => div()
                .id(id)
                .text_size(typography::ui())
                .text_color(color_value)
                .cursor_pointer()
                .tooltip(build_tooltip)
                .child(text)
                .into_any_element(),
            GlyphContent::Icon(path) => div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .tooltip(build_tooltip)
                .child(svg_icon(path, color_value, icon_size))
                .into_any_element(),
            GlyphContent::IconText { icon: path, text } => div()
                .id(id)
                .flex()
                .flex_row()
                .items_center()
                .gap(space::s4())
                .cursor_pointer()
                .tooltip(build_tooltip)
                .child(svg_icon(path, color_value, icon_size))
                .child(
                    div()
                        .text_size(typography::ui())
                        .text_color(color_value)
                        .child(text),
                )
                .into_any_element(),
        }
    }
}

fn svg_icon(path: &'static str, color: gpui::Rgba, size: Pixels) -> Svg {
    svg().path(path).size(size).text_color(color)
}

/// 构造 Glyph 共用的 tooltip 视图。
fn tooltip_view(cx: &mut App, label: String, hint: Option<String>) -> AnyView {
    cx.new(|_| GlyphTooltip { label, hint }).into()
}

/// Glyph 悬停时呈现的小视图。
struct GlyphTooltip {
    label: String,
    hint: Option<String>,
}

impl Render for GlyphTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(space::s6())
            .px(space::s6())
            .pt(space::s4())
            .pb(space::s6())
            .bg(color::gray::s03())
            .border_1()
            .border_color(color::gray::s05())
            .rounded(radius::r4())
            .child(
                div()
                    .text_size(typography::ui())
                    .text_color(color::gray::s09())
                    .child(self.label.clone()),
            );

        if let Some(hint) = &self.hint {
            row = row.child(
                div()
                    .text_size(typography::ui())
                    .text_color(color::gray::s08())
                    .child(hint.clone()),
            );
        }

        div().p(space::s6()).child(row)
    }
}
