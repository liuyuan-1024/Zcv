//! 嵌入文本编辑器的"插槽"——业务侧持的唯一句柄。
//!
//! 每个嵌入点在 [`ShellView`] 启动时各装配一个 [`TextEditorSlot`]，业务渲染
//! 只用 `slot.embed()` 一行拿到渲染元素：
//!
//! - 系统输入法的 [`EditorInputHost`] 注册由 slot 内部完成 —— 调用方不接触；
//! - 快照（文本 + 光标字节位）由 slot 通过 [`EditorRouter`] 反查 owner 取，
//!   调用方不再透传 `state.text` / `state.cursor_byte`；
//! - 跨帧稳定的 element id 由 slot 根据 [`TextTargetId`] 自带，调用方不再起名字；
//! - 光标闪烁由 [`super::CaretClock`] 全局承载，与 slot 无关。
//!
//! slot 不预设"什么场景配什么能力" —— 内核形态（多行 / 单行 + gutter / scroll /
//! viewport hook）由调用方在 `install` 时通过 [`EditorKernel`] builder 拼好
//! 直接传入；编辑器子系统对调用方一无所知。
//!
//! [`ShellView`]: crate::shell::view::ShellView
//! [`EditorRouter`]: super::EditorRouter

use std::cell::RefCell;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use gpui::{Context, ElementId, FocusHandle};

use crate::app::App;
use crate::focus::AppFocus;
use crate::shell::editor::kernel::EditorKernel;

use super::element::EditorElement;
use super::input_host::EditorInputHost;

pub(crate) struct TextEditorSlot {
    focus: AppFocus,
    kernel: EditorKernel,
    input: EditorInputHost,
    app: Rc<RefCell<App>>,
    element_id: ElementId,
}

impl TextEditorSlot {
    pub(crate) fn install<V: 'static>(
        app: Rc<RefCell<App>>,
        focus: AppFocus,
        kernel: EditorKernel,
        focus_handle: FocusHandle,
        cx: &mut Context<V>,
    ) -> Rc<Self> {
        let input = EditorInputHost::new(Rc::clone(&app), focus_handle, cx);

        // 基于 AppFocus 算出稳定的 ElementId
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        focus.hash(&mut hasher);
        let element_id = ElementId::from(hasher.finish() as usize);

        Rc::new(Self {
            focus,
            kernel,
            input,
            app,
            element_id,
        })
    }

    /// 嵌入：业务渲染唯一对外的工厂。
    ///
    /// 快照在调用瞬间从 App 拉一份 —— 渲染路径是单线程顺序的，此处的 `App`
    /// 借用不会与外层任何活借用冲突。
    pub(crate) fn embed(&self) -> EditorElement {
        let snapshot = self
            .app
            .borrow()
            .with_router(|router| router.snapshot_for_focus(self.focus));

        self.kernel
            .element(snapshot, self.input.focus_handle(), self.input.hook())
            .element_id(self.element_id.clone())
    }
}
