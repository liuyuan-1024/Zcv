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
const MIN_THUMB_SIZE: Pixels = px(25.);

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
}

struct ScrollbarElement<T: ScrollableHandle> {
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
struct ScrollbarLayout {
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

        Some(ScrollbarLayout {
            hitbox: window.insert_hitbox(thumb_bounds, HitboxBehavior::BlockMouseExceptScroll),
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
                let mut offset = handle.offset();
                let max_scroll = handle.max_offset().y;
                let scroll_top = (-offset.y + delta * move_layout.scroll_per_pixel)
                    .clamp(Pixels::ZERO, max_scroll);
                offset.y = -scroll_top;
                handle.set_offset(offset);
                window.refresh();
                cx.stop_propagation();
            } else if interaction.dragging.replace(false) {
                handle.drag_ended();
                window.refresh();
            }
        });

        if !dragging {
            let interaction = Rc::clone(&self.interaction);
            let handle = self.handle.clone();
            let down_layout = layout.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble
                    || event.button != MouseButton::Left
                    || !down_layout.hitbox.is_hovered(window)
                {
                    return;
                }
                interaction.dragging.set(true);
                interaction.last_mouse_y.set(event.position.y);
                handle.drag_started();
                window.refresh();
                cx.stop_propagation();
            });
        } else {
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
}
