//! 系统窗口控制的平台入口。

use gpui::{AnyElement, App, Window};

#[cfg(not(target_os = "macos"))]
use zcv_theme::scale;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
pub(in crate::top_bar) use macos::{WindowControls, leading_inset};

#[cfg(not(target_os = "macos"))]
#[derive(Default)]
pub(in crate::top_bar) struct WindowControls;

#[cfg(not(target_os = "macos"))]
impl WindowControls {
    pub(in crate::top_bar) fn align(&mut self, _: &Window, _: gpui::Pixels) {}
}

#[cfg(not(target_os = "macos"))]
pub(in crate::top_bar) fn leading_inset(window: &Window) -> gpui::Pixels {
    scale::to_pixels(scale::S6, window)
}

#[cfg(target_os = "windows")]
pub(in crate::top_bar) fn trailing(window: &Window, cx: &App) -> Option<AnyElement> {
    Some(windows::render(window, cx))
}

#[cfg(not(target_os = "windows"))]
pub(in crate::top_bar) fn trailing(_: &Window, _: &App) -> Option<AnyElement> {
    None
}
