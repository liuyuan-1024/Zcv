use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{Context, Div, Entity, FocusHandle, MouseButton, Render, Window, div, prelude::*};

use super::dock::{
    LayoutController, LayoutRef, LayoutSnapshot, PanelId, handle_close_tab,
    render_body as render_layout_body,
};
use super::pane_group::PaneId;
use super::recent_projects;
use crate::editor::registry::ViewRegistry;
use crate::keymap;
use crate::project_tree::ProjectTree;
use crate::workbench::bottom_bar::{
    BottomBar, ToggleDebug, ToggleKeyboardShortcuts, ToggleOutline, ToggleProjectTree,
    ToggleTerminal, ToggleVersionControl,
};
use crate::workbench::top_bar::{self, TopBar};
use crate::workbench::{project_picker, project_picker::OnProjectSelected, window_controls};

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

        // 创建项目切换回调
        let weak_self: gpui::WeakEntity<Self> = cx.weak_entity();
        let on_project_selected: OnProjectSelected = Rc::new(move |path, window, app| {
            if let Some(ws) = weak_self.upgrade() {
                ws.update(app, |workspace, cx| {
                    workspace.switch_project(&path, window, cx);
                });
            }
        });

        let top_bar = cx.new(|cx| TopBar::new(on_project_selected, cx));
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

    /// 切换面板焦点：若面板已聚焦则隐藏并退焦到编辑区，否则显示并聚焦。
    fn toggle_panel_focus(
        &mut self,
        panel: PanelId,
        focus_handle: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if focus_handle.contains_focused(window, cx) {
            // 面板已聚焦 → 隐藏面板，退焦到编辑区
            self.layout.borrow_mut().toggle_panel(panel);
            self.focus_center_pane(window, cx);
        } else {
            // 面板未聚焦 → 确保可见并聚焦
            if !self.layout.borrow().is_panel_active(panel) {
                self.layout.borrow_mut().toggle_panel(panel);
            }
            window.focus(focus_handle);
        }
        window.refresh();
    }

    /// 聚焦回编辑区。
    fn focus_center_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane_entity) = self.layout.borrow().focus_pane_entity() {
            window.focus(&pane_entity.read(cx).focus);
        }
    }

    fn handle_toggle_project_tree(
        &mut self,
        _: &ToggleProjectTree,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = self.project_tree.read(cx).focus.clone();
        self.toggle_panel_focus(PanelId::ProjectTree, &focus, window, cx);
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

    fn handle_toggle_project_picker(
        &mut self,
        _: &project_picker::ToggleProjectPicker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.top_bar.update(cx, |bar, cx| {
            bar.project_picker.update(cx, |picker, cx| {
                picker.toggle(window, cx);
            });
        });
    }

    /// 切换到指定目录作为项目根目录。
    fn switch_project(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let root = PathBuf::from(path);
        let label = root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        // 更新项目树
        self.project_tree.update(cx, |tree, cx| {
            tree.set_root(root.clone(), cx);
        });
        // 更新项目选择器显示的名称
        self.top_bar.update(cx, |bar, cx| {
            bar.project_picker.update(cx, |picker, _cx| {
                picker.set_current_label(label);
            });
        });
        // 持久化到最近项目
        recent_projects::add_to_recent(path);
        window.refresh();
    }
}

use crate::theme::{color, typography};
use crate::workbench::pane::Pane;

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
            .on_action(cx.listener(Self::handle_toggle_project_tree))
            .on_action(cx.listener(Self::handle_toggle_version_control))
            .on_action(cx.listener(Self::handle_toggle_outline))
            .on_action(cx.listener(Self::handle_toggle_terminal))
            .on_action(cx.listener(Self::handle_toggle_debug))
            .on_action(cx.listener(Self::handle_toggle_keyboard_shortcuts))
            .on_action(cx.listener(Self::handle_toggle_project_picker))
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
