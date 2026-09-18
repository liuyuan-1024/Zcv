//! 终端面板行为测试：新终端的工作目录来自面板所属 Project 的根。

use std::{path::Path, sync::Arc};

use gpui::{AppContext as _, TestAppContext};
use zcv_fs_watch::{FsEventStream, FsWatcher, Watcher};
use zcv_language::LanguageRegistry;
use zcv_project::Project;

use super::project_terminal_cwd;

/// 测试项目不注册真实文件系统监听，避免 OS 监听线程向 GPUI 测试调度器投递事件。
struct PassiveWatcher(FsWatcher);

impl PassiveWatcher {
    fn new() -> Self {
        Self(FsWatcher::new())
    }
}

impl Watcher for PassiveWatcher {
    fn add(&self, _path: &Path) -> anyhow::Result<()> {
        Ok(())
    }

    fn remove(&self, _path: &Path) -> anyhow::Result<()> {
        Ok(())
    }

    fn events(&self) -> FsEventStream {
        self.0.events()
    }
}

#[gpui::test]
fn new_terminal_uses_the_project_root_as_working_directory(cx: &mut TestAppContext) {
    let temporary_directory = tempfile::tempdir().expect("应创建临时项目目录");
    let expected_root = temporary_directory
        .path()
        .canonicalize()
        .expect("临时项目根目录应可规范化");
    let project = cx.new(|cx| {
        Project::new_with_watcher(
            temporary_directory.path().to_path_buf(),
            Arc::new(PassiveWatcher::new()),
            Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    let cwd = cx.read_entity(&project, |project, _| project_terminal_cwd(project));
    assert_eq!(
        cwd.as_deref(),
        Some(expected_root.as_path()),
        "新终端的工作目录应为所属项目的根"
    );
}
