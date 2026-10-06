use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use gpui::{AppContext as _, TestAppContext};

use zcv_buffer_diff::{DiffHunkKind, DiffHunkStaging, PendingHunk};
use zcv_fs_watch::{FsEventStream, FsWatcher, Watcher};
use zcv_language::{LanguageBuffer, LanguageRegistry};
use zcv_multi_buffer::{DiffExcerptRanges, ExcerptDiffKind};
use zcv_text::{Buffer, BufferConfig, ByteOffset, Edit, Line, TextRange, TransactionMetadata};

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

    fn watch(&self, latency: std::time::Duration) -> FsEventStream {
        self.watcher.watch(latency)
    }
}

fn test_project(root: PathBuf, cx: &mut TestAppContext) -> Entity<Project> {
    let watcher: Arc<dyn Watcher> = Arc::new(PassiveWatcher::new());
    cx.new(|cx| {
        Project::new_with_watcher(root, watcher, Arc::new(LanguageRegistry::new()), cx)
            .expect("测试项目根目录应可规范化")
    })
}

fn canonical_root(path: &Path) -> PathBuf {
    AbsolutePathBuf::canonicalize(path)
        .expect("应规范化仓库路径")
        .into_path_buf()
}

#[gpui::test]
fn conflict_projection_refresh_preserves_unchanged_files(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let root = canonical_root(directory.path());
    let first = root.join("first.rs");
    let second = root.join("second.rs");
    let third = root.join("third.rs");
    std::fs::create_dir(root.join("sub")).expect("应创建路径归一化测试目录");
    let second_display = root.join("sub").join("..").join("second.rs");
    assert_ne!(second_display, second);
    let conflict = "<<<<<<< ours\nours\n=======\ntheirs\n>>>>>>> theirs\n";
    for path in [&first, &second, &third] {
        std::fs::write(path, conflict).expect("应写入冲突文件");
    }
    let project = test_project(root, cx);
    let view = cx.new(|cx| DiffView::new(ProjectDiffKind::Conflict, project, cx));
    let project_file = |path: PathBuf| GitChangeFile {
        path,
        status: FileStatus::Unmerged,
    };

    view.update(cx, |view, cx| {
        view.files = vec![
            project_file(first.clone()),
            project_file(second_display.clone()),
        ];
        view.sync_conflict_projection(cx);
    });
    let (topology, source_ids) = cx.update_entity(&view, |view, cx| {
        let snapshot = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        let source_ids = view
            .multi_buffer
            .read(cx)
            .file_buffers(cx)
            .into_iter()
            .map(|(source, path)| (path, source.entity_id()))
            .collect::<HashMap<_, _>>();
        (snapshot.topology_version(), source_ids)
    });
    assert_eq!(source_ids.len(), 2);

    view.update(cx, |view, cx| view.sync_conflict_projection(cx));
    let refreshed = cx.update_entity(&view, |view, cx| {
        view.multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx))
    });
    assert_eq!(refreshed.topology_version(), topology);

    view.update(cx, |view, cx| {
        view.files = vec![
            project_file(second_display.clone()),
            project_file(third.clone()),
        ];
        view.sync_conflict_projection(cx);
    });
    let (paths, remaining_ids, hunks, topology_after_paths) =
        cx.update_entity(&view, |view, cx| {
            let snapshot = view
                .multi_buffer
                .update(cx, |buffer, cx| buffer.snapshot(cx));
            let paths = snapshot
                .excerpts()
                .map(|excerpt| excerpt.path().to_path_buf())
                .collect::<Vec<_>>();
            let ids = view
                .multi_buffer
                .read(cx)
                .file_buffers(cx)
                .into_iter()
                .map(|(source, path)| (path, source.entity_id()))
                .collect::<HashMap<_, _>>();
            (
                paths,
                ids,
                view.conflict_editor_hunks(cx),
                snapshot.topology_version(),
            )
        });
    assert_eq!(paths, vec![second.clone(), third.clone()]);
    assert_eq!(remaining_ids[&second], source_ids[&second]);
    assert!(!remaining_ids.contains_key(&first));
    assert_eq!(hunks.len(), 2);

    let second_source = cx.read_entity(&view, |view, cx| {
        view.multi_buffer
            .read(cx)
            .file_buffers(cx)
            .into_iter()
            .find(|(_, path)| path == &second)
            .expect("保留的冲突文件应仍在投影中")
            .0
    });
    second_source.update(cx, |source, cx| {
        let range = TextRange::new(ByteOffset::ZERO, source.text_snapshot().len_bytes()).unwrap();
        source
            .edit(
                [Edit::replace(range, "resolved\n")],
                TransactionMetadata::default(),
                cx,
            )
            .unwrap();
    });
    view.update(cx, |view, cx| view.sync_conflict_projection(cx));
    let (updated, hunks) = cx.update_entity(&view, |view, cx| {
        let snapshot = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        (snapshot.clone(), view.conflict_editor_hunks(cx))
    });
    let second_buffer_id = cx.read_entity(&second_source, |source, _| source.buffer_id());
    let second_text = updated
        .text_for_range(updated.buffer_range(second_buffer_id).unwrap())
        .unwrap();
    assert!(second_text.starts_with("resolved\n"));
    assert!(!second_text.contains("<<<<<<<"));
    assert_eq!(updated.topology_version(), topology_after_paths);
    assert_eq!(hunks.len(), 1, "源文本解决冲突后仅保留其他文件的装饰");

    view.update(cx, |view, cx| {
        view.files.clear();
        view.sync_conflict_projection(cx);
    });
    cx.update_entity(&view, |view, cx| {
        let snapshot = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        assert_eq!(snapshot.excerpts().count(), 0);
        assert!(view.conflict_editor_hunks(cx).is_empty());
    });
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
                base_text: Some(Arc::from(base_text)),
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
        excerpt_ranges: DiffExcerptRanges::FullFile,
    }
}

/// 单独测量真实宿主委托的控件构建与控件绘制，排除文档文本布局。
/// 固定每文件 1536 行、48 个修改块，30 组预热后的样本；耗时只用于人工比较。
#[gpui::test]
#[ignore]
fn project_diff_control_render_cost(cx: &mut TestAppContext) {
    use std::time::Instant;

    struct ControlProbe {
        delegate: ProjectDiffHunkDelegate,
        target: HunkControlTarget,
        editor: Entity<Editor>,
    }

    impl Render for ControlProbe {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.delegate
                .render_hunk_controls(&self.target, 0, &self.editor, window, cx)
                .expect("应绘制 hunk 控件")
        }
    }

    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let root = canonical_root(directory.path());
    for kind in [ProjectDiffKind::Staged, ProjectDiffKind::Unstaged] {
        for file_count in [2, 16] {
            let project = test_project(root.clone(), cx);
            let view = cx.new(|cx| DiffView::new(kind, project, cx));
            cx.run_until_parked();
            let registry = Arc::new(LanguageRegistry::new());
            let base = (0..1536)
                .map(|line| format!("let line_{line} = original_value;\n"))
                .collect::<String>();
            let working = (0..1536)
                .map(|line| {
                    if line % 32 < 8 {
                        format!("let line_{line} = changed_value;\n")
                    } else {
                        format!("let line_{line} = original_value;\n")
                    }
                })
                .collect::<String>();
            let mut files = Vec::new();
            for index in 0..file_count {
                let path = root.join(format!("file_{index:02}.rs"));
                let source = cx.new(|cx| {
                    LanguageBuffer::new(
                        Buffer::from_text(working.clone(), BufferConfig::default()).unwrap(),
                        Some(path.clone()),
                        registry.clone(),
                        cx,
                    )
                });
                let diff = cx.new(|cx| {
                    BufferDiff::new(
                        BufferDiffInput {
                            working: source,
                            base_text: Some(Arc::from(base.clone())),
                            index_text: Some(Arc::from(if kind == ProjectDiffKind::Staged {
                                working.clone()
                            } else {
                                base.clone()
                            })),
                            path: path.clone(),
                            language_registry: registry.clone(),
                            key: index,
                            operations: None,
                        },
                        cx,
                    )
                });
                view.update(cx, |view, cx| {
                    view.files.push(GitChangeFile {
                        path: path.clone(),
                        status: FileStatus::Tracked {
                            index_status: StatusCode::Modified,
                            worktree_status: StatusCode::Modified,
                        },
                    });
                    view.subscribe_to_diff_ranges(&diff, cx);
                });
                files.push(DiffFile {
                    diff,
                    display_path: path,
                    excerpt_ranges: DiffExcerptRanges::Windows(
                        (0..1536).step_by(32).map(|line| line..line + 10).collect(),
                    ),
                });
            }
            view.update(cx, |view, cx| {
                view.editor
                    .update(cx, |editor, cx| editor.set_diff_files(files, cx));
            });
            cx.run_until_parked();
            let hunk = cx.update_entity(&view, |view, cx| {
                let snapshot = view
                    .multi_buffer
                    .update(cx, |buffer, cx| buffer.snapshot(cx));
                snapshot
                    .diff_hunks_in_lines(
                        snapshot.line_count().saturating_sub(20)..snapshot.line_count(),
                    )
                    .pop()
                    .expect("应有可见 hunk")
                    .1
                    .source
            });
            let editor = cx.read_entity(&view, |view, _| view.editor.clone());
            let (probe, visual) = cx.add_window_view(|_, _| ControlProbe {
                delegate: ProjectDiffHunkDelegate {
                    view: view.downgrade(),
                },
                target: HunkControlTarget::Diff(hunk),
                editor,
            });
            let mut render = || {
                visual.draw(
                    gpui::point(gpui::px(0.), gpui::px(0.)),
                    gpui::size(
                        gpui::AvailableSpace::MinContent,
                        gpui::AvailableSpace::MinContent,
                    ),
                    |_, _| probe.clone().into_any_element(),
                );
            };
            for _ in 0..10 {
                render();
            }
            let mut samples = Vec::new();
            for _ in 0..30 {
                let start = Instant::now();
                for _ in 0..10 {
                    render();
                }
                samples.push(start.elapsed().as_secs_f64() * 1000.0 / 10.0);
            }
            samples.sort_by(f64::total_cmp);
            println!(
                "宿主控件 {kind:?}/{file_count} 文件：中位数 {:.6} ms，范围 {:.6}..{:.6} ms",
                samples[15], samples[0], samples[29]
            );
        }
    }
}

#[gpui::test]
fn hunk_actions_keep_the_source_after_display_indices_change(cx: &mut TestAppContext) {
    use std::sync::Mutex;
    use zcv_buffer_diff::DiffOperations;
    use zcv_text::Anchor;

    struct RecordedAction {
        operation: GitHunkOperation,
        buffer_id: BufferId,
        ranges: Vec<Range<Anchor>>,
    }
    #[derive(Default)]
    struct Operations(Mutex<Vec<RecordedAction>>);
    impl Operations {
        fn record(
            &self,
            operation: GitHunkOperation,
            diff: Entity<BufferDiff>,
            ranges: Vec<Range<Anchor>>,
            cx: &App,
        ) {
            self.0.lock().unwrap().push(RecordedAction {
                operation,
                buffer_id: diff.read(cx).working().read(cx).buffer_id(),
                ranges,
            });
        }
    }
    impl DiffOperations for Operations {
        fn supports_staging(&self) -> bool {
            true
        }
        fn supports_unstaging(&self) -> bool {
            true
        }
        fn supports_restore(&self) -> bool {
            true
        }
        fn stage(&self, diff: Entity<BufferDiff>, ranges: Vec<Range<Anchor>>, cx: &mut App) {
            self.record(GitHunkOperation::Stage, diff, ranges, cx);
        }
        fn unstage(&self, diff: Entity<BufferDiff>, ranges: Vec<Range<Anchor>>, cx: &mut App) {
            self.record(GitHunkOperation::Unstage, diff, ranges, cx);
        }
        fn restore(&self, diff: Entity<BufferDiff>, ranges: Vec<Range<Anchor>>, cx: &mut App) {
            self.record(GitHunkOperation::Restore, diff, ranges, cx);
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let root = canonical_root(directory.path());
    let project = test_project(root.clone(), cx);
    let view = cx.new(|cx| DiffView::new(ProjectDiffKind::Unstaged, project, cx));
    cx.run_until_parked();
    let operations = Arc::new(Operations::default());
    let registry = Arc::new(LanguageRegistry::new());
    let mut files = Vec::new();
    for name in ["a.rs", "b.rs"] {
        let path = root.join(name);
        let working = cx.new(|cx| {
            LanguageBuffer::new(
                Buffer::from_text("before\nnew\nafter\n".into(), BufferConfig::default()).unwrap(),
                Some(path.clone()),
                registry.clone(),
                cx,
            )
        });
        let diff = cx.new(|cx| {
            BufferDiff::new(
                BufferDiffInput {
                    working,
                    base_text: Some("before\nold\nafter\n".into()),
                    index_text: None,
                    path: path.clone(),
                    language_registry: registry.clone(),
                    key: 0,
                    operations: Some(operations.clone()),
                },
                cx,
            )
        });
        view.update(cx, |view, cx| view.subscribe_to_diff_ranges(&diff, cx));
        files.push(DiffFile {
            diff,
            display_path: path,
            excerpt_ranges: DiffExcerptRanges::FullFile,
        });
    }
    view.update(cx, |view, cx| {
        view.editor
            .update(cx, |editor, cx| editor.set_diff_files(files, cx));
    });
    cx.run_until_parked();
    let target = cx.update_entity(&view, |view, cx| {
        let snapshot = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        let hunks = snapshot.resolved_diff_hunks();
        assert_eq!(hunks.len(), 2);
        hunks[1].1.source.clone()
    });
    view.update(cx, |view, cx| {
        view.editor.update(cx, |editor, cx| {
            editor.remove_diff(&root.join("a.rs"), cx);
        });
        let snapshot = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        assert_eq!(snapshot.resolved_diff_hunks()[0].1.source, target);
        for operation in [
            GitHunkOperation::Stage,
            GitHunkOperation::Unstage,
            GitHunkOperation::Restore,
        ] {
            view.apply_hunk_action(target.clone(), operation, cx);
        }
        view.restore_all(cx);
    });
    let recorded = operations.0.lock().unwrap();
    for (action, operation) in recorded.iter().zip([
        GitHunkOperation::Stage,
        GitHunkOperation::Unstage,
        GitHunkOperation::Restore,
    ]) {
        assert_eq!(action.operation, operation);
        assert_eq!(action.buffer_id, target.buffer_id);
        assert_eq!(action.ranges, vec![target.range.clone().unwrap()]);
    }
    assert_eq!(
        recorded.len(),
        5,
        "重做全部应读取两个源 diff，单块操作不依赖显示序号"
    );
    assert!(
        recorded[3..]
            .iter()
            .all(|action| action.operation == GitHunkOperation::Restore)
    );
}

#[gpui::test]
fn created_file_and_pure_deletion_targets_stage_and_unstage_without_display_lookup(
    cx: &mut TestAppContext,
) {
    let directory = tempfile::tempdir().expect("应创建临时仓库");
    let root = canonical_root(directory.path());
    run_in(&root, &["git", "init", "-q", "-b", "master"]);
    run_in(&root, &["git", "config", "user.email", "test@example.com"]);
    run_in(&root, &["git", "config", "user.name", "Test User"]);
    let deleted_path = root.join("deleted.txt");
    std::fs::write(&deleted_path, "removed\n").unwrap();
    run_in(&root, &["git", "add", "."]);
    run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
    std::fs::write(&deleted_path, "").unwrap();
    std::fs::write(root.join("created.txt"), "created\n").unwrap();

    let project = test_project(root.clone(), cx);
    for (kind, operation) in [
        (ProjectDiffKind::Unstaged, GitHunkOperation::Stage),
        (ProjectDiffKind::Staged, GitHunkOperation::Unstage),
    ] {
        let view = cx.new(|cx| DiffView::new(kind, project.clone(), cx));
        for _ in 0..3 {
            cx.run_until_parked();
        }
        let targets = cx.update_entity(&view, |view, cx| {
            let snapshot = view
                .multi_buffer
                .update(cx, |buffer, cx| buffer.snapshot(cx));
            let hunks = snapshot.resolved_diff_hunks();
            assert_eq!(
                hunks.len(),
                2,
                "分组={kind:?}，显示索引={:?}，输出区域={:?}",
                snapshot.diff_display(),
                snapshot.regions().collect::<Vec<_>>()
            );
            let targets = hunks
                .into_iter()
                .map(|(_, hunk)| hunk.source)
                .collect::<Vec<_>>();
            assert!(targets[0].range.is_none(), "新增文件必须走整文件路径操作");
            let deletion = targets[1]
                .range
                .as_ref()
                .expect("纯删除必须保留行级源 hunk");
            let diff = &view
                .diff_subscriptions
                .get(&targets[1].buffer_id)
                .unwrap()
                .diff;
            let text = diff.read(cx).working().read(cx).text_snapshot();
            assert_eq!(
                deletion.start.resolve_in(&text).unwrap(),
                deletion.end.resolve_in(&text).unwrap()
            );
            targets
        });
        for target in targets {
            view.update(cx, |view, cx| view.apply_hunk_action(target, operation, cx));
            for _ in 0..3 {
                cx.run_until_parked();
            }
        }
        let output = Command::new("git")
            .args(["diff", "--cached", "--name-status"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(output.status.success());
        let status = String::from_utf8(output.stdout).unwrap();
        if kind == ProjectDiffKind::Unstaged {
            assert!(
                status.contains("A\tcreated.txt"),
                "新增块应完成整文件暂存：{status}"
            );
            assert!(
                status.contains("M\tdeleted.txt"),
                "纯删除块应完成行级暂存：{status}"
            );
        } else {
            assert!(status.is_empty(), "两个源目标都应取消暂存：{status}");
        }
    }
}

#[gpui::test]
fn empty_project_diff_renders_blank_focusable_view(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let project = test_project(directory.path().to_path_buf(), cx);
    let (view, cx) =
        cx.add_window_view(move |_, cx| DiffView::new(ProjectDiffKind::Staged, project, cx));
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
    let view = cx.new(|cx| DiffView::new(ProjectDiffKind::Unstaged, project, cx));

    // DiffView 创建时 MultiBuffer 仍为空，但应立即向 GitStore 声明文件 hunk 需求。
    cx.run_until_parked();
    cx.run_until_parked();

    cx.update_entity(&view, |view, cx| {
        let multi_buffer = view.multi_buffer(cx).expect("项目差异应提供组合文档");
        let text = String::from_utf8(
            multi_buffer.update(cx, |buffer, cx| buffer.snapshot(cx).text_bytes()),
        )
        .expect("投影文本应为 UTF-8");
        assert_eq!(text, "line1\nline2\n原内容\n新内容\nline4\nline5\n");
        assert_eq!(
            multi_buffer.read(cx).diff_hunks().len(),
            1,
            "组合文档最初为空时，GitStore 的 hunk 请求仍应产出可见 hunk"
        );
    });
}

/// 端到端：git 删除文件中间一行后，展开的旧侧行保持原始位置；折叠时只移除旧侧内容。
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
    let view = cx.new(|cx| DiffView::new(ProjectDiffKind::Unstaged, project, cx));
    cx.run_until_parked();
    cx.run_until_parked();

    // 默认只显示 hunk 上下各两行上下文，并将被删旧侧行放回原始位置。
    cx.update_entity(&view, |view, cx| {
        let text = String::from_utf8(
            view.multi_buffer
                .update(cx, |buffer, cx| buffer.snapshot(cx).text_bytes()),
        )
        .expect("投影应为 UTF-8");
        let text_lines = text.lines().collect::<Vec<_>>();
        assert_eq!(text_lines.len(), 5, "展开后只显示 hunk 与两行上下文");
        assert_eq!(text_lines[0], "line 15", "上下文从 hunk 前两行开始");
        assert_eq!(text_lines[1], "line 16", "第 16 行顺序保持");
        assert_eq!(
            text_lines[2], "line 17",
            "被删行应投影到 line 16 之后（原始位置）"
        );
        assert_eq!(text_lines[3], "line 18", "被删行后的行顺序保持");
        assert_eq!(text_lines[4], "line 19", "上下文包含 hunk 后两行");
    });

    // 折叠删除块时旧侧内容消失，不在人为插入占位行。
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
        let text_lines = text.lines().collect::<Vec<_>>();
        assert_eq!(text_lines.len(), 4, "折叠后仍只显示 hunk 上下文：{text:?}");
        assert!(!text_lines.contains(&"line 17"), "折叠后旧侧行应消失");
        assert_eq!(text_lines[0], "line 15", "折叠后仍从两行上下文开始");
        assert_eq!(text_lines[1], "line 16", "折叠后第 16 行保持");
        assert_eq!(text_lines[2], "line 18", "折叠后原第 18 行紧随第 16 行");
        assert_eq!(text_lines[3], "line 19", "折叠后保留 hunk 后两行上下文");
        // 折叠删除块保留一个 hunk（显示坐标为组合坐标，不在此断言源行号）。
        let snapshot = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        let hunks = snapshot.resolved_diff_hunks();
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
    let view = cx.new(|cx| DiffView::new(ProjectDiffKind::Unstaged, project, cx));
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
        "将被删除\n\nline2\nline3\n修改前\n修改后\nline5\nline6\n\n新增\n"
    );
}

/// 从删除输出区域打开文件时，换算到工作区文件中的真实行列。
#[gpui::test]
fn deleted_output_region_maps_to_working_tree_hunk_position(cx: &mut TestAppContext) {
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
    let view = cx.new(|cx| DiffView::new(ProjectDiffKind::Unstaged, project, cx));
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
        let mut regions = snapshot.regions();
        // 删除区域首行（旧侧 "修改前"）→ 工作区第 5 行（0-based 4）。
        let modified_region = regions
            .find(|region| {
                region.path() == modified_path
                    && region.diff_kind() == Some(ExcerptDiffKind::Deleted)
            })
            .expect("修改行应有删除输出区域");
        let location = ExcerptLocation {
            path: modified_path.clone(),
            source_range: modified_region.source_range(),
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
            modified_region.source_range(),
            TextRange::new(ByteOffset::new(24), ByteOffset::new(34),).expect("旧侧第 5 行范围"),
            "删除输出区域应覆盖被修改的旧行（含行尾换行）"
        );
        // 整文件删除：纯删除 hunk 的 range 为空，锚定到变更块起点（0-based 0）。
        let removed_region = regions
            .find(|region| {
                region.path().file_name().and_then(|name| name.to_str()) == Some("removed.txt")
                    && region.diff_kind() == Some(ExcerptDiffKind::Deleted)
            })
            .expect("删除文件应有删除输出区域");
        let removed_location = ExcerptLocation {
            path: removed_region.path().to_path_buf(),
            source_range: removed_region.source_range(),
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
    let staged_view = cx.new(|cx| DiffView::new(ProjectDiffKind::Staged, project.clone(), cx));
    let unstaged_view = cx.new(|cx| DiffView::new(ProjectDiffKind::Unstaged, project, cx));
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
fn staging_one_hunk_refreshes_the_projection_with_new_index(cx: &mut TestAppContext) {
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
    let view = cx.new(|cx| DiffView::new(ProjectDiffKind::Unstaged, project, cx));
    cx.run_until_parked();
    cx.run_until_parked();
    cx.run_until_parked();

    let (hunk_source, initial_version) = cx.update_entity(&view, |view, cx| {
        let snapshot = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        let hunks = snapshot.resolved_diff_hunks();
        assert_eq!(hunks.len(), 2);
        let hunk_source = hunks[0].1.source.clone();
        let version = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx).version().get());
        (hunk_source, version)
    });
    view.update(cx, |view, cx| {
        view.apply_hunk_action(hunk_source, GitHunkOperation::Stage, cx);
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

    let staged = std::process::Command::new("git")
        .args(["diff", "--cached", "--", "two-hunks.txt"])
        .current_dir(&root)
        .output()
        .expect("应执行 git diff --cached");
    let staged = String::from_utf8_lossy(&staged.stdout);
    assert!(
        staged.contains("第一个变更块"),
        "点击暂存后 index 必须包含该 hunk，实际 diff：{staged}"
    );
    cx.update_entity(&view, |view, cx| {
        let snapshot = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        let hunks = snapshot.resolved_diff_hunks();
        assert_eq!(hunks.len(), 1, "暂存后未暂存视图只应剩第二个 hunk");
        let source = &hunks[0].1.source;
        let diff = &view
            .diff_subscriptions
            .get(&source.buffer_id)
            .expect("源 diff 应保留订阅")
            .diff;
        assert!(
            diff.read(cx).snapshot().pending_hunks().is_empty(),
            "暂存完成后必须按新 index 重建 diff，而不是靠旧 diff 的 pending 抑制"
        );
    });

    cx.update_entity(&view, |view, cx| {
        let snapshot = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        let text = String::from_utf8(snapshot.text_bytes()).expect("投影应为 UTF-8");
        assert!(
            snapshot.version().get() > initial_version,
            "暂存后投影必须按新 index 推进"
        );
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

#[gpui::test]
fn staging_first_hunk_keeps_last_deletion_visible_after_reopening(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时仓库");
    let root = canonical_root(directory.path());
    run_in(&root, &["git", "init", "-q", "-b", "master"]);
    run_in(&root, &["git", "config", "user.email", "test@example.com"]);
    run_in(&root, &["git", "config", "user.name", "Test User"]);
    let path = root.join("last-deletion.txt");
    let original = (0..30)
        .map(|line| format!("line{line}"))
        .collect::<Vec<_>>();
    std::fs::write(&path, format!("{}\n", original.join("\n"))).expect("应创建文件");
    run_in(&root, &["git", "add", "last-deletion.txt"]);
    run_in(&root, &["git", "commit", "-q", "-m", "initial"]);

    let mut changed = original;
    changed[3] = "已修改的第一个块".into();
    changed.remove(29);
    std::fs::write(&path, format!("{}\n", changed.join("\n"))).expect("应修改文件");

    let project = test_project(root.clone(), cx);
    let view = cx.new(|cx| DiffView::new(ProjectDiffKind::Unstaged, project.clone(), cx));
    for _ in 0..3 {
        cx.run_until_parked();
    }
    let first = view.update(cx, |view, cx| {
        let snapshot = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        let hunks = snapshot.resolved_diff_hunks();
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[1].1.hunk.kind, DiffHunkKind::Deleted);
        hunks[0].1.source.clone()
    });
    view.update(cx, |view, cx| {
        view.apply_hunk_action(first, GitHunkOperation::Stage, cx);
    });
    for _ in 0..3 {
        cx.run_until_parked();
    }

    let assert_last_deletion = |view: &Entity<DiffView>, cx: &mut TestAppContext| {
        view.update(cx, |view, cx| {
            let snapshot = view
                .multi_buffer
                .update(cx, |buffer, cx| buffer.snapshot(cx));
            let hunks = snapshot.resolved_diff_hunks();
            assert_eq!(hunks.len(), 1, "暂存首块后必须保留最后的删除块");
            assert_eq!(hunks[0].1.hunk.kind, DiffHunkKind::Deleted);
            assert_eq!(hunks[0].1.hunk.staging, DiffHunkStaging::Unstaged);
            assert!(hunks[0].1.old_range.is_some(), "删除块的旧侧必须可见");
            let text = String::from_utf8(snapshot.text_bytes()).expect("投影应为 UTF-8");
            assert!(text.contains("line29"), "末行删除内容必须显示：{text:?}");
        });
    };
    assert_last_deletion(&view, cx);
    let reopened = cx.new(|cx| DiffView::new(ProjectDiffKind::Unstaged, project, cx));
    for _ in 0..3 {
        cx.run_until_parked();
    }
    assert_last_deletion(&reopened, cx);
}

#[gpui::test]
fn initially_staged_deleted_file_displays_old_side(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时仓库");
    let root = canonical_root(directory.path());
    run_in(&root, &["git", "init", "-q", "-b", "master"]);
    run_in(&root, &["git", "config", "user.email", "test@example.com"]);
    run_in(&root, &["git", "config", "user.name", "Test User"]);
    let path = root.join("deleted.txt");
    let original = (0..98)
        .map(|line| format!("删除前的内容 {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&path, format!("{original}\n")).expect("应创建文件");
    run_in(&root, &["git", "add", "deleted.txt"]);
    run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
    run_in(&root, &["git", "rm", "-q", "deleted.txt"]);

    let project = test_project(root, cx);
    let view = cx.new(|cx| DiffView::new(ProjectDiffKind::Staged, project, cx));
    for _ in 0..3 {
        cx.run_until_parked();
    }
    view.update(cx, |view, cx| {
        let snapshot = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        let hunks = snapshot.resolved_diff_hunks();
        assert_eq!(hunks.len(), 1, "已暂存删除文件必须保留删除块");
        assert_eq!(hunks[0].1.hunk.kind, DiffHunkKind::Deleted);
        let text = String::from_utf8(snapshot.text_bytes()).expect("投影应为 UTF-8");
        assert!(
            text.contains("删除前的内容 0"),
            "旧侧首行必须显示：{text:?}"
        );
        assert!(
            text.contains("删除前的内容 97"),
            "旧侧末行必须显示：{text:?}"
        );
    });
}

#[gpui::test]
fn staged_projection_refreshes_without_status_change_or_restart(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时仓库");
    let root = canonical_root(directory.path());
    run_in(&root, &["git", "init", "-q", "-b", "master"]);
    run_in(&root, &["git", "config", "user.email", "test@example.com"]);
    run_in(&root, &["git", "config", "user.name", "Test User"]);
    let path = root.join("staged.txt");
    std::fs::write(&path, "原始内容\n").unwrap();
    run_in(&root, &["git", "add", "staged.txt"]);
    run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
    std::fs::write(&path, "暂存甲\n").unwrap();
    run_in(&root, &["git", "add", "staged.txt"]);
    let project = test_project(root.clone(), cx);
    let store = project.read_with(cx, |project, _| project.git_store());
    let view = cx.new(|cx| DiffView::new(ProjectDiffKind::Staged, project, cx));
    cx.run_until_parked();
    let initial_status = store.read_with(cx, |store, _| store.status_for_path(&path).cloned());
    let source_id = view.update(cx, |view, cx| {
        let snapshot = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        assert!(!snapshot.resolved_diff_hunks().is_empty());
        snapshot.excerpts().next().unwrap().buffer_id()
    });

    // 不打开工作区文件，只接收 index 的文件监听事件；删除／恢复也沿同一条源链推进。
    for expected in [Some("暂存乙\n"), None, Some("暂存丙\n")] {
        match expected {
            Some(text) => {
                std::fs::write(&path, text).unwrap();
                run_in(&root, &["git", "add", "staged.txt"]);
            }
            None => run_in(&root, &["git", "rm", "-q", "--cached", "staged.txt"]),
        }
        store.update(cx, |store, cx| {
            store.refresh_statuses_for_paths(&[root.join(".git/index")], cx)
        });
        cx.run_until_parked();
        if expected.is_some() {
            assert_eq!(
                store.read_with(cx, |store, _| store.status_for_path(&path).cloned()),
                initial_status
            );
        }
        view.update(cx, |view, cx| {
            let snapshot = view
                .multi_buffer
                .update(cx, |buffer, cx| buffer.snapshot(cx));
            let text = String::from_utf8(snapshot.text_bytes()).unwrap();
            assert!(
                text.contains(expected.unwrap_or("原始内容\n")),
                "index 内容必须直接推进已打开的组合文档：{text}"
            );
            assert!(!text.contains("暂存甲"));
            assert_eq!(snapshot.excerpts().next().unwrap().buffer_id(), source_id);
            let hunks = snapshot.resolved_diff_hunks();
            assert!(!hunks.is_empty(), "刷新后必须保留 Git 差异高亮来源");
            assert!(
                hunks
                    .iter()
                    .all(|(_, hunk)| hunk.hunk.staging == DiffHunkStaging::Staged)
            );
            assert!(
                view.diff_subscriptions
                    .values()
                    .all(|subscription| subscription
                        .diff
                        .read(cx)
                        .is_current_version_calculated(cx))
            );
        });
    }
}

#[gpui::test]
fn head_change_updates_staged_projection_without_rebuilding_file_list(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时仓库");
    let root = canonical_root(directory.path());
    run_in(&root, &["git", "init", "-q", "-b", "master"]);
    run_in(&root, &["git", "config", "user.email", "test@example.com"]);
    run_in(&root, &["git", "config", "user.name", "Test User"]);
    let path = root.join("staged.txt");
    std::fs::write(&path, "首次提交\n").unwrap();
    run_in(&root, &["git", "add", "staged.txt"]);
    run_in(&root, &["git", "commit", "-q", "-m", "first"]);
    std::fs::write(&path, "第二次提交\n").unwrap();
    run_in(&root, &["git", "add", "staged.txt"]);
    run_in(&root, &["git", "commit", "-q", "-m", "second"]);
    std::fs::write(&path, "当前暂存\n").unwrap();
    run_in(&root, &["git", "add", "staged.txt"]);

    let project = test_project(root.clone(), cx);
    let store = project.read_with(cx, |project, _| project.git_store());
    let view = cx.new(|cx| DiffView::new(ProjectDiffKind::Staged, project, cx));
    cx.run_until_parked();
    let (source_id, before) = view.update(cx, |view, cx| {
        let snapshot = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        (
            snapshot.excerpts().next().unwrap().buffer_id(),
            String::from_utf8(snapshot.text_bytes()).unwrap(),
        )
    });
    assert!(before.contains("第二次提交"));
    assert!(before.contains("当前暂存"));

    run_in(&root, &["git", "reset", "--soft", "HEAD~1"]);
    store.update(cx, |store, cx| {
        store.refresh_statuses_for_paths(&[root.join(".git/HEAD")], cx)
    });
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        let snapshot = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        let text = String::from_utf8(snapshot.text_bytes()).unwrap();
        assert!(text.contains("首次提交"));
        assert!(text.contains("当前暂存"));
        assert!(!text.contains("第二次提交"));
        assert_eq!(snapshot.excerpts().next().unwrap().buffer_id(), source_id);
    });
}

#[gpui::test]
fn clearing_pending_hunk_restores_projection_without_rebuilding_files(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时仓库");
    let root = canonical_root(directory.path());
    run_in(&root, &["git", "init", "-q", "-b", "master"]);
    run_in(&root, &["git", "config", "user.email", "test@example.com"]);
    run_in(&root, &["git", "config", "user.name", "Test User"]);
    let path = root.join("pending.txt");
    std::fs::write(&path, "原始内容\n").unwrap();
    run_in(&root, &["git", "add", "pending.txt"]);
    run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
    std::fs::write(&path, "修改内容\n").unwrap();

    let project = test_project(root, cx);
    let view = cx.new(|cx| DiffView::new(ProjectDiffKind::Unstaged, project, cx));
    cx.run_until_parked();
    let (diff, source_id) = view.update(cx, |view, cx| {
        let snapshot = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        assert_eq!(snapshot.diff_hunk_count(), 1);
        let source_id = snapshot.excerpts().next().unwrap().buffer_id();
        let diff = view
            .diff_subscriptions
            .get(&source_id)
            .unwrap()
            .diff
            .clone();
        (diff, source_id)
    });
    diff.update(cx, |diff, cx| {
        let hunk = diff.snapshot().hunks().next().unwrap().clone();
        let version = diff.working().read(cx).text_snapshot().version();
        diff.set_pending_hunks(vec![PendingHunk::set_staging(&hunk, version, true)], cx);
    });
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        let snapshot = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        let hunks = snapshot.resolved_diff_hunks();
        assert_eq!(hunks.len(), 1, "暂存中的块仍须参与组合投影");
        assert_eq!(hunks[0].1.hunk.staging, DiffHunkStaging::StagingPending);
        let text = String::from_utf8(snapshot.text_bytes()).expect("投影应为 UTF-8");
        assert!(text.contains("原始内容"), "暂存中的旧侧文本必须保留");
    });
    diff.update(cx, |diff, cx| {
        let hunk = diff.snapshot().hunks().next().unwrap().clone();
        let version = hunk.buffer_range.start.version();
        diff.set_pending_hunks(vec![PendingHunk::suppress(&hunk, version)], cx);
    });
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        let snapshot = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        assert_eq!(snapshot.diff_hunk_count(), 0);
    });

    diff.update(cx, |diff, cx| diff.clear_pending_hunks(cx));
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        let snapshot = view
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        assert_eq!(snapshot.diff_hunk_count(), 1);
        assert_eq!(snapshot.excerpts().next().unwrap().buffer_id(), source_id);
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
    let view = cx.new(|cx| DiffView::new(ProjectDiffKind::Unstaged, project.clone(), cx));
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
            .excerpts()
            .find(|excerpt| excerpt.path() == modified_path)
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

/// 复现：普通编辑器展开 hunk（singleton → excerpts）后触发 git hunks 刷新，与 DiffView 共享仓库时不应让统一投影重建 panic。
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
    // DiffView：同一仓库。
    let view = cx.new(|cx| DiffView::new(ProjectDiffKind::Unstaged, project.clone(), cx));
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
/// DiffView 的片段映射不应 panic。
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
    // DiffView：同一仓库。
    let view = cx.new(|cx| DiffView::new(ProjectDiffKind::Unstaged, project.clone(), cx));
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
        cx.add_window_view(move |_, cx| DiffView::new(ProjectDiffKind::Staged, project, cx));
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

#[gpui::test]
fn diff_view_scroll_keeps_host_quiet_and_folding_notifies(cx: &mut TestAppContext) {
    use gpui::{ScrollDelta, ScrollWheelEvent, point, px};
    use std::cell::Cell;
    use std::rc::Rc;
    use zcv_multi_buffer::ExcerptRange;

    let directory = tempfile::tempdir().unwrap();
    let project = test_project(directory.path().to_path_buf(), cx);
    let (view, visual) =
        cx.add_window_view(move |_, cx| DiffView::new(ProjectDiffKind::Staged, project, cx));
    visual.run_until_parked();
    view.update(&mut *visual, |view, cx| {
        for file in 0..2 {
            let path = directory.path().join(format!("{file}.rs"));
            let source = cx.new(|cx| {
                LanguageBuffer::new(
                    Buffer::from_text("line\n".repeat(100), BufferConfig::default()).unwrap(),
                    Some(path.clone()),
                    Arc::new(LanguageRegistry::new()),
                    cx,
                )
            });
            view.files.push(GitChangeFile {
                path,
                status: FileStatus::Tracked {
                    index_status: StatusCode::Modified,
                    worktree_status: StatusCode::Unmodified,
                },
            });
            view.multi_buffer.update(cx, |buffer, cx| {
                buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(source, 0..100, cx)], cx)
            });
        }
    });
    visual.run_until_parked();
    visual.refresh().unwrap();
    let notifications = Rc::new(Cell::new(0));
    let observed = notifications.clone();
    let _subscription =
        visual.update(|_, cx| cx.observe(&view, move |_, _| observed.set(observed.get() + 1)));
    for delta in [-120., -1_000., 120., 1_000.] {
        visual.simulate_event(ScrollWheelEvent {
            position: point(px(300.), px(300.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
            ..Default::default()
        });
        visual.refresh().unwrap();
        visual.run_until_parked();
    }
    assert_eq!(notifications.get(), 0, "滚动重绘不得通知宿主重建工具栏");
    view.update(&mut *visual, |view, cx| view.set_all_files_folded(true, cx));
    visual.run_until_parked();
    assert!(notifications.get() > 0, "文件折叠语义变化必须刷新宿主");
    assert!(!visual.read_entity(&view, |view, cx| {
        view.editor.read(cx).has_expanded_buffers(cx)
    }));
}
