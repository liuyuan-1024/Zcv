//! 可复用于不同滚动容器的滚动条。

use std::any::Any;
use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, Corners, CursorStyle, DispatchPhase, Element, ElementId,
    GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, IntoElement, LayoutId, ListState,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollHandle, Size,
    Style, UniformListDecoration, UniformListScrollHandle, Window, fill, point, px, relative, size,
};
use zcv_theme::color;

const WIDTH: Pixels = px(6.);
const PADDING: Pixels = px(3.);
/// thumb 最小高度。
pub const MIN_THUMB_SIZE: Pixels = px(25.);

/// 统一滚动条所需的滚动容器接口。
///
/// 该抽象属于 UI 层，使滚动条不依赖某一种列表实现。
pub trait ScrollableHandle: 'static + Any + Sized + Clone {
    fn max_offset(&self) -> Point<Pixels>;
    fn set_offset(&self, point: Point<Pixels>);
    fn offset(&self) -> Point<Pixels>;
    fn viewport(&self) -> Bounds<Pixels>;
    fn drag_started(&self) {}
    fn drag_ended(&self) {}

    fn content_size(&self) -> Size<Pixels> {
        let viewport = self.viewport().size;
        let max_offset = self.max_offset();
        size(
            viewport.width + max_offset.x,
            viewport.height + max_offset.y,
        )
    }
}

impl ScrollableHandle for UniformListScrollHandle {
    fn max_offset(&self) -> Point<Pixels> {
        let size = self.0.borrow().base_handle.max_offset();
        point(size.width, size.height)
    }

    fn set_offset(&self, point: Point<Pixels>) {
        self.0.borrow().base_handle.set_offset(point);
    }

    fn offset(&self) -> Point<Pixels> {
        self.0.borrow().base_handle.offset()
    }

    fn viewport(&self) -> Bounds<Pixels> {
        self.0.borrow().base_handle.bounds()
    }
}

impl ScrollableHandle for ListState {
    fn max_offset(&self) -> Point<Pixels> {
        let size = self.max_offset_for_scrollbar();
        point(size.width, size.height)
    }

    fn set_offset(&self, point: Point<Pixels>) {
        self.set_offset_from_scrollbar(point);
    }

    fn offset(&self) -> Point<Pixels> {
        self.scroll_px_offset_for_scrollbar()
    }

    fn viewport(&self) -> Bounds<Pixels> {
        self.viewport_bounds()
    }

    fn drag_started(&self) {
        self.scrollbar_drag_started();
    }

    fn drag_ended(&self) {
        self.scrollbar_drag_ended();
    }
}

impl ScrollableHandle for ScrollHandle {
    fn max_offset(&self) -> Point<Pixels> {
        let size = self.max_offset();
        point(size.width, size.height)
    }

    fn set_offset(&self, point: Point<Pixels>) {
        self.set_offset(point);
    }

    fn offset(&self) -> Point<Pixels> {
        self.offset()
    }

    fn viewport(&self) -> Bounds<Pixels> {
        self.bounds()
    }
}

/// 共享垂直滚动条，可作为统一列表的 decoration 使用。
#[derive(Clone)]
pub struct Scrollbar<T: ScrollableHandle> {
    handle: T,
    interaction: Rc<ScrollbarInteraction>,
}

impl<T: ScrollableHandle> Scrollbar<T> {
    pub fn vertical(handle: T) -> Self {
        Self {
            handle,
            interaction: Rc::new(ScrollbarInteraction::default()),
        }
    }
}

impl<T: ScrollableHandle> IntoElement for Scrollbar<T> {
    type Element = ScrollbarElement<T>;

    fn into_element(self) -> Self::Element {
        ScrollbarElement {
            handle: self.handle,
            interaction: self.interaction,
            origin: point(Pixels::ZERO, Pixels::ZERO),
        }
    }
}

impl<T: ScrollableHandle> UniformListDecoration for Scrollbar<T> {
    fn compute(
        &self,
        _visible_range: std::ops::Range<usize>,
        _bounds: Bounds<Pixels>,
        scroll_offset: Point<Pixels>,
        _item_height: Pixels,
        _item_count: usize,
        _window: &mut Window,
        _cx: &mut App,
    ) -> AnyElement {
        ScrollbarElement {
            handle: self.handle.clone(),
            interaction: Rc::clone(&self.interaction),
            origin: point(-scroll_offset.x, -scroll_offset.y),
        }
        .into_any_element()
    }
}

#[derive(Default)]
struct ScrollbarInteraction {
    dragging: Cell<bool>,
    last_mouse_y: Cell<Pixels>,
    /// 拖拽期间的乐观位置。
    /// 滚动容器异步更新时，不能每次都回读可能滞后的 offset。
    drag_scroll_top: Cell<Pixels>,
}

/// 滚动条的可渲染元素；通常通过 [`Scrollbar`] 构造。
pub struct ScrollbarElement<T: ScrollableHandle> {
    handle: T,
    interaction: Rc<ScrollbarInteraction>,
    origin: Point<Pixels>,
}

impl<T: ScrollableHandle> IntoElement for ScrollbarElement<T> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[derive(Clone)]
/// 滚动条在布局阶段计算出的交互区域与滑块位置。
pub struct ScrollbarLayout {
    hitbox: Hitbox,
    thumb_bounds: Bounds<Pixels>,
    scroll_per_pixel: f32,
}

impl<T: ScrollableHandle> Element for ScrollbarElement<T> {
    type RequestLayoutState = ();
    type PrepaintState = Option<ScrollbarLayout>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        let viewport_bounds = Bounds::new(bounds.origin + self.origin, bounds.size);
        let max_scroll = self.handle.max_offset().y;
        let track_height = (viewport_bounds.size.height - PADDING * 2.).max(Pixels::ZERO);
        if max_scroll <= Pixels::ZERO || track_height <= Pixels::ZERO {
            return None;
        }

        let thumb_track_top = viewport_bounds.top() + PADDING;
        let content_height = track_height + max_scroll;
        let thumb_height = (track_height * (track_height / content_height))
            .max(MIN_THUMB_SIZE)
            .min(track_height);
        let travel = (track_height - thumb_height).max(px(1.));
        let scroll_per_pixel = max_scroll / travel;
        let scroll_top = (-self.handle.offset().y).clamp(Pixels::ZERO, max_scroll);
        let thumb_top = thumb_track_top + scroll_top / scroll_per_pixel;
        let thumb_bounds = Bounds::new(
            point(viewport_bounds.right() - WIDTH - PADDING, thumb_top),
            size(WIDTH, thumb_height),
        );
        // 可见滑块保持紧凑；交互区域扩展到右侧边缘，避免细滑块难以命中。
        let interaction_bounds = Bounds::new(
            point(thumb_bounds.left() - PADDING, thumb_bounds.top()),
            size(
                thumb_bounds.size.width + PADDING * 2.,
                thumb_bounds.size.height,
            ),
        );

        Some(ScrollbarLayout {
            hitbox: window
                .insert_hitbox(interaction_bounds, HitboxBehavior::BlockMouseExceptScroll),
            thumb_bounds,
            scroll_per_pixel,
        })
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        layout: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(layout) = layout.clone() else {
            return;
        };
        let dragging = self.interaction.dragging.get();
        let hovered = layout.thumb_bounds.contains(&window.mouse_position());
        let colors = color::current(cx);
        let thumb_color = if dragging {
            colors.scrollbar_thumb_active_background
        } else if hovered {
            colors.scrollbar_thumb_hover_background
        } else {
            colors.scrollbar_thumb_background
        };
        window.paint_quad(
            fill(layout.thumb_bounds, thumb_color)
                .corner_radii(Corners::all(layout.thumb_bounds.size.width / 2.)),
        );

        if dragging {
            window.set_window_cursor_style(CursorStyle::Arrow);
        } else {
            window.set_cursor_style(CursorStyle::Arrow, &layout.hitbox);
        }

        let interaction = Rc::clone(&self.interaction);
        let handle = self.handle.clone();
        let move_layout = layout.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            if interaction.dragging.get() && event.dragging() {
                let delta = event.position.y - interaction.last_mouse_y.get();
                interaction.last_mouse_y.set(event.position.y);
                let max_scroll = handle.max_offset().y;
                let scroll_top = (interaction.drag_scroll_top.get()
                    + delta * move_layout.scroll_per_pixel)
                    .clamp(Pixels::ZERO, max_scroll);
                interaction.drag_scroll_top.set(scroll_top);
                let mut offset = handle.offset();
                offset.y = -scroll_top;
                handle.set_offset(offset);
                window.refresh();
                cx.stop_propagation();
            } else if interaction.dragging.replace(false) {
                handle.drag_ended();
                window.refresh();
            }
        });

        let interaction = Rc::clone(&self.interaction);
        let handle = self.handle.clone();
        let down_layout = layout.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            // 不能依赖 is_hovered：终端刚接收键盘输入时，框架会暂时关闭悬停状态。
            if phase != DispatchPhase::Bubble
                || event.button != MouseButton::Left
                || !down_layout.hitbox.bounds.contains(&event.position)
            {
                return;
            }
            interaction.dragging.set(true);
            interaction.last_mouse_y.set(event.position.y);
            interaction
                .drag_scroll_top
                .set((-handle.offset().y).clamp(Pixels::ZERO, handle.max_offset().y));
            handle.drag_started();
            window.refresh();
            cx.stop_propagation();
        });

        let interaction = Rc::clone(&self.interaction);
        let handle = self.handle.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                return;
            }
            if interaction.dragging.replace(false) {
                handle.drag_ended();
                window.refresh();
                cx.stop_propagation();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    use gpui::{Context, Render, TestAppContext, Window, div, prelude::*};

    #[derive(Clone)]
    struct TestScrollHandle {
        requested_offset: Rc<Cell<Pixels>>,
        drag_started: Rc<Cell<bool>>,
    }

    impl ScrollableHandle for TestScrollHandle {
        fn max_offset(&self) -> Point<Pixels> {
            point(Pixels::ZERO, px(1000.))
        }

        fn set_offset(&self, point: Point<Pixels>) {
            self.requested_offset.set(point.y);
        }

        fn offset(&self) -> Point<Pixels> {
            // 模拟异步容器：拖拽事件之间，已呈现的偏移还没有更新。
            point(Pixels::ZERO, Pixels::ZERO)
        }

        fn viewport(&self) -> Bounds<Pixels> {
            Bounds::default()
        }

        fn drag_started(&self) {
            self.drag_started.set(true);
        }
    }

    struct ScrollbarTestView {
        scrollbar: Scrollbar<TestScrollHandle>,
    }

    impl Render for ScrollbarTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .relative()
                .child(div().absolute().inset_0().child(self.scrollbar.clone()))
        }
    }

    #[gpui::test]
    fn dragging_the_scrollbar_thumb_updates_its_handle(cx: &mut TestAppContext) {
        let handle = TestScrollHandle {
            requested_offset: Rc::new(Cell::new(Pixels::ZERO)),
            drag_started: Rc::new(Cell::new(false)),
        };
        let scrollbar = Scrollbar::vertical(handle.clone());
        let (_, cx) = cx.add_window_view(move |_window, _cx| ScrollbarTestView { scrollbar });
        cx.refresh().expect("测试窗口应完成首次绘制");

        let (right, top) = cx.update(|window, _| (window.bounds().right(), window.bounds().top()));
        let thumb = point(right - px(4.), top + px(10.));
        cx.simulate_mouse_down(thumb, MouseButton::Left, gpui::Modifiers::default());
        assert!(handle.drag_started.get(), "按下滑块应开始拖拽");
        cx.refresh().expect("按下滑块后应刷新");
        cx.simulate_mouse_move(
            point(thumb.x, thumb.y + px(120.)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        let first_offset = handle.requested_offset.get();
        cx.simulate_mouse_move(
            point(thumb.x, thumb.y + px(240.)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );

        assert!(
            handle.requested_offset.get() < first_offset,
            "异步偏移尚未刷新时，连续拖动仍应累积"
        );
    }
}
