//! `Zcv` 二进制测试共享的测试替身。
//!
//! 确定性测试不应启动真实的 OS 监听线程：它会把事件唤醒调度进 gpui 测试执行器，
//! 触发「测试不确定」断言。这里提供静默监听器，供需要装配 Workspace 的测试注入。

use std::{path::Path, sync::Arc, time::Duration};

use zcv_fs_watch::{FsEventStream, Watcher};

use crate::workspace::FileWatcherFactory;

/// 不注册真实监听、也不产生事件的监听器。
struct QuietWatcher;

impl Watcher for QuietWatcher {
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

/// 返回一个静默监听器实例。
pub(crate) fn quiet_watcher() -> Arc<dyn Watcher> {
    Arc::new(QuietWatcher)
}

/// 每次都返回静默监听器的工厂，用于走完整工作区装配路径的测试。
pub(crate) fn quiet_watcher_factory() -> FileWatcherFactory {
    Arc::new(|| quiet_watcher())
}
