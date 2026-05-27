//! 编辑器输入宿主。
//!
//! 系统输入法注册与 GPUI input handler 装配都内聚在这里；渲染元素由
//! [`super::slot::TextEditorSlot`] 直接创建。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    App as GpuiApp, AppContext, Bounds, ElementInputHandler, Entity, FocusHandle, Pixels, Window,
};

use crate::app::App;
use crate::shell::editor::input::{CaretLayout, EditorInput};
use crate::shell::editor::target::TextTargetId;

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

/// element prepaint 末尾用来把当前视口状态（顶部可见逻辑行 + 可见行数）写回
/// 关联 view 的钩子。只有主编辑区会装配一个真实实现；单行嵌入式编辑器同样
/// 拥有视口模型，但其视口固定为单行，无需跨帧写回。
pub(crate) type EditorViewportSyncHook = Rc<dyn Fn(u64, u64, &mut GpuiApp)>;

#[derive(Clone)]
pub(crate) struct EditorInputHost {
    focus: FocusHandle,
    input: Entity<EditorInput>,
}

impl EditorInputHost {
    pub(crate) fn new<T>(
        app: Rc<RefCell<App>>,
        target: TextTargetId,
        focus: FocusHandle,
        cx: &mut gpui::Context<T>,
    ) -> Self {
        let input = cx.new(|_| EditorInput::new(app, target));
        Self { focus, input }
    }

    pub(super) fn hook(&self) -> EditorInputHook {
        let focus = self.focus.clone();
        let input = self.input.clone();
        Rc::new(move |info: EditorPaintInfo, window, cx| {
            // 先把 caret 几何写回 input 实体，再 handle_input —— 顺序很重要：
            // IME 接管输入后立刻可能问 bounds_for_range，那时 caret_layout 必须
            // 已经是本帧的最新值。
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
