//! TopBar —— 窗口级顶部外壳。

use gpui::{
    AnyElement, AnyView, App, Context, Div, Entity, Subscription, WeakEntity, Window, div,
    prelude::*,
};
use zcv_actions::OpenSettings;
use zcv_project::{GitStoreEvent, Project, ProjectEvent};
use zcv_title_bar::TitleBar;
use zcv_ui::{Button, PopoverLayer};

use crate::{
    OnProjectSelected, Workspace, quick_access::QuickAccess, recent_projects::RecentProjects,
};

mod application_menu;

use application_menu::ApplicationMenu;

pub struct TopBar {
    pub recent_projects: Entity<RecentProjects>,
    quick_access: Entity<QuickAccess>,
    /// 应用级更新控件由 binary 装配层注入；TopBar 只负责其固定布局位置。
    update_control: Option<AnyView>,
    title_bar: TitleBar,
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
            title_bar: TitleBar::default(),
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
        let content = div()
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
            .child(
                div()
                    .min_w_0()
                    .overflow_x_hidden()
                    .flex()
                    .child(cluster(trailing_slots(self.update_control.as_ref(), cx))),
            );
        self.title_bar.render("top-bar", content, window, cx)
    }
}

fn cluster(items: Vec<AnyElement>) -> Div {
    div().flex().items_center().gap_2().children(items)
}

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

fn trailing_slots(update_control: Option<&AnyView>, cx: &App) -> Vec<AnyElement> {
    let mut out = Vec::new();
    if let Some(update_control) = update_control {
        out.push(update_control.clone().into_any_element());
    }
    out.push(
        Button::icon("top-bar.settings", "icons/settings.svg")
            .label("设置")
            .shortcut(zcv_keymap::display_shortcut(&OpenSettings, cx))
            .on_click(move |_, window, cx| {
                window.dispatch_action(Box::new(OpenSettings), cx);
            })
            .into_any_element(),
    );
    out
}
