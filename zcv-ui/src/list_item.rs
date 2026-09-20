//! `ListItem` —— 通用列表项组件。
//!
//! hover、选中、间距样式。可用于 picker 列表、菜单列表等。

use gpui::{
    AnyElement, App, ElementId, IntoElement, RenderOnce, ViewElement, Window, div, prelude::*,
};
use zcv_theme::{color, space, typography};

/// 通用列表项。
pub struct ListItem {
    id: ElementId,
    toggle_state: bool,
    start_slot: Option<AnyElement>,
    child: Option<AnyElement>,
    subtitle: Option<AnyElement>,
    end_slot: Option<AnyElement>,
}

impl ListItem {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            toggle_state: false,
            start_slot: None,
            child: None,
            subtitle: None,
            end_slot: None,
        }
    }

    /// 首部插槽。
    pub fn start_slot(mut self, slot: impl IntoElement) -> Self {
        self.start_slot = Some(slot.into_any_element());
        self
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

    /// 次行内容（主内容下方，灰色小字）。
    pub fn subtitle(mut self, subtitle: impl IntoElement) -> Self {
        self.subtitle = Some(subtitle.into_any_element());
        self
    }

    /// 尾部插槽。
    pub fn end_slot(mut self, slot: impl IntoElement) -> Self {
        self.end_slot = Some(slot.into_any_element());
        self
    }
}

impl IntoElement for ListItem {
    type Element = ViewElement<Self>;

    fn into_element(self) -> Self::Element {
        ViewElement::new(self)
    }
}

impl RenderOnce for ListItem {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let ui_size = window.rem_size();
        let ui_line = typography::ui_line_at(ui_size, cx);
        // hover 闭包只有 style 参数，先取色再 move 进闭包
        let hover_bg = color::current(cx).element_hover;
        let mut row = div()
            .id(self.id)
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(space::S6)
            .p(space::S6)
            .cursor_pointer()
            // test cfg 下注册 debug bounds，供行高断言使用。
            .debug_selector(|| "list-item".into())
            .hover(move |style| style.bg(hover_bg));

        if self.toggle_state {
            row = row.bg(color::current(cx).element_selected);
        }

        // 首部插槽
        if let Some(slot) = self.start_slot {
            row = row.child(slot);
        }

        // 主内容（含次行时两行排列）。文本允许自动换行，行高由内容决定。
        if let Some(child) = self.child {
            let mut content = div().flex_1().min_w_0().child(child);
            // 次行主题色依赖 cx，只能在 render 中解析
            if let Some(subtitle) = self.subtitle {
                content = content.child(
                    div()
                        .text_color(color::current(cx).text_placeholder)
                        .text_size(ui_size)
                        .line_height(ui_line)
                        .child(subtitle),
                );
            }
            row = row.child(content);
        }

        // 尾部插槽
        if let Some(slot) = self.end_slot {
            row = row.child(slot);
        }

        row.into_any_element()
    }
}

#[cfg(test)]
#[path = "test/list_item_tests.rs"]
mod tests;
