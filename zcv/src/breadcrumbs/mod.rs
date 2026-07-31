//! Breadcrumbs —— Toolbar 中的面包屑导航子项。
//!
//! 与 Zed 一致，文件相对路径作为一个完整分段，后续符号层级才使用 `›` 分隔。
//! 层级过长时保留首尾各六段并折叠中间内容。
//! 订阅 item 的 `UpdateBreadcrumbs` 事件，路径变化时自动刷新。

use gpui::{AnyElement, Context, EventEmitter, Render, Subscription, Window, div, prelude::*};

use crate::workspace::{ItemEvent, ItemHandle};
use crate::workspace::{ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView};
use zcv_theme::{color, typography};

const MAX_SEGMENTS: usize = 12;

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
            let path_segments = collapse_middle_segments(path_segments);
            for (i, segment) in path_segments.iter().enumerate() {
                if i > 0 {
                    children.push(
                        div()
                            .text_color(color::current().text_disabled)
                            .child("›")
                            .into_any_element(),
                    );
                }

                children.push(
                    div()
                        .text_color(color::current().text_muted)
                        .child(segment.replace('\n', " "))
                        .into_any_element(),
                );
            }
        }

        div()
            .id("breadcrumbs")
            .flex()
            .items_center()
            .flex_1()
            .gap_1()
            .overflow_x_scroll()
            .text_size(typography::ui())
            .children(children)
    }
}

fn collapse_middle_segments(mut segments: Vec<gpui::SharedString>) -> Vec<gpui::SharedString> {
    let prefix_end = segments.len().min(MAX_SEGMENTS / 2);
    let suffix_start = prefix_end.max(segments.len().saturating_sub(MAX_SEGMENTS / 2));
    if suffix_start > prefix_end {
        segments.splice(prefix_end..suffix_start, ["⋯".into()]);
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_breadcrumbs_keep_the_same_prefix_and_suffix_as_zed() {
        let segments = (0..15)
            .map(|index| index.to_string().into())
            .collect::<Vec<gpui::SharedString>>();
        let collapsed = collapse_middle_segments(segments);

        assert_eq!(
            collapsed
                .iter()
                .map(|segment| segment.as_ref())
                .collect::<Vec<_>>(),
            [
                "0", "1", "2", "3", "4", "5", "⋯", "9", "10", "11", "12", "13", "14"
            ]
        );
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
