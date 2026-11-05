//! TopBar —— 窗口级顶部外壳。

use gpui::{
    AnyElement, AnyView, App, Context, Div, Entity, Subscription, WeakEntity, Window,
    WindowControlArea, div, prelude::*,
};
use zcv_actions::OpenSettings;
use zcv_project::{GitStoreEvent, Project, ProjectEvent};
use zcv_theme::{color, scale, typography};
use zcv_ui::{Button, PopoverLayer};

use crate::{
    OnProjectSelected, Workspace, quick_access::QuickAccess, recent_projects::RecentProjects,
};

mod application_menu;
mod window_controls;

use application_menu::ApplicationMenu;
use window_controls::WindowControls;

pub struct TopBar {
    pub recent_projects: Entity<RecentProjects>,
    quick_access: Entity<QuickAccess>,
    /// 应用级更新控件由 binary 装配层注入；TopBar 只负责其固定布局位置。
    update_control: Option<AnyView>,
    workspace: WeakEntity<Workspace>,
    window_controls: WindowControls,
    application_menu: ApplicationMenu,
    _subscriptions: Vec<Subscription>,
}

impl TopBar {
    pub fn new(
        on_selected: OnProjectSelected,
        workspace: WeakEntity<Workspace>,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let picker_layer = PopoverLayer::default();
        let application_menu = ApplicationMenu::new(picker_layer.clone(), cx);
        let recent_projects = cx.new(|cx| {
            RecentProjects::new(
                on_selected,
                project.clone(),
                workspace.clone(),
                picker_layer.clone(),
                window,
                cx,
            )
        });
        let quick_access = cx.new(|cx| {
            QuickAccess::new(
                project.clone(),
                workspace.clone(),
                picker_layer.clone(),
                window,
                cx,
            )
        });
        let git_store = project.read(cx).git_store();
        let git_subscription = cx.subscribe(&git_store, |_, _, event, cx| {
            if matches!(
                event,
                GitStoreEvent::Repositories | GitStoreEvent::ActiveRepositoryChanged
            ) {
                cx.notify();
            }
        });
        let project_subscription = cx.subscribe(&project, |_, _, event, cx| {
            if matches!(
                event,
                ProjectEvent::WorktreeAdded { .. }
                    | ProjectEvent::WorktreeRemoved { .. }
                    | ProjectEvent::WorktreeRootChanged { .. }
            ) {
                cx.notify();
            }
        });
        Self {
            recent_projects,
            quick_access,
            update_control: None,
            workspace,
            window_controls: WindowControls::new(),
            application_menu,
            _subscriptions: vec![git_subscription, project_subscription],
        }
    }

    pub fn set_update_control(&mut self, update_control: AnyView, cx: &mut Context<Self>) {
        self.update_control = Some(update_control);
        cx.notify();
    }

    pub fn focus_quick_access(&self, window: &mut Window, cx: &mut App) {
        self.quick_access
            .update(cx, |search, cx| search.focus(window, cx));
    }

    pub fn focus_line_column(
        &self,
        target: Box<dyn crate::ItemHandle>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.quick_access.update(cx, |search, cx| {
            search.focus_line_column(target, window, cx)
        });
    }
}

impl gpui::Render for TopBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let height = bar_height(window, cx);
        self.window_controls.align(window, height);
        window_controls::titlebar_interaction(bar_frame(height, cx).id("top-bar"))
            .child(
                content_frame(window)
                    .gap(scale::S6)
                    .child(
                        div()
                            .min_w_0()
                            .overflow_x_hidden()
                            .child(cluster(leading_slots(self, cx))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(self.quick_access.clone()),
                    )
                    .child(div().min_w_0().overflow_x_hidden().flex().child(cluster(
                        trailing_slots(self.update_control.as_ref(), self.workspace.clone(), cx),
                    ))),
            )
            .when_some(window_controls::trailing(window, cx), |bar, controls| {
                bar.child(controls)
            })
            .child(bar_bottom_border(cx))
    }
}

fn bar_height(window: &Window, cx: &App) -> gpui::Pixels {
    typography::ui_line_at(window.rem_size(), cx)
        + scale::to_pixels(scale::S12, window)
        + scale::to_pixels(scale::S8, window)
}

fn content_frame(window: &Window) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .flex_1()
        .min_w_0()
        .py(scale::S6)
        .pr(scale::S6)
        .pl(window_controls::leading_inset(window))
}

fn bar_frame(height: gpui::Pixels, cx: &App) -> Div {
    div()
        .window_control_area(WindowControlArea::Drag)
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .h(height)
        .bg(color::current(cx).title_bar_background)
        .text_color(color::current(cx).text)
}

fn bar_bottom_border(cx: &App) -> Div {
    div()
        .absolute()
        .inset_0()
        .border_b_1()
        .border_color(color::current(cx).border)
}

fn cluster(items: Vec<AnyElement>) -> Div {
    div().flex().items_center().gap_2().children(items)
}

#[cfg(test)]
#[path = "test/top_bar_tests.rs"]
mod tests;

fn leading_slots(top_bar: &TopBar, cx: &App) -> Vec<AnyElement> {
    let mut out: Vec<AnyElement> = Vec::new();

    if let Some(menu) = top_bar.application_menu.leading_slot() {
        out.push(menu);
    }
    if top_bar.application_menu.is_expanded(cx) {
        return out;
    }

    // 项目选择器
    out.push(top_bar.recent_projects.clone().into_any_element());

    out
}

fn trailing_slots(
    update_control: Option<&AnyView>,
    workspace: WeakEntity<Workspace>,
    cx: &App,
) -> Vec<AnyElement> {
    let mut out = Vec::new();
    if let Some(update_control) = update_control {
        out.push(update_control.clone().into_any_element());
    }
    out.push(
        Button::icon("top-bar.settings", "icons/settings.svg")
            .label("设置")
            .shortcut(zcv_keymap::display_shortcut(&OpenSettings, cx))
            .on_click(move |_, window, cx| {
                workspace
                    .update(cx, |workspace, cx| workspace.open_settings(window, cx))
                    .ok();
            })
            .into_any_element(),
    );
    out
}
