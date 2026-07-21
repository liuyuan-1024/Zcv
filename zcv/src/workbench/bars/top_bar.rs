//! TopBar —— 窗口级顶部外壳。

use gpui::{AnyElement, Context, Corner, Div, Window, actions, div, prelude::*};

use super::frame::{BarEdge, bar_frame};
use super::window_controls;
use crate::features::{branch_picker, project_picker};
use crate::shared::Glyph;
use crate::surface::{
    AnchorRegistry, SurfaceAnchor, SurfaceId, SurfaceManager, anchor_from_bounds, track_anchor,
};

actions!(
    top_bar,
    [
        OpenSettings,
        OpenProjectPicker,
        ToggleBranchPicker,
        GitFetch,
        GitPull,
        GitPush,
    ]
);

pub(crate) fn handle_open_settings(_: &OpenSettings, _: &mut Window, _: &mut gpui::App) {
    println!("设置");
}
pub(crate) fn handle_open_project_picker(
    _: &OpenProjectPicker,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let anchor = cx
        .try_global::<AnchorRegistry>()
        .and_then(|reg| reg.resolve(window, &"top-bar.project-picker".into()))
        .map(|bounds| anchor_from_bounds(bounds, Corner::TopLeft))
        .unwrap_or(SurfaceAnchor::Center);
    project_picker::open(anchor, window, cx);
}
pub(crate) fn handle_toggle_branch_picker(
    _: &ToggleBranchPicker,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let anchor = cx
        .try_global::<AnchorRegistry>()
        .and_then(|reg| reg.resolve(window, &"top-bar.branch".into()))
        .map(|bounds| anchor_from_bounds(bounds, Corner::TopLeft))
        .unwrap_or(SurfaceAnchor::Center);
    branch_picker::open(anchor, window, cx);
}
pub(crate) fn handle_git_fetch(_: &GitFetch, _: &mut Window, _: &mut gpui::App) {
    println!("fetch");
}
pub(crate) fn handle_git_pull(_: &GitPull, _: &mut Window, _: &mut gpui::App) {
    println!("pull");
}
pub(crate) fn handle_git_push(_: &GitPush, _: &mut Window, _: &mut gpui::App) {
    println!("push");
}

pub(crate) struct TopBar;

impl TopBar {
    pub(crate) fn new(_cx: &mut gpui::Context<Self>) -> Self {
        Self
    }
}

impl gpui::Render for TopBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let is_active = |id: SurfaceId| -> bool {
            cx.try_global::<SurfaceManager>()
                .map(|m| m.is_active(id))
                .unwrap_or(false)
        };

        bar_frame(BarEdge::Top)
            .id("top-bar")
            .child(cluster(leading_slots(window, &is_active)))
            .child(drag_spacer())
            .child(cluster(trailing_slots()))
    }
}

fn cluster(items: Vec<AnyElement>) -> Div {
    div().flex().items_center().gap_2().children(items)
}

fn drag_spacer() -> Div {
    div().flex_1().h_full()
}

fn leading_slots(window: &Window, is_active: &dyn Fn(SurfaceId) -> bool) -> Vec<AnyElement> {
    let mut out: Vec<AnyElement> = Vec::new();

    // 窗口控制
    out.push(window_controls::render(window).into_any_element());

    // 项目选择器 + 分支 + Git 操作
    out.push(
        track_anchor(
            "top-bar.project-picker",
            Glyph::text("top-bar.project-picker", "打开项目", "项目选择器")
                .active(is_active(SurfaceId::ProjectPicker))
                .on_click(|window, cx| {
                    window.dispatch_action(Box::new(OpenProjectPicker), cx);
                })
                .into_any_element(),
        )
        .into_any_element(),
    );
    out.push(
        track_anchor(
            "top-bar.branch",
            Glyph::icon_text(
                "top-bar.branch",
                "icons/panels/version_control.svg",
                "main",
                "分支",
            )
            .active(is_active(SurfaceId::BranchPicker))
            .on_click(|window, cx| {
                window.dispatch_action(Box::new(ToggleBranchPicker), cx);
            })
            .into_any_element(),
        )
        .into_any_element(),
    );
    out.push(
        Glyph::icon(
            "top-bar.git-fetch",
            "icons/actions/arrow_circle.svg",
            "fetch",
        )
        .on_click(|_, _| println!("点击 fetch"))
        .into_any_element(),
    );
    out.push(
        Glyph::icon_text(
            "top-bar.git-pull",
            "icons/actions/arrow_down.svg",
            "0",
            "pull",
        )
        .on_click(|_, _| println!("点击 pull"))
        .into_any_element(),
    );
    out.push(
        Glyph::icon_text(
            "top-bar.git-push",
            "icons/actions/arrow_up.svg",
            "0",
            "push",
        )
        .on_click(|_, _| println!("点击 push"))
        .into_any_element(),
    );

    out
}

fn trailing_slots() -> Vec<AnyElement> {
    vec![
        Glyph::icon("top-bar.settings", "icons/actions/settings.svg", "设置")
            .on_click(|_, _| println!("点击设置"))
            .into_any_element(),
    ]
}
