//! 鼠标交互产出的编辑意图边界。
//!
//! 本模块只描述 pointer 子系统交给宿主的结果；如何把它落成真正的
//! editor command / text target mutation 由上层负责。

use std::rc::Rc;

use gpui::{App as GpuiApp, Pixels, Window};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PointerSelectionUpdate {
    pub(crate) anchor: usize,
    pub(crate) head: usize,
}

impl PointerSelectionUpdate {
    pub(crate) const fn new(anchor: usize, head: usize) -> Self {
        Self { anchor, head }
    }
}

/// 鼠标 selection 手势交给宿主的唯一出口。
pub(crate) type PointerSelectionHook =
    Rc<dyn Fn(PointerSelectionUpdate, &mut Window, &mut GpuiApp)>;

/// 鼠标滚轮交给宿主的唯一出口；宿主负责累计像素并折算为视觉行。
pub(crate) type PointerScrollHook = Rc<dyn Fn(Pixels, Pixels, &mut Window, &mut GpuiApp)>;
