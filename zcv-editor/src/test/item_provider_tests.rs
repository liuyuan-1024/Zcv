use std::{fs, process::Command, slice, sync::Arc};

use gpui::{Entity, TestAppContext};
use zcv_buffer_diff::{DiffHunkKind, DiffHunkStaging};
use zcv_git::{FileStatus, GitRevision};
use zcv_language::LanguageRegistry;
use zcv_multi_buffer::ResolvedDiffHunk;
use zcv_path::AbsolutePathBuf;
use zcv_settings::{SettingsStore, UserSettings};

use super::*;

#[gpui::test]
fn provider_supports_files_without_extension(cx: &mut TestAppContext) {
    cx.read(|cx| {
        // .gitignore / Makefile 等无扩展名文件必须可打开（文本兜底）。
        assert!(TextFileProvider.supports(Path::new(".gitignore"), cx));
        assert!(TextFileProvider.supports(Path::new("Makefile"), cx));
        assert!(TextFileProvider.supports(Path::new("src/main.rs"), cx));
    });
}
/// 测试用：走生产路径（快照 `resolved_diff_hunks`）读取编辑器 diff 视图。
fn editor_diff_hunks(editor: &Entity<Editor>, cx: &mut TestAppContext) -> Vec<ResolvedDiffHunk> {
    editor.update(cx, |editor, cx| {
        editor.multi_buffer().update(cx, |buffer, cx| {
            buffer
                .snapshot(cx)
                .resolved_diff_hunks()
                .into_iter()
                .map(|(_, resolved)| resolved)
                .collect::<Vec<_>>()
        })
    })
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
    fs::write(root.join("tracked.txt"), "原始内容\n").expect("应写入已跟踪文件");
    fs::write(root.join(".gitignore"), "ignored.txt\n").expect("应写入忽略规则");
    run_git(&root, &["add", "tracked.txt", ".gitignore"]);
    run_git(&root, &["commit", "-q", "-m", "initial"]);
    let project = cx.new(|cx| {
        Project::new(
            vec![AbsolutePathBuf::canonicalize(&root).unwrap()],
            Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    cx.run_until_parked();
    (directory, root, project)
}

fn editor_for_file(
    path: &Path,
    project: &Entity<Project>,
    cx: &mut TestAppContext,
) -> Entity<Editor> {
    let task = cx.update(|cx| TextFileProvider.open_item(path.to_path_buf(), project.clone(), cx));
    let item = futures_lite::future::block_on(task).expect("应打开文本文件");
    cx.read(|cx| item.act_as::<Editor>(cx).expect("文本文件应打开为编辑器"))
}

#[gpui::test]
fn untracked_file_uses_the_same_git_projection_before_and_after_staging(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let (_directory, root, project) = project_with_repo(cx);
    let path = root.join("new.txt");
    fs::write(&path, "新增第一行\n新增第二行\n").expect("应写入新文件");
    let store = project.read_with(cx, |project, _| project.git_store());
    store.update(cx, |store, cx| {
        store.refresh_statuses_for_paths(slice::from_ref(&path), cx)
    });
    cx.run_until_parked();
    assert_eq!(
        store.read_with(cx, |store, _| store
            .status_for_path(&path)
            .map(|entry| entry.status)),
        Some(FileStatus::Untracked)
    );

    let editor = editor_for_file(&path, &project, cx);
    cx.run_until_parked();
    let hunks = editor_diff_hunks(&editor, cx);
    assert_eq!(hunks.len(), 1, "未跟踪文件应显示整文件新增 hunk");
    assert_eq!(hunks[0].hunk.kind, DiffHunkKind::Added);
    assert_eq!(hunks[0].hunk.staging, DiffHunkStaging::Unstaged);

    editor.update(cx, |editor, cx| editor.toggle_diff_hunk_at(0, cx));
    assert!(editor_diff_hunks(&editor, cx)[0].expanded);

    run_git(&root, &["add", "new.txt"]);
    store.update(cx, |store, cx| {
        store.refresh_statuses_for_paths(slice::from_ref(&path), cx)
    });
    cx.run_until_parked();
    let hunks = editor_diff_hunks(&editor, cx);
    assert_eq!(hunks.len(), 1, "暂存后应保留同一条编辑器差异投影");
    assert_eq!(hunks[0].hunk.kind, DiffHunkKind::Added);
    assert_eq!(hunks[0].hunk.staging, DiffHunkStaging::Staged);
    assert!(hunks[0].expanded, "index 更新应保留用户展开的差异块");
}

#[gpui::test]
fn clean_tracked_file_and_ignored_file_have_distinct_diff_eligibility(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let (_directory, root, project) = project_with_repo(cx);
    let clean_path = root.join("tracked.txt");
    let ignored_path = root.join("ignored.txt");
    fs::write(&ignored_path, "忽略内容\n").expect("应写入忽略文件");
    let store = project.read_with(cx, |project, _| project.git_store());
    store.update(cx, |store, cx| {
        store.refresh_statuses_for_paths(slice::from_ref(&ignored_path), cx)
    });
    cx.run_until_parked();
    assert!(store.read_with(cx, |store, _| store.status_for_path(&clean_path).is_none()));
    assert!(store.read_with(cx, |store, _| store.has_diff_context_for_path(&clean_path)));
    assert!(!store.read_with(cx, |store, _| {
        store.has_diff_context_for_path(&ignored_path)
    }));

    let editor = editor_for_file(&clean_path, &project, cx);
    cx.run_until_parked();
    assert!(editor_diff_hunks(&editor, cx).is_empty());
    assert!(store.read_with(cx, |store, _| {
        store.revision_document_loaded(GitRevision::Head, &clean_path)
    }));
    editor.update(cx, |editor, cx| editor.set_text("未保存的修改\n", cx));
    cx.run_until_parked();
    let hunks = editor_diff_hunks(&editor, cx);
    assert_eq!(hunks.len(), 1, "干净文件的未保存编辑应产生差异");
    assert_eq!(hunks[0].hunk.kind, DiffHunkKind::Modified);

    let ignored_editor = editor_for_file(&ignored_path, &project, cx);
    cx.run_until_parked();
    assert!(ignored_editor.read_with(cx, |editor, cx| editor.diff_paths(cx).is_empty()));
}

#[gpui::test]
fn unsupported_index_is_not_treated_as_missing_and_recovers_after_staging(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let (_directory, root, project) = project_with_repo(cx);
    let path = root.join("tracked.txt");
    fs::write(&path, b"binary\0index").unwrap();
    run_git(&root, &["add", "tracked.txt"]);
    fs::write(&path, "工作区文本\n").unwrap();

    let editor = editor_for_file(&path, &project, cx);
    cx.run_until_parked();
    assert!(editor.read_with(cx, |editor, cx| editor.diff_paths(cx).is_empty()));

    run_git(&root, &["add", "tracked.txt"]);
    let store = project.read_with(cx, |project, _| project.git_store());
    store.update(cx, |store, cx| {
        store.refresh_statuses_for_paths(slice::from_ref(&path), cx)
    });
    cx.run_until_parked();
    let hunks = editor_diff_hunks(&editor, cx);
    assert_eq!(hunks.len(), 1);
    assert_eq!(hunks[0].hunk.staging, DiffHunkStaging::Staged);
}
