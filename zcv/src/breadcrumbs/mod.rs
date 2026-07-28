//! Breadcrumbs —— Toolbar 中的面包屑导航子项。
//!
//! 显示当前激活 item 的文件路径分段，段间用 `›` 分隔。
//! 所有分段使用一致的 muted 色，不区分最后一个。
//! 订阅 item 的 `UpdateBreadcrumbs` 事件，路径变化时自动刷新。

use gpui::{AnyElement, Context, EventEmitter, Render, Subscription, Window, div, prelude::*};

use crate::workspace::{ItemEvent, ItemHandle};
use crate::workspace::{ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView};
use zcv_theme::{color, space};

pub(crate) struct Breadcrumbs {
    active_item: Option<Box<dyn ItemHandle>>,
    subscription: Option<Subscription>,
}

impl Default for Breadcrumbs {
    fn default() -> Self {
        Self::new()
    }
}

impl Breadcrumbs {
    pub fn new() -> Self {
        Self {
            active_item: None,
            subscription: None,
        }
    }
}

impl EventEmitter<ToolbarItemEvent> for Breadcrumbs {}

impl Render for Breadcrumbs {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let segments = self
            .active_item
            .as_ref()
            .and_then(|item| item.breadcrumbs(cx));

        let mut children: Vec<AnyElement> = Vec::new();

        if let Some((path_segments, _font)) = segments {
            for (i, segment) in path_segments.iter().enumerate() {
                // 段间用 › 分隔（Zed 风格）
                if i > 0 {
                    children.push(
                        div()
                            .text_color(color::current().gray.s[6])
                            .child("›")
                            .into_any_element(),
                    );
                }

                // 所有分段使用一致的 muted 色，不区分最后一个
                children.push(
                    div()
                        .text_color(color::current().gray.s[7])
                        .child(segment.clone())
                        .into_any_element(),
                );
            }
        }

        div()
            .id("breadcrumbs")
            .flex()
            .items_center()
            .flex_1()
            .gap(space::S2)
            .overflow_x_scroll()
            .children(children)
    }
}

impl ToolbarItemView for Breadcrumbs {
    fn set_active_item(
        &mut self,
        active_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        cx.notify();
        self.active_item = None;
        self.subscription = None;

        let Some(item) = active_item else {
            return ToolbarItemLocation::Hidden;
        };

        let this = cx.entity().downgrade();
        self.subscription = Some(item.subscribe_to_item_events(
            _window,
            cx,
            Box::new(move |ItemEvent::UpdateBreadcrumbs, cx| {
                this.update(cx, |this, cx| {
                    cx.notify();
                    if let Some(active_item) = this.active_item.as_ref() {
                        cx.emit(ToolbarItemEvent::ChangeLocation(
                            active_item.breadcrumb_location(cx),
                        ))
                    }
                })
                .ok();
            }),
        ));
        self.active_item = Some(item.boxed_clone());
        item.breadcrumb_location(cx)
    }
}
