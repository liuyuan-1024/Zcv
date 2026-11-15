//! Toast —— 工作区内的成功、信息、警告和错误提示。
//!
//! 三段式形态：头部行（左侧语义图标，右侧复制/关闭）、内容主体（固定最大宽度内自动换行）、底部操作按钮。
//! 由宿主窗口持有并叠加在根视图上，各条提示独立计时和关闭。
//! 容器自身不注册鼠标事件（不创建 hitbox），不会拦截下方视图的交互。

use std::{
    rc::Rc,
    time::{Duration, Instant},
};

use crate::{Button, ButtonStyle, ProgressIndicator, ProgressMode, SvgIcon};
use gpui::{
    AlignItems, AnyElement, App, AsyncApp, Bounds, ClipboardItem, Context, Display, Element,
    ElementId, EntityId, FlexDirection, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    Pixels, Render, Rgba, ScrollHandle, SharedString, Style, Task, WeakEntity, Window, auto, div,
    point, prelude::*, px, relative, size,
};
use zcv_theme::{
    color::{self, ThemeColors},
    scale,
};

/// 复制反馈的展示时长。
const COPIED_FEEDBACK_DURATION: Duration = Duration::from_secs(2);
/// 减少动态效果时，按较低频率刷新剩余时长。
const REDUCED_MOTION_REFRESH_INTERVAL: Duration = Duration::from_millis(500);
/// 恢复计时的最短剩余时长。
const MINIMUM_RESUME_DURATION: Duration = Duration::from_millis(800);
/// 新提示从下方进入的时长。
const TOAST_ENTER_DURATION: Duration = Duration::from_millis(280);
/// 内容主体的最大宽度，超长文本在此宽度内自动换行。
const TOAST_MAX_WIDTH: f32 = 384.0;
/// 常驻提示边框中语义色的混合权重。
const PERSISTENT_BORDER_TINT: f32 = 0.65;

type ToastClickHandler = Rc<dyn Fn(&mut Window, &mut App) + 'static>;

/// Toast 的语义类型（决定图标与颜色）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Info,
    Warning,
    Error,
}

/// Toast 上的操作按钮（如"重试"）：点击执行后关闭所属 toast。
#[derive(Clone)]
pub struct ToastAction {
    label: SharedString,
    on_click: ToastClickHandler,
}

impl ToastAction {
    pub fn new(
        label: impl Into<SharedString>,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            on_click: Rc::new(on_click),
        }
    }
}

/// 工作区内单条提示的身份。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToastId {
    layer: EntityId,
    sequence: u64,
}

/// 正在展示的 toast。
struct ActiveToast {
    id: ToastId,
    kind: ToastKind,
    message: SharedString,
    actions: Vec<ToastAction>,
    /// 复制后短暂展示"已复制"反馈。
    copied: bool,
    copy_feedback_task: Option<Task<()>>,
    countdown: Option<ToastCountdown>,
}

struct ToastCountdown {
    total: Duration,
    remaining: Duration,
    running: Option<RunningCountdown>,
}

struct RunningCountdown {
    started_at: Instant,
    duration: Duration,
    _task: Task<()>,
}

impl ToastCountdown {
    fn remaining_at(&self, now: Instant) -> Duration {
        let Some(running) = &self.running else {
            return self.remaining;
        };
        let elapsed = now.saturating_duration_since(running.started_at);
        if elapsed >= running.duration {
            return Duration::ZERO;
        }
        // 恢复时保留最短阅读时间，但从暂停时的弧长继续。
        if running.duration == self.remaining {
            self.remaining - elapsed
        } else {
            self.remaining.mul_f64(
                (running.duration - elapsed).as_secs_f64() / running.duration.as_secs_f64(),
            )
        }
    }

    fn fraction_at(&self, now: Instant) -> f32 {
        if self.total.is_zero() {
            0.0
        } else {
            self.remaining_at(now).as_secs_f32() / self.total.as_secs_f32()
        }
    }
}

impl ActiveToast {
    fn appearance(&self, colors: &ThemeColors) -> (&'static str, Rgba, Rgba) {
        let persistent = self.countdown.is_none();
        let (icon, icon_color) = match self.kind {
            ToastKind::Success => ("icons/check.svg", colors.status_success),
            ToastKind::Info if persistent => ("icons/info.svg", colors.icon_accent),
            ToastKind::Info => ("icons/clock.svg", colors.text_muted),
            ToastKind::Warning => ("icons/warning.svg", colors.status_warning),
            ToastKind::Error if persistent => ("icons/x_circle.svg", colors.status_error),
            ToastKind::Error => ("icons/warning.svg", colors.status_error),
        };
        let border = if persistent {
            colors
                .border
                .blend(icon_color.opacity(PERSISTENT_BORDER_TINT))
        } else {
            colors.border
        };
        (icon, icon_color, border)
    }
}

/// 宿主窗口持有的提示层。
pub struct ToastLayer {
    toasts: Vec<ActiveToast>,
    next_id: u64,
    entering_toast: Option<ToastId>,
    countdown_refresh: Option<Task<()>>,
    scroll_handle: ScrollHandle,
}

struct ToastStackContent {
    bubbles: Vec<AnyElement>,
    ids: Vec<ToastId>,
    entering_toast: Option<ToastId>,
}

/// 上一帧的位置只用于接续动画；提示的排版始终由当前布局决定。
struct ToastStackAnimationState {
    positions: Vec<(ToastId, Pixels)>,
    entering_toast: Option<ToastId>,
    started_at: Instant,
    start_offset: Pixels,
}

impl IntoElement for ToastStackContent {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for ToastStackContent {
    type RequestLayoutState = Vec<LayoutId>;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some("toast-stack-content".into())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let child_layout_ids: Vec<_> = self
            .bubbles
            .iter_mut()
            .map(|bubble| bubble.request_layout(window, cx))
            .collect();
        let style = Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            align_items: Some(AlignItems::FlexEnd),
            gap: size(scale::S8, scale::S8),
            size: size(relative(1.0).into(), auto()),
            flex_shrink: 0.0,
            ..Style::default()
        };
        let layout_id = window.request_layout(style, child_layout_ids.iter().copied(), cx);
        (layout_id, child_layout_ids)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        child_layout_ids: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let parent_offset = window.element_offset().y;
        let natural_positions: Vec<_> = self
            .ids
            .iter()
            .zip(child_layout_ids.iter())
            .map(|(toast_id, layout_id)| {
                (
                    *toast_id,
                    window.layout_bounds(*layout_id).top() + parent_offset,
                )
            })
            .collect();
        let rise = window.with_element_state(
            id.unwrap(),
            |state: Option<ToastStackAnimationState>, window| {
                let mut state = state.unwrap_or_else(|| ToastStackAnimationState {
                    positions: Vec::new(),
                    entering_toast: None,
                    started_at: Instant::now(),
                    start_offset: px(0.0),
                });
                if state.entering_toast != self.entering_toast {
                    state.start_offset = self
                        .entering_toast
                        .and_then(|_| {
                            natural_positions
                                .iter()
                                .find_map(|(toast_id, natural_top)| {
                                    state
                                        .positions
                                        .iter()
                                        .find(|(previous_id, _)| previous_id == toast_id)
                                        .map(|(_, previous_top)| *previous_top - *natural_top)
                                })
                        })
                        .unwrap_or(px(0.0));
                    state.started_at = Instant::now();
                    state.entering_toast = self.entering_toast;
                }
                let progress = if self.entering_toast.is_none() || cx.reduce_motion() {
                    1.0
                } else {
                    (state.started_at.elapsed().as_secs_f32() / TOAST_ENTER_DURATION.as_secs_f32())
                        .min(1.0)
                };
                let eased = 1.0 - (1.0 - progress).powi(3);
                let rise = state.start_offset * (1.0 - eased);
                if progress < 1.0 {
                    window.request_animation_frame();
                }
                state.positions = natural_positions
                    .iter()
                    .map(|(toast_id, natural_top)| (*toast_id, *natural_top + rise))
                    .collect();
                (rise, state)
            },
        );
        window.with_element_offset(point(px(0.0), rise), |window| {
            for bubble in &mut self.bubbles {
                bubble.prepaint(window, cx);
            }
        });
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        for bubble in &mut self.bubbles {
            bubble.paint(window, cx);
        }
    }
}

impl Default for ToastLayer {
    fn default() -> Self {
        Self {
            toasts: Vec::new(),
            next_id: 0,
            entering_toast: None,
            countdown_refresh: None,
            scroll_handle: ScrollHandle::new(),
        }
    }
}

impl ToastLayer {
    /// 展示一条提示；`dismiss_after` 之后自动消失（None 表示仅手动关闭）。
    pub fn show(
        &mut self,
        kind: ToastKind,
        message: impl Into<SharedString>,
        actions: Vec<ToastAction>,
        dismiss_after: Option<Duration>,
        cx: &mut Context<Self>,
    ) -> ToastId {
        let id = ToastId {
            layer: cx.entity_id(),
            sequence: self.next_id,
        };
        self.next_id += 1;
        self.toasts.push(ActiveToast {
            id,
            kind,
            message: message.into(),
            actions,
            copied: false,
            copy_feedback_task: None,
            countdown: dismiss_after.map(|total| ToastCountdown {
                total,
                remaining: total,
                running: None,
            }),
        });
        self.entering_toast = Some(id);
        self.scroll_handle.scroll_to_bottom();
        self.start_dismiss_timer(id, Duration::ZERO, cx);
        cx.notify();
        id
    }

    fn start_dismiss_timer(
        &mut self,
        id: ToastId,
        minimum_duration: Duration,
        cx: &mut Context<Self>,
    ) {
        let Some(countdown) = self
            .toasts
            .iter_mut()
            .find(|toast| toast.id == id)
            .and_then(|toast| toast.countdown.as_mut())
        else {
            return;
        };
        if countdown.running.is_some() {
            return;
        }
        let duration = countdown.remaining.max(minimum_duration);
        let started_at = cx.background_executor().now();
        let timer = cx.background_executor().timer(duration);
        let task = cx.spawn(move |this: WeakEntity<Self>, asynccx: &mut AsyncApp| {
            let mut cx = asynccx.clone();
            async move {
                timer.await;
                if let Some(this) = this.upgrade() {
                    this.update(&mut cx, |layer, cx| layer.hide(id, cx));
                }
            }
        });
        countdown.running = Some(RunningCountdown {
            started_at,
            duration,
            _task: task,
        });
        cx.notify();
    }

    fn pause_dismiss_timer(&mut self, id: ToastId, cx: &mut Context<Self>) {
        let Some(countdown) = self
            .toasts
            .iter_mut()
            .find(|toast| toast.id == id)
            .and_then(|toast| toast.countdown.as_mut())
        else {
            return;
        };
        if countdown.running.is_some() {
            countdown.remaining = countdown.remaining_at(cx.background_executor().now());
            countdown.running = None;
            cx.notify();
        }
    }

    /// 关闭指定提示；其计时和复制反馈任务随提示一起结束。
    pub fn hide(&mut self, id: ToastId, cx: &mut Context<Self>) {
        if let Some(index) = self.toasts.iter().position(|toast| toast.id == id) {
            self.toasts.remove(index);
            if self.entering_toast == Some(id) {
                self.entering_toast = None;
            }
            cx.notify();
        }
    }

    pub fn contains(&self, id: ToastId) -> bool {
        self.toasts.iter().any(|toast| toast.id == id)
    }

    /// 标记指定提示已复制：图标短暂切换为对勾，之后自动恢复。
    fn mark_copied(&mut self, id: ToastId, cx: &mut Context<Self>) {
        let Some(toast) = self.toasts.iter_mut().find(|toast| toast.id == id) else {
            return;
        };
        toast.copied = true;
        let timer = cx.background_executor().timer(COPIED_FEEDBACK_DURATION);
        toast.copy_feedback_task = Some(cx.spawn(
            move |this: WeakEntity<Self>, asynccx: &mut AsyncApp| {
                let mut cx = asynccx.clone();
                async move {
                    timer.await;
                    if let Some(this) = this.upgrade() {
                        this.update(&mut cx, |layer, cx| {
                            if let Some(toast) =
                                layer.toasts.iter_mut().find(|toast| toast.id == id)
                            {
                                toast.copied = false;
                                toast.copy_feedback_task = None;
                                cx.notify();
                            }
                        });
                    }
                }
            },
        ));
        cx.notify();
    }
}

impl Render for ToastLayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui_size = window.rem_size();
        let mut stack = div()
            .id("toast-stack")
            .flex()
            .flex_col()
            .items_end()
            .w_full()
            .max_w(px(TOAST_MAX_WIDTH))
            .max_h(relative(1.0))
            .overflow_y_scroll()
            .track_scroll(&self.scroll_handle);
        let mut content = ToastStackContent {
            bubbles: Vec::with_capacity(self.toasts.len()),
            ids: Vec::with_capacity(self.toasts.len()),
            entering_toast: self.entering_toast.filter(|_| self.toasts.len() > 1),
        };

        let now = cx.background_executor().now();
        let reduce_motion = cx.reduce_motion();
        let mut countdown_running = false;
        for toast in &self.toasts {
            let id = toast.id;
            let colors = color::current(cx);
            let (icon, icon_color, border_color) = toast.appearance(colors);

            let mut leading = div()
                .flex()
                .items_center()
                .gap(scale::S6)
                .child(SvgIcon::new(icon).color(icon_color).size(ui_size));
            if let Some(countdown) = &toast.countdown {
                let fraction = countdown.fraction_at(now);
                countdown_running |= countdown.running.is_some() && fraction > 0.0;
                let fraction = if reduce_motion {
                    (fraction * 10.0).ceil() / 10.0
                } else {
                    fraction
                };
                leading = leading.child(
                    div()
                        .debug_selector(move || format!("toast-countdown-{}", id.sequence))
                        .child(ProgressIndicator::new(
                            "countdown",
                            ProgressMode::Determinate(fraction),
                        )),
                );
            }

            let message = toast.message.clone();
            let layer = cx.entity().clone();
            let (copy_icon, copy_color, copy_label) = if toast.copied {
                (
                    "icons/check.svg",
                    color::current(cx).status_success,
                    "已复制",
                )
            } else {
                ("icons/copy.svg", color::current(cx).text_muted, "复制")
            };
            let copy_button = Button::icon(format!("toast-copy-{}", id.sequence), copy_icon)
                .no_occlude()
                .color(copy_color)
                .label(copy_label)
                .on_click(move |_, _window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(message.to_string()));
                    layer.update(cx, |layer, cx| layer.mark_copied(id, cx));
                });

            let layer = cx.entity().clone();
            let close_button =
                Button::icon(format!("toast-close-{}", id.sequence), "icons/close.svg")
                    .no_occlude()
                    .color(color::current(cx).text_muted)
                    .label("关闭")
                    .on_click(move |_, _window, cx| {
                        layer.update(cx, |layer, cx| layer.hide(id, cx));
                    });

            let mut bubble = div()
                .id(("toast-bubble", id.sequence as usize))
                .debug_selector(move || format!("toast-{}", id.sequence))
                .flex()
                .flex_col()
                .flex_shrink_0()
                .gap(scale::S8)
                .p(scale::S10)
                .w_full()
                .max_h(relative(1.0))
                .overflow_hidden()
                .rounded_lg()
                .bg(color::current(cx).surface_background)
                .border_1()
                .border_color(border_color)
                .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                    if *hovered {
                        this.pause_dismiss_timer(id, cx);
                    } else {
                        this.start_dismiss_timer(id, MINIMUM_RESUME_DURATION, cx);
                    }
                }))
                .text_color(color::current(cx).text)
                .child(
                    div()
                        .flex_shrink_0()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(leading)
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap(scale::S6)
                                .child(copy_button)
                                .child(close_button),
                        ),
                )
                .child(
                    div()
                        .id(("toast-message", id.sequence as usize))
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .overflow_y_scroll()
                        .text_color(color::current(cx).text)
                        .child(toast.message.clone()),
                );

            if !toast.actions.is_empty() {
                let mut buttons = div()
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .justify_end()
                    .gap(scale::S6);
                for (index, action) in toast.actions.iter().enumerate() {
                    let on_click = action.on_click.clone();
                    let layer = cx.entity().clone();
                    buttons = buttons.child(
                        div()
                            .debug_selector(move || format!("toast-action-{}-{index}", id.sequence))
                            .child(
                                Button::text(
                                    format!("toast-action-{}-{index}", id.sequence),
                                    action.label.clone(),
                                )
                                .style(ButtonStyle::Solid)
                                .no_occlude()
                                .on_click(move |_, window, cx| {
                                    on_click(window, cx);
                                    layer.update(cx, |layer, cx| layer.hide(id, cx));
                                }),
                            ),
                    );
                }
                bubble = bubble.child(buttons);
            }

            content.bubbles.push(bubble.into_any_element());
            content.ids.push(id);
        }

        if reduce_motion && countdown_running {
            if self.countdown_refresh.is_none() {
                let timer = cx
                    .background_executor()
                    .timer(REDUCED_MOTION_REFRESH_INTERVAL);
                self.countdown_refresh = Some(cx.spawn(async move |layer, cx| {
                    timer.await;
                    let _ = layer.update(cx, |layer, cx| {
                        layer.countdown_refresh = None;
                        cx.notify();
                    });
                }));
            }
        } else {
            self.countdown_refresh = None;
            if countdown_running {
                window.request_animation_frame();
            }
        }

        stack = stack.child(content);

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_end()
            .justify_end()
            .pb(scale::S8)
            .pr(scale::S8)
            .child(stack)
    }
}

#[cfg(test)]
#[path = "test/toast_tests.rs"]
mod test;
