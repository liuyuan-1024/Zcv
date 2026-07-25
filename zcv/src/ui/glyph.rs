//! `Glyph` —— shell 内复用的基础展示组件。
//!
//! 可以承载文字、图标、图标 + 文字。
//! label 和 shortcut 是可选的 tooltip 信息，通过 builder 方法设置。

use std::rc::Rc;

use gpui::{
    Action, AnyElement, AnyView, App, Context, ElementId, IntoElement, Render, Window, div,
    prelude::*,
};

use crate::keymap::KeyBindings;
use crate::theme::{color, radius, space, typography};
use crate::ui::icon::SvgIcon;

type ClickHandler = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone)]
enum GlyphContent {
    Icon(&'static str),
    Text(String),
    IconText { icon: &'static str, text: String },
}

/// 一个基础视觉标记。
///
/// 通过 builder 设置 label/shortcut/color 来控制展示内容。
pub(crate) struct Glyph {
    id: ElementId,
    content: GlyphContent,
    color: gpui::Rgba,
    label: Option<String>,
    shortcut: Option<String>,
    on_click: Option<ClickHandler>,
}

impl Glyph {
    pub(crate) fn icon(id: impl Into<ElementId>, path: &'static str) -> Self {
        Self::new(id, GlyphContent::Icon(path))
    }

    pub(crate) fn text(id: impl Into<ElementId>, text: impl Into<String>) -> Self {
        Self::new(id, GlyphContent::Text(text.into()))
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
            color: color::default(),
            label: None,
            shortcut: None,
            on_click: None,
        }
    }

    /// 设置 tooltip 标签文字。
    pub(crate) fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// 从当前 keymap 中获取 action 对应的快捷键并设为提示。
    pub(crate) fn shortcut(mut self, action: &impl Action, cx: &App) -> Self {
        self.shortcut = cx
            .try_global::<KeyBindings>()
            .and_then(|kb| kb.display_shortcut(action.name()));
        self
    }

    /// 按 action 名称从 keymap 中查找快捷键并设为提示（不需要具体 action 类型）。
    pub(crate) fn shortcut_by_name(mut self, action_name: &str, cx: &App) -> Self {
        self.shortcut = cx
            .try_global::<KeyBindings>()
            .and_then(|kb| kb.display_shortcut(action_name));
        self
    }

    /// 设置颜色（覆盖默认色）。
    pub(crate) fn color(mut self, color: gpui::Rgba) -> Self {
        self.color = color;
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
        let icon_size = typography::ui();
        let label = self.label;
        let shortcut = self.shortcut;
        let on_click = self.on_click;
        let has_tooltip = label.is_some() || shortcut.is_some();

        let build_tooltip = move |_: &mut Window, cx: &mut App| -> AnyView {
            tooltip_view(cx, label.clone(), shortcut.clone())
        };

        let base = |mut el: gpui::Stateful<gpui::Div>| {
            el = el.cursor_pointer();
            if has_tooltip {
                el = el.tooltip(build_tooltip);
            }
            if let Some(ref handler) = on_click {
                let h = Rc::clone(handler);
                el = el.on_click(move |_, window, cx| h(window, cx));
            }
            el.into_any_element()
        };

        match self.content {
            GlyphContent::Text(text) => base(div().id(self.id).text_color(self.color).child(text)),
            GlyphContent::Icon(path) => base(
                div()
                    .id(self.id)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(SvgIcon::new(path).size(icon_size).color(self.color)),
            ),
            GlyphContent::IconText { icon: path, text } => base(
                div()
                    .id(self.id)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(space::S2)
                    .child(SvgIcon::new(path).size(icon_size).color(self.color))
                    .child(div().text_color(self.color).child(text)),
            ),
        }
    }
}

/// 构造 Glyph 共用的 tooltip 视图。
fn tooltip_view(cx: &mut App, label: Option<String>, shortcut: Option<String>) -> AnyView {
    cx.new(|_| GlyphTooltip { label, shortcut }).into()
}

/// Glyph 悬停时呈现的小视图。
struct GlyphTooltip {
    label: Option<String>,
    shortcut: Option<String>,
}

impl Render for GlyphTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let popup = div()
            .flex()
            .items_center()
            .gap(space::S6)
            .p(space::S6)
            .text_size(typography::ui())
            .line_height(typography::ui())
            .bg(color::current().gray.s[2])
            .border_1()
            .border_color(color::current().gray.s[4])
            .rounded(radius::R4)
            .children(self.label.as_ref().map(|l| {
                div()
                    .text_color(color::current().gray.s[8])
                    .child(l.clone())
                    .into_any_element()
            }))
            .children(self.shortcut.as_ref().map(|s| {
                div()
                    .text_color(color::current().gray.s[5])
                    .child(s.clone())
                    .into_any_element()
            }));

        // 外层 div(.p) 提供与光标之间的间距，防止 tooltip 气泡被鼠标遮挡
        div().p(space::S6).child(popup)
    }
}
