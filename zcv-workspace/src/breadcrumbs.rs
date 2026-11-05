use std::path::PathBuf;

use gpui::{
    AnyElement, ClipboardItem, Context, Entity, Render, SharedString, Subscription, Window, div,
    prelude::*,
};
use zcv_path::DisplayPath;
use zcv_project::{Project, ProjectEvent};
use zcv_theme::color;
use zcv_ui::{ButtonLike, TooltipSpec};

use crate::{ItemEvent, ItemHandle, typography_for_window};

const MAX_SEGMENTS: usize = 12;

/// 文档视图顶部的路径导航。
///
/// Breadcrumbs 是根段的唯一所有者：
/// 活动 Item 只提供根内相对路径，根标签的消歧、更新与多根显示均由本组件根据 Project 状态统一处理。
pub struct Breadcrumbs {
    project: Entity<Project>,
    item: Option<Box<dyn ItemHandle>>,
    _project_subscription: Subscription,
    _item_subscription: Option<Subscription>,
}

impl Breadcrumbs {
    pub fn new(project: Entity<Project>, cx: &mut Context<Self>) -> Self {
        let _project_subscription = cx.subscribe(&project, |_, _, event, cx| {
            if matches!(
                event,
                ProjectEvent::WorktreeAdded { .. }
                    | ProjectEvent::WorktreeRemoved { .. }
                    | ProjectEvent::WorktreeRootChanged { .. }
            ) {
                cx.notify();
            }
        });
        Self {
            project,
            item: None,
            _project_subscription,
            _item_subscription: None,
        }
    }

    pub fn set_item(&mut self, item: Option<&dyn ItemHandle>, cx: &mut Context<Self>) {
        self._item_subscription = None;
        self.item = item.map(ItemHandle::boxed_clone);
        if let Some(item) = self.item.as_deref() {
            let this = cx.entity().downgrade();
            self._item_subscription = Some(item.subscribe_to_item_events(
                cx,
                Box::new(move |event, cx| {
                    if matches!(
                        event,
                        ItemEvent::PathChanged
                            | ItemEvent::UpdateTab
                            | ItemEvent::UpdateBreadcrumbs
                    ) {
                        this.update(cx, |_, cx| cx.notify()).ok();
                    }
                }),
            ));
        }
        cx.notify();
    }
}

impl Render for Breadcrumbs {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let typography = typography_for_window(window, cx);
        let active_path = self.item.as_ref().and_then(|item| item.active_path(cx));
        let (display_path, show_root) = {
            let project = self.project.read(cx);
            let display_path = active_path
                .as_deref()
                .and_then(|path| project.display_path_for_path(path));
            (display_path, project.has_multiple_roots())
        };
        // Item 只负责根内相对路径；根段由 Breadcrumbs 统一添加。
        let item_path = display_path
            .as_ref()
            .map(|path| path.relative().as_path())
            .or(active_path.as_deref());
        let segments = self
            .item
            .as_ref()
            .and_then(|item| item.breadcrumbs(item_path, cx));
        let copy_path = active_path.and_then(|path| absolute_path(path, display_path.as_ref()));
        let mut children: Vec<AnyElement> = Vec::new();

        if let Some((segments, _font)) = segments {
            let segments = breadcrumb_segments(segments, display_path.as_ref(), show_root);
            for (index, segment) in segments.iter().enumerate() {
                if index > 0 {
                    children.push(
                        div()
                            .text_color(color::current(cx).text_disabled)
                            .child("›")
                            .into_any_element(),
                    );
                }
                children.push(
                    div()
                        .text_color(color::current(cx).text_muted)
                        .child(segment.replace('\n', " "))
                        .into_any_element(),
                );
            }
        }

        let button = ButtonLike::new("breadcrumbs").child(
            div()
                .id("breadcrumbs-content")
                .flex()
                .items_center()
                .gap_1()
                .text_size(typography.ui_size())
                .children(children),
        );
        let button = if let Some(path) = copy_path {
            button
                .tooltip(TooltipSpec::from_lines(["右键复制绝对路径"]))
                .on_right_click(move |_event, _window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        path.to_string_lossy().into_owned(),
                    ));
                })
        } else {
            button
        };

        div()
            .id("breadcrumbs-viewport")
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .overflow_x_scroll()
            .child(
                div()
                    .debug_selector(|| "breadcrumbs-button".into())
                    .child(button),
            )
    }
}

fn absolute_path(path: PathBuf, display_path: Option<&DisplayPath>) -> Option<PathBuf> {
    if path.is_absolute() {
        Some(path)
    } else {
        display_path.map(|display_path| {
            display_path
                .root()
                .full_path
                .as_path()
                .join(display_path.relative().as_path())
        })
    }
}

fn collapse_middle_segments(mut segments: Vec<SharedString>) -> Vec<SharedString> {
    let prefix_end = segments.len().min(MAX_SEGMENTS / 2);
    let suffix_start = prefix_end.max(segments.len().saturating_sub(MAX_SEGMENTS / 2));
    if suffix_start > prefix_end {
        segments.splice(prefix_end..suffix_start, ["⋯".into()]);
    }
    segments
}

fn breadcrumb_segments(
    segments: Vec<SharedString>,
    display_path: Option<&DisplayPath>,
    show_root: bool,
) -> Vec<SharedString> {
    let mut segments = collapse_middle_segments(segments);
    if show_root && let Some(display_path) = display_path {
        segments.insert(
            0,
            SharedString::from(display_path.root().label().to_string()),
        );
    }
    segments
}

#[cfg(test)]
#[path = "test/breadcrumbs_tests.rs"]
mod tests;
