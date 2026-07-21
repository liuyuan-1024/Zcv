//! WorkbenchFrame —— 窗口顶层装配。

mod bars;
mod layout;
pub(crate) mod project_tree;

use gpui::{Div, Entity, div, prelude::*};

use crate::theme::{color, typography};

pub(crate) use bars::{
    bottom_bar, bottom_bar::BottomBar, top_bar, top_bar::TopBar, window_controls,
};
pub(crate) use layout::{
    CloseTab, LayoutController, LayoutRef, LayoutSnapshot, NextTab, Pane, PaneId, PanelId, PrevTab,
    ViewId, handle_close_tab, render_body as render_layout_body,
};
pub(crate) use project_tree::ProjectTree;

pub(crate) fn render(
    top_bar: &Entity<TopBar>,
    bottom_bar: &Entity<BottomBar>,
    layout: &LayoutSnapshot,
    project_tree: &Entity<ProjectTree>,
) -> Div {
    let tree = project_tree.clone();
    let panel_content = move |panel: PanelId| -> Option<Div> {
        match panel {
            PanelId::ProjectTree => Some(div().size_full().child(tree.clone())),
            _ => None,
        }
    };

    div()
        .relative()
        .flex()
        .flex_col()
        .size_full()
        .overflow_hidden()
        .bg(color::current().gray.s[1])
        .font(typography::ui_font())
        .text_size(typography::ui())
        .line_height(typography::ui())
        .text_color(color::current().gray.s[8])
        .child(top_bar.clone())
        .child(render_layout_body(layout, &panel_content))
        .child(bottom_bar.clone())
}
