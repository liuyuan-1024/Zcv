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
use super::{EditorInput, EditorKind, EditorSnapshot, TextTargetId};

pub(crate) type EditorInputHook = Rc<dyn Fn(Bounds<Pixels>, &mut Window, &mut GpuiApp)>;

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
        Rc::new(move |bounds, window, cx| {
            window.handle_input(&focus, ElementInputHandler::new(bounds, input.clone()), cx);
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
