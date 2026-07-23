//! TopBar —— 窗口级顶部外壳。

use gpui::{AnyElement, Context, Div, Entity, Window, actions, div, prelude::*};

use super::frame::{BarEdge, bar_frame};
use super::project_picker::{OnProjectSelected, ProjectPicker};
use super::window_controls;
use crate::ui::glyph::Glyph;

actions!(top_bar, [OpenSettings, GitFetch, GitPull, GitPush,]);

pub(crate) struct TopBar {
    pub(crate) project_picker: Entity<ProjectPicker>,
}

impl TopBar {
    pub(crate) fn new(on_selected: OnProjectSelected, cx: &mut gpui::Context<Self>) -> Self {
        let project_picker = cx.new(|cx| ProjectPicker::new(on_selected, cx));
        Self { project_picker }
    }
}

impl gpui::Render for TopBar {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl gpui::IntoElement {
        bar_frame(BarEdge::Top)
            .id("top-bar")
            .child(cluster(leading_slots(window, &self.project_picker)))
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

fn leading_slots(window: &Window, project_picker: &Entity<ProjectPicker>) -> Vec<AnyElement> {
    let mut out: Vec<AnyElement> = Vec::new();

    // 窗口控制
    out.push(window_controls::render(window).into_any_element());

    // 项目选择器
    out.push(project_picker.clone().into_any_element());
    // Git 分支
    out.push(
        Glyph::icon_text("top-bar.branch", "icons/panels/version_control.svg", "main")
            .label("分支")
            .on_click(|_, _| println!("点击分支"))
            .into_any_element(),
    );
    // Git fetch
    out.push(
        Glyph::icon("top-bar.git-fetch", "icons/actions/arrow_circle.svg")
            .label("fetch")
            .on_click(|_, _| println!("点击 fetch"))
            .into_any_element(),
    );
    // Git pull
    out.push(
        Glyph::icon_text("top-bar.git-pull", "icons/actions/arrow_down.svg", "0")
            .label("pull")
            .on_click(|_, _| println!("点击 pull"))
            .into_any_element(),
    );
    // Git push
    out.push(
        Glyph::icon_text("top-bar.git-push", "icons/actions/arrow_up.svg", "0")
            .label("push")
            .on_click(|_, _| println!("点击 push"))
            .into_any_element(),
    );

    out
}

fn trailing_slots() -> Vec<AnyElement> {
    vec![
        Glyph::icon("top-bar.settings", "icons/actions/settings.svg")
            .label("设置")
            .on_click(|_, _| println!("点击设置"))
            .into_any_element(),
    ]
}
