use gpui::{DefiniteLength, Pixels, Point, Window, point, px};
use zcv_theme::scale;

const TRAFFIC_LIGHT_HEIGHT: Pixels = px(14.0);
const TRAFFIC_LIGHT_MARGIN: DefiniteLength = scale::structural(9.0);
const TRAFFIC_LIGHT_GROUP_WIDTH: Pixels = px(60.0);

#[derive(Default)]
pub(in crate::top_bar) struct WindowControls {
    traffic_light_position: Option<Point<Pixels>>,
}

impl WindowControls {
    pub(in crate::top_bar) fn align(&mut self, window: &Window, bar_height: Pixels) {
        let x = traffic_light_x(window);
        let y = (bar_height - TRAFFIC_LIGHT_HEIGHT) / 2.0;
        let position = point(x, y);
        if self.traffic_light_position != Some(position) {
            window.set_traffic_light_position(position);
            self.traffic_light_position = Some(position);
        }
    }
}

fn traffic_light_x(window: &Window) -> Pixels {
    scale::to_pixels(TRAFFIC_LIGHT_MARGIN, window)
}

pub(in crate::top_bar) fn leading_inset(window: &Window) -> Pixels {
    if !window.is_fullscreen() && !window.is_simple_fullscreen() {
        // 原生按钮保持固定尺寸，两侧留白随 UI 字号缩放。
        traffic_light_x(window)
            + TRAFFIC_LIGHT_GROUP_WIDTH
            + scale::to_pixels(TRAFFIC_LIGHT_MARGIN, window)
    } else {
        scale::to_pixels(scale::S6, window)
    }
}
