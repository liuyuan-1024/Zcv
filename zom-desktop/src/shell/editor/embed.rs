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
}

impl EditorEmbed {
    pub(crate) fn new(kind: EditorKind, snapshot: EditorSnapshot, input: EditorInputHost) -> Self {
        Self {
            kind,
            snapshot,
            input,
            element_id: None,
        }
    }

    pub(crate) fn element_id(mut self, id: impl Into<ElementId>) -> Self {
        self.element_id = Some(id.into());
        self
    }
}

impl IntoElement for EditorEmbed {
    type Element = EditorElement;

    fn into_element(self) -> Self::Element {
        let mut element = EditorElement::new(
            self.kind,
            self.snapshot.text,
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
        element
    }
}
