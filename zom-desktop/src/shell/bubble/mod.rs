//! BubbleLayer —— 轻量气泡提示层 portal（布局模型 8 / 手册 21）。
//!
//! 管道通知，不让业务 / feature 主动拿 BubbleLayer handle 去调用。
//! 所有请求都通过 `HostEffect::ShowBubble` 进入 shell 解释器，再落到 [`BubbleRuntime`]。

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt as _, Context, Entity, Render, Subscription, Window, deferred, div,
    ease_out_quint, prelude::*, px,
};
use zom_command::commands::bubble;
use zom_command::{BubbleKind, BubbleRequest};

use crate::host_intent::{CommandRequest, HostIntent, HostIntentRequest};
use crate::shell::shared::{CommandBinding, Glyph};
use crate::shell::{CommandTitleLookup, ShortcutLookup};
use crate::theme::{color, radius, space, typography};

const BUBBLE_RIGHT_PX: f32 = 16.0;
const BUBBLE_BOTTOM_PX: f32 = 16.0;
const BUBBLE_FLOAT_PX: f32 = 14.0;
const BUBBLE_ENTER_MS: u64 = 180;
const HOVER_POLL_MS: u64 = 500;
const COPY_ICON: &str = "icons/actions/copy.svg";
/// 复制成功反馈图标。
const CHECK_ICON: &str = "icons/actions/check.svg";

#[derive(Clone)]
struct ActiveBubble {
    request: BubbleRequest,
    expires_at: Option<Instant>,
    seq: u64,
}

/// 每窗口气泡运行态。当前同时最多显示一个气泡；新请求替换旧请求。
pub(crate) struct BubbleRuntime {
    active: Option<ActiveBubble>,
    next_seq: u64,
    hovering: bool,
    dismiss_generation: u64,
    intent_request: Option<HostIntentRequest>,
    copied_seq: Option<u64>,
}

impl BubbleRuntime {
    pub(crate) fn new() -> Self {
        Self {
            active: None,
            next_seq: 0,
            hovering: false,
            dismiss_generation: 0,
            intent_request: None,
            copied_seq: None,
        }
    }

    pub(crate) fn set_intent_request(&mut self, request: HostIntentRequest) {
        self.intent_request = Some(request);
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
                self.dismiss_generation += 1;
                schedule_expiry(seq, self.dismiss_generation, ttl, cx);
            }
            return;
        }

        self.hovering = false;
        self.copied_seq = None;
        self.active = Some(ActiveBubble {
            expires_at,
            request,
            seq,
        });
        cx.notify();
        if let Some(ttl) = ttl {
            self.dismiss_generation += 1;
            schedule_expiry(seq, self.dismiss_generation, ttl, cx);
        }
    }

    fn active(&mut self, cx: &mut Context<Self>) -> Option<ActiveBubble> {
        if self.hovering {
            return self.active.clone();
        }
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

fn schedule_expiry(seq: u64, generation: u64, ttl: Duration, cx: &mut Context<BubbleRuntime>) {
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(ttl).await;
        this.update(cx, |runtime, cx| {
            if runtime.dismiss_generation != generation {
                return;
            }
            let is_current = runtime
                .active
                .as_ref()
                .is_some_and(|active| active.seq == seq);
            if !is_current {
                return;
            }
            if runtime.hovering {
                schedule_expiry(seq, generation, Duration::from_millis(HOVER_POLL_MS), cx);
                return;
            }
            runtime.active = None;
            cx.notify();
        })
        .ok();
    })
    .detach();
}

pub(crate) struct BubbleShell {
    runtime: Entity<BubbleRuntime>,
    title_lookup: CommandTitleLookup,
    shortcut_lookup: ShortcutLookup,
    _runtime_observer: Subscription,
}

impl BubbleShell {
    pub(crate) fn new(
        runtime: Entity<BubbleRuntime>,
        title_lookup: CommandTitleLookup,
        shortcut_lookup: ShortcutLookup,
        cx: &mut Context<Self>,
    ) -> Self {
        let runtime_observer = cx.observe(&runtime, |_, _, cx| cx.notify());
        Self {
            runtime,
            title_lookup,
            shortcut_lookup,
            _runtime_observer: runtime_observer,
        }
    }
}

impl Render for BubbleShell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (active, intent, is_copied) = self.runtime.update(cx, |runtime, cx| {
            let active = runtime.active(cx);
            let copied = runtime.copied_seq;
            let is_copied = active.as_ref().is_some_and(|a| copied == Some(a.seq));
            (active, runtime.intent_request.clone(), is_copied)
        });
        let Some(active) = active else {
            return div().absolute().top_0().left_0().size_full().invisible();
        };

        let runtime = self.runtime.clone();
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .on_mouse_move({
                let runtime = runtime.clone();
                move |_, _, cx| {
                    runtime.update(cx, |this, cx| {
                        if this.hovering {
                            this.hovering = false;
                            this.dismiss_generation += 1;
                            if let Some(ref mut active) = this.active
                                && let Some(ttl_ms) = active.request.ttl_ms
                            {
                                active.expires_at =
                                    Some(Instant::now() + Duration::from_millis(ttl_ms));
                                let generation = this.dismiss_generation;
                                schedule_expiry(
                                    active.seq,
                                    generation,
                                    Duration::from_millis(ttl_ms),
                                    cx,
                                );
                            }
                            cx.notify();
                        }
                    });
                }
            })
            .child(
                deferred(render_bubble(
                    active,
                    runtime,
                    intent,
                    self.title_lookup.clone(),
                    self.shortcut_lookup.clone(),
                    is_copied,
                ))
                .priority(40),
            )
    }
}

fn render_bubble(
    active: ActiveBubble,
    runtime: Entity<BubbleRuntime>,
    intent: Option<HostIntentRequest>,
    title_lookup: CommandTitleLookup,
    shortcut_lookup: ShortcutLookup,
    is_copied: bool,
) -> impl IntoElement {
    let accent = match active.request.kind {
        BubbleKind::Info => color::current().blue.s07,
        BubbleKind::Success => color::current().green.s07,
        BubbleKind::Warning => color::current().yellow.s07,
        BubbleKind::Error => color::current().red.s07,
    };
    let message = active.request.message.clone();
    let icon_path = if is_copied { CHECK_ICON } else { COPY_ICON };

    let copy_glyph = {
        let mut glyph = Glyph::icon("bubble-copy", icon_path);
        if let Some(intent) = intent {
            let intent_req = intent.clone();
            let seq = active.seq;
            let runtime_for_copy = runtime.clone();
            let command_request: CommandRequest = Rc::new(move |window, cx| {
                let invocation = bubble::copy(message.clone());
                intent_req(HostIntent::Command(invocation), window, cx);
                runtime_for_copy.update(cx, |this, cx| {
                    this.copied_seq = Some(seq);
                    cx.notify();
                });
            });
            let binding = CommandBinding {
                id: bubble::COPY.to_string(),
                title: title_lookup.clone(),
                shortcut: shortcut_lookup.clone(),
                request: command_request,
            };
            glyph = glyph.command(binding);
        }
        if is_copied {
            glyph = glyph.color(color::current().green.s07);
        }
        glyph.render()
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
        .on_mouse_move({
            let runtime = runtime.clone();
            move |_, _, cx| {
                runtime.update(cx, |this, cx| {
                    if !this.hovering {
                        this.hovering = true;
                        cx.notify();
                    }
                });
                cx.stop_propagation();
            }
        })
        .child(
            div()
                .flex_none()
                .w(px(3.0))
                .h(px(18.0))
                .rounded(radius::full())
                .bg(accent),
        )
        .child(div().flex_1().min_w_0().child(active.request.message))
        .child(
            div()
                .absolute()
                .top(px(4.0))
                .right(px(4.0))
                .child(copy_glyph),
        )
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
