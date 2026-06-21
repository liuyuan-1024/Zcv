//! 搜索协调：把搜索栏输入 + 活动 buffer 的 `BufferSearch` + 活动 view 的 selection 拧成一束。
//!
//! 这一层不存任何状态——所有可变状态分属 [`SearchModel`]（bar 输入）、[`Workspace`]（buffer 内的 `BufferSearch`）、[`ViewSet`]（selection / viewport）。
//! 函数只承担"把这三家拉到一起"的流程，所以全是 free function。
//!
//! 入口分两类：
//! - **HostEffect 落地**：`on_opened` / `on_closed` /
//!   `toggle_option` / `find_*` / `replace_*` —— 由 [`super::effects`] 在 bar 按钮 / 快捷键触发时调。
//! - **派发尾部 / IME 同步**：`sync_active_buffer_search` —— 由 `App` 在每次命令派发尾部 + IME preedit 更新时调，确保 bar 文本一改就立刻在 buffer 里重算命中。

use zom_command::SearchOption;
use zom_engine::{Selection, SelectionSet, TextRange};
use zom_workspace::view::{RevealKind, ViewId, ViewSet};
use zom_workspace::{BufferId, SearchSyncOutcome, Workspace, WorkspaceBuffer};

use super::model::{HitCount, SearchModel};

/// 从活动 view 投影到背后的 buffer——搜索协调里所有对"活动 buffer"的访问都先经此一跳。
/// 预览 view 也可以参与搜索；调用方按需进一步筛选。
fn active_buffer_id(views: &ViewSet, active_view_id: Option<ViewId>) -> Option<BufferId> {
    let view_id = active_view_id?;
    views.view(view_id).map(zom_workspace::view::View::buffer)
}

fn active_buffer<'a>(
    workspace: &'a Workspace,
    views: &ViewSet,
    active_view_id: Option<ViewId>,
) -> Option<&'a WorkspaceBuffer> {
    workspace.buffer(active_buffer_id(views, active_view_id)?)
}

fn active_buffer_mut<'a>(
    workspace: &'a mut Workspace,
    views: &ViewSet,
    active_view_id: Option<ViewId>,
) -> Option<&'a mut WorkspaceBuffer> {
    workspace.buffer_mut(active_buffer_id(views, active_view_id)?)
}

/// 搜索栏刚被打开（从隐藏切到显示）。
/// 立刻 sync 一次把当前 query 推进活动 buffer 并 reveal 第一项命中——用户期待的"打开搜索就看到结果"。
pub(crate) fn on_opened(
    search: &mut SearchModel,
    workspace: &mut Workspace,
    views: &mut ViewSet,
    active_view_id: Option<ViewId>,
) {
    search.set_open(true);
    sync_active_buffer_search(search, workspace, views, active_view_id);
}

/// 搜索栏被关闭：清掉活动 buffer 的 BufferSearch 高亮，
/// 同时把 open 设回 false 让后续 dispatch tail 的 sync 跳过——不然下一个编辑器按键会触发同步把高亮复活。
///
/// bar 上的 query/replacement 文本本身**不动**：再开 bar 时仍能看到上次输入，按 Enter 即可重新搜。
pub(crate) fn on_closed(
    search: &mut SearchModel,
    workspace: &mut Workspace,
    views: &ViewSet,
    active_view_id: Option<ViewId>,
) {
    search.set_open(false);
    if let Some(wb) = active_buffer_mut(workspace, views, active_view_id) {
        // 把 query 置空 → BufferSearch.slot 被清空 → ranges() 空 → EditorView 阶段 2 没东西画。
        // 下次 bar 再开会重新 set_query。
        wb.search_mut().set_query(String::new());
    }
}

pub(crate) fn toggle_option(
    search: &mut SearchModel,
    workspace: &mut Workspace,
    views: &mut ViewSet,
    active_view_id: Option<ViewId>,
    option: SearchOption,
) {
    search.toggle_option(option);
    // 选项变化要立刻同步到活动 buffer 的 BufferSearch。
    // 否则下一次 find_next 或渲染会读到不一致的命中（旧选项算出来的）。
    sync_active_buffer_search(search, workspace, views, active_view_id);
}

pub(crate) fn find_next(
    search: &mut SearchModel,
    workspace: &mut Workspace,
    views: &mut ViewSet,
    active_view_id: Option<ViewId>,
) {
    sync_active_buffer_search(search, workspace, views, active_view_id);
    let Some(wb) = active_buffer_mut(workspace, views, active_view_id) else {
        return;
    };
    if let Some(range) = wb.search_mut().advance() {
        move_selection_to_match(views, active_view_id, range);
    }
}

pub(crate) fn find_previous(
    search: &mut SearchModel,
    workspace: &mut Workspace,
    views: &mut ViewSet,
    active_view_id: Option<ViewId>,
) {
    sync_active_buffer_search(search, workspace, views, active_view_id);
    let Some(wb) = active_buffer_mut(workspace, views, active_view_id) else {
        return;
    };
    if let Some(range) = wb.search_mut().retreat() {
        move_selection_to_match(views, active_view_id, range);
    }
}

pub(crate) fn replace_next(
    search: &mut SearchModel,
    workspace: &mut Workspace,
    views: &mut ViewSet,
    active_view_id: Option<ViewId>,
) {
    sync_active_buffer_search(search, workspace, views, active_view_id);
    let replacement = search.replacement_text();
    let Some(wb) = active_buffer_mut(workspace, views, active_view_id) else {
        return;
    };
    // replace_current_search_match 自动 pump_post_edit → 余下命中 try_remap。
    // 被替换那条会被 try_remap 丢掉，BufferSearch.current_hit 自然指向其替换后位置的下一个命中（or None 如果是最后一条）。
    let _ = wb.replace_current_search_match(&replacement);
    if let Some(range) = wb.search().current_range() {
        move_selection_to_match(views, active_view_id, range);
    }
}

/// Enter 退出搜索：把活动 view 的 selection 折叠到当前命中末尾，方便用户接着在匹配尾部继续编辑。
/// 如果没有活动 buffer / 没有 current_range，什么也不做。
///
/// 与 `move_selection_to_match` 不同：那个把选区设为整条命中（start..end），而这里要的是一个 caret（end..end）。
pub(crate) fn confirm_match(
    workspace: &mut Workspace,
    views: &mut ViewSet,
    active_view_id: Option<ViewId>,
) {
    let Some(wb) = active_buffer(workspace, views, active_view_id) else {
        return;
    };
    let Some(range) = wb.search().current_range() else {
        return;
    };
    let Some(view) = active_view_id.and_then(|id| views.edit_view_mut(id)) else {
        return;
    };
    let end = range.end();
    *view.selection_mut() = SelectionSet::new(vec![Selection::new(end, end)]);
    view.request_reveal(end, RevealKind::Match);
}

pub(crate) fn replace_all(
    search: &mut SearchModel,
    workspace: &mut Workspace,
    views: &mut ViewSet,
    active_view_id: Option<ViewId>,
) {
    sync_active_buffer_search(search, workspace, views, active_view_id);
    let replacement = search.replacement_text();
    let Some(wb) = active_buffer_mut(workspace, views, active_view_id) else {
        return;
    };
    let _ = wb.replace_all_search_matches(&replacement);
    // 全替换后命中通常全部被 remap 吃掉。
    // 若 BufferSearch 内还残留 current_hit 把光标挪过去，没有就让光标停在原位。
    if let Some(range) = wb.search().current_range() {
        move_selection_to_match(views, active_view_id, range);
    }
}

/// 把搜索栏的 query / options 推进**活动 buffer** 的 BufferSearch，并 `sync` 一次，确保后续 find/replace handler 读到的命中是最新的。
///
/// 调用点：命令派发尾部、IME preedit 更新、以及 bar 自己各 handler 的前置位置——一处做完，渲染 / 后续命令读到的都是新真值。
///
/// bar 没在屏上时整条早退——避免编辑器普通按键的 dispatch tail 把上一轮搜索的高亮复活（参见 [`on_closed`]）。
///
/// ## 异步 sync + 延迟 reveal
///
/// `sync_search` 现在是非阻塞的：发现 query/options 变了，立刻 spawn 后台搜索，但不一定**当帧**就能拿到结果。
/// reveal 触发条件：
/// - 本帧 `sync_search` 直接返回 `JustReady`（小 buffer / 命中很快算完），OR
/// - 本次调用确实改了 query/options 且 reveal 不能在 spawn 时立刻发生——交给下次 `pump_pending_search` 收割时再补 reveal（依赖[`super::super::super::super::super::app::App::pump_pending_search`]）。
///
/// 这里只处理"当帧 JustReady"的情况；延迟 reveal 走 pump 路径。
pub(crate) fn sync_active_buffer_search(
    search: &mut SearchModel,
    workspace: &mut Workspace,
    views: &mut ViewSet,
    active_view_id: Option<ViewId>,
) {
    if !search.is_open() {
        return;
    }
    let query = search.query_text();
    let options = search.buffer_search_options();
    let Some(wb) = active_buffer_mut(workspace, views, active_view_id) else {
        return;
    };
    let _query_changed = wb.search_mut().set_query(query);
    let _options_changed = wb.search_mut().set_options(options);
    // 先把 buffer 上累积的 DeltaEvent 喂回 BufferSearch 再 sync。
    // sync 检测版本时优先复用 try_remap 的结果，没有时才 spawn。
    let _ = wb.pump_post_edit();
    let outcome = wb.sync_search().unwrap_or(SearchSyncOutcome::Idle);

    // 真的拿到了新结果 → reveal 首条命中。两种触发场景：
    // 1. query/options 刚变，对应 spawn 在本帧立即完成（小 buffer）；
    // 2. 上次留下的 pending 在本帧收割完。
    if matches!(outcome, SearchSyncOutcome::JustReady)
        && let Some(range) = wb.search().current_range()
    {
        move_selection_to_match(views, active_view_id, range);
    }
}

/// 渲染线程每帧调一次：收割已就绪的后台搜索结果，必要时 reveal 首条命中。
/// 与 `sync_active_buffer_search` 的区别——本函数**不**主动 spawn，只检查 in-flight pending 是否完成。
pub(crate) fn pump_active_buffer_search(
    workspace: &mut Workspace,
    views: &mut ViewSet,
    active_view_id: Option<ViewId>,
) {
    let Some(wb) = active_buffer_mut(workspace, views, active_view_id) else {
        return;
    };
    if matches!(wb.pump_pending_search(), SearchSyncOutcome::JustReady)
        && let Some(range) = wb.search().current_range()
    {
        move_selection_to_match(views, active_view_id, range);
    }
}

/// 给 panel UI 渲染填 "3 / 27" 标签——从活动 buffer 的 BufferSearch 读 `(current_hit_ordinal, hit_count)`。
/// 无活动 buffer 或无命中时返回 None。
pub(crate) fn current_hit_count(
    workspace: &Workspace,
    views: &ViewSet,
    active_view_id: Option<ViewId>,
) -> Option<HitCount> {
    let wb = active_buffer(workspace, views, active_view_id)?;
    let bs = wb.search();
    let current = bs.current_hit_ordinal()?;
    Some(HitCount {
        current,
        total: bs.hit_count(),
    })
}

/// 把活动 view 的 selection 挪到 search 命中的 range，并 reveal。
fn move_selection_to_match(views: &mut ViewSet, active_view_id: Option<ViewId>, range: TextRange) {
    let Some(view) = active_view_id.and_then(|id| views.edit_view_mut(id)) else {
        return;
    };
    *view.selection_mut() = SelectionSet::new(vec![Selection::new(range.start(), range.end())]);
    // `RevealKind::Match`：命中已在视区时不滚动；不在则按 1/3 摆位。
    view.request_reveal(range.start(), RevealKind::Match);
}
