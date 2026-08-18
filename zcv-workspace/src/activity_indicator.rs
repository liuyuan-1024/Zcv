//! ActivityIndicator —— 状态栏后台任务指示器（对齐 zed 的 ActivityIndicator）。
//!
//! 订阅 GitStore 的任务事件，展示排队/执行/取消/确认阶段；远程任务提供显式取消入口。

use std::time::Duration;

use gpui::{
    Animation, AnimationExt, Entity, MouseButton, Render, Subscription, Window, div, prelude::*,
};
use zcv_project::{GitJobPhase, GitJobStatus, GitStore, GitStoreEvent};
use zcv_theme::{color, space};
use zcv_ui::TooltipSpec;

use crate::{ItemHandle, StatusItemView};

pub struct ActivityIndicator {
    git_store: Entity<GitStore>,
    _git_subscription: Subscription,
}

impl ActivityIndicator {
    pub fn new(git_store: Entity<GitStore>, cx: &mut Context<Self>) -> Self {
        let _git_subscription = cx.subscribe(&git_store, |_item, _store, event, cx| {
            if matches!(event, GitStoreEvent::JobsUpdated) {
                cx.notify();
            }
        });
        Self {
            git_store,
            _git_subscription,
        }
    }
}

impl StatusItemView for ActivityIndicator {
    fn set_active_pane_item(&mut self, _: Option<&dyn ItemHandle>, _: &mut Context<Self>) {}
}

impl Render for ActivityIndicator {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        // 动画帧直接读取共享进度，使 git 流式输出无需逐行跨线程发送界面事件也能更新。
        let Some(task) = self.git_store.read(cx).current_job() else {
            // 无任务时不占位。
            return div().into_any_element();
        };
        let mut item = div()
            .id("activity-indicator")
            .flex()
            .items_center()
            .gap(space::S6)
            .text_color(color::current(cx).text_muted)
            .child(animated_task(task.clone()));
        if let Some(build) = TooltipSpec::new(match task.phase {
            GitJobPhase::Queued if task.cancellable => "远程操作已排队，右键取消",
            GitJobPhase::Queued => "后台任务已排队",
            GitJobPhase::Running if task.cancellable => "右键取消并终止 Git 进程",
            GitJobPhase::Running => "后台任务正在执行",
            GitJobPhase::Cancelling => "正在终止 Git 及其子进程",
            GitJobPhase::Reconciling => "正在检查操作中断前远端是否已更新",
        })
        .build()
        {
            item = item.tooltip(build);
        }
        let git_store = self.git_store.clone();
        item.on_mouse_up(MouseButton::Right, move |_event, _window, cx| {
            git_store.update(cx, |store, cx| store.cancel_current_job(cx));
        })
        .into_any_element()
    }
}

// ═══ 私有渲染辅助 ═════════════════════════════════════════════════

/// spinner 帧：Braille 点阵字符（等宽，帧间无布局抖动；与 zed SpinnerLabel 同款）。
const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn animated_task(task: GitJobStatus) -> impl gpui::IntoElement {
    div().with_animation(
        "activity-indicator-task",
        Animation::new(Duration::from_millis(1000)).repeat(),
        move |row, delta| {
            let frame = (delta * SPINNER_FRAMES.len() as f32) as usize % SPINNER_FRAMES.len();
            row.flex()
                .items_center()
                .gap(space::S6)
                .child(SPINNER_FRAMES[frame])
                .child(task_message(&task))
        },
    )
}

fn task_message(task: &GitJobStatus) -> String {
    let mut message = match task.phase {
        GitJobPhase::Queued => format!("等待{}…", task.name),
        GitJobPhase::Running => format!("正在{}", task.name),
        GitJobPhase::Cancelling => format!("正在取消{}…", task.name),
        GitJobPhase::Reconciling => "正在确认远端状态…".to_string(),
    };
    if task.phase == GitJobPhase::Running
        && let Some(progress) = task.progress()
    {
        let progress: String = progress.chars().take(72).collect();
        message.push_str(" · ");
        message.push_str(&progress);
    }
    message
}
