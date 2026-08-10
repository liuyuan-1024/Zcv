//! Tab —— 标签页 UI 组件。
//!
//! 纯视觉组件，不依赖业务类型。
//! 通过 builder 设置图标、关闭按钮、选中状态，调用方通过 [`InteractiveElement`] 方法挂载事件（点击、拖拽等）。

use gpui::{
    AnyElement, App, Div, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    Stateful, StatefulInteractiveElement, Window, div, prelude::*,
};

use zcv_theme::{color, space};

/// 标签页组件。
pub struct Tab {
    pub div: Stateful<Div>,
    pub selected: bool,
    pub italic: bool,
    pub start_slot: Option<AnyElement>,
    pub end_slot: Option<AnyElement>,
    pub children: Vec<AnyElement>,
}

impl Tab {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            div: div().id(id),
            selected: false,
            italic: false,
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

    /// 设置标签正文是否使用斜体；起止槽位中的图标不受影响。
    pub fn italic(mut self, italic: bool) -> Self {
        self.italic = italic;
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
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let text_color = if self.selected {
            color::current(cx).text
        } else {
            color::current(cx).text_disabled
        };
        let bg = if self.selected {
            color::current(cx).tab_active_background
        } else {
            gpui::rgba(0)
        };

        let border_color = color::current(cx).border_variant;

        let tab = self
            .div
            .flex()
            .flex_row()
            .items_center()
            .gap(space::S6)
            .p(space::S6)
            .cursor_pointer()
            .text_color(text_color)
            .bg(bg)
            .border_color(border_color)
            .border_r_1()
            .children(self.start_slot);
        let tab = if self.italic {
            tab.child(
                div()
                    .text_color(text_color)
                    .italic()
                    .children(self.children),
            )
        } else {
            // 普通标签保持原来的直接子元素结构，避免新增容器改变文字颜色继承。
            tab.children(self.children)
        };
        tab.children(self.end_slot)
    }
}
