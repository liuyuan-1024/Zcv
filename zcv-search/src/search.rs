//! 搜索能力域：统一搜索栏与具体搜索目标的产品级装配。
//!
//! `zcv-workspace` 只提供 `SearchableItem` 和 Toolbar 扩展协议；本 crate 持有搜索状态、交互与项目搜索视图，避免工作区框架依赖具体功能。

mod buffer_search;
mod project_search;
mod search_bar;

use gpui::{Context, Window};
use zcv_actions::{Deploy as DeployProjectSearch, DeploySearch};
use zcv_workspace::Workspace;

pub use project_search::ProjectSearchButton;

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
        if is_project_search {
            project_bar_for_action.update(cx, |search_bar, cx| {
                search_bar.deploy(window, cx);
            });
        } else {
            buffer_bar_for_action.update(cx, |search_bar, cx| {
                search_bar.deploy(window, cx);
            });
        }
    });

    workspace.register_action(move |workspace, _: &DeployProjectSearch, window, cx| {
        project_search::deploy(workspace, window, cx);
        project_search_bar.update(cx, |search_bar, cx| search_bar.deploy(window, cx));
    });
}
