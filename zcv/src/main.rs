#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

pub(crate) mod editor;
mod features;
mod keymap;
mod shared;
mod surface;
mod theme;
mod workbench;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    App, AppContext, Application, Bounds, Context, Entity, FocusHandle, MouseButton, Render,
    TitlebarOptions, Window, WindowBounds, WindowOptions, div, point, prelude::*, px, size,
};

use editor::{EditableText, ViewRegistry};
use features::project_picker::ProjectSearchEditor;
use shared::assets::{EmbeddedAssets, embedded_fonts};
use surface::{SurfaceManager, SurfaceShell};
use theme::Theme;
use workbench::bottom_bar::{
    ToggleDebug, ToggleKeyboardShortcuts, ToggleOutline, ToggleProjectTree, ToggleTerminal,
    ToggleVersionControl,
};
use workbench::{
    BottomBar, LayoutController, LayoutRef, Pane, PaneId, PanelId, ProjectTree, TopBar,
    handle_close_tab, top_bar, window_controls,
};

struct AppView {
    layout: Rc<RefCell<LayoutController>>,
    focus: FocusHandle,
    top_bar: Entity<TopBar>,
    bottom_bar: Entity<BottomBar>,
    surface_shell: Entity<SurfaceShell>,
    project_tree: Entity<ProjectTree>,
}

impl AppView {
    fn new(cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();

        let keybindings = keymap::load();
        cx.bind_keys(keybindings.bindings.clone());
        cx.set_global(keybindings);

        let top_bar = cx.new(TopBar::new);
        let bottom_bar = cx.new(BottomBar::new);

        let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        let initial_pane = cx.new(|cx| Pane::new(PaneId(1), cx));
        let layout = Rc::new(RefCell::new(LayoutController::with_initial_pane(
            initial_pane,
        )));
        cx.set_global(LayoutRef(Rc::downgrade(&layout)));

        let project_tree = cx.new(|cx| ProjectTree::new(root, cx));

        let surface_shell = cx.new(|_| SurfaceShell::new());
        cx.set_global(SurfaceManager::new());
        cx.set_global(ViewRegistry::new());

        let picker_search = cx.new(|cx| EditableText::new("picker-search", cx));
        cx.set_global(ProjectSearchEditor(picker_search));

        Self {
            focus,
            top_bar,
            bottom_bar,
            layout,
            project_tree,
            surface_shell,
        }
    }

    fn handle_toggle_project_tree(
        &mut self,
        _: &ToggleProjectTree,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut layout = self.layout.borrow_mut();
        layout.toggle_panel(PanelId::ProjectTree);
        drop(layout);
        window.focus(&self.project_tree.read(cx).focus);
        window.refresh();
    }

    fn handle_toggle_version_control(
        &mut self,
        _: &ToggleVersionControl,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.layout
            .borrow_mut()
            .toggle_panel(PanelId::VersionControl);
        window.refresh();
    }

    fn handle_toggle_outline(
        &mut self,
        _: &ToggleOutline,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.layout.borrow_mut().toggle_panel(PanelId::Outline);
        window.refresh();
    }

    fn handle_toggle_terminal(
        &mut self,
        _: &ToggleTerminal,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.layout.borrow_mut().toggle_panel(PanelId::Terminal);
        window.refresh();
    }

    fn handle_toggle_debug(
        &mut self,
        _: &ToggleDebug,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.layout.borrow_mut().toggle_panel(PanelId::Debug);
        window.refresh();
    }

    fn handle_toggle_keyboard_shortcuts(
        &mut self,
        _: &ToggleKeyboardShortcuts,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.layout
            .borrow_mut()
            .toggle_panel(PanelId::KeyboardShortcuts);
        window.refresh();
    }
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.layout.borrow().snapshot();

        let layout_clone1 = self.layout.clone();
        let layout_clone2 = self.layout.clone();

        div()
            .id("app-view")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .child(workbench::render(
                &self.top_bar,
                &self.bottom_bar,
                &snapshot,
                &self.project_tree,
            ))
            .child(self.surface_shell.clone())
            .on_action(window_controls::handle_quit)
            .on_action(window_controls::handle_minimize)
            .on_action(window_controls::handle_toggle_maximize)
            .on_action(handle_close_tab)
            .on_action(top_bar::handle_open_settings)
            .on_action(top_bar::handle_open_project_picker)
            .on_action(top_bar::handle_toggle_branch_picker)
            .on_action(top_bar::handle_git_fetch)
            .on_action(top_bar::handle_git_pull)
            .on_action(top_bar::handle_git_push)
            .on_action(cx.listener(Self::handle_toggle_project_tree))
            .on_action(cx.listener(Self::handle_toggle_version_control))
            .on_action(cx.listener(Self::handle_toggle_outline))
            .on_action(cx.listener(Self::handle_toggle_terminal))
            .on_action(cx.listener(Self::handle_toggle_debug))
            .on_action(cx.listener(Self::handle_toggle_keyboard_shortcuts))
            // 拖拽分隔线时：鼠标移动 → 更新 dock 尺寸
            .on_mouse_move(move |event, window, _cx| {
                let mut ctrl = layout_clone1.borrow_mut();
                if ctrl.is_dragging() {
                    ctrl.drag_to(event.position, window.bounds().size);
                    window.refresh();
                }
            })
            // 拖拽结束 → 清理状态
            .on_mouse_up(MouseButton::Left, move |_event, window, _cx| {
                layout_clone2.borrow_mut().end_drag();
                window.refresh();
            })
    }
}

fn main() {
    Application::new()
        .with_assets(EmbeddedAssets)
        .run(|cx: &mut App| {
            cx.text_system()
                .add_fonts(embedded_fonts())
                .expect("内置字体应能注册");

            Theme::OneDark.apply(None);

            let bounds = Bounds::centered(None, size(px(1200.0), px(900.0)), cx);

            let _window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        titlebar: Some(TitlebarOptions {
                            title: Some("".into()),
                            appears_transparent: true,
                            traffic_light_position: Some(point(px(-100.0), px(-100.0))),
                        }),
                        ..Default::default()
                    },
                    |_, cx| cx.new(AppView::new),
                )
                .expect("主窗口应能创建");
        });
}
