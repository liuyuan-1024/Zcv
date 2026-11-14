use super::*;
use std::{cell::Cell, fs, path::Path, sync::Arc, time::Duration};

use gpui::{App, KeyBinding, Modifiers, TestAppContext, VisualTestContext, point, px, size};
use tempfile::TempDir;

use zcv_git::StatusCode;
use zcv_keymap::init;
use zcv_language::LanguageRegistry;
use zcv_project::StatusEntry;
use zcv_settings::{SettingsStore, UserSettings};
use zcv_ui::tree_row_height;

use crate::{
    GitBranchAction,
    test::{test_project, test_project_roots},
};

/// 构造快照：路径 → 状态；diff 统计取固定样例值（staged/unstaged 可区分）。
fn snapshot(entries: &[(&str, FileStatus)]) -> RepositorySnapshot {
    RepositorySnapshot {
        branch: None,
        head: None,
        last_commit_message: None,
        has_remote: false,
        ahead: 0,
        behind: 0,
        branch_list: Vec::new(),
        statuses_by_path: entries
            .iter()
            .map(|(path, status)| {
                (
                    RelativePathBuf::from_path(Path::new(path))
                        .expect("测试路径应为有效的仓库相对路径"),
                    StatusEntry {
                        status: *status,
                        diff_stat: DiffStat {
                            added: 2,
                            deleted: 1,
                        },
                        staged_diff_stat: DiffStat {
                            added: 1,
                            deleted: 0,
                        },
                        unstaged_diff_stat: DiffStat {
                            added: 0,
                            deleted: 1,
                        },
                    },
                )
            })
            .collect(),
    }
}

fn absolute(path: PathBuf) -> AbsolutePathBuf {
    let path = if path.is_absolute() {
        path
    } else {
        let relative = path
            .to_string_lossy()
            .trim_start_matches(&['/', '\\'][..])
            .to_owned();
        std::env::current_dir()
            .expect("测试应能取得当前目录")
            .join(relative)
    };
    AbsolutePathBuf::new(path).expect("测试树路径应为绝对路径")
}

fn relative(path: &str) -> RelativePathBuf {
    RelativePathBuf::from_path(Path::new(path)).expect("测试路径应为有效的仓库相对路径")
}

fn active_workdir(
    cx: &VisualTestContext,
    git_store: &Entity<zcv_project::GitStore>,
) -> Option<PathBuf> {
    cx.read_entity(git_store, |store, _| {
        store
            .active_repository_workdir()
            .map(|workdir| workdir.to_path_buf())
    })
}

fn active_branch(
    cx: &VisualTestContext,
    git_store: &Entity<zcv_project::GitStore>,
) -> Option<String> {
    cx.read_entity(git_store, |store, _| {
        store.current_branch().map(str::to_string)
    })
}

fn panel_entries(
    cx: &VisualTestContext,
    panel: &Entity<VersionControlPanel>,
) -> Vec<(GitSection, String)> {
    cx.read_entity(panel, |panel, _| section_entries(panel))
}

impl VersionControlPanel {
    fn test_workdir(&self) -> AbsolutePathBuf {
        self.commit_states
            .keys()
            .next()
            .expect("应有仓库提交框")
            .clone()
    }

    fn test_commit_editor(&self) -> Entity<Editor> {
        self.commit_states
            .values()
            .next()
            .expect("应有仓库提交框")
            .editor
            .clone()
    }

    fn test_last_commit_message(&self, cx: &App) -> Option<String> {
        let store = self.project.read(cx).git_store();
        store
            .read(cx)
            .active_repository_snapshot()
            .and_then(|(_, snapshot)| snapshot.last_commit_message.clone())
    }
}

fn build_rows(_root: &Path, repos: &[(&Path, &RepositorySnapshot)]) -> Vec<GitRow> {
    let trees = build_section_trees(repos.first().map(|(path, _)| *path), repos.iter().copied());
    flatten_rows(&trees, &HashSet::new(), &HashSet::new())
}

/// 行列表 → (分组, 显示名) 序列。
fn entry_keys(rows: &[GitRow]) -> Vec<(GitSection, String)> {
    rows.iter()
        .filter_map(|row| match row {
            GitRow::Entry(entry) => Some((entry.section, entry.name.clone())),
            GitRow::Header { .. } | GitRow::Empty(_) => None,
        })
        .collect()
}

#[test]
fn headers_always_appear_and_partially_staged_entries_duplicate_across_sections() {
    let root = absolute(PathBuf::from("/project")).into_path_buf();
    let partial = FileStatus::Tracked {
        index_status: StatusCode::Modified,
        worktree_status: StatusCode::Modified,
    };
    let snapshot = snapshot(&[("src/a.rs", partial)]);
    let rows = build_rows(&root, &[(root.as_path(), &snapshot)]);

    let headers: Vec<_> = rows
        .iter()
        .filter_map(|row| match row {
            GitRow::Header { section, .. } => Some(section.label()),
            GitRow::Empty(_) | GitRow::Entry(_) => None,
        })
        .collect();
    assert_eq!(headers, vec!["冲突", "已暂存", "未暂存"]);

    // 条目位于 src/ 下，折叠时两组各出现一个 src 目录行。
    let entries = entry_keys(&rows);
    assert_eq!(
        entries,
        vec![
            (GitSection::Staged, "src".into()),
            (GitSection::Unstaged, "src".into())
        ]
    );
    // 两组各带对应视角的 diff 统计（目录聚合 = 子项求和）。
    let staged = rows.iter().find_map(|row| match row {
        GitRow::Entry(e) if e.section == GitSection::Staged => Some(e),
        _ => None,
    });
    let unstaged = rows.iter().find_map(|row| match row {
        GitRow::Entry(e) if e.section == GitSection::Unstaged => Some(e),
        _ => None,
    });
    assert_eq!(
        staged.unwrap().diff_stat,
        DiffStat {
            added: 1,
            deleted: 0
        }
    );
    assert_eq!(
        unstaged.unwrap().diff_stat,
        DiffStat {
            added: 0,
            deleted: 1
        }
    );
}

#[test]
fn expanded_empty_sections_show_unselectable_prompt_rows() {
    let trees = GitSections::default();
    let rows = flatten_rows(&trees, &HashSet::new(), &HashSet::new());

    assert_eq!(GitSection::Staged.empty_message(), "没有已暂存的更改");
    assert_eq!(GitSection::Unstaged.empty_message(), "没有未暂存的更改");
    assert_eq!(rows.len(), 6);
    assert!(matches!(
        rows[0],
        GitRow::Header {
            section: GitSection::Conflict,
            file_count: 0
        }
    ));
    assert!(matches!(rows[1], GitRow::Empty(GitSection::Conflict)));
    assert!(matches!(
        rows[2],
        GitRow::Header {
            section: GitSection::Staged,
            file_count: 0
        }
    ));
    assert!(matches!(rows[3], GitRow::Empty(GitSection::Staged)));
    assert!(matches!(
        rows[4],
        GitRow::Header {
            section: GitSection::Unstaged,
            file_count: 0
        }
    ));
    assert!(matches!(rows[5], GitRow::Empty(GitSection::Unstaged)));
    assert!(rows.iter().all(|row| row_entry_key(row).is_none()));

    let collapsed = HashSet::from([GitSection::Staged]);
    let rows = flatten_rows(&trees, &HashSet::new(), &collapsed);
    assert_eq!(rows.len(), 5);
    assert!(matches!(
        rows[0],
        GitRow::Header {
            section: GitSection::Conflict,
            file_count: 0
        }
    ));
    assert!(matches!(rows[1], GitRow::Empty(GitSection::Conflict)));
    assert!(matches!(
        rows[2],
        GitRow::Header {
            section: GitSection::Staged,
            file_count: 0
        }
    ));
    assert!(matches!(
        rows[3],
        GitRow::Header {
            section: GitSection::Unstaged,
            file_count: 0
        }
    ));
    assert!(matches!(rows[4], GitRow::Empty(GitSection::Unstaged)));
}

#[test]
fn section_header_counts_files_inside_collapsed_directories_and_sections() {
    let root = absolute(PathBuf::from("/project"));
    let snapshot = snapshot(&[
        (
            "src/a.rs",
            FileStatus::Tracked {
                index_status: StatusCode::Modified,
                worktree_status: StatusCode::Modified,
            },
        ),
        ("src/nested/b.rs", FileStatus::Untracked),
    ]);
    let trees = build_section_trees(
        Some(root.as_path()),
        [(root.as_path(), &snapshot)].into_iter(),
    );
    let rows = flatten_rows(
        &trees,
        &HashSet::new(),
        &HashSet::from([GitSection::Unstaged]),
    );

    assert!(rows.iter().any(|row| matches!(
        row,
        GitRow::Header {
            section: GitSection::Staged,
            file_count: 1
        }
    )));
    assert!(rows.iter().any(|row| matches!(
        row,
        GitRow::Header {
            section: GitSection::Unstaged,
            file_count: 2
        }
    )));
}

#[test]
fn statuses_are_filtered_into_their_sections() {
    let root = absolute(PathBuf::from("/project")).into_path_buf();
    let snapshot = snapshot(&[
        ("conflict.txt", FileStatus::Unmerged),
        ("new.txt", FileStatus::Untracked),
        ("ignored.log", FileStatus::Ignored),
    ]);
    let rows = build_rows(&root, &[(root.as_path(), &snapshot)]);

    // Ignored 过滤；冲突进入独立分组并排在普通变更之前，untracked 归未暂存组。
    let entries = entry_keys(&rows);
    assert_eq!(
        entries,
        vec![
            (GitSection::Conflict, "conflict.txt".into()),
            (GitSection::Unstaged, "new.txt".into())
        ]
    );
}

#[test]
fn directories_aggregate_status_and_diff_and_respect_expansion() {
    let root = absolute(PathBuf::from("/project")).into_path_buf();
    let modified = FileStatus::Tracked {
        index_status: StatusCode::Unmodified,
        worktree_status: StatusCode::Modified,
    };
    let snapshot = snapshot(&[
        ("src/a.rs", FileStatus::Untracked),
        ("src/sub/b.rs", modified),
    ]);
    let trees = build_section_trees(
        Some(root.as_path()),
        [(root.as_path(), &snapshot)].into_iter(),
    );

    // 折叠：只显示顶层目录 src。
    let rows = flatten_rows(&trees, &HashSet::new(), &HashSet::new());
    assert_eq!(
        entry_keys(&rows),
        vec![(GitSection::Unstaged, "src".into())]
    );
    let src = rows.iter().find_map(|row| match row {
        GitRow::Entry(e) if e.name == "src" => Some(e),
        _ => None,
    });
    let src = src.expect("应有 src 目录行");
    assert!(src.is_dir);
    assert!(!src.expanded);
    // 聚合：modified 的 priority 高于 untracked；
    // diff 取该分组视角的 unstaged 统计求和（每条 0 增 1 删，共 2 条）。
    assert_eq!(src.status, Some(modified));
    assert_eq!(
        src.diff_stat,
        DiffStat {
            added: 0,
            deleted: 2
        }
    );

    // 展开 src：sub（目录优先）与 a.rs 都出现，sub 未展开时其子项不可见。
    let mut expanded = HashSet::new();
    expanded.insert((GitSection::Unstaged, absolute(root.join("src"))));
    let rows = flatten_rows(&trees, &expanded, &HashSet::new());
    assert_eq!(
        entry_keys(&rows),
        vec![
            (GitSection::Unstaged, "src".into()),
            (GitSection::Unstaged, "sub".into()),
            (GitSection::Unstaged, "a.rs".into())
        ]
    );

    // 再展开 sub：叶子出现，目录优先排序（sub 子树在 a.rs 之前）。
    expanded.insert((GitSection::Unstaged, absolute(root.join("src").join("sub"))));
    let rows = flatten_rows(&trees, &expanded, &HashSet::new());
    assert_eq!(
        entry_keys(&rows),
        vec![
            (GitSection::Unstaged, "src".into()),
            (GitSection::Unstaged, "sub".into()),
            (GitSection::Unstaged, "b.rs".into()),
            (GitSection::Unstaged, "a.rs".into())
        ]
    );
}

#[test]
fn change_tree_uses_same_directory_and_name_order_as_file_tree() {
    let root = absolute(PathBuf::from("/project")).into_path_buf();
    let snapshot = snapshot(&[
        ("Zeta/child.rs", FileStatus::Untracked),
        ("file10.rs", FileStatus::Untracked),
        ("File2.rs", FileStatus::Untracked),
        ("file2.rs", FileStatus::Untracked),
    ]);
    let trees = build_section_trees(
        Some(root.as_path()),
        [(root.as_path(), &snapshot)].into_iter(),
    );
    let rows = flatten_rows(&trees, &HashSet::new(), &HashSet::new());
    assert_eq!(
        entry_keys(&rows),
        vec![
            (GitSection::Unstaged, "Zeta".into()),
            (GitSection::Unstaged, "file2.rs".into()),
            (GitSection::Unstaged, "File2.rs".into()),
            (GitSection::Unstaged, "file10.rs".into()),
        ]
    );
}

#[test]
fn active_tree_only_contains_paths_owned_by_the_active_repository() {
    let root = absolute(PathBuf::from("/project")).into_path_buf();
    let vendor = root.join("vendor");
    let outer = snapshot(&[("vendor/lib.rs", FileStatus::Untracked)]);
    let inner = snapshot(&[("lib.rs", FileStatus::Untracked)]);

    let outer_tree = build_section_trees(
        Some(root.as_path()),
        [(root.as_path(), &outer), (vendor.as_path(), &inner)].into_iter(),
    );
    assert!(entry_keys(&flatten_rows(&outer_tree, &HashSet::new(), &HashSet::new())).is_empty());

    let inner_tree = build_section_trees(
        Some(vendor.as_path()),
        [(root.as_path(), &outer), (vendor.as_path(), &inner)].into_iter(),
    );
    assert_eq!(
        entry_keys(&flatten_rows(&inner_tree, &HashSet::new(), &HashSet::new())),
        vec![(GitSection::Unstaged, "lib.rs".into())]
    );
}

#[test]
fn consecutive_single_change_directories_are_display_compressed() {
    let root = absolute(PathBuf::from("/project")).into_path_buf();
    let snapshot = snapshot(&[("src/components/editor/mod.rs", FileStatus::Untracked)]);
    let trees = build_section_trees(
        Some(root.as_path()),
        [(root.as_path(), &snapshot)].into_iter(),
    );
    let expanded = HashSet::from([
        (GitSection::Unstaged, absolute(root.join("src"))),
        (GitSection::Unstaged, absolute(root.join("src/components"))),
        (
            GitSection::Unstaged,
            absolute(root.join("src/components/editor")),
        ),
    ]);

    let rows = flatten_rows(&trees, &expanded, &HashSet::new());
    let entries: Vec<_> = rows
        .iter()
        .filter_map(|row| match row {
            GitRow::Entry(entry) => Some((entry.name.as_str(), entry.path.as_path(), entry.depth)),
            GitRow::Header { .. } | GitRow::Empty(_) => None,
        })
        .collect();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].0, "src/components/editor");
    assert_eq!(entries[0].1, root.join("src/components/editor"));
    assert_eq!(entries[1].0, "mod.rs");
    assert_eq!(entries[1].2, 1);
}

#[test]
fn switching_active_repository_switches_tree_contents() {
    let root = absolute(PathBuf::from("/project")).into_path_buf();
    let other = absolute(PathBuf::from("/other")).into_path_buf();
    let first = snapshot(&[("README.md", FileStatus::Untracked)]);
    let second = snapshot(&[("lib.rs", FileStatus::Untracked)]);
    let repositories = [(root.as_path(), &first), (other.as_path(), &second)];

    let first_rows = flatten_rows(
        &build_section_trees(Some(root.as_path()), repositories.into_iter()),
        &HashSet::new(),
        &HashSet::new(),
    );
    assert_eq!(
        entry_keys(&first_rows),
        vec![(GitSection::Unstaged, "README.md".into())]
    );

    let second_rows = flatten_rows(
        &build_section_trees(Some(other.as_path()), repositories.into_iter()),
        &HashSet::new(),
        &HashSet::new(),
    );
    assert_eq!(
        entry_keys(&second_rows),
        vec![(GitSection::Unstaged, "lib.rs".into())]
    );
}

/// 创建带一个修改文件的临时 git 仓库。
fn test_repo() -> (PathBuf, TempDir) {
    let temp_dir = tempfile::tempdir().expect("应创建临时目录");
    let root = temp_dir.path().to_path_buf();
    run_in(&root, &["git", "init", "-q", "-b", "master"]);
    run_in(&root, &["git", "config", "user.email", "test@example.com"]);
    run_in(&root, &["git", "config", "user.name", "Test User"]);
    fs::write(root.join("tracked.txt"), "第一行\n第二行\n").expect("应写入初始文件");
    run_in(&root, &["git", "add", "tracked.txt"]);
    run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
    fs::write(root.join("tracked.txt"), "修改后的内容\n").expect("应修改文件");
    (root, temp_dir)
}

fn run_in(dir: &Path, args: &[&str]) {
    let output = std::process::Command::new(args[0])
        .args(&args[1..])
        .current_dir(dir)
        .output()
        .expect("应执行成功");
    assert!(
        output.status.success(),
        "命令 {:?} 失败：{}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_test_context(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    cx.update(|cx| zcv_editor::init(cx, Arc::new(LanguageRegistry::new())));
}

#[gpui::test]
fn repository_picker_switches_tree_stats_commit_and_graph_to_selected_repository(
    cx: &mut TestAppContext,
) {
    init_test_context(cx);
    let (root_a, _temp_a) = test_repo();
    let (root_b, _temp_b) = test_repo();
    fs::write(root_a.join("only_a.txt"), "A\n").expect("应写入仓库 A 的独有文件");
    fs::write(root_b.join("only_b.txt"), "B\n").expect("应写入仓库 B 的独有文件");
    run_in(&root_b, &["git", "add", "tracked.txt"]);

    let opened_graph = Rc::new(Cell::new(0));
    let opened_graph_for_panel = opened_graph.clone();
    let project = test_project_roots(vec![root_a.clone(), root_b.clone()], cx);
    let git_store = cx.read_entity(&project, |project, _| project.git_store());
    let project_for_panel = project.clone();
    let (panel, cx) = cx.add_window_view(move |window, cx| {
        let mut panel = VersionControlPanel::new(project_for_panel, window, cx);
        panel.set_on_open_graph(Rc::new(move |_, _| {
            opened_graph_for_panel.set(opened_graph_for_panel.get() + 1);
        }));
        panel
    });
    cx.simulate_resize(size(px(420.), px(740.)));
    cx.run_until_parked();
    cx.run_until_parked();

    let workdir_a = AbsolutePathBuf::canonicalize(&root_a).expect("仓库 A 路径应可规范化");
    let workdir_b = AbsolutePathBuf::canonicalize(&root_b).expect("仓库 B 路径应可规范化");
    assert_eq!(
        active_workdir(cx, &git_store).as_deref(),
        Some(workdir_a.as_path()),
        "首次扫描应选中发现顺序中的第一个仓库"
    );
    let entries = panel_entries(cx, &panel);
    assert!(entries.iter().any(|(_, name)| name == "only_a.txt"));
    assert!(!entries.iter().any(|(_, name)| name == "only_b.txt"));

    // 项目名按钮打开仓库选择器；选择第二项切换到仓库 B。
    let _ = cx.refresh();
    cx.update(|_, _| {});
    let button = cx
        .debug_bounds("version-control-repository-button")
        .expect("项目名按钮应可定位");
    cx.simulate_click(button.center(), Modifiers::default());
    cx.run_until_parked();
    let match_b = cx
        .debug_bounds("picker-match-1")
        .expect("仓库选择器应列出第二个仓库");
    cx.simulate_click(match_b.center(), Modifiers::default());
    cx.run_until_parked();
    cx.run_until_parked();

    assert_eq!(
        active_workdir(cx, &git_store).as_deref(),
        Some(workdir_b.as_path()),
        "仓库选择器应成为活动仓库的唯一用户入口"
    );
    let entries = panel_entries(cx, &panel);
    assert!(entries.iter().any(|(_, name)| name == "only_b.txt"));
    assert!(!entries.iter().any(|(_, name)| name == "only_a.txt"));

    // 提交按钮作用于界面所示仓库 B，A 的提交信息保持不变。
    cx.update_entity(&panel, |panel, cx| {
        panel.commit_states[&workdir_b]
            .editor
            .update(cx, |editor, cx| editor.set_text("提交 B 的修改", cx));
    });
    let _ = cx.refresh();
    cx.update(|_, _| {});
    let commit_button = cx
        .debug_bounds("version-control-commit-button")
        .expect("当前仓库提交按钮应可定位");
    cx.simulate_click(commit_button.center(), Modifiers::default());
    cx.run_until_parked();
    cx.run_until_parked();
    cx.update(|_, cx| {
        let store = git_store.read(cx);
        let a = store
            .repositories()
            .find(|(path, _)| *path == workdir_a.as_path())
            .expect("仓库 A 应存在");
        let b = store
            .repositories()
            .find(|(path, _)| *path == workdir_b.as_path())
            .expect("仓库 B 应存在");
        assert_eq!(a.1.last_commit_message.as_deref(), Some("initial"));
        assert_eq!(b.1.last_commit_message.as_deref(), Some("提交 B 的修改"));
        assert_eq!(
            panel.read(cx).commit_states[&workdir_b]
                .editor
                .read(cx)
                .text(cx),
            ""
        );
    });

    // 撤销提交也作用于当前活动仓库。
    let undo_button = cx
        .debug_bounds("version-control-uncommit-button")
        .expect("当前仓库撤销提交按钮应可定位");
    cx.simulate_click(undo_button.center(), Modifiers::default());
    cx.run_until_parked();
    cx.run_until_parked();
    cx.update(|_, cx| {
        let store = git_store.read(cx);
        let b = store
            .repositories()
            .find(|(path, _)| *path == workdir_b.as_path())
            .expect("仓库 B 应存在");
        assert_eq!(b.1.last_commit_message.as_deref(), Some("initial"));
        assert_eq!(
            panel.read(cx).commit_states[&workdir_b]
                .editor
                .read(cx)
                .text(cx),
            "提交 B 的修改"
        );
    });

    let graph_button = cx
        .debug_bounds("version-control-git-graph-button")
        .expect("提交图入口应可定位");
    cx.simulate_click(graph_button.center(), Modifiers::default());
    assert_eq!(opened_graph.get(), 1);
}

#[gpui::test]
fn repository_switch_preserves_drafts_and_removal_falls_back_to_remaining_repository(
    cx: &mut TestAppContext,
) {
    init_test_context(cx);
    let (root_a, _temp_a) = test_repo();
    let (root_b, _temp_b) = test_repo();
    let project = test_project_roots(vec![root_a.clone(), root_b.clone()], cx);
    let git_store = cx.read_entity(&project, |project, _| project.git_store());
    let project_for_panel = project.clone();
    let (panel, cx) = cx
        .add_window_view(move |window, cx| VersionControlPanel::new(project_for_panel, window, cx));
    cx.run_until_parked();
    cx.run_until_parked();
    let workdir_a = AbsolutePathBuf::canonicalize(&root_a).expect("仓库 A 路径应可规范化");
    let workdir_b = AbsolutePathBuf::canonicalize(&root_b).expect("仓库 B 路径应可规范化");

    cx.update_entity(&panel, |panel, cx| {
        panel.commit_states[&workdir_a]
            .editor
            .update(cx, |editor, cx| editor.set_text("A 的草稿", cx));
        panel.commit_states[&workdir_b]
            .editor
            .update(cx, |editor, cx| editor.set_text("B 的草稿", cx));
        panel.select_repository(workdir_b.clone(), cx);
    });
    cx.run_until_parked();
    assert_eq!(
        active_workdir(cx, &git_store).as_deref(),
        Some(workdir_b.as_path())
    );
    cx.read_entity(&panel, |panel, cx| {
        assert_eq!(
            panel.commit_states[&workdir_b].editor.read(cx).text(cx),
            "B 的草稿"
        );
    });

    cx.update_entity(&panel, |panel, cx| {
        panel.select_repository(workdir_a.clone(), cx);
    });
    cx.run_until_parked();
    cx.read_entity(&panel, |panel, cx| {
        assert_eq!(
            panel.commit_states[&workdir_a].editor.read(cx).text(cx),
            "A 的草稿",
            "切回仓库 A 后其草稿应保留"
        );
    });

    cx.update_entity(&panel, |panel, cx| {
        panel.select_repository(workdir_b.clone(), cx);
    });
    cx.run_until_parked();
    project.update(cx, |project, cx| {
        project
            .remove_worktree(&root_b, cx)
            .expect("应移除仓库 B 的项目根")
    });
    cx.run_until_parked();
    cx.run_until_parked();
    assert_eq!(
        active_workdir(cx, &git_store).as_deref(),
        Some(workdir_a.as_path()),
        "活动仓库消失后应回退到仍存在的首个仓库"
    );
    cx.read_entity(&panel, |panel, cx| {
        assert!(!panel.commit_states.contains_key(&workdir_b));
        assert_eq!(
            panel.commit_states[&workdir_a].editor.read(cx).text(cx),
            "A 的草稿"
        );
    });
}

#[gpui::test]
fn stats_row_wraps_and_keeps_graph_button_right_aligned(cx: &mut TestAppContext) {
    init_test_context(cx);
    let (root, _temp) = test_repo();
    let project = test_project(root, cx);
    let (_, cx) =
        cx.add_window_view(move |window, cx| VersionControlPanel::new(project, window, cx));

    let bounds = |cx: &mut VisualTestContext, width| {
        cx.simulate_resize(size(width, px(740.)));
        cx.run_until_parked();
        let _ = cx.refresh();
        cx.update(|_, _| {});
        (
            cx.debug_bounds("version-control-diff-counts")
                .expect("应显示变更行数统计"),
            cx.debug_bounds("version-control-git-graph-button")
                .expect("应显示版本控制图按钮"),
        )
    };

    let (wide_counts, wide_graph) = bounds(cx, px(420.));
    assert!(wide_graph.left() >= wide_counts.right());
    let (counts, graph) = bounds(cx, px(75.));
    assert!(graph.top() >= counts.bottom(), "版本控制图按钮应换行");
    assert_eq!(px(75.) - graph.right(), px(420.) - wide_graph.right());
}

#[gpui::test]
fn repository_header_wraps_long_names_and_keeps_remote_buttons_right_aligned(
    cx: &mut TestAppContext,
) {
    init_test_context(cx);
    let (root, _temp) = test_repo();
    let parent = tempfile::tempdir().expect("应创建仓库父目录");
    let long_root = parent.path().join("repository".repeat(7));
    fs::rename(root, &long_root).expect("应设置长仓库名");
    let branch_name = "feature".repeat(10);
    run_in(&long_root, &["git", "branch", "-m", &branch_name]);
    run_in(&long_root, &["git", "remote", "add", "origin", "."]);
    let project = test_project(long_root, cx);
    let (_, cx) =
        cx.add_window_view(move |window, cx| VersionControlPanel::new(project, window, cx));

    let bounds = |cx: &mut VisualTestContext, width| {
        cx.simulate_resize(size(width, px(740.)));
        cx.run_until_parked();
        let _ = cx.refresh();
        cx.update(|_, _| {});
        (
            cx.debug_bounds("version-control-repository-button")
                .expect("应显示仓库名"),
            cx.debug_bounds("version-control-branch-button")
                .expect("应显示分支名"),
            cx.debug_bounds("version-control-remote-operations")
                .expect("应显示远程操作"),
        )
    };

    let (wide_repo, wide_branch, wide_operations) = bounds(cx, px(1600.));
    assert!(wide_branch.left() >= wide_repo.right());
    assert!(wide_operations.left() >= wide_branch.right());

    let (repo, branch, operations) = bounds(cx, px(240.));
    assert!(repo.right() <= px(240.) && branch.right() <= px(240.));
    assert!(repo.size.height > wide_repo.size.height, "仓库名应多行显示");
    assert!(
        branch.size.height > wide_branch.size.height,
        "分支名应多行显示"
    );
    assert!(branch.top() >= repo.bottom(), "分支应移至下一行");
    assert!(operations.top() >= branch.bottom(), "远程操作应移至下一行");
    assert_eq!(
        px(240.) - operations.right(),
        px(1600.) - wide_operations.right()
    );
}

#[gpui::test]
fn panel_branch_picker_switches_active_repository_branch(cx: &mut TestAppContext) {
    init_test_context(cx);
    let (root, _temp) = test_repo();
    run_in(&root, &["git", "branch", "feature"]);
    let project = test_project(root.clone(), cx);
    let git_store = cx.read_entity(&project, |project, _| project.git_store());
    let selected = Rc::new(RefCell::new(None));
    let selected_for_panel = selected.clone();
    let store_for_panel = git_store.clone();
    let project_for_panel = project.clone();
    let (_panel, cx) = cx.add_window_view(move |window, cx| {
        let mut panel = VersionControlPanel::new(project_for_panel, window, cx);
        panel.set_on_branch_selected(Rc::new(move |action, _, cx| {
            if let GitBranchAction::Checkout(name) = action {
                *selected_for_panel.borrow_mut() = Some(name.clone());
                store_for_panel.update(cx, |store, cx| {
                    let _task = store.checkout_branch_with_result(name, cx);
                });
            }
        }));
        panel
    });
    cx.run_until_parked();
    cx.run_until_parked();
    let _ = cx.refresh();
    cx.update(|_, _| {});
    let button = cx
        .debug_bounds("version-control-branch-button")
        .expect("分支名按钮应可定位");
    cx.simulate_click(button.center(), Modifiers::default());
    cx.run_until_parked();
    let branch_index = cx.read_entity(&git_store, |store, _| {
        store
            .active_branch_list()
            .expect("应能读取分支列表")
            .iter()
            .position(|branch| branch.name == "feature")
            .expect("应存在 feature 分支")
    });
    let selector = Box::leak(format!("picker-match-{branch_index}").into_boxed_str());
    let branch = cx
        .debug_bounds(selector)
        .expect("分支选择器应列出 feature 分支");
    cx.simulate_click(branch.center(), Modifiers::default());
    cx.run_until_parked();
    cx.run_until_parked();
    assert_eq!(selected.borrow().as_deref(), Some("feature"));
    assert_eq!(
        active_branch(cx, &git_store).as_deref(),
        Some("feature"),
        "面板分支选择器应切换当前活动仓库的分支"
    );
}

#[gpui::test]
fn empty_state_initializes_repository_and_builds_section_tree(cx: &mut TestAppContext) {
    init_test_context(cx);
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let project_root = directory.path().to_path_buf();
    let project = test_project(project_root.clone(), cx);
    let project_for_panel = project.clone();
    let (panel, cx) = cx
        .add_window_view(move |window, cx| VersionControlPanel::new(project_for_panel, window, cx));
    cx.run_until_parked(); // 首次扫描完成：无仓库，行模型为空。

    let panel_focused = cx.update(|window, cx| panel.read(cx).focus.contains_focused(window, cx));
    assert!(!panel_focused, "前置条件：面板不应持有焦点");

    let _ = cx.refresh();
    cx.update(|_, _| {});
    let bounds = cx
        .debug_bounds("version-control-init")
        .expect("初始化仓库按钮应可定位");
    cx.simulate_click(bounds.center(), Modifiers::default());
    cx.run_until_parked(); // init job 完成
    cx.run_until_parked(); // 其触发的重扫落地 → Repositories 事件 → 重建行模型

    cx.read_entity(&panel, |panel, cx| {
        assert!(
            panel
                .project
                .read(cx)
                .git_store()
                .read(cx)
                .has_repositories(),
            "焦点不在面板时点击初始化仓库按钮仍应创建仓库"
        );
        let rows = panel.state.borrow().row_snapshot();
        let headers: Vec<_> = rows
            .iter()
            .filter_map(|row| match row {
                GitRow::Header { section, .. } => Some(section.label()),
                GitRow::Empty(_) | GitRow::Entry(_) => None,
            })
            .collect();
        assert_eq!(headers, vec!["冲突", "已暂存", "未暂存"]);
    });
}

#[gpui::test]
fn first_click_on_unfocused_panel_focuses_and_opens(cx: &mut TestAppContext) {
    init_test_context(cx);
    let (root, _temp) = test_repo();
    let project_root = root.clone();
    let open_count = Rc::new(Cell::new(0));
    let last_focus_opened = Rc::new(Cell::new(true));
    let opened_path = Rc::new(RefCell::new(None));
    let callback_count = Rc::clone(&open_count);
    let callback_focus = Rc::clone(&last_focus_opened);
    let callback_path = Rc::clone(&opened_path);

    let project = test_project(project_root.clone(), cx);
    let (panel, cx) = cx.add_window_view(move |window, cx| {
        let mut panel = VersionControlPanel::new(project, window, cx);
        panel.set_on_open_file(Rc::new(move |_, path, focus_opened_item, _, _| {
            callback_count.set(callback_count.get() + 1);
            callback_focus.set(focus_opened_item);
            *callback_path.borrow_mut() = Some(path);
        }));
        panel
    });
    cx.run_until_parked(); // 扫描完成，行模型就绪。

    // 行布局：三组标题/空提示，加上未暂存组中的一个文件行。
    let row_count = cx.read_entity(&panel, |panel, _| panel.state.borrow().rows().len());
    assert_eq!(row_count, 6);

    // 扫描完成后强制重绘：首帧是空态，点击命中测试需要最新帧的行布局。
    // refresh 只入队 effect，需要一次 update 周期 flush 后窗口才真正重绘。
    let _ = cx.refresh();
    cx.update(|_, _| {});
    cx.run_until_parked();

    // 单击 tracked.txt 行内容区（x=100 避开行首复选框）：
    // 行高为 ui_line()，以临时标签打开（focus_opened_item=false）。
    let row_height = cx.update(|window, cx| tree_row_height(window, cx));
    let click = |cx: &mut VisualTestContext| {
        // y 加 1 行偏移：顶部统计行占一行高度；
        // 冲突组固定在顶部后再经过两个冲突行。
        cx.simulate_click(
            point(px(100.), px(f32::from(row_height) * 7.5)),
            Modifiers::default(),
        );
        cx.run_until_parked();
    };

    // 首击同时完成聚焦、选中与单击预览，用户无需先额外点击一次聚焦。
    click(cx);
    assert_eq!(open_count.get(), 1, "未聚焦首击也应打开文件");
    assert_eq!(
        opened_path.borrow().as_deref(),
        Some(
            AbsolutePathBuf::canonicalize(&root.join("tracked.txt"))
                .unwrap()
                .as_path(),
        ),
        "首击应把实际点击的文件路径传给打开回调"
    );
    let panel_focused = cx.update(|window, cx| panel.read(cx).focus.contains_focused(window, cx));
    assert!(panel_focused, "首击应聚焦变更面板");

    assert!(
        !last_focus_opened.get(),
        "单击应打开临时标签但焦点留在面板（focus_opened_item=false）"
    );
}

#[gpui::test]
fn keyboard_navigation_opens_file_as_active(cx: &mut TestAppContext) {
    init_test_context(cx);
    let (root, _temp) = test_repo();
    let project_root = root.clone();
    let open_count = Rc::new(Cell::new(0));
    let last_focus_opened = Rc::new(Cell::new(false));
    let callback_count = Rc::clone(&open_count);
    let callback_focus = Rc::clone(&last_focus_opened);

    let project = test_project(project_root.clone(), cx);
    let (panel, cx) = cx.add_window_view(move |window, cx| {
        cx.bind_keys([
            KeyBinding::new("down", SelectNext, Some("GitPanel && ChangesList")),
            KeyBinding::new("enter", Activate, Some("GitPanel && ChangesList")),
        ]);
        let mut panel = VersionControlPanel::new(project, window, cx);
        panel.set_on_open_file(Rc::new(move |_, _, focus_opened_item, _, _| {
            callback_count.set(callback_count.get() + 1);
            callback_focus.set(focus_opened_item);
        }));
        panel
    });
    cx.update(|window, cx| {
        let focus = panel.read(cx).focus.clone();
        window.focus(&focus, cx);
    });
    cx.run_until_parked();

    // down 选中第一个条目行（跳过分组头），enter 激活打开。
    cx.simulate_keystrokes("down");
    cx.simulate_keystrokes("enter");

    assert_eq!(open_count.get(), 1, "enter 应打开选中的文件");
    assert!(
        last_focus_opened.get(),
        "enter 打开应为激活（focus_opened_item=true）"
    );
}

#[gpui::test]
fn staged_section_opens_the_staged_project_diff(cx: &mut TestAppContext) {
    init_test_context(cx);
    let (root, _temp) = test_repo();
    run_in(&root, &["git", "add", "tracked.txt"]);
    let opened_kind = Rc::new(Cell::new(None));
    let callback_kind = Rc::clone(&opened_kind);
    let project_root = root.clone();
    let project = test_project(project_root, cx);
    let (panel, cx) = cx.add_window_view(move |window, cx| {
        let mut panel = VersionControlPanel::new(project, window, cx);
        panel.set_on_open_file(Rc::new(move |kind, _, _, _, _| {
            callback_kind.set(Some(kind));
        }));
        panel
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.state.borrow_mut().select_down();
            panel.activate_selected(true, window, cx);
        });
    });

    assert_eq!(opened_kind.get(), Some(ProjectDiffKind::Staged));
}

/// 面板行模型中各条目的 (分组, 显示名) 序列。
fn section_entries(panel: &VersionControlPanel) -> Vec<(GitSection, String)> {
    panel
        .state
        .borrow()
        .rows()
        .iter()
        .filter_map(|row| match row {
            GitRow::Entry(entry) => Some((entry.section, entry.name.clone())),
            GitRow::Header { .. } | GitRow::Empty(_) => None,
        })
        .collect()
}

#[gpui::test]
fn directories_expand_by_default(cx: &mut TestAppContext) {
    init_test_context(cx);
    // 目录下有变更文件：首次构建后目录默认展开，子文件可见。
    let (root, _temp) = test_repo();
    fs::create_dir_all(root.join("src")).expect("应创建目录");
    fs::write(root.join("src/a.txt"), "改动\n").expect("应写入文件");

    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) =
        cx.add_window_view(move |window, cx| VersionControlPanel::new(project, window, cx));
    cx.run_until_parked(); // 扫描 + 重建 + 首次全展开

    let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert_eq!(
        entries,
        vec![
            (GitSection::Unstaged, "src".into()),
            (GitSection::Unstaged, "a.txt".into()),
            (GitSection::Unstaged, "tracked.txt".into())
        ],
        "首次构建后目录应默认展开"
    );
}

#[gpui::test]
fn new_directories_expand_by_default_while_manually_collapsed_stay(cx: &mut TestAppContext) {
    init_test_context(cx);
    let (root, _temp) = test_repo();
    fs::create_dir_all(root.join("src")).expect("应创建目录");
    fs::write(root.join("src/a.txt"), "改动\n").expect("应写入文件");

    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) =
        cx.add_window_view(move |window, cx| VersionControlPanel::new(project, window, cx));
    cx.run_until_parked(); // 扫描 + 重建

    // 首次：src 默认展开，子文件可见。
    let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert!(entries.contains(&(GitSection::Unstaged, "a.txt".into())));

    // 用户折叠 src（模拟点击目录行折叠）；树键用 canonicalize 后的根（macOS /var → /private/var）。
    let canonical_root = AbsolutePathBuf::canonicalize(&root)
        .expect("仓库根应可规范化")
        .into_path_buf();
    cx.update_entity(&panel, |panel, cx| {
        let key = (GitSection::Unstaged, absolute(canonical_root.join("src")));
        panel.state.borrow_mut().collapse(&key);
        panel.rebuild_rows(cx);
    });
    let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert!(
        !entries.contains(&(GitSection::Unstaged, "a.txt".into())),
        "用户折叠的目录应保持折叠"
    );

    // 新目录 src2 出现：增量刷新 GitStore（fs watch 是真实线程，测试里走公开刷新入口）→ Statuses 事件 → 面板重建。
    fs::create_dir_all(root.join("src2")).expect("应创建目录");
    fs::write(root.join("src2/b.txt"), "改动\n").expect("应写入文件");
    let git_store = cx.read_entity(&panel, |panel, cx| {
        panel.project.read(cx).git_store().clone()
    });
    cx.update_entity(&git_store, |store, cx| {
        store.refresh_statuses_for_paths(&[canonical_root.join("src2")], cx);
    });
    cx.run_until_parked(); // 增量扫描落地
    cx.run_until_parked(); // Statuses 事件触发的面板重建落地
    let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert!(entries.contains(&(GitSection::Unstaged, "src2".into())));
    assert!(
        entries.contains(&(GitSection::Unstaged, "b.txt".into())),
        "新出现的目录应默认展开"
    );
    assert!(
        !entries.contains(&(GitSection::Unstaged, "a.txt".into())),
        "用户折叠的 src 在重建后仍保持折叠"
    );
}

#[gpui::test]
fn staging_a_collapsed_directory_opens_new_section_by_default(cx: &mut TestAppContext) {
    init_test_context(cx);
    let (root, _temp) = test_repo();
    fs::create_dir_all(root.join("src")).expect("应创建目录");
    fs::write(root.join("src/a.txt"), "改动\n").expect("应写入文件");
    let project = test_project(root.clone(), cx);
    let (panel, cx) =
        cx.add_window_view(move |window, cx| VersionControlPanel::new(project, window, cx));
    cx.run_until_parked();

    let root = AbsolutePathBuf::canonicalize(&root)
        .expect("仓库根应可规范化")
        .into_path_buf();
    let directory = absolute(root.join("src"));
    cx.update_entity(&panel, |panel, cx| {
        let key = (GitSection::Unstaged, directory.clone());
        panel.state.borrow_mut().collapse(&key);
        panel.rebuild_rows(cx);
        panel.toggle_staged_for(GitSection::Unstaged, &directory, cx);
    });
    cx.run_until_parked();
    cx.run_until_parked();

    let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert!(entries.contains(&(GitSection::Staged, "src".into())));
    assert!(
        entries.contains(&(GitSection::Staged, "a.txt".into())),
        "目标分区中新出现的目录应默认展开"
    );

    cx.update_entity(&panel, |panel, cx| {
        panel.toggle_staged_for(GitSection::Staged, &directory, cx);
    });
    cx.run_until_parked();
    cx.run_until_parked();
    let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert!(entries.contains(&(GitSection::Unstaged, "src".into())));
    assert!(
        entries.contains(&(GitSection::Unstaged, "a.txt".into())),
        "源分区目录消失后再次出现应默认展开"
    );
}

#[gpui::test]
fn same_directory_in_staged_and_unstaged_expands_independently(cx: &mut TestAppContext) {
    init_test_context(cx);
    let (root, _temp) = test_repo();
    fs::create_dir_all(root.join("src")).expect("应创建目录");
    fs::write(root.join("src/a.txt"), "已暂存内容\n").expect("应写入文件");
    run_in(&root, &["git", "add", "src/a.txt"]);
    fs::write(root.join("src/a.txt"), "已暂存内容\n未暂存内容\n").expect("应修改文件");

    let project = test_project(root.clone(), cx);
    let (panel, cx) =
        cx.add_window_view(move |window, cx| VersionControlPanel::new(project, window, cx));
    cx.run_until_parked();

    let root = AbsolutePathBuf::canonicalize(&root)
        .expect("仓库根应可规范化")
        .into_path_buf();
    let staged = (GitSection::Staged, absolute(root.join("src")));
    let unstaged = (GitSection::Unstaged, staged.1.clone());
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.state.borrow_mut().select(unstaged.clone());
            panel.activate_selected(true, window, cx);
        });
    });
    let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert!(entries.contains(&(GitSection::Staged, "a.txt".into())));
    assert!(!entries.contains(&(GitSection::Unstaged, "a.txt".into())));

    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.state.borrow_mut().select(staged.clone());
            panel.activate_selected(true, window, cx);
        });
    });
    let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert!(!entries.contains(&(GitSection::Staged, "a.txt".into())));
    assert!(!entries.contains(&(GitSection::Unstaged, "a.txt".into())));

    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.state.borrow_mut().select(unstaged.clone());
            panel.activate_selected(true, window, cx);
        });
    });
    let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert!(!entries.contains(&(GitSection::Staged, "a.txt".into())));
    assert!(entries.contains(&(GitSection::Unstaged, "a.txt".into())));
}

#[gpui::test]
fn section_header_collapse_hides_entries_and_expand_restores(cx: &mut TestAppContext) {
    init_test_context(cx);
    let (root, _temp) = test_repo();
    fs::write(root.join("tracked.txt"), "改动\n").expect("应写入文件");

    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) =
        cx.add_window_view(move |window, cx| VersionControlPanel::new(project, window, cx));
    cx.run_until_parked(); // 扫描 + 重建

    // 初始：未暂存组条目可见。
    let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert!(entries.contains(&(GitSection::Unstaged, "tracked.txt".into())));

    let header_index = cx.read_entity(&panel, |panel, _| {
        panel
            .state
            .borrow()
            .rows()
            .iter()
            .position(|row| {
                matches!(
                    row,
                    GitRow::Header {
                        section: GitSection::Unstaged,
                        ..
                    }
                )
            })
            .expect("未暂存分区标题应存在")
    });
    let row_height = cx.update(|window, cx| tree_row_height(window, cx));
    let click_disclosure = |cx: &mut VisualTestContext| {
        cx.simulate_click(
            point(
                px(14.),
                px(f32::from(row_height) * (header_index as f32 + 2.5)),
            ),
            Modifiers::default(),
        );
        cx.run_until_parked();
    };
    let _ = cx.refresh();
    cx.update(|_, _| {});
    cx.run_until_parked();

    // 点击行首折叠控件只能切换一次；若事件冒泡到整行，会立即切回展开。
    click_disclosure(cx);
    let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert!(entries.is_empty(), "折叠后该分区条目应隐藏（仅剩标题行）");

    click_disclosure(cx);
    let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert!(entries.contains(&(GitSection::Unstaged, "tracked.txt".into())));
}

#[gpui::test]
fn collapsed_section_keeps_header_checkbox_and_select_all(cx: &mut TestAppContext) {
    init_test_context(cx);
    // 分组折叠只隐藏条目行；只要该组仍有文件，标题行全选复选框就必须常驻且可整组暂存。
    let (root, _temp) = test_repo();
    fs::write(root.join("tracked.txt"), "改动\n").expect("应写入文件");

    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) =
        cx.add_window_view(move |window, cx| VersionControlPanel::new(project, window, cx));
    cx.run_until_parked();

    cx.update_entity(&panel, |panel, cx| {
        panel.toggle_section_collapsed(GitSection::Unstaged, cx);
    });
    cx.run_until_parked();

    // 冲突组固定在顶部，折叠后的未暂存标题行是第 6 个列表行。
    assert_hover_tooltip(cx, 4);

    // 折叠态下整组操作也必须生效（不能因条目不可见而找不到路径）。
    cx.update_entity(&panel, |panel, cx| {
        panel.toggle_section_all(GitSection::Unstaged, cx);
    });
    cx.run_until_parked();
    cx.run_until_parked();
    let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert_eq!(
        entries,
        vec![(GitSection::Staged, "tracked.txt".into())],
        "折叠态标题行全选应把该组文件全部暂存"
    );
}

#[gpui::test]
fn header_checkbox_selects_all_entries_in_section(cx: &mut TestAppContext) {
    init_test_context(cx);
    // 两个未暂存文件：header 全选 → 全部暂存，未暂存组清空、已暂存组出现两项。
    let (root, _temp) = test_repo();
    fs::write(root.join("tracked.txt"), "改动\n").expect("应写入文件");
    fs::write(root.join("second.txt"), "第二个文件\n").expect("应写入文件");

    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) =
        cx.add_window_view(move |window, cx| VersionControlPanel::new(project, window, cx));
    cx.run_until_parked();

    let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert_eq!(
        entries
            .iter()
            .filter(|(s, _)| *s == GitSection::Unstaged)
            .count(),
        2,
        "两个文件都在未暂存组"
    );

    cx.update_entity(&panel, |panel, cx| {
        panel.toggle_section_all(GitSection::Unstaged, cx);
    });
    cx.run_until_parked(); // stage job 完成
    cx.run_until_parked(); // 其触发的重扫落地
    let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert_eq!(
        entries
            .iter()
            .filter(|(s, _)| *s == GitSection::Staged)
            .count(),
        2,
        "全选后两个文件都应进入已暂存组"
    );
    assert!(
        entries.iter().all(|(s, _)| *s == GitSection::Staged),
        "未暂存组应清空"
    );

    // 已暂存组 header 全选：全部取消暂存，回到未暂存组。
    cx.update_entity(&panel, |panel, cx| {
        panel.toggle_section_all(GitSection::Staged, cx);
    });
    cx.run_until_parked();
    cx.run_until_parked();
    let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert_eq!(
        entries
            .iter()
            .filter(|(s, _)| *s == GitSection::Unstaged)
            .count(),
        2,
        "取消全选后两个文件都应回到未暂存组"
    );
}

#[gpui::test]
fn space_toggles_staging_and_moves_row_between_sections(cx: &mut TestAppContext) {
    init_test_context(cx);
    let (root, _temp) = test_repo();
    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) = cx.add_window_view(move |window, cx| {
        cx.bind_keys([
            KeyBinding::new("down", SelectNext, Some("GitPanel && ChangesList")),
            KeyBinding::new("space", ToggleStaged, Some("GitPanel && ChangesList")),
        ]);
        VersionControlPanel::new(project, window, cx)
    });
    cx.update(|window, cx| {
        let focus = panel.read(cx).focus.clone();
        window.focus(&focus, cx);
    });
    cx.run_until_parked();

    // down 选中未暂存组的 tracked.txt → space 暂存 → 行移到已暂存组。
    cx.simulate_keystrokes("down");
    cx.simulate_keystrokes("space");
    cx.run_until_parked(); // stage job 完成
    cx.run_until_parked(); // 其触发的重扫落地
    let sections = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert_eq!(sections, vec![(GitSection::Staged, "tracked.txt".into())]);

    // 再 space：取消暂存 → 回到未暂存组。
    // 只有一行时没有相邻行，重绘后由 ensure_selected 选择唯一的文件行。
    let _ = cx.refresh();
    cx.update(|_, _| {});
    cx.run_until_parked();
    cx.simulate_keystrokes("space");
    cx.run_until_parked();
    cx.run_until_parked();
    let sections = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert_eq!(sections, vec![(GitSection::Unstaged, "tracked.txt".into())]);
}

#[gpui::test]
fn space_in_commit_editor_inserts_text_without_toggling_staging(cx: &mut TestAppContext) {
    init_test_context(cx);
    let (root, _temp) = test_repo();
    let project_root = root.clone();
    let project = test_project(project_root, cx);
    let (panel, cx) = cx.add_window_view(move |window, cx| {
        init(cx).expect("应注册内置快捷键");
        VersionControlPanel::new(project, window, cx)
    });
    cx.run_until_parked();
    cx.run_until_parked();

    let editor_focus = cx.read_entity(&panel, |panel, cx| {
        panel.test_commit_editor().read(cx).focus_handle()
    });
    cx.update(|window, cx| window.focus(&editor_focus, cx));
    let _ = cx.refresh();
    cx.update(|_, _| {});

    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            let context = panel.dispatch_context(window, cx);
            assert!(context.contains("CommitEditor"));
            assert!(!context.contains("ChangesList"));
        });
    });

    let before = cx.read_entity(&panel, |panel, _| section_entries(panel));
    cx.simulate_keystrokes("space");
    cx.run_until_parked();

    cx.update(|_, cx| {
        assert_eq!(
            panel.read(cx).test_commit_editor().read(cx).text(cx),
            " ",
            "提交信息编辑器聚焦时，空格应输入文本"
        );
    });
    let after = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert_eq!(after, before, "提交信息中的空格不应切换暂存状态");
}

#[gpui::test]
fn selection_border_only_shows_when_changes_tree_is_focused(cx: &mut TestAppContext) {
    init_test_context(cx);
    let (root, _temp) = test_repo();
    let project = test_project(root, cx);
    let (panel, cx) =
        cx.add_window_view(move |window, cx| VersionControlPanel::new(project, window, cx));
    cx.run_until_parked();
    cx.run_until_parked();

    let changes_tree_focus = cx.read_entity(&panel, |panel, _| panel.focus.clone());
    cx.update(|window, cx| window.focus(&changes_tree_focus, cx));
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("version-control-selection-border")
            .is_some(),
        "变更树聚焦时应显示选中框"
    );

    let commit_editor_focus = cx.read_entity(&panel, |panel, cx| {
        panel.test_commit_editor().read(cx).focus_handle()
    });
    cx.update(|window, cx| window.focus(&commit_editor_focus, cx));
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("version-control-selection-border")
            .is_none(),
        "提交信息编辑器聚焦时不应显示变更树选中框"
    );
    assert!(
        cx.read_entity(&panel, |panel, _| panel.state.borrow().selected().is_some()),
        "切换焦点只隐藏选中框，不应清除变更树选中状态"
    );
}

/// 悬停指定行尾的复选框并断言 tooltip 气泡出现。
///
/// 测试时钟不会自动推进：手动拨过 500ms tooltip 显示延迟后再渲染一帧。
/// 顶部统计行占一行高度，行坐标加 1 行偏移。
fn assert_hover_tooltip(cx: &mut VisualTestContext, row_index: usize) {
    let row_height = cx.update(|window, cx| tree_row_height(window, cx));
    cx.simulate_mouse_move(
        point(
            px(1907.),
            px(f32::from(row_height) * (row_index as f32 + 2.5)),
        ),
        None,
        Modifiers::default(),
    );
    cx.executor().advance_clock(Duration::from_millis(600));
    cx.run_until_parked();
    let _ = cx.refresh();
    cx.update(|_, _| {}); // 触发一帧渲染，tooltip 请求进入 frame
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("tooltip-view").is_some(),
        "悬停复选框应显示 tooltip 气泡"
    );
}

#[gpui::test]
fn hovering_unchecked_checkbox_shows_tooltip(cx: &mut TestAppContext) {
    init_test_context(cx);
    let (root, _temp) = test_repo();
    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (_panel, cx) =
        cx.add_window_view(move |window, cx| VersionControlPanel::new(project, window, cx));
    cx.run_until_parked();
    let _ = cx.refresh();
    cx.update(|_, _| {});
    cx.run_until_parked();

    // 未暂存组 tracked.txt 行尾复选框；空的已暂存组占一行提示。
    assert_hover_tooltip(cx, 5);
}

#[gpui::test]
fn hovering_checkbox_shows_tooltip_with_many_rows(cx: &mut TestAppContext) {
    init_test_context(cx);
    // 复现真实场景：大量变更文件（行数超过可视区域，触发 uniform_list 虚拟化）。
    let (root, _temp) = test_repo();
    for index in 0..40 {
        fs::write(
            root.join(format!("file-{index:02}.txt")),
            format!("内容 {index}\n"),
        )
        .expect("应写入文件");
    }
    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (_panel, cx) =
        cx.add_window_view(move |window, cx| VersionControlPanel::new(project, window, cx));
    cx.run_until_parked();
    let _ = cx.refresh();
    cx.update(|_, _| {});
    cx.run_until_parked();

    // 悬停可视区第 5 行（未暂存组的一个文件）行尾复选框；顶部统计行占一行，坐标加偏移。
    let row_height = cx.update(|window, cx| tree_row_height(window, cx));
    let hover_y = f32::from(row_height) * 8.5;
    cx.simulate_mouse_move(point(px(1907.), px(hover_y)), None, Modifiers::default());
    cx.executor().advance_clock(Duration::from_millis(600));
    cx.run_until_parked();
    let _ = cx.refresh();
    cx.update(|_, _| {});
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("tooltip-view").is_some(),
        "多行场景悬停复选框也应显示 tooltip 气泡"
    );
}

#[gpui::test]
fn hover_tooltip_survives_row_rebuild(cx: &mut TestAppContext) {
    init_test_context(cx);
    // 复现真实场景：行重建（git 操作/滚动后 rebuild_rows）后再次 hover 复选框，
    // tooltip 应仍然显示。
    let (root, _temp) = test_repo();
    fs::write(root.join("second.txt"), "第二个文件\n").expect("应写入文件");
    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) =
        cx.add_window_view(move |window, cx| VersionControlPanel::new(project, window, cx));
    cx.run_until_parked();
    let _ = cx.refresh();
    cx.update(|_, _| {});
    cx.run_until_parked();

    // 先悬停未暂存组的一个文件复选框，确认 tooltip 正常；顶部统计行占一行，坐标加偏移。
    let row_height = cx.update(|window, cx| tree_row_height(window, cx));
    cx.simulate_mouse_move(
        point(px(1907.), px(f32::from(row_height) * 7.5)),
        None,
        Modifiers::default(),
    );
    cx.executor().advance_clock(Duration::from_millis(600));
    cx.run_until_parked();
    let _ = cx.refresh();
    cx.update(|_, _| {});
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("tooltip-view").is_some(),
        "重建前悬停复选框应显示 tooltip"
    );

    // 触发行重建：暂存第二个文件（空格选中并暂存），行集合变化。
    cx.update(|window, cx| {
        let focus = panel.read(cx).focus.clone();
        window.focus(&focus, cx);
    });
    cx.simulate_keystrokes("down");
    cx.simulate_keystrokes("space");
    cx.run_until_parked();
    cx.run_until_parked();
    let _ = cx.refresh();
    cx.update(|_, _| {});
    cx.run_until_parked();

    // 移开鼠标再移回剩余未暂存文件的复选框；顶部统计行占一行。
    cx.simulate_mouse_move(
        point(px(100.), px(f32::from(row_height) * 7.5)),
        None,
        Modifiers::default(),
    );
    cx.run_until_parked();
    cx.simulate_mouse_move(
        point(px(1907.), px(f32::from(row_height) * 7.5)),
        None,
        Modifiers::default(),
    );
    cx.executor().advance_clock(Duration::from_millis(600));
    cx.run_until_parked();
    let _ = cx.refresh();
    cx.update(|_, _| {});
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("tooltip-view").is_some(),
        "行重建后悬停复选框应仍显示 tooltip"
    );
}

#[gpui::test]
fn hover_tooltip_with_partially_staged_file(cx: &mut TestAppContext) {
    init_test_context(cx);
    // 部分暂存（MM）：文件同时出现在已暂存与未暂存两组，两个复选框 id 相同
    // （按 path 生成）——验证两组的 tooltip 是否互相干扰。
    let (root, _temp) = test_repo();
    fs::write(root.join("tracked.txt"), "第一次修改\n").expect("应修改文件");
    run_in(&root, &["git", "add", "tracked.txt"]);
    fs::write(root.join("tracked.txt"), "第二次修改\n").expect("应再次修改文件");

    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (_panel, cx) =
        cx.add_window_view(move |window, cx| VersionControlPanel::new(project, window, cx));
    cx.run_until_parked();
    let _ = cx.refresh();
    cx.update(|_, _| {});
    cx.run_until_parked();

    // 行布局：两个分组标题 + 已暂存组 tracked.txt + 未暂存组 tracked.txt；
    // 顶部统计行占一行，坐标加偏移。
    let row_height = cx.update(|window, cx| tree_row_height(window, cx));
    // 先悬停未暂存组的复选框（第 5 行）。
    cx.simulate_mouse_move(
        point(px(1907.), px(f32::from(row_height) * 7.5)),
        None,
        Modifiers::default(),
    );
    cx.executor().advance_clock(Duration::from_millis(600));
    cx.run_until_parked();
    let _ = cx.refresh();
    cx.update(|_, _| {});
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("tooltip-view").is_some(),
        "部分暂存场景悬停未暂存组复选框应显示 tooltip"
    );
}

#[gpui::test]
fn hovering_checked_checkbox_shows_tooltip(cx: &mut TestAppContext) {
    init_test_context(cx);
    let (root, _temp) = test_repo();
    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) =
        cx.add_window_view(move |window, cx| VersionControlPanel::new(project, window, cx));
    cx.run_until_parked();
    let _ = cx.refresh();
    cx.update(|_, _| {});
    cx.run_until_parked();

    // 先暂存文件（空格），行移到已暂存组（带对勾）。
    cx.update(|window, cx| {
        let focus = panel.read(cx).focus.clone();
        window.focus(&focus, cx);
    });
    cx.simulate_keystrokes("down");
    cx.simulate_keystrokes("space");
    cx.run_until_parked();
    cx.run_until_parked();
    let _ = cx.refresh();
    cx.update(|_, _| {});
    cx.run_until_parked();

    // 已暂存组复选框（带对勾）。
    assert_hover_tooltip(cx, 4);
}

#[gpui::test]
fn checkbox_click_stages_file_without_opening_it(cx: &mut TestAppContext) {
    init_test_context(cx);
    let (root, _temp) = test_repo();
    let project_root = root.clone();
    let open_count = Rc::new(Cell::new(0));
    let callback_count = Rc::clone(&open_count);
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) = cx.add_window_view(move |window, cx| {
        let mut panel = VersionControlPanel::new(project, window, cx);
        panel.set_on_open_file(Rc::new(move |_, _, _, _, _| {
            callback_count.set(callback_count.get() + 1);
        }));
        panel
    });
    cx.run_until_parked();
    let _ = cx.refresh();
    cx.update(|_, _| {});
    cx.run_until_parked();

    // tracked.txt（未暂存组）行尾复选框：窗口 1920 宽，右边缘 6px + 复选框半宽 7px；
    // 顶部统计行占一行，坐标加偏移。
    let row_height = cx.update(|window, cx| tree_row_height(window, cx));
    cx.simulate_click(
        point(px(1907.), px(f32::from(row_height) * 7.5)),
        Modifiers::default(),
    );
    cx.run_until_parked();
    cx.run_until_parked();

    let sections = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert_eq!(
        sections,
        vec![(GitSection::Staged, "tracked.txt".into())],
        "点击复选框应暂存文件"
    );
    assert_eq!(
        open_count.get(),
        0,
        "点击复选框不应触发行的打开逻辑（stop_propagation）"
    );
}

#[gpui::test]
fn commit_button_commits_without_panel_focus(cx: &mut TestAppContext) {
    init_test_context(cx);
    // 回归：焦点不在版本控制面板时，点击"提交"按钮也应生效。
    // dispatch_action 从当前焦点沿焦点链派发，面板 handler 收不到动作，按钮必须直接调用面板方法而不是依赖焦点链。
    let (root, _temp) = test_repo();
    run_in(&root, &["git", "add", "tracked.txt"]);
    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) =
        cx.add_window_view(move |window, cx| VersionControlPanel::new(project, window, cx));
    cx.run_until_parked(); // 首次扫描完成（tracked.txt 已暂存）。
    cx.run_until_parked();

    let panel_focused = cx.update(|window, cx| panel.read(cx).focus.contains_focused(window, cx));
    assert!(!panel_focused, "前置条件：面板不应持有焦点");

    cx.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            panel
                .test_commit_editor()
                .update(cx, |editor, cx| editor.set_text("按钮提交", cx));
        });
    });
    let _ = cx.refresh();
    cx.update(|_, _| {});
    cx.run_until_parked();

    let bounds = cx
        .debug_bounds("version-control-commit-button")
        .expect("提交按钮应可定位");
    cx.simulate_click(bounds.center(), Modifiers::default());
    cx.run_until_parked(); // commit job 完成
    cx.run_until_parked(); // 重扫落地 → Head/Statuses 事件

    cx.update(|_, cx| {
        assert_eq!(
            panel.read(cx).test_last_commit_message(cx).as_deref(),
            Some("按钮提交"),
            "焦点不在面板时点击提交按钮也应提交"
        );
        let text = panel.read(cx).test_commit_editor().read(cx).text(cx);
        assert!(text.is_empty(), "提交成功后编辑器应清空，实际：{text:?}");
    });
}

#[gpui::test]
fn commit_flow_clears_editor_and_refreshes_last_commit(cx: &mut TestAppContext) {
    init_test_context(cx);
    let (root, _temp) = test_repo();
    run_in(&root, &["git", "add", "tracked.txt"]);
    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) =
        cx.add_window_view(move |window, cx| VersionControlPanel::new(project, window, cx));
    cx.run_until_parked(); // 首次扫描完成（tracked.txt 已暂存）。
    cx.run_until_parked();

    // 底部提交区显示初始提交的 subject。
    let initial = cx.read_entity(&panel, |panel, cx| panel.test_last_commit_message(cx));
    assert_eq!(
        initial.as_deref(),
        Some("initial"),
        "应显示初始提交 subject"
    );

    // 写提交消息并提交（等价于 cmd-enter / 提交按钮的 handler 行为）。
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel
                .test_commit_editor()
                .update(cx, |editor, cx| editor.set_text("改动提交", cx));
            panel.commit_repository(&panel.test_workdir(), window, cx);
        });
    });
    cx.run_until_parked(); // commit job 完成
    cx.run_until_parked(); // 重扫落地 → Head/Statuses 事件

    // 编辑器清空、上次提交信息更新、工作树干净。
    cx.update(|_, cx| {
        let text = panel.read(cx).test_commit_editor().read(cx).text(cx);
        assert!(text.is_empty(), "提交成功后编辑器应清空，实际：{text:?}");
        assert_eq!(
            panel.read(cx).test_last_commit_message(cx).as_deref(),
            Some("改动提交"),
            "上次提交信息应刷新为新提交 subject"
        );
        let statuses = panel
            .read(cx)
            .project
            .read(cx)
            .git_store()
            .read(cx)
            .repositories()
            .next()
            .map(|(_, snapshot)| snapshot.statuses_by_path.len());
        assert_eq!(statuses, Some(0), "已暂存改动应随提交清空");
    });
}

#[gpui::test]
fn commit_shortcut_submits_staged_changes_from_editor(cx: &mut TestAppContext) {
    init_test_context(cx);
    let (root, _temp) = test_repo();
    run_in(&root, &["git", "add", "tracked.txt"]);
    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) = cx.add_window_view(move |window, cx| {
        init(cx).expect("应注册内置快捷键");
        VersionControlPanel::new(project, window, cx)
    });
    cx.run_until_parked();
    cx.run_until_parked();

    let editor_focus = cx.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            panel
                .test_commit_editor()
                .update(cx, |editor, cx| editor.set_text("快捷键提交", cx));
            panel.test_commit_editor().read(cx).focus_handle()
        })
    });
    cx.update(|window, cx| window.focus(&editor_focus, cx));

    #[cfg(target_os = "macos")]
    cx.simulate_keystrokes("cmd-enter");
    #[cfg(not(target_os = "macos"))]
    cx.simulate_keystrokes("ctrl-enter");
    cx.run_until_parked();
    cx.run_until_parked();

    cx.update(|_, cx| {
        assert_eq!(
            panel.read(cx).test_last_commit_message(cx).as_deref(),
            Some("快捷键提交"),
            "提交信息框聚焦时，提交快捷键应提交当前暂存"
        );
    });
}

#[gpui::test]
fn commit_without_staged_changes_is_ignored(cx: &mut TestAppContext) {
    init_test_context(cx);
    let (root, _temp) = test_repo();
    fs::write(root.join("untracked.txt"), "新文件\n").expect("应创建未跟踪文件");

    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) =
        cx.add_window_view(move |window, cx| VersionControlPanel::new(project, window, cx));
    cx.run_until_parked();
    cx.run_until_parked();

    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel
                .test_commit_editor()
                .update(cx, |editor, cx| editor.set_text("改动提交", cx));
            panel.commit_repository(&panel.test_workdir(), window, cx);
        });
    });
    cx.run_until_parked();
    cx.run_until_parked();

    cx.update(|_, cx| {
        let panel = panel.read(cx);
        assert!(
            panel
                .commit_states
                .values()
                .all(|state| state.pending_message.is_none()),
            "无已暂存改动时不应发起提交"
        );
        assert_eq!(
            panel.test_commit_editor().read(cx).text(cx),
            "改动提交",
            "未发起提交时应保留提交信息"
        );
        assert_eq!(
            panel.test_last_commit_message(cx).as_deref(),
            Some("initial"),
            "无已暂存改动时 HEAD 不应变化"
        );
        let store = panel.project.read(cx).git_store();
        let store = store.read(cx);
        let snapshot = store
            .repositories()
            .next()
            .map(|(_, snapshot)| snapshot)
            .expect("应存在仓库快照");
        assert!(
            snapshot
                .statuses_by_path
                .get(&relative("tracked.txt"))
                .is_some_and(|entry| entry.status.has_unstaged()),
            "已跟踪改动应保持未暂存"
        );
        assert!(
            snapshot
                .statuses_by_path
                .get(&relative("untracked.txt"))
                .is_some_and(|entry| matches!(entry.status, zcv_git::FileStatus::Untracked)),
            "未跟踪文件应保持未暂存"
        );
    });
}

#[gpui::test]
fn uncommit_restores_previous_message_into_editor(cx: &mut TestAppContext) {
    init_test_context(cx);
    let (root, _temp) = test_repo();
    // 第二次提交：多行消息（subject + 空行 + body），随后制造未暂存改动。
    fs::write(root.join("tracked.txt"), "第二次内容\n").expect("应修改文件");
    run_in(&root, &["git", "add", "tracked.txt"]);
    run_in(
        &root,
        &["git", "commit", "-q", "-m", "第二次提交", "-m", "详细说明"],
    );
    fs::write(root.join("tracked.txt"), "第三次内容\n").expect("应再次修改文件");

    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) =
        cx.add_window_view(move |window, cx| VersionControlPanel::new(project, window, cx));
    cx.run_until_parked();
    cx.run_until_parked();

    // 触发 uncommit（等价于"撤销"按钮的 handler 行为）。
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.handle_uncommit(&Uncommit, window, cx);
        });
    });
    cx.run_until_parked(); // uncommit job 完成
    cx.run_until_parked(); // 重扫落地 → Head 事件

    // 被撤销提交的完整消息（含 body）填回编辑器；上次提交信息回到 initial。
    cx.update(|_, cx| {
        let text = panel.read(cx).test_commit_editor().read(cx).text(cx);
        assert_eq!(
            text, "第二次提交\n\n详细说明",
            "uncommit 后应把被撤销提交的完整消息填回编辑器"
        );
        assert_eq!(
            panel.read(cx).test_last_commit_message(cx).as_deref(),
            Some("initial"),
            "上次提交信息应回到被撤销提交之前的提交"
        );
    });
}

#[gpui::test]
fn uncommit_button_restores_message_without_panel_focus(cx: &mut TestAppContext) {
    init_test_context(cx);
    // 回归：焦点不在版本控制面板时，点击"撤销"按钮也应生效（与 commit 按钮同理：不依赖焦点链派发）。
    let (root, _temp) = test_repo();
    fs::write(root.join("tracked.txt"), "第二次内容\n").expect("应修改文件");
    run_in(&root, &["git", "add", "tracked.txt"]);
    run_in(
        &root,
        &["git", "commit", "-q", "-m", "第二次提交", "-m", "详细说明"],
    );
    fs::write(root.join("tracked.txt"), "第三次内容\n").expect("应再次修改文件");

    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) =
        cx.add_window_view(move |window, cx| VersionControlPanel::new(project, window, cx));
    cx.run_until_parked();
    cx.run_until_parked();

    let panel_focused = cx.update(|window, cx| panel.read(cx).focus.contains_focused(window, cx));
    assert!(!panel_focused, "前置条件：面板不应持有焦点");

    let _ = cx.refresh();
    cx.update(|_, _| {});
    cx.run_until_parked();

    let bounds = cx
        .debug_bounds("version-control-uncommit-button")
        .expect("撤销按钮应可定位");
    cx.simulate_click(bounds.center(), Modifiers::default());
    cx.run_until_parked(); // uncommit job 完成
    cx.run_until_parked(); // 重扫落地 → Head 事件

    cx.update(|_, cx| {
        let text = panel.read(cx).test_commit_editor().read(cx).text(cx);
        assert_eq!(
            text, "第二次提交\n\n详细说明",
            "焦点不在面板时点击撤销按钮也应撤销提交并填回消息"
        );
        assert_eq!(
            panel.read(cx).test_last_commit_message(cx).as_deref(),
            Some("initial"),
            "撤销后上次提交信息应回到 initial"
        );
    });
}
