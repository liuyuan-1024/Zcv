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
