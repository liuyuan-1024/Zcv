//! Tab —— 标签页 UI 组件。
//!
//! 纯视觉组件，不依赖业务类型。
//! 通过 builder 设置图标、关闭按钮、选中状态，调用方通过 [`InteractiveElement`] 方法挂载事件（点击、拖拽等）。

use gpui::{
    AnyElement, App, Div, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels,
    RenderOnce, Stateful, StatefulInteractiveElement, ViewElement, Window, div, prelude::*,
};
use zcv_theme::{color, scale, typography};

/// 标签页组件。
pub struct Tab {
    div: Stateful<Div>,
    selected: bool,
    italic: bool,
    start_slot: Option<AnyElement>,
    end_slot: Option<AnyElement>,
    children: Vec<AnyElement>,
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

    /// 标签容器高度：标签栏与各面板首行共用的显式高度基准。
    ///
    /// 由窗口 UI 行高与结构内边距派生，改字号时一起缩放；
    /// 行内图标/按钮不参与撑高，保证标签栏与面板首行的底部分隔线对齐。
    pub fn container_height(window: &Window, cx: &App) -> Pixels {
        let ui_line = typography::ui_line_at(window.rem_size(), cx);
        ui_line
            + scale::to_pixels(scale::S2, window) * 2.0
            + scale::to_pixels(scale::S6, window) * 2.0
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
    type Element = ViewElement<Self>;
    fn into_element(self) -> Self::Element {
        ViewElement::new(self)
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
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
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

        let border_color = color::current(cx).border;

        let tab = self
            .div
            .flex()
            .flex_row()
            .items_center()
            .gap(scale::S6)
            .p(scale::S6)
            .h(Self::container_height(window, cx))
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
