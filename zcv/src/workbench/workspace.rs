use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{Context, Div, Entity, FocusHandle, MouseButton, Render, Window, div, prelude::*};

use super::bars::bottom_bar::{
    ToggleDebug, ToggleKeyboardShortcuts, ToggleOutline, ToggleProjectTree, ToggleTerminal,
    ToggleVersionControl,
};
use super::{
    BottomBar, LayoutController, LayoutRef, LayoutSnapshot, Pane, PaneId, PanelId, ProjectTree,
    TopBar, handle_close_tab, render_layout_body, top_bar, window_controls,
};
use crate::editor::ViewRegistry;
use crate::keymap;

pub(crate) struct Workspace {
    layout: Rc<RefCell<LayoutController>>,
    focus: FocusHandle,
    top_bar: Entity<TopBar>,
    bottom_bar: Entity<BottomBar>,
    project_tree: Entity<ProjectTree>,
}

impl Workspace {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
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

        cx.set_global(ViewRegistry::new());

        Self {
            focus,
            top_bar,
            bottom_bar,
            layout,
            project_tree,
        }
    }

    fn handle_open_project_picker(
        &mut self,
        _: &top_bar::OpenProjectPicker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let picker = self.top_bar.read(cx).project_picker.clone();
        picker.update(cx, |p, app| {
            p.toggle(window, app);
        });
    }

    fn handle_git_fetch(_: &top_bar::GitFetch, _: &mut Window, _: &mut gpui::App) {
        println!("fetch");
    }

    fn handle_git_pull(_: &top_bar::GitPull, _: &mut Window, _: &mut gpui::App) {
        println!("pull");
    }

    fn handle_git_push(_: &top_bar::GitPush, _: &mut Window, _: &mut gpui::App) {
        println!("push");
    }

    fn handle_open_settings(_: &top_bar::OpenSettings, _: &mut Window, _: &mut gpui::App) {
        println!("设置");
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

use crate::theme::{color, typography};

/// 工作台顶层框架组装。
fn render_frame(
    top_bar: &Entity<TopBar>,
    bottom_bar: &Entity<BottomBar>,
    layout: &LayoutSnapshot,
    project_tree: &Entity<ProjectTree>,
) -> Div {
    let tree = project_tree.clone();
    let panel_content = move |panel: PanelId| -> Option<Div> {
        match panel {
            PanelId::ProjectTree => Some(div().size_full().child(tree.clone())),
            _ => None,
        }
    };

    div()
        .relative()
        .flex()
        .flex_col()
        .size_full()
        .overflow_hidden()
        .bg(color::current().gray.s[1])
        .font(typography::ui_font())
        .text_size(typography::ui())
        .line_height(typography::ui())
        .text_color(color::current().gray.s[8])
        .child(top_bar.clone())
        .child(render_layout_body(layout, &panel_content))
        .child(bottom_bar.clone())
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.layout.borrow().snapshot();

        let layout_clone1 = self.layout.clone();
        let layout_clone2 = self.layout.clone();

        div()
            .id("app-view")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .child(render_frame(
                &self.top_bar,
                &self.bottom_bar,
                &snapshot,
                &self.project_tree,
            ))
            .on_action(window_controls::handle_quit)
            .on_action(window_controls::handle_minimize)
            .on_action(window_controls::handle_toggle_maximize)
            .on_action(handle_close_tab)
            .on_action(Self::handle_git_fetch)
            .on_action(Self::handle_git_pull)
            .on_action(Self::handle_git_push)
            .on_action(Self::handle_open_settings)
            .on_action(cx.listener(Self::handle_open_project_picker))
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
