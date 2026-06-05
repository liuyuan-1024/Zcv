//! 本地项目选择对话框。
//!
//! 文件夹选择属于平台层职责；选择结果如何进入 workspace / layout 由
//! `shell::view` 和 `app` 组合根处理。

use std::path::PathBuf;

use gpui::{App, PathPromptOptions};

/// 用户选择目录的结果。
/// - `Ok(Some)` 选了一个路径；
/// - `Ok(None)` 用户主动取消（不应弹气泡）；
/// - `Err` 选择器自身失败（应弹气泡告知用户）。
pub(crate) type DirectorySelection = Result<Option<PathBuf>, String>;

pub(crate) fn prompt_for_local_project(
    cx: &mut App,
) -> impl std::future::Future<Output = DirectorySelection> + use<> {
    prompt_for_directory(cx, "打开本地项目")
}

pub(crate) fn prompt_for_clone_parent(
    cx: &mut App,
) -> impl std::future::Future<Output = DirectorySelection> + use<> {
    prompt_for_directory(cx, "选择克隆位置")
}

fn prompt_for_directory(
    cx: &mut App,
    error_context: &'static str,
) -> impl std::future::Future<Output = DirectorySelection> + use<> {
    let receiver = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: None,
    });

    async move {
        match receiver.await {
            Ok(Ok(Some(paths))) => Ok(paths.into_iter().next()),
            Ok(Ok(None)) => Ok(None),
            Ok(Err(error)) => Err(format!("{error_context}失败：{error}")),
            Err(_) => Err(format!("{error_context}失败：文件选择器提前关闭。")),
        }
    }
}
