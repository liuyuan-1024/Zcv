//! 嵌入文本编辑器的"插槽"——业务侧持的唯一句柄。
//!
//! 每个嵌入点在 [`ShellView`] 启动时各装配一个 [`TextEditorSlot`]，业务渲染
//! 只用 `slot.embed()` 一行拿到渲染元素：
//!
//! - 系统输入法的 [`EditorInputHost`] 注册由 slot 内部完成 —— 调用方不接触；
//! - 快照（文本 + 光标字节位）由 slot 通过 [`EditorRouter`] 反查 owner 取，
//!   调用方不再透传 `state.text` / `state.cursor_byte`；
//! - 跨帧稳定的 element id 由 slot 根据 [`AppFocus`] 派生，调用方不再起名字；
//! - 光标闪烁由 [`super::CaretClock`] 全局承载，与 slot 无关。
//!
//! slot 不预设"什么场景配什么能力" —— 内核形态（多行 / 单行 + gutter / scroll /
//! viewport hook）由调用方在 `install` 时通过 [`EditorKernel`] builder 拼好
//! 直接传入；编辑器子系统对调用方一无所知。
//!
//! [`ShellView`]: crate::shell::view::ShellView
//! [`EditorRouter`]: super::EditorRouter
//! [`AppFocus`]: crate::focus::AppFocus

use std::cell::{Cell, RefCell};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use gpui::{App as GpuiApp, Context, ElementId, FocusHandle, Pixels, Window};
use zom_engine::ByteOffset;

use crate::app::App;
use crate::editor::kernel::EditorKernel;
use crate::editor::pointer::{PointerScrollHook, PointerSelectionHook, PointerSelectionSession};
use crate::focus::AppFocus;
use crate::host_intent::{HostIntent, HostIntentRequest, InteractionIntent, PointerIntent};

use super::element::EditorElement;
use super::input_host::EditorInputHost;

pub(crate) struct TextEditorSlot {
    focus: AppFocus,
    kernel: EditorKernel,
    input: EditorInputHost,
    host_intent: HostIntentRequest,
    app: Rc<RefCell<App>>,
    element_id: ElementId,
    scroll_accumulated_y: Rc<Cell<f32>>,
    pointer_session: PointerSelectionSession,
}

impl TextEditorSlot {
    pub(crate) fn install<V: 'static>(
        app: Rc<RefCell<App>>,
        host_intent: HostIntentRequest,
        focus: AppFocus,
        kernel: EditorKernel,
        focus_handle: FocusHandle,
        cx: &mut Context<V>,
    ) -> Rc<Self> {
        let input =
            EditorInputHost::new(Rc::clone(&app), Rc::clone(&host_intent), focus_handle, cx);

        // 基于 AppFocus 算出稳定的 ElementId
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        focus.hash(&mut hasher);
        let element_id = ElementId::from(hasher.finish() as usize);

        Rc::new(Self {
            focus,
            kernel,
            input,
            host_intent,
            app,
            element_id,
            scroll_accumulated_y: Rc::new(Cell::new(0.0)),
            pointer_session: PointerSelectionSession::new(),
        })
    }

    /// 嵌入：业务渲染唯一对外的工厂。
    ///
    /// 渲染路径是单线程顺序的，对 `App` 的两次借用（先 mut settle、再 ref snapshot）
    /// 不会与外层任何活借用冲突。
    ///
    /// 1. **settle**：把视口 Y 轴推进到本帧应切的窗口（吸收 reveal / edge-scroll）。
    ///    必须先于 `snapshot_for_focus` —— snapshot 切片范围依赖 `view.viewport().top_line`。
    /// 2. **snapshot**：按已落定的视口拼快照供 element 消费。
    pub(crate) fn embed(&self) -> EditorElement {
        self.app
            .borrow_mut()
            .with_router_mut(|mut router| router.settle_viewport_for_focus(self.focus));

        let snapshot = self
            .app
            .borrow()
            .with_router(|router| router.snapshot_for_focus(self.focus));

        self.kernel
            .element(snapshot, self.input.focus_handle(), self.input.hook())
            .element_id(self.element_id.clone())
            .scroll_hook(self.scroll_hook())
            .selection_hook(self.selection_hook())
            .pointer_session(self.pointer_session())
    }

    fn scroll_hook(&self) -> PointerScrollHook {
        let host_intent = Rc::clone(&self.host_intent);
        let focus = self.focus;
        let accumulated_scroll_y = Rc::clone(&self.scroll_accumulated_y);
        Rc::new(
            move |incoming: Pixels, line_height: Pixels, window: &mut Window, cx: &mut GpuiApp| {
                let delta_visual_rows =
                    consume_scroll_rows(&accumulated_scroll_y, incoming, line_height);
                if delta_visual_rows == 0 {
                    return;
                }
                host_intent(
                    HostIntent::Interaction(InteractionIntent::Pointer(
                        PointerIntent::ScrollViewport {
                            focus,
                            delta_visual_rows,
                        },
                    )),
                    window,
                    cx,
                );
            },
        )
    }

    fn selection_hook(&self) -> PointerSelectionHook {
        let host_intent = Rc::clone(&self.host_intent);
        let focus = self.focus;
        let focus_handle = self.input.focus_handle();
        Rc::new(move |update, window: &mut Window, cx: &mut GpuiApp| {
            window.focus(&focus_handle);
            host_intent(
                HostIntent::Interaction(InteractionIntent::Pointer(PointerIntent::SetSelection {
                    focus,
                    anchor: ByteOffset::new(update.anchor),
                    head: ByteOffset::new(update.head),
                })),
                window,
                cx,
            );
        })
    }

    pub(crate) fn pointer_session(&self) -> PointerSelectionSession {
        self.pointer_session.clone()
    }

    pub(crate) fn cancel_pointer_selection(&self) {
        self.pointer_session.clear();
    }
}

fn consume_scroll_rows(
    accumulated_scroll_y: &Cell<f32>,
    incoming: Pixels,
    line_height: Pixels,
) -> i64 {
    let line_height_f: f32 = line_height.into();
    if line_height_f <= 0.0 {
        return 0;
    }

    let accumulated_f = accumulated_scroll_y.get();
    let incoming_f: f32 = incoming.into();
    let incoming_f = if cfg!(target_os = "macos") {
        -incoming_f
    } else {
        incoming_f
    };
    let combined = if incoming_f == 0.0 {
        accumulated_f
    } else if accumulated_f == 0.0 || accumulated_f.signum() == incoming_f.signum() {
        accumulated_f + incoming_f
    } else {
        incoming_f
    };

    let rows_f = combined / line_height_f;
    let whole_rows = if rows_f >= 1.0 {
        rows_f.floor() as i64
    } else if rows_f <= -1.0 {
        rows_f.ceil() as i64
    } else {
        0
    };
    accumulated_scroll_y.set(combined - whole_rows as f32 * line_height_f);
    whole_rows
}
