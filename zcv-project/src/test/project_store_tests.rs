use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{AppContext, TestAppContext};
use zcv_fs_watch::FsEventStream;
use zcv_language::LanguageBuffer;
use zcv_settings::{SettingsStore, UserSettings};
use zcv_text::{Buffer, BufferConfig, ByteOffset, Edit, TransactionMetadata};

use super::*;
use crate::git_store::StatusEntry;
use crate::test_support::{TestWatcher, test_git_repo, test_languages, test_project};

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

#[gpui::test]
fn project_construction_survives_root_removed_after_validation(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let root = AbsolutePathBuf::canonicalize(directory.path()).expect("项目根目录应可规范化");
    fs::remove_dir(directory.path()).expect("应移除已校验的项目根目录");

    let project = cx.new(|cx| Project::new(root.clone(), test_languages(), cx));
    cx.read_entity(&project, |project, _| {
        assert_eq!(project.root(), Some(root.as_path()));
    });
}

#[gpui::test]
async fn background_file_load_installs_one_shared_document(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let first_path = directory.path().join("first.txt");
    let second_path = directory.path().join("second.txt");
    fs::write(&first_path, "后台读取\n").expect("应写入文件");
    fs::write(&second_path, "同一路径\n").expect("应写入文件");
    let project = test_project(directory.path().to_path_buf(), cx);

    let first = project
        .update(cx, |project, cx| {
            project.open_buffer_async(first_path.clone(), cx)
        })
        .await
        .expect("后台打开应成功");
    let reopened = project.update(cx, |project, cx| project.open_buffer(&first_path, cx));
    assert_eq!(first, reopened.expect("已打开的文件应复用"));
    let snapshot = cx.read_entity(&first, |buffer, _| buffer.text_snapshot());
    let range = TextRange::new(ByteOffset::ZERO, snapshot.len_bytes()).expect("全文范围应有效");
    assert_eq!(snapshot.slice_text(range).unwrap().as_str(), "后台读取\n");

    let loading = project.update(cx, |project, cx| {
        project.open_buffer_async(second_path.clone(), cx)
    });
    let opened = project
        .update(cx, |project, cx| project.open_buffer(&second_path, cx))
        .expect("同步打开应成功");
    assert_eq!(loading.await.expect("后台打开应复用已存在文档"), opened);
}

#[gpui::test]
fn released_document_drops_project_subscription_and_path_index(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let file = directory.path().join("open.txt");
    fs::write(&file, "内容").expect("应创建文件");
    let project = test_project(directory.path().to_path_buf(), cx);
    let buffer = project
        .update(cx, |project, cx| project.open_buffer(&file, cx))
        .expect("应打开文件");
    let indexed_file = normalize_for_comparison(&file).expect("文件路径应可规范化");
    project.read_with(cx, |project, _| {
        assert_eq!(project.buffer_subscriptions.len(), 1);
        assert_eq!(
            project.buffer_store.buffers_under_path(&indexed_file),
            vec![buffer.clone()]
        );
    });

    drop(buffer);
    cx.run_until_parked();
    project.read_with(cx, |project, _| {
        assert!(project.buffer_subscriptions.is_empty());
        assert!(
            project
                .buffer_store
                .buffers_under_path(&indexed_file)
                .is_empty()
        );
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

    fn watch(&self, latency: std::time::Duration) -> FsEventStream {
        self.watcher.watch(latency)
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
        .expect("测试项目根目录应可规范化")
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

#[gpui::test]
fn saving_buffer_writes_current_version_and_marks_it_clean(cx: &mut TestAppContext) {
    let path = test_file_path();
    let language_buffer = cx.new(|cx| {
        let buffer =
            Buffer::from_text("旧内容".to_owned(), BufferConfig::default()).expect("应创建 Buffer");
        LanguageBuffer::new(buffer, None, test_languages(), cx)
    });
    language_buffer.update(cx, |language_buffer, cx| {
        let len = language_buffer.len_bytes();
        language_buffer
            .edit(
                [Edit::insert(len, " + 新内容").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("测试编辑应成功");
    });
    assert!(language_buffer.read_with(cx, |language_buffer, _| language_buffer.is_dirty()));

    language_buffer.update(cx, |language_buffer, cx| {
        write_buffer_to_path(&language_buffer.text_snapshot(), &path).expect("保存应成功");
        let version = language_buffer.version();
        language_buffer.did_save(version, cx);
    });

    assert_eq!(
        fs::read_to_string(&path).expect("应读回文件"),
        "旧内容 + 新内容"
    );
    assert!(!language_buffer.read_with(cx, |language_buffer, _| language_buffer.is_dirty()));
    fs::remove_file(path).expect("测试文件应可删除");
}

#[gpui::test]
fn saving_crlf_input_writes_the_source_snapshot_and_marks_that_version_clean(
    cx: &mut TestAppContext,
) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let path = directory.path().join("document.txt");
    fs::write(&path, "甲\r\n乙\r").expect("应创建文件");
    let project = test_project(directory.path().to_path_buf(), cx);
    let buffer = project
        .update(cx, |project, cx| project.open_buffer(&path, cx))
        .expect("应打开文件");
    buffer
        .update(cx, |buffer, cx| {
            buffer.edit(
                [Edit::insert(buffer.len_bytes(), "丙\r\n").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
        })
        .expect("编辑应成功");

    let snapshot = buffer.read_with(cx, |buffer, _| buffer.text_snapshot());
    let expected = snapshot
        .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
        .expect("快照应可读取")
        .as_str()
        .to_owned();
    assert_eq!(expected, "甲\n乙\n丙\n");

    project
        .update(cx, |project, cx| {
            project.save_file_buffers(
                vec![(buffer.clone(), path.clone())],
                FileSaveConflictPolicy::Reject,
                cx,
            )
        })
        .expect("保存应成功");

    assert_eq!(fs::read_to_string(&path).expect("应读回文件"), expected);
    buffer.read_with(cx, |buffer, _| {
        assert!(!buffer.is_dirty());
    });
}

#[gpui::test]
fn save_formats_the_source_buffer_as_one_undo_step_before_writing(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let path = directory.path().join("document.txt");
    fs::write(&path, "甲  \n乙\t \n \t\n").expect("应创建文件");
    let project = test_project(directory.path().to_path_buf(), cx);
    let buffer = project
        .update(cx, |project, cx| project.open_buffer(&path, cx))
        .expect("应打开文件");
    let before = buffer.read_with(cx, |buffer, _| buffer.version());

    project
        .update(cx, |project, cx| {
            project.save_file_buffers(
                vec![(buffer.clone(), path.clone())],
                FileSaveConflictPolicy::Reject,
                cx,
            )
        })
        .expect("保存应成功");

    assert_eq!(fs::read_to_string(&path).unwrap(), "甲\n乙\n");
    buffer.read_with(cx, |buffer, _| {
        let snapshot = buffer.text_snapshot();
        assert_ne!(snapshot.version(), before);
        assert!(!buffer.is_dirty());
    });
    buffer
        .update(cx, |buffer, cx| buffer.undo(cx))
        .expect("撤销应成功")
        .expect("保存整理应可撤销");
    buffer.read_with(cx, |buffer, _| {
        let snapshot = buffer.text_snapshot();
        assert_eq!(
            snapshot
                .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
                .unwrap()
                .as_str(),
            "甲  \n乙\t \n \t\n"
        );
        assert!(buffer.is_dirty());
        assert!(!buffer.can_undo());
    });
}

#[gpui::test]
fn saving_empty_or_already_formatted_file_does_not_create_an_edit(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let project = test_project(directory.path().to_path_buf(), cx);
    for (name, contents) in [("empty.txt", ""), ("formatted.txt", "内容\n")] {
        let path = directory.path().join(name);
        fs::write(&path, contents).unwrap();
        let buffer = project
            .update(cx, |project, cx| project.open_buffer(&path, cx))
            .unwrap();
        let before = buffer.read_with(cx, |buffer, _| buffer.version());

        project
            .update(cx, |project, cx| {
                project.save_file_buffers(
                    vec![(buffer.clone(), path.clone())],
                    FileSaveConflictPolicy::Reject,
                    cx,
                )
            })
            .unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), contents);
        buffer.read_with(cx, |buffer, _| {
            assert_eq!(buffer.version(), before);
            assert!(!buffer.is_dirty());
            assert!(!buffer.can_undo());
        });
    }
}

#[gpui::test]
fn save_whitespace_settings_apply_independently(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("document.txt");
    fs::write(&path, "a  \nb  \n\n").unwrap();
    cx.update(|cx| {
        let mut settings = UserSettings::default();
        settings.remove_trailing_whitespace_on_save = false;
        cx.set_global(SettingsStore::new(settings));
    });
    let project = test_project(directory.path().to_path_buf(), cx);
    let buffer = project
        .update(cx, |project, cx| project.open_buffer(&path, cx))
        .unwrap();

    project
        .update(cx, |project, cx| {
            project.save_file_buffers(
                vec![(buffer.clone(), path.clone())],
                FileSaveConflictPolicy::Reject,
                cx,
            )
        })
        .unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "a  \nb\n");
}

#[gpui::test]
fn disabling_final_newline_keeps_extra_blank_lines_after_trimming(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("document.txt");
    fs::write(&path, "a  \nb  \n\n").unwrap();
    cx.update(|cx| {
        let mut settings = UserSettings::default();
        settings.ensure_final_newline_on_save = false;
        cx.set_global(SettingsStore::new(settings));
    });
    let project = test_project(directory.path().to_path_buf(), cx);
    let buffer = project
        .update(cx, |project, cx| project.open_buffer(&path, cx))
        .unwrap();

    project
        .update(cx, |project, cx| {
            project.save_file_buffers(
                vec![(buffer.clone(), path.clone())],
                FileSaveConflictPolicy::Reject,
                cx,
            )
        })
        .unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "a\nb\n\n");
}

#[gpui::test]
fn save_uses_language_override_without_changing_global_defaults(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let rust_path = directory.path().join("main.rs");
    let text_path = directory.path().join("notes.txt");
    fs::write(&rust_path, "fn main() {}  \n\n").unwrap();
    fs::write(&text_path, "notes  \n\n").unwrap();
    cx.update(|cx| {
        let mut settings = SettingsStore::new(UserSettings::default());
        settings
            .set_user_settings(
                r#"{"languages":{"Rust":{"remove_trailing_whitespace_on_save":false,"ensure_final_newline_on_save":false}}}"#,
            )
            .unwrap();
        cx.set_global(settings);
    });
    let project = test_project(directory.path().to_path_buf(), cx);
    let rust = project
        .update(cx, |project, cx| project.open_buffer(&rust_path, cx))
        .unwrap();
    let text = project
        .update(cx, |project, cx| project.open_buffer(&text_path, cx))
        .unwrap();

    project
        .update(cx, |project, cx| {
            project.save_file_buffers(
                vec![(rust, rust_path.clone()), (text, text_path.clone())],
                FileSaveConflictPolicy::Reject,
                cx,
            )
        })
        .unwrap();
    assert_eq!(
        fs::read_to_string(&rust_path).unwrap(),
        "fn main() {}  \n\n"
    );
    assert_eq!(fs::read_to_string(&text_path).unwrap(), "notes\n");
}

#[gpui::test]
fn failed_write_keeps_save_format_edit_dirty(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("document.txt");
    fs::write(&path, "内容  ").unwrap();
    let project = test_project(directory.path().to_path_buf(), cx);
    let buffer = project
        .update(cx, |project, cx| project.open_buffer(&path, cx))
        .unwrap();
    let missing_path = directory.path().join("missing").join("document.txt");

    assert!(
        project
            .update(cx, |project, cx| {
                project.save_file_buffers(
                    vec![(buffer.clone(), missing_path)],
                    FileSaveConflictPolicy::Reject,
                    cx,
                )
            })
            .is_err()
    );
    buffer.read_with(cx, |buffer, _| {
        let snapshot = buffer.text_snapshot();
        assert_eq!(
            snapshot
                .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
                .unwrap()
                .as_str(),
            "内容\n"
        );
        assert!(buffer.is_dirty());
    });
    assert_eq!(fs::read_to_string(&path).unwrap(), "内容  ");
}

#[gpui::test]
fn failed_save_keeps_buffer_dirty(cx: &mut TestAppContext) {
    let path = test_file_path().join("missing.txt");
    let language_buffer = cx.new(|cx| {
        let buffer =
            Buffer::from_text("内容".to_owned(), BufferConfig::default()).expect("应创建 Buffer");
        LanguageBuffer::new(buffer, None, test_languages(), cx)
    });
    language_buffer.update(cx, |language_buffer, cx| {
        language_buffer
            .edit(
                [Edit::insert(ByteOffset::ZERO, "未保存").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("测试编辑应成功");
    });

    assert!(
        write_buffer_to_path(
            &language_buffer.read_with(cx, |language_buffer, _| language_buffer.text_snapshot()),
            &path
        )
        .is_err()
    );
    assert!(language_buffer.read_with(cx, |language_buffer, _| language_buffer.is_dirty()));
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
async fn copying_external_directory_into_project(cx: &mut gpui::TestAppContext) {
    let project_dir = tempfile::tempdir().expect("应创建项目目录");
    let external_dir = tempfile::tempdir().expect("应创建外部目录");
    let source = external_dir.path().join("源目录");
    fs::create_dir(&source).expect("应创建源目录");
    fs::write(source.join("内容.txt"), "外部内容").expect("应创建外部文件");
    let destination = project_dir.path().join("源目录");
    let project = test_project(project_dir.path().to_path_buf(), cx);

    assert!(
        project
            .update(cx, |project, cx| project.copy_path(
                &source,
                &destination,
                false,
                cx
            ))
            .is_err(),
        "项目内复制入口仍应要求项目内源路径"
    );
    let task = project
        .update(cx, |project, cx| {
            project.copy_external_path(&source, &destination, false, cx)
        })
        .expect("外部复制入口应接收项目外源路径");
    task.await.expect("应完成外部目录复制");

    assert_eq!(
        fs::read_to_string(destination.join("内容.txt")).expect("应读取导入文件"),
        "外部内容"
    );
    assert!(source.join("内容.txt").exists(), "导入不移动源目录");
}

#[gpui::test]
fn copying_external_path_rejects_target_outside_project(cx: &mut gpui::TestAppContext) {
    let project_dir = tempfile::tempdir().expect("应创建项目目录");
    let external_dir = tempfile::tempdir().expect("应创建外部目录");
    let source = external_dir.path().join("source.txt");
    fs::write(&source, "内容").expect("应创建外部文件");
    let destination = external_dir.path().join("destination.txt");
    let project = test_project(project_dir.path().to_path_buf(), cx);

    assert!(
        project
            .update(cx, |project, cx| {
                project.copy_external_path(&source, &destination, false, cx)
            })
            .is_err(),
        "目标必须位于项目内"
    );
    assert!(!destination.exists());
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
fn directory_and_rescan_events_reload_open_document(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let root = directory.path();
    let folder = root.join("nested");
    fs::create_dir(&folder).expect("应创建子目录");
    let file = folder.join("open.txt");
    fs::write(&file, "初始内容").expect("应创建文件");
    let project = test_project(root.to_path_buf(), cx);
    let buffer = project
        .update(cx, |project, cx| project.open_buffer(&file, cx))
        .expect("应打开文件");

    for (event_path, kind, expected) in [
        (folder.clone(), Some(PathEventKind::Changed), "目录事件"),
        (root.to_path_buf(), Some(PathEventKind::Rescan), "重新扫描"),
        (file.clone(), None, "未知事件"),
    ] {
        fs::write(&file, expected).expect("应修改文件");
        project.update(cx, |project, cx| {
            project.process_fs_events(vec![PathEvent::new(event_path, kind).unwrap()], cx);
        });
        cx.run_until_parked();
        let snapshot = cx.read_entity(&buffer, |buffer, _| buffer.text_snapshot());
        assert_eq!(
            snapshot
                .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
                .unwrap()
                .as_str(),
            expected
        );
        assert!(!cx.read_entity(&buffer, |buffer, _| buffer.is_dirty()));
    }
}

#[gpui::test]
fn reload_waits_for_file_recreated_after_transient_read_failure(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let file = directory.path().join("open.txt");
    fs::write(&file, "旧内容").expect("应创建文件");
    let project = test_project(directory.path().to_path_buf(), cx);
    let buffer = project
        .update(cx, |project, cx| project.open_buffer(&file, cx))
        .expect("应打开文件");

    fs::remove_file(&file).expect("模拟写入期间文件暂时消失");
    project.update(cx, |project, cx| {
        project.process_fs_events(
            vec![PathEvent::new(file.clone(), Some(PathEventKind::Removed)).unwrap()],
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(
        cx.read_entity(&buffer, |buffer, _| {
            let snapshot = buffer.text_snapshot();
            snapshot
                .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
                .unwrap()
                .as_str()
                .to_owned()
        }),
        "旧内容"
    );

    fs::write(&file, "重建内容").expect("文件应恢复");
    cx.executor().advance_clock(RELOAD_RETRY_INITIAL);
    cx.run_until_parked();
    assert_eq!(
        cx.read_entity(&buffer, |buffer, _| {
            let snapshot = buffer.text_snapshot();
            snapshot
                .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
                .unwrap()
                .as_str()
                .to_owned()
        }),
        "重建内容",
        "没有第二条文件事件时，待完成的重载也应收敛"
    );
}

#[gpui::test]
fn empty_project_watches_its_open_file(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let file = directory.path().join("standalone.txt");
    fs::write(&file, "初始内容").expect("应创建文件");
    let project = cx
        .new(|cx| Project::empty_with_watcher(test_languages(), Arc::new(TestWatcher::new()), cx));
    let buffer = project
        .update(cx, |project, cx| project.open_buffer(&file, cx))
        .expect("空工作区应能打开文件");
    project.read_with(cx, |project, _| {
        assert_eq!(project.external_watch_counts.len(), 1)
    });

    fs::write(&file, "外部更新").expect("应修改文件");
    project.update(cx, |project, cx| {
        project.process_fs_events(
            vec![PathEvent::new(file.clone(), Some(PathEventKind::Changed)).unwrap()],
            cx,
        );
    });
    cx.run_until_parked();
    let snapshot = cx.read_entity(&buffer, |buffer, _| buffer.text_snapshot());
    assert_eq!(
        snapshot
            .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
            .unwrap()
            .as_str(),
        "外部更新"
    );

    drop(snapshot);
    drop(buffer);
    cx.update(|_| {});
    cx.run_until_parked();
    project.read_with(cx, |project, _| {
        assert!(
            project.external_watch_counts.is_empty(),
            "外部监听 {}，重载任务 {}，订阅 {}",
            project.external_watch_counts.len(),
            project.reload_tasks.len(),
            project.buffer_subscriptions.len()
        );
    });
}

#[gpui::test]
fn project_watches_open_file_outside_its_root(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().expect("应创建项目目录");
    let external = tempfile::tempdir().expect("应创建外部目录");
    let file = external.path().join("outside.txt");
    fs::write(&file, "初始内容").expect("应创建文件");
    let project = test_project(root.path().to_path_buf(), cx);
    let buffer = project
        .update(cx, |project, cx| project.open_buffer(&file, cx))
        .expect("应打开项目外文件");
    project.read_with(cx, |project, _| {
        assert_eq!(project.external_watch_counts.len(), 1)
    });

    fs::write(&file, "外部更新").expect("应修改文件");
    project.update(cx, |project, cx| {
        project.process_fs_events(
            vec![PathEvent::new(file.clone(), Some(PathEventKind::Changed)).unwrap()],
            cx,
        );
    });
    cx.run_until_parked();
    let snapshot = cx.read_entity(&buffer, |buffer, _| buffer.text_snapshot());
    assert_eq!(
        snapshot
            .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
            .unwrap()
            .as_str(),
        "外部更新"
    );
}

#[gpui::test]
fn external_files_share_parent_watch_until_last_document_closes(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let first_path = directory.path().join("first.txt");
    let second_path = directory.path().join("second.txt");
    fs::write(&first_path, "第一份").expect("应创建文件");
    fs::write(&second_path, "第二份").expect("应创建文件");
    let project = cx
        .new(|cx| Project::empty_with_watcher(test_languages(), Arc::new(TestWatcher::new()), cx));
    let first = project
        .update(cx, |project, cx| project.open_buffer(&first_path, cx))
        .expect("应打开第一份文件");
    let second = project
        .update(cx, |project, cx| project.open_buffer(&second_path, cx))
        .expect("应打开第二份文件");
    project.read_with(cx, |project, _| {
        assert_eq!(
            project
                .external_watch_counts
                .values()
                .copied()
                .collect::<Vec<_>>(),
            vec![2]
        );
    });

    drop(first);
    cx.update(|_| {});
    project.read_with(cx, |project, _| {
        assert_eq!(
            project
                .external_watch_counts
                .values()
                .copied()
                .collect::<Vec<_>>(),
            vec![1]
        );
    });
    drop(second);
    cx.update(|_| {});
    project.read_with(cx, |project, _| {
        assert!(project.external_watch_counts.is_empty())
    });
}

#[gpui::test]
fn external_change_waits_for_unsaved_edits_to_be_undone(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let file = directory.path().join("open.txt");
    fs::write(&file, "磁盘旧内容").expect("应创建文件");
    let project = test_project(directory.path().to_path_buf(), cx);
    let buffer = project
        .update(cx, |project, cx| project.open_buffer(&file, cx))
        .expect("应打开文件");
    buffer
        .update(cx, |buffer, cx| {
            buffer.edit(
                [Edit::insert(buffer.len_bytes(), " 本地编辑").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
        })
        .expect("应编辑文档");

    fs::write(&file, "磁盘新内容").expect("应外部修改文件");
    project.update(cx, |project, cx| {
        project.process_fs_events(
            vec![PathEvent::new(file.clone(), Some(PathEventKind::Changed)).unwrap()],
            cx,
        );
    });
    cx.run_until_parked();
    let snapshot = cx.read_entity(&buffer, |buffer, _| buffer.text_snapshot());
    assert_eq!(
        snapshot
            .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
            .unwrap()
            .as_str(),
        "磁盘旧内容 本地编辑"
    );

    buffer
        .update(cx, |buffer, cx| buffer.undo(cx))
        .expect("撤销应成功")
        .expect("应撤销本地编辑");
    cx.run_until_parked();
    let snapshot = cx.read_entity(&buffer, |buffer, _| buffer.text_snapshot());
    assert_eq!(
        snapshot
            .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
            .unwrap()
            .as_str(),
        "磁盘新内容"
    );
    assert!(!cx.read_entity(&buffer, |buffer, _| buffer.is_dirty()));
}

#[gpui::test]
fn external_change_requires_explicit_choice_before_saving(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let file = directory.path().join("open.txt");
    fs::write(&file, "磁盘旧内容").expect("应创建文件");
    let project = test_project(directory.path().to_path_buf(), cx);
    let buffer = project
        .update(cx, |project, cx| project.open_buffer(&file, cx))
        .expect("应打开文件");
    buffer
        .update(cx, |buffer, cx| {
            buffer.edit(
                [Edit::insert(buffer.len_bytes(), " 本地编辑").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
        })
        .expect("编辑应成功");

    fs::write(&file, "磁盘新内容").expect("应外部修改文件");
    project.update(cx, |project, cx| {
        project.process_fs_events(
            vec![PathEvent::new(file.clone(), Some(PathEventKind::Changed)).unwrap()],
            cx,
        );
    });
    cx.run_until_parked();
    assert!(cx.read_entity(&buffer, |buffer, _| buffer.has_disk_conflict()));

    let result = project.update(cx, |project, cx| {
        project.save_file_buffers(
            vec![(buffer.clone(), file.clone())],
            FileSaveConflictPolicy::Reject,
            cx,
        )
    });
    assert!(matches!(result, Err(BufferSaveError::Conflict(path)) if path == file));
    assert_eq!(fs::read_to_string(&file).unwrap(), "磁盘新内容");
    assert!(cx.read_entity(&buffer, |buffer, _| buffer.is_dirty()));

    project
        .update(cx, |project, cx| {
            project.save_file_buffers(
                vec![(buffer.clone(), file.clone())],
                FileSaveConflictPolicy::Overwrite,
                cx,
            )
        })
        .expect("显式覆盖应成功");
    assert_eq!(fs::read_to_string(&file).unwrap(), "磁盘旧内容 本地编辑\n");
    assert!(!cx.read_entity(&buffer, |buffer, _| buffer.has_disk_conflict()));
    assert!(!cx.read_entity(&buffer, |buffer, _| buffer.is_dirty()));
}

#[gpui::test]
fn restore_conflicted_file_reads_disk_without_replacing_the_document_entity(
    cx: &mut gpui::TestAppContext,
) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let file = directory.path().join("open.txt");
    fs::write(&file, "磁盘旧内容").expect("应创建文件");
    let project = test_project(directory.path().to_path_buf(), cx);
    let buffer = project
        .update(cx, |project, cx| project.open_buffer(&file, cx))
        .expect("应打开文件");
    buffer
        .update(cx, |buffer, cx| {
            buffer.edit(
                [Edit::insert(buffer.len_bytes(), " 本地编辑").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
        })
        .expect("编辑应成功");
    fs::write(&file, "磁盘新内容").expect("应外部修改文件");
    project.update(cx, |project, cx| {
        project.process_fs_events(
            vec![PathEvent::new(file.clone(), Some(PathEventKind::Changed)).unwrap()],
            cx,
        );
    });
    cx.run_until_parked();

    let task = project.update(cx, |project, cx| {
        project.reload_conflicted_file_buffers(vec![(buffer.clone(), file.clone())], cx)
    });
    let result = cx.foreground_executor().block_test(task);
    result.expect("还原应成功");
    let reopened = project
        .update(cx, |project, cx| project.open_buffer(&file, cx))
        .expect("应复用文档实体");
    assert_eq!(reopened, buffer);
    let snapshot = cx.read_entity(&buffer, |buffer, _| buffer.text_snapshot());
    assert_eq!(
        snapshot
            .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
            .unwrap()
            .as_str(),
        "磁盘新内容"
    );
    assert!(!cx.read_entity(&buffer, |buffer, _| buffer.is_dirty()));
    assert!(!cx.read_entity(&buffer, |buffer, _| buffer.has_disk_conflict()));
}

#[gpui::test]
fn conflict_in_a_later_source_prevents_partial_multi_file_save(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let first = directory.path().join("first.txt");
    let second = directory.path().join("second.txt");
    fs::write(&first, "第一份原内容").unwrap();
    fs::write(&second, "第二份原内容").unwrap();
    let project = test_project(directory.path().to_path_buf(), cx);
    let first_buffer = project
        .update(cx, |project, cx| project.open_buffer(&first, cx))
        .unwrap();
    let second_buffer = project
        .update(cx, |project, cx| project.open_buffer(&second, cx))
        .unwrap();
    for buffer in [&first_buffer, &second_buffer] {
        buffer
            .update(cx, |buffer, cx| {
                buffer.edit(
                    [Edit::insert(buffer.len_bytes(), " 本地编辑").unwrap()],
                    TransactionMetadata::default(),
                    cx,
                )
            })
            .unwrap();
    }
    fs::write(&second, "第二份磁盘改动").unwrap();
    second_buffer.update(cx, |buffer, cx| buffer.file_changed_on_disk(cx));

    let result = project.update(cx, |project, cx| {
        project.save_file_buffers(
            vec![
                (first_buffer.clone(), first.clone()),
                (second_buffer.clone(), second.clone()),
            ],
            FileSaveConflictPolicy::Reject,
            cx,
        )
    });
    assert!(matches!(result, Err(BufferSaveError::Conflict(path)) if path == second));
    assert_eq!(fs::read_to_string(&first).unwrap(), "第一份原内容");
    assert_eq!(fs::read_to_string(&second).unwrap(), "第二份磁盘改动");
    first_buffer.read_with(cx, |buffer, _| {
        let snapshot = buffer.text_snapshot();
        assert_eq!(
            snapshot
                .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
                .unwrap()
                .as_str(),
            "第一份原内容 本地编辑"
        );
    });
}

#[gpui::test]
fn restore_does_not_discard_edits_made_after_the_choice(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("open.txt");
    fs::write(&file, "磁盘旧内容").unwrap();
    let project = test_project(directory.path().to_path_buf(), cx);
    let buffer = project
        .update(cx, |project, cx| project.open_buffer(&file, cx))
        .unwrap();
    buffer
        .update(cx, |buffer, cx| {
            buffer.edit(
                [Edit::insert(buffer.len_bytes(), " 本地编辑").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
        })
        .unwrap();
    fs::write(&file, "磁盘新内容").unwrap();
    buffer.update(cx, |buffer, cx| buffer.file_changed_on_disk(cx));
    let task = project.update(cx, |project, cx| {
        project.reload_conflicted_file_buffers(vec![(buffer.clone(), file.clone())], cx)
    });
    buffer
        .update(cx, |buffer, cx| {
            buffer.edit(
                [Edit::insert(buffer.len_bytes(), " 后续编辑").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
        })
        .unwrap();

    assert!(cx.foreground_executor().block_test(task).is_err());
    let snapshot = cx.read_entity(&buffer, |buffer, _| buffer.text_snapshot());
    assert_eq!(
        snapshot
            .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
            .unwrap()
            .as_str(),
        "磁盘旧内容 本地编辑 后续编辑"
    );
    assert!(cx.read_entity(&buffer, |buffer, _| buffer.has_disk_conflict()));
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
fn external_removal_keeps_live_document_for_recreated_file(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let root = directory.path().to_path_buf();
    let project = test_project(root.clone(), cx);
    let file = root.join("open.txt");
    fs::write(&file, "打开内容\n").expect("应写入文件");

    let opened = project.update(cx, |project, cx| {
        project.open_buffer(&file, cx).expect("应打开文件")
    });
    cx.read_entity(&opened, |language_buffer, _| {
        let snapshot = language_buffer.text_snapshot();
        let range = TextRange::new(ByteOffset::ZERO, snapshot.len_bytes()).expect("全文范围应有效");
        assert_eq!(snapshot.slice_text(range).unwrap().as_str(), "打开内容\n");
    });

    // 文件暂时消失时仍保留存活文档的路径身份；重建后更新同一实体。
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

    fs::write(&file, "重建内容\n").expect("应重建文件");
    project.update(cx, |project, cx| {
        project.process_fs_events(
            vec![PathEvent::new(file.clone(), Some(PathEventKind::Created)).unwrap()],
            cx,
        );
    });
    cx.run_until_parked();
    let reopened = project
        .update(cx, |project, cx| project.open_buffer(&file, cx))
        .expect("应复用文档");
    assert_eq!(opened, reopened);
    let snapshot = cx.read_entity(&opened, |buffer, _| buffer.text_snapshot());
    assert_eq!(
        snapshot
            .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
            .unwrap()
            .as_str(),
        "重建内容\n"
    );
}

#[gpui::test]
fn removal_event_after_atomic_replacement_reloads_live_document(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let file = directory.path().join("open.txt");
    fs::write(&file, "旧内容").expect("应创建文件");
    let project = test_project(directory.path().to_path_buf(), cx);
    let buffer = project
        .update(cx, |project, cx| project.open_buffer(&file, cx))
        .expect("应打开文件");

    fs::write(&file, "新内容").expect("替换事件交付时文件应已存在");
    project.update(cx, |project, cx| {
        project.process_fs_events(
            vec![PathEvent::new(file.clone(), Some(PathEventKind::Removed)).unwrap()],
            cx,
        );
    });
    cx.run_until_parked();

    let snapshot = cx.read_entity(&buffer, |buffer, _| buffer.text_snapshot());
    assert_eq!(
        snapshot
            .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
            .unwrap()
            .as_str(),
        "新内容"
    );
    let reopened = project
        .update(cx, |project, cx| project.open_buffer(&file, cx))
        .expect("应复用原文档");
    assert_eq!(buffer, reopened);
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
            project.save_file_buffers(
                vec![(buffer.clone(), file.clone())],
                FileSaveConflictPolicy::Reject,
                cx,
            )
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
            project.save_file_buffers(
                vec![(language_buffer.clone(), file.clone())],
                FileSaveConflictPolicy::Reject,
                cx,
            )
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
        .expect("保存整理应可撤销");
    language_buffer.read_with(cx, |language_buffer, _| {
        let snapshot = language_buffer.text_snapshot();
        assert_eq!(
            snapshot
                .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
                .expect("应读取完整文本")
                .as_str(),
            "原内容 + 修改"
        );
    });
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
