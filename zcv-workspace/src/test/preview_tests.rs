use gpui::{Focusable, TestAppContext, px};

use super::*;
use zcv_fs_watch::FsWatcher;
use zcv_project::Project;

use crate::Breadcrumbs;

struct DiagramProvider;

struct LaterDiagramProvider;

struct StandaloneDiagramProvider;

struct PreviewViewportTestView {
    focus: FocusHandle,
    viewport: PreviewViewport,
}

impl Render for PreviewViewportTestView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.viewport.render(
            Some(Size {
                width: px(100.),
                height: px(100.),
            }),
            |_| div().size_full().into_any_element(),
            PreviewViewportOptions::new(
                &self.focus,
                "PreviewViewportTest",
                "preview-viewport-test-scroll",
                "preview-viewport-test-horizontal-scroll",
                Rgba::default(),
            ),
            window,
            cx,
        )
    }
}

impl PreviewProvider for DiagramProvider {
    fn supports(&self, path: &Path, _cx: &App) -> bool {
        path.extension()
            .is_some_and(|extension| extension == "diagram")
    }

    fn mode(&self) -> PreviewMode {
        PreviewMode::Source
    }

    fn presentation(&self) -> PreviewPresentation {
        PreviewPresentation::Canvas
    }

    fn create(&self, _document: PreviewDocument, _cx: &mut App) -> Box<dyn ItemHandle> {
        panic!("注册表匹配测试不应创建视图")
    }
}

impl PreviewProvider for LaterDiagramProvider {
    fn supports(&self, path: &Path, _cx: &App) -> bool {
        path.extension()
            .is_some_and(|extension| extension == "diagram")
    }

    fn mode(&self) -> PreviewMode {
        PreviewMode::Source
    }

    fn presentation(&self) -> PreviewPresentation {
        PreviewPresentation::Canvas
    }

    fn create(&self, _document: PreviewDocument, _cx: &mut App) -> Box<dyn ItemHandle> {
        panic!("注册表优先级测试不应创建视图")
    }
}

impl PreviewProvider for StandaloneDiagramProvider {
    fn supports(&self, path: &Path, _cx: &App) -> bool {
        path.extension()
            .is_some_and(|extension| extension == "diagram")
    }

    fn mode(&self) -> PreviewMode {
        PreviewMode::Standalone
    }

    fn presentation(&self) -> PreviewPresentation {
        PreviewPresentation::Canvas
    }

    fn create(&self, _document: PreviewDocument, _cx: &mut App) -> Box<dyn ItemHandle> {
        panic!("注册表模式过滤测试不应创建视图")
    }
}

#[gpui::test]
fn provider_is_discovered_and_duplicate_registration_is_ignored(cx: &mut TestAppContext) {
    cx.update(|cx| {
        register(DiagramProvider, cx);
        register(DiagramProvider, cx);
    });

    cx.read(|cx| {
        let provider = source_provider_for(Path::new("architecture.diagram"), cx)
            .expect("新格式应由注册的 Provider 匹配");
        assert!(provider.supports(Path::new("architecture.diagram"), cx));
        assert!(source_provider_for(Path::new("architecture.txt"), cx).is_none());
        assert_eq!(
            cx.global::<ProviderRegistry<RegisteredPreview>>()
                .providers
                .len(),
            1
        );
    });
}

#[gpui::test]
fn last_registered_matching_provider_takes_priority(cx: &mut TestAppContext) {
    cx.update(|cx| {
        register(DiagramProvider, cx);
        register(LaterDiagramProvider, cx);
    });

    cx.read(|cx| {
        let selected = source_provider_for(Path::new("architecture.diagram"), cx).unwrap();
        let registry = cx.global::<ProviderRegistry<RegisteredPreview>>();
        let RegisteredPreview::Tab(last) = registry.providers.last().unwrap().provider.as_ref()
        else {
            panic!("最后注册的应为标签页预览");
        };
        assert!(Arc::ptr_eq(&selected, last));
    });
}

#[gpui::test]
fn provider_priority_is_scoped_to_the_requested_preview_mode(cx: &mut TestAppContext) {
    cx.update(|cx| {
        register(DiagramProvider, cx);
        register(StandaloneDiagramProvider, cx);
    });

    cx.read(|cx| {
        assert_eq!(
            source_provider_for(Path::new("architecture.diagram"), cx)
                .unwrap()
                .mode(),
            PreviewMode::Source
        );
        assert_eq!(
            standalone_provider_for(Path::new("architecture.diagram"), cx)
                .unwrap()
                .mode(),
            PreviewMode::Standalone
        );
    });
}

#[test]
fn centered_scroll_offset_tracks_scaled_content_before_layout() {
    assert_eq!(centered_scroll_offset(px(800.), 1.), px(0.));
    assert_eq!(centered_scroll_offset(px(800.), 1.5), px(-200.));
    assert_eq!(centered_scroll_offset(px(800.), 0.5), px(0.));
}

#[gpui::test]
fn viewport_centers_only_after_scroll_layout_is_available(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| PreviewViewportTestView {
        focus: cx.focus_handle(),
        viewport: PreviewViewport::new(),
    });

    cx.refresh().expect("预览视口应完成首次绘制");

    cx.read_entity(&view, |view, _| {
        assert!(!view.viewport.center_after_layout.get());
    });
}

struct ProbeItem {
    focus: FocusHandle,
    is_preview: bool,
    source_item: Option<Box<dyn ItemHandle>>,
}

impl EventEmitter<()> for ProbeItem {}

impl Focusable for ProbeItem {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ProbeItem {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl Item for ProbeItem {
    type Event = ();

    fn tab_content_text(&self, _cx: &App) -> gpui::SharedString {
        "预览探针".into()
    }

    fn as_preview_item(
        &self,
        self_handle: &Entity<Self>,
        _cx: &App,
    ) -> Option<Box<dyn PreviewItemHandle>> {
        self.is_preview
            .then(|| Box::new(self_handle.clone()) as Box<dyn PreviewItemHandle>)
    }
}

impl PreviewItem for ProbeItem {
    fn source_item(&self, _cx: &App) -> Option<Box<dyn ItemHandle>> {
        self.source_item.clone()
    }
}

struct TestView;

impl Render for TestView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// 独立预览显示面包屑行但没有「返回源码」，源码派生预览才显示该按钮。
#[gpui::test]
fn preview_controls_follow_preview_items(cx: &mut TestAppContext) {
    let project = cx.new(|cx| {
        Project::empty_with_watcher(
            Arc::new(zcv_language::LanguageRegistry::new()),
            Arc::new(FsWatcher::new()),
            cx,
        )
    });
    let pane = cx.new(|cx| Pane::new(project.clone(), cx));
    let breadcrumbs = cx.new(|cx| Breadcrumbs::new(project, cx));
    let button = cx.new(|_| PreviewButton::new(pane.downgrade()));
    cx.add_window_view(|window, cx| {
        let preview = cx.new(|cx| ProbeItem {
            focus: cx.focus_handle(),
            is_preview: true,
            source_item: None,
        });
        assert_eq!(
            breadcrumbs.update(cx, |view, cx| view.set_active_pane_item(
                Some(&preview),
                window,
                cx
            )),
            ToolbarItemLocation::PrimaryLeft
        );
        assert_eq!(
            button.update(cx, |button, cx| button.set_active_pane_item(
                Some(&preview),
                window,
                cx
            )),
            ToolbarItemLocation::Hidden
        );

        let source = cx.new(|cx| ProbeItem {
            focus: cx.focus_handle(),
            is_preview: false,
            source_item: None,
        });
        let derived_preview = cx.new(|cx| ProbeItem {
            focus: cx.focus_handle(),
            is_preview: true,
            source_item: Some(Box::new(source)),
        });
        assert_eq!(
            breadcrumbs.update(cx, |view, cx| view.set_active_pane_item(
                Some(&derived_preview),
                window,
                cx
            )),
            ToolbarItemLocation::PrimaryLeft
        );
        assert_eq!(
            button.update(cx, |button, cx| button.set_active_pane_item(
                Some(&derived_preview),
                window,
                cx
            )),
            ToolbarItemLocation::PrimaryRight
        );
        assert_eq!(button.read(cx).action, Some(PreviewAction::ReturnToSource));

        let plain = cx.new(|cx| ProbeItem {
            focus: cx.focus_handle(),
            is_preview: false,
            source_item: None,
        });
        assert_eq!(
            breadcrumbs.update(cx, |view, cx| view.set_active_pane_item(
                Some(&plain),
                window,
                cx
            )),
            ToolbarItemLocation::Hidden
        );
        assert_eq!(
            button.update(cx, |button, cx| button.set_active_pane_item(
                Some(&plain),
                window,
                cx
            )),
            ToolbarItemLocation::Hidden
        );
        TestView
    });
}
