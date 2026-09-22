use super::*;
use std::cell::Cell;
use std::sync::Arc;

use gpui::{KeyBinding, TestAppContext, VisualTestContext, point, px};
use tempfile::TempDir;

use zcv_fs_watch::{FsEventStream, FsWatcher, Watcher};
use zcv_git::StatusCode;
use zcv_keymap::init;
use zcv_language::LanguageRegistry;
use zcv_project::{Project, StatusEntry};
use zcv_ui::tree_row_height;

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
                    RelativePathBuf::from_path(std::path::Path::new(path))
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
    RelativePathBuf::from_path(std::path::Path::new(path)).expect("测试路径应为有效的仓库相对路径")
}

fn build_rows(root: &Path, repos: &[(&Path, &RepositorySnapshot)]) -> Vec<GitRow> {
    let trees = build_section_trees(root, repos.iter().copied());
    flatten_rows(&trees, &HashSet::new(), &HashSet::new())
}

/// 行列表 → (分组, 显示名) 序列。
fn entry_keys(rows: &[GitRow]) -> Vec<(GitSection, String)> {
    rows.iter()
        .filter_map(|row| match row {
            GitRow::Entry(entry) => Some((entry.section, entry.name.clone())),
            GitRow::Header(_) | GitRow::Empty(_) => None,
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
            GitRow::Header(section) => Some(section.label()),
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
    let trees = GitSections::<Vec<GitTreeNode>>::default();
    let rows = flatten_rows(&trees, &HashSet::new(), &HashSet::new());

    assert_eq!(GitSection::Staged.empty_message(), "没有已暂存的更改");
    assert_eq!(GitSection::Unstaged.empty_message(), "没有未暂存的更改");
    assert_eq!(rows.len(), 6);
    assert!(matches!(rows[0], GitRow::Header(GitSection::Conflict)));
    assert!(matches!(rows[1], GitRow::Empty(GitSection::Conflict)));
    assert!(matches!(rows[2], GitRow::Header(GitSection::Staged)));
    assert!(matches!(rows[3], GitRow::Empty(GitSection::Staged)));
    assert!(matches!(rows[4], GitRow::Header(GitSection::Unstaged)));
    assert!(matches!(rows[5], GitRow::Empty(GitSection::Unstaged)));
    assert!(rows.iter().all(|row| row_entry_key(row).is_none()));

    let collapsed = HashSet::from([GitSection::Staged]);
    let rows = flatten_rows(&trees, &HashSet::new(), &collapsed);
    assert_eq!(rows.len(), 5);
    assert!(matches!(rows[0], GitRow::Header(GitSection::Conflict)));
    assert!(matches!(rows[1], GitRow::Empty(GitSection::Conflict)));
    assert!(matches!(rows[2], GitRow::Header(GitSection::Staged)));
    assert!(matches!(rows[3], GitRow::Header(GitSection::Unstaged)));
    assert!(matches!(rows[4], GitRow::Empty(GitSection::Unstaged)));
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
    let trees = build_section_trees(&root, [(root.as_path(), &snapshot)].into_iter());

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
fn consecutive_single_change_directories_are_display_compressed() {
    let root = absolute(PathBuf::from("/project")).into_path_buf();
    let snapshot = snapshot(&[("src/components/editor/mod.rs", FileStatus::Untracked)]);
    let trees = build_section_trees(&root, [(root.as_path(), &snapshot)].into_iter());
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
            GitRow::Header(_) | GitRow::Empty(_) => None,
        })
        .collect();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].0, "src/components/editor");
    assert_eq!(entries[0].1, root.join("src/components/editor"));
    assert_eq!(entries[1].0, "mod.rs");
    assert_eq!(entries[1].2, 1);
}

#[test]
fn nested_repositories_merge_into_one_tree() {
    let root = absolute(PathBuf::from("/project")).into_path_buf();
    let vendor = root.join("vendor");
    let outer = snapshot(&[("README.md", FileStatus::Untracked)]);
    let inner = snapshot(&[("lib.rs", FileStatus::Untracked)]);
    let repos = [(root.as_path(), &outer), (vendor.as_path(), &inner)];
    let trees = build_section_trees(&root, repos.into_iter());

    // vendor 为目录行，优先于根文件 README.md；展开后 lib.rs 归入其下。
    let mut expanded = HashSet::new();
    expanded.insert((GitSection::Unstaged, absolute(root.join("vendor"))));
    let rows = flatten_rows(&trees, &expanded, &HashSet::new());
    let paths: Vec<_> = rows
        .iter()
        .filter_map(|row| match row {
            GitRow::Entry(entry) => Some((entry.path.clone(), entry.depth)),
            GitRow::Header(_) | GitRow::Empty(_) => None,
        })
        .collect();
    assert_eq!(
        paths,
        vec![
            (absolute(root.join("vendor")), 0),
            (absolute(root.join("vendor/lib.rs")), 1),
            (absolute(root.join("README.md")), 0)
        ]
    );
}

/// 创建带一个修改文件的临时 git 仓库。
fn test_repo() -> (PathBuf, TempDir) {
    let temp_dir = tempfile::tempdir().expect("应创建临时目录");
    let root = temp_dir.path().to_path_buf();
    run_in(&root, &["git", "init", "-q", "-b", "master"]);
    run_in(&root, &["git", "config", "user.email", "test@example.com"]);
    run_in(&root, &["git", "config", "user.name", "Test User"]);
    std::fs::write(root.join("tracked.txt"), "第一行\n第二行\n").expect("应写入初始文件");
    run_in(&root, &["git", "add", "tracked.txt"]);
    run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
    std::fs::write(root.join("tracked.txt"), "修改后的内容\n").expect("应修改文件");
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

/// UI 行为测试不需要验证 OS 文件监听；使用被动后端避免外部线程向测试调度器注入事件。
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

#[gpui::test]
fn empty_state_initializes_repository_and_builds_section_tree(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let project_root = directory.path().to_path_buf();
    let project = test_project(project_root.clone(), cx);
    let project_for_panel = project.clone();
    let (panel, cx) =
        cx.add_window_view(move |_, cx| VersionControlPanel::new(project_for_panel, cx));
    cx.run_until_parked(); // 首次扫描完成：无仓库，行模型为空。

    let panel_focused = cx.update(|window, cx| panel.read(cx).focus.contains_focused(window, cx));
    assert!(!panel_focused, "前置条件：面板不应持有焦点");

    let _ = cx.refresh();
    cx.update(|_, _| {});
    let bounds = cx
        .debug_bounds("version-control-init")
        .expect("初始化仓库按钮应可定位");
    cx.simulate_click(bounds.center(), gpui::Modifiers::default());
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
        let rows = panel.state.borrow().rows.clone();
        let headers: Vec<_> = rows
            .iter()
            .filter_map(|row| match row {
                GitRow::Header(section) => Some(section.label()),
                GitRow::Empty(_) | GitRow::Entry(_) => None,
            })
            .collect();
        assert_eq!(headers, vec!["冲突", "已暂存", "未暂存"]);
    });
}

#[gpui::test]
fn first_click_on_unfocused_panel_focuses_and_opens(cx: &mut TestAppContext) {
    let (root, _temp) = test_repo();
    let project_root = root.clone();
    let open_count = Rc::new(Cell::new(0));
    let last_focus_opened = Rc::new(Cell::new(true));
    let opened_path = Rc::new(RefCell::new(None));
    let callback_count = Rc::clone(&open_count);
    let callback_focus = Rc::clone(&last_focus_opened);
    let callback_path = Rc::clone(&opened_path);

    let project = test_project(project_root.clone(), cx);
    let (panel, cx) = cx.add_window_view(move |_, cx| {
        let mut panel = VersionControlPanel::new(project, cx);
        panel.set_on_open_file(Rc::new(move |_, path, focus_opened_item, _, _| {
            callback_count.set(callback_count.get() + 1);
            callback_focus.set(focus_opened_item);
            *callback_path.borrow_mut() = Some(path);
        }));
        panel
    });
    cx.run_until_parked(); // 扫描完成，行模型就绪。

    // 行布局：三组标题/空提示，加上未暂存组中的一个文件行。
    let row_count = cx.read_entity(&panel, |panel, _| panel.state.borrow().rows.len());
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
            point(px(100.), px(f32::from(row_height) * 6.5)),
            gpui::Modifiers::default(),
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
    let (root, _temp) = test_repo();
    let project_root = root.clone();
    let open_count = Rc::new(Cell::new(0));
    let last_focus_opened = Rc::new(Cell::new(false));
    let callback_count = Rc::clone(&open_count);
    let callback_focus = Rc::clone(&last_focus_opened);

    let project = test_project(project_root.clone(), cx);
    let (panel, cx) = cx.add_window_view(move |_, cx| {
        cx.bind_keys([
            KeyBinding::new("down", SelectNext, Some("GitPanel && ChangesList")),
            KeyBinding::new("enter", Activate, Some("GitPanel && ChangesList")),
        ]);
        let mut panel = VersionControlPanel::new(project, cx);
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
    let (root, _temp) = test_repo();
    run_in(&root, &["git", "add", "tracked.txt"]);
    let opened_kind = Rc::new(Cell::new(None));
    let callback_kind = Rc::clone(&opened_kind);
    let project_root = root.clone();
    let project = test_project(project_root, cx);
    let (panel, cx) = cx.add_window_view(move |_, cx| {
        let mut panel = VersionControlPanel::new(project, cx);
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
        .rows
        .iter()
        .filter_map(|row| match row {
            GitRow::Entry(entry) => Some((entry.section, entry.name.clone())),
            GitRow::Header(_) | GitRow::Empty(_) => None,
        })
        .collect()
}

#[gpui::test]
fn directories_expand_by_default(cx: &mut TestAppContext) {
    // 目录下有变更文件：首次构建后目录默认展开，子文件可见。
    let (root, _temp) = test_repo();
    std::fs::create_dir_all(root.join("src")).expect("应创建目录");
    std::fs::write(root.join("src/a.txt"), "改动\n").expect("应写入文件");

    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
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
    let (root, _temp) = test_repo();
    std::fs::create_dir_all(root.join("src")).expect("应创建目录");
    std::fs::write(root.join("src/a.txt"), "改动\n").expect("应写入文件");

    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
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
        panel.collapsed_dirs.insert(key.clone());
        panel.state.borrow_mut().expanded.remove(&key);
        panel.rebuild_rows(cx);
    });
    let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert!(
        !entries.contains(&(GitSection::Unstaged, "a.txt".into())),
        "用户折叠的目录应保持折叠"
    );

    // 新目录 src2 出现：增量刷新 GitStore（fs watch 是真实线程，测试里走公开刷新入口）→ Statuses 事件 → 面板重建。
    std::fs::create_dir_all(root.join("src2")).expect("应创建目录");
    std::fs::write(root.join("src2/b.txt"), "改动\n").expect("应写入文件");
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
fn section_header_collapse_hides_entries_and_expand_restores(cx: &mut TestAppContext) {
    let (root, _temp) = test_repo();
    std::fs::write(root.join("tracked.txt"), "改动\n").expect("应写入文件");

    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
    cx.run_until_parked(); // 扫描 + 重建

    // 初始：未暂存组条目可见。
    let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert!(entries.contains(&(GitSection::Unstaged, "tracked.txt".into())));

    // 折叠未暂存分区：条目不渲染，标题行保留。
    cx.update_entity(&panel, |panel, cx| {
        panel.toggle_section_collapsed(GitSection::Unstaged, cx);
    });
    let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert!(entries.is_empty(), "折叠后该分区条目应隐藏（仅剩标题行）");

    // 再展开：条目恢复。
    cx.update_entity(&panel, |panel, cx| {
        panel.toggle_section_collapsed(GitSection::Unstaged, cx);
    });
    let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert!(entries.contains(&(GitSection::Unstaged, "tracked.txt".into())));
}

#[gpui::test]
fn collapsed_section_keeps_header_checkbox_and_select_all(cx: &mut TestAppContext) {
    // 分组折叠只隐藏条目行；只要该组仍有文件，标题行全选复选框就必须常驻且可整组暂存。
    let (root, _temp) = test_repo();
    std::fs::write(root.join("tracked.txt"), "改动\n").expect("应写入文件");

    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
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
    // 两个未暂存文件：header 全选 → 全部暂存，未暂存组清空、已暂存组出现两项。
    let (root, _temp) = test_repo();
    std::fs::write(root.join("tracked.txt"), "改动\n").expect("应写入文件");
    std::fs::write(root.join("second.txt"), "第二个文件\n").expect("应写入文件");

    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
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
    let (root, _temp) = test_repo();
    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) = cx.add_window_view(move |_, cx| {
        cx.bind_keys([
            KeyBinding::new("down", SelectNext, Some("GitPanel && ChangesList")),
            KeyBinding::new("space", ToggleStaged, Some("GitPanel && ChangesList")),
        ]);
        VersionControlPanel::new(project, cx)
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
    let (root, _temp) = test_repo();
    let project_root = root.clone();
    let project = test_project(project_root, cx);
    let (panel, cx) = cx.add_window_view(move |_, cx| {
        init(cx).expect("应注册内置快捷键");
        VersionControlPanel::new(project, cx)
    });
    cx.run_until_parked();
    cx.run_until_parked();

    let editor_focus = cx.read_entity(&panel, |panel, cx| {
        panel.commit_editor.read(cx).focus_handle()
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
            panel.read(cx).commit_editor.read(cx).text(cx),
            " ",
            "提交信息编辑器聚焦时，空格应输入文本"
        );
    });
    let after = cx.read_entity(&panel, |panel, _| section_entries(panel));
    assert_eq!(after, before, "提交信息中的空格不应切换暂存状态");
}

#[gpui::test]
fn selection_border_only_shows_when_changes_tree_is_focused(cx: &mut TestAppContext) {
    let (root, _temp) = test_repo();
    let project = test_project(root, cx);
    let (panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
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
        panel.commit_editor.read(cx).focus_handle()
    });
    cx.update(|window, cx| window.focus(&commit_editor_focus, cx));
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("version-control-selection-border")
            .is_none(),
        "提交信息编辑器聚焦时不应显示变更树选中框"
    );
    assert!(
        cx.read_entity(&panel, |panel, _| panel.state.borrow().selected.is_some()),
        "切换焦点只隐藏选中框，不应清除变更树选中状态"
    );
}

/// 悬停指定行尾的复选框并断言 tooltip 气泡出现。
///
/// 测试时钟不会自动推进：手动拨过 500ms tooltip 显示延迟后再渲染一帧。
/// 顶部统计行占一行高度，行坐标加 1 行偏移。
fn assert_hover_tooltip(cx: &mut gpui::VisualTestContext, row_index: usize) {
    let row_height = cx.update(|window, cx| tree_row_height(window, cx));
    cx.simulate_mouse_move(
        point(
            px(1907.),
            px(f32::from(row_height) * (row_index as f32 + 1.5)),
        ),
        None,
        gpui::Modifiers::default(),
    );
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(600));
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
    let (root, _temp) = test_repo();
    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (_panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
    cx.run_until_parked();
    let _ = cx.refresh();
    cx.update(|_, _| {});
    cx.run_until_parked();

    // 未暂存组 tracked.txt 行尾复选框；空的已暂存组占一行提示。
    assert_hover_tooltip(cx, 5);
}

#[gpui::test]
fn hovering_checkbox_shows_tooltip_with_many_rows(cx: &mut TestAppContext) {
    // 复现真实场景：大量变更文件（行数超过可视区域，触发 uniform_list 虚拟化）。
    let (root, _temp) = test_repo();
    for index in 0..40 {
        std::fs::write(
            root.join(format!("file-{index:02}.txt")),
            format!("内容 {index}\n"),
        )
        .expect("应写入文件");
    }
    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (_panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
    cx.run_until_parked();
    let _ = cx.refresh();
    cx.update(|_, _| {});
    cx.run_until_parked();

    // 悬停可视区第 5 行（未暂存组的一个文件）行尾复选框；顶部统计行占一行，坐标加偏移。
    let row_height = cx.update(|window, cx| tree_row_height(window, cx));
    let hover_y = f32::from(row_height) * 7.5;
    cx.simulate_mouse_move(
        point(px(1907.), px(hover_y)),
        None,
        gpui::Modifiers::default(),
    );
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(600));
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
    // 复现真实场景：行重建（git 操作/滚动后 rebuild_rows）后再次 hover 复选框，
    // tooltip 应仍然显示。
    let (root, _temp) = test_repo();
    std::fs::write(root.join("second.txt"), "第二个文件\n").expect("应写入文件");
    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
    cx.run_until_parked();
    let _ = cx.refresh();
    cx.update(|_, _| {});
    cx.run_until_parked();

    // 先悬停未暂存组的一个文件复选框，确认 tooltip 正常；顶部统计行占一行，坐标加偏移。
    let row_height = cx.update(|window, cx| tree_row_height(window, cx));
    cx.simulate_mouse_move(
        point(px(1907.), px(f32::from(row_height) * 6.5)),
        None,
        gpui::Modifiers::default(),
    );
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(600));
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
        point(px(100.), px(f32::from(row_height) * 6.5)),
        None,
        gpui::Modifiers::default(),
    );
    cx.run_until_parked();
    cx.simulate_mouse_move(
        point(px(1907.), px(f32::from(row_height) * 6.5)),
        None,
        gpui::Modifiers::default(),
    );
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(600));
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
    // 部分暂存（MM）：文件同时出现在已暂存与未暂存两组，两个复选框 id 相同
    // （按 path 生成）——验证两组的 tooltip 是否互相干扰。
    let (root, _temp) = test_repo();
    std::fs::write(root.join("tracked.txt"), "第一次修改\n").expect("应修改文件");
    run_in(&root, &["git", "add", "tracked.txt"]);
    std::fs::write(root.join("tracked.txt"), "第二次修改\n").expect("应再次修改文件");

    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (_panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
    cx.run_until_parked();
    let _ = cx.refresh();
    cx.update(|_, _| {});
    cx.run_until_parked();

    // 行布局：两个分组标题 + 已暂存组 tracked.txt + 未暂存组 tracked.txt；
    // 顶部统计行占一行，坐标加偏移。
    let row_height = cx.update(|window, cx| tree_row_height(window, cx));
    // 先悬停未暂存组的复选框（第 5 行）。
    cx.simulate_mouse_move(
        point(px(1907.), px(f32::from(row_height) * 6.5)),
        None,
        gpui::Modifiers::default(),
    );
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(600));
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
    let (root, _temp) = test_repo();
    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
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
    let (root, _temp) = test_repo();
    let project_root = root.clone();
    let open_count = Rc::new(Cell::new(0));
    let callback_count = Rc::clone(&open_count);
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) = cx.add_window_view(move |_, cx| {
        let mut panel = VersionControlPanel::new(project, cx);
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
        point(px(1907.), px(f32::from(row_height) * 6.5)),
        gpui::Modifiers::default(),
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
    // 回归：焦点不在版本控制面板时，点击"提交"按钮也应生效。
    // dispatch_action 从当前焦点沿焦点链派发，面板 handler 收不到动作，按钮必须直接调用面板方法而不是依赖焦点链。
    let (root, _temp) = test_repo();
    run_in(&root, &["git", "add", "tracked.txt"]);
    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
    cx.run_until_parked(); // 首次扫描完成（tracked.txt 已暂存）。
    cx.run_until_parked();

    let panel_focused = cx.update(|window, cx| panel.read(cx).focus.contains_focused(window, cx));
    assert!(!panel_focused, "前置条件：面板不应持有焦点");

    cx.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            panel
                .commit_editor
                .update(cx, |editor, cx| editor.set_text("按钮提交", cx));
        });
    });
    let _ = cx.refresh();
    cx.update(|_, _| {});
    cx.run_until_parked();

    let bounds = cx
        .debug_bounds("version-control-commit-button")
        .expect("提交按钮应可定位");
    cx.simulate_click(bounds.center(), gpui::Modifiers::default());
    cx.run_until_parked(); // commit job 完成
    cx.run_until_parked(); // 重扫落地 → Head/Statuses 事件

    cx.update(|_, cx| {
        assert_eq!(
            panel.read(cx).last_commit_message.as_deref(),
            Some("按钮提交"),
            "焦点不在面板时点击提交按钮也应提交"
        );
        let text = panel.read(cx).commit_editor.read(cx).text(cx);
        assert!(text.is_empty(), "提交成功后编辑器应清空，实际：{text:?}");
    });
}

#[gpui::test]
fn commit_flow_clears_editor_and_refreshes_last_commit(cx: &mut TestAppContext) {
    let (root, _temp) = test_repo();
    run_in(&root, &["git", "add", "tracked.txt"]);
    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
    cx.run_until_parked(); // 首次扫描完成（tracked.txt 已暂存）。
    cx.run_until_parked();

    // 底部提交区显示初始提交的 subject。
    let initial = cx.read_entity(&panel, |panel, _| panel.last_commit_message.clone());
    assert_eq!(
        initial.as_deref(),
        Some("initial"),
        "应显示初始提交 subject"
    );

    // 写提交消息并提交（等价于 cmd-enter / 提交按钮的 handler 行为）。
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel
                .commit_editor
                .update(cx, |editor, cx| editor.set_text("改动提交", cx));
            panel.handle_commit(&Commit, window, cx);
        });
    });
    cx.run_until_parked(); // commit job 完成
    cx.run_until_parked(); // 重扫落地 → Head/Statuses 事件

    // 编辑器清空、上次提交信息更新、工作树干净。
    cx.update(|_, cx| {
        let text = panel.read(cx).commit_editor.read(cx).text(cx);
        assert!(text.is_empty(), "提交成功后编辑器应清空，实际：{text:?}");
        assert_eq!(
            panel.read(cx).last_commit_message.as_deref(),
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
    let (root, _temp) = test_repo();
    run_in(&root, &["git", "add", "tracked.txt"]);
    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) = cx.add_window_view(move |_, cx| {
        init(cx).expect("应注册内置快捷键");
        VersionControlPanel::new(project, cx)
    });
    cx.run_until_parked();
    cx.run_until_parked();

    let editor_focus = cx.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            panel
                .commit_editor
                .update(cx, |editor, cx| editor.set_text("快捷键提交", cx));
            panel.commit_editor.read(cx).focus_handle()
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
            panel.read(cx).last_commit_message.as_deref(),
            Some("快捷键提交"),
            "提交信息框聚焦时，提交快捷键应提交当前暂存"
        );
    });
}

#[gpui::test]
fn commit_without_staged_changes_is_ignored(cx: &mut TestAppContext) {
    let (root, _temp) = test_repo();
    std::fs::write(root.join("untracked.txt"), "新文件\n").expect("应创建未跟踪文件");

    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
    cx.run_until_parked();
    cx.run_until_parked();

    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel
                .commit_editor
                .update(cx, |editor, cx| editor.set_text("改动提交", cx));
            panel.handle_commit(&Commit, window, cx);
        });
    });
    cx.run_until_parked();
    cx.run_until_parked();

    cx.update(|_, cx| {
        let panel = panel.read(cx);
        assert!(!panel.pending_commit, "无已暂存改动时不应发起提交");
        assert_eq!(
            panel.commit_editor.read(cx).text(cx),
            "改动提交",
            "未发起提交时应保留提交信息"
        );
        assert_eq!(
            panel.last_commit_message.as_deref(),
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
                .is_some_and(|entry| entry.status.is_untracked()),
            "未跟踪文件应保持未暂存"
        );
    });
}

#[gpui::test]
fn uncommit_restores_previous_message_into_editor(cx: &mut TestAppContext) {
    let (root, _temp) = test_repo();
    // 第二次提交：多行消息（subject + 空行 + body），随后制造未暂存改动。
    std::fs::write(root.join("tracked.txt"), "第二次内容\n").expect("应修改文件");
    run_in(&root, &["git", "add", "tracked.txt"]);
    run_in(
        &root,
        &["git", "commit", "-q", "-m", "第二次提交", "-m", "详细说明"],
    );
    std::fs::write(root.join("tracked.txt"), "第三次内容\n").expect("应再次修改文件");

    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
    cx.run_until_parked();
    cx.run_until_parked();

    // 触发 uncommit（等价于"撤销"按钮的 handler 行为）。
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| panel.handle_uncommit(&Uncommit, window, cx));
    });
    cx.run_until_parked(); // uncommit job 完成
    cx.run_until_parked(); // 重扫落地 → Head 事件

    // 被撤销提交的完整消息（含 body）填回编辑器；上次提交信息回到 initial。
    cx.update(|_, cx| {
        let text = panel.read(cx).commit_editor.read(cx).text(cx);
        assert_eq!(
            text, "第二次提交\n\n详细说明",
            "uncommit 后应把被撤销提交的完整消息填回编辑器"
        );
        assert_eq!(
            panel.read(cx).last_commit_message.as_deref(),
            Some("initial"),
            "上次提交信息应回到被撤销提交之前的提交"
        );
    });
}

#[gpui::test]
fn uncommit_button_restores_message_without_panel_focus(cx: &mut TestAppContext) {
    // 回归：焦点不在版本控制面板时，点击"撤销"按钮也应生效（与 commit 按钮同理：不依赖焦点链派发）。
    let (root, _temp) = test_repo();
    std::fs::write(root.join("tracked.txt"), "第二次内容\n").expect("应修改文件");
    run_in(&root, &["git", "add", "tracked.txt"]);
    run_in(
        &root,
        &["git", "commit", "-q", "-m", "第二次提交", "-m", "详细说明"],
    );
    std::fs::write(root.join("tracked.txt"), "第三次内容\n").expect("应再次修改文件");

    let project_root = root.clone();
    let project = test_project(project_root.clone(), cx);
    let (panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
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
    cx.simulate_click(bounds.center(), gpui::Modifiers::default());
    cx.run_until_parked(); // uncommit job 完成
    cx.run_until_parked(); // 重扫落地 → Head 事件

    cx.update(|_, cx| {
        let text = panel.read(cx).commit_editor.read(cx).text(cx);
        assert_eq!(
            text, "第二次提交\n\n详细说明",
            "焦点不在面板时点击撤销按钮也应撤销提交并填回消息"
        );
        assert_eq!(
            panel.read(cx).last_commit_message.as_deref(),
            Some("initial"),
            "撤销后上次提交信息应回到 initial"
        );
    });
}
