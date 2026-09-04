//! Breadcrumbs —— Toolbar 中的面包屑导航子项。
//!
//! 文件相对路径作为一个完整分段，后续符号层级才使用 `›` 分隔。
//! 层级过长时保留首尾各六段并折叠中间内容。
//! 订阅 item 的 `UpdateBreadcrumbs` 事件，路径变化时自动刷新。

use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, ClipboardItem, Context, Entity, EventEmitter, Render, Subscription, Window, div,
    prelude::*,
};
use zcv_project::Project;
use zcv_theme::{color, typography};
use zcv_ui::{ButtonLike, TooltipSpec};
use zcv_workspace::{ItemEvent, ItemHandle};
use zcv_workspace::{ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView};

const MAX_SEGMENTS: usize = 12;

pub(crate) struct Breadcrumbs {
    project: Entity<Project>,
    active_item: Option<Box<dyn ItemHandle>>,
    subscription: Option<Subscription>,
}

impl Breadcrumbs {
    pub(crate) fn new(project: Entity<Project>) -> Self {
        Self {
            project,
            active_item: None,
            subscription: None,
        }
    }
}

impl EventEmitter<ToolbarItemEvent> for Breadcrumbs {}

impl Render for Breadcrumbs {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let project = self.project.read(cx);
        let project_root = project.root().map(Path::to_path_buf);
        let segments = self
            .active_item
            .as_ref()
            .and_then(|item| item.breadcrumbs(project_root.as_deref(), cx));
        let copy_path = self
            .active_item
            .as_ref()
            .and_then(|item| item.active_path(cx))
            .and_then(|path| absolute_path(path, project_root.as_deref()));

        let mut children: Vec<AnyElement> = Vec::new();

        if let Some((path_segments, _font)) = segments {
            let path_segments = collapse_middle_segments(path_segments);
            for (i, segment) in path_segments.iter().enumerate() {
                if i > 0 {
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

        let breadcrumbs = ButtonLike::new("breadcrumbs").child(
            div()
                .id("breadcrumbs-content")
                .flex()
                .items_center()
                .gap_1()
                .text_size(typography::ui_size())
                .children(children),
        );

        let breadcrumbs = if let Some(copy_path) = copy_path {
            breadcrumbs
                .tooltip(TooltipSpec::new("右键复制绝对路径"))
                .on_right_click(move |_event, _window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        copy_path.to_string_lossy().into_owned(),
                    ));
                })
        } else {
            breadcrumbs
        };
        let breadcrumbs = div()
            .debug_selector(|| "breadcrumbs-button".into())
            .flex_none()
            .child(breadcrumbs);

        // Toolbar 剩余空间只限制面包屑的最大宽度；交互背景仍按内容宽度绘制。
        div()
            .id("breadcrumbs-viewport")
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .overflow_x_scroll()
            .child(breadcrumbs)
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

impl ToolbarItemView for Breadcrumbs {
    fn set_active_pane_item(
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

        let location = item.breadcrumb_location(cx);
        let this = cx.entity().downgrade();
        self.subscription = Some(item.subscribe_to_item_events(
            cx,
            Box::new(move |event, cx| {
                if matches!(event, ItemEvent::PathChanged | ItemEvent::UpdateBreadcrumbs) {
                    this.update(cx, |_, cx| {
                        cx.notify();
                        cx.emit(ToolbarItemEvent::ChangeLocation(location));
                    })
                    .ok();
                }
            }),
        ));
        self.active_item = Some(item.boxed_clone());
        location
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use gpui::{AppContext, Modifiers, MouseButton, TestAppContext};
    use zcv_editor::Editor;

    use super::*;

    struct TestView;

    impl Render for TestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    struct BreadcrumbTestView {
        breadcrumbs: Entity<Breadcrumbs>,
    }

    impl Render for BreadcrumbTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .debug_selector(|| "breadcrumbs".into())
                .child(self.breadcrumbs.clone())
        }
    }

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

    #[test]
    fn relative_active_path_uses_the_project_root_for_copying() {
        assert_eq!(
            absolute_path(PathBuf::from("src/main.rs"), Some(Path::new("/project"))),
            Some(PathBuf::from("/project/src/main.rs"))
        );
    }

    #[test]
    fn absolute_active_path_is_not_rebased_for_copying() {
        assert_eq!(
            absolute_path(PathBuf::from("/other/main.rs"), Some(Path::new("/project"))),
            Some(PathBuf::from("/other/main.rs"))
        );
    }

    #[gpui::test]
    fn right_clicking_breadcrumbs_copies_the_absolute_active_path(cx: &mut TestAppContext) {
        let editor = cx.new(Editor::single_line);
        editor.update(cx, |editor, cx| {
            editor.set_file_path(PathBuf::from("/project/src/main.rs"), cx);
        });
        let project = cx.new(Project::empty);
        let breadcrumbs = cx.new(|_| Breadcrumbs::new(project));

        let (_, cx) = cx.add_window_view(|window, cx| {
            breadcrumbs.update(cx, |breadcrumbs, cx| {
                let item: &dyn ItemHandle = &editor;
                breadcrumbs.set_active_pane_item(Some(item), window, cx);
            });
            BreadcrumbTestView { breadcrumbs }
        });
        let bounds = cx
            .debug_bounds("breadcrumbs-button")
            .expect("面包屑应参与布局");
        let position = bounds.center();
        cx.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
        cx.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());

        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("/project/src/main.rs".to_string())
        );
    }

    #[gpui::test]
    fn path_change_does_not_read_editor_during_its_update(cx: &mut TestAppContext) {
        let editor = cx.new(Editor::single_line);
        let project = cx.new(Project::empty);
        let breadcrumbs = cx.new(|_| Breadcrumbs::new(project));

        cx.add_window_view(|window, cx| {
            breadcrumbs.update(cx, |breadcrumbs, cx| {
                let item: &dyn ItemHandle = &editor;
                breadcrumbs.set_active_pane_item(Some(item), window, cx);
            });
            editor.update(cx, |editor, cx| {
                editor.set_file_path(PathBuf::from("/project/new.rs"), cx);
            });
            TestView
        });
    }
}
