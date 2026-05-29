//! 搜索协调：把 panel 状态 + 活动 buffer 的 `BufferSearch` + 活动 view 的
//! selection 拧成一束。
//!
//! 这一层不存任何状态——所有可变状态分属 [`SearchModel`]（panel 输入）、
//! [`Workspace`]（buffer 内的 `BufferSearch`）、[`ViewSet`]（selection /
//! viewport）。函数只承担"把这三家拉到一起"的流程，所以全是 free function。
//!
//! 入口分两类：
//! - **HostEffect 落地**：`on_panel_opened` / `on_panel_closed` /
//!   `toggle_option` / `find_*` / `replace_*` —— 由 [`super::effects`] 在
//!   面板按钮 / 快捷键触发时调。
//! - **派发尾部 / IME 同步**：`sync_active_buffer_search` —— 由 `App` 在
//!   每次命令派发尾部 + IME preedit 更新时调，确保 panel 文本一改就立刻
//!   在 buffer 里重算命中。

use zom_command::SearchOption;
use zom_engine::{Selection, SelectionSet, TextRange};
use zom_view::{RevealKind, ViewSet};
use zom_workspace::Workspace;

use super::model::{HitCount, SearchModel};

/// 面板刚被打开（mod-f 从隐藏切到显示）。立刻 sync 一次把当前 query 推进
/// 活动 buffer 并 reveal 第一项命中——用户期待的"打开搜索就看到结果"。
pub(crate) fn on_panel_opened(
    search: &mut SearchModel,
    workspace: &mut Workspace,
    views: &mut ViewSet,
) {
    search.set_panel_open(true);
    sync_active_buffer_search(search, workspace, views);
}

/// 面板被关闭：清掉活动 buffer 的 BufferSearch 高亮，同时把 panel_open 设
/// 回 false 让后续 dispatch tail 的 sync 跳过——不然下一个编辑器按键会触
/// 发同步把高亮复活。
///
/// panel 上的 query/replacement 文本本身**不动**：再开面板时仍能看到上次
/// 输入，按 Enter 即可重新搜。
pub(crate) fn on_panel_closed(search: &mut SearchModel, workspace: &mut Workspace) {
    search.set_panel_open(false);
    if let Some(wb) = workspace.active_buffer_mut() {
        // 把 query 置空 → BufferSearch.slot 被清空 → ranges() 空 →
        // EditorView 阶段 2 没东西画。下次面板再开会重新 set_query。
        wb.search_mut().set_query(String::new());
    }
}

pub(crate) fn toggle_option(
    search: &mut SearchModel,
    workspace: &mut Workspace,
    views: &mut ViewSet,
    option: SearchOption,
) {
    search.toggle_option(option);
    // 选项变化要立刻同步到活动 buffer 的 BufferSearch，否则下一次 find_next
    // 或渲染会读到不一致的命中（旧选项算出来的）。
    sync_active_buffer_search(search, workspace, views);
}

pub(crate) fn find_next(search: &mut SearchModel, workspace: &mut Workspace, views: &mut ViewSet) {
    sync_active_buffer_search(search, workspace, views);
    let Some(wb) = workspace.active_buffer_mut() else {
        return;
    };
    if let Some(range) = wb.search_mut().advance() {
        move_selection_to_match(views, range);
    }
}

pub(crate) fn find_previous(
    search: &mut SearchModel,
    workspace: &mut Workspace,
    views: &mut ViewSet,
) {
    sync_active_buffer_search(search, workspace, views);
    let Some(wb) = workspace.active_buffer_mut() else {
        return;
    };
    if let Some(range) = wb.search_mut().retreat() {
        move_selection_to_match(views, range);
    }
}

pub(crate) fn replace_next(
    search: &mut SearchModel,
    workspace: &mut Workspace,
    views: &mut ViewSet,
) {
    sync_active_buffer_search(search, workspace, views);
    let replacement = search.replacement_text();
    let Some(wb) = workspace.active_buffer_mut() else {
        return;
    };
    // replace_current_search_match 自动 pump_search → 余下命中 try_remap。
    // 被替换那条会被 try_remap 丢掉，BufferSearch.current_hit 自然指向其
    // 替换后位置的下一个命中（or None 如果是最后一条）。
    let _ = wb.replace_current_search_match(&replacement);
    if let Some(range) = wb.search().current_range() {
        move_selection_to_match(views, range);
    }
}

pub(crate) fn replace_all(
    search: &mut SearchModel,
    workspace: &mut Workspace,
    views: &mut ViewSet,
) {
    sync_active_buffer_search(search, workspace, views);
    let replacement = search.replacement_text();
    let Some(wb) = workspace.active_buffer_mut() else {
        return;
    };
    let _ = wb.replace_all_search_matches(&replacement);
    // 全替换后命中通常全部被 remap 吃掉；若 BufferSearch 内还残留 current_hit
    // 把光标挪过去，没有就让光标停在原位。
    if let Some(range) = wb.search().current_range() {
        move_selection_to_match(views, range);
    }
}

/// 把 panel 的 query / options 推进**活动 buffer** 的 BufferSearch，并
/// `sync` 一次，确保后续 find/replace handler 读到的命中是最新的。
///
/// 调用点：命令派发尾部、IME preedit 更新、以及 panel 自己各 handler 的
/// 前置位置——一处做完，渲染 / 后续命令读到的都是新真值。
///
/// 面板没在屏上时整条早退——避免编辑器普通按键的 dispatch tail 把上一轮
/// 搜索的高亮复活（参见 [`on_panel_closed`]）。query/options 在本次调用
/// 中确有变化时，自动把选区+视口 reveal 到第一项命中（VS Code 风格：边
/// 输入边定位首条）。
pub(crate) fn sync_active_buffer_search(
    search: &mut SearchModel,
    workspace: &mut Workspace,
    views: &mut ViewSet,
) {
    if !search.panel_open() {
        return;
    }
    let query = search.query_text();
    let options = search.buffer_search_options();
    let Some(wb) = workspace.active_buffer_mut() else {
        return;
    };
    let query_changed = wb.search_mut().set_query(query);
    let options_changed = wb.search_mut().set_options(options);
    // 先把 buffer 上累积的 DeltaEvent（任何编辑路径都可能产生）喂回
    // BufferSearch 再 sync：sync 检测版本时优先复用 try_remap 的结果，没
    // 有时才重跑。
    let _ = wb.pump_search();
    let _ = wb.sync_search();

    // query / options 这一次真的变了 → 让选区落到新结果集的第一项并
    // reveal。`normalize_current_hit_after_rerun` 已经把 current_hit 摆
    // 到 0，所以 current_range 即第一条。
    //
    // 没变化时不触发：避免覆盖 find_next / replace 等命令自己刚 advance
    // 出来的 current hit——它们走 dispatch 尾部时会再次进本函数，但
    // query/options 不变就不会被反向拉回首条。
    if query_changed || options_changed {
        if let Some(range) = wb.search().current_range() {
            move_selection_to_match(views, range);
        }
    }
}

/// 给 panel UI 渲染填 "3 / 27" 标签——从活动 buffer 的 BufferSearch 读
/// `(current_hit_ordinal, hit_count)`。无活动 buffer 或无命中时返回 None。
pub(crate) fn current_hit_count(workspace: &Workspace) -> Option<HitCount> {
    let wb = workspace.active_buffer()?;
    let bs = wb.search();
    let current = bs.current_hit_ordinal()?;
    Some(HitCount {
        current,
        total: bs.hit_count(),
    })
}

/// 把活动 view 的 selection 挪到 search 命中的 range，并 reveal。
fn move_selection_to_match(views: &mut ViewSet, range: TextRange) {
    let Some(view) = views.active_view_mut() else {
        return;
    };
    *view.selection_mut() = SelectionSet::new(vec![Selection::new(range.start(), range.end())]);
    // `RevealKind::Match`：命中已在视区时不滚动；不在则按 1/3 摆位。
    view.request_reveal(range.start(), RevealKind::Match);
}
