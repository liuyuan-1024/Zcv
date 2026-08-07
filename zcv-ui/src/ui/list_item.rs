//! `ListItem` —— 通用列表项组件。
//!
//! 对标 zed `ui/src/components/list_item.rs`，统一 hover、选中、间距样式。
//! 可用于 picker 列表、菜单列表等。

use gpui::{
    AnyElement, App, Component, ElementId, IntoElement, RenderOnce, Window, div, prelude::*,
};

use zcv_theme::{color, space, typography};

/// 通用列表项。
pub struct ListItem {
    id: ElementId,
    toggle_state: bool,
    child: Option<AnyElement>,
    end_slot: Option<AnyElement>,
}

impl ListItem {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            toggle_state: false,
            child: None,
            end_slot: None,
        }
    }

    /// 选中态（高亮背景）。
    pub fn toggle_state(mut self, selected: bool) -> Self {
        self.toggle_state = selected;
        self
    }

    /// 主内容。
    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.child = Some(child.into_any_element());
        self
    }

    /// 尾部插槽。
    pub fn end_slot(mut self, slot: impl IntoElement) -> Self {
        self.end_slot = Some(slot.into_any_element());
        self
    }
}

impl IntoElement for ListItem {
    type Element = Component<Self>;

    fn into_element(self) -> Self::Element {
        Component::new(self)
    }
}

impl RenderOnce for ListItem {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        // hover 闭包只有 style 参数，先取色再 move 进闭包
        let hover_bg = color::current(cx).element_hover;
        let mut row = div()
            .id(self.id)
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .p(space::S6)
            .cursor_pointer()
            .hover(move |style| style.bg(hover_bg));

        if self.toggle_state {
            row = row.bg(color::current(cx).element_selected);
        }

        // 主内容
        if let Some(child) = self.child {
            row = row.child(div().flex_1().min_w_0().child(child));
        }

        // 尾部插槽
        if let Some(slot) = self.end_slot {
            row = row.child(slot);
        }

        row.into_any_element()
    }
}

/// 标准两行标签：主标题 + 灰色副标题。
pub fn list_item_two_line(title: impl IntoElement, subtitle: impl IntoElement) -> impl IntoElement {
    // 主题色依赖 cx，通过 Component 延迟到 render 解析
    Component::new(TwoLine {
        title: title.into_any_element(),
        subtitle: subtitle.into_any_element(),
    })
}

/// 两行标签的渲染载体。
struct TwoLine {
    title: AnyElement,
    subtitle: AnyElement,
}

impl RenderOnce for TwoLine {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        div().flex_1().min_w_0().child(self.title).child(
            div()
                .text_color(color::current(cx).text_placeholder)
                .text_size(typography::ui())
                .line_height(typography::ui())
                .child(self.subtitle),
        )
    }
}
