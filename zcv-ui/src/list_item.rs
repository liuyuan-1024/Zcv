//! `ListItem` —— 通用列表项组件。
//!
//! 列表行的选中样式和内容布局。

use gpui::{
    AnyElement, App, CursorStyle, ElementId, IntoElement, RenderOnce, ViewElement, Window, div,
    prelude::*,
};
use zcv_theme::{color, scale, typography};

/// 行内光标：禁用行不给出可点击提示，其余行提示可点击。
fn cursor_for_state(disabled: bool) -> Option<CursorStyle> {
    (!disabled).then_some(CursorStyle::PointingHand)
}

/// 通用列表项。
pub struct ListItem {
    id: ElementId,
    disabled: bool,
    selected: bool,
    rounded: bool,
    start_slot: Option<AnyElement>,
    child: Option<AnyElement>,
    subtitle: Option<AnyElement>,
    end_slot: Option<AnyElement>,
}

impl ListItem {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            disabled: false,
            selected: false,
            rounded: false,
            start_slot: None,
            child: None,
            subtitle: None,
            end_slot: None,
        }
    }

    /// 禁用时使用默认光标，内容置灰由调用方负责。
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// 行的高亮背景使用小圆角。
    pub(crate) fn rounded(mut self) -> Self {
        self.rounded = true;
        self
    }

    /// 首部插槽。
    pub fn start_slot(mut self, slot: impl IntoElement) -> Self {
        self.start_slot = Some(slot.into_any_element());
        self
    }

    /// 由所在列表决定当前项，并据此绘制行高亮。
    pub fn toggle_state(mut self, selected: bool) -> Self {
        self.selected = selected;
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
        let disabled = self.disabled;
        let mut row = div()
            .id(self.id)
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(scale::S6)
            .p(scale::S6)
            .when(self.rounded, |row| row.rounded_sm())
            // test cfg 下注册 debug bounds，供行高断言使用。
            .debug_selector(|| "list-item".into());
        if let Some(cursor) = cursor_for_state(disabled) {
            row = row.cursor(cursor);
        }
        if self.selected {
            row = row.bg(color::current(cx).element_highlight);
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
