use gpui::{Context, Entity, Render, TestAppContext, Window, div, prelude::*};

use super::common::test_buffer;
use super::*;

struct TwoEditors {
    focused: Entity<Editor>,
    unfocused: Entity<Editor>,
}

impl Render for TwoEditors {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .child(self.focused.clone())
            .child(self.unfocused.clone())
    }
}

#[gpui::test]
fn focused_editor_stops_blinking_when_window_deactivates(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "光标测试");
    let (editor, cx) = cx.add_window_view(move |window, cx| {
        let editor = Editor::for_buffer(buffer, cx);
        window.focus(&editor.focus_handle());
        editor
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();

    cx.update(|window, cx| {
        assert!(window.is_window_active());
        assert!(editor.read(cx).focus.is_focused(window));
        assert!(editor.read(cx).blink_manager.read(cx).enabled());
    });

    cx.deactivate_window();
    cx.run_until_parked();
    cx.update(|window, cx| {
        assert!(!window.is_window_active());
        assert!(editor.read(cx).focus.is_focused(window));
        assert!(!editor.read(cx).blink_manager.read(cx).enabled());
        assert!(!editor.read(cx).show_local_cursors(window, cx));
    });

    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    cx.update(|window, cx| {
        assert!(window.is_window_active());
        assert!(editor.read(cx).blink_manager.read(cx).enabled());
    });
}

#[gpui::test]
fn window_reactivation_only_resumes_the_focused_editor(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "双编辑器光标测试");
    let (view, cx) = cx.add_window_view(move |window, cx| {
        let focused = cx.new(|cx| Editor::for_buffer(buffer.clone(), cx));
        let unfocused = cx.new(|cx| Editor::for_buffer(buffer, cx));
        window.focus(&focused.read(cx).focus_handle());
        TwoEditors { focused, unfocused }
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();

    let editors = cx.read_entity(&view, |view, _| {
        (view.focused.clone(), view.unfocused.clone())
    });
    cx.update(|_window, cx| {
        assert!(editors.0.read(cx).blink_manager.read(cx).enabled());
        assert!(!editors.1.read(cx).blink_manager.read(cx).enabled());
    });

    cx.deactivate_window();
    cx.run_until_parked();
    cx.update(|_window, cx| {
        assert!(!editors.0.read(cx).blink_manager.read(cx).enabled());
        assert!(!editors.1.read(cx).blink_manager.read(cx).enabled());
    });

    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    cx.update(|_window, cx| {
        assert!(editors.0.read(cx).blink_manager.read(cx).enabled());
        assert!(!editors.1.read(cx).blink_manager.read(cx).enabled());
    });
}
