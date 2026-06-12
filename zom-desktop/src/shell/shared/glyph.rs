//! `Glyph` —— shell 内复用的基础视觉标记。
//!
//! 它可以承载文字、图标、图标 + 文字，
//! 统一携带 tooltip 数据并在悬停时由 GPUI 的 tooltip 通道呈现。
//! `Glyph` 只表达视觉，不知道命令系统、bar、surface 或 invoker。

use gpui::{
    AnyElement, AnyView, App, Context, ElementId, IntoElement, MouseButton, Pixels, Render,
    Stateful, Svg, Window, div, prelude::*, svg,
};

use crate::host_intent::CommandRequest;
use crate::theme::{color, radius, space, typography};

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
    icon_size: Pixels,
    font_size: Pixels,
    line_height: Pixels,
    press: Option<CommandRequest>,
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
            icon_size: typography::ui(),
            font_size: typography::ui(),
            line_height: typography::ui_line(),
            press: None,
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

    /// 绑定一个已由 shell 预先接好的命令请求。
    ///
    /// `Glyph` 不知道命令 id 或 Invocation，只在鼠标按下时转发请求。
    pub(crate) fn on_press(mut self, request: CommandRequest) -> Self {
        self.press = Some(request);
        self
    }

    pub(crate) fn render(self) -> AnyElement {
        let color_value = if self.active {
            color::current().blue.s07
        } else {
            color::current().gray.s09
        };
        let id = self.id.clone();
        let icon_size = self.icon_size;
        let font_size = self.font_size;
        let line_height = self.line_height;
        let tooltip = self.tooltip.clone();
        let hint = self.hint.clone();
        let press = self.press;

        let build_tooltip = move |_window: &mut Window, cx: &mut App| -> AnyView {
            tooltip_view(cx, tooltip.clone(), hint.clone())
        };

        match self.content {
            GlyphContent::Text(text) => pressable(
                div()
                    .id(id)
                    .text_size(font_size)
                    .line_height(line_height)
                    .text_color(color_value)
                    .cursor_pointer()
                    .tooltip(build_tooltip)
                    .child(text),
                press,
            )
            .into_any_element(),
            GlyphContent::Icon(path) => pressable(
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .tooltip(build_tooltip)
                    .child(svg_icon(path, color_value, icon_size)),
                press,
            )
            .into_any_element(),
            GlyphContent::IconText { icon: path, text } => pressable(
                div()
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
                            .text_size(font_size)
                            .line_height(line_height)
                            .text_color(color_value)
                            .child(text),
                    ),
                press,
            )
            .into_any_element(),
        }
    }
}

fn pressable(element: Stateful<gpui::Div>, request: Option<CommandRequest>) -> Stateful<gpui::Div> {
    let Some(request) = request else {
        return element;
    };

    element.on_mouse_down(MouseButton::Left, move |_, window, cx| {
        request(window, cx);
        cx.stop_propagation();
    })
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
            .bg(color::current().gray.s03)
            .border_1()
            .border_color(color::current().gray.s05)
            .rounded(radius::r4())
            .child(
                div()
                    .text_size(typography::ui())
                    .text_color(color::current().gray.s09)
                    .child(self.label.clone()),
            );

        if let Some(hint) = &self.hint {
            row = row.child(
                div()
                    .text_size(typography::ui())
                    .text_color(color::current().gray.s08)
                    .child(hint.clone()),
            );
        }

        div().p(space::s6()).child(row)
    }
}
