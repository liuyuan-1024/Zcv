//! 锚定浮层呈现原语：定位、窗口收缩与点击外部关闭。
//!
//! 会话（互斥与焦点归还）由 PopoverHost 承担，内容外观由各内容组件自带；
//! 这里只负责把内容挂到窗口浮层层。

use gpui::{
    Anchor, App, Div, MouseButton, MouseDownEvent, Pixels, Point, Window, anchored, deferred, div,
    point, prelude::*,
};
use zcv_theme::scale;

/// 点击浮层外部时的关闭回调。
type OutsideHandler = Box<dyn Fn(&MouseDownEvent, &mut Window, &mut App)>;

/// 浮层锚定方式。
pub enum OverlayAnchor {
    /// 锚定在挂载点旁（触发器下方）。
    AtSlot,
    /// 出现在窗口坐标（如右键光标处）。
    AtPosition(Point<Pixels>),
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
        anchor: OverlayAnchor::AtSlot,
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

    /// 挂载内容：deferred + anchored + occlude，宽度始终收缩在窗口内。
    pub fn child(self, content: impl IntoElement) -> Div {
        let window = self.window;
        let margin = scale::to_pixels(scale::S6, window);
        let win_size = window.bounds().size;
        // 窗口过窄时建议宽度让位于窗口边界。
        let available_width = win_size.width - margin * 2.0;
        let suggested_width = self.min_width.map(|width| width.min(available_width));

        let mut anchored = anchored().snap_to_window_with_margin(margin);
        anchored = match self.anchor {
            OverlayAnchor::AtSlot => anchored
                .anchor(Anchor::TopLeft)
                .position(point(Pixels::ZERO, Pixels::ZERO))
                .position_mode(gpui::AnchoredPositionMode::Local),
            OverlayAnchor::AtPosition(position) => anchored.position(position),
        };

        let mut panel = div()
            .occlude()
            // 浮层内的点击不落到下层。
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .debug_selector(|| "overlay-panel".into())
            .max_w(available_width)
            .when_some(suggested_width, |panel, width| panel.min_w(width))
            .child(content);

        let mut overlay = div();
        match self.outside {
            Some(OverlayOutside::Passthrough(handler)) => {
                panel = panel.on_mouse_down_out(handler);
            }
            Some(OverlayOutside::Intercept(handler)) => {
                overlay = overlay.child(
                    deferred(
                        div()
                            .absolute()
                            .top(Pixels::ZERO)
                            .left(Pixels::ZERO)
                            .w(win_size.width)
                            .h(win_size.height)
                            .occlude()
                            .on_mouse_down(MouseButton::Left, handler),
                    )
                    .with_priority(0),
                );
            }
            None => {}
        }
        overlay.child(deferred(anchored.child(panel)).with_priority(1))
    }
}

#[cfg(test)]
#[path = "test/overlay_tests.rs"]
mod tests;
