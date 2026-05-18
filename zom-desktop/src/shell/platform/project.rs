//! 本地项目选择对话框。
//!
//! 文件夹选择属于平台层职责；选择结果如何进入 workspace / layout 由
//! `shell::view` 和 `app` 组合根处理。

use std::path::PathBuf;

use gpui::{App, PathPromptOptions, SharedString};

pub(crate) fn prompt_for_local_project(
    cx: &mut App,
) -> impl std::future::Future<Output = Option<PathBuf>> + use<> {
    let receiver = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some(SharedString::from("打开本地项目")),
    });

    async move {
        match receiver.await {
            Ok(Ok(Some(paths))) => paths.into_iter().next(),
            Ok(Ok(None)) => None,
            Ok(Err(error)) => {
                eprintln!("打开本地项目失败：{error}");
                None
            }
            Err(_) => {
                eprintln!("打开本地项目失败：文件选择器提前关闭。");
                None
            }
        }
    }
}
