//! 嵌入文本编辑器的"插槽"——业务侧持的唯一句柄。
//!
//! 三处嵌入点（主编辑区 / 文件树新建条目 / 项目选择器查询框）在 [`ShellView`]
//! 启动时各装配一个 [`TextEditorSlot`]，业务渲染只用 `slot.embed(kind)` 一行
//! 拿到 [`EditorEmbed`]：
//!
//! - 系统输入法的 [`EditorInputHost`] 注册由 slot 内部完成 —— 调用方不接触；
//! - 快照（文本 + 光标字节位）由 slot 通过 [`EditorRouter`] 反查 owner 取，
//!   调用方不再透传 `state.text` / `state.cursor_byte`；
//! - 跨帧稳定的 element id 由 slot 根据 [`TextTargetId`] 自带，调用方不再起名字；
//! - 光标闪烁由 [`super::CaretClock`] 全局承载，与 slot 无关。
//!
//! [`ShellView`]: crate::shell::view::ShellView
//! [`EditorRouter`]: super::EditorRouter

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Context, ElementId, FocusHandle};

use crate::app::App;

use super::embed::{EditorEmbed, EditorInputHost};
use super::{EditorKind, TextTargetId};

pub(crate) struct TextEditorSlot {
    target_id: TextTargetId,
    input: EditorInputHost,
    app: Rc<RefCell<App>>,
    element_id: ElementId,
}

impl TextEditorSlot {
    pub(crate) fn install<V: 'static>(
        app: Rc<RefCell<App>>,
        target_id: TextTargetId,
        focus: FocusHandle,
        cx: &mut Context<V>,
    ) -> Rc<Self> {
        let input = EditorInputHost::new(Rc::clone(&app), target_id, focus, cx);
        Rc::new(Self {
            target_id,
            input,
            app,
            element_id: element_id_for(target_id).into(),
        })
    }

    /// 嵌入：业务渲染唯一对外的工厂。
    ///
    /// 快照在调用瞬间从 App 拉一份 —— 渲染路径是单线程顺序的，此处的 `App`
    /// 借用不会与外层任何活借用冲突。
    pub(crate) fn embed(&self, kind: EditorKind) -> EditorEmbed {
        let snapshot = self
            .app
            .borrow()
            .with_router(|router| router.snapshot_for(self.target_id));
        EditorEmbed::new(kind, snapshot, self.input.clone()).element_id(self.element_id.clone())
    }
}

/// 嵌入点稳定 element id —— 跨帧保留 [`super::element::EditorElement`] 滚动偏移。
fn element_id_for(target: TextTargetId) -> &'static str {
    match target {
        TextTargetId::MainEditor => "zom-editor-main",
        TextTargetId::FileTreePendingName => "zom-editor-file-tree-pending",
        TextTargetId::ProjectPickerQuery => "zom-editor-project-picker-query",
    }
}
