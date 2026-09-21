use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{AppContext, TestAppContext};
use zcv_fs_watch::FsEventStream;
use zcv_text::{Buffer, BufferConfig, ByteOffset, Edit, TransactionMetadata};

use super::*;
use crate::git_store::StatusEntry;
use crate::test_support::{test_git_repo, test_languages, test_project};

fn git_status_for_path(project: &Project, path: &Path, cx: &App) -> Option<StatusEntry> {
    project.git_store.read(cx).status_for_path(path).cloned()
}

#[gpui::test]
fn empty_project_has_no_worktree_or_project_services(cx: &mut TestAppContext) {
    let project = cx.update(|cx| cx.new(|cx| Project::empty(test_languages(), cx)));
    cx.read_entity(&project, |project, _| {
        assert!(!project.has_worktree());
        assert!(project.root().is_none());
        assert!(project.try_git_store().is_none());
    });
}

struct FailingWatcher {
    watcher: FsWatcher,
}

impl FailingWatcher {
    fn new() -> Self {
        Self {
            watcher: FsWatcher::new(),
        }
    }
}

impl Watcher for FailingWatcher {
    fn add(&self, _path: &Path) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("测试监听失败"))
    }

    fn remove(&self, _path: &Path) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("测试停止监听失败"))
    }

    fn events(&self) -> FsEventStream {
        self.watcher.events()
    }
}

#[gpui::test]
fn initial_file_watcher_error_is_buffered_until_workspace_subscribes(
    cx: &mut gpui::TestAppContext,
) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let root = directory.path().to_path_buf();
    let project = cx.new(|cx| {
        Project::new_with_watcher(
            root.clone(),
            Arc::new(FailingWatcher::new()),
            test_languages(),
            cx,
        )
    });

    let errors = project.update(cx, |project, _| project.take_pending_file_watcher_errors());

    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].operation, FileWatcherOperation::Add);
    assert_eq!(
        errors[0].path,
        normalize_for_comparison(&root).unwrap().into_path_buf()
    );
    assert_eq!(errors[0].error, "测试监听失败");
    assert!(
        project
            .update(cx, |project, _| {
                project.take_pending_file_watcher_errors()
            })
            .is_empty()
    );
}

#[test]
fn saving_buffer_writes_current_version_and_marks_it_clean() {
    let path = test_file_path();
    let mut buffer =
        Buffer::from_text("旧内容".to_owned(), BufferConfig::default()).expect("应创建 Buffer");
    buffer
        .edit(
            [Edit::insert(buffer.len_bytes(), " + 新内容").unwrap()],
            TransactionMetadata::default(),
        )
        .expect("测试编辑应成功");
    assert!(buffer.is_dirty());

    write_buffer_to_path(&buffer.snapshot(), &path).expect("保存应成功");
    buffer.mark_saved();

    assert_eq!(
        fs::read_to_string(&path).expect("应读回文件"),
        "旧内容 + 新内容"
    );
    assert!(!buffer.is_dirty());
    fs::remove_file(path).expect("测试文件应可删除");
}

#[test]
fn failed_save_keeps_buffer_dirty() {
    let path = test_file_path().join("missing.txt");
    let mut buffer =
        Buffer::from_text("内容".to_owned(), BufferConfig::default()).expect("应创建 Buffer");
    buffer
        .edit(
            [Edit::insert(ByteOffset::ZERO, "未保存").unwrap()],
            TransactionMetadata::default(),
        )
        .expect("测试编辑应成功");

    assert!(write_buffer_to_path(&buffer.snapshot(), &path).is_err());
    assert!(buffer.is_dirty());
}

#[gpui::test]
fn renaming_file_keeps_open_buffer_indexed_by_new_path(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let old_path = directory.path().join("old.txt");
    let new_path = directory.path().join("new.txt");
    fs::write(&old_path, "content").expect("应创建测试文件");

    let project = test_project(directory.path().to_path_buf(), cx);
    let original = project.update(cx, |project, cx| {
        project.open_buffer(&old_path, cx).expect("应打开测试文件")
    });
    project
        .update(cx, |project, cx| {
            project.rename_path(&old_path, &new_path, cx)
        })
        .expect("应重命名测试文件");
    let reopened = project.update(cx, |project, cx| {
        project
            .open_buffer(&new_path, cx)
            .expect("应从新路径打开文件")
    });

    assert_eq!(original, reopened);
    assert!(!old_path.exists());
    assert!(new_path.exists());
}

#[gpui::test]
fn creating_path_rejects_existing_file_and_directory_collisions(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let file = directory.path().join("src/components/new.txt");
    let folder = directory.path().join("assets/icons/new-folder");
    let project = test_project(directory.path().to_path_buf(), cx);

    project
        .update(cx, |project, cx| project.create_path(&file, false, cx))
        .expect("应创建文件");
    project
        .update(cx, |project, cx| project.create_path(&folder, true, cx))
        .expect("应创建目录");
    fs::write(&file, "existing content").expect("应写入已有文件内容");

    for (path, is_dir) in [
        (&file, false),
        (&file, true),
        (&folder, false),
        (&folder, true),
    ] {
        assert!(
            project
                .update(cx, |project, cx| project.create_path(path, is_dir, cx))
                .is_err(),
            "不应覆盖已有条目：{}",
            path.display()
        );
    }
    assert_eq!(
        fs::read_to_string(&file).expect("应读取已有文件"),
        "existing content",
        "创建冲突不应改动已有文件内容"
    );
    assert!(folder.is_dir(), "创建冲突不应替换已有目录");

    let unsafe_path = directory.path().join("../outside.txt");
    assert!(
        project
            .update(cx, |project, cx| {
                project.create_path(&unsafe_path, false, cx)
            })
            .is_err(),
        "不应允许父目录逃逸"
    );
}

#[gpui::test]
fn trashing_path_rejects_project_root_and_outside_entries(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let project = test_project(directory.path().to_path_buf(), cx);

    for path in [
        directory.path().to_path_buf(),
        PathBuf::from("/outside/file.txt"),
    ] {
        assert!(
            project
                .update(cx, |project, cx| project.trash_path(&path, cx))
                .is_err(),
            "不应允许删除 {}",
            path.display()
        );
    }
}

#[gpui::test]
fn trashing_path_moves_file_to_system_trash(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let file = directory.path().join("to-trash.txt");
    fs::write(&file, "content").expect("应创建测试文件");
    let project = test_project(directory.path().to_path_buf(), cx);

    project.update(cx, |project, cx| {
        project.trash_path(&file, cx).expect("应移到系统废纸篓")
    });

    assert!(!file.exists(), "被删除文件应不再位于原路径");
}

#[gpui::test]
fn moving_file_across_directories_keeps_open_buffer_indexed_by_new_path(
    cx: &mut gpui::TestAppContext,
) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let old_path = directory.path().join("old.txt");
    let new_path = directory.path().join("sub").join("new.txt");
    fs::create_dir(directory.path().join("sub")).expect("应创建子目录");
    fs::write(&old_path, "content").expect("应创建测试文件");

    let project = test_project(directory.path().to_path_buf(), cx);
    let original = project.update(cx, |project, cx| {
        project.open_buffer(&old_path, cx).expect("应打开测试文件")
    });
    project
        .update(cx, |project, cx| {
            project.move_path(&old_path, &new_path, false, cx)
        })
        .expect("应跨目录移动测试文件");
    let reopened = project.update(cx, |project, cx| {
        project
            .open_buffer(&new_path, cx)
            .expect("应从新路径打开文件")
    });

    assert_eq!(original, reopened);
    assert!(!old_path.exists());
    assert!(new_path.exists());
}

#[gpui::test]
fn moving_directory_into_itself_is_rejected(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let dir = directory.path().join("dir");
    fs::create_dir_all(dir.join("sub")).expect("应创建嵌套目录");
    fs::write(dir.join("file.txt"), "内容").expect("应创建测试文件");
    let project = test_project(directory.path().to_path_buf(), cx);

    let destination = dir.join("sub").join("x");
    assert!(
        project
            .update(cx, |project, cx| {
                project.move_path(&dir, &destination, false, cx)
            })
            .is_err(),
        "不应允许把目录移动到自身内部"
    );
    assert!(dir.is_dir(), "原目录应完好");
    assert!(dir.join("file.txt").is_file(), "原目录内文件应完好");
    assert!(!destination.exists(), "目标不应被创建");
}

#[gpui::test]
fn moving_file_overwrites_or_rejects_existing_destination(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let source = directory.path().join("source.txt");
    let target = directory.path().join("target.txt");
    fs::write(&source, "源内容").expect("应创建源文件");
    fs::write(&target, "目标内容").expect("应创建目标文件");
    let project = test_project(directory.path().to_path_buf(), cx);

    assert!(
        project
            .update(cx, |project, cx| {
                project.move_path(&source, &target, false, cx)
            })
            .is_err(),
        "无 overwrite 时冲突应被拒绝"
    );
    assert_eq!(
        fs::read_to_string(&target).expect("应读取目标文件"),
        "目标内容",
        "冲突被拒后目标内容不应变化"
    );

    project
        .update(cx, |project, cx| {
            project.move_path(&source, &target, true, cx)
        })
        .expect("overwrite 时应替换目标文件");
    assert_eq!(
        fs::read_to_string(&target).expect("应读取目标文件"),
        "源内容",
        "替换后目标内容应为源内容"
    );
    assert!(!source.exists(), "源文件应已移走");
}

#[gpui::test]
fn moving_directory_moves_nested_files(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let source_dir = directory.path().join("src");
    let target_dir = directory.path().join("dest");
    fs::create_dir_all(source_dir.join("nested")).expect("应创建嵌套目录");
    fs::write(source_dir.join("nested").join("file.txt"), "内容").expect("应创建测试文件");
    let project = test_project(directory.path().to_path_buf(), cx);

    project
        .update(cx, |project, cx| {
            project.move_path(&source_dir, &target_dir, false, cx)
        })
        .expect("应移动目录");

    assert!(!source_dir.exists(), "旧目录不应再存在");
    assert_eq!(
        fs::read_to_string(target_dir.join("nested").join("file.txt")).expect("应读取迁移后的文件"),
        "内容"
    );
}

#[gpui::test]
fn moving_directory_with_overwrite_replaces_existing_directory(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let source_dir = directory.path().join("src");
    let target_dir = directory.path().join("dest");
    fs::create_dir_all(source_dir.join("nested")).expect("应创建源目录");
    fs::write(source_dir.join("nested").join("file.txt"), "新内容").expect("应创建测试文件");
    fs::create_dir_all(target_dir.join("old")).expect("应创建目标目录");
    fs::write(target_dir.join("old").join("legacy.txt"), "旧内容").expect("应创建测试文件");
    let project = test_project(directory.path().to_path_buf(), cx);

    project
        .update(cx, |project, cx| {
            project.move_path(&source_dir, &target_dir, true, cx)
        })
        .expect("overwrite 时应替换目标目录");

    assert!(!source_dir.exists(), "源目录应已移走");
    assert_eq!(
        fs::read_to_string(target_dir.join("nested").join("file.txt")).expect("应读取替换后的文件"),
        "新内容"
    );
    assert!(
        !target_dir.join("old").join("legacy.txt").exists(),
        "目标目录原有内容应被移除"
    );
}

#[gpui::test]
async fn copying_directory_recursively_replicates_contents(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let source_dir = directory.path().join("src");
    fs::create_dir_all(source_dir.join("嵌套")).expect("应创建嵌套目录");
    fs::write(source_dir.join("顶层.md"), "顶层内容").expect("应创建测试文件");
    fs::write(source_dir.join("嵌套").join("中文文件.txt"), "嵌套内容").expect("应创建测试文件");
    let destination_dir = directory.path().join("copy");
    let project = test_project(directory.path().to_path_buf(), cx);

    let task = project
        .update(cx, |project, cx| {
            project.copy_path(&source_dir, &destination_dir, false, cx)
        })
        .expect("应复制目录");
    // 复制本体在后台线程执行，await 任务后新路径才存在。
    task.await.expect("复制任务应成功");

    assert_eq!(
        fs::read_to_string(destination_dir.join("顶层.md")).expect("应读取复制出的文件"),
        "顶层内容"
    );
    assert_eq!(
        fs::read_to_string(destination_dir.join("嵌套").join("中文文件.txt"))
            .expect("应读取复制出的文件"),
        "嵌套内容"
    );
    // 复制不改动源目录。
    assert!(source_dir.join("嵌套").join("中文文件.txt").is_file());
}

#[gpui::test]
fn copying_into_own_subtree_is_rejected(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let source_dir = directory.path().join("src");
    fs::create_dir_all(&source_dir).expect("应创建源目录");
    fs::write(source_dir.join("file.txt"), "内容").expect("应创建测试文件");
    let project = test_project(directory.path().to_path_buf(), cx);

    let destination = source_dir.join("copy");
    assert!(
        project
            .update(cx, |project, cx| {
                project.copy_path(&source_dir, &destination, false, cx)
            })
            .is_err(),
        "不应允许把目录复制到自身内部"
    );
    assert!(!destination.exists(), "目标不应被创建");
}

#[gpui::test]
fn copying_without_overwrite_rejects_existing_destination(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let source = directory.path().join("source.txt");
    let destination = directory.path().join("destination.txt");
    fs::write(&source, "源内容").expect("应创建源文件");
    fs::write(&destination, "目标内容").expect("应创建目标文件");
    let project = test_project(directory.path().to_path_buf(), cx);

    assert!(
        project
            .update(cx, |project, cx| {
                project.copy_path(&source, &destination, false, cx)
            })
            .is_err(),
        "无 overwrite 时冲突应被拒绝"
    );
    assert_eq!(
        fs::read_to_string(&destination).expect("应读取目标文件"),
        "目标内容",
        "冲突被拒后目标内容不应变化"
    );
    assert!(source.is_file(), "源文件不应被删除");
}

#[gpui::test]
async fn copying_file_completes_in_background(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let source = directory.path().join("source.txt");
    let destination = directory.path().join("destination.txt");
    fs::write(&source, "内容").expect("应创建测试文件");
    let project = test_project(directory.path().to_path_buf(), cx);

    let task = project
        .update(cx, |project, cx| {
            project.copy_path(&source, &destination, false, cx)
        })
        .expect("应复制文件");
    // 复制本体在后台线程执行，await 任务后新路径才存在。
    task.await.expect("复制任务应成功");

    assert_eq!(
        fs::read_to_string(&destination).expect("应读取复制出的文件"),
        "内容"
    );
    assert!(source.is_file(), "复制不删除源文件");
}

#[gpui::test]
async fn copying_with_overwrite_replaces_existing_destination(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let source = directory.path().join("source.txt");
    let destination = directory.path().join("destination.txt");
    fs::write(&source, "新内容").expect("应创建源文件");
    fs::write(&destination, "旧内容").expect("应创建目标文件");
    let project = test_project(directory.path().to_path_buf(), cx);

    let task = project
        .update(cx, |project, cx| {
            project.copy_path(&source, &destination, true, cx)
        })
        .expect("overwrite 时应替换目标文件");
    task.await.expect("复制任务应成功");

    assert_eq!(
        fs::read_to_string(&destination).expect("应读取目标文件"),
        "新内容",
        "复制后目标内容应为源内容"
    );
    assert!(source.is_file(), "复制不删除源文件");
}

#[gpui::test]
fn moving_directory_into_own_ancestor_is_rejected_even_with_overwrite(
    cx: &mut gpui::TestAppContext,
) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let ancestor = directory.path().join("dir");
    let source = ancestor.join("sub");
    fs::create_dir_all(&source).expect("应创建源目录");
    fs::write(source.join("file.txt"), "源内容").expect("应创建测试文件");
    fs::write(ancestor.join("keep.txt"), "原有内容").expect("应创建测试文件");
    let project = test_project(directory.path().to_path_buf(), cx);

    // 目标是源的祖先目录；若不拦截，覆盖路径的「先删目标」会把源一起递归删掉。
    let result = project.update(cx, |project, cx| {
        project.move_path(&source, &ancestor, true, cx)
    });
    assert!(result.is_err(), "不应允许把条目移动到自身的祖先目录");
    assert!(source.is_dir(), "源目录应完好");
    assert_eq!(
        fs::read_to_string(source.join("file.txt")).expect("应读取源目录内文件"),
        "源内容",
        "源目录内容不应被破坏"
    );
    assert_eq!(
        fs::read_to_string(ancestor.join("keep.txt")).expect("应读取祖先目录原有文件"),
        "原有内容",
        "祖先目录原有内容不应被破坏"
    );
}

#[gpui::test]
fn copying_directory_into_own_ancestor_is_rejected(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let source = directory.path().join("src");
    fs::create_dir_all(&source).expect("应创建源目录");
    fs::write(source.join("file.txt"), "内容").expect("应创建测试文件");
    let project = test_project(directory.path().to_path_buf(), cx);

    // 目标是项目根（源的祖先）：即便允许覆盖也必须拒绝。
    let destination = directory.path().to_path_buf();
    let result = project.update(cx, |project, cx| {
        project.copy_path(&source, &destination, true, cx)
    });
    assert!(result.is_err(), "不应允许把条目复制到自身的祖先目录");
    assert!(source.join("file.txt").is_file(), "源目录内容应完好");
}

#[gpui::test]
fn fs_events_trigger_incremental_git_status_refresh(cx: &mut gpui::TestAppContext) {
    let (root, _temp) = test_git_repo();
    let project = test_project(root.clone(), cx);
    cx.run_until_parked();

    // 初始扫描后文件干净，无 git 状态。
    let file = root.join("tracked.txt");
    assert!(
        project
            .update(cx, |project, cx| git_status_for_path(project, &file, cx))
            .is_none()
    );

    // 文件被外部修改 → fs 事件 → 增量刷新。
    fs::write(&file, "已修改\n").expect("应修改文件");
    project.update(cx, |project, cx| {
        project.process_fs_events(
            vec![
                PathEvent::new(file.clone(), Some(PathEventKind::Changed))
                    .expect("测试事件路径应为绝对路径"),
            ],
            cx,
        );
    });
    cx.run_until_parked();

    let entry = project
        .update(cx, |project, cx| git_status_for_path(project, &file, cx))
        .expect("应有 git 状态");
    assert!(entry.status.is_modified());
}

#[gpui::test]
fn fs_removal_events_trigger_full_rescan(cx: &mut gpui::TestAppContext) {
    let (root, _temp) = test_git_repo();
    let project = test_project(root.clone(), cx);
    cx.run_until_parked();

    // 未跟踪文件出现，随后被删除：Removed 事件应触发全量扫描，
    // 状态表不再包含该路径。
    let file = root.join("scratch.txt");
    fs::write(&file, "临时\n").expect("应创建文件");
    project.update(cx, |project, cx| {
        project.process_fs_events(
            vec![
                PathEvent::new(file.clone(), Some(PathEventKind::Created))
                    .expect("测试事件路径应为绝对路径"),
            ],
            cx,
        );
    });
    cx.run_until_parked();
    assert!(
        project
            .update(cx, |project, cx| git_status_for_path(project, &file, cx))
            .is_some()
    );

    fs::remove_file(&file).expect("应删除文件");
    project.update(cx, |project, cx| {
        project.process_fs_events(
            vec![
                PathEvent::new(file.clone(), Some(PathEventKind::Removed))
                    .expect("测试事件路径应为绝对路径"),
            ],
            cx,
        );
    });
    cx.run_until_parked();
    assert!(
        project
            .update(cx, |project, cx| git_status_for_path(project, &file, cx))
            .is_none()
    );
}

#[gpui::test]
fn fs_rescan_events_discover_unreported_file(cx: &mut gpui::TestAppContext) {
    let (root, _temp) = test_git_repo();
    let project = test_project(root.clone(), cx);
    cx.run_until_parked();

    // 文件在监听器失步期间出现，没有 Created 事件；Rescan 必须让项目重新发现它。
    let file = root.join("discovered-after-rescan.txt");
    fs::write(&file, "重新扫描发现\n").expect("应创建测试文件");
    assert!(
        project
            .update(cx, |project, cx| git_status_for_path(project, &file, cx))
            .is_none()
    );

    project.update(cx, |project, cx| {
        project.process_fs_events(
            vec![
                PathEvent::new(root.clone(), Some(PathEventKind::Rescan))
                    .expect("测试事件路径应为绝对路径"),
            ],
            cx,
        );
    });
    cx.run_until_parked();

    assert!(
        project
            .update(cx, |project, cx| git_status_for_path(project, &file, cx))
            .is_some(),
        "Rescan 后应发现监听器未报告的新文件"
    );
}

// 依赖真实 FSEvents 事件：并行测试下系统会合并/延迟事件导致偶发超时，
// 串行（--test-threads=1）或单独运行时稳定。用 `cargo test -- --ignored` 显式验证。
#[gpui::test]
#[ignore]
fn real_fs_watcher_triggers_git_refresh(cx: &mut gpui::TestAppContext) {
    // 模拟生产的 Project root：生产路径经 canonicalize 归一化（macOS 上
    // /var → /private/var），否则 FSEvents 返回的实际路径与注册路径
    // 前缀不匹配，事件会被 fs_watcher 过滤掉。
    let (root, _temp) = test_git_repo();
    let root = root.canonicalize().expect("应可 canonicalize");
    let project = cx.new(|cx| Project::new(root.clone(), test_languages(), cx));
    cx.run_until_parked();

    // 等 notify 在后台线程建立 watch，避免写入事件丢失。
    std::thread::sleep(std::time::Duration::from_millis(500));
    // 真实写文件 → notify 监听 → process_fs_events → git 增量刷新。
    fs::write(root.join("tracked.txt"), "外部修改\n").expect("应写入文件");
    let file = root.join("tracked.txt");
    // FSEvents 事件在并行测试负载下可能延迟数秒，放宽超时。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        if project
            .update(cx, |project, cx| git_status_for_path(project, &file, cx))
            .is_some()
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "等待 fs 事件驱动的 git 刷新超时"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[gpui::test]
fn saving_buffer_refreshes_git_status(cx: &mut gpui::TestAppContext) {
    let (root, _temp) = test_git_repo();
    let project = test_project(root.clone(), cx);
    cx.run_until_parked();

    // 打开并修改 buffer（未保存），git 状态应仍为干净（status 反映磁盘）。
    let file = root.join("tracked.txt");
    let buffer = project
        .update(cx, |project, cx| project.open_buffer(&file, cx))
        .expect("应打开文件");
    buffer
        .update(cx, |language_buffer, cx| {
            let offset = language_buffer.len_bytes();
            language_buffer.edit(
                [Edit::insert(offset, "新增行\n").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
        })
        .expect("编辑应成功");
    cx.run_until_parked();
    assert!(
        project
            .update(cx, |project, cx| git_status_for_path(project, &file, cx))
            .is_none()
    );

    // 保存后 git 状态应变为已修改。
    project
        .update(cx, |project, cx| {
            project.save_file_buffers(vec![(buffer.clone(), file.clone())], cx)
        })
        .expect("保存应成功");
    cx.run_until_parked();
    let entry = project
        .update(cx, |project, cx| git_status_for_path(project, &file, cx))
        .expect("保存后应有 git 状态");
    assert!(entry.status.is_modified());
}

#[gpui::test]
fn save_events_preserve_undo_history(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let file = directory.path().join("document.txt");
    fs::write(&file, "原内容").expect("应创建测试文件");
    let project = test_project(directory.path().to_path_buf(), cx);
    let language_buffer = project
        .update(cx, |project, cx| project.open_buffer(&file, cx))
        .expect("应打开测试文件");
    language_buffer
        .update(cx, |language_buffer, cx| {
            let offset = language_buffer.len_bytes();
            language_buffer.edit(
                [Edit::insert(offset, " + 修改").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
        })
        .expect("编辑应成功");

    project
        .update(cx, |project, cx| {
            project.save_file_buffers(vec![(language_buffer.clone(), file.clone())], cx)
        })
        .expect("保存应成功");
    project.update(cx, |project, cx| {
        project.process_fs_events(
            vec![
                PathEvent::new(file.clone(), Some(PathEventKind::Changed))
                    .expect("测试事件路径应为绝对路径"),
            ],
            cx,
        );
    });

    language_buffer.read_with(cx, |language_buffer, _| assert!(language_buffer.can_undo()));
    language_buffer
        .update(cx, |language_buffer, cx| language_buffer.undo(cx))
        .expect("撤销应成功")
        .expect("保存前的编辑应仍在历史中");
    language_buffer.read_with(cx, |language_buffer, _| {
        let snapshot = language_buffer.text_snapshot();
        assert_eq!(
            snapshot
                .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
                .expect("应读取完整文本")
                .as_str(),
            "原内容"
        );
        assert!(language_buffer.is_dirty());
        assert!(language_buffer.can_redo());
    });

    // 同一次保存可能产生重复或延迟事件；用户撤销后文档已变脏，事件不能反向覆盖。
    project.update(cx, |project, cx| {
        project.process_fs_events(
            vec![
                PathEvent::new(file.clone(), Some(PathEventKind::Changed))
                    .expect("测试事件路径应为绝对路径"),
            ],
            cx,
        );
    });
    language_buffer.read_with(cx, |language_buffer, _| {
        let snapshot = language_buffer.text_snapshot();
        assert_eq!(
            snapshot
                .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
                .expect("应读取完整文本")
                .as_str(),
            "原内容"
        );
        assert!(language_buffer.can_redo());
    });
}

/// 复制失败的注入点选在同步入口（源不存在）：直接调内部函数验证失败路径。
/// （copy_path 后台任务的失败同样从 `copy_entry_overwrite` 起源，覆盖同一条失败链。）
#[test]
fn failed_copy_preserves_existing_destination_and_leaves_no_tmp() {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let source = directory.path().join("missing.txt");
    let destination = directory.path().join("destination.txt");
    fs::write(&destination, "目标内容").expect("应创建目标文件");

    assert!(
        copy_entry_overwrite(&source, &destination).is_err(),
        "源不存在时复制应失败"
    );
    assert_eq!(
        fs::read_to_string(&destination).expect("应读取目标文件"),
        "目标内容",
        "复制失败不应破坏原目标"
    );
    assert!(
        !sibling_tmp_path(&destination).exists(),
        "失败后不应残留临时文件"
    );
}

#[test]
fn overwrite_copy_replaces_existing_file_and_directory() {
    // 文件覆盖文件：目标内容被替换，源不动。
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let source_file = directory.path().join("source.txt");
    let dest_file = directory.path().join("destination.txt");
    fs::write(&source_file, "新内容").expect("应创建源文件");
    fs::write(&dest_file, "旧内容").expect("应创建目标文件");
    copy_entry_overwrite(&source_file, &dest_file).expect("应覆盖文件");
    assert_eq!(
        fs::read_to_string(&dest_file).expect("应读取目标文件"),
        "新内容"
    );

    // 目录覆盖目录：旧内容被移除，新内容入位。
    let source_dir = directory.path().join("source-dir");
    let dest_dir = directory.path().join("destination-dir");
    fs::create_dir_all(&source_dir).expect("应创建源目录");
    fs::write(source_dir.join("new.txt"), "新目录内容").expect("应创建测试文件");
    fs::create_dir_all(&dest_dir).expect("应创建目标目录");
    fs::write(dest_dir.join("old.txt"), "旧目录内容").expect("应创建测试文件");
    copy_entry_overwrite(&source_dir, &dest_dir).expect("应覆盖目录");
    assert_eq!(
        fs::read_to_string(dest_dir.join("new.txt")).expect("应读取替换后的文件"),
        "新目录内容"
    );
    assert!(!dest_dir.join("old.txt").exists(), "目标目录旧内容应被移除");
    assert!(source_dir.join("new.txt").is_file(), "源目录不应被删除");
}

#[cfg(unix)]
#[test]
fn copying_directory_with_ancestor_symlink_does_not_hang() {
    // 链接指向自身祖先目录（链接环）：按链接本身复制，不跟随目标、不挂死。
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let source_dir = directory.path().join("dir");
    fs::create_dir_all(&source_dir).expect("应创建源目录");
    fs::write(source_dir.join("real.txt"), "内容").expect("应创建测试文件");
    std::os::unix::fs::symlink(directory.path(), source_dir.join("loop"))
        .expect("应创建指向祖先目录的符号链接");
    let destination = directory.path().join("copy");

    copy_entry_recursive(&source_dir, &destination).expect("应完成含链接环的复制");

    assert_eq!(
        fs::read_to_string(destination.join("real.txt")).expect("应读取复制出的文件"),
        "内容"
    );
    let link = destination.join("loop");
    assert!(
        link.symlink_metadata()
            .expect("应读取链接元信息")
            .file_type()
            .is_symlink(),
        "链接应按链接本身复制"
    );
    assert_eq!(
        fs::read_link(&link).expect("应读取链接目标"),
        directory.path().to_path_buf(),
        "链接目标应保持原样"
    );
}

fn test_file_path() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("系统时间应晚于 Unix Epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "project-save-test-{}-{nonce}.txt",
        std::process::id()
    ))
}
