use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use gpui::{AppContext as _, TestAppContext};

use zcv_fs_watch::{FsEventStream, FsWatcher, Watcher};
use zcv_language::{LanguageBuffer, LanguageRegistry};
use zcv_multi_buffer::ExcerptDiffKind;
use zcv_text::{Buffer, BufferConfig, Edit, Line, TransactionMetadata};

#[test]
fn includes_matches_section_membership_for_every_status() {
    // 冲突条目只属于冲突组，不进入暂存/未暂存组。
    assert!(!ProjectDiffKind::Staged.includes(FileStatus::Unmerged));
    assert!(!ProjectDiffKind::Unstaged.includes(FileStatus::Unmerged));
    assert!(ProjectDiffKind::Conflict.includes(FileStatus::Unmerged));

    assert!(!ProjectDiffKind::Staged.includes(FileStatus::Untracked));
    assert!(ProjectDiffKind::Unstaged.includes(FileStatus::Untracked));

    assert!(!ProjectDiffKind::Staged.includes(FileStatus::Ignored));
    assert!(!ProjectDiffKind::Unstaged.includes(FileStatus::Ignored));
    assert!(!ProjectDiffKind::Conflict.includes(FileStatus::Ignored));

    let staged = FileStatus::Tracked {
        index_status: StatusCode::Modified,
        worktree_status: StatusCode::Unmodified,
    };
    assert!(ProjectDiffKind::Staged.includes(staged));
    assert!(!ProjectDiffKind::Unstaged.includes(staged));

    let unstaged = FileStatus::Tracked {
        index_status: StatusCode::Unmodified,
        worktree_status: StatusCode::Modified,
    };
    assert!(!ProjectDiffKind::Staged.includes(unstaged));
    assert!(ProjectDiffKind::Unstaged.includes(unstaged));

    // 部分暂存：两组同时出现。
    let partial = FileStatus::Tracked {
        index_status: StatusCode::Added,
        worktree_status: StatusCode::Deleted,
    };
    assert!(ProjectDiffKind::Staged.includes(partial));
    assert!(ProjectDiffKind::Unstaged.includes(partial));
}

#[test]
fn project_diff_persistence_state_keeps_group_and_active_path() {
    let (kind, active_path) = project_diff_state(&serde_json::json!({
        "kind": "unstaged",
        "active_path": "src/main.rs",
    }))
    .expect("有效的项目差异状态应能恢复");
    assert_eq!(kind, ProjectDiffKind::Unstaged);
    assert_eq!(active_path, Some(PathBuf::from("src/main.rs")));

    assert!(project_diff_state(&serde_json::json!({ "kind": "unknown" })).is_err());
}

/// 把列（Unicode scalar 计数）钳制到文本中指定行的有效长度（行 0-based）。
///
/// Deleted 片段换算出的列来自 Git 修订行，工作区对应行可能因修改而变短，越界列会导致行列导航失败，必须钳制到行尾。
fn clamp_column_to_line(text: &Snapshot, line: usize, column: usize) -> usize {
    let line = line.min(text.line_count().saturating_sub(1));
    let line_chars = text
        .line_content(Line::new(line), None)
        .map_or(0, |content| content.len_chars());
    column.min(line_chars)
}

use super::*;

/// 项目差异测试使用的无事件监听器，避免真实 OS 事件唤醒 GPUI 测试调度器。
struct PassiveWatcher {
    watcher: FsWatcher,
}

impl PassiveWatcher {
    fn new() -> Self {
        Self {
            watcher: FsWatcher::new(),
        }
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
        self.watcher.events()
    }
}

fn test_project(root: PathBuf, cx: &mut TestAppContext) -> Entity<Project> {
    let watcher: Arc<dyn Watcher> = Arc::new(PassiveWatcher::new());
    cx.new(|cx| Project::new_with_watcher(root, watcher, Arc::new(LanguageRegistry::new()), cx))
}

fn canonical_root(path: &Path) -> PathBuf {
    AbsolutePathBuf::canonicalize(path)
        .expect("应规范化仓库路径")
        .into_path_buf()
}

/// 测试辅助：按工作区源与 base 全文预创建普通编辑器 diff 注入项。
fn plain_diff_file(
    working: Entity<LanguageBuffer>,
    base_text: &str,
    path: PathBuf,
    cx: &mut Context<Editor>,
) -> DiffFile {
    let registry = working.read(cx).language_registry();
    let diff = cx.new(|cx| {
        BufferDiff::new(
            BufferDiffInput {
                working,
                base_text: Some(base_text.to_owned()),
                index_text: None,
                path: path.clone(),
                language_registry: registry,
                key: 0,
                operations: None,
            },
            cx,
        )
    });
    DiffFile {
        diff,
        display_path: path,
        context_lines: None,
    }
}

#[gpui::test]
fn empty_project_diff_renders_blank_focusable_view(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let project = test_project(directory.path().to_path_buf(), cx);
    let (view, cx) =
        cx.add_window_view(move |_, cx| ProjectDiffView::new(ProjectDiffKind::Staged, project, cx));
    cx.run_until_parked();
    let _ = cx.refresh();
    cx.update(|_, _| {});
    cx.run_until_parked();

    let focus = cx.read_entity(&view, |view, cx| {
        assert!(view.is_empty(cx));
        assert!(view.focus_handle(cx) == view.empty_focus);
        assert!(
            view.editor.read(cx).cursor_text(cx).is_empty(),
            "无变更文件时底栏不应显示光标行列"
        );
        view.focus_handle(cx)
    });
    assert!(
        cx.debug_bounds("empty-project-diff-view").is_some(),
        "空项目差异应渲染纯空白容器"
    );
    assert!(
        cx.debug_bounds("project-diff-view").is_none(),
        "空项目差异不应渲染 Editor"
    );
    cx.update(|window, cx| window.focus(&focus, cx));
    cx.update(|window, _| {
        assert!(focus.is_focused(window), "空白区域仍应能持有 Item 焦点");
    });
}

#[gpui::test]
fn project_diff_keeps_hunk_interest_while_its_multibuffer_is_empty(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时仓库");
    let root = canonical_root(directory.path());
    run_in(&root, &["git", "init", "-q", "-b", "master"]);
    run_in(&root, &["git", "config", "user.email", "test@example.com"]);
    run_in(&root, &["git", "config", "user.name", "Test User"]);
    let path = root.join("tracked.txt");
    std::fs::write(&path, "line0\nline1\nline2\n原内容\nline4\nline5\nline6\n")
        .expect("应创建文件");
    run_in(&root, &["git", "add", "tracked.txt"]);
    run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
    std::fs::write(&path, "line0\nline1\nline2\n新内容\nline4\nline5\nline6\n")
        .expect("应修改文件");

    let project = test_project(root.clone(), cx);
    let view = cx.new(|cx| ProjectDiffView::new(ProjectDiffKind::Unstaged, project, cx));

    // ProjectDiffView 创建时 MultiBuffer 仍为空，但应立即向 GitStore 声明文件 hunk 需求。
    cx.run_until_parked();
    cx.run_until_parked();

    cx.update_entity(&view, |view, cx| {
        let multi_buffer = view.multi_buffer(cx).expect("项目差异应提供组合文档");
        let text = String::from_utf8(
            multi_buffer.update(cx, |buffer, cx| buffer.snapshot(cx).text_bytes()),
        )
        .expect("投影文本应为 UTF-8");
        assert_eq!(text, "line1\nline2\n原内容\n新内容\nline4\nline5\n");
    });
}

/// 端到端：git 删除文件中间一行（第 17 行，1-based）后，展开的被删行必须投影到其原始位置（组合第 17 行），上下文行顺序不重排。
#[gpui::test]
fn deleted_middle_row_projects_to_its_original_position(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时仓库");
    let root = canonical_root(directory.path());
    run_in(&root, &["git", "init", "-q", "-b", "master"]);
    run_in(&root, &["git", "config", "user.email", "test@example.com"]);
    run_in(&root, &["git", "config", "user.name", "Test User"]);
    let path = root.join("readme.md");
    let original = (1..=20)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>();
    std::fs::write(&path, original.join("\n")).expect("应创建文件");
    run_in(&root, &["git", "add", "."]);
    run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
    // 删除第 17 行（1-based）。
    let mut changed = original.clone();
    changed.remove(16);
    std::fs::write(&path, changed.join("\n")).expect("应写入删除后的文件");

    let project = test_project(root.clone(), cx);
    let view = cx.new(|cx| ProjectDiffView::new(ProjectDiffKind::Unstaged, project, cx));
    cx.run_until_parked();
    cx.run_until_parked();

    // 默认展开（上下文裁剪 ±2 行）：被删行（line 17）显示在 line 16 之后、line 18 之前，即其原始位置，上下文行顺序不重排。
    cx.update_entity(&view, |view, cx| {
        let text = String::from_utf8(
            view.multi_buffer
                .update(cx, |buffer, cx| buffer.snapshot(cx).text_bytes()),
        )
        .expect("投影应为 UTF-8");
        let text_lines = text.split('\n').collect::<Vec<_>>();
        assert_eq!(
            text_lines.len(),
            6,
            "展开后应显示 hunk 上下文（±2 行）+ 旧侧行"
        );
        assert_eq!(text_lines[1], "line 16", "上下文第 16 行顺序保持");
        assert_eq!(
            text_lines[2], "line 17",
            "被删行应投影到 line 16 之后（原始位置）"
        );
        assert_eq!(text_lines[3], "line 18", "被删行后的行顺序保持");
    });

    // 折叠删除块：删除点锚定在 0-based 16 行（组合 16/17 行边界，line 18 行首）。
    cx.update_entity(&view, |view, cx| {
        let editor = view.editor.clone();
        editor.update(cx, |editor, cx| editor.toggle_diff_hunk_at(0, cx));
    });
    cx.run_until_parked();
    cx.update_entity(&view, |view, cx| {
        let text = String::from_utf8(
            view.multi_buffer
                .update(cx, |buffer, cx| buffer.snapshot(cx).text_bytes()),
        )
        .expect("投影应为 UTF-8");
        let text_lines = text.split('\n').collect::<Vec<_>>();
        assert_eq!(
            text_lines.len(),
            6,
            "折叠后应显示 hunk 上下文（±2 行）+ 删除点占位行：{text:?}"
        );
        assert!(!text_lines.contains(&"line 17"), "折叠后旧侧行应消失");
        assert_eq!(text_lines[1], "line 16", "折叠后第 16 行保持");
        assert_eq!(text_lines[2], "", "折叠后删除点占位行（原 line 17 位置）");
        assert_eq!(text_lines[3], "line 18", "折叠后原第 18 行紧跟删除点占位行");
        // 折叠删除块保留一个 hunk（显示坐标为组合坐标，不在此断言源行号）。
        let hunks = view.multi_buffer.read(cx).diff_hunks().to_vec();
        assert_eq!(hunks.len(), 1, "应保留一个删除 hunk");
    });
}

#[gpui::test]
fn git_status_drives_one_ordered_excerpt_per_changed_file(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时仓库");
    let root = canonical_root(directory.path());
    run_in(&root, &["git", "init", "-q", "-b", "master"]);
    run_in(&root, &["git", "config", "user.email", "test@example.com"]);
    run_in(&root, &["git", "config", "user.name", "Test User"]);
    std::fs::write(root.join("deleted.txt"), "将被删除\n").expect("应创建文件");
    std::fs::write(
        root.join("modified.txt"),
        "line0\nline1\nline2\nline3\n修改前\nline5\nline6\nline7\n",
    )
    .expect("应创建文件");
    run_in(&root, &["git", "add", "."]);
    run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
    std::fs::remove_file(root.join("deleted.txt")).expect("应删除文件");
    std::fs::write(
        root.join("modified.txt"),
        "line0\nline1\nline2\nline3\n修改后\nline5\nline6\nline7\n",
    )
    .expect("应修改文件");
    std::fs::write(root.join("untracked.txt"), "新增\n").expect("应创建未跟踪文件");

    let project = test_project(root.clone(), cx);
    let view = cx.new(|cx| ProjectDiffView::new(ProjectDiffKind::Unstaged, project, cx));
    cx.run_until_parked();
    cx.run_until_parked();

    let (paths, text) = cx.update_entity(&view, |view, cx| {
        let snapshot = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        let paths = snapshot
            .excerpt_boundaries()
            .map(|boundary| {
                boundary
                    .next()
                    .path()
                    .file_name()
                    .expect("变更应有文件名")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        (paths, String::from_utf8(snapshot.text_bytes()).unwrap())
    });
    assert_eq!(paths, vec!["deleted.txt", "modified.txt", "untracked.txt"]);
    assert_eq!(
        text,
        "将被删除\nline2\nline3\n修改前\n修改后\nline5\nline6\n新增\n"
    );
}

/// 回归：从 Deleted 片段打开文件时，必须换算到工作区文件中的真实行列，而不是把 Git 修订文本的坐标直接套到工作区文件上。
#[gpui::test]
fn deleted_excerpt_maps_to_working_tree_hunk_position(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时仓库");
    let root = canonical_root(directory.path());
    run_in(&root, &["git", "init", "-q", "-b", "master"]);
    run_in(&root, &["git", "config", "user.email", "test@example.com"]);
    run_in(&root, &["git", "config", "user.name", "Test User"]);
    let modified_path = root.join("modified.txt");
    std::fs::write(
        &modified_path,
        "line0\nline1\nline2\nline3\n修改前\nline5\nline6\nline7\n",
    )
    .expect("应创建文件");
    std::fs::write(root.join("removed.txt"), "将被删除\n").expect("应创建文件");
    run_in(&root, &["git", "add", "."]);
    run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
    // 工作区：修改第 5 行（0-based 4），并删除 removed.txt。
    std::fs::write(
        &modified_path,
        "line0\nline1\nline2\nline3\n修改后\nline5\nline6\nline7\n",
    )
    .expect("应修改文件");
    std::fs::remove_file(root.join("removed.txt")).expect("应删除文件");

    let project = test_project(root.clone(), cx);
    let view = cx.new(|cx| ProjectDiffView::new(ProjectDiffKind::Unstaged, project, cx));
    cx.run_until_parked();
    cx.run_until_parked();

    // 工作区文件文本（与磁盘内容一致），供换算钳制行列。
    let working_text = Buffer::from_text(
        "line0\nline1\nline2\nline3\n修改后\nline5\nline6\nline7\n".to_owned(),
        BufferConfig::default(),
    )
    .expect("测试 Buffer 应能创建")
    .snapshot();

    cx.update_entity(&view, |view, cx| {
        let snapshot = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        let mut excerpts = snapshot.excerpts();
        // 修改行的 Deleted 片段：首行（旧侧 "修改前"）→ 工作区第 5 行（0-based 4）。
        let modified_excerpt = excerpts
            .find(|excerpt| {
                excerpt.path() == modified_path
                    && excerpt.diff_kind() == Some(ExcerptDiffKind::Deleted)
            })
            .expect("修改行应有 Deleted 片段");
        let location = ExcerptLocation {
            path: modified_path.clone(),
            source_range: modified_excerpt.source_range(),
        };
        let target = view
            .deleted_navigation_target(&location, &working_text, cx)
            .expect("Deleted 片段应能换算到工作区行列");
        assert_eq!(target.0, modified_path);
        assert_eq!(target.1, 4, "修改行映射到工作区第 5 行（0-based 4）");
        assert_eq!(target.2, 0);
        // 行内位置：旧行 "修改前" 第 2 个字符（逻辑列 1）→ 工作区同列。
        let inner_location = ExcerptLocation {
            path: modified_path.clone(),
            source_range: TextRange::new(ByteOffset::new(27), ByteOffset::new(34))
                .expect("旧行内范围"),
        };
        let inner_target = view
            .deleted_navigation_target(&inner_location, &working_text, cx)
            .expect("Deleted 片段行内位置应能换算");
        assert_eq!(
            (inner_target.1, inner_target.2),
            (4, 1),
            "修订行内列应映射到工作区同列"
        );
        assert_eq!(
            modified_excerpt.source_range(),
            TextRange::new(ByteOffset::new(24), ByteOffset::new(34),).expect("旧侧第 5 行范围"),
            "夹具应让 Deleted 片段正好覆盖被修改的旧行（含行尾换行）"
        );
        // 整文件删除：纯删除 hunk 的 range 为空，锚定到变更块起点（0-based 0）。
        let removed_excerpt = excerpts
            .find(|excerpt| {
                excerpt.path().file_name().and_then(|name| name.to_str()) == Some("removed.txt")
                    && excerpt.diff_kind() == Some(ExcerptDiffKind::Deleted)
            })
            .expect("删除文件应有 Deleted 片段");
        let removed_location = ExcerptLocation {
            path: removed_excerpt.path().to_path_buf(),
            source_range: removed_excerpt.source_range(),
        };
        // 已删除文件的工作区文本为空。
        let empty_text = Buffer::from_text(String::new(), BufferConfig::default())
            .expect("空 Buffer 应能创建")
            .snapshot();
        let removed_target = view
            .deleted_navigation_target(&removed_location, &empty_text, cx)
            .expect("删除片段应锚定到变更块起点");
        assert_eq!(removed_target.1, 0);
        assert_eq!(removed_target.2, 0);
    });
}

#[test]
fn clamp_column_to_line_caps_at_line_length() {
    let snapshot = Buffer::from_text(
        "abc\n一个很长的中文行\n".to_owned(),
        BufferConfig::default(),
    )
    .expect("测试 Buffer 应能创建")
    .snapshot();
    assert_eq!(clamp_column_to_line(&snapshot, 0, 0), 0);
    assert_eq!(clamp_column_to_line(&snapshot, 0, 99), 3, "列应钳制到行尾");
    assert_eq!(clamp_column_to_line(&snapshot, 1, 1), 1);
    assert_eq!(
        clamp_column_to_line(&snapshot, 1, 99),
        8,
        "中文行按字符计数钳制"
    );
    assert_eq!(
        clamp_column_to_line(&snapshot, 99, 5),
        0,
        "越界行钳制到最后一行"
    );
}

#[gpui::test]
fn partially_staged_file_has_distinct_staged_and_unstaged_views(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时仓库");
    let root = canonical_root(directory.path());
    run_in(&root, &["git", "init", "-q", "-b", "master"]);
    run_in(&root, &["git", "config", "user.email", "test@example.com"]);
    run_in(&root, &["git", "config", "user.name", "Test User"]);
    let path = root.join("partial.txt");
    let original = (0..12)
        .map(|line| format!("line{line}"))
        .collect::<Vec<_>>();
    std::fs::write(&path, format!("{}\n", original.join("\n"))).expect("应创建文件");
    run_in(&root, &["git", "add", "partial.txt"]);
    run_in(&root, &["git", "commit", "-q", "-m", "initial"]);

    let mut staged = original.clone();
    staged[2] = "已暂存内容".into();
    std::fs::write(&path, format!("{}\n", staged.join("\n"))).expect("应写入暂存版本");
    run_in(&root, &["git", "add", "partial.txt"]);

    let mut worktree = staged;
    worktree[9] = "未暂存内容".into();
    std::fs::write(&path, format!("{}\n", worktree.join("\n"))).expect("应写入工作区版本");

    let project = test_project(root.clone(), cx);
    let staged_view =
        cx.new(|cx| ProjectDiffView::new(ProjectDiffKind::Staged, project.clone(), cx));
    let unstaged_view = cx.new(|cx| ProjectDiffView::new(ProjectDiffKind::Unstaged, project, cx));
    cx.run_until_parked();
    cx.run_until_parked();
    cx.run_until_parked();

    let staged_text = cx.update_entity(&staged_view, |view, cx| {
        String::from_utf8(
            view.multi_buffer
                .update(cx, |buffer, cx| buffer.snapshot(cx).text_bytes()),
        )
        .expect("已暂存投影应为 UTF-8")
    });
    let unstaged_text = cx.update_entity(&unstaged_view, |view, cx| {
        String::from_utf8(
            view.multi_buffer
                .update(cx, |buffer, cx| buffer.snapshot(cx).text_bytes()),
        )
        .expect("未暂存投影应为 UTF-8")
    });

    assert!(staged_text.contains("已暂存内容"));
    assert!(!staged_text.contains("未暂存内容"));
    assert!(unstaged_text.contains("未暂存内容"));
    assert!(!unstaged_text.contains("line2"));
    cx.read_entity(&staged_view, |view, cx| {
        assert!(view.multi_buffer.read(cx).is_read_only());
        assert_eq!(view.tab_content_text(cx), "已暂存更改");
        assert_eq!(
            view.tab_icon(cx).map(|icon| icon.to_string()),
            Some("icons/lock.svg".into())
        );
    });
    cx.read_entity(&unstaged_view, |view, cx| {
        assert!(!view.multi_buffer.read(cx).is_read_only());
        assert_eq!(view.tab_content_text(cx), "未暂存更改");
        assert_eq!(
            view.tab_icon(cx).map(|icon| icon.to_string()),
            Some("icons/diff.svg".into())
        );
    });
}

#[gpui::test]
fn staging_one_hunk_rebuilds_the_projection_once_after_refresh(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时仓库");
    let root = canonical_root(directory.path());
    run_in(&root, &["git", "init", "-q", "-b", "master"]);
    run_in(&root, &["git", "config", "user.email", "test@example.com"]);
    run_in(&root, &["git", "config", "user.name", "Test User"]);
    let path = root.join("two-hunks.txt");
    let original = (0..30)
        .map(|line| format!("line{line} {}", "主".repeat(12)))
        .collect::<Vec<_>>();
    std::fs::write(&path, format!("{}\n", original.join("\n"))).expect("应创建文件");
    run_in(&root, &["git", "add", "two-hunks.txt"]);
    run_in(&root, &["git", "commit", "-q", "-m", "initial"]);

    let mut changed = original;
    changed[3] = "第一个变更块".into();
    changed[25] = "第二个变更块".into();
    std::fs::write(&path, format!("{}\n", changed.join("\n"))).expect("应修改文件");

    let project = test_project(root.clone(), cx);
    let view = cx.new(|cx| ProjectDiffView::new(ProjectDiffKind::Unstaged, project, cx));
    cx.run_until_parked();
    cx.run_until_parked();
    cx.run_until_parked();

    let (hunk_source, initial_version) = cx.update_entity(&view, |view, cx| {
        let hunks = view.multi_buffer.read(cx).diff_hunks().to_vec();
        assert_eq!(hunks.len(), 2);
        let hunk_source = view
            .diff_hunk_source_info(&hunks[0], cx)
            .expect("第一个 hunk 应有稳定源定位");
        let version = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx).version().get());
        (hunk_source, version)
    });
    view.update(cx, |view, cx| {
        view.apply_hunk_action(hunk_source, GitHunkOperation::Stage, cx)
            .expect("第一个变更块应能暂存");
    });
    cx.update_entity(&view, |view, cx| {
        let snapshot = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        let text = String::from_utf8(snapshot.text_bytes()).expect("投影应为 UTF-8");
        for offset in 0..text.len() {
            if text.is_char_boundary(offset) {
                snapshot
                    .chunk_at_byte(ByteOffset::new(offset).into())
                    .expect("暂存 hunk 后应能读取组合文本块");
            }
        }
    });
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update_entity(&view, |view, cx| {
            let snapshot = view
                .multi_buffer
                .update(cx, |buffer, cx| buffer.snapshot(cx));
            let text = String::from_utf8(snapshot.text_bytes()).expect("投影应为 UTF-8");
            for offset in 0..text.len() {
                if text.is_char_boundary(offset) {
                    snapshot
                        .chunk_at_byte(ByteOffset::new(offset).into())
                        .expect("暂存 hunk 处理中应能读取组合文本块");
                }
            }
        });
    }

    cx.update_entity(&view, |view, cx| {
        let snapshot = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        let text = String::from_utf8(snapshot.text_bytes()).expect("投影应为 UTF-8");
        assert_eq!(snapshot.version().get(), initial_version + 1);
        assert_eq!(view.multi_buffer.read(cx).diff_hunks().len(), 1);
        assert!(!text.contains("第一个变更块"));
        assert!(text.contains("第二个变更块"));
        for offset in 0..text.len() {
            if text.is_char_boundary(offset) {
                snapshot
                    .chunk_at_byte(ByteOffset::new(offset).into())
                    .expect("暂存 hunk 后每个 UTF-8 边界都应能读取组合文本块");
            }
        }
    });
}

/// Git hunk 多文件编辑器默认展开；用户折叠后刷新仍保持折叠，且映射保持一致。
#[gpui::test]
fn expanding_hunk_then_refreshing_hunks_keeps_mapping_consistent(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时仓库");
    let root = canonical_root(directory.path());
    run_in(&root, &["git", "init", "-q", "-b", "master"]);
    run_in(&root, &["git", "config", "user.email", "test@example.com"]);
    run_in(&root, &["git", "config", "user.name", "Test User"]);
    let modified_path = root.join("modified.txt");
    std::fs::write(&modified_path, "line0\nline1\nline2\nline3\nline4").expect("应创建文件");
    run_in(&root, &["git", "add", "."]);
    run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
    std::fs::write(&modified_path, "line0\n改过\nline2\nline3\nline4").expect("应修改文件");

    let project = test_project(root.clone(), cx);
    let view = cx.new(|cx| ProjectDiffView::new(ProjectDiffKind::Unstaged, project.clone(), cx));
    cx.run_until_parked();
    cx.run_until_parked();

    cx.update_entity(&view, |view, cx| {
        assert!(
            view.multi_buffer
                .read(cx)
                .diff_hunk_expanded()
                .iter()
                .all(|&expanded| expanded),
            "Git hunk 多文件编辑器应默认展开修改块"
        );
        let text = String::from_utf8(
            view.multi_buffer
                .update(cx, |buffer, cx| buffer.snapshot(cx).text_bytes()),
        )
        .expect("投影应为 UTF-8");
        assert!(text.contains("line1"), "默认展开时应包含旧侧文本");
        assert!(text.contains("改过"), "默认展开时应包含新侧文本");
    });

    let modified_buffer_id = cx.update_entity(&view, |view, cx| {
        let snapshot = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        snapshot
            .excerpts_for_path(&modified_path)
            .next()
            .expect("修改文件应有 excerpt")
            .buffer_id()
    });
    cx.update_entity(&view, |view, cx| view.set_all_files_folded(true, cx));
    cx.read_entity(&view, |view, cx| {
        assert!(
            view.editor
                .read(cx)
                .is_buffer_folded(modified_buffer_id, cx),
            "折叠全部文件后应折叠文件块"
        );
    });
    cx.update_entity(&view, |view, cx| view.set_all_files_folded(false, cx));
    cx.read_entity(&view, |view, cx| {
        assert!(
            !view
                .editor
                .read(cx)
                .is_buffer_folded(modified_buffer_id, cx),
            "展开全部文件后应展开文件块"
        );
    });

    // 用户折叠修改块。
    cx.update_entity(&view, |view, cx| {
        let editor = view.editor.clone();
        editor.update(cx, |editor, cx| editor.toggle_diff_hunk_at(0, cx));
    });
    cx.run_until_parked();
    cx.update_entity(&view, |view, cx| {
        assert!(
            !view
                .multi_buffer
                .read(cx)
                .diff_hunk_expanded()
                .iter()
                .any(|&expanded| expanded)
        );
        let text = String::from_utf8(
            view.multi_buffer
                .update(cx, |buffer, cx| buffer.snapshot(cx).text_bytes()),
        )
        .expect("投影应为 UTF-8");
        assert!(!text.contains("line1"), "折叠后旧侧文本应消失");
        assert!(text.contains("改过"), "折叠后新侧文本应保留");
    });

    // 触发 Git 状态刷新 → MultiBuffer 由 base/working 快照统一重建投影。
    project.update(cx, |project, cx| {
        let store = project.git_store();
        store.update(cx, |store, cx| {
            store.refresh_statuses_for_paths(std::slice::from_ref(&modified_path), cx);
        });
    });
    cx.run_until_parked();
    cx.run_until_parked();
    cx.read_entity(&view, |view, cx| {
        assert!(
            !view.multi_buffer.read(cx).diff_hunks().is_empty(),
            "刷新后仍应保留项目差异映射"
        );
        assert!(
            !view
                .multi_buffer
                .read(cx)
                .diff_hunk_expanded()
                .iter()
                .any(|&expanded| expanded),
            "刷新不能覆盖用户的折叠状态"
        );
    });
}

/// 复现：普通编辑器展开 hunk（singleton → excerpts）后触发 git hunks 刷新，与 ProjectDiffView 共享仓库时不应让统一投影重建 panic。
#[gpui::test]
fn plain_editor_expansion_then_git_refresh_keeps_diff_view_consistent(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时仓库");
    let root = canonical_root(directory.path());
    run_in(&root, &["git", "init", "-q", "-b", "master"]);
    run_in(&root, &["git", "config", "user.email", "test@example.com"]);
    run_in(&root, &["git", "config", "user.name", "Test User"]);
    let modified_path = root.join("modified.txt");
    std::fs::write(&modified_path, "line0\nline1\nline2\nline3\nline4\n").expect("应创建文件");
    run_in(&root, &["git", "add", "."]);
    run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
    std::fs::write(&modified_path, "line0\n改过\nline2\nline3\nline4\n").expect("应修改文件");

    let project = test_project(root.clone(), cx);
    // 普通编辑器：独立 excerpts 组合文档（整文件 excerpt，共享 LanguageBuffer 只作工作区源），与 item_provider 打开路径一致；展开修改块。
    let working = project
        .update(cx, |project, cx| project.open_buffer(&modified_path, cx))
        .expect("工作区文件应能打开");
    // 统一经 singleton 构建独立组合文档（与 item_provider 同一路径）。
    let combined = cx.new(|cx| MultiBuffer::singleton(working.clone(), cx));
    let editor = cx.new(|cx| Editor::for_multi_buffer(combined, cx));
    editor.update(cx, |editor, cx| {
        editor.set_diff_files(
            vec![plain_diff_file(
                working.clone(),
                "line0\nline1\nline2\nline3\nline4\n",
                modified_path.clone(),
                cx,
            )],
            cx,
        );
        editor.toggle_diff_hunk_at(0, cx);
    });
    // ProjectDiffView：同一仓库。
    let view = cx.new(|cx| ProjectDiffView::new(ProjectDiffKind::Unstaged, project.clone(), cx));
    cx.run_until_parked();
    cx.run_until_parked();

    // 触发 Git 状态刷新，两个视图各自重建。
    project.update(cx, |project, cx| {
        let store = project.git_store();
        store.update(cx, |store, cx| {
            store.refresh_statuses_for_paths(std::slice::from_ref(&modified_path), cx);
        });
    });
    cx.run_until_parked();
    cx.run_until_parked();
    cx.read_entity(&view, |view, cx| {
        assert!(
            !view.multi_buffer.read(cx).diff_hunks().is_empty(),
            "刷新后仍应保留项目差异映射"
        )
    });
}

/// 复现：普通编辑器展开 hunk 后编辑工作区（行数变化）再触发 git hunks 刷新，
/// ProjectDiffView 的片段映射不应 panic。
#[gpui::test]
fn expansion_edit_then_refresh_keeps_diff_view_consistent(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时仓库");
    let root = canonical_root(directory.path());
    run_in(&root, &["git", "init", "-q", "-b", "master"]);
    run_in(&root, &["git", "config", "user.email", "test@example.com"]);
    run_in(&root, &["git", "config", "user.name", "Test User"]);
    let modified_path = root.join("modified.txt");
    std::fs::write(
        &modified_path,
        "line0\nline1\nline2\nline3\nline4\nline5\nline6\nline7\n",
    )
    .expect("应创建文件");
    run_in(&root, &["git", "add", "."]);
    run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
    std::fs::write(
        &modified_path,
        "line0\n改过\nline2\nline3\nline4\nline5\nline6\nline7\n",
    )
    .expect("应修改文件");

    let project = test_project(root.clone(), cx);
    // 普通编辑器：独立 excerpts（item_provider 路径）+ 展开修改块。
    let working = project
        .update(cx, |project, cx| project.open_buffer(&modified_path, cx))
        .expect("工作区文件应能打开");
    // 统一经 singleton 构建独立组合文档（与 item_provider 同一路径）。
    let combined = cx.new(|cx| MultiBuffer::singleton(working.clone(), cx));
    let editor = cx.new(|cx| Editor::for_multi_buffer(combined, cx));
    editor.update(cx, |editor, cx| {
        editor.set_diff_files(
            vec![plain_diff_file(
                working.clone(),
                "line0\nline1\nline2\nline3\nline4\nline5\nline6\nline7\n",
                modified_path.clone(),
                cx,
            )],
            cx,
        );
        editor.toggle_diff_hunk_at(0, cx);
    });
    // ProjectDiffView：同一仓库。
    let view = cx.new(|cx| ProjectDiffView::new(ProjectDiffKind::Unstaged, project.clone(), cx));
    cx.run_until_parked();
    cx.run_until_parked();

    // 编辑工作区（删除 "改过" 行 → 行数变化）。
    working.update(cx, |working, cx| {
        working
            .edit(
                vec![Edit::delete(
                    TextRange::new(ByteOffset::new(6), ByteOffset::new(13))
                        .expect("删除范围应有效"),
                )],
                TransactionMetadata::default(),
                cx,
            )
            .expect("工作区编辑应成功");
    });
    // 触发 Git 状态刷新。
    project.update(cx, |project, cx| {
        let store = project.git_store();
        store.update(cx, |store, cx| {
            store.refresh_statuses_for_paths(std::slice::from_ref(&modified_path), cx);
        });
    });
    cx.run_until_parked();
    cx.run_until_parked();
    cx.read_entity(&view, |view, cx| {
        assert!(
            !view.multi_buffer.read(cx).diff_hunks().is_empty(),
            "刷新后仍应保留项目差异映射"
        )
    });
}

fn run_in(dir: &Path, args: &[&str]) {
    let output = Command::new(args[0])
        .args(&args[1..])
        .current_dir(dir)
        .output()
        .expect("应执行 Git 命令");
    assert!(
        output.status.success(),
        "命令 {args:?} 失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// 活动 Item 是本差异视图时工具栏显示在右侧；其他 Item 时隐藏。
#[gpui::test]
fn project_diff_toolbar_follows_active_item(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let project = test_project(directory.path().to_path_buf(), cx);
    let toolbar = cx.new(|_| ProjectDiffToolbar::new());
    let (view, cx) =
        cx.add_window_view(move |_, cx| ProjectDiffView::new(ProjectDiffKind::Staged, project, cx));
    cx.update(|window, cx| {
        let location = toolbar.update(cx, |toolbar, cx| {
            toolbar.set_active_pane_item(Some(&view as &dyn ItemHandle), window, cx)
        });
        assert_eq!(location, ToolbarItemLocation::PrimaryRight);

        let handle: &dyn ItemHandle = &view;
        assert!(
            handle.act_as::<Editor>(cx).is_some(),
            "差异视图应把内层编辑器暴露给 Item 协议"
        );

        let hidden = toolbar.update(cx, |toolbar, cx| {
            toolbar.set_active_pane_item(None, window, cx)
        });
        assert_eq!(hidden, ToolbarItemLocation::Hidden);
    });
}
