//! `Glyph` —— shell 内复用的基础视觉标记。
//!
//! 它可以承载文字、图标、图标 + 文字。
//! 通过 `.command(CommandBinding)` 绑定一条命令后，Glyph 自动从中央 lookup 拉取 tooltip 与快捷键 hint，并在点击时派发命令请求。
//! 未绑命令时为纯展示元素。

use gpui::{
    AnyElement, AnyView, App, Context, ElementId, IntoElement, MouseButton, Pixels, Render,
    Stateful, Svg, Window, div, prelude::*, svg,
};

use crate::host_intent::CommandRequest;
use crate::theme::{color, radius, space, typography};

use super::interaction::{CommandTitleLookup, ShortcutLookup};

#[derive(Clone)]
enum GlyphContent {
    Text(String),
    Icon(&'static str),
    IconText { icon: &'static str, text: String },
}

/// 一条命令在 Glyph 上的完整表达：标识 + 展示查表 + 点击执行。
#[derive(Clone)]
pub(crate) struct CommandBinding {
    pub(crate) id: String,
    pub(crate) title: CommandTitleLookup,
    pub(crate) shortcut: ShortcutLookup,
    pub(crate) request: CommandRequest,
}

/// 一个基础视觉标记。
pub(crate) struct Glyph {
    id: ElementId,
    content: GlyphContent,
    command: Option<CommandBinding>,
    color: Option<gpui::Rgba>,
}

impl Glyph {
    pub(crate) fn text(id: impl Into<ElementId>, text: impl Into<String>) -> Self {
        Self::new(id, GlyphContent::Text(text.into()))
    }

    pub(crate) fn icon(id: impl Into<ElementId>, path: &'static str) -> Self {
        Self::new(id, GlyphContent::Icon(path))
    }

    pub(crate) fn icon_text(
        id: impl Into<ElementId>,
        path: &'static str,
        text: impl Into<String>,
    ) -> Self {
        Self::new(
            id,
            GlyphContent::IconText {
                icon: path,
                text: text.into(),
            },
        )
    }

    fn new(id: impl Into<ElementId>, content: GlyphContent) -> Self {
        Self {
            id: id.into(),
            content,
            command: None,
            color: None,
        }
    }

    /// 覆盖默认的文字/图标颜色。不调用则使用 [`color::glyph_default`]。
    pub(crate) fn color(mut self, color: gpui::Rgba) -> Self {
        self.color = Some(color);
        self
    }

    /// 告诉 Glyph 它代表哪条命令。
    /// 渲染时自动从 lookup 拉取 tooltip 和快捷键 hint，并在点击时派发 `request`。
    pub(crate) fn command(mut self, binding: CommandBinding) -> Self {
        self.command = Some(binding);
        self
    }

    pub(crate) fn render(self) -> AnyElement {
        let color_value = self.color.unwrap_or(color::glyph_default());
        let id = self.id.clone();
        let icon_size = typography::ui();
        let font_size = typography::ui();
        let line_height = typography::ui_line();
        let (tooltip, hint, press) = if let Some(ref cmd) = self.command {
            let title = (cmd.title)(&cmd.id).unwrap_or_else(|| cmd.id.clone());
            let hint = (cmd.shortcut)(&cmd.id);
            (title, hint, Some(cmd.request.clone()))
        } else {
            (String::new(), None, None)
        };

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
                    .gap(space::s2())
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
