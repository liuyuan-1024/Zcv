//! TopBar —— 窗口级顶部外壳。

use gpui::{AnyElement, Div, Entity, Window, actions, div, prelude::*};

use super::window_controls;
use crate::recent_projects::{OnProjectSelected, ProjectPicker};
use crate::ui::Glyph;
use zcv_theme::{color, space};

actions!(top_bar, [OpenSettings, GitFetch, GitPull, GitPush,]);

pub(crate) struct TopBar {
    pub(crate) project_picker: Entity<ProjectPicker>,
}

impl TopBar {
    pub(crate) fn new(on_selected: OnProjectSelected, cx: &mut gpui::Context<Self>) -> Self {
        let project_picker = cx.new(|cx| ProjectPicker::new(on_selected, cx));
        Self { project_picker }
    }
}

fn bar_frame(cx: &gpui::App) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .px(space::S8)
        .py(space::S6)
        .gap(space::S6)
        .bg(color::current(cx).title_bar_background)
        .text_color(color::current(cx).text)
        .border_b_1()
        .border_color(color::current(cx).border_variant)
}

impl gpui::Render for TopBar {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        bar_frame(cx)
            .id("top-bar")
            .child(cluster(leading_slots(window, &self.project_picker, cx)))
            .child(drag_spacer())
            .child(cluster(trailing_slots(cx)))
    }
}

fn cluster(items: Vec<AnyElement>) -> Div {
    div().flex().items_center().gap_2().children(items)
}

fn drag_spacer() -> Div {
    div().flex_1().h_full()
}

fn leading_slots(
    window: &Window,
    project_picker: &gpui::Entity<ProjectPicker>,
    cx: &gpui::App,
) -> Vec<AnyElement> {
    let mut out: Vec<AnyElement> = Vec::new();

    // macOS 使用无标题栏窗口，因此在应用顶栏提供原生习惯的三色控制。
    #[cfg(target_os = "macos")]
    out.push(window_controls::render(window, cx).into_any_element());

    // 项目选择器
    out.push(project_picker.clone().into_any_element());
    // Git 分支：显示当前分支名。
    out.push(
        Glyph::icon_text("top-bar.branch", "icons/panels/version_control.svg", "main")
            .label("分支")
            .into_any_element(),
    );
    // Git fetch
    out.push(
        Glyph::icon("top-bar.git-fetch", "icons/actions/arrow_circle.svg")
            .label("fetch")
            .on_click(|window, cx| {
                window.dispatch_action(Box::new(GitFetch), cx);
            })
            .into_any_element(),
    );
    // Git pull
    out.push(
        Glyph::icon_text("top-bar.git-pull", "icons/actions/arrow_down.svg", "0")
            .label("pull")
            .on_click(|window, cx| {
                window.dispatch_action(Box::new(GitPull), cx);
            })
            .into_any_element(),
    );
    // Git push
    out.push(
        Glyph::icon_text("top-bar.git-push", "icons/actions/arrow_up.svg", "0")
            .label("push")
            .on_click(|window, cx| {
                window.dispatch_action(Box::new(GitPush), cx);
            })
            .into_any_element(),
    );

    out
}

fn trailing_slots(cx: &gpui::App) -> Vec<AnyElement> {
    vec![
        Glyph::icon("top-bar.settings", "icons/actions/settings.svg")
            .label("设置")
            .shortcut(&OpenSettings, cx)
            .on_click(|window, cx| {
                window.dispatch_action(Box::new(OpenSettings), cx);
            })
            .into_any_element(),
    ]
}
