//! 编辑器路由 —— 集中所有"按 AppFocus / target_id 反查 owner"的派发。
//!
//! 路由器是 App 端按当前借用强度临时构造的视图：
//! - 只读路径用 [`EditorRouter`]，持 `&dyn TextTargetQuery` 列表 ——
//!   做 preedit 查询、current focus profile 等。
//! - 可写路径用 [`EditorRouterMut`]，持 `&mut dyn TextTargetOwner` 列表 ——
//!   做 IME 写入回调；外部通过闭包接收 [`ImeTarget`]，借用在闭包结束后释放。
//!
//! 第二阶段起，业务查询按 App 持有的唯一 [`AppFocus`] 精确查 owner；
//! `target_id` 路径只服务 GPUI IME 回调和旧 slot 身份。
//!
//! 写入端的 `edit_target` 由于要把借用透传到 [`zom_command::CommandContext`]
//! 跨越执行器整个生命周期，不走路由器；App 直接顺序问 owners，路由器只承担
//! "查询 + IME 写入闭包"两类需求。

use std::ops::Range;

use zom_command::{CommandError, KeyContext};

use crate::focus::AppFocus;
use crate::shell::editor::input::{ImeQueryTarget, ImeTarget};
use crate::shell::editor::snapshot::EditorSnapshot;

use super::{TextTargetOwner, TextTargetQuery};

/// 只读路由：current_focus key contexts / snapshot / preedit 等。
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

    /// 指定 AppFocus 的 owner 按键解析上下文栈（优先级从高到低）。
    ///
    /// 路由层不替业务拼栈，只把 owner 给的 `key_contexts()` 透传出来。
    pub(crate) fn key_contexts_for(&self, focus: AppFocus) -> Option<Vec<KeyContext>> {
        self.owners
            .iter()
            .find(|owner| owner.accepts_focus(focus))
            .map(|owner| owner.key_contexts())
    }

    /// 当前活动 target 是否处于"非空 preedit"的输入法组合态。
    ///
    /// 空 preedit（系统输入法取消候选后留下的空壳）不算组合态：留着 IME会再吞下一个按键，表现为"取消候选后要多按一次 Esc 才退出新建"。
    pub(crate) fn is_composing(&self, focus: AppFocus) -> bool {
        self.preedit_text(focus)
            .is_some_and(|preedit| !preedit.is_empty())
    }

    pub(crate) fn snapshot_for_focus(&self, focus: AppFocus) -> EditorSnapshot {
        self.owners
            .iter()
            .find(|owner| owner.accepts_focus(focus))
            .map(|owner| owner.snapshot())
            .unwrap_or_default()
    }

    pub(crate) fn marked_range_utf16(&self, focus: AppFocus) -> Option<Range<usize>> {
        self.with_query(focus, |q| q.marked_range_utf16()).flatten()
    }

    pub(crate) fn selected_range_utf16(&self, focus: AppFocus) -> Option<(Range<usize>, bool)> {
        self.with_query(focus, |q| q.selected_range_utf16())
    }

    pub(crate) fn text_for_range_utf16(
        &self,
        focus: AppFocus,
        range: Range<usize>,
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
        focus: AppFocus,
        f: impl FnOnce(ImeTarget<'_>) -> Result<R, CommandError>,
    ) -> Result<R, CommandError> {
        for owner in self.owners.iter_mut() {
            if owner.accepts_focus(focus) {
                let ime = owner.ime_target().ok_or(CommandError::NoActiveView)?;
                let result = f(ime)?;
                owner.after_text_changed();
                return Ok(result);
            }
        }
        Err(CommandError::NoActiveView)
    }
}
