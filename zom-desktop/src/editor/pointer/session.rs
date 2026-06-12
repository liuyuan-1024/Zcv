//! 鼠标 selection 手势的跨帧状态。

use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone, Default)]
pub(crate) struct PointerSelectionSession {
    anchor: Rc<Cell<Option<usize>>>,
}

impl PointerSelectionSession {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn begin(&self, anchor: usize) {
        self.anchor.set(Some(anchor));
    }

    pub(crate) fn anchor(&self) -> Option<usize> {
        self.anchor.get()
    }

    pub(crate) fn clear(&self) {
        self.anchor.set(None);
    }

    pub(crate) fn is_active(&self) -> bool {
        self.anchor.get().is_some()
    }
}
