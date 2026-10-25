use std::sync::Arc;

use gpui::{App, Context, FocusHandle, Render, TestAppContext, Window, div, px};

use super::*;
use crate::{Panel, PanelEvent, PanelHandle};
use zcv_settings::{SettingsStore, UserSettings};

macro_rules! test_panel {
    ($name:ident, $persistent_name:literal) => {
        struct $name {
            focus: FocusHandle,
        }

        impl EventEmitter<PanelEvent> for $name {}

        impl Panel for $name {
            fn icon() -> &'static str {
                "icons/list_tree.svg"
            }

            fn label() -> &'static str {
                $persistent_name
            }

            fn persistent_name() -> &'static str {
                $persistent_name
            }

            fn focus_handle(&self, _: &App) -> FocusHandle {
                self.focus.clone()
            }
        }

        impl Render for $name {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().track_focus(&self.focus)
            }
        }
    };
}

test_panel!(FirstPanel, "first");
test_panel!(SecondPanel, "second");

struct InputPanel {
    focus: FocusHandle,
    input_focus: FocusHandle,
}

impl EventEmitter<PanelEvent> for InputPanel {}

impl Panel for InputPanel {
    fn icon() -> &'static str {
        "icons/list_tree.svg"
    }

    fn label() -> &'static str {
        "输入面板"
    }

    fn persistent_name() -> &'static str {
        "input-panel"
    }

    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }

    fn activation_focus_handle(&self, _: &App) -> FocusHandle {
        self.input_focus.clone()
    }
}

impl Render for InputPanel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus)
            .child(div().track_focus(&self.input_focus))
    }
}

#[gpui::test]
fn focusing_dock_enters_the_active_panel_input(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let panel = cx.new(|cx| InputPanel {
        focus: cx.focus_handle(),
        input_focus: cx.focus_handle(),
    });
    let input_focus = cx.read_entity(&panel, |panel, _| panel.input_focus.clone());
    let handle: Arc<dyn PanelHandle> = Arc::new(panel);
    let (dock, cx) = cx.add_window_view(move |window, cx| {
        let mut dock = Dock::new(
            DockPosition::Bottom,
            Vec::new(),
            px(200.0),
            None,
            window,
            cx,
        );
        dock.add_panel(handle, window, cx);
        dock.set_open(true, window, cx);
        dock
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    cx.update(|window, _| assert!(window.is_window_active()));
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let dock_focus = cx.read_entity(&dock, |dock, cx| dock.focus_handle(cx));
    cx.update(|window, cx| window.focus(&dock_focus, cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));

    cx.update(|window, _| {
        assert!(!dock_focus.is_focused(window), "Dock 根焦点应转发给面板");
        assert!(input_focus.is_focused(window));
    });
}

#[gpui::test]
fn restores_active_panel_after_late_registration(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let first = cx.new(|cx| FirstPanel {
        focus: cx.focus_handle(),
    });
    let second = cx.new(|cx| SecondPanel {
        focus: cx.focus_handle(),
    });
    let first_handle: Arc<dyn PanelHandle> = Arc::new(first);
    let second_handle: Arc<dyn PanelHandle> = Arc::new(second);
    let state = DockData {
        visible: true,
        active_panel: Some("second".into()),
        size: Some(333.0),
    };
    let (dock, cx) = cx.add_window_view(move |window, cx| {
        let mut dock = Dock::new(
            DockPosition::Left,
            Vec::new(),
            DockPosition::Left.default_size(),
            Some(state),
            window,
            cx,
        );
        dock.add_panel(first_handle, window, cx);
        assert!(!dock.is_open());
        dock.add_panel(second_handle, window, cx);
        dock
    });

    cx.read_entity(&dock, |dock, _| {
        assert!(dock.is_open());
        assert_eq!(dock.active_panel_index(), Some(1));
        assert_eq!(dock.capture_state().active_panel.as_deref(), Some("second"));
        assert_eq!(dock.capture_state().size, Some(333.0));
    });
}

/// 序列化 visible=true 的 dock 应随面板注册恢复打开（回归：终端面板重启不展开）。
#[gpui::test]
fn serialized_visible_dock_opens_on_panel_registration(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let panel = cx.new(|cx| FirstPanel {
        focus: cx.focus_handle(),
    });
    let handle: Arc<dyn PanelHandle> = Arc::new(panel);
    let serialized = DockData {
        visible: true,
        active_panel: Some("first".into()),
        size: Some(200.0),
    };
    let (dock, cx) = cx.add_window_view(move |window, cx| {
        let mut dock = Dock::new(
            DockPosition::Bottom,
            Vec::new(),
            px(200.0),
            Some(serialized),
            window,
            cx,
        );
        dock.add_panel(handle, window, cx);
        dock
    });

    cx.read_entity(&dock, |dock, _| {
        assert!(dock.is_open(), "可见的 dock 应随面板注册恢复打开");
    });
}

#[gpui::test]
fn capture_uses_stable_panel_name_not_index(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let panel = cx.new(|cx| SecondPanel {
        focus: cx.focus_handle(),
    });
    let handle: Arc<dyn PanelHandle> = Arc::new(panel);
    let (dock, cx) = cx.add_window_view(move |window, cx| {
        let mut dock = Dock::new(
            DockPosition::Bottom,
            Vec::new(),
            px(200.0),
            None,
            window,
            cx,
        );
        dock.add_panel(handle, window, cx);
        dock.set_open(true, window, cx);
        dock
    });

    cx.read_entity(&dock, |dock, _| {
        assert_eq!(dock.capture_state().active_panel.as_deref(), Some("second"));
    });
}
