//! `Glyph` —— shell 内复用的基础展示组件。
//!
//! 可以承载文字、图标、图标 + 文字。
//! label 和 shortcut 是可选的 tooltip 信息，通过 builder 方法设置。

use std::rc::Rc;

use gpui::{
    Action, App, ClickEvent, Component, ElementId, IntoElement, RenderOnce, Window, div, prelude::*,
};
use zcv_theme::{color, space, typography};

use crate::ui::{SvgIcon, TooltipSpec};

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(Clone)]
enum GlyphContent {
    Icon(&'static str),
    Text(String),
    IconText { icon: &'static str, text: String },
}

/// 一个基础视觉标记。
///
/// 通过 builder 设置 label/shortcut/color 来控制展示内容。
pub struct Glyph {
    id: ElementId,
    content: GlyphContent,
    color: Option<gpui::Rgba>,
    tooltip: TooltipSpec,
    on_click: Option<ClickHandler>,
}

impl Glyph {
    pub fn icon(id: impl Into<ElementId>, path: &'static str) -> Self {
        Self::new(id, GlyphContent::Icon(path))
    }

    pub fn text(id: impl Into<ElementId>, text: impl Into<String>) -> Self {
        Self::new(id, GlyphContent::Text(text.into()))
    }

    pub fn icon_text(
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
            // 默认色延迟到 render（有 cx）解析
            color: None,
            tooltip: TooltipSpec::default(),
            on_click: None,
        }
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
    ///
    /// 回调携带完整点击事件：行内使用时可在回调内 `cx.stop_propagation()`阻止所在行的点击行为（例如 picker 行的打开项目）。
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl IntoElement for Glyph {
    type Element = Component<Self>;

    fn into_element(self) -> Self::Element {
        Component::new(self)
    }
}

impl RenderOnce for Glyph {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        // 默认色依赖主题，只能在有 cx 的 render 中解析
        let color = self.color.unwrap_or_else(|| color::current(cx).text);
        let icon_size = typography::ui();
        let tooltip = self.tooltip;
        let on_click = self.on_click;

        let base = |mut el: gpui::Stateful<gpui::Div>| {
            // 只有可点击的 glyph 才显示手型光标
            if on_click.is_some() {
                el = el.cursor_pointer();
            }
            if let Some(build) = tooltip.build() {
                el = el.tooltip(build);
            }
            if let Some(ref handler) = on_click {
                let h = Rc::clone(handler);
                el = el.on_click(move |event, window, cx| h(event, window, cx));
            }
            el.into_any_element()
        };

        match self.content {
            GlyphContent::Text(text) => base(div().id(self.id).text_color(color).child(text)),
            GlyphContent::Icon(path) => base(
                div()
                    .id(self.id)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(SvgIcon::new(path).size(icon_size).color(color)),
            ),
            GlyphContent::IconText { icon: path, text } => base(
                div()
                    .id(self.id)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(space::S2)
                    .child(SvgIcon::new(path).size(icon_size).color(color))
                    .child(div().text_color(color).child(text)),
            ),
        }
    }
}
