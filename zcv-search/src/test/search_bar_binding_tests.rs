use std::{cell::Cell, rc::Rc};

use gpui::{AppContext, Empty};
use zcv_ui::MatchOptions;

use super::*;
use crate::{SearchBar, SearchBarBinding, SearchBarConfig, SearchBarPresentation};

fn search_bar(query: &str, cx: &mut App) -> Entity<SearchBar> {
    let bar = cx.new(|cx| {
        SearchBar::new(
            SearchBarConfig {
                id_prefix: "binding-test",
                key_context: "SearchBarBindingTest",
                query_placeholder: "搜索",
                replace_placeholder: "替换",
                presentation: SearchBarPresentation::Persistent,
            },
            Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    bar.update(cx, |bar, cx| {
        bar.restore(query, MatchOptions::default(), cx)
    });
    bar
}

fn search_target(cx: &mut App) -> Entity<TestItem> {
    cx.new(|cx| TestItem {
        focus: cx.focus_handle(),
        path: None,
        exposes_search: true,
        last_query: None,
    })
}

#[gpui::test]
fn switching_search_sessions_unbinds_previous_and_preserves_queries(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    cx.add_window_view(|window, cx| {
        let first = search_bar("first", cx);
        let second = search_bar("second", cx);
        let first_target = search_target(cx);
        let second_target = search_target(cx);
        let mut binding = SearchBarBinding::default();
        binding.set_active(
            Some((first.clone(), Box::new(first_target.downgrade()))),
            window,
            cx,
        );
        assert_eq!(first_target.read(cx).last_query.as_deref(), Some("first"));

        binding.set_active(
            Some((second.clone(), Box::new(second_target.downgrade()))),
            window,
            cx,
        );
        assert!(!first.read(cx).has_target());
        assert_eq!(second_target.read(cx).last_query.as_deref(), Some("second"));
        first.update(cx, |bar, cx| {
            bar.restore("inactive", MatchOptions::default(), cx);
            bar.toggle_case_sensitive(window, cx);
        });
        assert_eq!(
            first_target.read(cx).last_query.as_deref(),
            Some("first"),
            "非活动会话不得继续搜索旧目标"
        );

        binding.set_active(
            Some((first.clone(), Box::new(first_target.downgrade()))),
            window,
            cx,
        );
        assert_eq!(
            first_target.read(cx).last_query.as_deref(),
            Some("inactive")
        );
        assert!(first.read(cx).options().case_sensitive);
        assert_eq!(second.read(cx).query_text(cx), "second");
        assert!(!second.read(cx).has_target());
        binding.set_active(None, window, cx);
        assert!(!first.read(cx).has_target());
        assert_eq!(first.read(cx).query_text(cx), "inactive");
        Empty
    });
}

struct BindingHost {
    binding: SearchBarBinding,
}

impl Render for BindingHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[gpui::test]
fn rebinding_search_target_stops_previous_events_and_keeps_current_events(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let first = cx.update(search_target);
    let second = cx.update(search_target);
    let bar = cx.update(|cx| search_bar("needle", cx));
    let (host, visual) = cx.add_window_view(|window, cx| {
        let mut binding = SearchBarBinding::default();
        binding.set_active(Some((bar.clone(), Box::new(first.downgrade()))), window, cx);
        BindingHost { binding }
    });
    let notifications = Rc::new(Cell::new(0));
    let observed = notifications.clone();
    let _subscription =
        visual.update(|_, cx| cx.observe(&bar, move |_, _| observed.set(observed.get() + 1)));
    visual.run_until_parked();

    host.update_in(visual, |host, window, cx| {
        host.binding.set_active(
            Some((bar.clone(), Box::new(second.downgrade()))),
            window,
            cx,
        );
        host.binding.set_active(
            Some((bar.clone(), Box::new(second.downgrade()))),
            window,
            cx,
        );
    });
    visual.run_until_parked();
    notifications.set(0);
    first.update(&mut *visual, |_, cx| {
        cx.emit(SearchEvent::MatchesInvalidated)
    });
    visual.run_until_parked();
    assert_eq!(notifications.get(), 0, "旧目标事件不得刷新当前搜索栏");
    second.update(&mut *visual, |_, cx| {
        cx.emit(SearchEvent::MatchesInvalidated)
    });
    visual.run_until_parked();
    assert!(notifications.get() > 0, "重复绑定后仍应消费当前目标事件");

    host.update_in(visual, |host, window, cx| {
        host.binding.set_active(None, window, cx)
    });
    visual.run_until_parked();
    notifications.set(0);
    second.update(&mut *visual, |_, cx| {
        cx.emit(SearchEvent::MatchesInvalidated)
    });
    visual.run_until_parked();
    assert_eq!(notifications.get(), 0, "隐藏工具栏后应结束目标订阅");
}

#[gpui::test]
fn search_binding_does_not_keep_target_alive(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let target = cx.update(search_target);
    let weak = target.downgrade();
    let bar = cx.update(|cx| search_bar("needle", cx));
    let (_, visual) = cx.add_window_view(|window, cx| {
        let mut binding = SearchBarBinding::default();
        binding.set_active(
            Some((bar.clone(), Box::new(target.downgrade()))),
            window,
            cx,
        );
        BindingHost { binding }
    });
    drop(target);
    visual.run_until_parked();
    assert!(weak.upgrade().is_none(), "活动绑定不得延长目标生命周期");
    assert!(!bar.read_with(visual, |bar, _| bar.has_target()));
    visual.update(|window, cx| {
        bar.update(cx, |bar, cx| bar.toggle_case_sensitive(window, cx));
    });
}
