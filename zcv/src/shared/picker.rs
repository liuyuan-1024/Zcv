//! Picker —— 通用搜索-选择器。
//!
//! `render_picker` 只负责三个区域的纵向堆叠，不介入各区域内部样式。
//! 搜索框、条目列表、footer 各自独立实现自己的样式。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{AnyElement, App, Pixels, Window, div, prelude::*, px};

use crate::theme::{color, radius, space, typography};

/// Picker 数据源 trait。
pub(crate) trait PickerDelegate {
    fn placeholder(&self) -> &str;
    fn query(&self) -> &str;
    fn set_query(&mut self, query: &str);
    fn item_count(&self) -> usize;
    fn selected_index(&self) -> usize;
    fn move_selection(&mut self, delta: isize);
    fn confirm(&self, window: &mut Window, cx: &mut App);
    fn render_item(&self, index: usize, is_selected: bool) -> AnyElement;
    fn render_footer(&self) -> Option<AnyElement> {
        None
    }
}

/// 渲染完整的 Picker Surface 内容。
///
/// 三个区域各自维护自己的样式：
/// - `search_box`：调用方用 `picker_search_box()` 包裹
/// - 条目列表：Picker 内部用 `picker_row()` 渲染
/// - footer：调用方在 `render_footer()` 中用 `picker_footer()` 包裹
pub(crate) fn render_picker<D: PickerDelegate + 'static>(
    width: Pixels,
    delegate: Rc<RefCell<D>>,
    search_box: impl IntoElement,
) -> AnyElement {
    let (count, selected, footer) = {
        let d = delegate.borrow();
        (d.item_count(), d.selected_index(), d.render_footer())
    };

    let items: Vec<AnyElement> = {
        let d = delegate.borrow();
        (0..count)
            .map(|i| d.render_item(i, i == selected))
            .collect()
    };

    div()
        .w(width)
        .rounded(radius::R4)
        .border_1()
        .border_color(color::current().gray.s[4])
        .bg(color::current().gray.s[2])
        .overflow_hidden()
        .on_key_down({
            let delegate = Rc::clone(&delegate);
            move |event, window, cx| match event.keystroke.key.as_str() {
                "up" => {
                    delegate.borrow_mut().move_selection(-1);
                    window.refresh();
                }
                "down" => {
                    delegate.borrow_mut().move_selection(1);
                    window.refresh();
                }
                "enter" => {
                    let d = delegate.borrow();
                    if d.item_count() > 0 {
                        d.confirm(window, cx);
                    }
                }
                _ => {}
            }
        })
        .on_mouse_down(gpui::MouseButton::Left, move |_, _, cx| {
            cx.stop_propagation()
        })
        // 三个区域各自独立
        .child(search_box)
        .child(picker_list_view(items))
        .when_some(footer, |container, f| container.child(f))
        .into_any_element()
}

// ═══ 区域容器辅助 ═════════════════════════════════════════

/// 搜索框容器：带底部边框和间距。
pub(crate) fn picker_search_box(content: impl IntoElement) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .items_center()
        .px(space::S6)
        .h(px(32.0))
        .border_b_1()
        .border_color(color::current().gray.s[4])
        .child(content)
}

/// 列表视图容器：给项目列表提供垂直间距。
pub(crate) fn picker_list_view(items: Vec<AnyElement>) -> impl IntoElement {
    div().flex().flex_col().py(space::S6).children(items)
}

/// Footer 容器：带回顶部边框线和间距。
pub(crate) fn picker_footer(items: Vec<AnyElement>) -> impl IntoElement {
    div()
        .child(picker_divider())
        .child(div().pt(space::S2).pb(space::S2).children(items))
}

// ═══ 标准行渲染辅助 ═══════════════════════════════════════

/// 标准 Picker 行容器，含 hover / 选中边框 / 间距 / 圆角。
/// 调用方只需嵌入行内内容。
pub(crate) fn picker_row(content: impl IntoElement, is_selected: bool) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .px(space::S6)
        .py(space::S2)
        .rounded(radius::R2)
        .border_1()
        .border_color(if is_selected {
            color::current().blue.s[6]
        } else {
            gpui::rgba(0)
        })
        .cursor_pointer()
        .hover(|style| style.bg(color::current().gray.s[3]))
        .child(content)
        .into_any_element()
}

/// 标准 Picker footer 行（与列表项同款样式，无边框）。
pub(crate) fn picker_footer_row(content: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .px(space::S6)
        .py(space::S2)
        .cursor_pointer()
        .hover(|style| style.bg(color::current().gray.s[3]))
        .text_color(color::current().gray.s[8])
        .child(content)
        .into_any_element()
}

/// 标准两行标签：主标题 + 灰色副标题（如路径、描述）。
pub(crate) fn picker_two_line(
    title: impl IntoElement,
    subtitle: impl IntoElement,
) -> impl IntoElement {
    div().flex_1().child(title).child(
        div()
            .text_color(color::current().gray.s[5])
            .text_size(typography::ui())
            .child(subtitle),
    )
}

/// 分隔线。
pub(crate) fn picker_divider() -> impl IntoElement {
    div().w_full().h(px(1.0)).bg(color::current().gray.s[4])
}
