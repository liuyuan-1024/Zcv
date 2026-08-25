//! Toast —— 一次性全局提示（成功/错误/信息）。
//!
//! 对齐 Zed 通知的三段式形态：头部行（左侧语义图标，右侧复制/关闭）、内容主体（固定最大宽度内自动换行）、底部操作按钮。
//! 由 Workspace 持有并叠加在根视图上，默认一段时间后自动消失。
//! 容器自身不注册鼠标事件（不创建 hitbox），不会拦截下方视图的交互。

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    App, AsyncApp, BoxShadow, ClipboardItem, Context, MouseButton, Render, SharedString, Task,
    WeakEntity, Window, div, hsla, point, prelude::*, px, relative,
};
use zcv_theme::{color, space, typography};
use zcv_ui::{Button, SvgIcon};

/// 复制反馈的展示时长。
const COPIED_FEEDBACK_DURATION: Duration = Duration::from_secs(2);
/// 恢复计时的最短剩余时长（避免鼠标快速进出悬浮导致 toast 立即消失）。
const MINIMUM_RESUME_DURATION: Duration = Duration::from_millis(800);
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

/// Toast 上的操作按钮（如"重试"）：点击执行后关闭当前 toast。
#[derive(Clone)]
pub struct ToastAction {
    pub label: SharedString,
    pub on_click: ToastClickHandler,
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

/// 正在展示的 toast。
struct ActiveToast {
    kind: ToastKind,
    message: SharedString,
    action: Option<ToastAction>,
    /// 复制后短暂展示"已复制"反馈。
    copied: bool,
}

/// 全局提示层：Workspace 持有并叠加渲染。
pub(crate) struct ToastLayer {
    toast: Option<ActiveToast>,
    _dismiss_task: Option<Task<()>>,
    /// 自动消失的剩余时长；鼠标悬浮时暂停计时并记录，离开后按剩余时长恢复。
    dismiss_remaining: Option<Duration>,
    /// 当前计时开始时刻（暂停时用已流逝时长扣减剩余）。
    dismiss_started_at: Option<Instant>,
}

impl ToastLayer {
    pub(crate) fn new() -> Self {
        Self {
            toast: None,
            _dismiss_task: None,
            dismiss_remaining: None,
            dismiss_started_at: None,
        }
    }

    /// 展示一条提示；`dismiss_after` 之后自动消失（None 表示仅手动关闭）。
    pub(crate) fn show(
        &mut self,
        kind: ToastKind,
        message: impl Into<SharedString>,
        action: Option<ToastAction>,
        dismiss_after: Option<Duration>,
        cx: &mut Context<Self>,
    ) {
        self.toast = Some(ActiveToast {
            kind,
            message: message.into(),
            action,
            copied: false,
        });
        self.dismiss_remaining = dismiss_after;
        self.start_dismiss_timer(cx);
        cx.notify();
    }

    /// 启动自动消失计时（gpui 无延时 API：后台线程睡眠后回 UI 线程关闭，不阻塞其他任务）。
    fn start_dismiss_timer(&mut self, cx: &mut Context<Self>) {
        self._dismiss_task = None;
        let Some(duration) = self.dismiss_remaining else {
            return;
        };
        self.dismiss_started_at = Some(Instant::now());
        let timer = cx
            .background_executor()
            .spawn(async move { std::thread::sleep(duration) });
        self._dismiss_task = Some(cx.spawn(|this: WeakEntity<Self>, asynccx: &mut AsyncApp| {
            let mut cx = asynccx.clone();
            async move {
                timer.await;
                if let Some(this) = this.upgrade() {
                    this.update(&mut cx, |layer, cx| layer.hide(cx)).ok();
                }
            }
        }));
    }

    /// 暂停自动消失计时（鼠标悬浮常显）：扣减已流逝时长并取消计时任务。
    fn pause_dismiss_timer(&mut self, cx: &mut Context<Self>) {
        let Some(started_at) = self.dismiss_started_at.take() else {
            return;
        };
        if let Some(remaining) = self.dismiss_remaining.as_mut() {
            *remaining = remaining.saturating_sub(started_at.elapsed());
            if *remaining < MINIMUM_RESUME_DURATION {
                *remaining = MINIMUM_RESUME_DURATION;
            }
        }
        self._dismiss_task = None;
        cx.notify();
    }

    /// 恢复自动消失计时（鼠标离开后）。
    fn restart_dismiss_timer(&mut self, cx: &mut Context<Self>) {
        self.start_dismiss_timer(cx);
        cx.notify();
    }

    /// 关闭当前 toast。
    pub(crate) fn hide(&mut self, cx: &mut Context<Self>) {
        self._dismiss_task = None;
        self.dismiss_remaining = None;
        self.dismiss_started_at = None;
        self.toast = None;
        cx.notify();
    }

    /// 标记当前 toast 已复制：图标短暂切换为对勾，之后自动恢复。
    fn mark_copied(&mut self, cx: &mut Context<Self>) {
        let Some(toast) = self.toast.as_mut() else {
            return;
        };
        toast.copied = true;
        // 反馈时长结束后恢复，与自动关闭共用后台线程睡眠的方案。
        let timer = cx
            .background_executor()
            .spawn(async move { std::thread::sleep(COPIED_FEEDBACK_DURATION) });
        cx.spawn(|this: WeakEntity<Self>, asynccx: &mut AsyncApp| {
            let mut cx = asynccx.clone();
            async move {
                timer.await;
                if let Some(this) = this.upgrade() {
                    this.update(&mut cx, |layer, cx| {
                        if let Some(toast) = layer.toast.as_mut() {
                            toast.copied = false;
                        }
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
        cx.notify();
    }
}

impl Render for ToastLayer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let Some(toast) = &self.toast else {
            return div().into_any_element();
        };
        let (icon, icon_color) = match toast.kind {
            ToastKind::Success => ("icons/check.svg", color::current(cx).status_success),
            ToastKind::Info => ("icons/clock.svg", color::current(cx).text_muted),
            ToastKind::Error => ("icons/warning.svg", color::current(cx).status_error),
        };

        // 头部右侧的复制按钮：点击写入剪贴板，短暂切换为"已复制"对勾。
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
        let copy_button = Button::icon("toast-copy", copy_icon)
            .color(copy_color)
            .label(copy_label)
            .on_click(move |_, _window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(message.to_string()));
                layer.update(cx, |layer, cx| layer.mark_copied(cx));
            });

        let layer = cx.entity().clone();
        let close_button = Button::icon("toast-close", "icons/close.svg")
            .color(color::current(cx).text_muted)
            .label("关闭")
            .on_click(move |_, _window, cx| {
                layer.update(cx, |layer, cx| layer.hide(cx));
            });

        // surface 背景 + 大圆角 + ModalSurface 层级阴影；内容在最大宽度内自动换行。
        let mut bubble = div()
            .id("toast-bubble")
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap(space::S8)
            .p(space::S10)
            .w_full()
            .max_w(px(TOAST_MAX_WIDTH))
            .max_h(relative(1.0))
            .overflow_hidden()
            .rounded_lg()
            .bg(color::current(cx).surface_background)
            .border_1()
            .border_color(color::current(cx).border_variant)
            // 鼠标悬浮时暂停自动消失计时（toast 常显），离开后按剩余时长恢复。
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if *hovered {
                    this.pause_dismiss_timer(cx);
                } else {
                    this.restart_dismiss_timer(cx);
                }
            }))
            // ModalSurface 阴影（对齐 Zed elevation_3），让 toast 悬浮于内容之上。
            .shadow(vec![
                BoxShadow {
                    color: hsla(0., 0., 0., 0.12),
                    offset: point(px(0.), px(2.)),
                    blur_radius: px(3.),
                    spread_radius: px(0.),
                },
                BoxShadow {
                    color: hsla(0., 0., 0., 0.08),
                    offset: point(px(0.), px(3.)),
                    blur_radius: px(6.),
                    spread_radius: px(0.),
                },
                BoxShadow {
                    color: hsla(0., 0., 0., 0.04),
                    offset: point(px(0.), px(6.)),
                    blur_radius: px(12.),
                    spread_radius: px(0.),
                },
                BoxShadow {
                    color: hsla(0., 0., 0., 0.12),
                    offset: point(px(0.), px(1.)),
                    blur_radius: px(0.),
                    spread_radius: px(0.),
                },
            ])
            .text_color(color::current(cx).text)
            // 第一部分：头部行（左侧语义图标，右侧复制/关闭，从左到右）。
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(SvgIcon::new(icon).color(icon_color).size(typography::ui()))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(space::S6)
                            .child(copy_button)
                            .child(close_button),
                    ),
            )
            // 第二部分：内容主体，继承根元素字体/字号，在固定最大宽度内自动换行。
            .child(
                div()
                    .id("toast-message")
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_y_scroll()
                    .text_color(color::current(cx).text)
                    .child(toast.message.clone()),
            );

        // 第三部分：操作按钮（如"重试"），点击执行回调并关闭 toast。
        if let Some(action) = &toast.action {
            let label = action.label.clone();
            let on_click = action.on_click.clone();
            let layer = cx.entity().clone();
            bubble = bubble.child(
                div()
                    .flex_shrink_0()
                    .text_color(color::current(cx).icon_accent)
                    .child(label)
                    .on_mouse_up(MouseButton::Left, move |_, window, cx| {
                        on_click(window, cx);
                        layer.update(cx, |layer, cx| layer.hide(cx));
                    }),
            );
        }

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_end()
            .justify_end()
            .pb(space::S8)
            .pr(space::S8)
            .child(bubble)
            .into_any_element()
    }
}
