use std::cell::Cell;

use gpui::{Context, TestAppContext, div};

use super::*;

fn filter_query(delegate: &mut BranchPickerDelegate, query: String) {
    delegate.query = query;
    delegate.do_filter();
}

fn confirm_delegate(delegate: BranchPickerDelegate, cx: &mut TestAppContext) {
    cx.set_global(zcv_settings::SettingsStore::new(
        zcv_settings::UserSettings::default(),
    ));
    cx.update(|cx| {
        zcv_editor::init(
            cx,
            std::sync::Arc::new(zcv_language::LanguageRegistry::new()),
        )
    });
    let (picker, visual) =
        cx.add_window_view(|window, cx| Picker::new(delegate, PICKER_WIDTH, window, cx));
    let input = visual.read_entity(&picker, |picker, _| picker.search_input().clone());
    visual.update(|window, cx| window.focus(&input.focus_handle(cx), cx));
    visual.dispatch_action(zcv_actions::PickerConfirm);
}

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
fn display_name_follows_branch_state() {
    // 有分支：显示分支名。
    assert_eq!(
        branch_display_name(Some("feature"), None).as_deref(),
        Some("feature")
    );

    // detached HEAD（无分支但有提交）：显示 8 位短 SHA。
    assert_eq!(
        branch_display_name(None, Some("0123456789abcdef")).as_deref(),
        Some("01234567")
    );

    // 空仓库（无分支无提交）：按钮没有可显示的分支名。
    assert_eq!(branch_display_name(None, None), None);
}

#[test]
fn empty_query_selects_current_branch() {
    let delegate = test_delegate();
    assert_eq!(delegate.selected_index, 1);
    assert_eq!(delegate.match_count(), 3);
    assert!(!delegate.create_row_visible());
}

#[gpui::test]
fn delete_shortcut_reports_selected_branch_to_workspace(cx: &mut TestAppContext) {
    let requested = Rc::new(Cell::new(None::<GitBranchAction>));
    let on_select: OnBranchSelected = {
        let requested = requested.clone();
        Rc::new(move |action, _, _| requested.set(Some(action)))
    };
    let mut delegate = BranchPickerDelegate::new(
        vec![
            Branch {
                name: "other".into(),
                is_head: false,
            },
            Branch {
                name: "current".into(),
                is_head: true,
            },
        ],
        on_select,
    );
    let window = cx.add_window(|_, _| TestView);

    filter_query(&mut delegate, "current".into());
    assert_eq!(delegate.filtered, vec![1]);
    window
        .update(cx, |_, window, cx| delegate.delete_selected(window, cx))
        .expect("测试窗口应可更新");
    assert!(matches!(
        requested.take(),
        Some(GitBranchAction::Delete(name)) if name == "current"
    ));

    filter_query(&mut delegate, String::new());
    delegate.set_selected_index(0);
    window
        .update(cx, |_, window, cx| delegate.delete_selected(window, cx))
        .expect("测试窗口应可更新");
    assert!(matches!(
        requested.take(),
        Some(GitBranchAction::Delete(name)) if name == "other"
    ));

    filter_query(&mut delegate, "new-branch".into());
    assert!(delegate.filtered.is_empty());
    window
        .update(cx, |_, window, cx| delegate.delete_selected(window, cx))
        .expect("测试窗口应可更新");
    assert!(requested.take().is_none(), "创建分支行不能触发删除");
}

#[test]
fn query_ranks_fuzzy_matches_and_keeps_create_action() {
    let mut delegate = test_delegate();
    filter_query(&mut delegate, "ftr".into());
    assert_eq!(delegate.filtered, vec![1]);
    assert!(delegate.create_row_visible());
    assert_eq!(delegate.match_count(), 2);
}

#[test]
fn exact_branch_name_hides_create_action() {
    let mut delegate = test_delegate();
    filter_query(&mut delegate, "feature".into());
    assert_eq!(delegate.filtered, vec![1]);
    assert!(!delegate.create_row_visible());
}

#[test]
fn no_match_shows_create_row() {
    let mut delegate = test_delegate();
    filter_query(&mut delegate, "zzz".into());
    assert!(delegate.create_row_visible());
    // 只显示"创建分支"虚拟行，且默认选中它。
    assert_eq!(delegate.match_count(), 1);
    assert_eq!(delegate.selected_index, 0);
}

#[gpui::test]
fn confirm_create_invokes_callback(cx: &mut TestAppContext) {
    let triggered = Rc::new(Cell::new(None::<GitBranchAction>));
    let on_select: OnBranchSelected = {
        let triggered = triggered.clone();
        Rc::new(move |action, _window, _cx| triggered.set(Some(action)))
    };
    let mut delegate = BranchPickerDelegate::new(Vec::new(), on_select);
    filter_query(&mut delegate, "new-feat".into());

    confirm_delegate(delegate, cx);
    assert!(matches!(
        triggered.take(),
        Some(GitBranchAction::Create(name)) if name == "new-feat"
    ));
}

#[gpui::test]
fn confirm_create_row_works_alongside_fuzzy_matches(cx: &mut TestAppContext) {
    let triggered = Rc::new(Cell::new(None::<GitBranchAction>));
    let on_select: OnBranchSelected = {
        let triggered = triggered.clone();
        Rc::new(move |action, _window, _cx| triggered.set(Some(action)))
    };
    let mut delegate = BranchPickerDelegate::new(
        vec![Branch {
            name: "feature".into(),
            is_head: true,
        }],
        on_select,
    );
    filter_query(&mut delegate, "ftr".into());
    delegate.selected_index = delegate.filtered.len();

    confirm_delegate(delegate, cx);
    assert!(matches!(
        triggered.take(),
        Some(GitBranchAction::Create(name)) if name == "ftr"
    ));
}

#[gpui::test]
fn confirm_checkout_invokes_callback(cx: &mut TestAppContext) {
    let triggered = Rc::new(Cell::new(None::<GitBranchAction>));
    let on_select: OnBranchSelected = {
        let triggered = triggered.clone();
        Rc::new(move |action, _window, _cx| triggered.set(Some(action)))
    };
    let delegate = BranchPickerDelegate::new(
        vec![Branch {
            name: "feature".into(),
            is_head: false,
        }],
        on_select,
    );

    confirm_delegate(delegate, cx);
    assert!(matches!(
        triggered.take(),
        Some(GitBranchAction::Checkout(name)) if name == "feature"
    ));
}

#[test]
fn reload_replaces_branches_and_keeps_query() {
    let mut delegate = test_delegate();
    filter_query(&mut delegate, "feat".into());
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
    filter_query(&mut delegate, "feat".into());
    delegate.reload(vec![Branch {
        name: "master".into(),
        is_head: true,
    }]);
    // query 保留且新列表无匹配：落到"创建分支"虚拟行。
    assert!(delegate.filtered.is_empty());
    assert!(delegate.create_row_visible());
    assert_eq!(delegate.selected_index, 0);
}
