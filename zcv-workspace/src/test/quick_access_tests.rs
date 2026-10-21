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
    search: gpui::Entity<QuickAccess>,
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
fn escape_restores_the_focus_before_quick_access(cx: &mut TestAppContext) {
    let languages = Arc::new(LanguageRegistry::new());
    cx.update(|cx| zcv_editor::init(cx, languages.clone()));
    let (host, cx) = cx.add_window_view(|window, cx| {
        let workspace = cx.new(|cx| Workspace::new_empty(languages, window, cx));
        let project = workspace.read(cx).project().clone();
        let search = cx.new(|cx| {
            QuickAccess::new(
                project,
                workspace.downgrade(),
                PickerLayer::default(),
                window,
                cx,
            )
        });
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
fn clicking_quick_access_records_the_focus_before_the_input_takes_it(cx: &mut TestAppContext) {
    let languages = Arc::new(LanguageRegistry::new());
    cx.update(|cx| zcv_editor::init(cx, languages.clone()));
    let (host, cx) = cx.add_window_view(|window, cx| {
        let workspace = cx.new(|cx| Workspace::new_empty(languages, window, cx));
        let project = workspace.read(cx).project().clone();
        FocusHost {
            search: cx.new(|cx| {
                QuickAccess::new(
                    project,
                    workspace.downgrade(),
                    PickerLayer::default(),
                    window,
                    cx,
                )
            }),
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

fn files() -> Vec<Arc<FileCandidate>> {
    let root = Path::new("/project");
    [
        "/project/src/quick_access.rs",
        "/project/test/search.rs",
        "/project/src/search.rs",
        "/project/src/main.rs",
    ]
    .into_iter()
    .map(|path| Arc::new(FileCandidate::new(PathBuf::from(path), root)))
    .collect()
}

#[test]
fn matches_file_name_and_project_relative_path() {
    let files = files();
    let search = |query| ranked_results(&files, &[], query, false);
    assert_eq!(
        search("SEARCH.RS"),
        vec![
            SearchResult::File(files[2].clone()),
            SearchResult::File(files[1].clone())
        ]
    );
    assert_eq!(
        search("test/search"),
        vec![SearchResult::File(files[1].clone())]
    );
    assert_eq!(
        search("qckaccs"),
        vec![SearchResult::File(files[0].clone())]
    );
    assert_eq!(
        search("srchrs"),
        vec![
            SearchResult::File(files[2].clone()),
            SearchResult::File(files[1].clone())
        ]
    );
    assert!(search("not-found").is_empty());
}

#[test]
fn empty_query_does_not_show_arbitrary_project_files() {
    assert!(ranked_results(&files(), &[], " ", true).is_empty());
}

#[test]
fn prefixes_only_filter_the_default_mixed_results() {
    assert_eq!(input_mode("  "), InputMode::Empty);
    assert_eq!(input_mode("src/main"), InputMode::Mixed("src/main"));
    assert_eq!(input_mode("> save"), InputMode::Commands("save"));
    assert_eq!(input_mode(":42:8"), InputMode::LineColumn("42:8"));
    let files = [Arc::new(FileCandidate::new(
        PathBuf::from("/project/save.rs"),
        Path::new("/project"),
    ))];
    let commands = [CommandCandidate {
        action_name: "workspace::Save",
        title: "保存",
        description: "保存当前文档",
    }];
    let mixed = ranked_results(&files, &commands, "save", false);
    assert!(mixed.contains(&SearchResult::File(files[0].clone())));
    assert!(mixed.contains(&SearchResult::Command(0)));
    assert_eq!(
        ranked_results(&files, &commands, ">save", false),
        vec![SearchResult::Command(0)]
    );
    assert_eq!(
        ranked_results(&files, &commands, "保存", false),
        vec![SearchResult::Command(0)]
    );
    assert_eq!(
        ranked_results(&files, &commands, ":42", true),
        vec![SearchResult::LineColumn {
            line: 41,
            column: 0
        }]
    );
}

#[test]
fn line_column_requires_positive_unambiguous_numbers() {
    assert_eq!(parse_line_column("42"), Some((41, 0)));
    assert_eq!(parse_line_column("42:"), Some((41, 0)));
    assert_eq!(parse_line_column("42:8"), Some((41, 7)));
    for input in ["", "0", "1:0", "1:2:3", "abc", "1:abc"] {
        assert_eq!(parse_line_column(input), None, "{input}");
    }
}

#[test]
fn numeric_query_offers_navigation_without_hiding_other_results() {
    let names = [
        CommandCandidate {
            action_name: "workspace::Save",
            title: "保存",
            description: "保存当前文档",
        },
        CommandCandidate {
            action_name: "editor::SelectAll",
            title: "全选",
            description: "选中当前文档的全部内容",
        },
    ];
    let files = [Arc::new(FileCandidate::new(
        PathBuf::from("/project/42.rs"),
        Path::new("/project"),
    ))];
    assert!(ranked_results(&files, &names, "", true).is_empty());
    assert_eq!(
        ranked_results(&files, &names, "save", true),
        vec![SearchResult::Command(0)]
    );
    let results = ranked_results(&files, &names, "42", true);
    assert_eq!(
        results.first(),
        Some(&SearchResult::LineColumn {
            line: 41,
            column: 0
        })
    );
    assert!(results.contains(&SearchResult::File(files[0].clone())));
    assert_eq!(
        ranked_results(&files, &names, "42:", true).first(),
        Some(&SearchResult::LineColumn {
            line: 41,
            column: 0
        })
    );
    assert_eq!(
        ranked_results(&files, &names, "42", false),
        vec![SearchResult::File(files[0].clone())]
    );
}

#[gpui::test]
fn command_candidates_come_from_described_actions(cx: &mut TestAppContext) {
    let languages = Arc::new(LanguageRegistry::new());
    cx.update(|cx| zcv_editor::init(cx, languages.clone()));
    let (search, _) = cx.add_window_view(|window, cx| {
        let workspace = cx.new(|cx| Workspace::new_empty(languages, window, cx));
        QuickAccess::new(
            workspace.read(cx).project().clone(),
            workspace.downgrade(),
            PickerLayer::default(),
            window,
            cx,
        )
    });
    cx.read_entity(&search, |search, _| {
        assert!(
            search
                .commands
                .iter()
                .any(|command| command.action_name == "workspace::Save"
                    && command.title == "保存"
                    && command.description == "保存当前文档")
        );
        assert!(
            !search
                .commands
                .iter()
                .any(|command| command.action_name == "picker::PickerConfirm")
        );
        assert!(
            !search
                .commands
                .iter()
                .any(|command| command.action_name == "editor::RenameLocal")
        );
    });
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
    assert_eq!(
        ranked_results(&search.files, &[], "main", false),
        vec![SearchResult::File(search.files[0].clone())]
    );

    let window = cx.add_window(|_, _| TestView);
    window
        .update(cx, |_, window, cx| {
            search.confirm(&search.files[0], window, cx)
        })
        .expect("测试窗口应可更新");
    assert_eq!(
        opened.borrow().as_deref(),
        Some(Path::new("/project/src/main.rs"))
    );

    search.set_files(Vec::new(), Path::new("/project"));
    assert!(
        ranked_results(&search.files, &[], "main", false).is_empty(),
        "文件移除后不能保留旧候选"
    );
}

#[test]
fn ranked_file_result_survives_file_cache_replacement() {
    let mut state = FileSearchState::new(Box::new(|_, _, _| {}));
    state.set_files(
        vec![PathBuf::from("/project/src/main.rs")],
        Path::new("/project"),
    );
    let results = ranked_results(&state.files, &[], "main", false);
    state.begin_scan();

    let SearchResult::File(file) = &results[0] else {
        panic!("文件搜索结果应保留文件候选");
    };
    assert_eq!(file.path, Path::new("/project/src/main.rs"));
    assert_eq!(file.name, "main.rs");
    assert_eq!(file.relative, "src/main.rs");
}
