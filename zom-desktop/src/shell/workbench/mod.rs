//! WorkbenchFrame —— 窗口顶层装配（布局模型 4 / 手册 二十一 z-index 分层）。
//!
//! ```text
//! ┌────────────────────────────────────┐
//! │ TopBar                             │
//! ├───────┬────────────────────┬───────┤
//! │ Left  │    EditorGrid      │ Right │
//! │ Dock  ├────────────────────┤ Dock  │
//! │       │    BottomDock      │       │
//! ├───────┴────────────────────┴───────┤
//! │ BottomBar                          │
//! └────────────────────────────────────┘
//! ```
//!
//! 渲染层次（手册 21.2）：
//!   [10] WorkbenchFrame
//!   [20] OverlayShell
//!   [30] BubbleShell
//! 后两层骨架阶段为空 portal，不参与 layout。

use gpui::{Div, Entity, FocusHandle, Window, div, prelude::*};

use crate::shell::model::WorkbenchState;
use crate::shell::overlay::{AnchorRegistry, OverlayShell};
use crate::shell::panels::PanelHost;
use crate::shell::theme::{color, radius};
use crate::shell::{InputHandlerHook, KeyRequest, ShortcutLookup, WindowControlsHandlers};

mod body;
mod bottom_bar;
mod dock;
mod overlays;
mod top_bar;

pub(crate) fn render(
    state: &WorkbenchState,
    host: &PanelHost,
    window: &Window,
    window_controls: WindowControlsHandlers,
    overlay_shell: Entity<OverlayShell>,
    anchor_registry: Entity<AnchorRegistry>,
    workspace_active: bool,
    key_request: KeyRequest,
    shortcut_lookup: ShortcutLookup,
    input_handler_hook: InputHandlerHook,
    editor_focus: FocusHandle,
) -> Div {
    div()
        .relative()
        .flex()
        .flex_col()
        .size_full()
        .overflow_hidden()
        .rounded(radius::window())
        .border_1()
        .border_color(color::gray::g40())
        .bg(color::gray::g05())
        .text_color(color::gray::g90())
        .child(top_bar::render(
            window,
            window_controls,
            &shortcut_lookup,
            anchor_registry,
            workspace_active,
        ))
        .child(render_body(
            state,
            host,
            key_request,
            input_handler_hook,
            editor_focus,
        ))
        .child(bottom_bar::render(state, &shortcut_lookup))
        .child(overlay_shell)
        .child(overlays::bubble_layer::render())
}

fn render_body(
    state: &WorkbenchState,
    host: &PanelHost,
    key_request: KeyRequest,
    input_handler_hook: InputHandlerHook,
    editor_focus: FocusHandle,
) -> Div {
    let mut row = div().flex_1().flex().flex_row().w_full().overflow_hidden();

    if state.left_dock.is_visible() {
        row = row.child(dock::left::render(&state.left_dock, host));
    }
    row = row.child(body::render(
        &state.bottom_dock,
        &state.editor,
        host,
        key_request,
        input_handler_hook,
        editor_focus,
    ));
    if state.right_dock.is_visible() {
        row = row.child(dock::right::render(&state.right_dock, host));
    }

    row
}
