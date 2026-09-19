//! 搜索能力域：具体搜索视图与产品级装配。
//!
//! `zcv-workspace` 只提供 `SearchableItem` 与 Item 工具区接口；
//! 本 crate 持有搜索状态、交互与项目搜索视图，避免工作区框架依赖具体功能。

mod buffer_search;
mod project_search;
mod search_bar;

#[cfg(test)]
mod test;

pub use search_bar::{SearchBar, SearchBarConfig, SearchBarSlots};

use gpui::{App, AppContext, Context, Window};
use zcv_actions::{DeployBufferSearch, DeployProjectSearch};
use zcv_workspace::Workspace;

use project_search::{
    ProjectSearchButton, ProjectSearchSerializedItemProvider, ProjectSearchToolbar,
    deploy as deploy_project_search_view,
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
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let seed = query_suggestion(workspace, cx);
    deploy_project_search_view(workspace, seed, window, cx);
}

/// 注册搜索能力域的进程级 provider；在 `main` 初始化阶段调用一次。
pub fn init(cx: &mut App) {
    zcv_workspace::register_serialized_item_provider(ProjectSearchSerializedItemProvider, cx);
}

/// 把独立的 Buffer/Project 搜索栏及其 action 路由注入一个 Workspace。
pub fn install(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let pane = workspace.pane().clone();
    let document_toolbar = buffer_search::install(workspace, cx);
    let project_search_toolbar = cx.new(|_| ProjectSearchToolbar::new());
    pane.update(cx, |pane, cx| {
        pane.toolbar().update(cx, |toolbar, cx| {
            toolbar.add_item(document_toolbar.clone(), window, cx);
            toolbar.add_item(project_search_toolbar, window, cx);
        });
    });
    let workspace_handle = cx.weak_entity();
    let status_bar = workspace.status_bar().clone();
    status_bar.update(cx, |status_bar, cx| {
        status_bar.add_left_item(cx.new(|_| ProjectSearchButton::new(workspace_handle)), cx);
    });

    let document_toolbar_for_action = document_toolbar.clone();
    workspace.register_action(move |_workspace, _: &DeployBufferSearch, window, cx| {
        document_toolbar_for_action.update(cx, |toolbar, cx| {
            toolbar.deploy(None, window, cx);
        });
    });

    workspace.register_action(move |workspace, _: &DeployProjectSearch, window, cx| {
        deploy_project_search(workspace, window, cx);
    });
}
