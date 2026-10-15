use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use gpui::{AppContext as _, Entity, TestAppContext};
use zcv_buffer_diff::{DiffHunkKind, DiffHunkStaging};
use zcv_editor::Editor;
use zcv_fs_watch::{FsEventStream, FsWatcher, Watcher};
use zcv_git::{FileStatus, GitRevision};
use zcv_language::LanguageRegistry;
use zcv_multi_buffer::MultiBuffer;
use zcv_project::Project;

use super::inject_editor_diff;

struct PassiveWatcher(FsWatcher);

impl Watcher for PassiveWatcher {
    fn add(&self, _path: &Path) -> anyhow::Result<()> {
        Ok(())
    }

    fn remove(&self, _path: &Path) -> anyhow::Result<()> {
        Ok(())
    }

    fn watch(&self, latency: std::time::Duration) -> FsEventStream {
        self.0.watch(latency)
    }
}

fn run_git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("应执行 Git 命令");
    assert!(
        output.status.success(),
        "Git 命令失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn project_with_repo(cx: &mut TestAppContext) -> (tempfile::TempDir, PathBuf, Entity<Project>) {
    let directory = tempfile::tempdir().expect("应创建临时仓库");
    let root = directory.path().canonicalize().expect("应规范化仓库路径");
    run_git(&root, &["init", "-q", "-b", "master"]);
    run_git(&root, &["config", "user.email", "test@example.com"]);
    run_git(&root, &["config", "user.name", "Test User"]);
    std::fs::write(root.join("tracked.txt"), "原始内容\n").expect("应写入已跟踪文件");
    std::fs::write(root.join(".gitignore"), "ignored.txt\n").expect("应写入忽略规则");
    run_git(&root, &["add", "tracked.txt", ".gitignore"]);
    run_git(&root, &["commit", "-q", "-m", "initial"]);
    let watcher: Arc<dyn Watcher> = Arc::new(PassiveWatcher(FsWatcher::new()));
    let project = cx.new(|cx| {
        Project::new_with_watcher(root.clone(), watcher, Arc::new(LanguageRegistry::new()), cx)
            .expect("测试项目应创建成功")
    });
    cx.run_until_parked();
    (directory, root, project)
}

fn editor_for_file(
    path: &Path,
    project: &Entity<Project>,
    cx: &mut TestAppContext,
) -> Entity<Editor> {
    let source = project
        .update(cx, |project, cx| project.open_buffer(path, cx))
        .expect("文本文件应打开成功");
    let multi_buffer = cx.new(|cx| MultiBuffer::singleton(source, cx));
    cx.new(|cx| Editor::for_multi_buffer(multi_buffer, cx))
}

#[gpui::test]
fn untracked_file_uses_the_same_git_projection_before_and_after_staging(cx: &mut TestAppContext) {
    let (_directory, root, project) = project_with_repo(cx);
    let path = root.join("new.txt");
    std::fs::write(&path, "新增第一行\n新增第二行\n").expect("应写入新文件");
    let store = project.read_with(cx, |project, _| project.git_store());
    store.update(cx, |store, cx| {
        store.refresh_statuses_for_paths(std::slice::from_ref(&path), cx)
    });
    cx.run_until_parked();
    assert_eq!(
        store.read_with(cx, |store, _| store
            .status_for_path(&path)
            .map(|entry| entry.status)),
        Some(FileStatus::Untracked)
    );

    let editor = editor_for_file(&path, &project, cx);
    cx.update(|cx| inject_editor_diff(&editor, &path, &project, cx));
    cx.run_until_parked();
    let hunks = editor.read_with(cx, |editor, cx| editor.diff_hunks(cx));
    assert_eq!(hunks.len(), 1, "未跟踪文件应显示整文件新增 hunk");
    assert_eq!(hunks[0].kind, DiffHunkKind::Added);
    assert_eq!(hunks[0].staging, DiffHunkStaging::Unstaged);

    run_git(&root, &["add", "new.txt"]);
    store.update(cx, |store, cx| {
        store.refresh_statuses_for_paths(std::slice::from_ref(&path), cx)
    });
    cx.run_until_parked();
    let hunks = editor.read_with(cx, |editor, cx| editor.diff_hunks(cx));
    assert_eq!(hunks.len(), 1, "暂存后应保留同一条编辑器差异投影");
    assert_eq!(hunks[0].kind, DiffHunkKind::Added);
    assert_eq!(hunks[0].staging, DiffHunkStaging::Staged);
}

#[gpui::test]
fn clean_tracked_file_and_ignored_file_have_distinct_diff_eligibility(cx: &mut TestAppContext) {
    let (_directory, root, project) = project_with_repo(cx);
    let clean_path = root.join("tracked.txt");
    let ignored_path = root.join("ignored.txt");
    std::fs::write(&ignored_path, "忽略内容\n").expect("应写入忽略文件");
    let store = project.read_with(cx, |project, _| project.git_store());
    store.update(cx, |store, cx| {
        store.refresh_statuses_for_paths(std::slice::from_ref(&ignored_path), cx)
    });
    cx.run_until_parked();
    assert!(store.read_with(cx, |store, _| store.status_for_path(&clean_path).is_none()));
    assert!(store.read_with(cx, |store, _| store.has_diff_context_for_path(&clean_path)));
    assert!(!store.read_with(cx, |store, _| {
        store.has_diff_context_for_path(&ignored_path)
    }));

    let editor = editor_for_file(&clean_path, &project, cx);
    cx.update(|cx| inject_editor_diff(&editor, &clean_path, &project, cx));
    cx.run_until_parked();
    assert!(editor.read_with(cx, |editor, cx| editor.diff_hunks(cx).is_empty()));
    assert!(store.read_with(cx, |store, _| {
        store.revision_document_loaded(GitRevision::Head, &clean_path)
    }));
    editor.update(cx, |editor, cx| editor.set_text("未保存的修改\n", cx));
    cx.run_until_parked();
    let hunks = editor.read_with(cx, |editor, cx| editor.diff_hunks(cx));
    assert_eq!(hunks.len(), 1, "干净文件的未保存编辑应产生差异");
    assert_eq!(hunks[0].kind, DiffHunkKind::Modified);

    let ignored_editor = editor_for_file(&ignored_path, &project, cx);
    cx.update(|cx| inject_editor_diff(&ignored_editor, &ignored_path, &project, cx));
    assert!(ignored_editor.read_with(cx, |editor, cx| editor.diff_paths(cx).is_empty()));
}
