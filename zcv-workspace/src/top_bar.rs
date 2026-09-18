//! TopBar —— 窗口级顶部外壳。

use std::rc::Rc;

use gpui::{
    AnyElement, AnyView, App, Div, Entity, Subscription, WeakEntity, Window, WindowControlArea,
    div, prelude::*,
};
use zcv_actions::OpenSettings;
use zcv_project::{GitJobPhase, GitOperationKind, GitStore, GitStoreEvent, RemoteOperationState};
use zcv_theme::{color, space};
use zcv_ui::Button;

use crate::branch_picker::{BranchPicker, OnBranchSelected};
use crate::project_picker::ProjectPicker;
use crate::{OnProjectSelected, Workspace};

mod window_controls;

use window_controls::render as render_window_controls;

pub type TopBarCallback = Rc<dyn Fn(&mut Window, &mut App)>;

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
    /// Git 状态权威；TopBar 只在渲染时读取，不再由宿主推送可写副本。
    git_store: Entity<GitStore>,
    /// 应用级更新控件由 binary 装配层注入；TopBar 只负责其固定布局位置。
    update_control: Option<AnyView>,
    workspace: WeakEntity<Workspace>,
    callbacks: TopBarCallbacks,
    _git_subscription: Subscription,
}

impl TopBar {
    pub fn new(
        on_selected: OnProjectSelected,
        workspace: WeakEntity<Workspace>,
        git_store: Entity<GitStore>,
        on_branch: OnBranchSelected,
        callbacks: TopBarCallbacks,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let project_picker =
            cx.new(|cx| ProjectPicker::new(on_selected, workspace.clone(), window, cx));
        let branch_picker =
            cx.new(|cx| BranchPicker::new(git_store.clone(), on_branch, window, cx));
        let git_subscription = cx.subscribe(&git_store, |_, _, _: &GitStoreEvent, cx| cx.notify());
        Self {
            project_picker,
            branch_picker,
            git_store,
            update_control: None,
            workspace,
            callbacks,
            _git_subscription: git_subscription,
        }
    }

    pub fn set_update_control(&mut self, update_control: AnyView, cx: &mut gpui::Context<Self>) {
        self.update_control = Some(update_control);
        cx.notify();
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
            .child(cluster(leading_slots(window, self, cx)))
            .child(drag_spacer())
            .child(cluster(trailing_slots(
                self.update_control.as_ref(),
                self.workspace.clone(),
                cx,
            )))
    }
}

fn bar_frame(cx: &gpui::App) -> Div {
    div()
        .window_control_area(WindowControlArea::Drag)
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .p(space::S6)
        .gap(space::S6)
        .bg(color::current(cx).title_bar_background)
        .text_color(color::current(cx).text)
        .border_b_1()
        .border_color(color::current(cx).border)
}

fn cluster(items: Vec<AnyElement>) -> Div {
    div().flex().items_center().gap_2().children(items)
}

fn drag_spacer() -> Div {
    div().flex_1().h_full()
}

fn leading_slots(window: &Window, top_bar: &TopBar, cx: &App) -> Vec<AnyElement> {
    let mut out: Vec<AnyElement> = Vec::new();

    // 无标题栏窗口，因此在应用顶栏提供三色控制。
    out.push(render_window_controls(window, top_bar.workspace.clone()).into_any_element());

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
    cx: &gpui::App,
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
