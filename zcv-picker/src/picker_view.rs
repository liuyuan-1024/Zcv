//! Picker —— 通用搜索-选择器 Entity。
//!
//! - `Picker<D: PickerDelegate>` 是 gpui Entity，自管生命周期
//! - 内嵌统一 `Editor::single_line` 作为搜索框
//! - 搜索过滤、键盘导航、确认/取消均由 Picker 内部处理
//! - 调用方只需要实现 `PickerDelegate` 并提供数据

use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, FocusHandle, ListAlignment, ListSizingBehavior, ListState, Pixels,
    Render, SharedString, Window, div, list, prelude::*, px,
};
use zcv_actions::{
    MoveDown, MoveUp, PickerCancel, PickerConfirm, PickerSelectNext, PickerSelectPrev,
};
use zcv_theme::{color, space};
use zcv_ui::{EDITOR_FACTORY, ErasedEditor, ErasedEditorEvent};

use super::PICKER_MAX_HEIGHT;

// ═══ PickerDelegate ═════════════════════════════════════════════

/// Picker 数据源接口。
///
/// 调用方实现此 trait 提供数据、匹配逻辑和行渲染。
/// `Sized` 约束来自 `render_match` 中的 `Context<Picker<Self>>`，行内交互通过它绑定到 Picker 访问 delegate。
pub trait PickerDelegate: 'static + Sized {
    fn match_count(&self) -> usize;
    fn selected_index(&self) -> usize;
    fn set_selected_index(&mut self, ix: usize);
    fn update_matches(&mut self, query: String);
    fn confirm(&mut self, window: &mut Window, cx: &mut App);
    fn dismissed(&mut self);

    /// 渲染第 `ix` 行。
    ///
    /// `cx` 是 Picker 自身的 context：行内需要回调 delegate 的交互（例如删除按钮）通过 `cx.listener` 绑定到 Picker 再访问 delegate。
    fn render_match(&self, ix: usize, selected: bool, cx: &mut Context<Picker<Self>>)
    -> AnyElement;

    fn placeholder_text(&self) -> &str {
        "搜索..."
    }

    fn no_matches_text(&self) -> Option<SharedString> {
        Some("无匹配".into())
    }

    fn render_header(&self) -> Option<AnyElement> {
        None
    }

    fn render_footer(&self, _window: &mut Window, _cx: &mut App) -> Option<AnyElement> {
        None
    }
}

// ═══ Picker Entity ══════════════════════════════════════════════

/// 浮层关闭回调。
pub(crate) type OnDismiss = Box<dyn Fn(&mut Window, &mut App)>;

pub struct Picker<D: PickerDelegate> {
    delegate: D,
    /// 单行输入控件；由 zcv_editor::init 注入的 EDITOR_FACTORY 创建。
    search_input: Arc<dyn ErasedEditor>,
    focus_handle: FocusHandle,
    width: Pixels,
    query: String,
    on_dismiss: Option<OnDismiss>,
    list_state: ListState,
}

impl<D: PickerDelegate> Picker<D> {
    pub fn new(delegate: D, width: Pixels, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let placeholder = delegate.placeholder_text().to_owned();
        let match_count = delegate.match_count();
        // 编辑器工厂是搜索框的前提；缺失说明 zcv_editor::init 先于窗口/选择器装配被跳过，必须显式失败。
        let factory = EDITOR_FACTORY
            .get()
            .expect("Picker 需要 zcv_editor::init 注入编辑器工厂");
        let search_input = factory(cx);
        search_input.set_placeholder_text(&placeholder, cx);

        let picker = Self {
            delegate,
            search_input,
            focus_handle: focus,
            width,
            query: String::new(),
            on_dismiss: None,
            list_state: ListState::new(match_count, ListAlignment::Top, px(100.0)),
        };
        {
            let weak = cx.weak_entity();
            picker
                .search_input
                .subscribe(
                    Box::new(move |ErasedEditorEvent::Edited, _, cx| {
                        weak.update(cx, |picker, cx| {
                            let query = picker.search_input.text(cx);
                            if picker.query != query {
                                picker.query = query.clone();
                                picker.delegate.update_matches(query);
                                picker.list_state.reset(picker.delegate.match_count());
                                cx.notify();
                            }
                        })
                        .ok();
                    }),
                    window,
                    cx,
                )
                .detach();
        }
        picker
    }

    /// 设置关闭回调（由父 Entity 调用，例如关闭浮层）。
    pub fn set_on_dismiss(&mut self, f: OnDismiss) {
        self.on_dismiss = Some(f);
    }

    pub fn delegate(&self) -> &D {
        &self.delegate
    }

    pub fn delegate_mut(&mut self) -> &mut D {
        &mut self.delegate
    }

    pub fn search_input(&self) -> &Arc<dyn ErasedEditor> {
        &self.search_input
    }

    // ══ 内部：action handler ════════════════════════════════════

    fn select_next(&mut self, _: &PickerSelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.delegate.match_count();
        if count == 0 {
            return;
        }
        let next = (self.delegate.selected_index() + 1) % count;
        self.delegate.set_selected_index(next);
        self.list_state.scroll_to_reveal_item(next);
        cx.notify();
    }

    fn select_prev(&mut self, _: &PickerSelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.delegate.match_count();
        if count == 0 {
            return;
        }
        let prev = (self.delegate.selected_index() + count - 1) % count;
        self.delegate.set_selected_index(prev);
        self.list_state.scroll_to_reveal_item(prev);
        cx.notify();
    }

    fn editor_move_down(&mut self, _: &MoveDown, window: &mut Window, cx: &mut Context<Self>) {
        self.select_next(&PickerSelectNext, window, cx);
    }

    fn editor_move_up(&mut self, _: &MoveUp, window: &mut Window, cx: &mut Context<Self>) {
        self.select_prev(&PickerSelectPrev, window, cx);
    }

    fn confirm_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.delegate.confirm(window, cx);
        if let Some(ref on_dismiss) = self.on_dismiss {
            on_dismiss(window, cx);
        }
        cx.notify();
    }

    fn confirm(&mut self, _: &PickerConfirm, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_selection(window, cx);
    }

    fn cancel(&mut self, _: &PickerCancel, window: &mut Window, cx: &mut Context<Self>) {
        self.delegate.dismissed();
        if let Some(ref on_dismiss) = self.on_dismiss {
            on_dismiss(window, cx);
        }
    }
}

impl<D: PickerDelegate> Render for Picker<D> {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.delegate.match_count();
        if self.list_state.item_count() != count {
            self.list_state.reset(count);
        }

        // 无匹配提示
        let no_match = (count == 0)
            .then(|| self.delegate.no_matches_text())
            .flatten()
            .map(|text| {
                div()
                    .text_center()
                    .text_color(color::current(cx).text_placeholder)
                    .child(text)
            });

        // 列表项允许多行，使用可变行高虚拟列表；ListState 按需测量并缓存每行高度。
        // flex_grow 吸收剩余空间；min_h(0) 允许收缩——否则 flex item 的 min-height:auto 会把 footer 挤出可视区。
        let entity = cx.entity();
        let list = list(
            self.list_state.clone(),
            cx.processor(move |picker, index, _window, cx| {
                let entity = entity.clone();
                div()
                    .id(("picker-match", index))
                    .debug_selector(move || format!("picker-match-{index}"))
                    .on_click(move |_, window, cx| {
                        entity.update(cx, |picker, cx| {
                            picker.delegate.set_selected_index(index);
                            picker.confirm_selection(window, cx);
                        });
                        cx.stop_propagation();
                    })
                    .child(picker.delegate.render_match(
                        index,
                        index == picker.delegate.selected_index(),
                        cx,
                    ))
                    .into_any_element()
            }),
        )
        .with_sizing_behavior(ListSizingBehavior::Infer)
        .flex_grow(1.0)
        .min_h_0();
        let items = div()
            .id("picker-items")
            .flex_grow(1.0)
            .min_h_0()
            // test cfg 下注册 debug bounds，供布局断言使用。
            .debug_selector(|| "picker-list".into())
            .child(list);

        // 基础容器（视觉外壳由父组件提供）
        let root = div()
            .track_focus(&self.focus_handle)
            .key_context("Picker")
            .w(self.width)
            .max_h(PICKER_MAX_HEIGHT)
            .flex()
            .flex_col()
            .overflow_hidden()
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::editor_move_down))
            .on_action(cx.listener(Self::editor_move_up))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel));

        root.child(picker_search_box(self.search_input.render(), cx))
            .when_some(self.delegate.render_header(), |el, h| el.child(h))
            .when_some(no_match, |el, n| el.child(n))
            .child(items)
            .when_some(self.delegate.render_footer(_window, cx), |el, f| {
                el.child(f)
            })
    }
}

/// 搜索框容器：带回顶部边框和间距。
fn picker_search_box(content: impl IntoElement, cx: &App) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .flex_none()
        .items_center()
        .overflow_hidden()
        .p(space::S6)
        .border_b_1()
        .border_color(color::current(cx).border_variant)
        .child(content)
}

/// 分隔线。
pub fn picker_divider(cx: &App) -> impl IntoElement {
    div()
        .w_full()
        .h(space::S1)
        .bg(color::current(cx).border_variant)
}

#[cfg(test)]
#[path = "test/picker_view_tests.rs"]
mod tests;
