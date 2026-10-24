//! 系统窗口控制的平台入口。

use gpui::{AnyElement, App, Div, Stateful, Window};

#[cfg(target_os = "macos")]
use gpui::prelude::*;

#[cfg(not(target_os = "macos"))]
use zcv_theme::scale;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
pub(in crate::top_bar) use macos::{WindowControls, leading_inset};

pub(in crate::top_bar) fn titlebar_interaction(bar: Stateful<Div>) -> Stateful<Div> {
    #[cfg(target_os = "macos")]
    {
        bar.on_click(|event, window, _| {
            if event.click_count() == 2 {
                window.titlebar_double_click();
            }
        })
    }

    #[cfg(not(target_os = "macos"))]
    {
        bar
    }
}

#[cfg(not(target_os = "macos"))]
pub(in crate::top_bar) struct WindowControls;

#[cfg(not(target_os = "macos"))]
impl WindowControls {
    pub(in crate::top_bar) fn new() -> Self {
        Self
    }

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
