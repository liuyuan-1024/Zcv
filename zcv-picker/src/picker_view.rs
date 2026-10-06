//! Picker —— 通用搜索-选择器 Entity。
//!
//! - `Picker<D: PickerDelegate>` 是 gpui Entity，自管生命周期
//! - 内嵌统一 `Editor::single_line` 作为搜索框
//! - 搜索过滤、键盘导航、确认/取消均由 Picker 内部处理
//! - 调用方只需要实现 `PickerDelegate` 并提供数据

use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, FocusHandle, ListAlignment, ListOffset, ListSizingBehavior,
    ListState, Pixels, Render, SharedString, Window, div, list, prelude::*, px,
};
use zcv_actions::{
    MoveDown, MoveUp, PickerCancel, PickerConfirm, PickerSelectNext, PickerSelectPrev,
};
use zcv_theme::{color, fixed};
use zcv_ui::{EDITOR_FACTORY, ErasedEditor, ErasedEditorEvent, search_box};

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
    on_dismiss: Option<OnDismiss>,
    list_state: ListState,
    item_height_hint: Option<Pixels>,
    height_hint_pending: bool,
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
        let list_state = ListState::new(match_count, ListAlignment::Top, px(100.0));

        let picker = Self {
            delegate,
            search_input,
            focus_handle: focus,
            width,
            on_dismiss: None,
            list_state,
            item_height_hint: None,
            height_hint_pending: false,
        };
        picker.scroll_to_selection();
        {
            let weak = cx.weak_entity();
            picker
                .search_input
                .subscribe(
                    Box::new(move |ErasedEditorEvent::Edited, _, cx| {
                        weak.update(cx, |picker, cx| {
                            // 查询文本的权威是 search_input；这里只读回权威更新匹配。
                            let query = picker.search_input.text(cx);
                            picker.delegate.update_matches(query);
                            picker.matches_updated(cx);
                            picker.scroll_to_selection();
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

    /// 同步设置查询、重建匹配结果并定位选中项；打开浮层时也使用此入口。
    pub fn set_query(&mut self, query: &str, cx: &mut Context<Self>) {
        self.search_input.set_text(query, cx);
        // set_text 可能同步触发 Edited；也可能因文本未变而不触发。
        // 两种情况下都只从权威读回查询并更新匹配，不保留第二份可写副本。
        let query = self.search_input.text(cx);
        self.delegate.update_matches(query);
        self.matches_updated(cx);
        self.scroll_to_selection();
    }

    /// 数据源更新后重建行测量并保留滚动位置，包括行数不变的更新。
    pub fn matches_updated(&mut self, cx: &mut Context<Self>) {
        let offset = self.list_state.logical_scroll_top();
        if let Some(height) = self.item_height_hint {
            self.list_state
                .reset_with_uniform_height(self.delegate.match_count(), height);
        } else {
            self.list_state.reset(self.delegate.match_count());
        }
        self.list_state.scroll_to(offset);
        cx.notify();
    }

    fn scroll_to_selection(&self) {
        // 重建后的行尚未测量，直接使用逻辑行偏移，不能用缓存高度推算选中项的位置。
        self.list_state.scroll_to(ListOffset {
            item_ix: self.delegate.selected_index(),
            offset_in_item: Pixels::ZERO,
        });
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.delegate.match_count();
        if count > 0 && self.item_height_hint.is_none() && !self.height_hint_pending {
            self.height_hint_pending = true;
            // GPUI 首次确定列表宽度时会清除预填高度；
            // 首帧布局完成后再用实测行高估计离屏项。
            cx.on_next_frame(window, |picker, _, cx| {
                picker.height_hint_pending = false;
                let index = picker.list_state.logical_scroll_top().item_ix;
                if let Some(bounds) = picker.list_state.bounds_for_item(index) {
                    let height = bounds.size.height;
                    picker.list_state.clone().with_uniform_item_height(height);
                    picker.item_height_hint = Some(height);
                    cx.notify();
                }
            });
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

        // 列表项允许多行，使用可变行高虚拟列表；ListState 按需测量并缓存可见行高度。
        // 结果视口负责裁剪内容，ListState 只负责虚拟化和滚动状态。
        let entity = cx.entity();
        let width = self.width;
        let list = list(
            self.list_state.clone(),
            cx.processor(move |picker, index, _window, cx| {
                let entity = entity.clone();
                div()
                    .id(("picker-match", index))
                    // Infer 首次测量尚无视口宽度；换行必须使用与最终布局相同的选择器宽度。
                    .w(width)
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
        let results = div()
            .id("picker-results")
            .relative()
            .flex()
            .flex_col()
            .flex_grow(1.0)
            .min_h_0()
            .max_h(PICKER_MAX_HEIGHT)
            .overflow_hidden()
            .when_some(self.delegate.render_header(), |el, h| {
                el.child(div().flex_none().child(h))
            })
            .debug_selector(|| "picker-list".into())
            .child(list);

        // 基础容器（视觉外壳由父组件提供）
        let root = div()
            .track_focus(&self.focus_handle)
            .key_context("Picker")
            .w(self.width)
            .min_h_0()
            .max_h(PICKER_MAX_HEIGHT.min(window.viewport_size().height))
            .flex()
            .flex_col()
            .overflow_hidden()
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::editor_move_down))
            .on_action(cx.listener(Self::editor_move_up))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel));

        root.child(search_box(self.search_input.render(), cx))
            .when_some(no_match, |el, n| el.child(n))
            .child(results)
            .when_some(self.delegate.render_footer(window, cx), |el, f| {
                el.child(div().flex_none().child(f))
            })
    }
}

/// 分隔线。
pub fn picker_divider(cx: &App) -> impl IntoElement {
    div()
        .w_full()
        .h(fixed::HAIRLINE)
        .bg(color::current(cx).border)
}

#[cfg(test)]
#[path = "test/picker_view_tests.rs"]
mod tests;
