//! 对外的可嵌入编辑器 API。
//!
//! 调用方只提供目标、快照和外壳焦点；系统输入法注册、GPUI input handler
//! 和底层 [`EditorElement`] 组装都内聚在本模块。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    App as GpuiApp, AppContext, Bounds, ElementId, ElementInputHandler, Entity, FocusHandle,
    IntoElement, Pixels, Window,
};

use crate::app::App;

use super::element::EditorElement;
use super::input::CaretLayout;
use super::{EditorInput, EditorKind, EditorSnapshot, TextTargetId};

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
/// 关联 view 的钩子。只有主编辑区会装配一个真实实现；单行嵌入式编辑器一律
/// 不装（snapshot 路径不读视口，没人消费写回值）。
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

    fn hook(&self) -> EditorInputHook {
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

    fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }
}

pub(crate) struct EditorEmbed {
    kind: EditorKind,
    snapshot: EditorSnapshot,
    input: EditorInputHost,
    element_id: Option<ElementId>,
    viewport_sync: Option<EditorViewportSyncHook>,
}

impl EditorEmbed {
    pub(crate) fn new(kind: EditorKind, snapshot: EditorSnapshot, input: EditorInputHost) -> Self {
        Self {
            kind,
            snapshot,
            input,
            element_id: None,
            viewport_sync: None,
        }
    }

    pub(crate) fn element_id(mut self, id: impl Into<ElementId>) -> Self {
        self.element_id = Some(id.into());
        self
    }

    /// 装配视口写回钩子；只主编辑区会调用本方法。
    pub(crate) fn viewport_sync(mut self, hook: EditorViewportSyncHook) -> Self {
        self.viewport_sync = Some(hook);
        self
    }
}

impl IntoElement for EditorEmbed {
    type Element = EditorElement;

    fn into_element(self) -> Self::Element {
        let mut element = EditorElement::new(
            self.kind,
            self.snapshot.lines,
            self.snapshot.total_lines,
            self.snapshot.viewport_start_line,
            self.snapshot.cursor_position,
            self.snapshot.selection,
            self.input.focus_handle(),
            self.input.hook(),
        );
        if let Some(id) = self.element_id {
            element = element.element_id(id);
        }
        if let Some(reveal) = self.snapshot.reveal {
            element = element.reveal(reveal);
        }
        if let Some(hook) = self.viewport_sync {
            element = element.viewport_sync(hook);
        }
        // search overlay：单行嵌入输入框的 snapshot 自带空 Vec / None，不会画。
        element = element.search_overlay(self.snapshot.search_hits, self.snapshot.search_current);
        element
    }
}
