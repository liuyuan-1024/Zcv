//! 布局渲染 —— 将 LayoutSnapshot 渲染为 GPUI Div 树。

use gpui::{MouseButton, Pixels, Window, div, prelude::*, px};
use zcv_engine::{ByteOffset, TextRange};

use crate::editor::ViewRegistry;
use crate::theme::{color, radius, space, typography};

use super::types::{Axis, DockArea, DockState, LayoutSnapshot, Pane, PaneGroup, PanelId, ViewId};

/// 面板内容提供者：布局不感知具体 panel 类型，通过此回调获取内容。
pub(crate) type PanelContentFn<'a> = dyn Fn(PanelId) -> Option<gpui::Div> + 'a;

/// 渲染 workbench 主体（不包含顶栏和底栏）。
pub(crate) fn render_body(
    layout: &LayoutSnapshot,
    panel_content: &PanelContentFn,
    views: &ViewRegistry,
) -> gpui::Div {
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

    center_col = center_col.child(
        div()
            .flex_1()
            .min_h(space::S16)
            .child(render_pane_group(&layout.center, views)),
    );

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

/// 渲染单个 dock 面板（含边框 + 内嵌拖拽热区）。
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

    // 面板内容
    let body: gpui::Div = match state.active_panel.and_then(panel_content) {
        Some(content) => div().size_full().child(content),
        None => {
            let label = state.active_panel.map(|p| p.label()).unwrap_or("");
            render_placeholder(label)
        }
    };
    let frame = frame.child(body);

    // 拖拽热区嵌入在 dock 的边框内侧，与 border 绑死
    const HIT: Pixels = space::S6;
    match area {
        DockArea::Left => frame.child(dock_drag_zone(DockArea::Left).right(px(0.0)).w(HIT)),
        DockArea::Right => frame.child(dock_drag_zone(DockArea::Right).left(px(0.0)).w(HIT)),
        DockArea::Bottom => frame.child(dock_drag_zone(DockArea::Bottom).top(px(0.0)).h(HIT)),
    }
}

/// 占位文字（panel 无自定义内容时显示）。
fn render_placeholder(label: &str) -> gpui::Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(color::current().gray.s[5])
        .child(label.to_string())
}

// ── 拖拽热区和分隔线渲染（绝对定位，不参与 flex 流） ────────────

/// 拖拽热区 ── 事件处理内嵌，三个 dock 区域共用。
fn dock_drag_zone(area: DockArea) -> gpui::Div {
    let base = div()
        .absolute()
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            if let Some(layout_ref) = cx.try_global::<super::controller::LayoutRef>()
                && let Some(ctrl) = layout_ref.0.upgrade()
            {
                ctrl.borrow_mut().start_dock_drag(area, event.position);
                window.refresh();
            }
        })
        .on_mouse_up(MouseButton::Left, move |event, window, cx| {
            if event.click_count >= 2
                && let Some(layout_ref) = cx.try_global::<super::controller::LayoutRef>()
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
fn render_pane_group(group: &PaneGroup, views: &ViewRegistry) -> gpui::Div {
    match group {
        PaneGroup::Pane(pane) => render_pane(pane, views),
        PaneGroup::Split { axis, children, .. } => render_split(*axis, children, views),
    }
}

/// 渲染单个 Pane。
fn render_pane(pane: &Pane, views: &ViewRegistry) -> gpui::Div {
    let has_tabs = !pane.tabs.is_empty();

    // 标签栏
    let tab_bar: gpui::AnyElement = if has_tabs {
        div()
            .flex()
            .flex_row()
            .items_center()
            .h(typography::ui())
            .flex_shrink_0()
            .px(space::S4)
            .gap(space::S2)
            .bg(color::current().gray.s[2])
            .border_b_1()
            .border_color(color::current().gray.s[4])
            .children(pane.tabs.iter().map(|tab| {
                let is_active = Some(tab.view_id) == pane.active;
                div()
                    .px(space::S6)
                    .py(space::S2)
                    .rounded(radius::R2)
                    .text_color(if is_active {
                        color::current().gray.s[8]
                    } else {
                        color::current().gray.s[6]
                    })
                    .bg(if is_active {
                        color::current().gray.s[1]
                    } else {
                        gpui::rgba(0)
                    })
                    .child(tab.title.clone())
            }))
            .into_any_element()
    } else {
        gpui::div().into_any_element()
    };

    // 内容区
    let content: gpui::AnyElement = match pane.active.and_then(|vid| views.get(vid)) {
        Some(view) => render_editor_content(view).into_any_element(),
        None => render_placeholder("无打开文件").into_any_element(),
    };

    div()
        .flex_1()
        .flex()
        .flex_col()
        .overflow_hidden()
        .size_full()
        .bg(color::current().gray.s[1])
        .child(tab_bar)
        .child(content)
}

/// 渲染编辑器内容（多行文本）。
fn render_editor_content(view: &crate::editor::view::View) -> impl gpui::IntoElement {
    let text = {
        let buf = view.buffer.borrow();
        let len = buf.len_bytes();
        if len > ByteOffset::ZERO {
            let range = TextRange::new(ByteOffset::ZERO, len);
            match range {
                Ok(r) => buf
                    .snapshot()
                    .slice_text(r)
                    .ok()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default(),
                Err(_) => String::new(),
            }
        } else {
            String::new()
        }
    };

    let lines: Vec<&str> = text.split('\n').collect();
    if lines.is_empty() {
        return div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_color(color::current().gray.s[5])
            .child("空文件")
            .into_any_element();
    }

    let scroll = view.scroll_line.get() as usize;
    let scroll_idx = scroll.min(lines.len() - 1);
    const MAX_VISIBLE: usize = 200;
    let end = (scroll_idx + MAX_VISIBLE).min(lines.len());
    let view_id = view.id;

    div()
        .flex_1()
        .flex()
        .flex_col()
        .overflow_hidden()
        .font(typography::editor_font())
        .text_size(typography::editor())
        .line_height(typography::editor_line())
        .on_key_down(move |event, window, cx| {
            handle_editor_scroll(&event.keystroke, view_id, window, cx);
        })
        .children(lines[scroll_idx..end].iter().map(|line| {
            div()
                .h(typography::editor_line())
                .px(space::S4)
                .child(line.to_string())
        }))
        .into_any_element()
}

/// 处理编辑器键盘滚动。
fn handle_editor_scroll(
    keystroke: &gpui::Keystroke,
    view_id: ViewId,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let delta = match keystroke.key.as_str() {
        "up" => -1i32,
        "down" => 1,
        "pageup" => -20,
        "pagedown" => 20,
        _ => return,
    };

    cx.update_global::<ViewRegistry, _>(|reg, _| {
        if let Some(view) = reg.get_mut(view_id) {
            let current = view.scroll_line.get() as i32;
            let new = (current + delta).max(0) as u32;
            view.scroll_line.set(new);
        }
    });

    window.refresh();
}

/// 渲染分栏（当前等分渲染，ratio 用于后续 resize 交互）。
fn render_split(axis: Axis, children: &[Box<PaneGroup>; 2], views: &ViewRegistry) -> gpui::Div {
    let child_a = render_pane_group(&children[0], views);
    let child_b = render_pane_group(&children[1], views);

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
