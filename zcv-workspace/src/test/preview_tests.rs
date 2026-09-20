use gpui::{Focusable, TestAppContext, px};

use super::*;

struct DiagramProvider;

struct LaterDiagramProvider;

struct StandaloneDiagramProvider;

fn provider_for(path: &Path, cx: &App) -> Option<Arc<dyn PreviewProvider>> {
    ProviderRegistry::<dyn PreviewProvider>::find(cx, |provider| provider.supports(path, cx))
}

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
        let provider = provider_for(Path::new("architecture.diagram"), cx)
            .expect("新格式应由注册的 Provider 匹配");
        assert!(provider.supports(Path::new("architecture.diagram"), cx));
        assert!(provider_for(Path::new("architecture.txt"), cx).is_none());
        assert_eq!(
            cx.global::<ProviderRegistry<dyn PreviewProvider>>()
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
        let selected = provider_for(Path::new("architecture.diagram"), cx).unwrap();
        let registry = cx.global::<ProviderRegistry<dyn PreviewProvider>>();
        assert!(Arc::ptr_eq(
            &selected,
            &registry.providers.last().unwrap().provider
        ));
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

impl PreviewItem for ProbeItem {}

struct TestView;

impl Render for TestView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// 活动 Item 是预览视图时工具区显示；普通 Item 时隐藏。
#[gpui::test]
fn preview_toolbar_follows_preview_items(cx: &mut TestAppContext) {
    let pane = cx.new(Pane::new);
    let toolbar = cx.new(|cx| PreviewToolbar::new(pane.downgrade(), cx));
    cx.add_window_view(|window, cx| {
        let preview = cx.new(|cx| ProbeItem {
            focus: cx.focus_handle(),
            is_preview: true,
        });
        let preview_location = toolbar.update(cx, |toolbar, cx| {
            toolbar.set_active_pane_item(Some(&preview as &dyn ItemHandle), window, cx)
        });
        assert_eq!(preview_location, ToolbarItemLocation::Secondary);

        let plain = cx.new(|cx| ProbeItem {
            focus: cx.focus_handle(),
            is_preview: false,
        });
        let plain_location = toolbar.update(cx, |toolbar, cx| {
            toolbar.set_active_pane_item(Some(&plain as &dyn ItemHandle), window, cx)
        });
        assert_eq!(plain_location, ToolbarItemLocation::Hidden);
        TestView
    });
}
