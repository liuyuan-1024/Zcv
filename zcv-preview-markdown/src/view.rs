//! Markdown 预览 Item：观察源码 MultiBuffer，解析后渲染为原生块元素。

use std::{
    any::TypeId,
    cell::RefCell,
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use gpui::{
    AnyElement, AnyEntity, App, Bounds, ClipboardItem, Context, Corners, CursorStyle,
    DispatchPhase, Element, ElementId, Entity, EventEmitter, FocusHandle, Focusable, FontStyle,
    FontWeight, GlobalElementId, HighlightStyle, HitboxBehavior, Hsla, Image, ImageFormat,
    InspectorElementId, InteractiveText, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ObjectFit, Pixels, Render, RenderImage, Rgba, SMOOTH_SVG_SCALE_FACTOR,
    ScrollHandle, SharedString, Size, StatefulInteractiveElement, StrikethroughStyle, StyledImage,
    StyledText, Subscription, SvgRenderer, Task, TextAlign, TextLayout, UnderlineStyle, Window,
    canvas, div, fill, img, point, prelude::*, px, size,
};
use pulldown_cmark::Alignment;
use zcv_actions::{Copy, SelectAll, SelectDown, SelectLeft, SelectRight, SelectUp};
use zcv_language::{
    HighlightSpan, LanguageRegistry, SnippetHighlightCancellation, SnippetHighlights,
    highlight_snippet_with_cancellation,
};
use zcv_multi_buffer::{MultiBuffer, MultiBufferAnchor, MultiBufferEvent, MultiBufferSnapshot};
use zcv_project::Project;
use zcv_text::Affinity;
use zcv_theme::{color, fixed, scale, syntax, typography};
use zcv_ui::{
    AUTOSCROLL_INTERVAL, BlinkManager, DragAutoscroll, Scrollbar, SelectionContourLine,
    paint_selection_segment, selection_corner_radius, selection_line_end_overshoot,
    selection_segment_shapes,
};
use zcv_workspace::{
    Item, ItemEvent, ItemHandle, OpenPathCallback, PreviewDocument, PreviewItem, PreviewItemHandle,
    typography_for_window,
};

use crate::document::{Block, BlockKind, Inline, InlineStyle, parse};
use crate::selection::{
    PlaceholderMap, RenderSelection, RunLink, SelectableText, SelectionLayout, SelectionModel,
};

const MARKDOWN_REPARSE_DEBOUNCE: Duration = Duration::from_millis(200);
const INLINE_CODE_CHIP_VERTICAL_INSET: f32 = 0.1;
const INLINE_CODE_CHIP_CORNER_RADIUS: Pixels = px(4.);
/// 行内公式在文本流中预留宽度时使用的不可断行空格。
const MATH_PLACEHOLDER: char = '\u{00A0}';

/// 渲染后的公式：光栅图像与其逻辑尺寸。
///
/// 逻辑尺寸用于在文本流中预留宽度，使公式后的文字继续排在同一行而不是整体换行。
#[derive(Clone)]
struct MathImage {
    image: Arc<RenderImage>,
    size: Size<Pixels>,
}

type MathImages = HashMap<String, Result<MathImage, String>>;
type MermaidImages = HashMap<String, Result<MathImage, String>>;

/// 每个根块在最近一次布局中的几何，供源码偏移定位与可见锚点回报使用。
#[derive(Clone)]
struct BlockGeometry {
    source: Range<usize>,
    bounds: Bounds<Pixels>,
}

/// 预览当前帧的块几何索引。由渲染逐帧重建，不是位置权威：位置由其所有者以锚点保存。
#[derive(Default)]
struct PreviewLayout {
    blocks: RefCell<Vec<BlockGeometry>>,
}

struct MarkdownRenderContext<'a> {
    source_directory: Option<&'a Path>,
    type_scale: typography::Typography,
    math_images: &'a MathImages,
    mermaid_images: &'a MermaidImages,
    selection_layout: &'a Rc<RefCell<SelectionLayout>>,
    /// 本帧的选区快照；绘制只读，不写回选择状态。
    selection: Option<Range<usize>>,
    /// 本帧的光标偏移快照（活动端）；有焦点且闪烁可见时为 Some。
    caret: Option<usize>,
    window: &'a Window,
    cx: &'a App,
}

pub(crate) struct MarkdownPreviewView {
    source_item: Box<dyn ItemHandle>,
    multi_buffer: Entity<MultiBuffer>,
    /// 代码围栏高亮使用的语言注册表；与源码 MultiBuffer 共享同一份。
    language_registry: Arc<LanguageRegistry>,
    open_path: Option<OpenPathCallback>,
    focus: FocusHandle,
    scroll_handle: ScrollHandle,
    /// 选择坐标空间；refresh 时从块树投影，两次 refresh 之间稳定。
    selectable: SelectableText,
    /// 选择状态与鼠标手势；由事件驱动，渲染只读取快照。
    selection: SelectionModel,
    /// 当前帧命中几何；渲染逐帧重建，事件读取最新一帧。
    selection_layout: Rc<RefCell<SelectionLayout>>,
    /// 拖拽选区的越界自动滚动意图；16ms tick 消费。
    drag_autoscroll: DragAutoscroll,
    drag_autoscroll_scheduled: bool,
    /// 光标闪烁；焦点驱动启停，交互时暂停，与 Editor 共用同一状态机。
    blink_manager: Entity<BlinkManager>,
    blink_manager_initialized: bool,
    scrollbar: Scrollbar<ScrollHandle>,
    blocks: Arc<Vec<Block>>,
    /// 根块的 slug 到源码锚点映射；内部 #slug 锚点据此定位。
    heading_anchors: HashMap<String, MultiBufferAnchor>,
    /// 最近一次布局的根块几何；渲染逐帧重建，仅用于把源码偏移换算为预览滚动位置。
    layout: Rc<PreviewLayout>,
    /// 待执行的滚动揭示请求。以源码锚点保存，重解析或 debounce 后仍能按当前快照解析。
    pending_reveal: Rc<RefCell<Option<MultiBufferAnchor>>>,
    code_highlight_generation: u64,
    code_highlight_cancellation: Option<SnippetHighlightCancellation>,
    code_highlight_task: Option<Task<()>>,
    refresh_generation: u64,
    refresh_task: Option<Task<()>>,
    _document_subscription: Subscription,
    _item_subscription: Subscription,
    math_images: Arc<MathImages>,
    math_content_size: Option<Pixels>,
    math_color: Option<Rgba>,
    math_render_task: Option<Task<()>>,
    math_render_generation: u64,
    mermaid_images: Arc<MermaidImages>,
    mermaid_render_task: Option<Task<()>>,
    mermaid_render_generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MarkdownPreviewEvent {
    SourceMetadataChanged,
}

impl MarkdownPreviewView {
    pub(crate) fn new(document: PreviewDocument, cx: &mut Context<Self>) -> Self {
        let PreviewDocument::Source {
            source_item,
            multi_buffer,
            open_path,
            ..
        } = document
        else {
            panic!("Markdown 预览必须从源码 Item 创建")
        };
        let document_subscription = cx.subscribe(&multi_buffer, |view, _, event, cx| {
            if matches!(event, MultiBufferEvent::TextChanged) {
                view.schedule_refresh(cx);
            }
        });
        let this = cx.entity().downgrade();
        let item_subscription = source_item.subscribe_to_item_events(
            cx,
            Box::new(move |event, cx| {
                if matches!(
                    event,
                    ItemEvent::PathChanged | ItemEvent::UpdateTab | ItemEvent::UpdateBreadcrumbs
                ) {
                    this.update(cx, |_view, cx| {
                        cx.emit(MarkdownPreviewEvent::SourceMetadataChanged);
                        cx.notify();
                    })
                    .ok();
                }
            }),
        );
        let scroll_handle = ScrollHandle::new();
        let language_registry = multi_buffer
            .read(cx)
            .language_registry(cx)
            .expect("源码派生 Markdown 预览要求 MultiBuffer 携带语言注册表；缺少说明装配顺序错误");
        let mut view = Self {
            source_item,
            multi_buffer,
            language_registry,
            open_path,
            focus: cx.focus_handle(),
            scrollbar: Scrollbar::vertical(scroll_handle.clone()),
            scroll_handle,
            selectable: SelectableText::from_blocks(&[]),
            selection: SelectionModel::default(),
            selection_layout: Rc::default(),
            drag_autoscroll: DragAutoscroll::default(),
            drag_autoscroll_scheduled: false,
            blink_manager: cx.new(|_| BlinkManager::new(true)),
            blink_manager_initialized: false,
            blocks: Arc::new(Vec::new()),
            heading_anchors: HashMap::new(),
            layout: Rc::new(PreviewLayout::default()),
            pending_reveal: Rc::new(RefCell::new(None)),
            code_highlight_generation: 0,
            code_highlight_cancellation: None,
            code_highlight_task: None,
            refresh_generation: 0,
            refresh_task: None,
            _document_subscription: document_subscription,
            _item_subscription: item_subscription,
            math_images: Arc::new(HashMap::new()),
            math_content_size: None,
            math_color: None,
            math_render_task: None,
            math_render_generation: 0,
            mermaid_images: Arc::new(HashMap::new()),
            mermaid_render_task: None,
            mermaid_render_generation: 0,
        };
        // 闪烁翻转驱动重绘；订阅与视图同生命周期。
        cx.observe(&view.blink_manager, |_, _, cx| cx.notify())
            .detach();
        view.refresh(cx);
        view
    }

    fn schedule_refresh(&mut self, cx: &mut Context<Self>) {
        // 连续输入时仅保留最新一次更新，避免每个按键都重建整个预览。
        self.refresh_generation = self.refresh_generation.wrapping_add(1);
        let generation = self.refresh_generation;
        let timer = cx.background_executor().timer(MARKDOWN_REPARSE_DEBOUNCE);
        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |view, cx| {
                if view.refresh_generation != generation {
                    return;
                }
                view.refresh_task = None;
                view.refresh(cx);
            });
        }));
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        // 源码已变化：选择坐标空间随块树重建，既有选择失效。
        self.selection.reset();
        if let Some(cancellation) = self.code_highlight_cancellation.take() {
            cancellation.cancel();
        }
        // 预览渲染需要整份文档文本；这是只读边界，不进入编辑/显示热路径。
        let snapshot = self
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        let text = String::from_utf8(snapshot.text_bytes()).expect("编辑器文档应为 UTF-8");
        self.blocks = Arc::new(parse(&text));
        self.selectable = SelectableText::from_blocks(&self.blocks);
        self.heading_anchors = collect_heading_anchors(&self.blocks, &snapshot);
        self.ensure_mermaid_render(cx);
        self.math_render_generation = self.math_render_generation.wrapping_add(1);
        self.math_content_size = None;
        self.math_color = None;
        self.math_images = Arc::new(HashMap::new());
        self.math_render_task.take();
        self.code_highlight_generation = self.code_highlight_generation.wrapping_add(1);
        let generation = self.code_highlight_generation;
        let cancellation = SnippetHighlightCancellation::default();
        self.code_highlight_cancellation = Some(cancellation.clone());
        let mut blocks = (*self.blocks).clone();
        let language_registry = Arc::clone(&self.language_registry);
        let highlights = cx.background_spawn(async move {
            highlight_code_blocks(&mut blocks, &language_registry, &cancellation).then_some(blocks)
        });
        self.code_highlight_task = Some(cx.spawn(async move |this, cx| {
            let Some(blocks) = highlights.await else {
                return;
            };
            let _ = this.update(cx, |view, cx| {
                if view.code_highlight_generation != generation {
                    return;
                }
                view.blocks = Arc::new(blocks);
                view.code_highlight_cancellation = None;
                view.code_highlight_task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn ensure_mermaid_render(&mut self, cx: &mut Context<Self>) {
        self.mermaid_render_generation = self.mermaid_render_generation.wrapping_add(1);
        let generation = self.mermaid_render_generation;
        let sources = collect_mermaid_sources(&self.blocks);
        self.mermaid_images = Arc::new(MermaidImages::new());
        self.mermaid_render_task.take();
        if sources.is_empty() {
            return;
        }
        let renderer = cx.svg_renderer();
        let render_task = cx.background_spawn(async move {
            sources
                .into_iter()
                .map(|source| {
                    let result = render_mermaid(&source, renderer.clone());
                    (source, result)
                })
                .collect::<MermaidImages>()
        });
        self.mermaid_render_task = Some(cx.spawn(async move |this, cx| {
            let images = render_task.await;
            let _ = this.update(cx, |view, cx| {
                if view.mermaid_render_generation != generation {
                    return;
                }
                view.mermaid_images = Arc::new(images);
                view.mermaid_render_task = None;
                cx.notify();
            });
        }));
    }

    fn ensure_math_render(
        &mut self,
        content_size: Pixels,
        math_color: Rgba,
        cx: &mut Context<Self>,
    ) {
        if self.math_content_size == Some(content_size) && self.math_color == Some(math_color) {
            return;
        }

        self.math_content_size = Some(content_size);
        self.math_color = Some(math_color);
        self.math_render_generation = self.math_render_generation.wrapping_add(1);
        let generation = self.math_render_generation;
        let math_sources = collect_math_sources(&self.blocks);
        if math_sources.is_empty() {
            self.math_images = Arc::new(MathImages::new());
            self.math_render_task = None;
            return;
        }

        self.math_render_task.take();
        let math_font_size = f64::from(content_size.as_f32());
        let math_color =
            ratex_types::color::Color::new(math_color.r, math_color.g, math_color.b, math_color.a);
        let renderer = cx.svg_renderer();
        let math_task = cx.background_spawn(async move {
            math_sources
                .into_iter()
                .map(|(source, display)| {
                    let result = render_math(
                        &source,
                        display,
                        math_font_size,
                        math_color,
                        renderer.clone(),
                    );
                    (source, result)
                })
                .collect::<MathImages>()
        });
        self.math_render_task = Some(cx.spawn(async move |this, cx| {
            let images = math_task.await;
            let _ = this.update(cx, |view, cx| {
                if view.math_render_generation != generation {
                    return;
                }
                view.math_images = Arc::new(images);
                view.math_render_task = None;
                cx.notify();
            });
        }));
    }
}

impl EventEmitter<MarkdownPreviewEvent> for MarkdownPreviewView {}

impl Focusable for MarkdownPreviewView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for MarkdownPreviewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 首次渲染时注册焦点事件（构造函数中没有 Window）。
        if !self.blink_manager_initialized {
            cx.on_focus(&self.focus, window, |view, window, cx| {
                view.sync_cursor_blinking(window, cx);
            })
            .detach();
            cx.on_blur(&self.focus, window, |view, window, cx| {
                view.sync_cursor_blinking(window, cx);
            })
            .detach();
            cx.observe_window_activation(window, |view, window, cx| {
                view.sync_cursor_blinking(window, cx);
            })
            .detach();
            self.blink_manager_initialized = true;
        }
        // 命中几何逐帧重建；选择状态不参与重建，跨帧稳定。
        self.selection_layout
            .borrow_mut()
            .begin_frame(self.selectable.segments());
        let selection_snapshot = self.selection.range();
        // 光标是活动端的渲染投影：有焦点、存在锚点且闪烁可见时显示。
        let caret = (window.is_window_active()
            && self.focus.is_focused(window)
            && self.selection.has_anchor()
            && self.blink_manager.read(cx).visible())
        .then(|| self.selection.head());
        let type_scale = typography_for_window(window, cx);
        self.ensure_math_render(type_scale.content_size(), color::current(cx).text, cx);
        let source_path = self.source_item.item_path(cx);
        let source_directory = source_path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf);
        // 待执行的揭示请求在渲染时解析为当前快照下的源码偏移；解析失败即丢弃，不猜测坐标。
        let reveal_offset = self
            .pending_reveal
            .borrow_mut()
            .take()
            .and_then(|anchor| {
                self.multi_buffer.update(cx, |buffer, cx| {
                    buffer.snapshot(cx).anchor_offset(&anchor).ok()
                })
            })
            .map(|offset| offset.get());
        self.layout.blocks.borrow_mut().clear();
        let render_context = MarkdownRenderContext {
            source_directory: source_directory.as_deref(),
            type_scale,
            math_images: &self.math_images,
            mermaid_images: &self.mermaid_images,
            selection_layout: &self.selection_layout,
            selection: selection_snapshot,
            caret,
            window,
            cx,
        };
        let content = self
            .blocks
            .iter()
            .enumerate()
            .map(|(index, block)| {
                let mut next_key = 0;
                let layout = Rc::clone(&self.layout);
                let source = block.source.clone();
                div()
                    .w_full()
                    .flex_none()
                    .child(render_block(
                        block,
                        &mut next_key,
                        0,
                        index,
                        &render_context,
                    ))
                    .on_children_prepainted(move |bounds, _, _| {
                        if let Some(bounds) = bounds.first() {
                            layout.blocks.borrow_mut().push(BlockGeometry {
                                source: source.clone(),
                                bounds: *bounds,
                            });
                        }
                    })
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        // 事件表面：插入 hitbox 并注册窗口级鼠标监听，对齐 Editor 的事件接线。
        // 监听是窗口级的，拖拽越出预览区域仍能继续延伸选区。
        // 元素必须排在内容之前：冒泡阶段靠后的兄弟先收到事件，链接等内联交互要先于本表面合成点击，否则会被 stop_propagation 阻断；
        // hitbox 位于滚动条之下，点击滚动条时被其 BlockMouse 遮挡，不会误启选择。
        let view = cx.entity();
        let selection_layout = Rc::clone(&self.selection_layout);
        let focus = self.focus.clone();
        let autoscroll_line_height = type_scale.content_line();
        let link_source_directory = source_directory.clone();
        let link_open_path = self.open_path.clone();
        let link_heading_anchors = self.heading_anchors.clone();
        let link_pending_reveal = Rc::clone(&self.pending_reveal);
        let event_surface = canvas(
            move |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |_, hitbox, window, _cx| {
                window.on_mouse_event({
                    let view = view.clone();
                    let selection_layout = selection_layout.clone();
                    let focus = focus.clone();
                    let hitbox = hitbox.clone();
                    let source_directory = link_source_directory.clone();
                    let open_path = link_open_path.clone();
                    let heading_anchors = link_heading_anchors.clone();
                    let pending_reveal = Rc::clone(&link_pending_reveal);
                    move |event: &MouseDownEvent, phase, window, cx| {
                        if phase != DispatchPhase::Bubble
                            || event.button != MouseButton::Left
                            || !hitbox.is_hovered(window)
                        {
                            return;
                        }
                        // 链接命中优先于选择：一次按下直接跳转，不落光标。
                        if let Some(target) =
                            selection_layout.borrow().link_for_position(event.position)
                        {
                            open_markdown_link(
                                &target,
                                source_directory.as_deref(),
                                open_path.as_ref(),
                                &heading_anchors,
                                &pending_reveal,
                                window,
                                cx,
                            );
                            cx.stop_propagation();
                            return;
                        }
                        let Some(offset) =
                            selection_layout.borrow().index_for_position(event.position)
                        else {
                            return;
                        };
                        view.update(cx, |view, cx| {
                            window.focus(&focus, cx);
                            view.selection.begin_selection(
                                &view.selectable,
                                offset,
                                event.click_count,
                                event.modifiers.shift,
                            );
                            view.pause_cursor_blinking(cx);
                            cx.notify();
                        });
                        cx.stop_propagation();
                    }
                });
                window.on_mouse_event({
                    let view = view.clone();
                    let viewport = hitbox.bounds;
                    move |event: &MouseMoveEvent, phase, _window, cx| {
                        if phase != DispatchPhase::Bubble {
                            return;
                        }
                        view.update(cx, |view, cx| {
                            if !event.dragging() {
                                // 无按键移动：兜底结束漏网的拖拽（窗口外释放后移回）。
                                if view.selection.has_pending_selection() {
                                    view.selection.end_selection();
                                    view.drag_autoscroll.clear();
                                    cx.notify();
                                }
                                return;
                            }
                            if !view.selection.has_pending_selection() {
                                return;
                            }
                            let Some(offset) = view
                                .selection_layout
                                .borrow()
                                .index_for_position(event.position)
                            else {
                                return;
                            };
                            view.selection.update_selection(&view.selectable, offset);
                            view.pause_cursor_blinking(cx);
                            view.drag_autoscroll.update(
                                event.position,
                                viewport,
                                autoscroll_line_height,
                            );
                            if view.drag_autoscroll.is_active() {
                                view.schedule_drag_autoscroll(cx);
                            }
                            cx.notify();
                        });
                    }
                });
                window.on_mouse_event({
                    let view = view.clone();
                    move |_: &MouseUpEvent, phase, _window, cx| {
                        if phase != DispatchPhase::Bubble {
                            return;
                        }
                        view.update(cx, |view, cx| {
                            if view.selection.has_pending_selection() {
                                view.selection.end_selection();
                                view.drag_autoscroll.clear();
                                cx.notify();
                            }
                        });
                    }
                });
            },
        )
        .absolute()
        .inset_0();
        div()
            .id("markdown-preview")
            .track_focus(&self.focus)
            .key_context("MarkdownPreview")
            .tab_index(0)
            .size_full()
            .on_action(cx.listener(Self::handle_select_all))
            .on_action(cx.listener(Self::handle_copy))
            .on_action(cx.listener(Self::handle_select_left))
            .on_action(cx.listener(Self::handle_select_right))
            .on_action(cx.listener(Self::handle_select_up))
            .on_action(cx.listener(Self::handle_select_down))
            .relative()
            .overflow_hidden()
            .bg(color::current(cx).editor_background)
            .child(event_surface)
            .child(
                div()
                    .on_children_prepainted({
                        let layout = Rc::clone(&self.layout);
                        let scroll_handle = self.scroll_handle.clone();
                        move |_bounds, window, _cx| {
                            if let Some(offset) = reveal_offset {
                                reveal_offset_in_layout(&layout, &scroll_handle, offset, window);
                            }
                        }
                    })
                    .id("markdown-preview-scroll-container")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .restrict_scroll_to_axis()
                    .p(scale::S6)
                    .text_color(color::current(cx).text)
                    .text_size(type_scale.content_size())
                    .line_height(type_scale.content_line())
                    .cursor(CursorStyle::IBeam)
                    .flex()
                    .flex_col()
                    .gap(scale::S16)
                    .children(content),
            )
            .child(div().absolute().inset_0().child(self.scrollbar.clone()))
    }
}

impl MarkdownPreviewView {
    /// 当前视口顶部最近的根块所对应的源码锚点；无布局信息时返回 None。
    fn top_visible_source_anchor(&self, cx: &mut App) -> Option<MultiBufferAnchor> {
        let offset = {
            let blocks = self.layout.blocks.borrow();
            let viewport_top = self.scroll_handle.bounds().top();
            let mut best: Option<&BlockGeometry> = None;
            for geometry in blocks.iter() {
                if geometry.bounds.bottom() <= viewport_top {
                    continue;
                }
                match best {
                    Some(current) if current.bounds.top() <= geometry.bounds.top() => {}
                    _ => best = Some(geometry),
                }
            }
            best.map(|geometry| geometry.source.start)?
        };
        Some(self.multi_buffer.update(cx, |buffer, cx| {
            buffer.snapshot(cx).anchor_at(offset, Affinity::Before)
        }))
    }

    /// 窗口激活与预览焦点统一在这里决定闪烁生命周期。
    fn sync_cursor_blinking(&mut self, window: &Window, cx: &mut Context<Self>) {
        let should_blink = window.is_window_active() && self.focus.is_focused(window);
        self.blink_manager.update(cx, |manager, cx| {
            if should_blink {
                manager.enable(cx);
            } else {
                manager.disable(cx);
            }
        });
    }

    /// 交互后暂停闪烁：光标立即可见，短暂静止后恢复闪烁。
    fn pause_cursor_blinking(&mut self, cx: &mut Context<Self>) {
        self.blink_manager.update(cx, BlinkManager::pause_blinking);
    }

    fn handle_select_all(&mut self, _: &SelectAll, _window: &mut Window, cx: &mut Context<Self>) {
        self.selection.select_all(&self.selectable);
        self.pause_cursor_blinking(cx);
        cx.notify();
    }

    /// Shift+左：活动端向左移动一个字素。
    fn handle_select_left(&mut self, _: &SelectLeft, _window: &mut Window, cx: &mut Context<Self>) {
        let target = self.selectable.prev_grapheme_offset(self.selection.head());
        self.selection.extend_to(target);
        self.pause_cursor_blinking(cx);
        cx.notify();
    }

    /// Shift+右：活动端向右移动一个字素。
    fn handle_select_right(
        &mut self,
        _: &SelectRight,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.selectable.next_grapheme_offset(self.selection.head());
        self.selection.extend_to(target);
        self.pause_cursor_blinking(cx);
        cx.notify();
    }

    /// Shift+上：活动端在渲染几何中上移一行；首行保持不动。
    fn handle_select_up(&mut self, _: &SelectUp, _window: &mut Window, cx: &mut Context<Self>) {
        let head = self.selection.head();
        let target = self.selection_layout.borrow().vertical_move(head, true);
        if let Some(target) = target {
            self.selection.extend_to(target);
            self.pause_cursor_blinking(cx);
            cx.notify();
        }
    }

    /// Shift+下：活动端在渲染几何中下移一行；末行由命中钳制保持不动。
    fn handle_select_down(&mut self, _: &SelectDown, _window: &mut Window, cx: &mut Context<Self>) {
        let head = self.selection.head();
        let target = self.selection_layout.borrow().vertical_move(head, false);
        if let Some(target) = target {
            self.selection.extend_to(target);
            self.pause_cursor_blinking(cx);
            cx.notify();
        }
    }

    fn handle_copy(&mut self, _: &Copy, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.selection.selected_text(&self.selectable) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// 调度下一次拖拽自动滚动 tick；已有 tick 在途时不重复调度。
    fn schedule_drag_autoscroll(&mut self, cx: &mut Context<Self>) {
        if self.drag_autoscroll_scheduled || !self.drag_autoscroll.is_active() {
            return;
        }
        self.drag_autoscroll_scheduled = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(AUTOSCROLL_INTERVAL).await;
            let _ = this.update(cx, |this, cx| {
                this.drag_autoscroll_scheduled = false;
                this.drag_autoscroll_tick(cx);
            });
        })
        .detach();
    }

    /// 拖拽自动滚动 tick：应用越界滚动量，并按滚动后的内容位置延伸选区。
    fn drag_autoscroll_tick(&mut self, cx: &mut Context<Self>) {
        if !self.selection.has_pending_selection() {
            self.drag_autoscroll.clear();
            return;
        }
        let delta = self.drag_autoscroll.delta();
        if delta == point(Pixels::ZERO, Pixels::ZERO) {
            return;
        }
        let max = self.scroll_handle.max_offset();
        let current = self.scroll_handle.offset();
        let target = point(
            (current.x + delta.x).clamp(Pixels::ZERO, max.x),
            (current.y + delta.y).clamp(-max.y, Pixels::ZERO),
        );
        if target == current {
            // 已到内容边界：停止自动滚动，避免定时器空转。
            self.drag_autoscroll.clear();
            return;
        }
        self.scroll_handle.set_offset(target);
        // 本帧几何尚未随滚动更新：把指针换算到滚动前的坐标系再命中，
        // 等效于内容滚动后指针相对内容继续移动。
        let position = self.drag_autoscroll.position();
        let hit_position = point(position.x - delta.x, position.y - delta.y);
        let offset = self
            .selection_layout
            .borrow()
            .index_for_position(hit_position);
        if let Some(offset) = offset {
            self.selection.update_selection(&self.selectable, offset);
            self.pause_cursor_blinking(cx);
        }
        cx.notify();
        self.schedule_drag_autoscroll(cx);
    }
}

/// 收集标题 slug 到源码锚点的映射，供预览内部 #slug 锚点定位。
///
/// 锚点在解析时按当前快照生成，源码随后变化时由快照重新解析，不保存裸偏移。
fn collect_heading_anchors(
    blocks: &[Block],
    snapshot: &MultiBufferSnapshot,
) -> HashMap<String, MultiBufferAnchor> {
    fn visit(
        blocks: &[Block],
        snapshot: &MultiBufferSnapshot,
        anchors: &mut HashMap<String, MultiBufferAnchor>,
    ) {
        for block in blocks {
            match &block.kind {
                BlockKind::Heading { slug, .. } if !slug.is_empty() => {
                    anchors.insert(
                        slug.clone(),
                        snapshot.anchor_at(block.source.start, Affinity::Before),
                    );
                }
                BlockKind::Quote(children) => visit(children, snapshot, anchors),
                BlockKind::List { items, .. } => {
                    for item in items {
                        visit(item, snapshot, anchors);
                    }
                }
                _ => {}
            }
        }
    }
    let mut anchors = HashMap::new();
    visit(blocks, snapshot, &mut anchors);
    anchors
}

/// 把源码偏移换算为预览滚动位移，使命中块的顶边对齐到视口顶部。
///
/// 只消费一次布局结果；若偏移落在块间隙，取其后最近块，末尾则取最后一块。
fn reveal_offset_in_layout(
    layout: &PreviewLayout,
    scroll_handle: &ScrollHandle,
    offset: usize,
    window: &mut Window,
) {
    let blocks = layout.blocks.borrow();
    let geometry = blocks
        .iter()
        .filter(|geometry| geometry.source.start <= offset && offset < geometry.source.end)
        .max_by_key(|geometry| geometry.source.start)
        .or_else(|| {
            blocks
                .iter()
                .filter(|geometry| geometry.source.start <= offset)
                .max_by_key(|geometry| geometry.source.start)
        })
        .or_else(|| blocks.iter().min_by_key(|geometry| geometry.source.start));
    let Some(geometry) = geometry else {
        return;
    };
    let current = scroll_handle.offset();
    let scroll_top = scroll_handle.bounds().top();
    let target = current.y + scroll_top - geometry.bounds.top();
    let max = scroll_handle.max_offset().y;
    scroll_handle.set_offset(point(current.x, target.clamp(-max, Pixels::ZERO)));
    window.refresh();
}

fn render_block(
    block: &Block,
    next_key: &mut usize,
    list_depth: usize,
    namespace: usize,
    render_context: &MarkdownRenderContext<'_>,
) -> AnyElement {
    let source_directory = render_context.source_directory;
    let type_scale = render_context.type_scale;
    let math_images = render_context.math_images;
    let cx = render_context.cx;
    let key = *next_key;
    *next_key += 1;
    match &block.kind {
        BlockKind::Heading { level, content, .. } => {
            let size = heading_size(*level, type_scale);
            let line_height = heading_line_height(*level, size, type_scale)
                .max(inline_math_height(content, math_images).unwrap_or(Pixels::ZERO));
            div()
                .text_size(size)
                .line_height(line_height)
                .font_weight(FontWeight::BOLD)
                .child(render_inline(content, key, render_context))
                .into_any_element()
        }
        BlockKind::Paragraph(content) => div()
            .whitespace_normal()
            .line_height(
                type_scale
                    .content_line()
                    .max(inline_math_height(content, math_images).unwrap_or(Pixels::ZERO)),
            )
            .child(render_inline(content, key, render_context))
            .into_any_element(),
        BlockKind::Code {
            language,
            text,
            highlights,
        } => {
            let mut code = div()
                .rounded_md()
                .bg(color::current(cx).panel_background)
                .p(scale::S12)
                .font(typography::content_font())
                .text_size(type_scale.content_size())
                .flex()
                .flex_col();
            if let Some(language) = language {
                code = code.child(
                    div()
                        .mb(scale::S8)
                        .text_size(type_scale.content_size() * 0.85)
                        .text_color(color::current(cx).text_muted)
                        .child(language.clone()),
                );
            }
            let styles = highlights
                .as_ref()
                .map(|highlights| syntax::style_table(&highlights.capture_names, cx));
            let mut line_start = 0;
            let mut highlight_index = 0;
            for line in crate::document::code_lines(text) {
                let line_end = line_start + line.len();
                code = code.child(div().child(render_code_line(
                    line,
                    line_start..line_end,
                    highlights.as_ref(),
                    styles.as_deref(),
                    &mut highlight_index,
                    render_context,
                )));
                line_start = line_end + 1;
            }
            code.into_any_element()
        }
        BlockKind::Quote(blocks) => {
            let children = blocks
                .iter()
                .map(|block| render_block(block, next_key, list_depth, namespace, render_context))
                .collect::<Vec<_>>();
            div()
                .border_l_2()
                .border_color(color::current(cx).border)
                .pl(scale::S12)
                .flex()
                .flex_col()
                .gap(scale::S16)
                .text_color(color::current(cx).text_muted)
                .children(children)
                .into_any_element()
        }
        BlockKind::List { start, items } => {
            let marker_width = list_marker_width(*start, items.len(), type_scale);
            let children = items
                .iter()
                .enumerate()
                .map(|(item_index, item)| {
                    let marker_text = start.map_or_else(
                        || "•".to_owned(),
                        |start| format!("{}.", start + item_index as u64),
                    );
                    // 项头只占第一行的高度并顶端对齐，避免多行内容的项头被垂直居中。
                    let first_line_height = item
                        .first()
                        .map(|block| block_line_height(block, type_scale, math_images))
                        .unwrap_or_else(|| type_scale.content_line());
                    let mut item_children = item
                        .iter()
                        .map(|block| {
                            render_block(block, next_key, list_depth + 1, namespace, render_context)
                        })
                        .collect::<Vec<_>>()
                        .into_iter();
                    let marker_gap = scale::to_pixels(scale::S2, render_context.window);
                    let mut content = div().flex().flex_col().gap(scale::S4);
                    if let Some(first_child) = item_children.next() {
                        // 项头绝对定位到项首行顶端，避免被 flex 拉伸后垂直居中到多行内容中间。
                        content = content.child(
                            div()
                                .relative()
                                .w_full()
                                .flex()
                                .line_height(first_line_height)
                                .child(
                                    div()
                                        .debug_selector(|| "markdown-list-marker".into())
                                        .absolute()
                                        .left_0()
                                        .top_0()
                                        .w(marker_width)
                                        .h(first_line_height)
                                        .line_height(first_line_height)
                                        .text_left()
                                        .text_color(color::current(cx).text_muted)
                                        .child(marker_text.clone()),
                                )
                                .child(
                                    div()
                                        .debug_selector(|| "markdown-list-item-content".into())
                                        .flex_1()
                                        .min_w_0()
                                        .pl(marker_width + marker_gap)
                                        .child(first_child),
                                ),
                        );
                    } else {
                        content = content.child(
                            div()
                                .w(marker_width)
                                .flex_none()
                                .line_height(first_line_height)
                                .text_left()
                                .text_color(color::current(cx).text_muted)
                                .child(marker_text.clone()),
                        );
                    }
                    content.children(item_children)
                })
                .collect::<Vec<_>>();
            div()
                .when(list_depth > 0, |list| list.pl(scale::S16))
                .flex()
                .flex_col()
                .gap(scale::S4)
                .children(children)
                .into_any_element()
        }
        BlockKind::Table {
            alignments,
            header,
            rows,
        } => {
            let mut table = div()
                .id(ElementId::named_usize(
                    format!("markdown-table-{namespace}"),
                    key,
                ))
                .w_full()
                .overflow_x_scroll()
                .restrict_scroll_to_axis()
                .rounded_md()
                .border_1()
                .border_color(color::current(cx).border)
                .flex()
                .flex_col();
            if !header.is_empty() {
                table = table.child(render_table_row(
                    header,
                    alignments,
                    true,
                    next_key,
                    render_context,
                ));
            }
            for row in rows {
                table = table.child(render_table_row(
                    row,
                    alignments,
                    false,
                    next_key,
                    render_context,
                ));
            }
            table.into_any_element()
        }
        BlockKind::Image { source, alt } => render_image(source, alt, source_directory, cx),
        BlockKind::Math { source, display } => {
            render_math_block(source, *display, key, math_images, cx)
        }
        BlockKind::Mermaid { source } => {
            render_mermaid_block(source, namespace, key, render_context.mermaid_images, cx)
        }
        BlockKind::Rule => div()
            .h(fixed::HAIRLINE)
            .w_full()
            .bg(color::current(cx).border)
            .into_any_element(),
    }
}

/// 行内公式的最大逻辑高度。
///
/// 公式图像高于正文行高时需要把所在行撑高，否则相邻行会重叠。
fn inline_math_height(content: &[Inline], math_images: &MathImages) -> Option<Pixels> {
    content
        .iter()
        .filter(|inline| inline.style.math)
        .filter_map(|inline| math_images.get(&inline.text))
        .filter_map(|result| result.as_ref().ok())
        .map(|math| math.size.height)
        .max()
}

/// 块首行的有效行高；列表项头据此对齐到首行。
fn block_line_height(
    block: &Block,
    type_scale: typography::Typography,
    math_images: &MathImages,
) -> Pixels {
    match &block.kind {
        BlockKind::Heading { level, content, .. } => {
            let size = heading_size(*level, type_scale);
            heading_line_height(*level, size, type_scale)
                .max(inline_math_height(content, math_images).unwrap_or(Pixels::ZERO))
        }
        BlockKind::Paragraph(content) => type_scale
            .content_line()
            .max(inline_math_height(content, math_images).unwrap_or(Pixels::ZERO)),
        _ => type_scale.content_line(),
    }
}

fn list_marker_width(
    start: Option<u64>,
    item_count: usize,
    type_scale: typography::Typography,
) -> Pixels {
    let marker_char_count = list_marker_char_count(start, item_count);
    // 标记列按字符数预留，正文与编号之间只保留布局间距。
    type_scale.content_size() * (marker_char_count as f32 * 0.6)
}

fn list_marker_char_count(start: Option<u64>, item_count: usize) -> usize {
    start.map_or(1, |start| {
        let last_marker = start.saturating_add(item_count.saturating_sub(1) as u64);
        last_marker.to_string().len() + 1
    })
}

fn render_image(source: &str, alt: &str, source_directory: Option<&Path>, cx: &App) -> AnyElement {
    let fallback_alt = alt.to_owned();
    let muted = color::current(cx).text_muted;
    let loading_muted = muted;
    let image = if source.starts_with("http://") || source.starts_with("https://") {
        img(source.to_owned())
    } else {
        let path = Path::new(source);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            source_directory.map_or_else(|| path.to_path_buf(), |directory| directory.join(path))
        };
        img(path)
    }
    .max_w_full()
    .object_fit(ObjectFit::Contain)
    .with_loading(move || {
        div()
            .text_color(loading_muted)
            .child("正在加载图片…")
            .into_any_element()
    })
    .with_fallback(move || {
        div()
            .text_color(muted)
            .child(format!("无法加载图片：{fallback_alt}"))
            .into_any_element()
    });
    div()
        .w_full()
        .flex()
        .justify_center()
        .flex_none()
        .child(image)
        .into_any_element()
}

fn render_table_row(
    cells: &[Vec<Inline>],
    alignments: &[Alignment],
    is_header: bool,
    next_key: &mut usize,
    render_context: &MarkdownRenderContext<'_>,
) -> AnyElement {
    let cx = render_context.cx;
    div()
        .flex()
        .w_full()
        .when(!is_header, |row| {
            row.border_t_1().border_color(color::current(cx).border)
        })
        .children(cells.iter().enumerate().map(|(cell_index, cell_content)| {
            let cell = div()
                .flex_1()
                .min_w(scale::S32)
                .p(scale::S2)
                .line_height(
                    render_context.type_scale.content_line().max(
                        inline_math_height(cell_content, render_context.math_images)
                            .unwrap_or(Pixels::ZERO),
                    ),
                )
                .when(cell_index > 0, |cell| {
                    cell.border_l_1().border_color(color::current(cx).border)
                })
                .when(is_header, |cell| cell.font_weight(FontWeight::SEMIBOLD));
            let cell = match alignments
                .get(cell_index)
                .copied()
                .unwrap_or(Alignment::None)
            {
                Alignment::Left | Alignment::None => cell,
                Alignment::Center => cell.text_center(),
                Alignment::Right => cell.text_right(),
            };
            let key = *next_key;
            *next_key += 1;
            cell.child(render_inline(cell_content, key, render_context))
        }))
        .into_any_element()
}

/// 行内文本的构建结果：一段连续文本，加上叠绘在文本布局上的装饰。
#[derive(Default)]
struct InlineText {
    /// 渲染文本（公式以占位符或错误消息呈现）。
    text: String,
    /// 选择文本的局部长度（公式按 LaTeX 源码计）。
    selectable_len: usize,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    links: Vec<String>,
    link_ranges: Vec<Range<usize>>,
    code_ranges: Vec<Range<usize>>,
    math_placements: Vec<MathPlacement>,
    /// 渲染文本与选择文本的区间差异（公式占位/错误消息 vs LaTeX 源码）。
    placeholders: Vec<PlaceholderMap>,
}

/// 光标竖线宽度；与 Editor 的 bar 光标一致。
const CARET_WIDTH: Pixels = px(2.);

/// 行内公式在文本流中的位置、图像与逻辑尺寸。
#[derive(Clone)]
struct MathPlacement {
    /// 公式占位符在文本中的起始字节偏移。
    byte_offset: usize,
    image: Arc<RenderImage>,
    size: Size<Pixels>,
    /// 占位符预留宽度；不小于公式宽度，用于把公式居中绘制在占位区域内。
    reserved: Pixels,
}

impl InlineText {
    fn push(&mut self, text: &str, style: &InlineStyle, cx: &App) {
        let start = self.text.len();
        self.text.push_str(text);
        let end = self.text.len();
        if start == end {
            return;
        }
        self.selectable_len += text.len();
        if style.code {
            self.code_ranges.push(start..end);
        }
        if style.emphasis || style.strong || style.strikethrough || style.link.is_some() {
            self.highlights
                .push((start..end, inline_highlight(style, cx)));
        }
        if let Some(url) = &style.link {
            self.link_ranges.push(start..end);
            self.links.push(url.clone());
        }
    }

    /// 公式以不可断行空格预留宽度，绘制阶段再把图像覆盖到占位区域。
    ///
    /// 这样公式后的文字会继续排在同一行并按容器宽度换行，而不是整体掉到下一行。
    /// 渲染文本与选择文本（LaTeX 源码）长度不同时记录占位映射，
    /// 命中与选区绘制经它双向换算；未加载时两者同为源码，无需映射。
    fn push_math(&mut self, source: &str, render_context: &MarkdownRenderContext<'_>) {
        let selectable_start = self.selectable_len;
        self.selectable_len += source.len();
        match render_context.math_images.get(source) {
            Some(Ok(math)) => {
                let space =
                    math_placeholder_advance(render_context.window, render_context.type_scale);
                // 向上取整保证占位宽度不小于公式宽度，避免公式右侧被后续文字覆盖。
                let count = if space > Pixels::ZERO {
                    (math.size.width / space).ceil().max(1.) as usize
                } else {
                    1
                };
                let byte_offset = self.text.len();
                self.text
                    .extend(std::iter::repeat_n(MATH_PLACEHOLDER, count));
                self.math_placements.push(MathPlacement {
                    byte_offset,
                    image: math.image.clone(),
                    size: math.size,
                    reserved: space * count as f32,
                });
                self.placeholders.push(PlaceholderMap {
                    render: byte_offset..byte_offset + count * MATH_PLACEHOLDER.len_utf8(),
                    selectable: selectable_start..self.selectable_len,
                });
            }
            Some(Err(error)) => {
                let render_start = self.text.len();
                self.push_muted(&format!("公式渲染失败：{error}"), render_context.cx);
                self.placeholders.push(PlaceholderMap {
                    render: render_start..self.text.len(),
                    selectable: selectable_start..self.selectable_len,
                });
            }
            None => {
                self.push_muted(source, render_context.cx);
            }
        }
    }

    fn push_muted(&mut self, text: &str, cx: &App) {
        let start = self.text.len();
        self.text.push_str(text);
        let end = self.text.len();
        if start < end {
            self.highlights.push((
                start..end,
                HighlightStyle {
                    color: Some(color::current(cx).text_muted.into()),
                    ..Default::default()
                },
            ));
        }
    }

    fn finish(self, key: usize, render_context: &MarkdownRenderContext<'_>) -> AnyElement {
        let InlineText {
            text,
            selectable_len,
            highlights,
            links,
            link_ranges,
            code_ranges,
            math_placements,
            placeholders,
        } = self;
        let rendered_text = text.clone();
        let text = StyledText::new(text).with_highlights(highlights);
        let layout = text.layout().clone();
        // 空文本不产生可选片段（选择投影同样跳过），不注册命中几何。
        let selection = (!rendered_text.is_empty() || selectable_len > 0).then(|| {
            let run_index = render_context
                .selection_layout
                .borrow_mut()
                .register_next_run(rendered_text.len(), placeholders, layout.clone());
            let run_links = link_ranges
                .iter()
                .cloned()
                .zip(links.iter().cloned())
                .map(|(range, target)| RunLink { range, target })
                .collect();
            render_context
                .selection_layout
                .borrow_mut()
                .set_run_links(run_index, run_links);
            InlineSelectionContext {
                layout: Rc::clone(render_context.selection_layout),
                run_index,
                selection: render_context.selection.clone(),
                caret: render_context.caret,
            }
        });
        let text = if links.is_empty() {
            text.into_any_element()
        } else {
            // 手型光标依赖 gpui 的可点击区间 hover 逻辑（click_listener 存在时设置）。
            // 跳转不在这里合成 click：up 监听要等下一帧注册，快速点击会丢失；
            // 由事件面在 mouse down 时经命中几何直接判定。
            InteractiveText::new(("markdown-link", key), text)
                .on_click(link_ranges, |_, _, _| {})
                .into_any_element()
        };
        MarkdownInlineText {
            text,
            layout,
            code_ranges,
            code_color: color::current(render_context.cx).border_variant.into(),
            math_placements,
            selection,
            selection_color: color::current(render_context.cx)
                .editor_selection_background
                .into(),
            caret_color: color::current(render_context.cx).editor_cursor.into(),
        }
        .into_any_element()
    }
}

fn inline_highlight(style: &InlineStyle, cx: &App) -> HighlightStyle {
    HighlightStyle {
        font_style: style.emphasis.then_some(FontStyle::Italic),
        font_weight: style.strong.then_some(FontWeight::BOLD),
        strikethrough: style.strikethrough.then_some(StrikethroughStyle {
            thickness: px(2.),
            color: Some(color::current(cx).text.into()),
        }),
        color: style
            .link
            .as_ref()
            .map(|_| color::current(cx).icon_accent.into()),
        underline: style.link.as_ref().map(|_| UnderlineStyle {
            thickness: px(2.),
            color: Some(color::current(cx).icon_accent.into()),
            wavy: false,
        }),
        ..Default::default()
    }
}

fn math_placeholder_advance(window: &Window, type_scale: typography::Typography) -> Pixels {
    let font = window.text_style().font();
    let font_id = window.text_system().resolve_font(&font);
    window
        .text_system()
        .layout_width(font_id, type_scale.content_size(), MATH_PLACEHOLDER)
}

fn render_inline(
    content: &[Inline],
    key: usize,
    render_context: &MarkdownRenderContext<'_>,
) -> AnyElement {
    if content.iter().any(|inline| inline.style.math) {
        let mut inline_text = InlineText::default();
        for inline in content {
            if inline.style.math {
                inline_text.push_math(&inline.text, render_context);
            } else {
                inline_text.push(&inline.text, &inline.style, render_context.cx);
            }
        }
        return inline_text.finish(key, render_context);
    }
    render_text_inline(content, key, render_context)
}

fn render_text_inline(
    content: &[Inline],
    key: usize,
    render_context: &MarkdownRenderContext<'_>,
) -> AnyElement {
    let mut inline_text = InlineText::default();
    for inline in content {
        inline_text.push(&inline.text, &inline.style, render_context.cx);
    }
    inline_text.finish(key, render_context)
}

/// 处理预览内链接目标的跳转：
/// 文档内锚点滚动揭示，文件路径交给工作区打开，其余交给系统浏览器。
fn open_markdown_link(
    target: &str,
    source_directory: Option<&Path>,
    open_path: Option<&OpenPathCallback>,
    heading_anchors: &HashMap<String, MultiBufferAnchor>,
    pending_reveal: &Rc<RefCell<Option<MultiBufferAnchor>>>,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(slug) = target.strip_prefix('#') {
        if let Some(anchor) = heading_anchors.get(slug).copied() {
            *pending_reveal.borrow_mut() = Some(anchor);
            window.refresh();
        }
        return;
    }
    if let Some(path) = resolve_markdown_file_link(target, source_directory)
        && let Some(open_path) = open_path
    {
        open_path(path, window, cx);
    } else {
        cx.open_url(target);
    }
}

fn resolve_markdown_file_link(link: &str, source_directory: Option<&Path>) -> Option<PathBuf> {
    let (link, _) = link.split_once('#').unwrap_or((link, ""));
    if link.is_empty() || link.contains("://") || link.starts_with("mailto:") {
        return None;
    }
    let source_directory = source_directory?;
    let path = Path::new(link);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        source_directory.join(path)
    })
}

/// 内联文本元素携带的选择渲染上下文：命中几何、run 索引与选区、光标快照。
struct InlineSelectionContext {
    layout: Rc<RefCell<SelectionLayout>>,
    run_index: usize,
    selection: Option<Range<usize>>,
    caret: Option<usize>,
}

/// 在文本布局上叠加绘制行内代码背景、行内公式图像、选区高亮与光标。
struct MarkdownInlineText {
    text: AnyElement,
    layout: TextLayout,
    code_ranges: Vec<Range<usize>>,
    code_color: Hsla,
    math_placements: Vec<MathPlacement>,
    selection: Option<InlineSelectionContext>,
    selection_color: Hsla,
    caret_color: Hsla,
}

impl Element for MarkdownInlineText {
    type RequestLayoutState = ();
    type PrepaintState = ();

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
        (self.text.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.text.prepaint(window, cx);
        if let Some(context) = &self.selection {
            context
                .layout
                .borrow_mut()
                .set_run_bounds(context.run_index, self.layout.bounds());
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        paint_inline_code_chips(&self.layout, &self.code_ranges, self.code_color, window);
        paint_inline_math(&self.layout, &self.math_placements, window);
        if let Some(context) = &self.selection {
            let selection_layout = context.layout.borrow();
            if let Some(selection) = &context.selection
                && let Some(render_selection) =
                    selection_layout.render_range_for(context.run_index, selection)
            {
                paint_text_selection(
                    &self.layout,
                    &render_selection,
                    self.selection_color,
                    window,
                );
            }
            // 光标画在所属 run（含段尾间隙）内：2px 竖线，高为一个行高。
            if let Some(caret) = context.caret
                && let Some(render_offset) =
                    selection_layout.render_offset_for(context.run_index, caret)
                && let Some(position) = self.layout.position_for_index(render_offset)
            {
                window.paint_quad(fill(
                    Bounds::new(position, size(CARET_WIDTH, self.layout.line_height())),
                    self.caret_color,
                ));
            }
        }
        self.text.paint(window, cx);
    }
}

impl gpui::IntoElement for MarkdownInlineText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// 绘制选区高亮：连续行轮廓走共享形状原语（圆角、阶梯、行尾延伸），与 Editor 视觉一致。
fn paint_text_selection(
    layout: &TextLayout,
    selection: &RenderSelection,
    color: Hsla,
    window: &mut Window,
) {
    let text_align = window.text_style().text_align;
    let line_height = layout.line_height();
    let corner_radius = selection_corner_radius(line_height);
    let overshoot = selection_line_end_overshoot(corner_radius);
    let mut rows = Vec::new();
    for_each_text_range_row(
        layout,
        selection.range.clone(),
        text_align,
        |row, start_x, end_x| {
            rows.push((row, start_x, end_x));
        },
    );
    // 选区继续到后续块时，本块末行也做行尾延伸，保持跨块视觉衔接。
    let last_row = selection
        .ends_here
        .then(|| rows.last().map(|row| row.0))
        .flatten();
    let contour = rows
        .iter()
        .map(|&(row, start_x, end_x)| SelectionContourLine {
            index: row,
            start_x,
            end_x: if Some(row) == last_row {
                end_x
            } else {
                end_x + overshoot
            },
        })
        .collect::<Vec<_>>();
    let origin_y = layout.bounds().origin.y;
    for shape in selection_segment_shapes(&contour, corner_radius) {
        let y = origin_y + line_height * shape.index as f32;
        paint_selection_segment(
            Bounds::from_corners(point(shape.start_x, y), point(shape.end_x, y + line_height)),
            shape.corners,
            corner_radius,
            color,
            window,
        );
    }
}

fn for_each_text_range_bounds(
    layout: &TextLayout,
    range: Range<usize>,
    text_align: TextAlign,
    mut paint: impl FnMut(Bounds<Pixels>),
) {
    let line_height = layout.line_height();
    let origin_y = layout.bounds().origin.y;
    for_each_text_range_row(layout, range, text_align, |row, start_x, end_x| {
        let row_top = origin_y + line_height * row as f32;
        paint(Bounds::from_corners(
            point(start_x, row_top),
            point(end_x, row_top + line_height),
        ));
    });
}

/// 遍历区间覆盖的显示行片段：全局行索引（自布局顶部按行高累计）与行内水平区间。
fn for_each_text_range_row(
    layout: &TextLayout,
    range: Range<usize>,
    text_align: TextAlign,
    mut visit: impl FnMut(usize, Pixels, Pixels),
) {
    // GPUI 将软换行边界索引归到前一行；按塑形后的行片段裁范围，才能正确覆盖下一行首字。
    let line_bounds = layout.bounds();
    let mut row_index = 0;
    let mut line_start = 0;

    for line in layout.line_layouts() {
        let line_end = line_start + line.len();
        let unwrapped_layout = &line.unwrapped_layout;
        let mut row_start = line_start;
        let mut row_start_x = Pixels::ZERO;
        let row_ends = line
            .wrap_boundaries()
            .iter()
            .map(|boundary| {
                let glyph = &unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix];
                (line_start + glyph.index, glyph.position.x)
            })
            .chain([(line_end, unwrapped_layout.width)]);

        for (row_end, row_end_x) in row_ends {
            let selected_start = range.start.max(row_start);
            let selected_end = range.end.min(row_end);
            if selected_start < selected_end {
                let alignment_offset = match text_align {
                    TextAlign::Left => Pixels::ZERO,
                    TextAlign::Center => {
                        ((line_bounds.size.width - (row_end_x - row_start_x)) / 2.).max(px(0.))
                    }
                    TextAlign::Right => {
                        (line_bounds.size.width - (row_end_x - row_start_x)).max(px(0.))
                    }
                };
                let x_for_index = |index| {
                    line_bounds.left()
                        + alignment_offset
                        + unwrapped_layout.x_for_index(index - line_start)
                        - row_start_x
                };
                visit(
                    row_index,
                    x_for_index(selected_start),
                    x_for_index(selected_end),
                );
            }

            row_start = row_end;
            row_start_x = row_end_x;
            row_index += 1;
        }

        line_start = line_end + 1;
    }
}

fn paint_inline_code_chips(
    layout: &TextLayout,
    code_ranges: &[Range<usize>],
    color: Hsla,
    window: &mut Window,
) {
    let text_align = window.text_style().text_align;
    for code_range in code_ranges {
        for_each_text_range_bounds(layout, code_range.clone(), text_align, |bounds| {
            let vertical_inset = bounds.size.height * INLINE_CODE_CHIP_VERTICAL_INSET;
            let chip_outset = scale::to_pixels(scale::S2, window);
            window.paint_quad(
                fill(
                    Bounds::from_corners(
                        point(bounds.left() - chip_outset, bounds.top() + vertical_inset),
                        point(
                            bounds.right() + chip_outset,
                            bounds.bottom() - vertical_inset,
                        ),
                    ),
                    color,
                )
                .corner_radii(INLINE_CODE_CHIP_CORNER_RADIUS),
            );
        });
    }
}

/// 计算行内公式图像在文本布局中的绘制边界。
///
/// 公式占位起点常常正好落在换行边界上；此时按字节偏移定位会归到上一行末尾，
/// 因此这里沿换行边界逐行定位，保证图像落在占位符真正所在的行上。
fn inline_math_bounds(
    layout: &TextLayout,
    placement: &MathPlacement,
    text_align: TextAlign,
) -> Option<Bounds<Pixels>> {
    let line_bounds = layout.bounds();
    let line_height = layout.line_height();
    let mut row_top = line_bounds.origin.y;
    let mut logical_start = 0;

    for line in layout.line_layouts() {
        let logical_end = logical_start + line.len();
        let unwrapped_layout = &line.unwrapped_layout;
        let mut row_start = logical_start;
        let mut row_start_x = Pixels::ZERO;
        let row_ends = line
            .wrap_boundaries()
            .iter()
            .map(|boundary| {
                let glyph = &unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix];
                (glyph.index, glyph.position.x)
            })
            .chain([(logical_end, unwrapped_layout.width)]);

        for (row_end, row_end_x) in row_ends {
            if placement.byte_offset < row_start || placement.byte_offset >= row_end {
                row_start = row_end;
                row_start_x = row_end_x;
                row_top += line_height;
                continue;
            }

            let alignment_offset = match text_align {
                TextAlign::Left => Pixels::ZERO,
                TextAlign::Center => {
                    ((line_bounds.size.width - (row_end_x - row_start_x)) / 2.).max(px(0.))
                }
                TextAlign::Right => {
                    (line_bounds.size.width - (row_end_x - row_start_x)).max(px(0.))
                }
            };
            let x = line_bounds.left()
                + alignment_offset
                + unwrapped_layout.x_for_index(placement.byte_offset)
                - row_start_x;
            let top = row_top + (line_height - placement.size.height) / 2.;
            let left = x + (placement.reserved - placement.size.width) / 2.;
            return Some(Bounds {
                origin: point(left, top),
                size: placement.size,
            });
        }

        logical_start = logical_end + 1;
    }

    None
}

/// 把行内公式图像覆盖到文本流中预留的占位区域。
///
/// 占位宽度由不可断行空格近似公式宽度，图像在占位区域内垂直居中，
/// 因此公式与前后文字共享同一套换行与行高逻辑。
fn paint_inline_math(layout: &TextLayout, placements: &[MathPlacement], window: &mut Window) {
    let text_align = window.text_style().text_align;
    for placement in placements {
        let Some(bounds) = inline_math_bounds(layout, placement, text_align) else {
            continue;
        };
        // 图像已在渲染阶段成功光栅化，绘制失败只可能来自图集分配；预览不做额外恢复。
        let _ = window.paint_image(
            bounds,
            bounds,
            Corners::default(),
            placement.image.clone(),
            0,
            false,
        );
    }
}

fn render_math_inline(source: &str, math_images: &MathImages, cx: &App) -> AnyElement {
    match math_images.get(source) {
        Some(Ok(math)) => img(math.image.clone())
            .object_fit(ObjectFit::Contain)
            .max_w_full()
            .flex_none()
            .into_any_element(),
        Some(Err(error)) => div()
            .text_color(color::current(cx).status_error)
            .child(format!("公式渲染失败：{error}"))
            .into_any_element(),
        None => div()
            .text_color(color::current(cx).text_muted)
            .child("正在渲染公式…")
            .into_any_element(),
    }
}

fn render_math_block(
    source: &str,
    display: bool,
    key: usize,
    math_images: &MathImages,
    cx: &App,
) -> AnyElement {
    div()
        .id(("markdown-math-block", key))
        .w_full()
        .flex()
        .overflow_x_scroll()
        .restrict_scroll_to_axis()
        .when(display, |element| element.justify_center())
        .child(render_math_inline(source, math_images, cx))
        .into_any_element()
}

fn collect_math_sources(blocks: &[Block]) -> HashMap<String, bool> {
    let mut sources = HashMap::new();
    fn collect_inline(inlines: &[Inline], sources: &mut HashMap<String, bool>) {
        for inline in inlines {
            if inline.style.math {
                sources.entry(inline.text.clone()).or_insert(false);
            }
        }
    }
    fn visit(blocks: &[Block], sources: &mut HashMap<String, bool>) {
        for block in blocks {
            match &block.kind {
                BlockKind::Math { source, display } => {
                    sources.entry(source.clone()).or_insert(*display);
                }
                BlockKind::Heading { content, .. } | BlockKind::Paragraph(content) => {
                    collect_inline(content, sources);
                }
                BlockKind::Table { header, rows, .. } => {
                    for cell in header {
                        collect_inline(cell, sources);
                    }
                    for row in rows {
                        for cell in row {
                            collect_inline(cell, sources);
                        }
                    }
                }
                BlockKind::Quote(children) => visit(children, sources),
                BlockKind::List { items, .. } => {
                    for item in items {
                        visit(item, sources);
                    }
                }
                _ => {}
            }
        }
    }
    visit(blocks, &mut sources);
    sources
}

fn render_math(
    source: &str,
    _display: bool,
    font_size: f64,
    color: ratex_types::color::Color,
    renderer: SvgRenderer,
) -> Result<MathImage, String> {
    let nodes = ratex_parser::parse(source).map_err(|error| error.to_string())?;
    let layout_options = ratex_layout::LayoutOptions {
        color,
        ..Default::default()
    };
    let layout = ratex_layout::layout(&nodes, &layout_options);
    let list = ratex_layout::to_display_list(&layout);
    let svg = ratex_svg::render_to_svg(
        &list,
        &ratex_svg::SvgOptions {
            font_size,
            padding: 0.0,
            embed_glyphs: true,
            ..Default::default()
        },
    );
    let image = Image::from_bytes(ImageFormat::Svg, svg.as_bytes().to_vec())
        .to_image_data(renderer)
        .map_err(|error| error.to_string())?;
    // SVG 光栅化时按 SMOOTH_SVG_SCALE_FACTOR 放大，逻辑宽度需还原后再用于文本流占位。
    let raw = image.size(0);
    let size = size(
        px(raw.width.0 as f32 / SMOOTH_SVG_SCALE_FACTOR),
        px(raw.height.0 as f32 / SMOOTH_SVG_SCALE_FACTOR),
    );
    Ok(MathImage { image, size })
}

fn collect_mermaid_sources(blocks: &[Block]) -> Vec<String> {
    fn visit(blocks: &[Block], sources: &mut Vec<String>) {
        for block in blocks {
            match &block.kind {
                BlockKind::Mermaid { source } if !sources.contains(source) => {
                    sources.push(source.clone())
                }
                BlockKind::Quote(children) => visit(children, sources),
                BlockKind::List { items, .. } => {
                    for item in items {
                        visit(item, sources);
                    }
                }
                _ => {}
            }
        }
    }
    let mut sources = Vec::new();
    visit(blocks, &mut sources);
    sources
}

fn render_mermaid(source: &str, renderer: SvgRenderer) -> Result<MathImage, String> {
    let svg = render_mermaid_svg(source)?;
    let image = Image::from_bytes(ImageFormat::Svg, svg.as_bytes().to_vec())
        .to_image_data(renderer)
        .map_err(|error| error.to_string())?;
    let raw = image.size(0);
    let size = size(
        px(raw.width.0 as f32 / SMOOTH_SVG_SCALE_FACTOR),
        px(raw.height.0 as f32 / SMOOTH_SVG_SCALE_FACTOR),
    );
    Ok(MathImage { image, size })
}

fn render_mermaid_svg(source: &str) -> Result<String, String> {
    let config = merman::MermaidConfig::from_value(serde_json::json!({
        "htmlLabels": false,
        "flowchart": { "htmlLabels": false },
    }));
    let output = merman::Renderer::new()
        .with_engine(merman::Engine::new().with_site_config(config))
        .render(merman::RenderRequest::svg(
            source,
            merman::OperationControl::new(),
            merman::SvgRequest {
                pipeline: Some(merman::svg::SvgPipeline::resvg_safe()),
                ..Default::default()
            },
        ))
        .map_err(|error| error.to_string())?;
    let merman::RenderOutput::Svg(Some(svg)) = output else {
        return Err("Mermaid 未生成图表".to_owned());
    };
    Ok(svg.svg().to_owned())
}

fn render_mermaid_block(
    source: &str,
    namespace: usize,
    key: usize,
    images: &MermaidImages,
    cx: &App,
) -> AnyElement {
    let content = match images.get(source) {
        Some(Ok(diagram)) => img(diagram.image.clone())
            .w(diagram.size.width)
            .h(diagram.size.height)
            .flex_none()
            .into_any_element(),
        Some(Err(error)) => div()
            .text_color(color::current(cx).status_error)
            .child(format!("Mermaid 图表渲染失败：{error}"))
            .into_any_element(),
        None => div()
            .text_color(color::current(cx).text_muted)
            .child("正在渲染 Mermaid 图表…")
            .into_any_element(),
    };
    div()
        .id(ElementId::named_usize(
            format!("markdown-mermaid-{namespace}"),
            key,
        ))
        .w_full()
        .flex()
        .overflow_x_scroll()
        .restrict_scroll_to_axis()
        .child(content)
        .into_any_element()
}

fn highlight_code_blocks(
    blocks: &mut [Block],
    language_registry: &Arc<LanguageRegistry>,
    cancellation: &SnippetHighlightCancellation,
) -> bool {
    for block in blocks {
        if cancellation.is_cancelled() {
            return false;
        }
        match &mut block.kind {
            BlockKind::Code {
                language: Some(language),
                text,
                highlights,
            } => {
                *highlights = highlight_snippet_with_cancellation(
                    language_registry,
                    language,
                    text,
                    cancellation,
                )
            }
            BlockKind::Quote(blocks) => {
                if !highlight_code_blocks(blocks, language_registry, cancellation) {
                    return false;
                }
            }
            BlockKind::List { items, .. } => {
                for item in items {
                    if !highlight_code_blocks(item, language_registry, cancellation) {
                        return false;
                    }
                }
            }
            BlockKind::Heading { .. }
            | BlockKind::Paragraph(_)
            | BlockKind::Code { language: None, .. }
            | BlockKind::Table { .. }
            | BlockKind::Image { .. }
            | BlockKind::Math { .. }
            | BlockKind::Mermaid { .. }
            | BlockKind::Rule => {}
        }
    }
    !cancellation.is_cancelled()
}

fn render_code_line(
    line: &str,
    line_range: Range<usize>,
    highlights: Option<&SnippetHighlights>,
    styles: Option<&[HighlightStyle]>,
    highlight_index: &mut usize,
    render_context: &MarkdownRenderContext<'_>,
) -> AnyElement {
    let line_highlights: Vec<(Range<usize>, HighlightStyle)> = highlights
        .zip(styles)
        .map(|(highlights, styles)| {
            visible_highlights_for_line(&highlights.spans, line_range, highlight_index)
                .into_iter()
                .filter_map(|(range, capture)| {
                    styles
                        .get(capture as usize)
                        .copied()
                        .map(|style| (range, style))
                })
                .collect()
        })
        .unwrap_or_default();
    let text = StyledText::new(line.to_owned()).with_highlights(line_highlights);
    let layout = text.layout().clone();
    // 空行不产生可选片段（选择投影同样跳过），不注册命中几何。
    let selection = (!line.is_empty()).then(|| {
        let run_index = render_context
            .selection_layout
            .borrow_mut()
            .register_next_run(line.len(), Vec::new(), layout.clone());
        InlineSelectionContext {
            layout: Rc::clone(render_context.selection_layout),
            run_index,
            selection: render_context.selection.clone(),
            caret: render_context.caret,
        }
    });
    let selection_color = color::current(render_context.cx)
        .editor_selection_background
        .into();
    MarkdownInlineText {
        text: text.into_any_element(),
        layout,
        code_ranges: Vec::new(),
        code_color: selection_color,
        math_placements: Vec::new(),
        selection,
        selection_color,
        caret_color: color::current(render_context.cx).editor_cursor.into(),
    }
    .into_any_element()
}

/// 返回与当前行相交的高亮，并将游标推进到后续行无需再次检查的位置。
///
/// 高亮跨度按文档顺序且互不重叠。
/// 跨行跨度会保留在游标位置，直到其末行处理完毕。
fn visible_highlights_for_line(
    spans: &[HighlightSpan],
    line_range: Range<usize>,
    index: &mut usize,
) -> Vec<(Range<usize>, u32)> {
    while spans
        .get(*index)
        .is_some_and(|span| span.range.end <= line_range.start)
    {
        *index += 1;
    }

    let mut line_highlights = Vec::new();
    let mut current = *index;
    while let Some(span) = spans.get(current) {
        if span.range.start >= line_range.end {
            break;
        }
        let start = span.range.start.max(line_range.start);
        let end = span.range.end.min(line_range.end);
        if start < end {
            line_highlights.push((
                start - line_range.start..end - line_range.start,
                span.capture,
            ));
        }
        if span.range.end > line_range.end {
            break;
        }
        current += 1;
    }
    *index = current;
    line_highlights
}

fn heading_size(level: u8, type_scale: typography::Typography) -> Pixels {
    type_scale.content_size() * heading_scale(level)
}

fn heading_line_height(level: u8, size: Pixels, type_scale: typography::Typography) -> Pixels {
    // 标题继承用户的正文行高比例，但至少为自身字号保留可读的自然行距。
    (type_scale.content_line() * heading_scale(level)).max(size * 1.2)
}

fn heading_scale(level: u8) -> f32 {
    match level {
        1 => 1.8,
        2 => 1.5,
        3 => 1.3,
        _ => 1.1,
    }
}

impl Item for MarkdownPreviewView {
    type Event = MarkdownPreviewEvent;

    fn tab_content_text(&self, cx: &App) -> SharedString {
        self.source_item
            .item_path(cx)
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "Markdown Preview".to_owned())
            .into()
    }

    fn to_item_events(event: &Self::Event, emit: &mut dyn FnMut(ItemEvent)) {
        match event {
            MarkdownPreviewEvent::SourceMetadataChanged => {
                emit(ItemEvent::PathChanged);
                emit(ItemEvent::UpdateTab);
                emit(ItemEvent::UpdateBreadcrumbs);
            }
        }
    }

    fn is_dirty(&self, cx: &App) -> bool {
        self.source_item.is_dirty(cx)
    }

    fn item_path(&self, cx: &App) -> Option<PathBuf> {
        self.source_item.item_path(cx)
    }

    fn navigate_to_byte_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> bool {
        let len = self
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx).len_bytes().get());
        if range.end > len {
            return false;
        }
        let anchor = self.multi_buffer.update(cx, |buffer, cx| {
            buffer.snapshot(cx).anchor_at(range.start, Affinity::Before)
        });
        *self.pending_reveal.borrow_mut() = Some(anchor);
        cx.notify();
        true
    }

    fn visible_source_anchor(&mut self, cx: &mut Context<Self>) -> Option<MultiBufferAnchor> {
        self.top_visible_source_anchor(cx)
    }

    fn reveal_source_anchor(&mut self, anchor: MultiBufferAnchor, cx: &mut Context<Self>) -> bool {
        *self.pending_reveal.borrow_mut() = Some(anchor);
        cx.notify();
        true
    }

    fn rename_path(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        self.source_item.rename_path(from, to, cx);
    }

    fn multi_buffer(&self, _cx: &App) -> Option<Entity<MultiBuffer>> {
        Some(self.multi_buffer.clone())
    }

    fn as_preview_item(
        &self,
        self_handle: &Entity<Self>,
        _cx: &App,
    ) -> Option<Box<dyn PreviewItemHandle>> {
        Some(Box::new(self_handle.clone()))
    }

    fn can_save(&self, cx: &App) -> bool {
        self.source_item.can_save(cx)
    }

    fn conflicted_file_paths(&self, cx: &App) -> Vec<PathBuf> {
        self.source_item.conflicted_file_paths(cx)
    }

    fn save(
        &mut self,
        project: Entity<Project>,
        conflict_policy: zcv_project::FileSaveConflictPolicy,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        self.source_item.save(project, conflict_policy, window, cx)
    }

    fn reload_conflicted_files(
        &mut self,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        self.source_item
            .reload_conflicted_files(project, window, cx)
    }

    fn act_as_type(
        &self,
        type_id: TypeId,
        self_handle: &Entity<Self>,
        cx: &App,
    ) -> Option<AnyEntity> {
        if type_id == TypeId::of::<Self>() {
            Some(self_handle.clone().into())
        } else {
            self.source_item.act_as_type(type_id, cx)
        }
    }
}

impl PreviewItem for MarkdownPreviewView {
    fn source_item(&self, _cx: &App) -> Option<Box<dyn ItemHandle>> {
        Some(self.source_item.boxed_clone())
    }
}

#[cfg(test)]
#[path = "test/view_tests.rs"]
mod tests;
