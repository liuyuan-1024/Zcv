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

/// 构造三分支的数据源，第 2 个是当前分支。
fn test_delegate() -> BranchPickerDelegate {
    let on_select: OnBranchSelected = Rc::new(|_, _, _| {});
    BranchPickerDelegate::new(
        vec![
            Branch {
                name: "master".into(),
                is_head: false,
            },
            Branch {
                name: "feature".into(),
                is_head: true,
            },
            Branch {
                name: "hotfix".into(),
                is_head: false,
            },
        ],
        on_select,
    )
}

#[test]
fn display_name_and_visibility_follow_branch_state() {
    // 有分支：显示分支名。
    assert_eq!(
        branch_display_name(Some("feature"), None).as_deref(),
        Some("feature")
    );
    assert!(has_branch_context(Some("feature"), None));

    // detached HEAD（无分支但有提交）：显示 8 位短 SHA。
    assert_eq!(
        branch_display_name(None, Some("0123456789abcdef")).as_deref(),
        Some("01234567")
    );
    assert!(has_branch_context(None, Some("0123456789abcdef")));

    // 空仓库（无分支无提交）：不显示分支选择器。
    assert_eq!(branch_display_name(None, None), None);
    assert!(!has_branch_context(None, None));
}

#[test]
fn empty_query_selects_current_branch() {
    let delegate = test_delegate();
    assert_eq!(delegate.selected_index, 1);
    assert_eq!(delegate.match_count(), 3);
    assert!(!delegate.create_row_visible());
}

#[test]
fn query_filters_by_substring() {
    let mut delegate = test_delegate();
    delegate.update_matches("feat".into());
    assert_eq!(delegate.filtered, vec![1]);
    assert_eq!(delegate.match_count(), 1);
}

#[test]
fn no_match_shows_create_row() {
    let mut delegate = test_delegate();
    delegate.update_matches("zzz".into());
    assert!(delegate.create_row_visible());
    // 只显示"创建分支"虚拟行，且默认选中它。
    assert_eq!(delegate.match_count(), 1);
    assert_eq!(delegate.selected_index, 0);
}

#[gpui::test]
fn confirm_create_invokes_callback(cx: &mut gpui::TestAppContext) {
    let triggered = Rc::new(Cell::new(None::<GitBranchAction>));
    let on_select: OnBranchSelected = {
        let triggered = triggered.clone();
        Rc::new(move |action, _window, _cx| triggered.set(Some(action)))
    };
    let mut delegate = BranchPickerDelegate::new(Vec::new(), on_select);
    delegate.update_matches("new-feat".into());

    let window = cx.add_window(|_window, _cx| TestView);
    let _ = window.update(cx, |_, window, cx| {
        delegate.confirm(window, cx);
    });
    assert!(matches!(
        triggered.take(),
        Some(GitBranchAction::Create(name)) if name == "new-feat"
    ));
}

#[gpui::test]
fn confirm_checkout_invokes_callback(cx: &mut gpui::TestAppContext) {
    let triggered = Rc::new(Cell::new(None::<GitBranchAction>));
    let on_select: OnBranchSelected = {
        let triggered = triggered.clone();
        Rc::new(move |action, _window, _cx| triggered.set(Some(action)))
    };
    let mut delegate = BranchPickerDelegate::new(
        vec![Branch {
            name: "feature".into(),
            is_head: false,
        }],
        on_select,
    );

    let window = cx.add_window(|_window, _cx| TestView);
    let _ = window.update(cx, |_, window, cx| {
        delegate.confirm(window, cx);
    });
    assert!(matches!(
        triggered.take(),
        Some(GitBranchAction::Checkout(name)) if name == "feature"
    ));
}

#[test]
fn reload_replaces_branches_and_keeps_query() {
    let mut delegate = test_delegate();
    delegate.update_matches("feat".into());
    delegate.reload(vec![
        Branch {
            name: "feat-x".into(),
            is_head: true,
        },
        Branch {
            name: "other".into(),
            is_head: false,
        },
    ]);
    // query 保留：新列表按原 query 重过滤，命中项成为选中行。
    assert_eq!(delegate.filtered, vec![0]);
    assert_eq!(delegate.selected_index, 0);
}

#[test]
fn reload_without_match_shows_create_row() {
    let mut delegate = test_delegate();
    delegate.update_matches("feat".into());
    delegate.reload(vec![Branch {
        name: "master".into(),
        is_head: true,
    }]);
    // query 保留且新列表无匹配：落到"创建分支"虚拟行。
    assert!(delegate.filtered.is_empty());
    assert!(delegate.create_row_visible());
    assert_eq!(delegate.selected_index, 0);
}
