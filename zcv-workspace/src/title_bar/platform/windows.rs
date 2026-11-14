use gpui::{AnyElement, App, Window, WindowControlArea, div, prelude::*, px};
use zcv_theme::color;

pub(super) fn render(window: &Window, cx: &App) -> AnyElement {
    let maximized = window.is_maximized();
    let controls = window.window_controls();
    div()
        .id("windows-window-controls")
        .flex()
        .flex_row()
        .h_full()
        .font_family("Segoe MDL2 Assets")
        .child(button(Control::Minimize, controls.minimize, cx))
        .child(button(
            if maximized {
                Control::Restore
            } else {
                Control::Maximize
            },
            controls.maximize,
            cx,
        ))
        .child(button(Control::Close, true, cx))
        .into_any_element()
}

#[derive(Clone, Copy)]
enum Control {
    Minimize,
    Restore,
    Maximize,
    Close,
}

fn button(control: Control, enabled: bool, cx: &App) -> impl gpui::IntoElement {
    let colors = color::current(cx);
    let (id, symbol, area) = match control {
        Control::Minimize => (
            "window-controls.minimize",
            "\u{e921}",
            WindowControlArea::Min,
        ),
        Control::Restore => (
            "window-controls.restore",
            "\u{e923}",
            WindowControlArea::Max,
        ),
        Control::Maximize => (
            "window-controls.maximize",
            "\u{e922}",
            WindowControlArea::Max,
        ),
        Control::Close => (
            "window-controls.close",
            "\u{e8bb}",
            WindowControlArea::Close,
        ),
    };
    div()
        .id(id)
        .w(px(36.0))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(10.0))
        .text_color(if enabled {
            colors.text
        } else {
            colors.text_disabled
        })
        .occlude()
        .window_control_area(area)
        .when(enabled, |button| {
            if matches!(control, Control::Close) {
                button.hover(|style| style.bg(gpui::rgb(0xe81123)).text_color(gpui::white()))
            } else {
                button.hover(|style| style.bg(colors.ghost_element_hover))
            }
        })
        .child(symbol)
}
