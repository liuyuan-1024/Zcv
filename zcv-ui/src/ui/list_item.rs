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
    subtitle: Option<AnyElement>,
    end_slot: Option<AnyElement>,
}

impl ListItem {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            toggle_state: false,
            child: None,
            subtitle: None,
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
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .p(space::S6)
            .cursor_pointer()
            // test cfg 下注册 debug bounds，供行高断言使用。
            .debug_selector(|| "list-item".into())
            .hover(move |style| style.bg(hover_bg));

        if self.toggle_state {
            row = row.bg(color::current(cx).element_selected);
        }

        // 主内容（含次行时两行排列）。
        // 文本允许自动换行，行高由内容决定；配合变高列表（picker 的 list 容器）可完整展示长文本。
        if let Some(child) = self.child {
            let mut content = div().flex_1().min_w_0().child(child);
            // 次行主题色依赖 cx，只能在 render 中解析
            if let Some(subtitle) = self.subtitle {
                content = content.child(
                    div()
                        .text_color(color::current(cx).text_placeholder)
                        .text_size(typography::ui())
                        .line_height(typography::ui())
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
mod tests {
    use gpui::{Context, Render, TestAppContext, Window, prelude::*, px, size};

    use super::*;

    struct ShortRow;
    impl Render for ShortRow {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            ListItem::new("short").child("标题").subtitle("短路径")
        }
    }

    struct LongRow;
    impl Render for LongRow {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            // 文本远超任何测试窗口宽度，必然换行
            ListItem::new("long")
                .child("标题")
                .subtitle("这是一个非常长的路径，用来验证次行文本自动换行不会被截断：".repeat(500))
        }
    }

    /// 次行文本允许自动换行：行高随内容增长（变高列表按实际高度布局），
    /// 长路径完整展示而不被裁剪。
    #[gpui::test]
    fn long_subtitle_grows_row_height(cx: &mut TestAppContext) {
        // 窗口调窄，保证长路径必然换行
        let (_, cx) = cx.add_window_view(|_, _| ShortRow);
        cx.simulate_window_resize(cx.windows()[0], size(px(360.0), px(400.0)));
        let short_height = cx
            .debug_bounds("list-item")
            .expect("短路径行应参与布局")
            .size
            .height;

        let (_, cx) = cx.add_window_view(|_, _| LongRow);
        cx.simulate_window_resize(cx.windows()[1], size(px(360.0), px(400.0)));
        let long_height = cx
            .debug_bounds("list-item")
            .expect("长路径行应参与布局")
            .size
            .height;

        assert!(
            long_height > short_height,
            "长路径应换行撑高行（不被截断）：短行 {short_height}，长行 {long_height}"
        );
    }
}
