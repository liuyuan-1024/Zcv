//! Picker —— 通用搜索-选择器 Entity。
//!
//! 参考 zed `crates/picker/src/picker.rs` 架构：
//! - `Picker<D: PickerDelegate>` 是 gpui Entity，自管生命周期
//! - 内嵌 `EditableText` 作为搜索框（后续替换为 Editor）
//! - 搜索过滤、键盘导航、确认/取消均由 Picker 内部处理
//! - 调用方只需要实现 `PickerDelegate` 并提供数据

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Pixels, Render, SharedString, Window, actions,
    div, prelude::*, px,
};

use crate::editor::editable::EditableText;
use crate::theme::{color, space};

actions!(
    picker,
    [
        PickerSelectNext,
        PickerSelectPrev,
        PickerConfirm,
        PickerCancel
    ]
);

// ═══ PickerDelegate ═════════════════════════════════════════════

/// Picker 数据源接口。
///
/// 调用方实现此 trait 提供数据、匹配逻辑和行渲染。
pub trait PickerDelegate: 'static {
    fn match_count(&self) -> usize;
    fn selected_index(&self) -> usize;
    fn set_selected_index(&mut self, ix: usize);
    fn update_matches(&mut self, query: String);
    fn confirm(&mut self, window: &mut Window, cx: &mut App);
    fn dismissed(&mut self);
    fn render_match(&self, ix: usize, selected: bool) -> AnyElement;

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

pub struct Picker<D: PickerDelegate> {
    delegate: D,
    editor: Entity<EditableText>,
    focus_handle: FocusHandle,
    width: Pixels,
    pending_query: Rc<RefCell<String>>,
    on_dismiss: Option<Box<dyn Fn(&mut Window, &mut App)>>,
}

impl<D: PickerDelegate> Picker<D> {
    pub fn new(delegate: D, width: Pixels, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let editor = cx.new(|cx| EditableText::new("picker-search", cx));
        Self {
            delegate,
            editor,
            focus_handle: focus,
            width,
            pending_query: Rc::new(RefCell::new(String::new())),
            on_dismiss: None,
        }
    }

    /// Entity 创建后调用，连接编辑器输入。
    pub fn init(&mut self, cx: &mut Context<Self>) {
        let pending = self.pending_query.clone();
        self.editor.update(cx, |editor, _cx| {
            editor.set_on_change(Box::new(
                move |text: &str, window: &mut Window, _app: &mut App| {
                    *pending.borrow_mut() = text.to_string();
                    window.refresh();
                },
            ));
        });
    }

    /// 设置关闭回调（由父 Entity 调用，例如关闭浮层）。
    pub fn set_on_dismiss(&mut self, f: Box<dyn Fn(&mut Window, &mut App)>) {
        self.on_dismiss = Some(f);
    }

    pub fn delegate(&self) -> &D {
        &self.delegate
    }

    pub fn delegate_mut(&mut self) -> &mut D {
        &mut self.delegate
    }

    pub fn editor(&self) -> &Entity<EditableText> {
        &self.editor
    }

    /// 处理挂起的查询。
    fn flush_pending_query(&mut self) {
        let query = self.pending_query.borrow_mut();
        if !query.is_empty() {
            let q = query.clone();
            drop(query);
            self.delegate.update_matches(q);
            *self.pending_query.borrow_mut() = String::new();
        }
    }

    // ══ 内部：action handler ════════════════════════════════════

    fn select_next(&mut self, _: &PickerSelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.delegate.match_count();
        if count == 0 {
            return;
        }
        let next = (self.delegate.selected_index() + 1) % count;
        self.delegate.set_selected_index(next);
        cx.notify();
    }

    fn select_prev(&mut self, _: &PickerSelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.delegate.match_count();
        if count == 0 {
            return;
        }
        let prev = (self.delegate.selected_index() + count - 1) % count;
        self.delegate.set_selected_index(prev);
        cx.notify();
    }

    fn confirm(&mut self, _: &PickerConfirm, window: &mut Window, cx: &mut Context<Self>) {
        self.delegate.confirm(window, cx);
        cx.notify();
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
        // 先消费挂起的查询
        self.flush_pending_query();

        let count = self.delegate.match_count();

        // 无匹配提示
        let no_match = (count == 0)
            .then(|| self.delegate.no_matches_text())
            .flatten()
            .map(|text| {
                div()
                    .text_center()
                    .text_color(color::current().gray.s[5])
                    .child(text)
            });

        // 列表项（delegate 通过 ListItem 返回完整行）
        let items = (0..count)
            .map(|i| {
                self.delegate
                    .render_match(i, i == self.delegate.selected_index())
            })
            .collect::<Vec<AnyElement>>();

        // 基础容器（视觉外壳由父组件提供）
        let root = div()
            .track_focus(&self.focus_handle)
            .key_context("Picker")
            .w(self.width)
            .overflow_hidden()
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel));

        root.child(picker_search_box(self.editor.clone()))
            .when_some(self.delegate.render_header(), |el, h| el.child(h))
            .when_some(no_match, |el, n| el.child(n))
            .children(items)
            .when_some(self.delegate.render_footer(_window, cx), |el, f| {
                el.child(f)
            })
    }
}

/// 搜索框容器：带回顶部边框和间距。
pub fn picker_search_box(content: impl IntoElement) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .flex_none()
        .items_center()
        .overflow_hidden()
        .px(space::S4)
        .h(px(27.0))
        .border_b_1()
        .border_color(color::current().gray.s[4])
        .child(content)
}

/// 分隔线。
pub fn picker_divider() -> impl IntoElement {
    div().w_full().h(px(1.0)).bg(color::current().gray.s[4])
}
