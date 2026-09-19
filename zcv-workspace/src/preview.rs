//! 文件预览的公共协议与注册表。
//!
//! 该模块不包含具体格式实现。
//! 格式 crate 实现 [`PreviewProvider`]，并创建一个直接实现 Item 协议的具体预览视图。
//! 预览视图自身通过 [`PreviewItem`] 暴露与源码 Item 的关联，经 `Item::as_preview_item` 桥接获取，不占用 Item 主接口。

use std::any::TypeId;
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Bounds, Context, Element, ElementId, Entity, EventEmitter, FocusHandle,
    GlobalElementId, InspectorElementId, IntoElement, LayoutId, Pixels, Render, Rgba, ScrollHandle,
    Size, Style, Subscription, WeakEntity, Window, div, point, prelude::*, relative, size,
};
use zcv_actions::TogglePreview;
use zcv_multi_buffer::MultiBuffer;
use zcv_theme::{color, space};
use zcv_ui::Button;

use crate::breadcrumbs::Breadcrumbs;
use crate::item::{Item, ItemEvent, ItemHandle};
use crate::pane::Pane;
use crate::provider_registry::ProviderRegistry;
use crate::toolbar::{ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView};

/// 预览内容请求打开工作区文件时使用的宿主能力。
pub type OpenPathCallback = Rc<dyn Fn(PathBuf, &mut Window, &mut App)>;

/// 预览视口的宿主配置。
///
/// 视口拥有滚动状态，调用方只提供焦点、语义标识和背景等展示配置。
pub struct PreviewViewportOptions {
    focus: FocusHandle,
    key_context: &'static str,
    scroll_id: &'static str,
    horizontal_scroll_id: &'static str,
    background: Rgba,
}

impl PreviewViewportOptions {
    pub fn new(
        focus: &FocusHandle,
        key_context: &'static str,
        scroll_id: &'static str,
        horizontal_scroll_id: &'static str,
        background: Rgba,
    ) -> Self {
        Self {
            focus: focus.clone(),
            key_context,
            scroll_id,
            horizontal_scroll_id,
            background,
        }
    }
}

/// 预览视口：统一处理非文本预览的内容缩放、适应窗口、居中与滚动。
///
/// 视口不拥有预览内容；格式 crate 只提供内容的逻辑尺寸和构造闭包。
/// 工作区内容缩放由视口读取并应用到构造闭包收到的显示尺寸中，格式 crate 不需要直接读取排版状态。
/// 滚动状态属于具体预览 Item，由该 Item 持有一个视口实例。
pub struct PreviewViewport {
    vertical_scroll_handle: ScrollHandle,
    horizontal_scroll_handle: ScrollHandle,
    centered_layout_scale: Rc<Cell<Option<f32>>>,
    centered_viewport: Rc<Cell<Option<Size<Pixels>>>>,
    center_after_layout: Rc<Cell<bool>>,
}

impl Default for PreviewViewport {
    fn default() -> Self {
        Self::new()
    }
}

impl PreviewViewport {
    pub fn new() -> Self {
        Self {
            vertical_scroll_handle: ScrollHandle::new(),
            horizontal_scroll_handle: ScrollHandle::new(),
            centered_layout_scale: Rc::new(Cell::new(None)),
            centered_viewport: Rc::new(Cell::new(None)),
            center_after_layout: Rc::new(Cell::new(true)),
        }
    }

    /// 返回当前工作区生效的内容缩放。
    ///
    /// 仅供格式实现需要按缩放比例准备资源时使用；最终显示尺寸仍由 [`Self::render`] 统一计算。
    pub fn content_scale(window: &Window, cx: &App) -> f32 {
        crate::typography_for_window(window, cx).content_scale(cx)
    }

    /// 内容尺寸或资源变化后请求重新居中。
    pub fn invalidate_centering(&mut self) {
        self.center_after_layout.set(true);
    }

    /// 渲染一个统一的双向滚动预览视口。
    ///
    /// `content_size` 是未应用工作区内容缩放前的逻辑尺寸。
    /// 容器会先按当前视口计算适应窗口比例，再应用工作区内容缩放，并把最终显示尺寸传给 `content`。
    /// 这样每个格式实现都必须通过这个容器展示内容，不会遗漏内容缩放、适应窗口或居中滚动。
    pub fn render(
        &mut self,
        content_size: Option<Size<Pixels>>,
        content: impl FnOnce(Option<Size<Pixels>>) -> AnyElement + 'static,
        options: PreviewViewportOptions,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let content_scale = content_size
            .map(|_| Self::content_scale(window, cx))
            .unwrap_or(1.);
        PreviewViewportElement {
            content_size,
            content: Some(Box::new(content)),
            content_scale,
            options,
            vertical_scroll_handle: self.vertical_scroll_handle.clone(),
            horizontal_scroll_handle: self.horizontal_scroll_handle.clone(),
            centered_layout_scale: self.centered_layout_scale.clone(),
            centered_viewport: self.centered_viewport.clone(),
            center_after_layout: self.center_after_layout.clone(),
        }
        .into_any_element()
    }

    pub fn vertical_scroll_handle(&self) -> &ScrollHandle {
        &self.vertical_scroll_handle
    }

    pub fn horizontal_scroll_handle(&self) -> &ScrollHandle {
        &self.horizontal_scroll_handle
    }
}

type PreviewContent = Box<dyn FnOnce(Option<Size<Pixels>>) -> AnyElement>;

struct PreviewViewportElement {
    content_size: Option<Size<Pixels>>,
    content: Option<PreviewContent>,
    content_scale: f32,
    options: PreviewViewportOptions,
    vertical_scroll_handle: ScrollHandle,
    horizontal_scroll_handle: ScrollHandle,
    centered_layout_scale: Rc<Cell<Option<f32>>>,
    centered_viewport: Rc<Cell<Option<Size<Pixels>>>>,
    center_after_layout: Rc<Cell<bool>>,
}

impl IntoElement for PreviewViewportElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for PreviewViewportElement {
    type RequestLayoutState = ();
    type PrepaintState = Option<AnyElement>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (
            window.request_layout(
                Style {
                    size: size(relative(1.).into(), relative(1.).into()),
                    ..Default::default()
                },
                [],
                cx,
            ),
            (),
        )
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let content_size = self.content_size;
        let display_size = content_size.map(|content_size| {
            let fit_scale = fit_scale(bounds.size, content_size);
            Size {
                width: content_size.width * (fit_scale * self.content_scale),
                height: content_size.height * (fit_scale * self.content_scale),
            }
        });
        let content = (self.content.take()?)(display_size);
        let layout_scale = self.content_scale.max(1.);
        let viewport_changed = self.centered_viewport.get() != Some(bounds.size);
        let scale_changed = self.centered_layout_scale.get() != Some(layout_scale);
        if viewport_changed || scale_changed {
            self.centered_viewport.set(Some(bounds.size));
            self.centered_layout_scale.set(Some(layout_scale));
            self.center_after_layout.set(true);
        }
        if self.center_after_layout.get() && content_size.is_some() {
            center_scroll_handles_for_viewport(
                &self.vertical_scroll_handle,
                &self.horizontal_scroll_handle,
                bounds.size,
                layout_scale,
            );
            self.center_after_layout.set(false);
        }

        let mut scaled_content = div()
            .min_w_full()
            .min_h_full()
            .flex()
            .items_center()
            .justify_center()
            .child(content);
        scaled_content.style().size.width = Some(relative(layout_scale).into());
        scaled_content.style().size.height = Some(relative(1.).into());

        let mut horizontal_scroll = div()
            .id(self.options.horizontal_scroll_id)
            .w_full()
            .overflow_x_scroll()
            .track_scroll(&self.horizontal_scroll_handle)
            .child(scaled_content);
        horizontal_scroll.style().size.height = Some(relative(layout_scale).into());

        let vertical_scroll = div()
            .id(self.options.scroll_id)
            .track_focus(&self.options.focus)
            .key_context(self.options.key_context)
            .tab_index(0)
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.vertical_scroll_handle)
            .bg(self.options.background)
            .child(horizontal_scroll);
        let mut element = vertical_scroll.into_any_element();
        element.prepaint_as_root(bounds.origin, bounds.size.into(), window, cx);
        Some(element)
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(element) = prepaint {
            element.paint(window, cx);
        }
    }
}

fn fit_scale(viewport: Size<Pixels>, content_size: Size<Pixels>) -> f32 {
    let width = f32::from(content_size.width);
    let height = f32::from(content_size.height);
    if width <= 0.
        || height <= 0.
        || f32::from(viewport.width) <= 0.
        || f32::from(viewport.height) <= 0.
    {
        return 1.;
    }
    let scale_x = f32::from(viewport.width) / width;
    let scale_y = f32::from(viewport.height) / height;
    scale_x.min(scale_y).min(1.)
}

fn center_scroll_handles_for_viewport(
    vertical_scroll_handle: &ScrollHandle,
    horizontal_scroll_handle: &ScrollHandle,
    viewport: Size<Pixels>,
    layout_scale: f32,
) {
    if f32::from(viewport.height) <= 0. || f32::from(viewport.width) <= 0. {
        return;
    }

    vertical_scroll_handle.set_offset(point(
        Pixels::ZERO,
        centered_scroll_offset(viewport.height, layout_scale),
    ));
    horizontal_scroll_handle.set_offset(point(
        centered_scroll_offset(viewport.width, layout_scale),
        Pixels::ZERO,
    ));
}

fn centered_scroll_offset(viewport: Pixels, layout_scale: f32) -> Pixels {
    viewport * -((layout_scale - 1.).max(0.) / 2.)
}

/// 预览的来源关系。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewMode {
    /// 从已有源码 Item 派生预览，预览可以切回源码。
    Source,
    /// 文件本身只能以预览形式打开，没有对应的源码 Item。
    Standalone,
}

/// 预览的展示布局。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewPresentation {
    /// 由格式实现负责流式排版和滚动，例如 Markdown。
    Flow,
    /// 必须经 `PreviewViewport` 展示，例如图片和 SVG。
    Canvas,
}

/// 交给 Preview Provider 的文档输入。
#[derive(Clone)]
pub enum PreviewDocument {
    Source {
        path: PathBuf,
        source_item: Box<dyn ItemHandle>,
        multi_buffer: Entity<MultiBuffer>,
        /// 预览内容请求打开工作区文件时使用的宿主回调。
        open_path: Option<OpenPathCallback>,
    },
    Standalone {
        path: PathBuf,
    },
}

/// 预览视图 Item 的 object-safe 句柄，经 `Item::as_preview_item` 获取。
pub trait PreviewItemHandle: Send + 'static {
    /// 预览视图对应的源码 Item（通常是编辑器）；无法暴露时返回 None。
    fn source_item(&self, cx: &App) -> Option<Box<dyn ItemHandle>>;
}

/// 预览视图 Item 的协议：提供与源码 Item 的关联。
pub trait PreviewItem: Item {
    fn source_item(&self, _cx: &App) -> Option<Box<dyn ItemHandle>> {
        None
    }
}

impl<T: PreviewItem> PreviewItemHandle for Entity<T> {
    fn source_item(&self, cx: &App) -> Option<Box<dyn ItemHandle>> {
        self.read(cx).source_item(cx)
    }
}

/// 文件格式预览的工厂接口。
///
/// `mode` 决定预览是源码 Item 的派生视图还是独立文件视图；
/// `presentation` 决定视图使用流式排版还是画布视口。
/// 画布预览的 `Render` 实现必须把内容交给 `PreviewViewport`。
pub trait PreviewProvider: Send + Sync + 'static {
    fn supports(&self, path: &Path, cx: &App) -> bool;

    fn mode(&self) -> PreviewMode;

    fn presentation(&self) -> PreviewPresentation;

    fn create(&self, document: PreviewDocument, cx: &mut App) -> Box<dyn ItemHandle>;
}

/// 注册格式预览 Provider。同一具体 Provider 类型只注册一次。
pub fn register<P: PreviewProvider>(provider: P, cx: &mut App) {
    ProviderRegistry::<dyn PreviewProvider>::register(Arc::new(provider), TypeId::of::<P>(), cx);
}

pub(crate) fn source_provider_for(path: &Path, cx: &App) -> Option<Arc<dyn PreviewProvider>> {
    provider_for_mode(path, PreviewMode::Source, cx)
}

pub(crate) fn standalone_provider_for(path: &Path, cx: &App) -> Option<Arc<dyn PreviewProvider>> {
    provider_for_mode(path, PreviewMode::Standalone, cx)
}

fn provider_for_mode(path: &Path, mode: PreviewMode, cx: &App) -> Option<Arc<dyn PreviewProvider>> {
    ProviderRegistry::<dyn PreviewProvider>::find(cx, |provider| {
        provider.mode() == mode && provider.supports(path, cx)
    })
}

/// 源码派生预览共用的工具栏：左侧源码面包屑，右侧「返回源码」按钮。
///
/// 工具项是 Pane 级的持久实体：活动 Item 是预览视图时显示，否则隐藏。
/// 预览格式实现不再各自持有工具栏视图，也不再注入返回源码回调。
pub struct PreviewToolbar {
    pane: WeakEntity<Pane>,
    breadcrumbs: Entity<Breadcrumbs>,
    _source_subscription: Option<Subscription>,
}

impl PreviewToolbar {
    pub fn new(pane: WeakEntity<Pane>, cx: &mut Context<Self>) -> Self {
        let breadcrumbs = cx.new(|_| Breadcrumbs::without_project());
        Self {
            pane,
            breadcrumbs,
            _source_subscription: None,
        }
    }
}

impl EventEmitter<ToolbarItemEvent> for PreviewToolbar {}

impl ToolbarItemView for PreviewToolbar {
    fn set_active_pane_item(
        &mut self,
        item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        self._source_subscription = None;
        let preview = item.and_then(|item| item.as_preview_item(cx));
        let source = preview.as_ref().and_then(|preview| preview.source_item(cx));
        self.breadcrumbs.update(cx, |breadcrumbs, cx| {
            breadcrumbs.set_item(source.as_deref(), cx)
        });
        if let Some(source) = &source {
            let this = cx.entity().downgrade();
            self._source_subscription = Some(source.subscribe_to_item_events(
                cx,
                Box::new(move |event, cx| {
                    if matches!(
                        event,
                        ItemEvent::PathChanged
                            | ItemEvent::UpdateTab
                            | ItemEvent::UpdateBreadcrumbs
                    ) {
                        this.update(cx, |toolbar, cx| {
                            toolbar.breadcrumbs.update(cx, |_, cx| cx.notify());
                            cx.notify();
                        })
                        .ok();
                    }
                }),
            ));
        }
        if preview.is_some() {
            ToolbarItemLocation::Secondary
        } else {
            ToolbarItemLocation::Hidden
        }
    }
}

impl Render for PreviewToolbar {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .flex()
            .items_center()
            .gap(space::S6)
            .child(div().flex_1().min_w_0().child(self.breadcrumbs.clone()))
            .child(
                Button::icon("preview-toolbar-source", "icons/eye_off.svg")
                    .label("返回源码")
                    .on_click({
                        let pane = self.pane.clone();
                        move |_, window, cx| {
                            pane.update(cx, |pane, cx| {
                                pane.toggle_preview(window, cx);
                            })
                            .ok();
                        }
                    }),
            )
    }
}

/// 预览切换按钮的当前语义。
#[derive(Clone, Copy, PartialEq, Eq)]
enum PreviewControl {
    ShowPreview,
    ShowSource,
}

/// 预览能力入口。
pub struct PreviewButton {
    pane: WeakEntity<Pane>,
    control: Option<PreviewControl>,
    active_item: Option<Box<dyn ItemHandle>>,
    _subscription: Option<Subscription>,
}

impl PreviewButton {
    pub fn new(pane: WeakEntity<Pane>) -> Self {
        Self {
            pane,
            control: None,
            active_item: None,
            _subscription: None,
        }
    }

    fn control_for(item: Option<&dyn ItemHandle>, cx: &App) -> Option<PreviewControl> {
        if item.is_some_and(|item| item.as_preview_item(cx).is_some()) {
            Some(PreviewControl::ShowSource)
        } else if item
            .and_then(|item| item.item_path(cx))
            .as_deref()
            .is_some_and(|path| source_provider_for(path, cx).is_some())
        {
            Some(PreviewControl::ShowPreview)
        } else {
            None
        }
    }

    fn refresh_control(&mut self, cx: &mut Context<Self>) {
        let control = Self::control_for(self.active_item.as_deref(), cx);
        if control == self.control {
            return;
        }
        self.control = control;
        cx.notify();
    }
}

impl PreviewButton {
    pub fn set_active_item(
        &mut self,
        active_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self._subscription = None;
        self.active_item = active_item.map(ItemHandle::boxed_clone);
        self.control = Self::control_for(active_item, cx);

        if let Some(item) = active_item {
            let this = cx.entity().downgrade();
            self._subscription = Some(item.subscribe_to_item_events(
                cx,
                Box::new(move |event, cx| {
                    if event == ItemEvent::PathChanged {
                        let this = this.clone();
                        cx.defer(move |cx| {
                            this.update(cx, |this, cx| this.refresh_control(cx)).ok();
                        });
                    }
                }),
            ));
        }
        cx.notify();
    }
}

impl Render for PreviewButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = self.pane.clone();
        div().when_some(self.control, |controls, control| {
            let (icon, label) = match control {
                PreviewControl::ShowSource => ("icons/eye_off.svg", "源码"),
                PreviewControl::ShowPreview => ("icons/eye.svg", "预览"),
            };
            controls.child(
                Button::icon("toolbar-preview", icon)
                    .label(label)
                    .color(color::current(cx).text_muted)
                    .shortcut(zcv_keymap::display_shortcut(&TogglePreview, cx))
                    .on_click(move |_, window, cx| {
                        pane.update(cx, |pane, cx| {
                            pane.toggle_preview(window, cx);
                        })
                        .ok();
                    }),
            )
        })
    }
}

#[cfg(test)]
mod tests {
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
}
