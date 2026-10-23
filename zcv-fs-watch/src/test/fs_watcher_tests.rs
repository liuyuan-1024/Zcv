use super::*;
use std::{fs, thread};

/// 测试语义只关心路径层级；
/// 将简写路径锚定到当前目录，避免把 Unix 根路径误当作 Windows 上的绝对路径。
fn test_absolute_path(path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        let relative = path
            .to_string_lossy()
            .trim_start_matches(&['/', '\\'][..])
            .to_owned();
        std::env::current_dir()
            .expect("测试应能取得当前目录")
            .join(relative)
    }
}

fn rescan(path: &str) -> PathEvent {
    PathEvent {
        path: absolute_event_path(test_absolute_path(path)),
        kind: Some(PathEventKind::Rescan),
    }
}

fn changed(path: &str) -> PathEvent {
    PathEvent {
        path: absolute_event_path(test_absolute_path(path)),
        kind: Some(PathEventKind::Changed),
    }
}

#[test]
fn test_watch_key_exact_vs_folded() {
    let mixed = Path::new("/Repo/Proj");
    let lower = Path::new("/repo/proj");

    // Folded 键忽略大小写
    assert_eq!(WatchKey::folded(mixed), WatchKey::folded(lower));
    // Exact 键区分大小写
    assert_ne!(WatchKey::exact(mixed), WatchKey::exact(lower));
    // Exact 和 Folded 是不同的键空间
    assert_ne!(WatchKey::exact(mixed), WatchKey::folded(mixed));
}

#[test]
fn path_event_rejects_relative_paths() {
    assert!(PathEvent::new(PathBuf::from("relative/file.txt"), None).is_err());
}

#[test]
fn test_folded_path_preserves_component_boundaries() {
    assert_eq!(
        WatchKey::folded(Path::new("/Repo/Proj")),
        WatchKey::folded(Path::new("/repo/proj"))
    );
    assert_ne!(
        WatchKey::folded(Path::new("/repo/proj")),
        WatchKey::folded(Path::new("/repo/project"))
    );
    assert_ne!(
        WatchKey::folded(Path::new("/repo/proj")),
        WatchKey::folded(Path::new("/repo/proj-child"))
    );
}

#[test]
fn test_case_insensitive_event_path_uses_watched_root_spelling() {
    let root = Path::new("/Repo/Proj");
    let event_path = Path::new("/repo/proj/src/Main.rs");

    assert_eq!(
        path_relative_to_root(event_path, root, true),
        Some(PathBuf::from("/Repo/Proj/src/Main.rs"))
    );
    assert_eq!(path_relative_to_root(event_path, root, false), None);
}

#[test]
fn rescan_event_keeps_affected_subtree_and_pathless_rescans_cover_root() {
    let root = test_absolute_path("/watch-root");
    let subtree = root.join("nested");
    let (signal_tx, _) = async_channel::unbounded();
    let pending = Arc::new(Mutex::new(Vec::new()));

    let scoped = notify::Event::new(EventKind::Other)
        .add_path(subtree.clone())
        .set_flag(notify::event::Flag::Rescan);
    push_notify_event(&signal_tx, &pending, &root, false, &scoped);
    assert_eq!(
        std::mem::take(&mut *pending.lock().unwrap()),
        vec![rescan("/watch-root/nested")]
    );

    let pathless = notify::Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan);
    push_notify_event(&signal_tx, &pending, &root, false, &pathless);
    assert_eq!(
        std::mem::take(&mut *pending.lock().unwrap()),
        vec![rescan("/watch-root")]
    );
}

#[test]
fn test_coalesce_rescans() {
    // 子路径 Rescan 被 pending 中的祖先覆盖
    let mut pending = vec![rescan("/root")];
    let mut events = vec![rescan("/root/child"), rescan("/root/child/grandchild")];
    coalesce_pending_rescans(&mut pending, &mut events);
    assert_eq!(pending, vec![rescan("/root")]);
    assert!(events.is_empty());

    // 新祖先 Rescan 替换 pending 中的子 Rescan
    let mut pending = vec![changed("/other"), rescan("/root/child")];
    let mut events = vec![rescan("/root")];
    coalesce_pending_rescans(&mut pending, &mut events);
    assert_eq!(pending, vec![changed("/other")]);
    assert_eq!(events, vec![rescan("/root")]);
}

#[test]
fn test_ancestor_rescan_replaces_descendant_in_batch() {
    // 同一 batch 内先后出现的 Rescan，祖先应覆盖子路径
    let mut pending = vec![];
    let mut events = vec![rescan("/root/child"), rescan("/root")];
    coalesce_pending_rescans(&mut pending, &mut events);
    assert_eq!(events, vec![rescan("/root")]);
}

#[test]
fn test_unrelated_rescans_are_preserved() {
    let mut pending = vec![rescan("/root-a")];
    let mut events = vec![rescan("/root-b")];
    coalesce_pending_rescans(&mut pending, &mut events);
    assert_eq!(pending, vec![rescan("/root-a")]);
    assert_eq!(events, vec![rescan("/root-b")]);
}

#[test]
fn test_extend_sorted_dedup() {
    let mut dst = vec![changed("/a"), changed("/c")];
    let new = vec![changed("/b"), changed("/c")];
    extend_sorted(&mut dst, new);
    assert_eq!(dst, vec![changed("/a"), changed("/b"), changed("/c")]);
}

#[test]
fn test_extend_sorted_keeps_entries_after_replaced_path() {
    let mut dst = vec![changed("/a"), changed("/b"), changed("/c")];
    let new = vec![changed("/b")];
    extend_sorted(&mut dst, new);
    assert_eq!(dst, vec![changed("/a"), changed("/b"), changed("/c")]);
}

#[test]
fn test_path_semantics_can_be_queried_for_existing_path() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("ZcvCaseProbe");
    fs::create_dir(&first).unwrap();
    let second = temp.path().join("zcvcaseprobe");
    let expected_case_sensitive = match fs::create_dir(&second) {
        Ok(()) => {
            fs::remove_dir(&second).unwrap();
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(error) => panic!("创建大小写探测目录失败：{error}"),
    };
    fs::remove_dir(&first).unwrap();

    let semantics = platform::path_semantics(temp.path()).unwrap();
    assert_eq!(semantics.case_sensitive, expected_case_sensitive);
}

#[test]
fn test_fs_watcher_basic_lifecycle() {
    let watcher = FsWatcher::new();
    let temp = tempfile::tempdir().unwrap();

    // add/remove 一个存在的目录
    assert!(watcher.add(temp.path()).is_ok());
    assert!(watcher.remove(temp.path()).is_ok());
}

#[test]
fn child_registration_survives_removal_of_parent_in_same_watcher() {
    let temp = tempfile::tempdir().unwrap();
    let child = temp.path().join("child");
    fs::create_dir(&child).unwrap();
    let watcher = FsWatcher::new();

    watcher.add(temp.path()).unwrap();
    watcher.add(&child).unwrap();
    assert_eq!(watcher.registrations.lock().unwrap().len(), 2);
    watcher.remove(temp.path()).unwrap();
    assert_eq!(watcher.registrations.lock().unwrap().len(), 1);

    let canonical_parent = canonical_watch_path(temp.path());
    let semantics = platform::path_semantics(&canonical_parent).unwrap();
    let owner_key = WatchKey::for_path(&canonical_parent, !semantics.case_sensitive);
    let mut state = global_watcher().state.lock().unwrap();
    let mode = if semantics.requires_poll_watcher {
        WatcherMode::Poll
    } else {
        WatcherMode::Native
    };
    assert_eq!(
        state
            .path_registrations(mode)
            .0
            .get(&owner_key)
            .map(|registration| registration.os_watch_users),
        Some(1),
        "子路径的逻辑注册必须保留父目录的 OS 监听"
    );
}

#[test]
fn concurrent_add_of_same_path_keeps_one_registration() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let watcher = Arc::new(FsWatcher::new());
    let start = Arc::new(std::sync::Barrier::new(8));
    let threads = (0..8)
        .map(|_| {
            let watcher = watcher.clone();
            let root = root.clone();
            let start = start.clone();
            thread::spawn(move || {
                start.wait();
                watcher.add(&root).unwrap();
            })
        })
        .collect::<Vec<_>>();
    for thread in threads {
        thread.join().unwrap();
    }

    assert_eq!(watcher.registrations.lock().unwrap().len(), 1);
    let registered_path = canonical_watch_path(&root);
    let registrations = global_watcher().state.lock().unwrap();
    assert_eq!(
        registrations
            .watchers
            .values()
            .filter(|registration| {
                registration.key == WatchKey::exact(&registered_path)
                    || registration.key == WatchKey::folded(&registered_path)
            })
            .count(),
        1
    );
}

#[test]
fn overlapping_watchers_keep_parent_os_watch_until_last_child_closes() {
    let temp = tempfile::tempdir().unwrap();
    let child = temp.path().join("child");
    fs::create_dir(&child).unwrap();
    let parent_watcher = FsWatcher::new();
    let child_watcher = FsWatcher::new();
    parent_watcher.add(temp.path()).unwrap();
    child_watcher.add(&child).unwrap();

    let canonical_parent = canonical_watch_path(temp.path());
    let semantics = platform::path_semantics(&canonical_parent).unwrap();
    let owner_key = WatchKey::for_path(&canonical_parent, !semantics.case_sensitive);
    let mode = if semantics.requires_poll_watcher {
        WatcherMode::Poll
    } else {
        WatcherMode::Native
    };
    let users = || {
        let mut state = global_watcher().state.lock().unwrap();
        state
            .path_registrations(mode)
            .0
            .get(&owner_key)
            .map(|registration| registration.os_watch_users)
    };
    assert_eq!(users(), Some(2));

    drop(parent_watcher);
    assert_eq!(users(), Some(1), "子项目仍借用父目录的 OS 监听");

    drop(child_watcher);
    assert_eq!(users(), None, "最后一个订阅释放后撤销 OS 监听");
}

// 真实 OS 事件依赖宿主权限与调度；单独运行以验证交叠项目的监听生命周期。
#[test]
#[ignore]
fn child_watcher_receives_changes_after_parent_watcher_closes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let child = root.join("child");
    fs::create_dir(&child).unwrap();
    let parent_watcher = FsWatcher::new();
    let child_watcher = FsWatcher::new();
    let mut events = child_watcher.watch(Duration::ZERO);
    parent_watcher.add(&root).unwrap();
    child_watcher.add(&child).unwrap();
    drop(parent_watcher);
    thread::sleep(Duration::from_millis(500));

    let file = child.join("changed.txt");
    fs::write(&file, "外部写入").unwrap();
    let batch = smol::block_on(smol::future::or(events.next(), async {
        smol::Timer::after(Duration::from_secs(5)).await;
        None
    }))
    .expect("父项目关闭后子项目仍应收到系统事件");
    assert!(batch.iter().any(|event| event.path.as_path() == file));
}

// 真实 OS 事件受宿主调度影响；单独运行以核对原生后端到 PathEvent 流的链路。
#[test]
#[ignore]
fn real_file_change_reaches_registered_watcher() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let nested = root.join("nested");
    fs::create_dir(&nested).unwrap();
    let watcher = FsWatcher::new();
    let mut events = watcher.watch(Duration::ZERO);
    watcher.add(&root).unwrap();
    thread::sleep(Duration::from_millis(500));

    let file = nested.join("changed.txt");
    fs::write(&file, "外部写入").unwrap();
    let batch = smol::block_on(smol::future::or(events.next(), async {
        smol::Timer::after(Duration::from_secs(5)).await;
        None
    }))
    .expect("文件变化应触发系统事件");
    assert!(batch.iter().any(|event| event.path.starts_with(&nested)));
}

#[test]
fn test_fs_watcher_pending_path() {
    let watcher = FsWatcher::new();
    let temp = tempfile::tempdir().unwrap();
    let nonexistent = temp.path().join("nonexistent");

    // 添加不存在的路径——应启动 pending 轮询
    assert!(watcher.add(&nonexistent).is_ok());

    // 立刻移除——应取消 pending
    assert!(watcher.remove(&nonexistent).is_ok());
}

#[test]
fn test_fs_watcher_closes_event_stream_on_drop() {
    let watcher = FsWatcher::new();
    let mut events = watcher.watch(Duration::ZERO);

    drop(watcher);

    assert!(smol::block_on(events.next()).is_none());
}

/// latency 窗口内到达的事件因缓冲非空不再发信号，应合并为同一批次。
#[test]
fn watch_stream_coalesces_events_within_latency() {
    let watcher = FsWatcher::new();
    let mut events = watcher.watch(Duration::from_millis(10));

    enqueue_path_events(
        &watcher.signal_tx,
        &watcher.pending_path_events,
        vec![changed("/root/a.json")],
    );
    enqueue_path_events(
        &watcher.signal_tx,
        &watcher.pending_path_events,
        vec![changed("/root/b.json")],
    );

    assert_eq!(
        smol::block_on(events.next()),
        Some(vec![changed("/root/a.json"), changed("/root/b.json")])
    );
}

/// 事件入队后，订阅应立即交付包含该事件的批次（latency 为零表示不合并）。
#[test]
fn watch_stream_delivers_enqueued_batch() {
    let watcher = FsWatcher::new();
    let mut events = watcher.watch(Duration::ZERO);

    enqueue_path_events(
        &watcher.signal_tx,
        &watcher.pending_path_events,
        vec![changed("/root/settings.json")],
    );

    assert_eq!(
        smol::block_on(events.next()),
        Some(vec![changed("/root/settings.json")])
    );
}

/// 注册路径必须规范化，否则 macOS 等后端返回的规范化事件路径无法通过前缀过滤。
#[test]
fn add_registers_canonical_watch_root() {
    let temp = tempfile::tempdir().unwrap();
    let watcher = FsWatcher::new();
    watcher.add(temp.path()).unwrap();

    let canonical = canonical_watch_path(temp.path());
    let (has_canonical, has_original) = {
        let registrations = watcher.registrations.lock().unwrap();
        (
            registrations.contains_key(&WatchKey::exact(&canonical))
                || registrations.contains_key(&WatchKey::folded(&canonical)),
            registrations.contains_key(&WatchKey::exact(temp.path()))
                || registrations.contains_key(&WatchKey::folded(temp.path())),
        )
    };
    assert!(has_canonical);
    if canonical != temp.path() {
        assert!(!has_original, "非规范路径不应成为注册键");
    }
}
