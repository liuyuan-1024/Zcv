//! BubbleLayer —— 轻量气泡提示层 portal（布局模型 8 / 手册 21）。
//!
//! 管道通知，不让业务 / feature 主动拿 BubbleLayer handle 去调用。
//! 所有请求都通过 `HostEffect::ShowBubble` 进入 shell 解释器，再落到 [`BubbleRuntime`]。

use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt as _, Context, Entity, Render, Subscription, Window, deferred, div,
    ease_out_quint, prelude::*, px,
};
use zom_command::{BubbleKind, BubbleRequest};

use crate::theme::{color, radius, space, typography};

const BUBBLE_RIGHT_PX: f32 = 16.0;
const BUBBLE_BOTTOM_PX: f32 = 16.0;
const BUBBLE_FLOAT_PX: f32 = 14.0;
const BUBBLE_ENTER_MS: u64 = 180;

#[derive(Clone)]
struct ActiveBubble {
    request: BubbleRequest,
    expires_at: Option<Instant>,
    seq: u64,
}

/// 每窗口气泡运行态。当前同时最多显示一个气泡；新请求替换旧请求。
#[derive(Default)]
pub(crate) struct BubbleRuntime {
    active: Option<ActiveBubble>,
    next_seq: u64,
}

impl BubbleRuntime {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn push(&mut self, request: BubbleRequest, cx: &mut Context<Self>) {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        let expires_at = expires_at(request.ttl_ms);
        let ttl = request.ttl_ms.map(Duration::from_millis);

        if let (Some(active), Some(next_key)) = (&mut self.active, request.dedupe_key.as_ref())
            && active.request.dedupe_key.as_ref() == Some(next_key)
        {
            active.request = request;
            active.expires_at = expires_at;
            active.seq = seq;
            cx.notify();
            if let Some(ttl) = ttl {
                schedule_expiry(seq, ttl, cx);
            }
            return;
        }

        self.active = Some(ActiveBubble {
            expires_at,
            request,
            seq,
        });
        cx.notify();
        if let Some(ttl) = ttl {
            schedule_expiry(seq, ttl, cx);
        }
    }

    fn active(&mut self, cx: &mut Context<Self>) -> Option<ActiveBubble> {
        let expired = self
            .active
            .as_ref()
            .and_then(|active| active.expires_at)
            .is_some_and(|expires_at| Instant::now() >= expires_at);
        if expired {
            self.active = None;
            cx.notify();
        }
        self.active.clone()
    }
}

fn schedule_expiry(seq: u64, ttl: Duration, cx: &mut Context<BubbleRuntime>) {
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(ttl).await;
        this.update(cx, |runtime, cx| {
            let should_dismiss = runtime
                .active
                .as_ref()
                .is_some_and(|active| active.seq == seq);
            if should_dismiss {
                runtime.active = None;
                cx.notify();
            }
        })
        .ok();
    })
    .detach();
}

pub(crate) struct BubbleShell {
    runtime: Entity<BubbleRuntime>,
    _runtime_observer: Subscription,
}

impl BubbleShell {
    pub(crate) fn new(runtime: Entity<BubbleRuntime>, cx: &mut Context<Self>) -> Self {
        let runtime_observer = cx.observe(&runtime, |_, _, cx| cx.notify());
        Self {
            runtime,
            _runtime_observer: runtime_observer,
        }
    }
}

impl Render for BubbleShell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.runtime.update(cx, |runtime, cx| runtime.active(cx));
        let Some(active) = active else {
            return div().absolute().top_0().left_0().size_full().invisible();
        };

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(deferred(render_bubble(active)).priority(40))
    }
}

fn render_bubble(active: ActiveBubble) -> impl IntoElement {
    let accent = match active.request.kind {
        BubbleKind::Info => color::current().blue.s07,
        BubbleKind::Success => color::current().green.s07,
        BubbleKind::Warning => color::current().yellow.s07,
        BubbleKind::Error => color::current().red.s07,
    };

    div()
        .id(("bubble", active.seq))
        .absolute()
        .right(px(BUBBLE_RIGHT_PX))
        .bottom(px(BUBBLE_BOTTOM_PX))
        .w(px(420.0))
        .flex()
        .items_center()
        .gap_2()
        .rounded(radius::r4())
        .border_1()
        .border_color(color::current().gray.s05)
        .bg(color::current().gray.s03)
        .px(space::s12())
        .py(space::s8())
        .text_size(typography::ui())
        .text_color(color::current().gray.s09)
        .child(
            div()
                .flex_none()
                .w(px(3.0))
                .h(px(18.0))
                .rounded(radius::full())
                .bg(accent),
        )
        .child(div().flex_1().min_w_0().child(active.request.message))
        .with_animation(
            ("bubble-enter", active.seq),
            Animation::new(Duration::from_millis(BUBBLE_ENTER_MS)).with_easing(ease_out_quint()),
            |bubble, delta| {
                let bottom = BUBBLE_BOTTOM_PX - ((1.0 - delta) * BUBBLE_FLOAT_PX);
                bubble.bottom(px(bottom)).opacity(delta)
            },
        )
}

fn expires_at(ttl_ms: Option<u64>) -> Option<Instant> {
    ttl_ms.map(|ttl| Instant::now() + Duration::from_millis(ttl))
}
