//! 文件预览的公共协议与注册表。
//!
//! 该模块不包含具体格式实现。
//! 格式 crate 实现 [`PreviewProvider`]，并创建一个直接实现 Item 协议的具体预览视图。
//! 预览视图自身通过 [`PreviewItem`] 暴露预览身份及可选的源码 Item，经 `Item::as_preview_item` 桥接获取，不占用 Item 主接口。

use std::{
    any::TypeId,
    cell::Cell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use gpui::{
    AnyElement, App, Bounds, Context, Element, ElementId, Entity, EventEmitter, FocusHandle,
    GlobalElementId, InspectorElementId, IntoElement, LayoutId, Pixels, Render, Rgba, ScrollHandle,
    Size, Style, Subscription, WeakEntity, Window, div, point, prelude::*, relative, size,
};
use zcv_actions::TogglePreview;
use zcv_multi_buffer::MultiBuffer;
use zcv_project::Project;
use zcv_theme::color;
use zcv_ui::Button;

use crate::{
    item::{Item, ItemEvent, ItemHandle},
    pane::Pane,
    provider_registry::ProviderRegistry,
    toolbar::{ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView},
};

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
        /// 预览解析图片等文件文档所需的项目宿主。
        project: Entity<Project>,
    },
    Standalone {
        path: PathBuf,
        /// 独立预览打开文件文档所需的项目宿主。
        project: Entity<Project>,
    },
}

/// 预览视图 Item 的 object-safe 句柄，经 `Item::as_preview_item` 获取。
pub trait PreviewItemHandle: Send + 'static {
    /// 源码派生预览对应的源码 Item；独立预览返回 None。
    fn source_item(&self, cx: &App) -> Option<Box<dyn ItemHandle>>;
}

/// 预览视图 Item 的协议：独立预览没有源码 Item。
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

enum RegisteredPreview {
    Tab(Arc<dyn PreviewProvider>),
    External(Arc<dyn ExternalPreviewProvider>),
}

/// 注册工作区内的格式预览。同一具体 Provider 类型只注册一次。
pub fn register<P: PreviewProvider>(provider: P, cx: &mut App) {
    ProviderRegistry::register(
        Arc::new(RegisteredPreview::Tab(Arc::new(provider))),
        TypeId::of::<P>(),
        cx,
    );
}

pub(crate) fn source_provider_for(path: &Path, cx: &App) -> Option<Arc<dyn PreviewProvider>> {
    provider_for_mode(path, PreviewMode::Source, cx)
}

pub(crate) fn standalone_provider_for(path: &Path, cx: &App) -> Option<Arc<dyn PreviewProvider>> {
    provider_for_mode(path, PreviewMode::Standalone, cx)
}

fn provider_for_mode(path: &Path, mode: PreviewMode, cx: &App) -> Option<Arc<dyn PreviewProvider>> {
    ProviderRegistry::<RegisteredPreview>::find_map(cx, |entry| match entry.as_ref() {
        RegisteredPreview::Tab(provider)
            if provider.mode() == mode && provider.supports(path, cx) =>
        {
            Some(Arc::clone(provider))
        }
        _ => None,
    })
}

/// 用户显式触发的外部源码预览，例如通过系统阅读器打开编译产物。
pub trait ExternalPreviewProvider: Send + Sync + 'static {
    fn supports(&self, item: &dyn ItemHandle, path: &Path, cx: &App) -> bool;

    fn label(&self) -> &'static str;

    fn open(&self, item: Box<dyn ItemHandle>, path: PathBuf, window: &mut Window, cx: &mut App);
}

pub fn register_external_preview<P: ExternalPreviewProvider>(provider: P, cx: &mut App) {
    ProviderRegistry::register(
        Arc::new(RegisteredPreview::External(Arc::new(provider))),
        TypeId::of::<P>(),
        cx,
    );
}

pub(crate) enum SourcePreview {
    Tab {
        provider: Arc<dyn PreviewProvider>,
        path: PathBuf,
        multi_buffer: Entity<MultiBuffer>,
    },
    External {
        provider: Arc<dyn ExternalPreviewProvider>,
        path: PathBuf,
    },
}

impl SourcePreview {
    pub(crate) fn for_item(item: &dyn ItemHandle, cx: &App) -> Option<Self> {
        let path = item.item_path(cx)?;
        ProviderRegistry::<RegisteredPreview>::find_map(cx, |entry| match entry.as_ref() {
            RegisteredPreview::Tab(provider)
                if provider.mode() == PreviewMode::Source && provider.supports(&path, cx) =>
            {
                Some(Self::Tab {
                    provider: Arc::clone(provider),
                    path: path.clone(),
                    multi_buffer: item.multi_buffer(cx)?,
                })
            }
            RegisteredPreview::External(provider) if provider.supports(item, &path, cx) => {
                Some(Self::External {
                    provider: Arc::clone(provider),
                    path: path.clone(),
                })
            }
            _ => None,
        })
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Tab { .. } => "预览",
            Self::External { provider, .. } => provider.label(),
        }
    }
}

/// 在源码和对应预览之间切换，跟随活动 Item 选择按钮语义。
pub struct PreviewButton {
    pane: WeakEntity<Pane>,
    active_item: Option<Box<dyn ItemHandle>>,
    action: Option<PreviewAction>,
    _subscription: Option<Subscription>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreviewAction {
    Open(&'static str),
    ReturnToSource,
}

impl PreviewButton {
    pub fn new(pane: WeakEntity<Pane>) -> Self {
        Self {
            pane,
            active_item: None,
            action: None,
            _subscription: None,
        }
    }

    fn action_for(item: &dyn ItemHandle, cx: &App) -> Option<PreviewAction> {
        if let Some(preview) = item.as_preview_item(cx) {
            return preview
                .source_item(cx)
                .map(|_| PreviewAction::ReturnToSource);
        }
        SourcePreview::for_item(item, cx).map(|preview| PreviewAction::Open(preview.label()))
    }

    fn location(&self) -> ToolbarItemLocation {
        if self.action.is_some() {
            ToolbarItemLocation::PrimaryRight
        } else {
            ToolbarItemLocation::Hidden
        }
    }

    fn refresh_action(&mut self, cx: &mut Context<Self>) {
        let action = self
            .active_item
            .as_deref()
            .and_then(|item| Self::action_for(item, cx));
        if self.action != action {
            self.action = action;
            cx.emit(ToolbarItemEvent::ChangeLocation(self.location()));
            cx.notify();
        }
    }
}

impl EventEmitter<ToolbarItemEvent> for PreviewButton {}

impl ToolbarItemView for PreviewButton {
    fn set_active_pane_item(
        &mut self,
        item: Option<&dyn ItemHandle>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        self.active_item = item.map(ItemHandle::boxed_clone);
        self._subscription = item.map(|item| {
            let button = cx.weak_entity();
            item.subscribe_to_item_events(
                cx,
                Box::new(move |event, cx| {
                    if event == ItemEvent::PathChanged {
                        let button = button.clone();
                        cx.defer(move |cx| {
                            let _ = button.update(cx, |button, cx| button.refresh_action(cx));
                        });
                    }
                }),
            )
        });
        let action = item.and_then(|item| Self::action_for(item, cx));
        if self.action != action {
            self.action = action;
            cx.notify();
        }
        self.location()
    }
}

impl Render for PreviewButton {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = self.pane.clone();
        let button = match self.action {
            Some(PreviewAction::Open(label)) => Button::icon("toolbar-preview", "icons/eye.svg")
                .color(color::current(cx).text_muted)
                .label(label)
                .shortcut(zcv_keymap::display_shortcut(&TogglePreview, cx)),
            Some(PreviewAction::ReturnToSource) => {
                Button::icon("preview-toolbar-source", "icons/eye_off.svg").label("返回源码")
            }
            None => return div(),
        };
        div()
            .debug_selector(|| "document-preview".into())
            .child(button.on_click(move |_, window, cx| {
                let _ = pane.update(cx, |pane, cx| pane.toggle_preview(window, cx));
            }))
    }
}

#[cfg(test)]
#[path = "test/preview_tests.rs"]
mod tests;
