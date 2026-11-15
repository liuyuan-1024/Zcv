//! 状态栏后台活动指示器。

use std::rc::Rc;

use gpui::{MouseButton, Render, Subscription, Window, div, prelude::*};
use zcv_activity::{ActivityChanged, ActivityPhase, ActivitySource, BackgroundActivity};
use zcv_theme::{color, scale};
use zcv_ui::{ProgressIndicator, ProgressMode, TooltipSpec};

use crate::{ItemHandle, StatusItemView};

pub struct ActivityIndicator {
    sources: Vec<Box<dyn ActivitySource>>,
    _subscriptions: Vec<Subscription>,
}

impl ActivityIndicator {
    /// 按来源顺序选择主活动。
    pub fn new(sources: Vec<Box<dyn ActivitySource>>, cx: &mut Context<Self>) -> Self {
        let indicator = cx.weak_entity();
        let changed: ActivityChanged = Rc::new(move |cx| {
            let _ = indicator.update(cx, |_, cx| cx.notify());
        });
        let subscriptions = sources
            .iter()
            .map(|source| source.observe(changed.clone(), cx))
            .collect();
        Self {
            sources,
            _subscriptions: subscriptions,
        }
    }
}

impl StatusItemView for ActivityIndicator {
    fn set_active_pane_item(&mut self, _: Option<&dyn ItemHandle>, _: &mut Context<Self>) {}
}

impl Render for ActivityIndicator {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut activities = Vec::new();
        for source in &self.sources {
            source.collect(&mut activities, cx);
        }
        let mut activities = activities.into_iter();
        let Some(primary) = activities.next() else {
            return div().into_any_element();
        };
        let label = activity_label(&primary);
        let mut lines = vec![label.clone()];
        lines.extend(activities.map(|activity| activity_label(&activity)));
        let remaining = lines.len() - 1;
        let BackgroundActivity {
            progress,
            detail,
            cancel,
            ..
        } = primary;
        lines.extend(detail.map(|detail| detail.to_string()));
        if cancel.is_some() {
            lines.push("右键取消".into());
        }

        let mut message = label;
        if let Some(progress) = progress.and_then(|read| read()) {
            message.push_str(" · ");
            message.extend(progress.chars().take(72));
        }
        if remaining > 0 {
            message.push_str(&format!("（另有 {remaining} 项）"));
        }
        let mut indicator = div()
            .id("activity-indicator")
            .debug_selector(|| "workspace-background-task".into())
            .flex()
            .items_center()
            .gap(scale::S6)
            .text_color(color::current(cx).text_muted)
            .child(ProgressIndicator::new(
                "activity-progress",
                ProgressMode::Indeterminate,
            ))
            .child(message);
        if let Some(tooltip) = TooltipSpec::from_lines(lines).build() {
            indicator = indicator.tooltip(tooltip);
        }
        if let Some(cancel) = cancel {
            indicator = indicator.on_mouse_up(MouseButton::Right, move |_, _, cx| cancel(cx));
        }
        indicator.into_any_element()
    }
}

fn activity_label(activity: &BackgroundActivity) -> String {
    let name = &activity.name;
    match activity.phase {
        ActivityPhase::Queued => format!("等待{name}…"),
        ActivityPhase::Running => format!("正在{name}"),
        ActivityPhase::Cancelling => format!("正在取消{name}…"),
        ActivityPhase::Confirming => format!("正在确认{name}结果…"),
    }
}
