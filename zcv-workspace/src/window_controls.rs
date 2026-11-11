//! 窗口标题栏中的平台窗口控制区域与统一高度。

mod platform;

use gpui::{App, Pixels, Window};
use zcv_theme::{scale, typography};

pub use platform::{WindowControls, leading_inset, top_bar_interaction, trailing};

pub fn top_bar_height(window: &Window, cx: &App) -> Pixels {
    typography::ui_line_at(window.rem_size(), cx)
        + scale::to_pixels(scale::S12, window)
        + scale::to_pixels(scale::S8, window)
}
