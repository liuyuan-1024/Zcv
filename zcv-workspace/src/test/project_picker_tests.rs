use std::cell::Cell;

use gpui::{Context, div, prelude::*};

use super::*;

#[derive(Default)]
struct TestView;

impl Render for TestView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[gpui::test]
fn confirm_invokes_on_selected(cx: &mut gpui::TestAppContext) {
    let triggered = Rc::new(Cell::new(None::<String>));
    let on_selected: OnProjectSelected = {
        let triggered = triggered.clone();
        Rc::new(move |path, _window, _cx| triggered.set(Some(path)))
    };
    let mut delegate = ProjectPickerDelegate::new(
        vec![ProjectEntry {
            path: "/tmp/test-project".into(),
        }],
        on_selected,
        Rc::new(|_, _| {}),
        Rc::new(|_, _| {}),
    );
    let window = cx.add_window(|_window, _cx| TestView);
    let _ = window.update(cx, |_, window, cx| {
        delegate.confirm(window, cx);
    });
    assert_eq!(triggered.take().as_deref(), Some("/tmp/test-project"));
}

/// 构造 3 个项目的数据源，默认选中第一项。
fn test_delegate() -> ProjectPickerDelegate {
    let on_selected: OnProjectSelected = Rc::new(|_, _, _| {});
    ProjectPickerDelegate::new(
        vec![
            ProjectEntry {
                path: "/tmp/a".into(),
            },
            ProjectEntry {
                path: "/tmp/b".into(),
            },
            ProjectEntry {
                path: "/tmp/c".into(),
            },
        ],
        on_selected,
        Rc::new(|_, _| {}),
        Rc::new(|_, _| {}),
    )
}

#[test]
fn remove_project_drops_entry_and_keeps_filter() {
    let mut delegate = test_delegate();
    delegate.update_matches("tmp".into());
    delegate.remove_project_in_memory(2);
    assert_eq!(delegate.projects.len(), 2);
    assert!(delegate.projects.iter().all(|p| p.path != "/tmp/c"));
    assert_eq!(delegate.filtered, vec![0, 1]);
}

#[test]
fn remove_selected_project_selects_the_next_entry() {
    let mut delegate = test_delegate();
    delegate.selected_index = 1;
    delegate.remove_project_in_memory(1);
    assert_eq!(delegate.projects.len(), 2);
    assert_eq!(delegate.selected_index, 1);
    assert_eq!(delegate.projects[delegate.selected_index].label(), "c");
}

#[test]
fn remove_last_project_clamps_selection() {
    let mut delegate = test_delegate();
    delegate.selected_index = 1;
    delegate.remove_project_in_memory(2);
    assert_eq!(delegate.selected_index, 1);
    assert_eq!(delegate.projects[delegate.selected_index].label(), "b");
}
