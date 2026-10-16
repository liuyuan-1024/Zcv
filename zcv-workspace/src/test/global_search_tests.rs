use super::*;
use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Bounds, Context, FocusHandle, Pixels, TestAppContext, canvas, point};
use zcv_language::LanguageRegistry;

struct TestView;

impl Render for TestView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

struct FocusHost {
    search: gpui::Entity<GlobalSearch>,
    previous: FocusHandle,
    other: FocusHandle,
}

impl Render for FocusHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(div().track_focus(&self.previous))
            .child(div().track_focus(&self.other))
            .child(self.search.clone())
    }
}

#[gpui::test]
fn escape_restores_the_focus_before_global_search(cx: &mut TestAppContext) {
    let languages = Arc::new(LanguageRegistry::new());
    cx.update(|cx| zcv_editor::init(cx, languages.clone()));
    let (host, cx) = cx.add_window_view(|window, cx| {
        let workspace = cx.new(|cx| Workspace::new_empty(languages, window, cx));
        let project = workspace.read(cx).project().clone();
        let search = cx.new(|cx| GlobalSearch::new(project, workspace.downgrade(), window, cx));
        FocusHost {
            search,
            previous: cx.focus_handle(),
            other: cx.focus_handle(),
        }
    });
    let (search, previous, other) = cx.read_entity(&host, |host, _| {
        (
            host.search.clone(),
            host.previous.clone(),
            host.other.clone(),
        )
    });

    cx.update(|window, cx| window.focus(&previous, cx));
    search.update_in(cx, |search, window, cx| search.focus(window, cx));
    let input_focus = cx.read_entity(&search, |search, cx| search.input.focus_handle(cx));
    cx.update(|window, _| assert!(input_focus.is_focused(window)));

    search.update_in(cx, |search, window, cx| search.focus(window, cx));
    search.update_in(cx, |search, window, cx| {
        search.cancel(&PickerCancel, window, cx)
    });
    cx.update(|window, _| assert!(previous.is_focused(window)));

    search.update_in(cx, |search, window, cx| search.focus(window, cx));
    cx.update(|window, cx| window.focus(&other, cx));
    search.update_in(cx, |search, window, cx| {
        search.cancel(&PickerCancel, window, cx)
    });
    cx.update(|window, _| assert!(other.is_focused(window)));
}

#[gpui::test]
fn clicking_global_search_records_the_focus_before_the_input_takes_it(cx: &mut TestAppContext) {
    let languages = Arc::new(LanguageRegistry::new());
    cx.update(|cx| zcv_editor::init(cx, languages.clone()));
    let (host, cx) = cx.add_window_view(|window, cx| {
        let workspace = cx.new(|cx| Workspace::new_empty(languages, window, cx));
        let project = workspace.read(cx).project().clone();
        FocusHost {
            search: cx.new(|cx| GlobalSearch::new(project, workspace.downgrade(), window, cx)),
            previous: cx.focus_handle(),
            other: cx.focus_handle(),
        }
    });
    let (search, previous) = cx.read_entity(&host, |host, _| {
        (host.search.clone(), host.previous.clone())
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.focus(&previous, cx));
    cx.simulate_click(point(px(10.0), px(10.0)), gpui::Modifiers::default());
    cx.run_until_parked();
    let input_focus = cx.read_entity(&search, |search, cx| search.input.focus_handle(cx));
    cx.update(|window, _| assert!(input_focus.is_focused(window)));
    search.update_in(cx, |search, window, cx| {
        search.cancel(&PickerCancel, window, cx)
    });
    cx.update(|window, _| assert!(previous.is_focused(window)));
}

struct ResultsLayoutView {
    input_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    results_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
}

impl Render for ResultsLayoutView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let input_bounds = self.input_bounds.clone();
        let results_bounds = self.results_bounds.clone();
        div()
            .relative()
            .w(px(420.0))
            .h(search_height(window, cx))
            .child(
                canvas(
                    move |bounds, _, _| *input_bounds.borrow_mut() = Some(bounds),
                    |_, _, _, _| {},
                )
                .absolute()
                .top(Pixels::ZERO)
                .w_full()
                .h_full(),
            )
            .child(positioned_results(
                canvas(
                    move |bounds, _, _| *results_bounds.borrow_mut() = Some(bounds),
                    |_, _, _, _| {},
                )
                .w_full()
                .h(px(100.0)),
                window,
                cx,
            ))
    }
}

fn files() -> Vec<FileCandidate> {
    let root = Path::new("/project");
    [
        "/project/src/global_search.rs",
        "/project/test/search.rs",
        "/project/src/search.rs",
        "/project/src/main.rs",
    ]
    .into_iter()
    .map(|path| FileCandidate::new(PathBuf::from(path), root))
    .collect()
}

#[test]
fn matches_file_name_and_project_relative_path() {
    let files = files();
    assert_eq!(ranked_matches(&files, "SEARCH.RS"), vec![2, 1, 0]);
    assert_eq!(ranked_matches(&files, "test/search"), vec![1]);
    assert_eq!(ranked_matches(&files, "glbsrch"), vec![0]);
    assert_eq!(ranked_matches(&files, "srchrs"), vec![2, 1, 0]);
    assert!(ranked_matches(&files, "not-found").is_empty());
}

#[test]
fn empty_query_does_not_show_arbitrary_project_files() {
    assert!(ranked_matches(&files(), " ").is_empty());
}

#[gpui::test]
fn results_start_at_the_top_bar_input_bottom(cx: &mut TestAppContext) {
    let input_bounds = Rc::new(RefCell::new(None));
    let results_bounds = Rc::new(RefCell::new(None));
    let input = input_bounds.clone();
    let results = results_bounds.clone();
    let _window = cx.add_window(move |_, _| ResultsLayoutView {
        input_bounds: input,
        results_bounds: results,
    });
    cx.run_until_parked();

    let input = input_bounds.borrow().expect("搜索框必须完成布局");
    let results = results_bounds.borrow().expect("搜索结果必须完成布局");
    assert_eq!(results.left(), input.left());
    assert_eq!(
        results.top(),
        input.bottom(),
        "搜索结果应紧贴顶栏输入框下边缘"
    );
}

#[gpui::test]
fn confirming_a_match_requests_open_for_its_absolute_path(cx: &mut TestAppContext) {
    let opened = Rc::new(RefCell::new(None));
    let on_open: OnFileOpen = {
        let opened = opened.clone();
        Box::new(move |path, _, _| *opened.borrow_mut() = Some(path))
    };
    let mut search = FileSearchState::new(on_open);
    search.set_files(
        vec![PathBuf::from("/project/src/main.rs")],
        Path::new("/project"),
    );
    search.update_matches("main".into());

    let window = cx.add_window(|_, _| TestView);
    window
        .update(cx, |_, window, cx| search.confirm(window, cx))
        .expect("测试窗口应可更新");
    assert_eq!(
        opened.borrow().as_deref(),
        Some(Path::new("/project/src/main.rs"))
    );

    search.set_files(Vec::new(), Path::new("/project"));
    assert_eq!(search.match_count(), 0, "文件移除后不能保留旧候选");
}
