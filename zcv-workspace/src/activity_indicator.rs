//! ActivityIndicator —— 状态栏后台任务指示器（对齐 zed 的 ActivityIndicator）。
//!
//! 订阅 GitStore 的任务事件，展示在途任务（动画 spinner + 任务名），右键点击取消当前任务（无上下文菜单，tooltip 给出提示）；无任务时隐藏。

use std::{sync::Arc, time::Duration};

use crate::{ItemHandle, StatusItemView};
use gpui::{
    Animation, AnimationExt, Entity, MouseButton, Render, Subscription, Window, div, prelude::*,
};
use zcv_project::{GitStore, GitStoreEvent};
use zcv_theme::{color, space};
use zcv_ui::TooltipSpec;

pub struct ActivityIndicator {
    /// 在途任务名；None 表示没有进行中的任务。
    task: Option<Arc<str>>,
    git_store: Entity<GitStore>,
    _git_subscription: Subscription,
}

impl ActivityIndicator {
    pub fn new(git_store: Entity<GitStore>, cx: &mut Context<Self>) -> Self {
        let task = git_store.read(cx).active_task();
        // 任务开始/完成/取消都会发 Tasks 事件：重读在途任务并重绘。
        let _git_subscription = cx.subscribe(&git_store, |item, store, event, cx| {
            if matches!(event, GitStoreEvent::Tasks) {
                item.task = store.read(cx).active_task();
                cx.notify();
            }
        });
        Self {
            task,
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
        let Some(task) = self.task.clone() else {
            // 无任务时不占位。
            return div().into_any_element();
        };
        let git_store = self.git_store.clone();
        // tooltip 需要 Stateful 元素：加 id 激活。
        let mut item = div()
            .id("activity-indicator")
            .flex()
            .items_center()
            .gap(space::S6)
            .text_color(color::current(cx).text_muted)
            .child(spinner())
            .child(task.to_string());
        // 右键直接取消且无菜单可看，tooltip 是唯一可见提示。
        if let Some(build) = TooltipSpec::new("右键取消任务").build() {
            item = item.tooltip(build);
        }
        item.on_mouse_up(MouseButton::Right, move |_event, _window, cx| {
            // 取消同步发 Tasks 事件，订阅回调已 notify，无需手动 refresh。
            git_store.update(cx, |store, cx| store.cancel_active_task(cx));
        })
        .into_any_element()
    }
}

// ═══ 私有渲染辅助 ═════════════════════════════════════════════════

/// spinner 帧：Braille 点阵字符（等宽，帧间无布局抖动；与 zed SpinnerLabel 同款）。
const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// 旋转 spinner：1s 周期，按 delta 比例取帧；仅在有任务时挂载。
fn spinner() -> impl gpui::IntoElement {
    div().with_animation(
        "activity-indicator-spinner",
        Animation::new(Duration::from_millis(1000)).repeat(),
        |glyph, delta| {
            let frame = (delta * SPINNER_FRAMES.len() as f32) as usize % SPINNER_FRAMES.len();
            glyph.child(SPINNER_FRAMES[frame])
        },
    )
}
