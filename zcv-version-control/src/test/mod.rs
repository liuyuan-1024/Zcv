//! 版本控制视图测试共用的无事件项目装配。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{AppContext as _, Entity, TestAppContext};
use zcv_fs_watch::{FsEventStream, Watcher};
use zcv_language::LanguageRegistry;
use zcv_project::Project;

struct PassiveWatcher;

impl Watcher for PassiveWatcher {
    fn add(&self, _path: &Path) -> anyhow::Result<()> {
        Ok(())
    }

    fn remove(&self, _path: &Path) -> anyhow::Result<()> {
        Ok(())
    }

    fn watch(&self, _latency: Duration) -> FsEventStream {
        Box::pin(futures::stream::pending())
    }
}

pub(crate) fn test_project(root: PathBuf, cx: &mut TestAppContext) -> Entity<Project> {
    cx.new(|cx| {
        Project::new_with_watcher(
            root,
            Arc::new(PassiveWatcher),
            Arc::new(LanguageRegistry::new()),
            cx,
        )
        .expect("测试项目根目录应可规范化")
    })
}
