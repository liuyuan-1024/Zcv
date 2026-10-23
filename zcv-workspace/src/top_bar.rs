//! TopBar —— 窗口级顶部外壳。

use std::rc::Rc;

use gpui::{
    AnyElement, AnyView, App, Context, Div, Entity, Subscription, WeakEntity, Window,
    WindowControlArea, div, prelude::*,
};
use zcv_actions::OpenSettings;
use zcv_picker::PickerLayer;
use zcv_project::{
    GitJobPhase, GitOperationKind, GitStore, GitStoreEvent, Project, RemoteOperationState,
};
use zcv_theme::{color, scale, typography};
use zcv_ui::Button;

use crate::{
    OnProjectSelected, Workspace,
    branch_picker::{BranchPicker, OnBranchSelected},
    project_picker::ProjectPicker,
    quick_access::QuickAccess,
};

mod window_controls;

pub(crate) type TopBarCallback = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone)]
pub struct TopBarCallbacks {
    pub on_git_fetch: TopBarCallback,
    pub on_git_pull: TopBarCallback,
    pub on_git_push: TopBarCallback,
}

pub struct TopBar {
    pub project_picker: Entity<ProjectPicker>,
    /// 分支选择器（显示当前分支名；与 TopBar 一样直接读取 GitStore）。
    pub branch_picker: Entity<BranchPicker>,
    quick_access: Entity<QuickAccess>,
    /// Git 状态权威；TopBar 只在渲染时读取，不再由宿主推送可写副本。
    git_store: Entity<GitStore>,
    /// 应用级更新控件由 binary 装配层注入；TopBar 只负责其固定布局位置。
    update_control: Option<AnyView>,
    workspace: WeakEntity<Workspace>,
    callbacks: TopBarCallbacks,
    window_controls: window_controls::WindowControls,
    _git_subscription: Subscription,
}

impl TopBar {
    pub fn new(
        on_selected: OnProjectSelected,
        workspace: WeakEntity<Workspace>,
        project: Entity<Project>,
        on_branch: OnBranchSelected,
        callbacks: TopBarCallbacks,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let git_store = project.read(cx).git_store();
        let picker_layer = PickerLayer::default();
        let project_picker = cx.new(|cx| {
            ProjectPicker::new(
                on_selected,
                project.clone(),
                workspace.clone(),
                picker_layer.clone(),
                window,
                cx,
            )
        });
        let branch_picker = cx.new(|cx| {
            BranchPicker::new(
                git_store.clone(),
                on_branch,
                picker_layer.clone(),
                window,
                cx,
            )
        });
        let quick_access = cx.new(|cx| {
            QuickAccess::new(project, workspace.clone(), picker_layer.clone(), window, cx)
        });
        let git_subscription = cx.subscribe(&git_store, |_, _, _: &GitStoreEvent, cx| cx.notify());
        Self {
            project_picker,
            branch_picker,
            quick_access,
            git_store,
            update_control: None,
            workspace,
            callbacks,
            window_controls: window_controls::WindowControls::default(),
            _git_subscription: git_subscription,
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

    // 项目选择器
    out.push(top_bar.project_picker.clone().into_any_element());

    // Git 分支与同步/推送/拉取操作：项目不是 git 仓库时不显示。
    let has_repositories = top_bar.git_store.read(cx).has_repositories();
    if has_repositories {
        // 空仓库没有分支或 HEAD，不显示分支选择器；其余仓库由选择器显示分支名或短 SHA。
        if top_bar.branch_picker.read(cx).has_branch_context(cx) {
            out.push(top_bar.branch_picker.clone().into_any_element());
        }
        // 无 remote 时 fetch/pull/push 都会报错，不给出入口；
        // 有 remote 时同步常显（主动检查更新的兜底），推送/拉取仅在可推/可拉时出现。
        let remote_operation_state = top_bar.git_store.read(cx).remote_operation_state();
        if remote_operation_state.has_remote {
            let busy = remote_operation_state.operation.is_some();
            let operation_label = remote_operation_label(remote_operation_state);
            out.push(
                Button::icon("top-bar.git-fetch", "icons/arrow_circle.svg")
                    .label(operation_label.unwrap_or("同步"))
                    .disabled(busy)
                    .on_click({
                        let callback = top_bar.callbacks.on_git_fetch.clone();
                        move |_, window, cx| callback(window, cx)
                    })
                    .into_any_element(),
            );
            if remote_operation_state.behind > 0 {
                out.push(
                    Button::icon_text(
                        "top-bar.git-pull",
                        "icons/arrow_down.svg",
                        remote_operation_state.behind.to_string(),
                    )
                    .label(operation_label.unwrap_or("拉取"))
                    .disabled(busy)
                    .on_click({
                        let callback = top_bar.callbacks.on_git_pull.clone();
                        move |_, window, cx| callback(window, cx)
                    })
                    .into_any_element(),
                );
            }
            if remote_operation_state.ahead > 0 {
                out.push(
                    Button::icon_text(
                        "top-bar.git-push",
                        "icons/arrow_up.svg",
                        remote_operation_state.ahead.to_string(),
                    )
                    .label(operation_label.unwrap_or("推送"))
                    .disabled(busy)
                    .on_click({
                        let callback = top_bar.callbacks.on_git_push.clone();
                        move |_, window, cx| callback(window, cx)
                    })
                    .into_any_element(),
                );
            }
        }
    }

    out
}

fn remote_operation_label(state: RemoteOperationState) -> Option<&'static str> {
    let operation = state.operation?;
    Some(match state.phase.unwrap_or(GitJobPhase::Queued) {
        GitJobPhase::Queued => match operation {
            GitOperationKind::Fetch => "等待同步…",
            GitOperationKind::Pull => "等待拉取…",
            GitOperationKind::Push => "等待推送…",
        },
        GitJobPhase::Running => match operation {
            GitOperationKind::Fetch => "正在同步…",
            GitOperationKind::Pull => "正在拉取…",
            GitOperationKind::Push => "正在推送…",
        },
        GitJobPhase::Cancelling => "正在取消远程操作…",
        GitJobPhase::Reconciling => "正在确认远端状态…",
    })
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
