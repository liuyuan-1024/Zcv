//! 文件监听服务。
//!
//! [`FileWatcherService`] 是纯事件源——把 OS 文件系统事件收集起来，每帧由 App 排空。
//! 本模块不耦合任何消费方。

use std::path::{Path, PathBuf};

use notify::Watcher;

/// 文件系统事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FsEvent {
    pub(crate) path: PathBuf,
    pub(crate) kind: FsEventKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FsEventKind {
    Created,
    Modified,
    Removed,
}

/// 文件监听服务——纯事件源。
///
/// ## 生命周期
///
/// - 项目打开时 [`start`](Self::start)，项目关闭时 drop。
/// - drop 自动解注册 OS watch。
///
/// ## 线程模型
///
/// `notify` 在内部线程回调；回调只做 `tx.send()`（无界 channel，永不阻塞）。
/// 主线程每帧通过 [`drain_events`](Self::drain_events) 排空（非阻塞 `try_iter`）。
pub(crate) struct FileWatcherService {
    /// notify 的 OS watcher。drop 时自动解注册所有监听路径。
    _watcher: notify::RecommendedWatcher,
    /// 事件接收端。无界 channel，notify 回调永不阻塞。
    rx: crossbeam_channel::Receiver<FsEvent>,
}

impl FileWatcherService {
    /// 启动对 `root` 的递归文件监听。
    ///
    /// 使用 OS 原生后端（macOS FSEvents / Linux inotify）。
    /// 返回错误时，调用方应降级为手动刷新。
    pub(crate) fn start(root: &Path) -> Result<Self, notify::Error> {
        let root = root.to_path_buf();
        let (tx, rx) = crossbeam_channel::unbounded();

        let mut watcher = notify::RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                let Ok(event) = res else { return };
                let kind = match event.kind {
                    notify::EventKind::Create(_) => FsEventKind::Created,
                    notify::EventKind::Modify(_) => FsEventKind::Modified,
                    notify::EventKind::Remove(_) => FsEventKind::Removed,
                    _ => return,
                };
                for path in event.paths {
                    let _ = tx.send(FsEvent { path, kind });
                }
            },
            notify::Config::default(),
        )?;

        watcher.watch(&root, notify::RecursiveMode::Recursive)?;

        Ok(Self {
            _watcher: watcher,
            rx,
        })
    }

    /// 排空 OS 事件队列，返回去重后的事件列表。
    ///
    /// 每帧调用一次。非阻塞——没有事件时返回空 Vec。
    pub(crate) fn drain_events(&mut self) -> Vec<FsEvent> {
        let events: Vec<FsEvent> = self.rx.try_iter().collect();
        deduplicate(events)
    }
}

/// 同帧内事件去重合并。
///
/// 规则：
/// - 同路径同类型 → 跳过（重复事件）
/// - Created + Removed → 抵消（临时文件）
/// - Created 后紧跟 Modified → 忽略 Modified（创建已隐含内容写入）
fn deduplicate(mut events: Vec<FsEvent>) -> Vec<FsEvent> {
    if events.len() <= 1 {
        return events;
    }
    events.sort_by(|a, b| a.path.cmp(&b.path));
    let mut out: Vec<FsEvent> = Vec::with_capacity(events.len());
    for e in events {
        match out.last() {
            Some(prev)
                if prev.path == e.path
                    && prev.kind == FsEventKind::Created
                    && e.kind == FsEventKind::Removed =>
            {
                out.pop();
            }
            Some(prev)
                if prev.path == e.path
                    && prev.kind == FsEventKind::Created
                    && e.kind == FsEventKind::Modified => {}
            Some(prev) if prev.path == e.path && prev.kind == e.kind => {}
            _ => out.push(e),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deduplicate_should_merge_duplicate_events() {
        let events = vec![
            FsEvent {
                path: PathBuf::from("a.txt"),
                kind: FsEventKind::Modified,
            },
            FsEvent {
                path: PathBuf::from("a.txt"),
                kind: FsEventKind::Modified,
            },
            FsEvent {
                path: PathBuf::from("b.txt"),
                kind: FsEventKind::Created,
            },
        ];
        assert_eq!(deduplicate(events).len(), 2);
    }

    #[test]
    fn deduplicate_should_cancel_create_then_remove() {
        let events = vec![
            FsEvent {
                path: PathBuf::from("tmp.txt"),
                kind: FsEventKind::Created,
            },
            FsEvent {
                path: PathBuf::from("tmp.txt"),
                kind: FsEventKind::Removed,
            },
        ];
        assert!(deduplicate(events).is_empty());
    }

    #[test]
    fn deduplicate_should_ignore_modify_after_create() {
        let events = vec![
            FsEvent {
                path: PathBuf::from("new.txt"),
                kind: FsEventKind::Created,
            },
            FsEvent {
                path: PathBuf::from("new.txt"),
                kind: FsEventKind::Modified,
            },
        ];
        let result = deduplicate(events);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].kind, FsEventKind::Created);
    }

    #[test]
    fn deduplicate_should_preserve_modify_without_create() {
        let events = vec![
            FsEvent {
                path: PathBuf::from("existing.txt"),
                kind: FsEventKind::Modified,
            },
            FsEvent {
                path: PathBuf::from("existing.txt"),
                kind: FsEventKind::Modified,
            },
        ];
        let result = deduplicate(events);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].kind, FsEventKind::Modified);
    }

    /// 验证 notify 在 macOS FSEvents 下对根目录文件和子目录文件的修改都能产生事件。
    #[test]
    fn watcher_should_detect_modify_in_root_and_subdir() {
        let dir = std::env::temp_dir().join(format!("zom-fw-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();

        let root_file = dir.join("root_file.txt");
        let sub_file = dir.join("sub/sub_file.txt");

        // 先创建文件，让 watcher 在已有文件的基础上检测变化
        std::fs::write(&root_file, b"v1").unwrap();
        std::fs::write(&sub_file, b"v1").unwrap();

        let mut svc = FileWatcherService::start(&dir).expect("启动 watcher 失败");

        // 给 FSEvents 一些初始化时间
        std::thread::sleep(std::time::Duration::from_millis(300));

        // 排空初始化期间累积的事件
        svc.drain_events();

        // --- 用 append 方式修改文件（模拟 vim/外部编辑器的真实行为） ---
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&root_file)
                .unwrap();
            f.write_all(b"more").unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        let root_events: Vec<_> = svc.drain_events();
        let root_touched = !root_events.is_empty();

        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&sub_file)
                .unwrap();
            f.write_all(b"more").unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        let sub_events: Vec<_> = svc.drain_events();
        let sub_touched = !sub_events.is_empty();

        assert!(root_touched, "根目录下的文件修改应被 notify 检测到");
        assert!(sub_touched, "子目录下的文件修改应被 notify 检测到");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
