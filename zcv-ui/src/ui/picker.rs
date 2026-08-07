//! Picker —— 通用搜索-选择器 Entity。
//!
//! 参考 zed `crates/picker/src/picker.rs` 架构：
//! - `Picker<D: PickerDelegate>` 是 gpui Entity，自管生命周期
//! - 内嵌统一 `Editor::single_line` 作为搜索框
//! - 搜索过滤、键盘导航、确认/取消均由 Picker 内部处理
//! - 调用方只需要实现 `PickerDelegate` 并提供数据

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, MouseButton, Pixels, Render, SharedString,
    Window, actions, div, prelude::*, px,
};

use zcv_editor::{Editor, MoveDown, MoveUp};
use zcv_theme::{color, space};

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
    fn render_match(&self, ix: usize, selected: bool, cx: &App) -> AnyElement;

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
    editor: Entity<Editor>,
    focus_handle: FocusHandle,
    width: Pixels,
    query: String,
    on_dismiss: Option<OnDismiss>,
}

impl<D: PickerDelegate> Picker<D> {
    pub fn new(delegate: D, width: Pixels, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let placeholder = delegate.placeholder_text().to_owned();
        let editor = cx.new(|cx| {
            let mut editor = Editor::single_line(cx);
            editor.set_placeholder_text(placeholder, cx);
            editor
        });
        Self {
            delegate,
            editor,
            focus_handle: focus,
            width,
            query: String::new(),
            on_dismiss: None,
        }
    }

    /// Entity 创建后调用，连接编辑器输入。
    pub fn init(&mut self, cx: &mut Context<Self>) {
        cx.observe(&self.editor, |picker, editor, cx| {
            let query = editor.read(cx).text(cx);
            if picker.query != query {
                picker.query = query.clone();
                picker.delegate.update_matches(query);
                cx.notify();
            }
        })
        .detach();
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

    pub fn editor(&self) -> &Entity<Editor> {
        &self.editor
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

        // 列表项（delegate 通过 ListItem 返回完整行）
        let picker = cx.entity();
        let items = (0..count)
            .map(|index| {
                let picker = picker.clone();
                div()
                    .id(("picker-match", index))
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        picker.update(cx, |picker, cx| {
                            picker.delegate.set_selected_index(index);
                            picker.confirm_selection(window, cx);
                        });
                        cx.stop_propagation();
                    })
                    .child(self.delegate.render_match(
                        index,
                        index == self.delegate.selected_index(),
                        cx,
                    ))
                    .into_any_element()
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
            .on_action(cx.listener(Self::editor_move_down))
            .on_action(cx.listener(Self::editor_move_up))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel));

        root.child(picker_search_box(self.editor.clone(), cx))
            .when_some(self.delegate.render_header(), |el, h| el.child(h))
            .when_some(no_match, |el, n| el.child(n))
            .children(items)
            .when_some(self.delegate.render_footer(_window, cx), |el, f| {
                el.child(f)
            })
    }
}

/// 搜索框容器：带回顶部边框和间距。
pub fn picker_search_box(content: impl IntoElement, cx: &App) -> impl IntoElement {
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
        .h(px(1.0))
        .bg(color::current(cx).border_variant)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{AppContext, KeyBinding, TestAppContext};

    use super::*;
    use zcv_editor::Newline;

    struct TestDelegate {
        query: String,
    }

    impl PickerDelegate for TestDelegate {
        fn match_count(&self) -> usize {
            0
        }

        fn selected_index(&self) -> usize {
            0
        }

        fn set_selected_index(&mut self, _: usize) {}

        fn update_matches(&mut self, query: String) {
            self.query = query;
        }

        fn confirm(&mut self, _: &mut Window, _: &mut App) {}

        fn dismissed(&mut self) {}

        fn render_match(&self, _: usize, _: bool, _: &App) -> AnyElement {
            div().into_any_element()
        }
    }

    #[gpui::test]
    fn single_line_editor_drives_picker_query(cx: &mut TestAppContext) {
        let picker = cx.new(|cx| {
            let mut picker = Picker::new(
                TestDelegate {
                    query: String::new(),
                },
                px(300.0),
                cx,
            );
            picker.init(cx);
            picker
        });
        let editor = cx.read_entity(&picker, |picker, _| picker.editor().clone());

        cx.update_entity(&editor, |editor, cx| editor.set_text("zed\nstyle", cx));
        cx.run_until_parked();

        cx.read_entity(&picker, |picker, _| {
            assert_eq!(picker.query, "zedstyle");
            assert_eq!(picker.delegate().query, "zedstyle");
        });
    }

    struct ConfirmDelegate {
        confirmed: Rc<Cell<bool>>,
        selected_index: Rc<Cell<usize>>,
    }

    impl PickerDelegate for ConfirmDelegate {
        fn match_count(&self) -> usize {
            2
        }

        fn selected_index(&self) -> usize {
            self.selected_index.get()
        }

        fn set_selected_index(&mut self, index: usize) {
            self.selected_index.set(index);
        }

        fn update_matches(&mut self, _: String) {}

        fn confirm(&mut self, _: &mut Window, _: &mut App) {
            self.confirmed.set(true);
        }

        fn dismissed(&mut self) {}

        fn render_match(&self, _: usize, _: bool, _: &App) -> AnyElement {
            div().child("项目").into_any_element()
        }
    }

    #[gpui::test]
    fn navigation_and_confirm_work_while_search_editor_is_focused(cx: &mut TestAppContext) {
        let confirmed = Rc::new(Cell::new(false));
        let selected_index = Rc::new(Cell::new(0));
        let (picker, cx) = cx.add_window_view({
            let confirmed = confirmed.clone();
            let selected_index = selected_index.clone();
            move |_, cx| {
                cx.bind_keys([
                    KeyBinding::new("down", zcv_editor::MoveDown, Some("Editor")),
                    KeyBinding::new("down", PickerSelectNext, Some("Picker")),
                    KeyBinding::new("enter", Newline, Some("Editor")),
                    KeyBinding::new("enter", PickerConfirm, Some("Picker")),
                ]);
                Picker::new(
                    ConfirmDelegate {
                        confirmed,
                        selected_index,
                    },
                    px(300.0),
                    cx,
                )
            }
        });
        let editor = cx.read_entity(&picker, |picker, _| picker.editor().clone());
        cx.update(|window, cx| window.focus(&editor.read(cx).focus_handle()));

        cx.simulate_keystrokes("down");
        assert_eq!(selected_index.get(), 1);

        cx.simulate_keystrokes("enter");
        assert!(confirmed.get());
    }
}
