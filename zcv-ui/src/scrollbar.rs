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

#[derive(Clone, Copy)]
enum ScrollbarAxis {
    Vertical,
    Horizontal,
}

impl ScrollbarAxis {
    fn max_offset(self, offset: Point<Pixels>) -> Pixels {
        match self {
            Self::Vertical => offset.y,
            Self::Horizontal => offset.x,
        }
    }

    fn coordinate(self, point: Point<Pixels>) -> Pixels {
        match self {
            Self::Vertical => point.y,
            Self::Horizontal => point.x,
        }
    }

    fn set_offset(self, point: &mut Point<Pixels>, value: Pixels) {
        match self {
            Self::Vertical => point.y = value,
            Self::Horizontal => point.x = value,
        }
    }
}

impl ScrollableHandle for UniformListScrollHandle {
    fn max_offset(&self) -> Point<Pixels> {
        self.0.borrow().base_handle.max_offset()
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
        self.max_offset_for_scrollbar()
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
        self.max_offset()
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
    axis: ScrollbarAxis,
    interaction: Rc<ScrollbarInteraction>,
}

impl<T: ScrollableHandle> Scrollbar<T> {
    pub fn vertical(handle: T) -> Self {
        Self {
            handle,
            axis: ScrollbarAxis::Vertical,
            interaction: Rc::new(ScrollbarInteraction::default()),
        }
    }

    pub fn horizontal(handle: T) -> Self {
        Self {
            handle,
            axis: ScrollbarAxis::Horizontal,
            interaction: Rc::new(ScrollbarInteraction::default()),
        }
    }
}

impl<T: ScrollableHandle> IntoElement for Scrollbar<T> {
    type Element = ScrollbarElement<T>;

    fn into_element(self) -> Self::Element {
        ScrollbarElement {
            handle: self.handle,
            axis: self.axis,
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
            axis: self.axis,
            interaction: Rc::clone(&self.interaction),
            origin: point(-scroll_offset.x, -scroll_offset.y),
        }
        .into_any_element()
    }
}

#[derive(Default)]
struct ScrollbarInteraction {
    dragging: Cell<bool>,
    last_mouse_position: Cell<Pixels>,
    /// 拖拽期间的乐观位置。
    /// 滚动容器异步更新时，不能每次都回读可能滞后的 offset。
    drag_scroll_position: Cell<Pixels>,
}

/// 纯几何：在长度为 `track_length` 的轨道上，由内容滚动范围与当前位置推导 thumb。
///
/// 返回 `(thumb_length, thumb_position, scroll_per_pixel)`；
/// `thumb_position` 相对轨道起点（不含 padding），内容不高于视口时返回 None。
///
/// 公式：
/// - total = max_scroll + track_length（内容总高，轨道长度即视口长度）
/// - thumb_length = track_length × track_length/total，夹 [MIN_THUMB_SIZE, track_length]
/// - travel = track_length − thumb_length（thumb 行程），下限 1px 防除零
/// - scroll_per_pixel = max_scroll / travel
/// - thumb_position = clamp(scroll_top, 0, max_scroll) / scroll_per_pixel
pub fn thumb_geometry(
    track_length: Pixels,
    max_scroll: Pixels,
    scroll_top: Pixels,
) -> Option<(Pixels, Pixels, f32)> {
    if max_scroll <= Pixels::ZERO || track_length <= Pixels::ZERO {
        return None;
    }
    let total = max_scroll + track_length;
    let thumb_length = (track_length * (track_length / total))
        .max(MIN_THUMB_SIZE)
        .min(track_length);
    let travel = (track_length - thumb_length).max(px(1.));
    let scroll_per_pixel = max_scroll / travel;
    let scroll_position = scroll_top.clamp(Pixels::ZERO, max_scroll);
    let thumb_position = scroll_position / scroll_per_pixel;
    Some((thumb_length, thumb_position, scroll_per_pixel))
}

/// 滚动条的可渲染元素；通常通过 [`Scrollbar`] 构造。
pub struct ScrollbarElement<T: ScrollableHandle> {
    handle: T,
    axis: ScrollbarAxis,
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
        let max_scroll = self.axis.max_offset(self.handle.max_offset());
        let track_length = match self.axis {
            ScrollbarAxis::Vertical => {
                (viewport_bounds.size.height - PADDING * 2.).max(Pixels::ZERO)
            }
            ScrollbarAxis::Horizontal => {
                (viewport_bounds.size.width - PADDING * 2.).max(Pixels::ZERO)
            }
        };
        let (thumb_length, thumb_position, scroll_per_pixel) = thumb_geometry(
            track_length,
            max_scroll,
            -self.axis.max_offset(self.handle.offset()),
        )?;
        let thumb_position = PADDING + thumb_position;
        let thumb_bounds = match self.axis {
            ScrollbarAxis::Vertical => Bounds::new(
                point(
                    viewport_bounds.right() - WIDTH - PADDING,
                    viewport_bounds.top() + thumb_position,
                ),
                size(WIDTH, thumb_length),
            ),
            ScrollbarAxis::Horizontal => Bounds::new(
                point(
                    viewport_bounds.left() + thumb_position,
                    viewport_bounds.bottom() - WIDTH - PADDING,
                ),
                size(thumb_length, WIDTH),
            ),
        };
        // 可见滑块保持紧凑；交互区域扩展到右侧边缘，避免细滑块难以命中。
        let interaction_bounds = match self.axis {
            ScrollbarAxis::Vertical => Bounds::new(
                point(thumb_bounds.left() - PADDING, thumb_bounds.top()),
                size(
                    thumb_bounds.size.width + PADDING * 2.,
                    thumb_bounds.size.height,
                ),
            ),
            ScrollbarAxis::Horizontal => Bounds::new(
                point(thumb_bounds.left(), thumb_bounds.top() - PADDING),
                size(
                    thumb_bounds.size.width,
                    thumb_bounds.size.height + PADDING * 2.,
                ),
            ),
        };

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
            fill(layout.thumb_bounds, thumb_color).corner_radii(Corners::all(
                layout
                    .thumb_bounds
                    .size
                    .width
                    .min(layout.thumb_bounds.size.height)
                    / 2.,
            )),
        );

        if dragging {
            window.set_window_cursor_style(CursorStyle::Arrow);
        } else {
            window.set_cursor_style(CursorStyle::Arrow, &layout.hitbox);
        }

        let interaction = Rc::clone(&self.interaction);
        let handle = self.handle.clone();
        let move_layout = layout.clone();
        let axis = self.axis;
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            if interaction.dragging.get() && event.dragging() {
                let position = axis.coordinate(event.position);
                let delta = position - interaction.last_mouse_position.get();
                interaction.last_mouse_position.set(position);
                let max_scroll = axis.max_offset(handle.max_offset());
                let scroll_position = (interaction.drag_scroll_position.get()
                    + delta * move_layout.scroll_per_pixel)
                    .clamp(Pixels::ZERO, max_scroll);
                interaction.drag_scroll_position.set(scroll_position);
                let mut offset = handle.offset();
                axis.set_offset(&mut offset, -scroll_position);
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
            interaction
                .last_mouse_position
                .set(axis.coordinate(event.position));
            interaction.drag_scroll_position.set(
                (-axis.max_offset(handle.offset()))
                    .clamp(Pixels::ZERO, axis.max_offset(handle.max_offset())),
            );
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
#[path = "test/scrollbar_tests.rs"]
mod tests;
