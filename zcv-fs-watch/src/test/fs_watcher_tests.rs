use super::*;

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
    std::fs::create_dir(&first).unwrap();
    let second = temp.path().join("zcvcaseprobe");
    let expected_case_sensitive = match std::fs::create_dir(&second) {
        Ok(()) => {
            std::fs::remove_dir(&second).unwrap();
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(error) => panic!("创建大小写探测目录失败：{error}"),
    };
    std::fs::remove_dir(&first).unwrap();

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
