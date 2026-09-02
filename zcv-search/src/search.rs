//! 搜索能力域：统一搜索栏与具体搜索目标的产品级装配。
//!
//! `zcv-workspace` 只提供 `SearchableItem` 和 Toolbar 扩展协议；本 crate 持有搜索状态、交互与项目搜索视图，避免工作区框架依赖具体功能。

mod buffer_search;
mod project_search;
mod search_bar;

#[cfg(test)]
mod test;

use gpui::{App, AppContext, Context, Entity, Window};
use zcv_actions::{Deploy as DeployProjectSearch, DeploySearch};
use zcv_workspace::Workspace;

use project_search::{
    ProjectSearchBar, ProjectSearchButton, deploy as deploy_project_search_view,
    install_search_bar, is_project_search_item,
};

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

pub(crate) fn deploy_project_search(
    workspace: &mut Workspace,
    search_bar: &Entity<ProjectSearchBar>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let seed = query_suggestion(workspace, cx);
    deploy_project_search_view(workspace, window, cx);
    search_bar.update(cx, |search_bar, cx| search_bar.deploy(seed, window, cx));
}

/// 把独立的 Buffer/Project 搜索栏及其 action 路由注入一个 Workspace。
pub fn install(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let buffer_search_bar = buffer_search::install(workspace, window, cx);
    let project_search_bar = install_search_bar(workspace, window, cx);
    let workspace_handle = cx.weak_entity();
    let status_bar = workspace.status_bar().clone();
    status_bar.update(cx, |status_bar, cx| {
        status_bar.add_left_item(
            cx.new(|_| ProjectSearchButton::new(workspace_handle, project_search_bar.clone())),
            cx,
        );
    });

    let buffer_bar_for_action = buffer_search_bar.clone();
    let project_bar_for_action = project_search_bar.clone();
    workspace.register_action(move |workspace, _: &DeploySearch, window, cx| {
        let is_project_search = workspace
            .pane()
            .read(cx)
            .active_item()
            .is_some_and(|item| is_project_search_item(item, cx));
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
        deploy_project_search(workspace, &project_search_bar, window, cx);
    });
}
