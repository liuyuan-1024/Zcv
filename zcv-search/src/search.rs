//! 搜索能力域：统一搜索栏与具体搜索目标的产品级装配。
//!
//! `zcv-workspace` 只提供 `SearchableItem` 和 Toolbar 扩展协议；本 crate 持有搜索状态、交互与项目搜索视图，避免工作区框架依赖具体功能。

mod buffer_search;
mod project_search;
mod search_bar;

#[cfg(test)]
mod test;

use gpui::{App, Context, Window};
use zcv_actions::{Deploy as DeployProjectSearch, DeploySearch};
use zcv_workspace::Workspace;

pub use project_search::ProjectSearchButton;

/// 部署项目搜索时从活动 Item 取查询建议；
/// 必须在打开搜索视图前调用（deploy 会切换活动 Item）。
fn query_suggestion(workspace: &Workspace, cx: &App) -> Option<String> {
    workspace
        .pane()
        .read(cx)
        .active_item()
        .and_then(|item| item.as_searchable(cx))
        .and_then(|item| item.query_suggestion(cx))
}

/// 把独立的 Buffer/Project 搜索栏及其 action 路由注入一个 Workspace。
pub fn install(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let buffer_search_bar = buffer_search::install(workspace, window, cx);
    let project_search_bar = project_search::install_search_bar(workspace, window, cx);

    let buffer_bar_for_action = buffer_search_bar.clone();
    let project_bar_for_action = project_search_bar.clone();
    workspace.register_action(move |workspace, _: &DeploySearch, window, cx| {
        let is_project_search = workspace
            .pane()
            .read(cx)
            .active_item()
            .is_some_and(|item| project_search::is_project_search_item(item, cx));
        // 无种子：搜索条自行向活动 Item 请求查询建议。
        if is_project_search {
            project_bar_for_action.update(cx, |search_bar, cx| {
                search_bar.deploy(None, window, cx);
            });
        } else {
            buffer_bar_for_action.update(cx, |search_bar, cx| {
                search_bar.deploy(None, window, cx);
            });
        }
    });

    workspace.register_action(move |workspace, _: &DeployProjectSearch, window, cx| {
        let seed = query_suggestion(workspace, cx);
        project_search::deploy(workspace, window, cx);
        project_search_bar.update(cx, |search_bar, cx| search_bar.deploy(seed, window, cx));
    });
}
