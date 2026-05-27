//! 编辑器路由 —— 集中所有"按 target_id 反查 owner"的派发与活跃判定。
//!
//! 路由器是 App 端按当前借用强度临时构造的视图：
//! - 只读路径用 [`EditorRouter`]，持 `&dyn TextTargetQuery` 列表 ——
//!   做活跃判定、preedit 查询、focused profile 等。
//! - 可写路径用 [`EditorRouterMut`]，持 `&mut dyn TextTargetOwner` 列表 ——
//!   做 IME 写入回调；外部通过闭包接收 [`ImeTarget`]，借用在闭包结束后释放。
//!
//! Owner 数组顺序即优先级：第一个 `is_active()` 为 `true` 的 owner 即为
//! 当前焦点 target。主编辑区作为兜底放在数组末尾。
//!
//! 写入端的 `edit_target` 由于要把借用透传到 [`zom_command::CommandContext`]
//! 跨越执行器整个生命周期，不走路由器；App 直接顺序问 owners，路由器只承担
//! "查询 + IME 写入闭包"两类需求。

use std::ops::Range;

use zom_command::{CommandError, KeyContext};

use crate::shell::editor::input::{ImeQueryTarget, ImeTarget};
use crate::shell::editor::snapshot::EditorSnapshot;

use super::{TextTargetId, TextTargetOwner, TextTargetQuery};

/// 只读路由：focused_target / focused_key_contexts / is_composing / snapshot / preedit 等。
pub(crate) struct EditorRouter<'a> {
    owners: Vec<&'a dyn TextTargetQuery>,
}

/// 可写路由：仅承载 IME 写入回调（不返回借用）。
pub(crate) struct EditorRouterMut<'a> {
    owners: Vec<&'a mut dyn TextTargetOwner>,
}

impl<'a> EditorRouter<'a> {
    pub(crate) fn new(owners: Vec<&'a dyn TextTargetQuery>) -> Self {
        Self { owners }
    }

    /// 当前焦点 target —— 按 owner 数组顺序找第一个活跃者。
    pub(crate) fn focused_target(&self) -> Option<TextTargetId> {
        self.owners
            .iter()
            .find(|owner| owner.is_active())
            .map(|owner| owner.target_id())
    }

    /// 当前焦点 owner 的按键解析上下文栈（优先级从高到低）。
    ///
    /// 路由层不替业务拼栈，只把活跃 owner 给的 `key_contexts()` 透传出来。
    pub(crate) fn focused_key_contexts(&self) -> Option<Vec<KeyContext>> {
        self.owners
            .iter()
            .find(|owner| owner.is_active())
            .map(|owner| owner.key_contexts())
    }

    /// 当前活动 target 是否处于"非空 preedit"的输入法组合态。
    ///
    /// 空 preedit（系统输入法取消候选后留下的空壳）不算组合态：留着 IME
    /// 会再吞下一个按键，表现为"取消候选后要多按一次 Esc 才退出新建"。
    pub(crate) fn is_composing(&self) -> bool {
        let Some(id) = self.focused_target() else {
            return false;
        };
        self.preedit_text(id)
            .is_some_and(|preedit| !preedit.is_empty())
    }

    pub(crate) fn snapshot_for(&self, target: TextTargetId) -> EditorSnapshot {
        self.owners
            .iter()
            .find(|owner| owner.target_id() == target)
            .map(|owner| owner.snapshot())
            .unwrap_or_default()
    }

    pub(crate) fn marked_range_utf16(&self, target: TextTargetId) -> Option<Range<usize>> {
        self.with_query(target, |q| q.marked_range_utf16())
            .flatten()
    }

    pub(crate) fn selected_range_utf16(
        &self,
        target: TextTargetId,
    ) -> Option<(Range<usize>, bool)> {
        self.with_query(target, |q| q.selected_range_utf16())
    }

    pub(crate) fn text_for_range_utf16(
        &self,
        target: TextTargetId,
        range: Range<usize>,
    ) -> Option<String> {
        self.with_query(target, |q| q.text_for_range_utf16(range))
            .flatten()
    }

    pub(crate) fn preedit_text(&self, target: TextTargetId) -> Option<String> {
        self.with_query(target, |q| q.preedit_text()).flatten()
    }

    fn with_query<R>(
        &self,
        target: TextTargetId,
        f: impl FnOnce(&ImeQueryTarget<'_>) -> R,
    ) -> Option<R> {
        let owner = self
            .owners
            .iter()
            .find(|owner| owner.target_id() == target)?;
        let query = owner.ime_query_target()?;
        Some(f(&query))
    }
}

impl<'a> EditorRouterMut<'a> {
    pub(crate) fn new(owners: Vec<&'a mut dyn TextTargetOwner>) -> Self {
        Self { owners }
    }

    /// 把指定 target 的 IME 写入目标交给闭包；借用在闭包结束后立即释放。
    ///
    /// 闭包成功返回后调一次 owner 的 [`TextTargetOwner::after_text_changed`]
    /// 钩子 —— "文本变了之后 owner 要做什么"由 owner 自己说，router 不持那种
    /// 业务知识（picker 重置 selection 的逻辑因此不再 leak 进宿主）。
    pub(crate) fn with_ime_target<R>(
        mut self,
        target: TextTargetId,
        f: impl FnOnce(ImeTarget<'_>) -> Result<R, CommandError>,
    ) -> Result<R, CommandError> {
        for owner in self.owners.iter_mut() {
            if owner.target_id() != target {
                continue;
            }
            let ime = owner.ime_target().ok_or(CommandError::NoActiveView)?;
            let result = f(ime)?;
            // ime 已在 f 调用中消耗掉，借用释放；这里可以再次 &mut owner。
            owner.after_text_changed();
            return Ok(result);
        }
        Err(CommandError::NoActiveView)
    }
}
