use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AppContext, Entity, FocusHandle, KeyBinding, TestAppContext, actions, anchored, deferred,
    point, px, size,
};

use super::*;
use zcv_actions::Newline;
use zcv_ui::ListItem;

fn init(cx: &mut TestAppContext) {
    let languages = std::sync::Arc::new(zcv_editor::LanguageRegistry::new());
    cx.update(move |cx| zcv_editor::init(cx, languages));
}

/// 行内含超长文本（换行后行高很大），用于验证列表高度受容器约束。
struct TallDelegate {
    selected: Rc<Cell<usize>>,
}

impl PickerDelegate for TallDelegate {
    fn match_count(&self) -> usize {
        2
    }

    fn selected_index(&self) -> usize {
        self.selected.get()
    }

    fn set_selected_index(&mut self, ix: usize) {
        self.selected.set(ix);
    }

    fn update_matches(&mut self, _: String) {}

    fn confirm(&mut self, _: &mut Window, _: &mut App) {}

    fn dismissed(&mut self) {}

    fn render_match(
        &self,
        index: usize,
        selected: bool,
        _cx: &mut Context<Picker<Self>>,
    ) -> AnyElement {
        ListItem::new(index)
            .toggle_state(selected)
            .child("项目")
            .subtitle("超长路径文本，用于验证换行后列表高度受限于容器：".repeat(30))
            .into_any_element()
    }

    fn render_footer(&self, _window: &mut Window, _cx: &mut App) -> Option<AnyElement> {
        Some(
            div()
                .debug_selector(|| "picker-footer".into())
                .h(px(30.0))
                .child("打开本地项目")
                .into_any_element(),
        )
    }
}

/// 模拟项目选择器的浮层形态：deferred + anchored + auto 高度容器，与 project_picker.rs 的真实浮层结构一致。
struct PopoverWrapper {
    picker: Entity<Picker<TallDelegate>>,
}

impl Render for PopoverWrapper {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let popover = div().occlude().child(
            div()
                .bg(gpui::rgba(0xFF000000))
                .overflow_hidden()
                .child(self.picker.clone()),
        );
        deferred(
            anchored()
                .anchor(gpui::Anchor::TopLeft)
                .position(point(Pixels::ZERO, Pixels::ZERO))
                .position_mode(gpui::AnchoredPositionMode::Local)
                .child(popover),
        )
        .with_priority(1)
        .into_any_element()
    }
}

/// 换行后列表内容高于容器时，列表必须收缩在可用空间内滚动，footer 保持可见且不被列表遮挡。
#[gpui::test]
fn footer_stays_visible_with_tall_rows(cx: &mut TestAppContext) {
    init(cx);
    let (_, cx) = cx.add_window_view(|window, cx| {
        let picker = cx.new(|cx| {
            Picker::new(
                TallDelegate {
                    selected: Rc::new(Cell::new(0)),
                },
                px(300.0),
                window,
                cx,
            )
        });
        PopoverWrapper { picker }
    });
    // 窗口高度 ≥ 浮层 max_h，模拟真实屏幕
    cx.simulate_window_resize(cx.windows()[0], size(px(500.0), px(500.0)));

    let list = cx.debug_bounds("picker-list").expect("列表容器应参与布局");
    assert!(
        list.size.height > px(0.0),
        "自适应浮层中的列表高度应大于 0，实际 {list:?}"
    );
    cx.debug_bounds("picker-match-0")
        .expect("自适应浮层中的第一条列表项应被渲染");
    let footer = cx.debug_bounds("picker-footer").expect("footer 应参与布局");
    assert!(
        footer.origin.y >= list.bottom(),
        "footer 被列表遮挡：列表 {list:?}，footer {footer:?}"
    );
    assert!(
        footer.bottom() <= PICKER_MAX_HEIGHT,
        "footer 超出浮层 {footer:?}"
    );
}

actions!(picker_tests, [EditorDelete, PickerDelete]);

/// 模拟 ProjectPicker 根节点：提供 "ProjectPicker" key context，并挂两个测试 action 的 handler，用于观察哪个 action 被 keymap 匹配派发。
struct PickerWithContext {
    focus: FocusHandle,
    picker: Entity<Picker<ConfirmDelegate>>,
    editor_fired: Rc<Cell<bool>>,
    picker_fired: Rc<Cell<bool>>,
}

impl PickerWithContext {
    fn new(
        editor_fired: Rc<Cell<bool>>,
        picker_fired: Rc<Cell<bool>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus: cx.focus_handle(),
            picker: cx.new(|cx| {
                Picker::new(
                    ConfirmDelegate {
                        confirmed: Rc::new(Cell::new(false)),
                        selected_index: Rc::new(Cell::new(0)),
                    },
                    px(300.0),
                    window,
                    cx,
                )
            }),
            editor_fired,
            picker_fired,
        }
    }
}

impl Render for PickerWithContext {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus)
            .key_context("ProjectPicker")
            .on_action(cx.listener(|this, _: &EditorDelete, _, _| {
                this.editor_fired.set(true);
            }))
            .on_action(cx.listener(|this, _: &PickerDelete, _, _| {
                this.picker_fired.set(true);
            }))
            .child(self.picker.clone())
    }
}

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

    fn render_match(&self, _: usize, _: bool, _: &mut Context<Picker<Self>>) -> AnyElement {
        div().into_any_element()
    }
}

#[gpui::test]
fn search_input_drives_picker_query(cx: &mut TestAppContext) {
    init(cx);
    let (picker, cx) = cx.add_window_view(|window, cx| {
        Picker::new(
            TestDelegate {
                query: String::new(),
            },
            px(300.0),
            window,
            cx,
        )
    });
    let input = cx.read_entity(&picker, |picker, _| picker.search_input().clone());

    cx.update(|_, cx| input.set_text("分支", cx));
    cx.run_until_parked();

    cx.read_entity(&picker, |picker, _| {
        assert_eq!(picker.query, "分支");
        assert_eq!(picker.delegate().query, "分支");
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

    fn render_match(&self, _: usize, _: bool, _: &mut Context<Picker<Self>>) -> AnyElement {
        div().child("项目").into_any_element()
    }
}

#[gpui::test]
fn navigation_and_confirm_work_while_search_editor_is_focused(cx: &mut TestAppContext) {
    init(cx);
    let confirmed = Rc::new(Cell::new(false));
    let selected_index = Rc::new(Cell::new(0));
    let (picker, cx) = cx.add_window_view({
        let confirmed = confirmed.clone();
        let selected_index = selected_index.clone();
        move |window, cx| {
            cx.bind_keys([
                KeyBinding::new("down", MoveDown, Some("Editor")),
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
                window,
                cx,
            )
        }
    });
    let input = cx.read_entity(&picker, |picker, _| picker.search_input().clone());
    cx.update(|window, cx| {
        let focus = input.focus_handle(cx);
        window.focus(&focus, cx);
    });

    cx.simulate_keystrokes("down");
    assert_eq!(selected_index.get(), 1);

    cx.simulate_keystrokes("enter");
    assert!(confirmed.get());
}

/// 虚拟化列表必须获得确定高度，行才能被渲染出来。
#[gpui::test]
fn picker_list_has_visible_height(cx: &mut TestAppContext) {
    init(cx);
    let (view, cx) = cx.add_window_view(|window, cx| {
        Picker::new(
            ConfirmDelegate {
                confirmed: Rc::new(Cell::new(false)),
                selected_index: Rc::new(Cell::new(0)),
            },
            px(300.0),
            window,
            cx,
        )
    });
    let _ = view;
    let list_bounds = cx.debug_bounds("picker-list").expect("列表容器应参与布局");
    assert!(
        list_bounds.size.height > px(0.0),
        "列表高度应大于 0，实际 {list_bounds:?}"
    );
}

/// 焦点在搜索框（Editor context）时，同一按键在 Editor 的绑定
/// 优先于普通 "Picker" context 绑定 —— 这是最近项目删除快捷键
/// 被 DeleteToBeginningOfLine 抢占的原因。
#[gpui::test]
fn editor_binding_wins_over_plain_picker_context(cx: &mut TestAppContext) {
    init(cx);
    let editor_fired = Rc::new(Cell::new(false));
    let picker_fired = Rc::new(Cell::new(false));
    let (view, cx) = cx.add_window_view({
        let editor_fired = editor_fired.clone();
        let picker_fired = picker_fired.clone();
        move |window, cx| {
            cx.bind_keys([
                KeyBinding::new("cmd-backspace", EditorDelete, Some("Editor")),
                KeyBinding::new("cmd-backspace", PickerDelete, Some("Picker")),
            ]);
            PickerWithContext::new(editor_fired, picker_fired, window, cx)
        }
    });
    let input = cx.read_entity(&view, |view, cx| {
        view.picker.read(cx).search_input().clone()
    });
    cx.update(|window, cx| {
        let focus = input.focus_handle(cx);
        window.focus(&focus, cx);
    });

    cx.simulate_keystrokes("cmd-backspace");

    assert!(editor_fired.get());
    assert!(!picker_fired.get());
}

/// 复合 context（ProjectPicker > Picker > Editor）与 Editor 绑定同深度，
/// 后注册优先 —— 项目选择器打开时删除快捷键覆盖 Editor 的 cmd-backspace。
#[gpui::test]
fn composite_context_binding_wins_over_editor(cx: &mut TestAppContext) {
    init(cx);
    let editor_fired = Rc::new(Cell::new(false));
    let picker_fired = Rc::new(Cell::new(false));
    let (view, cx) = cx.add_window_view({
        let editor_fired = editor_fired.clone();
        let picker_fired = picker_fired.clone();
        move |window, cx| {
            cx.bind_keys([
                KeyBinding::new("cmd-backspace", EditorDelete, Some("Editor")),
                KeyBinding::new(
                    "cmd-backspace",
                    PickerDelete,
                    Some("Picker || (ProjectPicker > Picker > Editor)"),
                ),
            ]);
            PickerWithContext::new(editor_fired, picker_fired, window, cx)
        }
    });
    let input = cx.read_entity(&view, |view, cx| {
        view.picker.read(cx).search_input().clone()
    });
    cx.update(|window, cx| {
        let focus = input.focus_handle(cx);
        window.focus(&focus, cx);
    });

    cx.simulate_keystrokes("cmd-backspace");

    assert!(
        picker_fired.get(),
        "复合 context 应优先于 Editor 的 cmd-backspace 绑定"
    );
    assert!(!editor_fired.get());
}
