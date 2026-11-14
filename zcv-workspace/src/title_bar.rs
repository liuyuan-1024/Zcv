//! 窗口标题栏框架和平台窗口控制。

mod platform;

use gpui::{App, Div, ElementId, Pixels, Stateful, Window, WindowControlArea, div, prelude::*};
use zcv_theme::{color, scale, typography};

use platform::WindowControls;

/// 每个窗口持有自己的标题栏，内容由该窗口提供。
#[derive(Default)]
pub struct TitleBar {
    controls: WindowControls,
}

impl TitleBar {
    pub fn render(
        &mut self,
        id: impl Into<ElementId>,
        content: Div,
        window: &Window,
        cx: &App,
    ) -> Stateful<Div> {
        let height = height(window, cx);
        self.controls.align(window, height);
        let colors = color::current(cx);

        platform::top_bar_interaction(
            div()
                .id(id)
                .window_control_area(WindowControlArea::Drag)
                .relative()
                .flex()
                .items_center()
                .w_full()
                .h(height)
                .bg(colors.title_bar_background)
                .text_color(colors.text),
        )
        .child(
            content
                .flex()
                .items_center()
                .flex_1()
                .min_w_0()
                .py(scale::S6)
                .pr(scale::S6)
                .pl(platform::leading_inset(window))
                .gap(scale::S6),
        )
        .when_some(platform::trailing(window, cx), |bar, controls| {
            bar.child(controls)
        })
        .child(
            div()
                .absolute()
                .inset_0()
                .border_b_1()
                .border_color(colors.border),
        )
    }
}

fn height(window: &Window, cx: &App) -> Pixels {
    typography::ui_line_at(window.rem_size(), cx)
        + scale::to_pixels(scale::S12, window)
        + scale::to_pixels(scale::S8, window)
}

#[cfg(test)]
#[path = "test/title_bar_tests.rs"]
mod tests;
