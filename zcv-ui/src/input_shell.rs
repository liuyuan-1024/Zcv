//! 通用输入框外壳。

use gpui::{AnyElement, App, IntoElement, ParentElement, RenderOnce, Window, div, prelude::*};
use zcv_theme::{color, space, typography};

/// 一行 = 一个带边框的输入容器 + 若干个外部插槽(输入容器右侧按钮区);
/// 输入容器内提供文本输入位 + 若干个内部插槽(边框内右缘)。
/// 组件本身不含搜索/替换语义:输入、内部插槽与外部插槽的内容全部由插入方给定。
pub(crate) struct InputShell {
    input: AnyElement,
    internal: Vec<AnyElement>,
    external: Vec<AnyElement>,
}

impl InputShell {
    pub(crate) fn new(input: impl Into<AnyElement>) -> Self {
        Self {
            input: input.into(),
            internal: Vec::new(),
            external: Vec::new(),
        }
    }

    /// 内部插槽:控件渲染进输入框边框内、文本的右缘。
    pub(crate) fn internal(mut self, element: impl IntoElement) -> Self {
        self.internal.push(element.into_any_element());
        self
    }

    /// 外部插槽:控件渲染在输入框右侧`。
    pub(crate) fn external(mut self, element: impl IntoElement) -> Self {
        self.external.push(element.into_any_element());
        self
    }
}

impl IntoElement for InputShell {
    type Element = gpui::ViewElement<Self>;

    fn into_element(self) -> Self::Element {
        gpui::ViewElement::new(self)
    }
}

impl RenderOnce for InputShell {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = color::current(cx);
        div()
            .w_full()
            .flex()
            .items_center()
            .gap(space::S6)
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    // 外壳高度由输入框自身行数决定(auto_height 可随多行文本增高),与插槽内容无关。
                    .p(space::S6)
                    .gap(space::S6)
                    .rounded_sm()
                    .border_1()
                    .border_color(colors.border)
                    .child(self.input)
                    // 内部插槽控件放在恒等于单行墨迹高度的带内,垂直居中;
                    // 控件比文本行高出的部分在带内裁剪,既不撑高外壳,也不会随文本多行而错位。
                    .when(!self.internal.is_empty(), |box_| {
                        box_.child(
                            div()
                                .flex_none()
                                .flex()
                                .items_center()
                                .h(typography::ui_line())
                                .gap(space::S6)
                                .children(self.internal),
                        )
                    }),
            )
            .children(self.external)
    }
}
