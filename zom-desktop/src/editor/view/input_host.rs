//! 编辑器输入宿主。
//!
//! 系统输入法注册与 GPUI input handler 装配都内聚在这里；渲染元素由
//! [`super::slot::TextEditorSlot`] 直接创建。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    App as GpuiApp, AppContext, Bounds, ElementInputHandler, Entity, FocusHandle, Pixels, Window,
};
use zom_view::WrapMap;

use crate::app::App;
use crate::editor::input::{CaretLayout, EditorInput};
use crate::host_intent::HostIntentRequest;

/// element paint 阶段传给 input hook 的几何信息。
///
/// `bounds` 给 `ElementInputHandler` 作为 element_bounds 锚（GPUI 输入路径会
/// 把它原样喂给 `bounds_for_range`）；`caret_layout` 由 hook 写到 `EditorInput`
/// 实体，供 IME 候选窗定位。
pub(crate) struct EditorPaintInfo {
    pub bounds: Bounds<Pixels>,
    pub caret_layout: Option<CaretLayout>,
}

pub(crate) type EditorInputHook = Rc<dyn Fn(EditorPaintInfo, &mut Window, &mut GpuiApp)>;

/// element prepaint 中段同步给 view 的视口测量值。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EditorViewportMeasurement {
    pub visible_visual_rows: u64,
    pub visible_logical_lines: u64,
}

/// 同步钩子用本帧新 wrap_map 再 settle 一次后返回的视口顶端。
/// element 据此决定本帧实际渲染哪一段（避免「edit → settle 看不到新行」的一帧滞后）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SettledViewportTop {
    pub top_line: u64,
    pub top_subrow: u64,
}

/// element prepaint 中段用来把本帧测得的视口与 wrap_map 写回 view 的钩子。
///
/// 钩子内部先把 `wrap_map` / `visible_visual_rows` 落到 view，然后用最新 wrap_map 立即 [`zom_view::View::settle_viewport_y`] 一次，返回 settle 后的视口顶端；
/// element 用返回值决定本帧的 `top_visual_row`，从而软 / 硬换行下都能在「插入新行 / 触发新 sub-row」的同一帧把光标拉回视区。
///
/// 主编辑区装一个真实实现；其它单行嵌入编辑器视口固定为单行，无需钩子。
/// 钩子返回 `None` 表示没有活动 view 可 settle，调用方退回 view 当前 top。
pub(crate) type EditorViewportSyncHook = Rc<
    dyn Fn(EditorViewportMeasurement, Option<WrapMap>, &mut GpuiApp) -> Option<SettledViewportTop>,
>;

#[derive(Clone)]
pub(crate) struct EditorInputHost {
    focus: FocusHandle,
    input: Entity<EditorInput>,
}

impl EditorInputHost {
    pub(crate) fn new<T>(
        app: Rc<RefCell<App>>,
        host_intent: HostIntentRequest,
        focus: FocusHandle,
        cx: &mut gpui::Context<T>,
    ) -> Self {
        let input = cx.new(|_| EditorInput::new(app, host_intent));
        Self { focus, input }
    }

    pub(super) fn hook(&self) -> EditorInputHook {
        let focus = self.focus.clone();
        let input = self.input.clone();
        Rc::new(move |info: EditorPaintInfo, window, cx| {
            // 先把 caret 几何写回 input 实体，再 handle_input —— 顺序很重要。
            // IME 接管输入后立刻可能问 bounds_for_range，那时 caret_layout 必须已经是本帧的最新值。
            input.update(cx, |this, _| this.set_caret_layout(info.caret_layout));
            window.handle_input(
                &focus,
                ElementInputHandler::new(info.bounds, input.clone()),
                cx,
            );
        })
    }

    pub(super) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }
}
