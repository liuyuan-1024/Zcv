//! Tab —— 标签页 UI 组件。
//!
//! 纯视觉组件，不依赖业务类型。
//! 通过 builder 设置图标、关闭按钮、选中状态，调用方通过 [`InteractiveElement`] 方法挂载事件（点击、拖拽等）。

use gpui::{
    AnyElement, App, Div, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    Stateful, StatefulInteractiveElement, Window, div, prelude::*, px,
};

use crate::theme::{color, radius, space};

/// 标签页组件。
pub(crate) struct Tab {
    pub div: Stateful<Div>,
    pub selected: bool,
    pub start_slot: Option<AnyElement>,
    pub end_slot: Option<AnyElement>,
    pub children: Vec<AnyElement>,
}

impl Tab {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            div: div().id(id),
            selected: false,
            start_slot: None,
            end_slot: None,
            children: Vec::new(),
        }
    }

    /// 设置选中状态。
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// 起始槽位（文件图标）。
    pub fn start_slot(mut self, element: impl IntoElement) -> Self {
        self.start_slot = Some(element.into_any_element());
        self
    }

    /// 结束槽位（关闭按钮 / 脏指示器）。
    pub fn end_slot(mut self, element: impl IntoElement) -> Self {
        self.end_slot = Some(element.into_any_element());
        self
    }
}

impl IntoElement for Tab {
    type Element = gpui::Component<Self>;
    fn into_element(self) -> Self::Element {
        gpui::Component::new(self)
    }
}

impl InteractiveElement for Tab {
    fn interactivity(&mut self) -> &mut gpui::Interactivity {
        self.div.interactivity()
    }
}

impl StatefulInteractiveElement for Tab {}

impl ParentElement for Tab {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements)
    }
}

impl RenderOnce for Tab {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let text_color = if self.selected {
            color::current().gray.s[8]
        } else {
            color::current().gray.s[6]
        };
        let bg = if self.selected {
            color::current().gray.s[1]
        } else {
            gpui::rgba(0)
        };

        let border_color = color::current().gray.s[4];

        self.div
            .flex()
            .flex_row()
            .items_center()
            .gap(space::S6)
            .p(space::S6)
            .rounded(radius::R2)
            .cursor_pointer()
            .text_color(text_color)
            .bg(bg)
            .border_color(border_color)
            .border_r_1()
            .when(self.selected, |this| this.pb(px(1.0)))
            .children(self.start_slot)
            .children(self.children)
            .children(self.end_slot)
    }
}
