//! TopBar —— 窗口级顶部外壳。

use gpui::{AnyElement, Div, Entity, Window, div, prelude::*};
use zcv_actions::{GitFetch, GitPull, GitPush, OpenSettings};
use zcv_git::Branch;
use zcv_project::RemoteOperationState;
use zcv_theme::{color, space};
use zcv_ui::Glyph;

use crate::OnProjectSelected;
use crate::branch_picker::{BranchPicker, OnBranchSelected};
use crate::project_picker::ProjectPicker;
use crate::window_controls::render as render_window_controls;

pub struct TopBar {
    pub project_picker: Entity<ProjectPicker>,
    /// 分支选择器（glyph 显示当前分支名；由 Workspace 订阅 GitStore 事件刷新）。
    pub branch_picker: Entity<BranchPicker>,
    /// 项目是否已发现 git 仓库（非 git 项目不显示分支与同步/推送/拉取按钮）。
    has_repositories: bool,
    /// 活动仓库的远程操作状态（无 remote 时同步/推送/拉取按钮都不显示）。
    remote_operation_state: RemoteOperationState,
}

impl TopBar {
    pub fn new(
        on_selected: OnProjectSelected,
        on_branch: OnBranchSelected,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let project_picker = cx.new(|cx| ProjectPicker::new(on_selected, cx));
        let branch_picker = cx.new(|cx| BranchPicker::new(on_branch, cx));
        Self {
            project_picker,
            branch_picker,
            has_repositories: false,
            remote_operation_state: RemoteOperationState::default(),
        }
    }

    /// 分支数据由 Workspace 订阅 GitStore 事件后推送（glyph 与列表同仓库）。
    pub fn set_branch(&mut self, branch: Option<String>, cx: &mut gpui::Context<Self>) {
        self.branch_picker
            .update(cx, |picker, _| picker.set_branch(branch));
    }

    pub fn set_branches(&mut self, branches: Vec<Branch>, cx: &mut gpui::Context<Self>) {
        self.branch_picker
            .update(cx, |picker, _| picker.set_branches(branches));
    }

    pub fn set_has_repositories(&mut self, has_repositories: bool) {
        self.has_repositories = has_repositories;
    }

    pub fn set_remote_operation_state(&mut self, state: RemoteOperationState) {
        self.remote_operation_state = state;
    }
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
                &self.branch_picker,
                self.has_repositories,
                self.remote_operation_state,
                cx,
            )))
            .child(drag_spacer())
            .child(cluster(trailing_slots(cx)))
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

fn cluster(items: Vec<AnyElement>) -> Div {
    div().flex().items_center().gap_2().children(items)
}

fn drag_spacer() -> Div {
    div().flex_1().h_full()
}

fn leading_slots(
    window: &Window,
    project_picker: &gpui::Entity<ProjectPicker>,
    branch_picker: &gpui::Entity<BranchPicker>,
    has_repositories: bool,
    state: RemoteOperationState,
    cx: &gpui::App,
) -> Vec<AnyElement> {
    let mut out: Vec<AnyElement> = Vec::new();

    // macOS 使用无标题栏窗口，因此在应用顶栏提供原生习惯的三色控制。
    #[cfg(target_os = "macos")]
    out.push(render_window_controls(window, cx).into_any_element());

    // 项目选择器
    out.push(project_picker.clone().into_any_element());

    // Git 分支与同步/推送/拉取操作：项目不是 git 仓库时不显示（对齐 Zed 静默降级）。
    if has_repositories {
        // Git 分支：glyph 由分支选择器自含（点击弹出分支列表）。
        out.push(branch_picker.clone().into_any_element());
        // 无 remote 时 fetch/pull/push 都会报错，不给出入口；有 remote 时同步常显
        // （主动检查更新的兜底），推送/拉取仅在可推/可拉时出现（icon_text 复用为计数徽标）。
        if state.has_remote {
            out.push(
                Glyph::icon("top-bar.git-fetch", "icons/arrow_circle.svg")
                    .label("同步")
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(GitFetch), cx);
                    })
                    .into_any_element(),
            );
            if state.behind > 0 {
                out.push(
                    Glyph::icon_text(
                        "top-bar.git-pull",
                        "icons/arrow_down.svg",
                        state.behind.to_string(),
                    )
                    .label("拉取")
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(GitPull), cx);
                    })
                    .into_any_element(),
                );
            }
            if state.ahead > 0 {
                out.push(
                    Glyph::icon_text(
                        "top-bar.git-push",
                        "icons/arrow_up.svg",
                        state.ahead.to_string(),
                    )
                    .label("推送")
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(GitPush), cx);
                    })
                    .into_any_element(),
                );
            }
        }
    }

    out
}

fn trailing_slots(cx: &gpui::App) -> Vec<AnyElement> {
    vec![
        Glyph::icon("top-bar.settings", "icons/settings.svg")
            .label("设置")
            .shortcut(&OpenSettings, cx)
            .on_click(|_, window, cx| {
                window.dispatch_action(Box::new(OpenSettings), cx);
            })
            .into_any_element(),
    ]
}
