//! 锚定浮层呈现原语：定位、窗口收缩与点击外部关闭。
//!
//! 会话（互斥与焦点归还）由 PopoverHost 承担，内容外观由各内容组件自带；
//! 这里只负责把内容挂到窗口浮层层。

use gpui::{
    Anchor, App, AvailableSpace, Bounds, Div, Element, ElementId, GlobalElementId,
    InspectorElementId, LayoutId, MouseButton, MouseDownEvent, Pixels, Point, Style, Window,
    anchored, deferred, div, point, prelude::*, relative, size,
};
use zcv_theme::scale;

/// 点击浮层外部时的关闭回调。
type OutsideHandler = Box<dyn Fn(&MouseDownEvent, &mut Window, &mut App)>;

/// 浮层锚定方式。
pub enum OverlayAnchor {
    /// 相对挂载点定位（如触发器旁或顶栏输入框下缘）。
    AtLocalPosition(Point<Pixels>),
    /// 出现在窗口坐标（如右键光标处）。
    AtPosition(Point<Pixels>),
    /// 挂在相对定位的触发器容器内。下方空间不足时在上方展开。
    AtTrigger,
}

/// 点击浮层外部时的关闭方式。
pub enum OverlayOutside {
    /// 事件穿透到下层：点击外部既关闭浮层，也让下层响应（如右键菜单点击另一行）。
    Passthrough(OutsideHandler),
    /// 全屏拦截层：点击浮层外任何位置关闭，不激活下层。
    Intercept(OutsideHandler),
}

/// 锚定浮层挂载器；由 overlay 构造。
pub struct Overlay<'a> {
    window: &'a Window,
    anchor: OverlayAnchor,
    min_width: Option<Pixels>,
    outside: Option<OverlayOutside>,
}

/// 构造锚定浮层挂载器。
pub fn overlay(window: &Window) -> Overlay<'_> {
    Overlay {
        window,
        anchor: OverlayAnchor::AtLocalPosition(point(Pixels::ZERO, Pixels::ZERO)),
        min_width: None,
        outside: None,
    }
}

impl Overlay<'_> {
    pub fn anchor(mut self, anchor: OverlayAnchor) -> Self {
        self.anchor = anchor;
        self
    }

    /// 建议宽度：内容不足时保持体面宽度，窗口不够宽时收缩到窗口内。
    pub fn min_width(mut self, width: Pixels) -> Self {
        self.min_width = Some(width);
        self
    }

    pub fn outside(mut self, outside: OverlayOutside) -> Self {
        self.outside = Some(outside);
        self
    }

    /// 延迟绘制并拦截面板内的点击，宽度收缩在窗口内。
    pub fn child(self, content: impl IntoElement) -> Div {
        let window = self.window;
        let margin = scale::to_pixels(scale::S6, window);
        let win_size = window.bounds().size;
        // 窗口过窄时建议宽度让位于窗口边界。
        let available_width = win_size.width - margin * 2.0;
        let suggested_width = self.min_width.map(|width| width.min(available_width));

        let mut panel = div()
            .occlude()
            // 浮层内的点击不落到下层。
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .debug_selector(|| "overlay-panel".into())
            .max_w(available_width)
            .when_some(suggested_width, |panel, width| panel.min_w(width))
            .child(content);

        let mut overlay = div().when(matches!(self.anchor, OverlayAnchor::AtTrigger), |overlay| {
            overlay.absolute().inset_0()
        });
        match self.outside {
            Some(OverlayOutside::Passthrough(handler)) => {
                panel = panel.on_mouse_down_out(handler);
            }
            Some(OverlayOutside::Intercept(handler)) => {
                overlay = overlay.child(
                    deferred(
                        anchored()
                            .position(point(Pixels::ZERO, Pixels::ZERO))
                            .child(
                                div()
                                    .w(win_size.width)
                                    .h(win_size.height)
                                    .occlude()
                                    .on_mouse_down(MouseButton::Left, handler),
                            ),
                    )
                    .with_priority(0),
                );
            }
            None => {}
        }
        let anchored_panel = anchored().snap_to_window_with_margin(margin);
        let anchored_panel = match self.anchor {
            OverlayAnchor::AtTrigger => {
                return overlay.child(TriggerOverlay {
                    panel: Some(panel),
                    margin,
                    suggested_width,
                });
            }
            OverlayAnchor::AtLocalPosition(position) => anchored_panel
                .anchor(Anchor::TopLeft)
                .position(position)
                .position_mode(gpui::AnchoredPositionMode::Local),
            OverlayAnchor::AtPosition(position) => anchored_panel.position(position),
        };
        overlay.child(deferred(anchored_panel.child(panel)).with_priority(1))
    }
}

/// 在预绘制时使用触发器的实际位置，包含滚动容器施加的偏移。
struct TriggerOverlay {
    panel: Option<Div>,
    margin: Pixels,
    suggested_width: Option<Pixels>,
}

impl IntoElement for TriggerOverlay {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for TriggerOverlay {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let style = Style {
            size: size(relative(1.0).into(), relative(1.0).into()),
            ..Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        trigger: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let viewport = window.viewport_size();
        let gap = scale::to_pixels(scale::S4, window);
        let below = trigger.bottom() + gap;
        let above = trigger.top() - gap;
        let room_below = viewport.height - self.margin - below;
        let room_above = above - self.margin;
        let max_height = room_below.max(room_above).max(Pixels::ZERO);
        let available_width = (viewport.width - self.margin * 2.0).max(Pixels::ZERO);
        let min_width = trigger
            .size
            .width
            .max(self.suggested_width.unwrap_or(Pixels::ZERO))
            .min(available_width);
        let mut panel = self
            .panel
            .take()
            .expect("浮层只能在一帧内挂载一次")
            .min_w(min_width)
            .max_h(max_height)
            .flex()
            .flex_col()
            .overflow_hidden()
            .into_any_element();
        let panel_size = panel.layout_as_root(
            size(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
            window,
            cx,
        );
        let right_limit = viewport.width - panel_size.width - self.margin;
        let x = trigger.left().min(right_limit).max(self.margin);
        let y = if panel_size.height <= room_below || room_below >= room_above {
            below
        } else {
            above - panel_size.height
        };

        window.defer_draw(panel, point(x, y), 1, None);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }
}

#[cfg(test)]
#[path = "test/overlay_tests.rs"]
mod tests;
