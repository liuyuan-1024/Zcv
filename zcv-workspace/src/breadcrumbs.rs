use std::path::{Path, PathBuf};

use gpui::{AnyElement, ClipboardItem, Context, Entity, Render, Window, div, prelude::*};
use zcv_project::Project;
use zcv_theme::{color, typography};
use zcv_ui::{ButtonLike, TooltipSpec};

use crate::ItemHandle;

const MAX_SEGMENTS: usize = 12;

/// 文档视图顶部的路径导航。活动 Item 由所属视图显式更新。
pub struct Breadcrumbs {
    project: Option<Entity<Project>>,
    item: Option<Box<dyn ItemHandle>>,
}

impl Breadcrumbs {
    pub fn new(project: Entity<Project>) -> Self {
        Self {
            project: Some(project),
            item: None,
        }
    }

    pub fn without_project() -> Self {
        Self {
            project: None,
            item: None,
        }
    }

    pub fn set_item(&mut self, item: Option<&dyn ItemHandle>, cx: &mut Context<Self>) {
        self.item = item.map(ItemHandle::boxed_clone);
        cx.notify();
    }
}

impl Render for Breadcrumbs {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let project_root = self
            .project
            .as_ref()
            .and_then(|project| project.read(cx).root().map(Path::to_path_buf));
        let segments = self
            .item
            .as_ref()
            .and_then(|item| item.breadcrumbs(project_root.as_deref(), cx));
        let copy_path = self
            .item
            .as_ref()
            .and_then(|item| item.active_path(cx))
            .and_then(|path| absolute_path(path, project_root.as_deref()));
        let mut children: Vec<AnyElement> = Vec::new();

        if let Some((segments, _font)) = segments {
            for (index, segment) in collapse_middle_segments(segments).iter().enumerate() {
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
                .text_size(typography::ui_size())
                .children(children),
        );
        let button = if let Some(path) = copy_path {
            button
                .tooltip(TooltipSpec::new("右键复制绝对路径"))
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

fn absolute_path(path: PathBuf, project_root: Option<&Path>) -> Option<PathBuf> {
    path.is_absolute()
        .then_some(path.clone())
        .or_else(|| project_root.map(|root| root.join(path)))
}

fn collapse_middle_segments(mut segments: Vec<gpui::SharedString>) -> Vec<gpui::SharedString> {
    let prefix_end = segments.len().min(MAX_SEGMENTS / 2);
    let suffix_start = prefix_end.max(segments.len().saturating_sub(MAX_SEGMENTS / 2));
    if suffix_start > prefix_end {
        segments.splice(prefix_end..suffix_start, ["⋯".into()]);
    }
    segments
}
