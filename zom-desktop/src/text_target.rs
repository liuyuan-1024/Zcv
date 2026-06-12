//! 文本目标路由协议。
//!
//! 这是 app 与 shell 之间的共享词汇：
//! app 只知道“某个文本目标 owner 能按 [`crate::focus::AppFocus`] 提供命令 / IME 查询能力”，
//! 不知道这些 owner 来自哪个面板、surface 或 GPUI 组件。

use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

use crate::editor::text::{EditorSnapshot, ImeQueryTarget, ImeUtf16Range};
use crate::focus::AppFocus;
use zom_command::{CommandError, EditTarget, KeyContext};
use zom_engine::{ByteOffset, Selection, SelectionSet};

/// 只读侧：是哪个 target、当前是否活跃、给路由用的查询能力。
pub(crate) trait TextTargetQuery {
    /// 这个 owner 是否承载指定的应用语义焦点。
    fn accepts_focus(&self, focus: AppFocus) -> bool;

    /// 给指定 focus 的快照。多 focus owner（search）按 focus 选字段；单 focus
    /// owner 忽略参数。
    fn snapshot(&self, focus: AppFocus) -> EditorSnapshot;

    /// 该 owner 聚焦时的按键解析上下文栈（优先级从高到低）。
    fn key_contexts(&self) -> Vec<KeyContext>;

    /// 该 owner 在文本编辑层是否接受换行（影响 `KeyContext::text_edit` 的参数）。
    fn accepts_newline(&self) -> bool {
        false
    }

    fn ime_query_target(&self, focus: AppFocus) -> Option<ImeQueryTarget<'_>>;
}

/// 可写侧：编辑命令作用目标。
pub(crate) trait TextTargetOwner: TextTargetQuery {
    fn edit_target(&mut self, focus: AppFocus) -> Option<EditTarget<'_>>;

    /// 交互管线请求滚动视口。非视口型文本目标默认忽略。
    fn scroll_viewport(
        &mut self,
        _focus: AppFocus,
        _delta_visual_rows: i64,
    ) -> Result<(), CommandError> {
        Ok(())
    }

    /// 文本输入后置钩子。默认无操作；owner 想响应“文本变了”时自行 override。
    fn after_text_changed(&mut self) {}

    /// 视口 Y 轴落定钩子。默认无操作。
    fn settle_viewport_y(&mut self) {}
}

/// 只读路由：current_focus key contexts / snapshot / preedit 等。
pub(crate) struct EditorRouter<'a> {
    owners: Vec<&'a dyn TextTargetQuery>,
}

/// 可写路由：承载需要落到目标 owner 的可写回调。
pub(crate) struct EditorRouterMut<'a> {
    owners: Vec<&'a mut dyn TextTargetOwner>,
}

impl<'a> EditorRouter<'a> {
    pub(crate) fn new(owners: Vec<&'a dyn TextTargetQuery>) -> Self {
        Self { owners }
    }

    pub(crate) fn accepts_focus(&self, focus: AppFocus) -> bool {
        self.owners.iter().any(|owner| owner.accepts_focus(focus))
    }

    pub(crate) fn key_contexts_for(&self, focus: AppFocus) -> Option<Vec<KeyContext>> {
        self.owners
            .iter()
            .find(|owner| owner.accepts_focus(focus))
            .map(|owner| owner.key_contexts())
    }

    pub(crate) fn is_composing(&self, focus: AppFocus) -> bool {
        self.preedit_text(focus)
            .is_some_and(|preedit| !preedit.is_empty())
    }

    pub(crate) fn snapshot_for_focus(&self, focus: AppFocus) -> EditorSnapshot {
        self.owners
            .iter()
            .find(|owner| owner.accepts_focus(focus))
            .map(|owner| owner.snapshot(focus))
            .unwrap_or_default()
    }

    pub(crate) fn marked_range_utf16(&self, focus: AppFocus) -> Option<ImeUtf16Range> {
        self.with_query(focus, |q| q.marked_range_utf16()).flatten()
    }

    pub(crate) fn selected_range_utf16(&self, focus: AppFocus) -> Option<(ImeUtf16Range, bool)> {
        self.with_query(focus, |q| q.selected_range_utf16())
    }

    pub(crate) fn text_for_range_utf16(
        &self,
        focus: AppFocus,
        range: ImeUtf16Range,
    ) -> Option<String> {
        self.with_query(focus, |q| q.text_for_range_utf16(range))
            .flatten()
    }

    pub(crate) fn preedit_text(&self, focus: AppFocus) -> Option<String> {
        self.with_query(focus, |q| q.preedit_text()).flatten()
    }

    fn with_query<R>(
        &self,
        focus: AppFocus,
        f: impl FnOnce(&ImeQueryTarget<'_>) -> R,
    ) -> Option<R> {
        let owner = self
            .owners
            .iter()
            .find(|owner| owner.accepts_focus(focus))?;
        let query = owner.ime_query_target(focus)?;
        Some(f(&query))
    }
}

impl<'a> EditorRouterMut<'a> {
    pub(crate) fn new(owners: Vec<&'a mut dyn TextTargetOwner>) -> Self {
        Self { owners }
    }

    pub(crate) fn settle_viewport_for_focus(&mut self, focus: AppFocus) {
        if let Some(owner) = self.owners.iter_mut().find(|o| o.accepts_focus(focus)) {
            owner.settle_viewport_y();
        }
    }

    pub(crate) fn set_pointer_selection(
        &mut self,
        focus: AppFocus,
        anchor: ByteOffset,
        head: ByteOffset,
    ) -> Result<(), CommandError> {
        let owner = self
            .owners
            .iter_mut()
            .find(|owner| owner.accepts_focus(focus))
            .ok_or(CommandError::NoActiveView)?;
        let mut target = owner.edit_target(focus).ok_or(CommandError::NoActiveView)?;
        target.set_selection(SelectionSet::new(vec![Selection::new(anchor, head)]))
    }

    pub(crate) fn scroll_viewport(
        &mut self,
        focus: AppFocus,
        delta_visual_rows: i64,
    ) -> Result<(), CommandError> {
        let owner = self
            .owners
            .iter_mut()
            .find(|owner| owner.accepts_focus(focus))
            .ok_or(CommandError::NoActiveView)?;
        owner.scroll_viewport(focus, delta_visual_rows)
    }
}

/// 外部 owner 注册表。shell runtime 通过 `App::install_editor_owner` 注册 owner。
#[derive(Default)]
pub(crate) struct EditorTargetRegistry {
    owners: Vec<Rc<RefCell<dyn TextTargetOwner>>>,
}

impl EditorTargetRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn register(&mut self, owner: Rc<RefCell<dyn TextTargetOwner>>) {
        self.owners.push(owner);
    }

    pub(crate) fn borrow_all(&self) -> Vec<Ref<'_, dyn TextTargetOwner>> {
        let mut out = Vec::with_capacity(self.owners.len());
        for rc in &self.owners {
            out.push(rc.borrow());
        }
        out
    }

    pub(crate) fn borrow_all_mut(&self) -> Vec<RefMut<'_, dyn TextTargetOwner + 'static>> {
        let mut out = Vec::with_capacity(self.owners.len());
        for rc in &self.owners {
            out.push(rc.borrow_mut());
        }
        out
    }
}
