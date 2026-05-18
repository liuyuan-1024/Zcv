//! `ShellGlyph` —— shell bar 内的基础视觉标记（布局模型 3.2）。
//!
//! 它可以承载文字、图标、图标 + 文字或纯视觉内容，统一携带 tooltip 数据
//! 并在悬停时由 gpui 的 tooltip 通道呈现。
//!
//! `ShellGlyph` 不表达可点击语义：shell bar 内的标记更像键盘驱动工作流里
//! 的状态与入口提示；实际动作由 keymap / 命令系统承载。鼠标悬停只展示
//! 「可指向」的手型光标作为入口暗示。

use gpui::{
    AnyElement, AnyView, App, Context, ElementId, IntoElement, Render, Svg, Window, div,
    prelude::*, svg,
};

use crate::shell::ShortcutLookup;
use crate::shell::theme::{color, icon, radius, space, typography};

#[derive(Clone)]
enum GlyphContent {
    Text(String),
    Icon(&'static str),
    IconText { icon: &'static str, text: String },
}

/// 一个 shell bar 内的视觉标记。
///
/// 接入命令系统：`command_id` 标注本 glyph 所代表的命令；`render` 时通过
/// `ShortcutLookup` 反查 keymap，把快捷键文案填进 tooltip。
/// 没有命令的 glyph（纯状态指示，比如 LSP / 诊断计数）保留 `command_id = None`。
/// 不可点击 —— 触发动作的唯一入口仍是键盘 / 命令面板（布局模型 3.2）。
pub(crate) struct Glyph {
    id: ElementId,
    content: GlyphContent,
    tooltip: String,
    command_id: Option<&'static str>,
    active: bool,
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
            command_id: None,
            active: false,
        }
    }

    /// 标注本 glyph 代表的命令 id。tooltip 会自动反查 keymap 展示快捷键。
    pub(crate) fn command(mut self, command_id: &'static str) -> Self {
        self.command_id = Some(command_id);
        self
    }

    pub(crate) fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub(crate) fn render(self, shortcuts: &ShortcutLookup) -> AnyElement {
        let color_value = if self.active {
            color::gray::g95()
        } else {
            color::gray::g75()
        };
        let id = self.id.clone();
        let tooltip = self.tooltip.clone();
        let shortcut = self.command_id.and_then(|cmd| shortcuts(cmd));

        let build_tooltip = move |_window: &mut Window, cx: &mut App| -> AnyView {
            tooltip_view(cx, tooltip.clone(), shortcut.clone())
        };

        match self.content {
            GlyphContent::Text(text) => div()
                .id(id)
                .text_size(typography::body())
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
                .child(svg_icon(path, color_value))
                .into_any_element(),
            GlyphContent::IconText { icon: path, text } => div()
                .id(id)
                .flex()
                .flex_row()
                .items_center()
                .gap(space::s4())
                .cursor_pointer()
                .tooltip(build_tooltip)
                .child(svg_icon(path, color_value))
                .child(
                    div()
                        .text_size(typography::body())
                        .text_color(color_value)
                        .child(text),
                )
                .into_any_element(),
        }
    }
}

fn svg_icon(path: &'static str, color: gpui::Rgba) -> Svg {
    svg().path(path).size(icon::i16()).text_color(color)
}

/// 构造 Glyph 共用的 tooltip 视图（布局模型 3.2：标题 + 可选快捷键）。
fn tooltip_view(cx: &mut App, label: String, shortcut: Option<String>) -> AnyView {
    cx.new(|_| GlyphTooltip { label, shortcut }).into()
}

/// Glyph 悬停时呈现的小视图。
struct GlyphTooltip {
    label: String,
    shortcut: Option<String>,
}

impl Render for GlyphTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(space::s8())
            .px(space::s6())
            .pt(space::s4())
            .pb(space::s6())
            .bg(color::gray::g10())
            .border_1()
            .border_color(color::gray::g40())
            .rounded(radius::r4())
            .child(
                div()
                    .text_size(typography::caption())
                    .text_color(color::gray::g95())
                    .child(self.label.clone()),
            );

        if let Some(shortcut) = &self.shortcut {
            row = row.child(
                div()
                    .text_size(typography::caption())
                    .text_color(color::gray::g60())
                    .child(shortcut.clone()),
            );
        }

        div().p(space::s8()).child(row)
    }
}
