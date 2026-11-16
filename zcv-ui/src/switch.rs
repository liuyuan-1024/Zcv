use gpui::{App, ElementId, Window, div, prelude::*};
use zcv_theme::{color, scale};

use crate::{ButtonLike, button_like::keyboard_focus};

/// 用于即时生效的布尔设置，通过滑块位置和主题色显示开关状态。
///
/// 支持鼠标点击，以及聚焦后的空格和 Enter 操作。
/// 当前状态由调用方持有，状态变更或保存成功后由调用方传入新的值重新渲染。
#[derive(IntoElement)]
pub struct Switch {
    base: ButtonLike,
    checked: bool,
}

impl Switch {
    /// 创建开关。为每个开关提供在所在作用域内唯一的 `id`。
    ///
    /// 用户激活开关时，`on_change` 收到 `checked` 的反值。
    pub fn new(
        id: impl Into<ElementId>,
        checked: bool,
        on_change: impl Fn(bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            base: ButtonLike::new(id)
                .padding(scale::S2)
                .occlude(true)
                .on_click(move |_, window, cx| on_change(!checked, window, cx)),
            checked,
        }
    }
}

impl RenderOnce for Switch {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = *color::current(cx);
        let (track, border, thumb) = if self.checked {
            (
                colors.icon_accent,
                colors.icon_accent,
                colors.editor_background,
            )
        } else {
            (colors.panel_background, colors.border, colors.icon_muted)
        };

        keyboard_focus(
            self.base.render_root(cx).rounded_full(),
            colors.border_focused,
        )
        .child(
            div()
                .debug_selector(|| "switch-track".into())
                .flex()
                .items_center()
                .when(self.checked, |track| track.justify_end())
                .w(scale::S32)
                .h(scale::structural(18.0))
                .px(scale::S2)
                .rounded_full()
                .border_1()
                .border_color(border)
                .bg(track)
                .child(div().size(scale::S12).rounded_full().bg(thumb)),
        )
    }
}

#[cfg(test)]
#[path = "test/switch_tests.rs"]
mod tests;
