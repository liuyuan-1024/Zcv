//! 布局渲染 —— 将 LayoutSnapshot 渲染为 GPUI Div 树。

use gpui::{MouseButton, Pixels, div, prelude::*, px};

use crate::theme::{color, space};

use super::controller::{LayoutRef, LayoutSnapshot, PaneGroup};
use super::types::{Axis, DockArea, DockState, PanelId};

/// 面板内容提供者：布局不感知具体 panel 类型，通过此回调获取内容。
pub(crate) type PanelContentFn<'a> = dyn Fn(PanelId) -> Option<gpui::Div> + 'a;

/// 渲染 workbench 主体（不包含顶栏和底栏）。
pub(crate) fn render_body(layout: &LayoutSnapshot, panel_content: &PanelContentFn) -> gpui::Div {
    let mut row = div()
        .flex_1()
        .flex()
        .flex_row()
        .size_full()
        .overflow_hidden()
        .relative();

    // 左 Dock
    if layout.left_dock.is_visible() {
        row = row.child(render_dock(
            DockArea::Left,
            &layout.left_dock,
            panel_content,
        ));
    }

    // 中心列：PaneGroup + 底 Dock
    let mut center_col = div()
        .flex_1()
        .flex()
        .flex_col()
        .size_full()
        .overflow_hidden()
        .relative()
        .min_w(space::S16);

    center_col = center_col.child(render_pane_group(&layout.center));

    if layout.bottom_dock.is_visible() {
        center_col = center_col.child(render_dock(
            DockArea::Bottom,
            &layout.bottom_dock,
            panel_content,
        ));
    }

    row = row.child(center_col);

    // 右 Dock
    if layout.right_dock.is_visible() {
        row = row.child(render_dock(
            DockArea::Right,
            &layout.right_dock,
            panel_content,
        ));
    }

    row
}

// ── Dock 渲染 ────────────────────────────────────────────────────────

fn render_dock(area: DockArea, state: &DockState, panel_content: &PanelContentFn) -> gpui::Div {
    let frame = div()
        .relative()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .overflow_hidden()
        .bg(color::current().gray.s[1])
        .text_color(color::current().gray.s[8]);

    let frame = match area {
        DockArea::Left => frame
            .w(state.size)
            .h_full()
            .border_r_1()
            .border_color(color::current().gray.s[4]),
        DockArea::Right => frame
            .w(state.size)
            .h_full()
            .border_l_1()
            .border_color(color::current().gray.s[4]),
        DockArea::Bottom => frame
            .h(state.size)
            .w_full()
            .border_t_1()
            .border_color(color::current().gray.s[4]),
    };

    let body: gpui::Div = match state.active_panel.and_then(panel_content) {
        Some(content) => div().size_full().child(content),
        None => {
            let label = state.active_panel.map(|p| p.label()).unwrap_or("");
            render_placeholder(label)
        }
    };
    let frame = frame.child(body);

    const HIT: Pixels = space::S6;
    match area {
        DockArea::Left => frame.child(dock_drag_zone(DockArea::Left).right(px(0.0)).w(HIT)),
        DockArea::Right => frame.child(dock_drag_zone(DockArea::Right).left(px(0.0)).w(HIT)),
        DockArea::Bottom => frame.child(dock_drag_zone(DockArea::Bottom).top(px(0.0)).h(HIT)),
    }
}

fn render_placeholder(label: &str) -> gpui::Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(color::current().gray.s[5])
        .child(label.to_string())
}

// ── 拖拽热区和分隔线渲染 ────────────────────────────────────────────

fn dock_drag_zone(area: DockArea) -> gpui::Div {
    let base = div()
        .absolute()
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            if let Some(layout_ref) = cx.try_global::<LayoutRef>()
                && let Some(ctrl) = layout_ref.0.upgrade()
            {
                ctrl.borrow_mut().start_dock_drag(area, event.position);
                window.refresh();
            }
        })
        .on_mouse_up(MouseButton::Left, move |event, window, cx| {
            if event.click_count >= 2
                && let Some(layout_ref) = cx.try_global::<LayoutRef>()
                && let Some(ctrl) = layout_ref.0.upgrade()
            {
                ctrl.borrow_mut()
                    .reset_dock_size(area, window.bounds().size);
                window.refresh();
            }
        });

    match area {
        DockArea::Left | DockArea::Right => base.h_full().cursor_col_resize(),
        DockArea::Bottom => base.w_full().cursor_row_resize(),
    }
}

// ── 中心编辑区渲染 ──────────────────────────────────────────────────

/// 递归渲染 PaneGroup。
/// 每个 Pane Entity 渲染自身（标签栏 + 编辑器内容）。
fn render_pane_group(group: &PaneGroup) -> gpui::Div {
    match group {
        PaneGroup::Pane(_, entity) => div().flex_1().min_h(space::S16).child(entity.clone()),
        PaneGroup::Split { axis, children, .. } => render_split(*axis, children),
    }
}

/// 渲染分栏。
fn render_split(axis: Axis, children: &[Box<PaneGroup>; 2]) -> gpui::Div {
    let child_a = render_pane_group(&children[0]);
    let child_b = render_pane_group(&children[1]);

    match axis {
        Axis::Horizontal => div()
            .flex()
            .flex_row()
            .size_full()
            .overflow_hidden()
            .child(div().flex_1().min_w_0().child(child_a))
            .child(
                div()
                    .flex_shrink_0()
                    .w(px(1.0))
                    .h_full()
                    .bg(color::current().gray.s[4]),
            )
            .child(div().flex_1().min_w_0().child(child_b)),
        Axis::Vertical => div()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .child(div().flex_1().min_h_0().child(child_a))
            .child(
                div()
                    .flex_shrink_0()
                    .h(px(1.0))
                    .w_full()
                    .bg(color::current().gray.s[4]),
            )
            .child(div().flex_1().min_h_0().child(child_b)),
    }
}
