//! TopBar —— 窗口级顶部外壳。

use gpui::{AnyElement, Div, Entity, Window, actions, div, prelude::*};

use super::window_controls;
use crate::recent_projects::{OnProjectSelected, ProjectPicker};
use crate::ui::Glyph;
use zcv_theme::{color, space};

actions!(top_bar, [OpenSettings, GitFetch, GitPull, GitPush,]);

pub(crate) struct TopBar {
    pub(crate) project_picker: Entity<ProjectPicker>,
    /// 当前 git 分支名（由 Workspace 订阅 GitStore 的 Head 事件刷新）。
    branch: Option<String>,
}

impl TopBar {
    pub(crate) fn new(on_selected: OnProjectSelected, cx: &mut gpui::Context<Self>) -> Self {
        let project_picker = cx.new(|cx| ProjectPicker::new(on_selected, cx));
        Self {
            project_picker,
            branch: None,
        }
    }

    pub(crate) fn set_branch(&mut self, branch: Option<String>) {
        self.branch = branch;
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
            .child(cluster(leading_slots(
                window,
                &self.project_picker,
                self.branch.as_deref(),
                cx,
            )))
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
    branch: Option<&str>,
    cx: &gpui::App,
) -> Vec<AnyElement> {
    let mut out: Vec<AnyElement> = Vec::new();

    // macOS 使用无标题栏窗口，因此在应用顶栏提供原生习惯的三色控制。
    #[cfg(target_os = "macos")]
    out.push(window_controls::render(window, cx).into_any_element());

    // 项目选择器
    out.push(project_picker.clone().into_any_element());
    // Git 分支：显示当前分支名（扫描未完成时显示占位）。
    out.push(
        Glyph::icon_text(
            "top-bar.branch",
            "icons/panels/version_control.svg",
            branch.unwrap_or("--"),
        )
        .label("分支")
        .into_any_element(),
    );
    // Git fetch
    out.push(
        Glyph::icon("top-bar.git-fetch", "icons/actions/arrow_circle.svg")
            .label("同步")
            .on_click(|window, cx| {
                window.dispatch_action(Box::new(GitFetch), cx);
            })
            .into_any_element(),
    );
    // Git pull / push：计数徽标待 git panel 提供 ahead/behind 数据后接入。
    out.push(
        Glyph::icon("top-bar.git-pull", "icons/actions/arrow_down.svg")
            .label("推送")
            .on_click(|window, cx| {
                window.dispatch_action(Box::new(GitPull), cx);
            })
            .into_any_element(),
    );
    out.push(
        Glyph::icon("top-bar.git-push", "icons/actions/arrow_up.svg")
            .label("拉取")
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
