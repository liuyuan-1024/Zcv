//! `ConfirmOverlay` —— 确认浮层：半透明遮罩 + 居中卡片 + 三键选择（确认/跳过/取消）。
//!
//! 数据型交互组件（回调直连，不走 action 分发）：宿主通过 `on_answer` 接收用户选择，键盘路径（如 escape 取消）由宿主的按键绑定承担，浮层自身不做焦点管理与键盘导航。

use std::rc::Rc;

use gpui::{
    App, Component, ElementId, Hsla, IntoElement, Pixels, RenderOnce, SharedString, Window, div,
    prelude::*, px,
};
use zcv_theme::{color, space};

use crate::{Button, ButtonStyle};

/// 浮层三键选择的答案。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmAnswer {
    /// 确认：执行当前项（如覆盖目标）。
    Confirm,
    /// 跳过：不执行当前项，继续下一项。
    Skip,
    /// 取消：整体中止，不执行任何剩余项。
    Cancel,
}

type AnswerHandler = Rc<dyn Fn(ConfirmAnswer, &mut Window, &mut App)>;

/// 卡片宽度上限：防止长确认文案把浮层拉得过宽。
const CARD_MAX_WIDTH: Pixels = px(360.);

/// 确认浮层（Builder 模式）。
///
/// 按钮点击为数据型交互：直接回调 `on_answer`，不依赖焦点链分发；
/// 默认文案为 确认/跳过/取消，可按场景覆盖（如 覆盖/跳过/取消）。
pub struct ConfirmOverlay {
    id: ElementId,
    message: SharedString,
    detail: Option<SharedString>,
    confirm_label: SharedString,
    skip_label: SharedString,
    cancel_label: SharedString,
    on_answer: AnswerHandler,
}

impl ConfirmOverlay {
    /// 以主文案构造浮层；副文案与按钮文案随后续 builder 链覆盖。
    pub fn new(id: impl Into<ElementId>, message: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            message: message.into(),
            detail: None,
            confirm_label: "确认".into(),
            skip_label: "跳过".into(),
            cancel_label: "取消".into(),
            // 默认空回调：未接线时点击静默，保证渲染与交互安全。
            on_answer: Rc::new(|_, _, _| {}),
        }
    }

    /// 设置副文案（如「第 2/5 项」）。
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// 设置确认按钮文案（如「覆盖」）。
    pub fn confirm_label(mut self, label: impl Into<SharedString>) -> Self {
        self.confirm_label = label.into();
        self
    }

    /// 设置跳过按钮文案。
    pub fn skip_label(mut self, label: impl Into<SharedString>) -> Self {
        self.skip_label = label.into();
        self
    }

    /// 设置取消按钮文案。
    pub fn cancel_label(mut self, label: impl Into<SharedString>) -> Self {
        self.cancel_label = label.into();
        self
    }

    /// 设置答案回调（按钮点击直连，不走 action 分发）。
    pub fn on_answer(mut self, handler: AnswerHandler) -> Self {
        self.on_answer = handler;
        self
    }
}

impl IntoElement for ConfirmOverlay {
    type Element = Component<Self>;

    fn into_element(self) -> Self::Element {
        Component::new(self)
    }
}

impl RenderOnce for ConfirmOverlay {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = *color::current(cx);
        let on_answer = self.on_answer;

        // 三个按钮共用同一回调：各自携带答案分发（id 派生自 overlay id，保证多浮层不冲突）。
        let confirm = {
            let on_answer = Rc::clone(&on_answer);
            Button::text(
                ElementId::from((self.id.clone(), "confirm")),
                self.confirm_label.to_string(),
            )
            .style(ButtonStyle::Solid)
            .on_click(move |_, window, cx| on_answer(ConfirmAnswer::Confirm, window, cx))
        };
        let skip = {
            let on_answer = Rc::clone(&on_answer);
            Button::text(
                ElementId::from((self.id.clone(), "skip")),
                self.skip_label.to_string(),
            )
            .on_click(move |_, window, cx| on_answer(ConfirmAnswer::Skip, window, cx))
        };
        let cancel = {
            let on_answer = Rc::clone(&on_answer);
            Button::text(
                ElementId::from((self.id.clone(), "cancel")),
                self.cancel_label.to_string(),
            )
            .on_click(move |_, window, cx| on_answer(ConfirmAnswer::Cancel, window, cx))
        };

        let card = div()
            .flex()
            .flex_col()
            .gap(space::S6)
            .max_w(CARD_MAX_WIDTH)
            .p(space::S12)
            .rounded_md()
            .border_1()
            .border_color(colors.border)
            .bg(colors.panel_background)
            .child(
                div()
                    .text_color(colors.text)
                    .child(self.message.to_string()),
            )
            .when_some(self.detail, |element, detail| {
                element.child(
                    div()
                        .text_color(colors.text_muted)
                        .child(detail.to_string()),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .gap(space::S6)
                    .child(confirm)
                    .child(skip)
                    .child(cancel),
            );

        // 遮罩：绝对定位铺满宿主容器，边框色的半透明变体作语义中性的压暗层。
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(Hsla::from(colors.border).opacity(0.5))
            .child(card)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Context, Render, TestAppContext};

    use super::*;

    #[test]
    fn default_labels_are_confirm_skip_cancel() {
        let overlay = ConfirmOverlay::new("confirm", "目标已存在");
        assert_eq!(overlay.message.as_ref(), "目标已存在");
        assert_eq!(overlay.confirm_label.as_ref(), "确认");
        assert_eq!(overlay.skip_label.as_ref(), "跳过");
        assert_eq!(overlay.cancel_label.as_ref(), "取消");
        assert!(overlay.detail.is_none(), "默认无副文案");
    }

    #[test]
    fn builder_methods_override_labels_and_detail() {
        let overlay = ConfirmOverlay::new("confirm", "目标已存在")
            .detail("第 2/5 项")
            .confirm_label("覆盖")
            .skip_label("不覆盖")
            .cancel_label("全部取消");
        assert_eq!(
            overlay.detail.as_ref().map(|detail| detail.as_ref()),
            Some("第 2/5 项")
        );
        assert_eq!(overlay.confirm_label.as_ref(), "覆盖");
        assert_eq!(overlay.skip_label.as_ref(), "不覆盖");
        assert_eq!(overlay.cancel_label.as_ref(), "全部取消");
    }

    /// 渲染冒烟：挂载含浮层的宿主视图，遮罩 + 卡片 + 三按钮结构可正常构建。
    struct OverlayHost;

    impl Render for OverlayHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                ConfirmOverlay::new("tree-conflict", "目标已存在：a.txt")
                    .detail("第 1/3 项")
                    .on_answer(Rc::new(|_, _, _| {})),
            )
        }
    }

    #[gpui::test]
    fn renders_overlay_with_mask_and_buttons(cx: &mut TestAppContext) {
        let (_view, _cx) = cx.add_window_view(|_, _| OverlayHost);
    }
}
