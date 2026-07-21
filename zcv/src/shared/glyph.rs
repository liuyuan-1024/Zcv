//! `Glyph` —— shell 内复用的基础交互组件。
//!
//! 可以承载文字、图标、图标 + 文字。
//! tooltip 是必选项，快捷键（通过 action 关联）是可选项。

use std::rc::Rc;

use gpui::{
    AnyElement, AnyView, App, Context, ElementId, IntoElement, Render, Window, div, prelude::*,
};

use crate::shared::icon::SvgIcon;

use crate::keymap::KeyBindings;
use crate::theme::{color, radius, space, typography};

type ClickHandler = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone)]
enum GlyphContent {
    Icon(&'static str),
    Text(String),
    IconText { icon: &'static str, text: String },
}

/// 一个基础视觉标记。
pub(crate) struct Glyph {
    id: ElementId,
    content: GlyphContent,
    color: gpui::Rgba,
    active: bool,
    tooltip: String,
    action_name: Option<&'static str>,
    on_click: Option<ClickHandler>,
}

impl Glyph {
    pub(crate) fn icon(
        id: impl Into<ElementId>,
        path: &'static str,
        tooltip: impl Into<String>,
    ) -> Self {
        Self::new(id, GlyphContent::Icon(path), tooltip.into())
    }

    pub(crate) fn text(
        id: impl Into<ElementId>,
        text: impl Into<String>,
        tooltip: impl Into<String>,
    ) -> Self {
        Self::new(id, GlyphContent::Text(text.into()), tooltip.into())
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
            tooltip.into(),
        )
    }

    fn new(id: impl Into<ElementId>, content: GlyphContent, tooltip: String) -> Self {
        Self {
            id: id.into(),
            content,
            color: color::glyph_default(),
            active: false,
            tooltip,
            action_name: None,
            on_click: None,
        }
    }

    /// 设为激活态，文字/图标颜色自动切换为激活色。
    pub(crate) fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// 关联一个 action，tooltip 自动从 keymap 查询快捷键。
    pub(crate) fn action(mut self, action: impl gpui::Action + 'static) -> Self {
        self.action_name = Some(action.name());
        self
    }

    /// 设置点击回调。
    pub(crate) fn on_click(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl IntoElement for Glyph {
    type Element = AnyElement;

    fn into_element(self) -> Self::Element {
        let color_value = if self.active {
            color::glyph_active()
        } else {
            self.color
        };
        let icon_size = typography::ui();
        let label_text = self.tooltip;
        let action_name = self.action_name;
        let on_click = self.on_click;

        let build_tooltip = move |_: &mut Window, cx: &mut App| -> AnyView {
            let shortcut = action_name.and_then(|name| {
                cx.try_global::<KeyBindings>()
                    .and_then(|kb| kb.display_shortcut(name))
            });
            tooltip_view(cx, label_text.clone(), shortcut)
        };

        let base = |mut el: gpui::Stateful<gpui::Div>| {
            el = el.cursor_pointer().tooltip(build_tooltip);
            if let Some(ref handler) = on_click {
                let h = Rc::clone(handler);
                el = el.on_click(move |_, window, cx| h(window, cx));
            }
            el.into_any_element()
        };

        match self.content {
            GlyphContent::Text(text) => base(div().id(self.id).text_color(color_value).child(text)),
            GlyphContent::Icon(path) => base(
                div()
                    .id(self.id)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(SvgIcon::new(path).size(icon_size).color(color_value)),
            ),
            GlyphContent::IconText { icon: path, text } => base(
                div()
                    .id(self.id)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(space::S2)
                    .child(SvgIcon::new(path).size(icon_size).color(color_value))
                    .child(div().text_color(color_value).child(text)),
            ),
        }
    }
}

/// 构造 Glyph 共用的 tooltip 视图。
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
        let popup = div()
            .flex()
            .items_center()
            .gap(space::S8)
            .px(space::S6)
            .py(space::S4)
            .text_size(typography::ui())
            .line_height(typography::ui())
            .bg(color::current().gray.s[2])
            .border_1()
            .border_color(color::current().gray.s[4])
            .rounded(radius::R4)
            .child(
                div()
                    .text_color(color::current().gray.s[8])
                    .child(self.label.clone()),
            )
            .children(self.shortcut.as_ref().map(|s| {
                div()
                    .text_color(color::current().gray.s[5])
                    .child(s.clone())
                    .into_any_element()
            }));

        // 外层 div(.p) 提供与光标之间的间距，防止 tooltip 气泡被鼠标遮挡
        div().p(space::S8).child(popup)
    }
}
