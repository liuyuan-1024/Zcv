//! 编辑器鼠标子系统。
//!
//! 这里内聚“鼠标事件如何变成编辑意图”：命中测试、拖拽 session、滚轮分发都在
//! pointer 层完成。真正修改 buffer / selection 的语义仍交给宿主 hook。

mod hit_test;
mod intent;
mod session;

use std::rc::Rc;

use gpui::{
    Bounds, DispatchPhase, Hitbox, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, ScrollWheelEvent, Window,
};

pub(crate) use hit_test::{PointerHitLine, PointerHitTest};
pub(crate) use intent::{PointerScrollHook, PointerSelectionHook, PointerSelectionUpdate};
pub(crate) use session::PointerSelectionSession;

pub(crate) fn install_selection_handlers(
    hitbox: Hitbox,
    bounds: Bounds<Pixels>,
    hit_test: Rc<PointerHitTest>,
    session: PointerSelectionSession,
    selection_hook: PointerSelectionHook,
    window: &mut Window,
) {
    let hitbox_for_down = hitbox.clone();
    let hit_test_for_down = Rc::clone(&hit_test);
    let session_for_down = session.clone();
    let selection_hook_for_down = Rc::clone(&selection_hook);
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble
            && event.button == MouseButton::Left
            && hitbox_for_down.is_hovered(window)
            && let Some(byte) = hit_test_for_down.byte_for_point(event.position, bounds)
        {
            session_for_down.begin(byte);
            selection_hook_for_down(PointerSelectionUpdate::new(byte, byte), window, cx);
            cx.stop_propagation();
        }
    });

    let hit_test_for_move = Rc::clone(&hit_test);
    let session_for_move = session.clone();
    let selection_hook_for_move = Rc::clone(&selection_hook);
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble {
            return;
        }
        let Some(anchor) = session_for_move.anchor() else {
            return;
        };
        if !event.dragging() {
            session_for_move.clear();
            return;
        }
        if let Some(head) = hit_test_for_move.byte_for_point(event.position, bounds) {
            selection_hook_for_move(PointerSelectionUpdate::new(anchor, head), window, cx);
            cx.stop_propagation();
        }
    });

    let session_for_up = session;
    window.on_mouse_event(move |event: &MouseUpEvent, phase, _window, cx| {
        if phase == DispatchPhase::Bubble
            && event.button == MouseButton::Left
            && session_for_up.is_active()
        {
            session_for_up.clear();
            cx.stop_propagation();
        }
    });
}

pub(crate) fn install_scroll_handler(
    hitbox: Hitbox,
    line_height: Pixels,
    scroll_hook: PointerScrollHook,
    window: &mut Window,
) {
    window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && hitbox.should_handle_scroll(window) {
            let pixel_delta = event.delta.pixel_delta(line_height);
            scroll_hook(pixel_delta.y, line_height, window, cx);
            cx.stop_propagation();
        }
    });
}
