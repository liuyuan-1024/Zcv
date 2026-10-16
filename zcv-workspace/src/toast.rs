//! Toast —— 工作区内的提示（成功/错误/信息）。
//!
//! 三段式形态：头部行（左侧语义图标，右侧复制/关闭）、内容主体（固定最大宽度内自动换行）、底部操作按钮。
//! 由 Workspace 持有并叠加在根视图上，各条提示独立计时和关闭。
//! 容器自身不注册鼠标事件（不创建 hitbox），不会拦截下方视图的交互。

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AlignItems, AnyElement, App, AsyncApp, Bounds, ClipboardItem, Context, Display, Element,
    ElementId, EntityId, FlexDirection, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    Pixels, Render, ScrollHandle, SharedString, Style, Task, WeakEntity, Window, auto, div, point,
    prelude::*, px, relative, size,
};
use zcv_theme::{color, scale};
use zcv_ui::{Button, ButtonStyle, SvgIcon};

use crate::typography_for_window;

/// 复制反馈的展示时长。
const COPIED_FEEDBACK_DURATION: Duration = Duration::from_secs(2);
/// 恢复计时的最短剩余时长（避免鼠标快速进出悬浮导致 toast 立即消失）。
const MINIMUM_RESUME_DURATION: Duration = Duration::from_millis(800);
/// 新提示从下方进入的时长。
const TOAST_ENTER_DURATION: Duration = Duration::from_millis(280);
/// 内容主体的最大宽度，超长文本在此宽度内自动换行。
const TOAST_MAX_WIDTH: f32 = 384.0;

type ToastClickHandler = Rc<dyn Fn(&mut Window, &mut App) + 'static>;

/// Toast 的语义类型（决定图标与颜色）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Info,
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
    dismiss_task: Option<Task<()>>,
    /// 自动消失的剩余时长；鼠标悬浮时暂停计时并记录。
    dismiss_remaining: Option<Duration>,
    dismiss_started_at: Option<Instant>,
}

/// 全局提示层：Workspace 持有并叠加渲染。
pub(crate) struct ToastLayer {
    toasts: Vec<ActiveToast>,
    next_id: u64,
    entering_toast: Option<ToastId>,
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

impl ToastLayer {
    pub(crate) fn new() -> Self {
        Self {
            toasts: Vec::new(),
            next_id: 0,
            entering_toast: None,
            scroll_handle: ScrollHandle::new(),
        }
    }

    /// 展示一条提示；`dismiss_after` 之后自动消失（None 表示仅手动关闭）。
    pub(crate) fn show(
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
            dismiss_task: None,
            dismiss_remaining: dismiss_after,
            dismiss_started_at: None,
        });
        self.entering_toast = Some(id);
        self.scroll_handle.scroll_to_bottom();
        self.start_dismiss_timer(id, cx);
        cx.notify();
        id
    }

    fn start_dismiss_timer(&mut self, id: ToastId, cx: &mut Context<Self>) {
        let Some(toast) = self.toasts.iter_mut().find(|toast| toast.id == id) else {
            return;
        };
        let Some(duration) = toast.dismiss_remaining else {
            return;
        };
        toast.dismiss_started_at = Some(Instant::now());
        let timer = cx.background_executor().timer(duration);
        toast.dismiss_task = Some(cx.spawn(
            move |this: WeakEntity<Self>, asynccx: &mut AsyncApp| {
                let mut cx = asynccx.clone();
                async move {
                    timer.await;
                    if let Some(this) = this.upgrade() {
                        this.update(&mut cx, |layer, cx| layer.hide(id, cx));
                    }
                }
            },
        ));
    }

    /// 暂停自动消失计时（鼠标悬浮常显）：扣减已流逝时长并取消计时任务。
    fn pause_dismiss_timer(&mut self, id: ToastId) {
        let Some(toast) = self.toasts.iter_mut().find(|toast| toast.id == id) else {
            return;
        };
        let Some(started_at) = toast.dismiss_started_at.take() else {
            return;
        };
        if let Some(remaining) = toast.dismiss_remaining.as_mut() {
            *remaining = remaining.saturating_sub(started_at.elapsed());
            if *remaining < MINIMUM_RESUME_DURATION {
                *remaining = MINIMUM_RESUME_DURATION;
            }
        }
        toast.dismiss_task = None;
    }

    /// 恢复自动消失计时（鼠标离开后）。
    fn restart_dismiss_timer(&mut self, id: ToastId, cx: &mut Context<Self>) {
        if self
            .toasts
            .iter()
            .any(|toast| toast.id == id && toast.dismiss_started_at.is_none())
        {
            self.start_dismiss_timer(id, cx);
        }
    }

    /// 关闭指定提示；其计时和复制反馈任务随提示一起结束。
    pub(crate) fn hide(&mut self, id: ToastId, cx: &mut Context<Self>) {
        if let Some(index) = self.toasts.iter().position(|toast| toast.id == id) {
            self.toasts.remove(index);
            if self.entering_toast == Some(id) {
                self.entering_toast = None;
            }
            cx.notify();
        }
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let typography = typography_for_window(window, cx);
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

        for toast in &self.toasts {
            let id = toast.id;
            let (icon, icon_color) = match toast.kind {
                ToastKind::Success => ("icons/check.svg", color::current(cx).status_success),
                ToastKind::Info => ("icons/clock.svg", color::current(cx).text_muted),
                ToastKind::Error => ("icons/warning.svg", color::current(cx).status_error),
            };

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
                .border_color(color::current(cx).border)
                .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                    if *hovered {
                        this.pause_dismiss_timer(id);
                    } else {
                        this.restart_dismiss_timer(id, cx);
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
                        .child(
                            SvgIcon::new(icon)
                                .color(icon_color)
                                .size(typography.ui_size()),
                        )
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
