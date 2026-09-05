use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use gpui::{Context, Window, div, prelude::*};
use zcv_actions::{
    TreeActivate, TreeCancelConflict, TreeClearClipboard, TreeConfirmEdit, TreeCopy, TreeCut,
    TreeNewEntry, TreePaste, TreeRename, TreeSelectNextExtend, TreeTrash,
};

use zcv_ui::ConfirmAnswer;

use super::editing::EditOperation;
use super::execute::HOVER_EXPAND_DELAY;
use super::transfer::TreeClipboard;

use std::cell::Cell;

use gpui::{
    AppContext, KeyBinding, Modifiers, MouseButton, Render, TestAppContext, VisualTestContext,
    point, px,
};

use super::*;

struct TestView;

impl Render for TestView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn focus_tree(tree: &gpui::Entity<ProjectTreePanel>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        let focus = tree.read(cx).focus.clone();
        window.focus(&focus, cx);
    });
}

#[test]
fn rows_are_cached_until_rebuild_reinjects_them() {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let file = directory.path().join("cached.txt");
    std::fs::write(&file, "content").expect("应创建测试文件");
    let mut state = TreeState::new(|row: &ProjectTreeRow| Some(row.path.clone()));
    let root = directory.path().to_path_buf();
    let rows = vec![
        ProjectTreeRow {
            path: root.clone(),
            name: root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            depth: 0,
            is_dir: true,
            expanded: true,
            is_new: false,
            git_status: None,
        },
        ProjectTreeRow {
            path: file.clone(),
            name: "cached.txt".to_string(),
            depth: 1,
            is_dir: false,
            expanded: false,
            is_new: false,
            git_status: None,
        },
    ];
    state.replace_rows(rows);

    // 渲染读取的是注入的缓存：文件系统变化不影响行模型。
    std::fs::remove_file(&file).expect("应删除测试文件");
    assert!(state.rows().iter().any(|row| row.path == file));

    // 只有显式重建（由 ProjectTreePanel 调 worktree 遍历）才会反映文件系统。
    state.replace_rows(vec![ProjectTreeRow {
        path: root,
        name: "root".to_string(),
        depth: 0,
        is_dir: true,
        expanded: true,
        is_new: false,
        git_status: None,
    }]);
    assert!(!state.rows().iter().any(|row| row.path == file));
}

#[gpui::test]
fn revealing_active_file_expands_ancestors_and_keeps_mark_separate_from_selection(
    cx: &mut TestAppContext,
) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let nested = directory.path().join("src").join("feature");
    std::fs::create_dir_all(&nested).expect("应创建嵌套目录");
    let file = nested.join("mod.rs");
    std::fs::write(&file, "content").expect("应创建测试文件");
    let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));

    tree.update(cx, |tree, cx| {
        tree.reveal_active_path(Some(file.clone()), cx)
    });
    // 行重建异步进行：跑完事件循环后行模型才包含目标文件。
    cx.run_until_parked();
    cx.read_entity(&tree, |tree, _| {
        assert_eq!(tree.active_path.as_deref(), Some(file.as_path()));
        assert_eq!(
            tree.state.borrow().selected.as_deref(),
            Some(file.as_path())
        );
        assert!(tree.state.borrow().rows.iter().any(|row| row.path == file));
        assert!(
            tree.state
                .borrow()
                .expanded
                .contains(&directory.path().join("src"))
        );
        assert!(tree.state.borrow().expanded.contains(&nested));

        // 键盘游标移动不应改变活动文件标记。
        tree.state.borrow_mut().select_up();
        assert_ne!(
            tree.state.borrow().selected.as_deref(),
            Some(file.as_path())
        );
        assert_eq!(tree.active_path.as_deref(), Some(file.as_path()));
    });
}

#[gpui::test]
fn revealing_path_outside_project_clears_active_mark(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let file = directory.path().join("active.txt");
    std::fs::write(&file, "content").expect("应创建测试文件");
    let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));

    tree.update(cx, |tree, cx| {
        tree.reveal_active_path(Some(file.clone()), cx)
    });
    tree.update(cx, |tree, cx| {
        tree.reveal_active_path(Some(PathBuf::from("/outside/project.txt")), cx);
    });
    cx.read_entity(&tree, |tree, _| {
        assert!(tree.active_path.is_none());
    });
}

#[gpui::test]
fn applying_directory_rename_migrates_tree_paths(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let old_directory = directory.path().join("old");
    let old_file = old_directory.join("mod.rs");
    std::fs::create_dir(&old_directory).expect("应创建待重命名目录");
    std::fs::write(&old_file, "content").expect("应创建测试文件");
    let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));

    // reveal 展开祖先并标记活动文件。
    tree.update(cx, |tree, cx| {
        tree.reveal_active_path(Some(old_file.clone()), cx)
    });

    let new_directory = directory.path().join("new");
    std::fs::rename(&old_directory, &new_directory).expect("应重命名测试目录");
    tree.update(cx, |tree, cx| {
        tree.apply_rename(&old_directory, &new_directory, cx)
    });
    // 行重建异步进行：跑完事件循环后新路径行才出现。
    cx.run_until_parked();

    let new_file = new_directory.join("mod.rs");
    cx.read_entity(&tree, |tree, _| {
        assert!(tree.state.borrow().expanded.contains(&new_directory));
        assert_eq!(
            tree.state.borrow().selected.as_deref(),
            Some(new_file.as_path())
        );
        assert_eq!(tree.active_path.as_deref(), Some(new_file.as_path()));
        assert!(
            tree.state
                .borrow()
                .rows
                .iter()
                .any(|row| row.path == new_file)
        );
    });
}

#[gpui::test]
fn space_edits_the_name_instead_of_activating_the_row_while_renaming(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let file = directory.path().join("old.txt");
    std::fs::write(&file, "content").expect("应创建测试文件");
    let project_root = directory.path().to_path_buf();
    let selected_file = file.clone();
    let open_count = Rc::new(Cell::new(0));
    let callback_count = Rc::clone(&open_count);

    let project = cx.new(|cx| Project::new(project_root.clone(), cx));
    let (tree, cx) = cx.add_window_view(move |_, cx| {
        cx.bind_keys([
            KeyBinding::new("enter", TreeRename, Some("ProjectTree && not_editing")),
            KeyBinding::new("space", TreeActivate, Some("ProjectTree && not_editing")),
        ]);
        let mut tree = ProjectTreePanel::new(project.clone(), cx);
        tree.set_on_open_file(Rc::new(move |_, _, _, _| {
            callback_count.set(callback_count.get() + 1);
        }));
        tree.state.borrow_mut().select(selected_file.clone());
        tree
    });
    focus_tree(&tree, cx);

    cx.simulate_keystrokes("enter");
    let entry_name_editor = cx.read_entity(&tree, |tree, _| {
        assert!(matches!(
            tree.edit_state.as_ref().map(|state| &state.operation),
            Some(EditOperation::Rename { source, .. }) if source == &file
        ));
        tree.entry_name_editor.clone()
    });
    cx.simulate_keystrokes("space");

    assert_eq!(open_count.get(), 0);
    cx.read_entity(&tree, |tree, _| assert!(tree.edit_state.is_some()));
    cx.read_entity(&entry_name_editor, |editor, cx| {
        assert_eq!(editor.text(cx), " .txt");
    });
}

#[gpui::test]
fn first_click_on_unfocused_tree_focuses_and_opens(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let file = directory.path().join("a.txt");
    std::fs::write(&file, "hello").expect("应创建测试文件");
    let project_root = directory.path().to_path_buf();

    // 记录每次打开回调的 focus_opened_item：单击临时打开应为 false，双击激活应为 true。
    let open_count = Rc::new(Cell::new(0));
    let last_focus_opened = Rc::new(Cell::new(true));
    let callback_count = Rc::clone(&open_count);
    let callback_focus = Rc::clone(&last_focus_opened);

    let project = cx.new(|cx| Project::new(project_root.clone(), cx));
    let (tree, cx) = cx.add_window_view(move |_, cx| {
        let mut tree = ProjectTreePanel::new(project.clone(), cx);
        tree.set_on_open_file(Rc::new(move |_, focus_opened_item, _, _| {
            callback_count.set(callback_count.get() + 1);
            callback_focus.set(focus_opened_item);
        }));
        // 展开根目录使文件行可见；不聚焦项目树（模拟焦点在别处）。
        let root = tree.root.clone().expect("测试项目应包含根目录");
        tree.state.borrow_mut().expanded.insert(root);
        tree.rebuild_rows(cx);
        tree
    });
    cx.run_until_parked();

    // 单击第二行（a.txt，depth 1）：行高为 ui_line()。
    let row_height = zcv_theme::typography::ui_line();
    let click = |cx: &mut VisualTestContext| {
        cx.simulate_click(
            point(px(10.), px(f32::from(row_height) + 1.)),
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
    };

    // 首击同时完成聚焦、选中与单击预览，用户无需先额外点击一次聚焦。
    click(cx);
    assert_eq!(open_count.get(), 1, "未聚焦首击也应打开文件");
    let tree_focused = cx.update(|window, cx| tree.read(cx).focus.contains_focused(window, cx));
    assert!(tree_focused, "首击应聚焦项目树");
    let selected = cx.read_entity(&tree, |tree, _| tree.state.borrow().selected.clone());
    assert_eq!(
        selected.as_deref(),
        Some(file.as_path()),
        "首击应选中被点行"
    );

    assert!(
        !last_focus_opened.get(),
        "单击文件应打开临时标签但焦点留在项目树（focus_opened_item=false）"
    );
}

#[gpui::test]
fn rename_actions_edit_and_confirm_the_selected_row(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let old_path = directory.path().join("old.txt");
    let new_path = directory.path().join("new.txt");
    std::fs::write(&old_path, "content").expect("应创建测试文件");
    let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
    tree.update(cx, |tree, _| {
        tree.set_on_rename(Rc::new(|from, to, _| {
            std::fs::rename(from, to)?;
            Ok(())
        }));
        tree.state.borrow_mut().select(old_path.clone());
    });
    // 面板初始行重建异步完成，选中键才能解析到行。
    cx.run_until_parked();

    cx.add_window_view(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.handle_tree_rename(&TreeRename, window, cx);
            tree.entry_name_editor
                .update(cx, |editor, cx| editor.set_text("new.txt", cx));
            tree.handle_tree_confirm_edit(&TreeConfirmEdit, window, cx);
        });
        TestView
    });

    assert!(!old_path.exists());
    assert!(new_path.exists());
    cx.read_entity(&tree, |tree, _| {
        assert!(tree.edit_state.is_none());
        assert_eq!(
            tree.state.borrow().selected.as_deref(),
            Some(new_path.as_path())
        );
    });
}

#[gpui::test]
fn one_create_action_infers_nested_files_and_directories_from_the_path(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let file = directory.path().join("src/components/button.rs");
    let folder = directory.path().join("assets/icons");
    let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
    tree.update(cx, |tree, _| {
        tree.set_on_create(Rc::new(|path, is_dir, _| {
            std::fs::create_dir_all(path.parent().unwrap())?;
            if is_dir {
                std::fs::create_dir(path)?;
            } else {
                std::fs::File::create(path)?;
            }
            Ok(())
        }));
    });
    // 面板初始行重建异步完成，ensure_selected 才有可选行。
    cx.run_until_parked();

    cx.add_window_view(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.state.borrow_mut().ensure_selected();
            tree.handle_tree_new_entry(&TreeNewEntry, window, cx);
            tree.entry_name_editor.update(cx, |editor, cx| {
                editor.set_text("src/components/button.rs", cx)
            });
            assert!(
                tree.display_rows(cx)
                    .iter()
                    .any(|row| row.is_new && !row.is_dir)
            );
            tree.handle_tree_confirm_edit(&TreeConfirmEdit, window, cx);

            tree.state
                .borrow_mut()
                .select(directory.path().to_path_buf());
            tree.handle_tree_new_entry(&TreeNewEntry, window, cx);
            tree.entry_name_editor
                .update(cx, |editor, cx| editor.set_text("assets/icons/", cx));
            assert!(
                tree.display_rows(cx)
                    .iter()
                    .any(|row| row.is_new && row.is_dir)
            );
            tree.handle_tree_confirm_edit(&TreeConfirmEdit, window, cx);
        });
        TestView
    });

    assert!(file.is_file());
    assert!(folder.is_dir());
    cx.read_entity(&tree, |tree, _| {
        assert!(tree.edit_state.is_none());
        assert_eq!(
            tree.state.borrow().selected.as_deref(),
            Some(folder.as_path())
        );
    });
}

#[gpui::test]
fn trash_action_moves_the_selected_row_to_trash_and_selects_the_next_row(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let trashed_file = directory.path().join("trash-me.txt");
    let kept_file = directory.path().join("keep.txt");
    std::fs::write(&trashed_file, "content").expect("应创建测试文件");
    std::fs::write(&kept_file, "content").expect("应创建测试文件");
    let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
    let trashed = Rc::new(RefCell::new(None));
    let trashed_path = Rc::clone(&trashed);
    tree.update(cx, |tree, _| {
        tree.set_on_trash(Rc::new(move |path, _, _| {
            std::fs::remove_file(&path)?;
            *trashed_path.borrow_mut() = Some(path);
            Ok(())
        }));
        tree.state.borrow_mut().select(trashed_file.clone());
    });
    // 面板初始行重建异步完成，删除前才能定位被删行与邻居。
    cx.run_until_parked();

    cx.add_window_view(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.handle_tree_trash(&TreeTrash, window, cx);
        });
        TestView
    });

    assert_eq!(trashed.borrow().as_deref(), Some(trashed_file.as_path()));
    assert!(!trashed_file.exists());
    assert!(kept_file.exists());
    cx.read_entity(&tree, |tree, _| {
        assert_eq!(
            tree.state.borrow().selected.as_deref(),
            Some(kept_file.as_path()),
            "删除后应选中原位置的下一个条目"
        );
    });
}

#[gpui::test]
fn trash_action_ignores_the_root_row(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
    let called = Rc::new(Cell::new(false));
    let callback_called = Rc::clone(&called);
    tree.update(cx, |tree, _| {
        tree.set_on_trash(Rc::new(move |_, _, _| {
            callback_called.set(true);
            Ok(())
        }));
        tree.state
            .borrow_mut()
            .select(directory.path().to_path_buf());
    });

    cx.add_window_view(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.handle_tree_trash(&TreeTrash, window, cx);
        });
        TestView
    });

    assert!(!called.get(), "根目录行不应触发删除");
    assert!(directory.path().exists());
}

#[gpui::test]
fn trash_action_selects_the_last_row_after_deleting_the_final_entry(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let only_file = directory.path().join("only.txt");
    std::fs::write(&only_file, "content").expect("应创建测试文件");
    let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
    tree.update(cx, |tree, _| {
        tree.set_on_trash(Rc::new(|path, _, _| {
            std::fs::remove_file(path)?;
            Ok(())
        }));
        tree.state.borrow_mut().select(only_file.clone());
    });
    // 面板初始行重建异步完成，删除前才能定位被删行与邻居。
    cx.run_until_parked();

    cx.add_window_view(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.handle_tree_trash(&TreeTrash, window, cx);
        });
        TestView
    });

    cx.read_entity(&tree, |tree, _| {
        assert_eq!(
            tree.state.borrow().selected.as_deref(),
            Some(directory.path()),
            "删除最后一项后应选中新的最后一行（根目录）"
        );
    });
}

#[gpui::test]
fn git_status_events_update_row_colors(cx: &mut TestAppContext) {
    let (root, _temp) = test_git_repo();
    let project = cx.new(|cx| Project::new(root.clone(), cx));
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
    cx.run_until_parked();

    // 修改文件 → git 增量刷新 → StatusesChanged 事件 → 行颜色更新。
    let file = root.join("tracked.txt");
    std::fs::write(&file, "已修改\n").expect("应修改文件");
    project.update(cx, |project, cx| {
        project.git_store().update(cx, |store, cx| {
            store.refresh_statuses_for_paths(std::slice::from_ref(&file), cx);
        });
    });
    cx.run_until_parked();

    let status = cx.read_entity(&tree, |tree, _| {
        tree.state
            .borrow()
            .rows
            .iter()
            .find(|row| row.path == file)
            .and_then(|row| row.git_status)
    });
    assert!(
        status.is_some_and(|status| status.is_modified()),
        "行应携带 modified 状态"
    );
}

#[gpui::test]
fn git_status_events_color_directories_with_changed_children(cx: &mut TestAppContext) {
    let (root, _temp) = test_git_repo();
    // 建子目录并提交一个文件。
    let src = root.join("src");
    std::fs::create_dir_all(&src).expect("应创建目录");
    let src_file = src.join("main.rs");
    std::fs::write(&src_file, "fn main() {}\n").expect("应创建文件");
    let run = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .output()
            .expect("应执行成功");
        assert!(output.status.success(), "git {:?} 失败", args);
    };
    run(&["add", "src/main.rs"]);
    run(&["commit", "-q", "-m", "add src"]);

    let project = cx.new(|cx| Project::new(root.clone(), cx));
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
    cx.run_until_parked();

    // 修改目录内文件 → 增量刷新 → 目录行聚合为 modified。
    std::fs::write(&src_file, "fn main() { println!(); }\n").expect("应修改文件");
    project.update(cx, |project, cx| {
        project.git_store().update(cx, |store, cx| {
            store.refresh_statuses_for_paths(std::slice::from_ref(&src_file), cx);
        });
    });
    cx.run_until_parked();

    let status = cx.read_entity(&tree, |tree, _| {
        tree.state
            .borrow()
            .rows
            .iter()
            .find(|row| row.path == src)
            .and_then(|row| row.git_status)
    });
    assert!(
        status.is_some_and(|status| status.is_modified()),
        "目录行应聚合子项状态"
    );
}

#[gpui::test]
fn expanding_directory_fills_git_status_for_new_rows(cx: &mut TestAppContext) {
    // 回归：展开目录产生的新行此前未查询过，git 状态缓存需在展开时补齐。
    let (root, _temp) = test_git_repo();
    let sub = root.join("sub");
    std::fs::create_dir_all(&sub).expect("应创建目录");
    let sub_file = sub.join("untracked.txt");
    std::fs::write(&sub_file, "x\n").expect("应创建文件");

    let project = cx.new(|cx| Project::new(root.clone(), cx));
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
    cx.run_until_parked();

    // 展开 sub 目录（模拟 handle_tree_expand 的行重建路径）。
    tree.update(cx, |tree, cx| {
        tree.state.borrow_mut().expanded.insert(sub.clone());
        tree.rebuild_rows(cx);
    });
    tree.update(cx, |tree, cx| tree.refresh_git_statuses(cx));
    cx.run_until_parked();

    let status = cx.read_entity(&tree, |tree, _| {
        tree.state
            .borrow()
            .rows
            .iter()
            .find(|row| row.path == sub_file)
            .and_then(|row| row.git_status)
    });
    assert!(
        status.is_some_and(|status| status.is_untracked()),
        "展开后新出现的文件行应补齐 git 状态"
    );
}

#[gpui::test]
fn activating_directory_fills_git_status_for_new_rows(cx: &mut TestAppContext) {
    // 回归：鼠标点击/键盘激活目录统一走 TreeActivate handler（toggle_expand），
    // 展开后的新行 git 状态需在激活时补齐。
    let (root, _temp) = test_git_repo();
    let sub = root.join("sub");
    std::fs::create_dir_all(&sub).expect("应创建目录");
    let sub_file = sub.join("untracked.txt");
    std::fs::write(&sub_file, "x\n").expect("应创建文件");

    let project = cx.new(|cx| Project::new(root.clone(), cx));
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
    cx.run_until_parked();

    // 模拟鼠标点击：选中行后激活（与键盘 enter 同一 handler）。
    cx.add_window_view(|window, cx| {
        tree.update(cx, |tree, _| {
            tree.state.borrow_mut().select(sub.clone());
        });
        tree.update(cx, |tree, cx| {
            tree.handle_tree_activate(&TreeActivate, window, cx);
        });
        TestView
    });

    let status = cx.read_entity(&tree, |tree, _| {
        tree.state
            .borrow()
            .rows
            .iter()
            .find(|row| row.path == sub_file)
            .and_then(|row| row.git_status)
    });
    assert!(
        status.is_some_and(|status| status.is_untracked()),
        "激活展开后新出现的文件行应补齐 git 状态"
    );
}

/// 创建带一个初始提交的临时 git 仓库，返回 (仓库根, 目录句柄)。
fn test_git_repo() -> (PathBuf, tempfile::TempDir) {
    let temp_dir = tempfile::tempdir().expect("应创建临时目录");
    let root = temp_dir.path().to_path_buf();
    let run = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .output()
            .expect("应执行成功");
        assert!(
            output.status.success(),
            "git {:?} 失败：{}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run(&["init", "-q", "-b", "master"]);
    run(&["config", "user.email", "test@example.com"]);
    run(&["config", "user.name", "Test User"]);
    std::fs::write(root.join("tracked.txt"), "第一行\n第二行\n").expect("应写入初始文件");
    run(&["add", "tracked.txt"]);
    run(&["commit", "-q", "-m", "initial"]);
    (root, temp_dir)
}

/// gitignored 目录（如 .gitignore 命中 tmp/）可展开，子项进入行模型并继承忽略状态淡显。
///
/// 回归：此前被忽略目录的展开被拦截（防 node_modules 撑爆行模型的旧方案）， 表现为"点了没反应"；现展开正常，重型目录由扫描排除名单治理。
#[gpui::test]
fn ignored_directory_expands_with_ignored_children(cx: &mut TestAppContext) {
    let (root, _temp) = test_git_repo();
    std::fs::write(root.join(".gitignore"), "tmp/\n").expect("应写入 .gitignore");
    let tmp = root.join("tmp");
    let child = tmp.join("cordis-tutorial");
    std::fs::create_dir_all(&child).expect("应创建 tmp/子目录");
    std::fs::write(child.join("note.md"), "x\n").expect("应创建文件");

    let project = cx.new(|cx| Project::new(root.clone(), cx));
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
    cx.run_until_parked();

    // 模拟鼠标/键盘逐级展开 tmp 与子目录。
    tree.update(cx, |tree, cx| {
        tree.state.borrow_mut().expanded.insert(tmp.clone());
        tree.state.borrow_mut().expanded.insert(child.clone());
        tree.rebuild_rows(cx);
    });
    tree.update(cx, |tree, cx| tree.refresh_git_statuses(cx));
    cx.run_until_parked();

    let rows = cx.read_entity(&tree, |tree, _| {
        tree.state
            .borrow()
            .rows
            .iter()
            .map(|row| (row.path.clone(), row.git_status))
            .collect::<Vec<_>>()
    });
    // tmp、其子目录与文件都应进入行模型，且全部携带 Ignored 状态（淡显）。
    for expected in [&tmp, &child, &child.join("note.md")] {
        let found = rows
            .iter()
            .find(|(path, _)| path == expected)
            .unwrap_or_else(|| panic!("{expected:?} 应在行模型中，实际：{rows:?}"));
        assert!(
            found.1.is_some_and(|status| status.is_ignored()),
            "{expected:?} 应为 Ignored（淡显），实际：{:?}",
            found.1
        );
    }
}

/// 无 worktree 的空项目：面板照常构造，root 为空、行模型为空（渲染空态提示）。
#[gpui::test]
fn empty_project_has_no_root_and_empty_rows(cx: &mut TestAppContext) {
    let project = cx.update(|cx| cx.new(Project::empty));
    let (tree, cx) = cx.add_window_view(move |_, cx| ProjectTreePanel::new(project, cx));

    cx.read_entity(&tree, |tree, _| {
        assert!(tree.root.is_none());
        assert!(tree.state.borrow().rows.is_empty());
    });
}

#[gpui::test]
fn rapid_clicks_toggle_directory_each_click(cx: &mut TestAppContext) {
    // 快速连点：每次点击目录都切换展开/折叠（click_count 递增，不能吞掉 2+ 次点击）。
    let (root, _temp) = test_git_repo();
    let sub = root.join("sub");
    std::fs::create_dir_all(&sub).expect("应创建目录");
    std::fs::write(sub.join("file.txt"), "x\n").expect("应创建文件");

    let project = cx.new(|cx| Project::new(root.clone(), cx));
    let (tree, cx) = cx.add_window_view({
        let project = project.clone();
        move |_, cx| ProjectTreePanel::new(project, cx)
    });
    cx.run_until_parked();

    let click = |tree: &gpui::Entity<ProjectTreePanel>, cx: &mut VisualTestContext| {
        cx.update(|window, cx| {
            tree.update(cx, |tree, _| tree.state.borrow_mut().select(sub.clone()));
            tree.update(cx, |tree, cx| {
                tree.handle_tree_activate(&TreeActivate, window, cx)
            });
        });
    };

    // 第 1 次：展开；第 2 次：折叠；第 3 次：展开。
    click(&tree, cx);
    click(&tree, cx);
    cx.read_entity(&tree, |tree, _| {
        assert!(
            !tree.state.borrow().expanded.contains(&sub),
            "连点两次后应回到折叠"
        );
    });
    click(&tree, cx);
    cx.read_entity(&tree, |tree, _| {
        assert!(
            tree.state.borrow().expanded.contains(&sub),
            "连点三次后应保持展开"
        );
    });
}

/// 创建含 a/b/c 三个文件的项目并返回 (临时目录, Project, 各文件路径)。
/// 行序固定为 [根, a.txt, b.txt, c.txt]，供点击坐标计算。
fn three_file_project(
    cx: &mut TestAppContext,
) -> (
    tempfile::TempDir,
    gpui::Entity<Project>,
    PathBuf,
    PathBuf,
    PathBuf,
) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let file_a = directory.path().join("a.txt");
    let file_b = directory.path().join("b.txt");
    let file_c = directory.path().join("c.txt");
    std::fs::write(&file_a, "a").expect("应创建测试文件");
    std::fs::write(&file_b, "b").expect("应创建测试文件");
    std::fs::write(&file_c, "c").expect("应创建测试文件");
    let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));
    (directory, project, file_a, file_b, file_c)
}

#[gpui::test]
fn shift_click_extends_selection_range(cx: &mut TestAppContext) {
    let (_temp, project, file_a, file_b, file_c) = three_file_project(cx);

    let (tree, cx) = cx.add_window_view({
        let project = project.clone();
        move |_, cx| {
            let mut tree = ProjectTreePanel::new(project, cx);
            let root = tree.root.clone().expect("测试项目应包含根目录");
            tree.state.borrow_mut().expanded.insert(root);
            tree.rebuild_rows(cx);
            tree
        }
    });
    cx.run_until_parked();
    focus_tree(&tree, cx);

    // 行序 [根, a.txt, b.txt, c.txt]；行高为 ui_line()。
    let row_height = zcv_theme::typography::ui_line();
    // 先普通点击 a.txt（行 1）建立单选锚点。
    cx.simulate_click(
        point(px(10.), px(f32::from(row_height) * 1.0 + 1.)),
        gpui::Modifiers::default(),
    );
    cx.run_until_parked();
    // shift 点击 c.txt（行 3）：区间扩展为 {a, b, c}。
    cx.simulate_click(
        point(px(10.), px(f32::from(row_height) * 3.0 + 1.)),
        gpui::Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    cx.run_until_parked();

    cx.read_entity(&tree, |tree, _| {
        let state = tree.state.borrow();
        assert_eq!(state.selected.as_deref(), Some(file_c.as_path()));
        assert_eq!(
            state.selected_set,
            HashSet::from([file_a.clone(), file_b.clone(), file_c.clone()]),
            "shift 点击应把锚点到目标行之间的全部行收入集合"
        );
    });
}

#[gpui::test]
fn secondary_click_toggles_selection_membership(cx: &mut TestAppContext) {
    let (_temp, project, file_a, file_b, file_c) = three_file_project(cx);

    let (tree, cx) = cx.add_window_view({
        let project = project.clone();
        move |_, cx| {
            let mut tree = ProjectTreePanel::new(project, cx);
            let root = tree.root.clone().expect("测试项目应包含根目录");
            tree.state.borrow_mut().expanded.insert(root);
            tree.rebuild_rows(cx);
            tree
        }
    });
    cx.run_until_parked();
    focus_tree(&tree, cx);

    let row_height = zcv_theme::typography::ui_line();
    // 普通点击 a.txt（行 1）建立单选。
    cx.simulate_click(
        point(px(10.), px(f32::from(row_height) * 1.0 + 1.)),
        gpui::Modifiers::default(),
    );
    // secondary 点击 b.txt（行 2）与 c.txt（行 3）：逐项加入集合。
    cx.simulate_click(
        point(px(10.), px(f32::from(row_height) * 2.0 + 1.)),
        gpui::Modifiers::secondary_key(),
    );
    cx.simulate_click(
        point(px(10.), px(f32::from(row_height) * 3.0 + 1.)),
        gpui::Modifiers::secondary_key(),
    );
    cx.run_until_parked();

    cx.read_entity(&tree, |tree, _| {
        let state = tree.state.borrow();
        assert_eq!(state.selected.as_deref(), Some(file_c.as_path()));
        assert_eq!(
            state.selected_set,
            HashSet::from([file_a.clone(), file_b.clone(), file_c.clone()]),
            "首次标记并入普通点击的首项，后续逐项加入多选集合"
        );
        assert_eq!(
            state.anchor.as_deref(),
            Some(file_a.as_path()),
            "toggle 不应移动锚点"
        );
    });

    // 再次 secondary 点击 c.txt：移除标记，游标留在该行，并入的首项保留。
    cx.simulate_click(
        point(px(10.), px(f32::from(row_height) * 3.0 + 1.)),
        gpui::Modifiers::secondary_key(),
    );
    cx.run_until_parked();
    cx.read_entity(&tree, |tree, _| {
        let state = tree.state.borrow();
        assert_eq!(
            state.selected_set,
            HashSet::from([file_a.clone(), file_b.clone()])
        );
        assert_eq!(state.selected.as_deref(), Some(file_c.as_path()));
    });
}

#[gpui::test]
fn shift_down_keystroke_extends_selection_to_next_row(cx: &mut TestAppContext) {
    let (_temp, project, file_a, file_b, _file_c) = three_file_project(cx);

    let (tree, cx) = cx.add_window_view({
        let project = project.clone();
        let selected = file_a.clone();
        move |_, cx| {
            cx.bind_keys([KeyBinding::new(
                "shift-down",
                TreeSelectNextExtend,
                Some("ProjectTree && not_editing"),
            )]);
            let mut tree = ProjectTreePanel::new(project, cx);
            let root = tree.root.clone().expect("测试项目应包含根目录");
            tree.state.borrow_mut().expanded.insert(root);
            tree.rebuild_rows(cx);
            tree.state.borrow_mut().select(selected);
            tree
        }
    });
    cx.run_until_parked();
    focus_tree(&tree, cx);

    cx.simulate_keystrokes("shift-down");

    cx.read_entity(&tree, |tree, _| {
        let state = tree.state.borrow();
        assert_eq!(state.selected.as_deref(), Some(file_b.as_path()));
        assert_eq!(
            state.selected_set,
            HashSet::from([file_a.clone(), file_b.clone()]),
            "shift-down 应把锚点到新游标的区间收入集合"
        );
        assert_eq!(state.anchor.as_deref(), Some(file_a.as_path()));
    });
}

#[gpui::test]
fn trash_action_deletes_multiple_selected_rows(cx: &mut TestAppContext) {
    let (_temp, project, file_a, file_b, file_c) = three_file_project(cx);
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
    let trashed_paths = Rc::new(RefCell::new(Vec::new()));
    let recorded = Rc::clone(&trashed_paths);
    tree.update(cx, |tree, _| {
        tree.set_on_trash(Rc::new(move |path, _, _| {
            std::fs::remove_file(&path)?;
            recorded.borrow_mut().push(path);
            Ok(())
        }));
        let mut state = tree.state.borrow_mut();
        state.select(file_a.clone());
        state.toggle_selection(&file_a);
        state.toggle_selection(&file_b);
    });
    // 面板初始行重建异步完成，有效选中集才能从行模型解析。
    cx.run_until_parked();

    cx.add_window_view(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.handle_tree_trash(&TreeTrash, window, cx);
        });
        TestView
    });

    let trashed = trashed_paths.borrow().clone();
    assert_eq!(trashed.len(), 2, "应删除两条路径");
    assert!(trashed.contains(&file_a), "应包含 {}", file_a.display());
    assert!(trashed.contains(&file_b), "应包含 {}", file_b.display());
    assert!(!file_a.exists());
    assert!(!file_b.exists());
    assert!(file_c.exists(), "未选中文件不应被删除");
    cx.read_entity(&tree, |tree, _| {
        assert_eq!(
            tree.state.borrow().selected.as_deref(),
            Some(file_c.as_path()),
            "批量删除后游标应收拢到原位置邻居"
        );
    });
}

#[gpui::test]
fn scheduled_refresh_coalesces_rapid_entries_changed_events(cx: &mut TestAppContext) {
    let (temp, project, _file_a, _file_b, _file_c) = three_file_project(cx);
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
    cx.run_until_parked();

    // 批量新建文件后连发两次 EntriesChanged（模拟事件风暴）。
    let file = temp.path().join("new.txt");
    std::fs::write(&file, "content").expect("应创建测试文件");
    tree.update(cx, |tree, cx| tree.schedule_refresh(cx));
    tree.update(cx, |tree, cx| tree.schedule_refresh(cx));
    cx.run_until_parked();

    let contains_new_file = |cx: &mut TestAppContext| {
        cx.read_entity(&tree, |tree, _| {
            tree.state.borrow().rows.iter().any(|row| row.path == file)
        })
    };
    // 防抖窗口内未到期：行模型尚未包含新文件。
    assert!(!contains_new_file(cx), "防抖窗口内不应刷新行模型");

    // 推进 120ms：被覆盖的旧任务作废，只有最新会话执行一次刷新。
    cx.executor().advance_clock(REFRESH_DEBOUNCE);
    cx.run_until_parked();
    assert!(contains_new_file(cx), "到期后应完成一次合并刷新");
}

// ═══ M3：剪贴板、粘贴与冲突确认 ═══════════════════════════

/// 构造含目标目录的测试项目：root/{a.txt, b.txt, dst/}，返回
/// (临时目录, project, root, a.txt, b.txt, dst)。
fn paste_project(
    cx: &mut TestAppContext,
) -> (
    tempfile::TempDir,
    gpui::Entity<Project>,
    PathBuf,
    PathBuf,
    PathBuf,
    PathBuf,
) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let root = directory.path().to_path_buf();
    let file_a = root.join("a.txt");
    let file_b = root.join("b.txt");
    let target = root.join("dst");
    std::fs::write(&file_a, "a").expect("应创建测试文件");
    std::fs::write(&file_b, "b").expect("应创建测试文件");
    std::fs::create_dir(&target).expect("应创建目标目录");
    let project = cx.new(|cx| Project::new(root.clone(), cx));
    (directory, project, root, file_a, file_b, target)
}

/// 构造含三个文件与目标目录的拖拽测试项目：root/{a.txt, b.txt, c.txt, dst/}，返回
/// (临时目录, project, root, a.txt, b.txt, c.txt, dst)。
/// 行序固定为 [根, dst, a.txt, b.txt, c.txt]（目录先于文件），供点击/拖拽坐标计算。
fn drag_project(
    cx: &mut TestAppContext,
) -> (
    tempfile::TempDir,
    gpui::Entity<Project>,
    PathBuf,
    PathBuf,
    PathBuf,
    PathBuf,
    PathBuf,
) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let root = directory.path().to_path_buf();
    let file_a = root.join("a.txt");
    let file_b = root.join("b.txt");
    let file_c = root.join("c.txt");
    let target = root.join("dst");
    std::fs::write(&file_a, "a").expect("应创建测试文件");
    std::fs::write(&file_b, "b").expect("应创建测试文件");
    std::fs::write(&file_c, "c").expect("应创建测试文件");
    std::fs::create_dir(&target).expect("应创建目标目录");
    let project = cx.new(|cx| Project::new(root.clone(), cx));
    (directory, project, root, file_a, file_b, file_c, target)
}

#[gpui::test]
fn copy_paste_duplicates_files_into_selected_directory(cx: &mut TestAppContext) {
    let (_temp, project, _root, file_a, file_b, target) = paste_project(cx);
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
    cx.run_until_parked();

    // 多选 a.txt 与 b.txt 后复制，选中目标目录粘贴。
    tree.update(cx, |tree, _| {
        let mut state = tree.state.borrow_mut();
        state.select(file_a.clone());
        state.toggle_selection(&file_a);
        state.toggle_selection(&file_b);
    });
    cx.add_window_view(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.handle_tree_copy(&TreeCopy, window, cx);
            tree.state.borrow_mut().select(target.clone());
            tree.handle_tree_paste(&TreePaste, window, cx);
        });
        TestView
    });
    cx.run_until_parked();

    // Copy 语义：目标出现副本、源文件仍在。
    assert_eq!(
        std::fs::read_to_string(target.join("a.txt")).expect("应读取复制出的文件"),
        "a"
    );
    assert_eq!(
        std::fs::read_to_string(target.join("b.txt")).expect("应读取复制出的文件"),
        "b"
    );
    assert!(file_a.is_file(), "复制不删除源文件");
    assert!(file_b.is_file(), "复制不删除源文件");
    // 完成后回到单选态：选中首个成功目标、进度清零。
    cx.read_entity(&tree, |tree, _| {
        assert_eq!(
            tree.state.borrow().selected.as_deref(),
            Some(target.join("a.txt").as_path()),
            "粘贴完成后应选中首个成功目标"
        );
        assert!(
            tree.state.borrow().selected_set.is_empty(),
            "粘贴完成后应回到单选态"
        );
        assert!(tree.active_transfer.is_none(), "复制完成后进度应清零");
    });
}

#[gpui::test]
fn copy_paste_reports_progress_middle_state_and_clears_on_completion(cx: &mut TestAppContext) {
    let (_temp, project, _root, file_a, file_b, target) = paste_project(cx);
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
    cx.run_until_parked();

    // 直接驱动两项复制（粘贴的 Copy 执行入口）：派发后后台任务尚未被驱动，
    // 此刻读取可稳定卡住复制进行中的 Some((n, total)) 中间态。
    tree.update(cx, |tree, cx| {
        tree.execute_copy(
            vec![
                (file_a.clone(), target.join("a.txt")),
                (file_b.clone(), target.join("b.txt")),
            ],
            &HashSet::new(),
            cx,
        );
    });

    cx.read_entity(&tree, |tree, _| {
        assert_eq!(
            tree.active_transfer,
            Some((0, 2)),
            "复制进行中应报告 (已完成, 总数) 中间态"
        );
    });

    cx.run_until_parked();
    // 进度值单调递增至总数后清零；两项副本落地。
    cx.read_entity(&tree, |tree, _| {
        assert!(tree.active_transfer.is_none(), "复制完成后进度应清零");
        assert_eq!(
            tree.state.borrow().selected.as_deref(),
            Some(target.join("a.txt").as_path()),
            "完成后应选中首个成功目标"
        );
    });
    assert!(target.join("a.txt").is_file(), "第一项应复制完成");
    assert!(target.join("b.txt").is_file(), "第二项应复制完成");
}

#[gpui::test]
fn paste_during_active_copy_is_ignored(cx: &mut TestAppContext) {
    let (_temp, project, _root, file_a, _file_b, target) = paste_project(cx);
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
    cx.run_until_parked();

    // 模拟进行中的异步传输：进度 0/3 与剪贴板单项不同，便于观察进度是否被新传输重置。
    tree.update(cx, |tree, _| {
        tree.active_transfer = Some((0, 3));
        tree.clipboard = Some(TreeClipboard::Copied(vec![file_a.clone()]));
        tree.state.borrow_mut().select(target.clone());
    });
    cx.add_window_view(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.handle_tree_paste(&TreePaste, window, cx);
        });
        TestView
    });

    cx.read_entity(&tree, |tree, _| {
        assert_eq!(
            tree.active_transfer,
            Some((0, 3)),
            "进行中粘贴应被忽略，进度不被重置"
        );
        assert!(tree.conflict.is_none(), "进行中粘贴不应进入冲突会话");
    });
    assert!(!target.join("a.txt").exists(), "进行中粘贴不应产生副本");

    // 传输结束后粘贴恢复正常语义。
    tree.update(cx, |tree, _| tree.active_transfer = None);
    cx.add_window_view(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.handle_tree_paste(&TreePaste, window, cx);
        });
        TestView
    });
    cx.run_until_parked();
    assert!(target.join("a.txt").is_file(), "传输结束后粘贴应正常执行");
}

#[gpui::test]
fn copy_paste_recurses_into_directories_with_chinese_names(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let root = directory.path().to_path_buf();
    let source_dir = root.join("源码");
    std::fs::create_dir_all(source_dir.join("嵌套")).expect("应创建嵌套目录");
    std::fs::write(source_dir.join("嵌套").join("中文文件.txt"), "内容").expect("应创建测试文件");
    let target = root.join("备份");
    std::fs::create_dir(&target).expect("应创建目标目录");
    let project = cx.new(|cx| Project::new(root.clone(), cx));
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
    cx.run_until_parked();

    cx.add_window_view(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.state.borrow_mut().select(source_dir.clone());
            tree.handle_tree_copy(&TreeCopy, window, cx);
            tree.state.borrow_mut().select(target.clone());
            tree.handle_tree_paste(&TreePaste, window, cx);
        });
        TestView
    });
    cx.run_until_parked();

    assert_eq!(
        std::fs::read_to_string(target.join("源码").join("嵌套").join("中文文件.txt"))
            .expect("应递归复制出嵌套中文文件"),
        "内容"
    );
    assert!(
        source_dir.join("嵌套").join("中文文件.txt").is_file(),
        "复制不删除源目录"
    );
}

#[gpui::test]
fn cut_paste_moves_file_and_degrades_clipboard(cx: &mut TestAppContext) {
    let (_temp, project, _root, file_a, _file_b, target) = paste_project(cx);
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
    cx.run_until_parked();

    // on_move 转发数据层 move_path（装配层注入的真实回调）。
    let project_for_move = project.clone();
    tree.update(cx, |tree, _| {
        tree.set_on_move(Rc::new(
            move |from: PathBuf, to: PathBuf, overwrite: bool, cx: &mut gpui::App| {
                project_for_move.update(cx, |project, cx| {
                    project.move_path(&from, &to, overwrite, cx)
                })
            },
        ));
    });

    cx.add_window_view(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.state.borrow_mut().select(file_a.clone());
            tree.handle_tree_cut(&TreeCut, window, cx);
            tree.state.borrow_mut().select(target.clone());
            tree.handle_tree_paste(&TreePaste, window, cx);
        });
        TestView
    });
    cx.run_until_parked();

    // Move 语义：原路径消失、新路径存在。
    assert!(!file_a.exists(), "剪切粘贴后原路径应消失");
    assert_eq!(
        std::fs::read_to_string(target.join("a.txt")).expect("应读取移动后的文件"),
        "a"
    );
    cx.read_entity(&tree, |tree, _| {
        // 首次粘贴后剪切降级为复制。
        assert!(
            matches!(tree.clipboard, Some(TreeClipboard::Copied(_))),
            "剪切首次粘贴后应降级为复制"
        );
        // 游标收拢到成功目标（selected 未指向成功项时选中首个目标）。
        assert_eq!(
            tree.state.borrow().selected.as_deref(),
            Some(target.join("a.txt").as_path())
        );
    });
}

#[gpui::test]
fn paste_into_file_row_targets_parent_directory(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let root = directory.path().to_path_buf();
    let anchor = root.join("a.txt");
    let source = root.join("src").join("x.txt");
    std::fs::write(&anchor, "anchor").expect("应创建测试文件");
    std::fs::create_dir_all(root.join("src")).expect("应创建源目录");
    std::fs::write(&source, "x").expect("应创建测试文件");
    let project = cx.new(|cx| Project::new(root.clone(), cx));
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
    cx.run_until_parked();

    cx.add_window_view(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.state.borrow_mut().select(source.clone());
            tree.handle_tree_copy(&TreeCopy, window, cx);
            // 选中文件行 a.txt：粘贴目标应取其父目录（根）。
            tree.state.borrow_mut().select(anchor.clone());
            tree.handle_tree_paste(&TreePaste, window, cx);
        });
        TestView
    });
    cx.run_until_parked();

    assert!(root.join("x.txt").is_file(), "粘贴到文件行应落到其父目录");
    assert!(source.is_file(), "Copy 语义源不动");
}

#[gpui::test]
fn paste_conflict_confirm_overwrites_target_item_by_item(cx: &mut TestAppContext) {
    let (_temp, project, _root, file_a, file_b, target) = paste_project(cx);
    // 预置两个同名冲突目标。
    std::fs::write(target.join("a.txt"), "旧 a").expect("应创建冲突目标");
    std::fs::write(target.join("b.txt"), "旧 b").expect("应创建冲突目标");
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
    cx.run_until_parked();

    cx.add_window_view(|window, cx| {
        tree.update(cx, |tree, cx| {
            let mut state = tree.state.borrow_mut();
            state.select(file_a.clone());
            state.toggle_selection(&file_a);
            state.toggle_selection(&file_b);
            drop(state);
            tree.handle_tree_copy(&TreeCopy, window, cx);
            tree.state.borrow_mut().select(target.clone());
            tree.handle_tree_paste(&TreePaste, window, cx);
        });
        TestView
    });
    cx.run_until_parked();

    // 两项全冲突：进入会话，决策前不写入。
    cx.read_entity(&tree, |tree, _| {
        let conflict = tree.conflict.as_ref().expect("应进入冲突会话");
        assert_eq!(conflict.len(), 2);
    });
    assert_eq!(
        std::fs::read_to_string(target.join("a.txt")).expect("应读取目标文件"),
        "旧 a",
        "决策前不应写入"
    );

    // 第一项确认：会话未结束，仍停留在第二项。
    tree.update(cx, |tree, cx| {
        tree.resolve_conflict(ConfirmAnswer::Confirm, cx)
    });
    cx.read_entity(&tree, |tree, _| {
        assert!(tree.conflict.is_some(), "剩余项待决策时会话应保留");
    });

    // 第二项确认：会话出队执行，两项均被覆盖。
    tree.update(cx, |tree, cx| {
        tree.resolve_conflict(ConfirmAnswer::Confirm, cx)
    });
    cx.run_until_parked();

    cx.read_entity(&tree, |tree, _| {
        assert!(tree.conflict.is_none(), "全部决策完成后会话应结束");
    });
    assert_eq!(
        std::fs::read_to_string(target.join("a.txt")).expect("应读取覆盖后的目标"),
        "a",
        "确认后目标应被覆盖"
    );
    assert_eq!(
        std::fs::read_to_string(target.join("b.txt")).expect("应读取覆盖后的目标"),
        "b"
    );
    assert!(file_a.is_file() && file_b.is_file(), "Copy 语义源不动");
}

#[gpui::test]
fn conflict_session_defers_clean_items_until_resolved(cx: &mut TestAppContext) {
    let (_temp, project, _root, file_a, file_b, target) = paste_project(cx);
    // 仅 b.txt 与目标同名冲突；a.txt 为非冲突项。
    std::fs::write(target.join("b.txt"), "旧 b").expect("应创建冲突目标");
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
    cx.run_until_parked();

    cx.add_window_view(|window, cx| {
        tree.update(cx, |tree, cx| {
            let mut state = tree.state.borrow_mut();
            state.select(file_a.clone());
            state.toggle_selection(&file_a);
            state.toggle_selection(&file_b);
            drop(state);
            tree.handle_tree_copy(&TreeCopy, window, cx);
            tree.state.borrow_mut().select(target.clone());
            tree.handle_tree_paste(&TreePaste, window, cx);
        });
        TestView
    });
    cx.run_until_parked();

    // 会话只含冲突项 b.txt；非冲突项 a.txt 也暂不执行。
    cx.read_entity(&tree, |tree, _| {
        assert_eq!(
            tree.conflict.as_ref().expect("应进入冲突会话").len(),
            1,
            "会话只装冲突项"
        );
    });
    assert!(!target.join("a.txt").exists(), "决策完成前非冲突项不应执行");

    // 决策完成后非冲突项与冲突项合成完整清单统一执行。
    tree.update(cx, |tree, cx| {
        tree.resolve_conflict(ConfirmAnswer::Confirm, cx)
    });
    cx.run_until_parked();

    assert!(target.join("a.txt").is_file(), "非冲突项应在决策后执行");
    assert_eq!(
        std::fs::read_to_string(target.join("b.txt")).expect("应读取覆盖后的目标"),
        "b",
        "冲突项应按覆盖决策执行"
    );
}

#[gpui::test]
fn paste_conflict_skip_keeps_both_sides(cx: &mut TestAppContext) {
    let (_temp, project, _root, file_a, _file_b, target) = paste_project(cx);
    std::fs::write(target.join("a.txt"), "旧 a").expect("应创建冲突目标");
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
    cx.run_until_parked();

    cx.add_window_view(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.state.borrow_mut().select(file_a.clone());
            tree.handle_tree_copy(&TreeCopy, window, cx);
            tree.state.borrow_mut().select(target.clone());
            tree.handle_tree_paste(&TreePaste, window, cx);
        });
        TestView
    });
    cx.run_until_parked();
    assert!(cx.read_entity(&tree, |tree, _| tree.conflict.is_some()));

    tree.update(cx, |tree, cx| {
        tree.resolve_conflict(ConfirmAnswer::Skip, cx)
    });
    cx.run_until_parked();

    assert!(cx.read_entity(&tree, |tree, _| tree.conflict.is_none()));
    assert_eq!(
        std::fs::read_to_string(target.join("a.txt")).expect("应读取目标文件"),
        "旧 a",
        "跳过项目标不应变化"
    );
    assert_eq!(
        std::fs::read_to_string(&file_a).expect("应读取源文件"),
        "a",
        "跳过项源不应变化"
    );
}

#[gpui::test]
fn paste_conflict_cancel_changes_nothing(cx: &mut TestAppContext) {
    let (_temp, project, _root, file_a, file_b, target) = paste_project(cx);
    std::fs::write(target.join("b.txt"), "旧 b").expect("应创建冲突目标");
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
    cx.run_until_parked();

    cx.add_window_view(|window, cx| {
        tree.update(cx, |tree, cx| {
            let mut state = tree.state.borrow_mut();
            state.select(file_a.clone());
            state.toggle_selection(&file_a);
            state.toggle_selection(&file_b);
            drop(state);
            tree.handle_tree_copy(&TreeCopy, window, cx);
            tree.state.borrow_mut().select(target.clone());
            tree.handle_tree_paste(&TreePaste, window, cx);
        });
        TestView
    });
    cx.run_until_parked();
    assert!(cx.read_entity(&tree, |tree, _| tree.conflict.is_some()));

    tree.update(cx, |tree, cx| {
        tree.resolve_conflict(ConfirmAnswer::Cancel, cx)
    });
    cx.run_until_parked();

    // 整体中止：无任何项被执行（含非冲突项）。
    assert!(cx.read_entity(&tree, |tree, _| tree.conflict.is_none()));
    assert!(!target.join("a.txt").exists(), "取消后非冲突项不应执行");
    assert_eq!(
        std::fs::read_to_string(target.join("b.txt")).expect("应读取目标文件"),
        "旧 b",
        "取消后目标不应变化"
    );
    assert!(file_a.is_file() && file_b.is_file(), "取消后源不应变化");
}

#[gpui::test]
fn paste_without_clipboard_is_noop(cx: &mut TestAppContext) {
    let (_temp, project, _root, _file_a, _file_b, target) = paste_project(cx);
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
    cx.run_until_parked();

    cx.add_window_view(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.state.borrow_mut().select(target.clone());
            tree.handle_tree_paste(&TreePaste, window, cx);
        });
        TestView
    });
    cx.run_until_parked();

    cx.read_entity(&tree, |tree, _| {
        assert!(tree.clipboard.is_none());
        assert!(tree.conflict.is_none());
        assert!(tree.active_transfer.is_none());
    });
    assert!(
        !target.join("a.txt").exists(),
        "空剪贴板粘贴不应产生任何写入"
    );
}

#[gpui::test]
fn escape_keystroke_cancels_conflict_session(cx: &mut TestAppContext) {
    let (_temp, project, _root, file_a, _file_b, target) = paste_project(cx);
    std::fs::write(target.join("a.txt"), "旧 a").expect("应创建冲突目标");

    // 面板作为窗口根视图（key_context 才能进入按键分发链），
    // 同时注册 escape 绑定（keymap 的 ProjectTree && conflict 分组）。
    let (tree, cx) = cx.add_window_view(|_window, cx| {
        cx.bind_keys([KeyBinding::new(
            "escape",
            TreeCancelConflict,
            Some("ProjectTree && conflict"),
        )]);
        let tree = ProjectTreePanel::new(project, cx);
        tree.state.borrow_mut().select(file_a.clone());
        tree
    });
    cx.run_until_parked();

    // 行模型就绪后制造冲突会话。
    cx.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.handle_tree_copy(&TreeCopy, window, cx);
            tree.state.borrow_mut().select(target.clone());
            tree.handle_tree_paste(&TreePaste, window, cx);
        });
    });
    cx.run_until_parked();
    assert!(cx.read_entity(&tree, |tree, _| tree.conflict.is_some()));

    focus_tree(&tree, cx);
    cx.simulate_keystrokes("escape");

    cx.read_entity(&tree, |tree, _| {
        assert!(tree.conflict.is_none(), "escape 应取消冲突会话");
    });
    assert_eq!(
        std::fs::read_to_string(target.join("a.txt")).expect("应读取目标文件"),
        "旧 a",
        "escape 取消后目标不应变化"
    );
}

#[gpui::test]
fn cut_marks_clipboard_paths_for_dimming(cx: &mut TestAppContext) {
    let (_temp, project, _root, file_a, file_b, _target) = paste_project(cx);
    let tree = cx.new(|cx| ProjectTreePanel::new(project.clone(), cx));
    cx.run_until_parked();

    tree.update(cx, |tree, _| {
        let mut state = tree.state.borrow_mut();
        state.select(file_a.clone());
        state.toggle_selection(&file_a);
        state.toggle_selection(&file_b);
    });
    cx.add_window_view(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.handle_tree_cut(&TreeCut, window, cx);
        });
        TestView
    });

    // 淡显数据源：cut 后剪贴板携带路径且选中集保持（视觉基础）。
    cx.read_entity(&tree, |tree, _| {
        match &tree.clipboard {
            Some(TreeClipboard::Cut(paths)) => {
                assert_eq!(paths, &vec![file_a.clone(), file_b.clone()]);
            }
            other => panic!("cut 后剪贴板应为 Cut，实际 {other:?}"),
        }
        assert_eq!(
            tree.state.borrow().selected_set.len(),
            2,
            "cut 应保持选中集作淡显视觉基础"
        );
    });

    // copy 则为 Copied（无淡显）。
    cx.add_window_view(|window, cx| {
        tree.update(cx, |tree, cx| tree.handle_tree_copy(&TreeCopy, window, cx));
        TestView
    });
    cx.read_entity(&tree, |tree, _| {
        assert!(matches!(tree.clipboard, Some(TreeClipboard::Copied(_))));
    });
}

#[gpui::test]
fn escape_clears_cut_clipboard_and_dimming(cx: &mut TestAppContext) {
    let (_temp, project, _root, file_a, _file_b, _target) = paste_project(cx);
    // 面板作为窗口根视图（key_context 才能进入按键分发链），
    // 注册 escape 清空绑定（keymap 的 ProjectTree && not_editing 分组）。
    let (tree, cx) = cx.add_window_view(|_window, cx| {
        cx.bind_keys([KeyBinding::new(
            "escape",
            TreeClearClipboard,
            Some("ProjectTree && not_editing"),
        )]);
        let tree = ProjectTreePanel::new(project, cx);
        tree.state.borrow_mut().select(file_a.clone());
        tree
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        tree.update(cx, |tree, cx| tree.handle_tree_cut(&TreeCut, window, cx));
    });
    cx.read_entity(&tree, |tree, _| {
        assert!(matches!(tree.clipboard, Some(TreeClipboard::Cut(_))));
    });

    focus_tree(&tree, cx);
    cx.simulate_keystrokes("escape");

    // 剪贴板置空：淡显数据源（渲染时从 clipboard 派生的 Cut 路径集）随之为空，行不再淡显。
    cx.read_entity(&tree, |tree, _| {
        assert!(tree.clipboard.is_none(), "escape 应清空剪贴板");
    });
}

#[gpui::test]
fn escape_clears_copied_clipboard(cx: &mut TestAppContext) {
    let (_temp, project, _root, file_a, _file_b, _target) = paste_project(cx);
    let (tree, cx) = cx.add_window_view(|_window, cx| {
        cx.bind_keys([KeyBinding::new(
            "escape",
            TreeClearClipboard,
            Some("ProjectTree && not_editing"),
        )]);
        let tree = ProjectTreePanel::new(project, cx);
        tree.state.borrow_mut().select(file_a.clone());
        tree
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        tree.update(cx, |tree, cx| tree.handle_tree_copy(&TreeCopy, window, cx));
    });
    cx.read_entity(&tree, |tree, _| {
        assert!(matches!(tree.clipboard, Some(TreeClipboard::Copied(_))));
    });

    focus_tree(&tree, cx);
    cx.simulate_keystrokes("escape");

    cx.read_entity(&tree, |tree, _| {
        assert!(tree.clipboard.is_none(), "escape 应清空剪贴板");
    });
}

#[gpui::test]
fn escape_without_clipboard_is_noop(cx: &mut TestAppContext) {
    let (_temp, project, _root, file_a, _file_b, _target) = paste_project(cx);
    let (tree, cx) = cx.add_window_view(|_window, cx| {
        cx.bind_keys([KeyBinding::new(
            "escape",
            TreeClearClipboard,
            Some("ProjectTree && not_editing"),
        )]);
        let tree = ProjectTreePanel::new(project, cx);
        tree.state.borrow_mut().select(file_a.clone());
        tree
    });
    cx.run_until_parked();

    focus_tree(&tree, cx);
    cx.simulate_keystrokes("escape");

    // 无剪贴板时按 escape：无副作用（无冲突会话、选中不变）。
    cx.read_entity(&tree, |tree, _| {
        assert!(tree.clipboard.is_none());
        assert!(tree.conflict.is_none());
        assert_eq!(
            tree.state.borrow().selected.as_deref(),
            Some(file_a.as_path())
        );
    });
}

#[gpui::test]
fn escape_during_conflict_cancels_conflict_and_keeps_clipboard(cx: &mut TestAppContext) {
    let (_temp, project, _root, file_a, _file_b, target) = paste_project(cx);
    std::fs::write(target.join("a.txt"), "旧 a").expect("应创建冲突目标");

    // 与 keymap 一致的分组与顺序：not_editing 先注册、conflict 后注册，
    // 同深度命中时后注册者优先——冲突态下 escape 仍取消冲突而非清空剪贴板。
    let (tree, cx) = cx.add_window_view(|_window, cx| {
        cx.bind_keys([
            KeyBinding::new(
                "escape",
                TreeClearClipboard,
                Some("ProjectTree && not_editing"),
            ),
            KeyBinding::new(
                "escape",
                TreeCancelConflict,
                Some("ProjectTree && conflict"),
            ),
        ]);
        let tree = ProjectTreePanel::new(project, cx);
        tree.state.borrow_mut().select(file_a.clone());
        tree
    });
    cx.run_until_parked();

    // 行模型就绪后制造冲突会话。
    cx.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.handle_tree_copy(&TreeCopy, window, cx);
            tree.state.borrow_mut().select(target.clone());
            tree.handle_tree_paste(&TreePaste, window, cx);
        });
    });
    cx.run_until_parked();
    assert!(cx.read_entity(&tree, |tree, _| tree.conflict.is_some()));

    focus_tree(&tree, cx);
    cx.simulate_keystrokes("escape");

    cx.read_entity(&tree, |tree, _| {
        assert!(tree.conflict.is_none(), "escape 应取消冲突会话");
        assert!(
            matches!(tree.clipboard, Some(TreeClipboard::Copied(_))),
            "冲突取消不应清空剪贴板"
        );
    });
}

// ═══ M4：树内拖拽移动 ═══════════════════════════════════

/// 模拟一次拖拽手势：按住源行 → 移动（越过拖拽阈值）→ 在目标行释放。
/// 行 i 的 y 坐标为 ui_line * i + 1（与既有点击测试的坐标约定一致）。
fn simulate_drag(cx: &mut VisualTestContext, from_row: usize, to_row: usize) {
    let row_height = zcv_theme::typography::ui_line();
    let row_y = |row: usize| px(f32::from(row_height) * row as f32 + 1.);
    cx.simulate_mouse_down(
        point(px(10.), row_y(from_row)),
        MouseButton::Left,
        Modifiers::default(),
    );
    // 小幅度移动越过拖拽阈值：发起拖拽。
    cx.simulate_mouse_move(
        point(px(14.), row_y(from_row)),
        MouseButton::Left,
        Modifiers::default(),
    );
    // 移动到目标行：触发目标行的 drag_over / on_drag_move。
    cx.simulate_mouse_move(
        point(px(10.), row_y(to_row)),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.run_until_parked();
    // 释放：触发目标行的 drop。
    cx.simulate_mouse_up(
        point(px(10.), row_y(to_row)),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.run_until_parked();
}

#[gpui::test]
fn drag_file_into_directory_moves_it_via_move_pipeline(cx: &mut TestAppContext) {
    let (_temp, project, _root, file_a, _file_b, target) = paste_project(cx);
    let project_for_move = project.clone();
    let (tree, cx) = cx.add_window_view(move |_, cx| {
        let mut tree = ProjectTreePanel::new(project.clone(), cx);
        // 装配层注入的真实回调：转发数据层 move_path。
        tree.set_on_move(Rc::new(
            move |from: PathBuf, to: PathBuf, overwrite: bool, cx: &mut gpui::App| {
                project_for_move.update(cx, |project, cx| {
                    project.move_path(&from, &to, overwrite, cx)
                })
            },
        ));
        tree
    });
    cx.run_until_parked();

    // 行序 [根, dst, a.txt, b.txt]：拖 a.txt（行 2）放到 dst（行 1）。
    simulate_drag(cx, 2, 1);

    assert!(!file_a.exists(), "拖拽移动后源文件应消失");
    assert_eq!(
        std::fs::read_to_string(target.join("a.txt")).expect("应读取移动后的文件"),
        "a",
        "拖拽放下应把文件移入目标目录"
    );
    cx.read_entity(&tree, |tree, _| {
        assert!(tree.conflict.is_none(), "无冲突放下不应进入会话");
        assert!(tree.hover_expand_task.is_none(), "放下后悬停状态应清理");
        assert!(tree.drag_hover_path.is_none(), "放下后悬停记录应清理");
        assert_eq!(
            tree.state.borrow().selected.as_deref(),
            Some(target.join("a.txt").as_path()),
            "移动后游标应收拢到首个成功目标"
        );
    });
}

#[gpui::test]
fn drag_from_marked_row_moves_the_whole_multi_selection(cx: &mut TestAppContext) {
    let (_temp, project, _root, file_a, file_b, target) = paste_project(cx);
    let project_for_move = project.clone();
    // move 闭包捕获选中路径的克隆，原值留给闭包后的文件系统断言。
    let select_a = file_a.clone();
    let toggle_a = file_a.clone();
    let toggle_b = file_b.clone();
    let (_tree, cx) = cx.add_window_view(move |_, cx| {
        let mut tree = ProjectTreePanel::new(project.clone(), cx);
        tree.set_on_move(Rc::new(
            move |from: PathBuf, to: PathBuf, overwrite: bool, cx: &mut gpui::App| {
                project_for_move.update(cx, |project, cx| {
                    project.move_path(&from, &to, overwrite, cx)
                })
            },
        ));
        // 多选 a.txt 与 b.txt（标记集合）。
        let mut state = tree.state.borrow_mut();
        state.select(select_a);
        state.toggle_selection(&toggle_a);
        state.toggle_selection(&toggle_b);
        drop(state);
        tree
    });
    cx.run_until_parked();

    // 从已标记的 a.txt（行 2）拖起：载荷应为净化后的全部选中项。
    simulate_drag(cx, 2, 1);

    assert!(!file_a.exists() && !file_b.exists(), "多选各项都应被移动");
    assert!(
        target.join("a.txt").is_file() && target.join("b.txt").is_file(),
        "多选各项都应落入目标目录"
    );
}

/// 缺陷回归：按下即打开文件会在拖动时误开预览——打开文件经
/// reveal_active_path 的 select() 清空多选集合，选区拖拽随之中途退化为单项；
/// 拖动目录行也会误展开/折叠。修复后打开/展开动作延迟到 click（mouse_up
/// 未拖拽）派发，拖动不打开文件、不破坏选区。
#[gpui::test]
fn drag_does_not_open_file_preview_and_keeps_selection(cx: &mut TestAppContext) {
    let (_temp, project, _root, file_a, file_b, _file_c, target) = drag_project(cx);
    let project_for_move = project.clone();
    let open_count = Rc::new(Cell::new(0));
    let callback_count = Rc::clone(&open_count);
    let (tree, cx) = cx.add_window_view(move |_, cx| {
        let mut tree = ProjectTreePanel::new(project.clone(), cx);
        tree.set_on_move(Rc::new(
            move |from: PathBuf, to: PathBuf, overwrite: bool, cx: &mut gpui::App| {
                project_for_move.update(cx, |project, cx| {
                    project.move_path(&from, &to, overwrite, cx)
                })
            },
        ));
        tree.set_on_open_file(Rc::new(move |_, _, _, _| {
            callback_count.set(callback_count.get() + 1);
        }));
        tree
    });
    cx.run_until_parked();
    focus_tree(&tree, cx);

    // 多选 {a, b} 并完成渲染（普通点击 a 预览打开一次属正常点击行为），
    // 记录打开次数后从 a 行拖起放到 dst。
    let row_height = zcv_theme::typography::ui_line();
    let row_y = |row: usize| px(f32::from(row_height) * row as f32 + 1.);
    cx.simulate_click(point(px(10.), row_y(2)), gpui::Modifiers::default());
    cx.simulate_click(point(px(10.), row_y(3)), gpui::Modifiers::secondary_key());
    cx.run_until_parked();
    let opened_before_drag = open_count.get();
    simulate_drag(cx, 2, 1);

    assert_eq!(open_count.get(), opened_before_drag, "拖动不应打开文件预览");
    assert!(!file_a.exists() && !file_b.exists(), "多选各项都应被移动");
    assert!(
        target.join("a.txt").is_file() && target.join("b.txt").is_file(),
        "多选各项都应落入目标目录"
    );
}

/// 缺陷回归：拖动目录行不应触发展开/折叠。
/// 展开/折叠动作在 click（mouse_up 未拖拽）派发，拖拽消费了 click 则不执行。
#[gpui::test]
fn drag_directory_row_does_not_toggle_expansion(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let root = directory.path().to_path_buf();
    let src = root.join("src");
    std::fs::create_dir(&src).expect("应创建源目录");
    std::fs::write(src.join("inner.txt"), "x").expect("应创建测试文件");
    let project = cx.new(|cx| Project::new(root.clone(), cx));
    let project_for_move = project.clone();
    let (tree, cx) = cx.add_window_view(move |_, cx| {
        let mut tree = ProjectTreePanel::new(project.clone(), cx);
        tree.set_on_move(Rc::new(
            move |from: PathBuf, to: PathBuf, overwrite: bool, cx: &mut gpui::App| {
                project_for_move.update(cx, |project, cx| {
                    project.move_path(&from, &to, overwrite, cx)
                })
            },
        ));
        tree
    });
    cx.run_until_parked();
    focus_tree(&tree, cx);

    // 行序 [根, src]（src 折叠，子项不可见）：拖 src（行 1）放到根行（行 0）。
    simulate_drag(cx, 1, 0);

    cx.read_entity(&tree, |tree, _| {
        assert!(
            !tree.state.borrow().expanded.contains(&src),
            "拖动目录行不应触发展开"
        );
    });
    assert!(src.join("inner.txt").is_file(), "目录不应被移动");
}

/// 缺陷回归：真实用户路径「先普通点击首项、再逐个 cmd 点击」建立多选后，
/// 从选中行拖起必须移动全部选中项。
///
/// 缺陷机理：旧实现中 `toggle_selection` 首次打标记时不把当前游标行并入集合，
/// 普通点击的首项（游标）留在集合之外；随后按下该行时 `in_set` 为假被收拢为单选，
/// 拖拽载荷也只含该行，表现为「多选拖拽只移动一项」。既有通过测试是手工把首项
/// 也 toggle 进集合构造的，与用户真实点击序列不一致，故未能暴露。
#[gpui::test]
fn drag_after_cmd_click_multi_selection_moves_all_items(cx: &mut TestAppContext) {
    let (_temp, project, _root, file_a, file_b, file_c, target) = drag_project(cx);
    let project_for_move = project.clone();
    let (tree, cx) = cx.add_window_view(move |_, cx| {
        let mut tree = ProjectTreePanel::new(project.clone(), cx);
        tree.set_on_move(Rc::new(
            move |from: PathBuf, to: PathBuf, overwrite: bool, cx: &mut gpui::App| {
                project_for_move.update(cx, |project, cx| {
                    project.move_path(&from, &to, overwrite, cx)
                })
            },
        ));
        tree
    });
    cx.run_until_parked();
    focus_tree(&tree, cx);

    // 行序 [根, dst, a, b, c]：真实点击序列——普通点击 a 后逐个 cmd 点击 b、c。
    let row_height = zcv_theme::typography::ui_line();
    let row_y = |row: usize| px(f32::from(row_height) * row as f32 + 1.);
    cx.simulate_click(point(px(10.), row_y(2)), gpui::Modifiers::default());
    cx.simulate_click(point(px(10.), row_y(3)), gpui::Modifiers::secondary_key());
    cx.simulate_click(point(px(10.), row_y(4)), gpui::Modifiers::secondary_key());
    cx.run_until_parked();

    // 从首个点选行（a，行 2）拖起放到 dst（行 1）：a 必须仍在多选集合内。
    cx.read_entity(&tree, |tree, _| {
        assert!(
            tree.state.borrow().is_in_selection_set(&file_a),
            "普通点击的首项应随首次 cmd 点击并入多选集合"
        );
    });
    simulate_drag(cx, 2, 1);

    assert!(
        !file_a.exists() && !file_b.exists() && !file_c.exists(),
        "多选各项都应被移动，不应只移动拖起的一行"
    );
    assert!(
        target.join("a.txt").is_file()
            && target.join("b.txt").is_file()
            && target.join("c.txt").is_file(),
        "多选各项都应落入目标目录"
    );
}

/// 缺陷回归：放下信任发起时冻结的载荷，拖拽期间选区如何变化都不影响移动清单。
///
/// 缺陷机理（用户可感知）：「选区还在、拖拽却只移动一项」的根因之一是旧实现放下时实时读 selected_set——拖拽期间集合被清空/剪枝后，被拖行不在集合内即退化为单项；
/// 而载荷（发起时所见）与行为（放下时）时基分裂。修复后载荷在渲染期冻结完整多选快照（与视觉同帧），发起与放下都信任它。
///
/// 时序设计：建立多选 {a, b} 并完成渲染（载荷冻结含两项）→ 从 a 拖起 → 拖拽移动中把集合清空（模拟拖拽期间任何选区变化，不 notify 使场景最极端） → 放下到 dst：仍应移动发起时载荷中的 {a, b}。
#[gpui::test]
fn drop_trusts_snapshot_frozen_at_drag_start(cx: &mut TestAppContext) {
    let (_temp, project, _root, file_a, file_b, _file_c, target) = drag_project(cx);
    let project_for_move = project.clone();
    let (tree, cx) = cx.add_window_view(move |_, cx| {
        let mut tree = ProjectTreePanel::new(project.clone(), cx);
        tree.set_on_move(Rc::new(
            move |from: PathBuf, to: PathBuf, overwrite: bool, cx: &mut gpui::App| {
                project_for_move.update(cx, |project, cx| {
                    project.move_path(&from, &to, overwrite, cx)
                })
            },
        ));
        tree
    });
    cx.run_until_parked();
    focus_tree(&tree, cx);

    let row_height = zcv_theme::typography::ui_line();
    let row_y = |row: usize| px(f32::from(row_height) * row as f32 + 1.);
    // 普通点击 a（行 2）+ cmd 点击 b（行 3）：建立多选 {a, b} 并完成渲染。
    cx.simulate_click(point(px(10.), row_y(2)), gpui::Modifiers::default());
    cx.simulate_click(point(px(10.), row_y(3)), gpui::Modifiers::secondary_key());
    cx.run_until_parked();
    cx.read_entity(&tree, |tree, _| {
        assert_eq!(
            tree.state.borrow().selected_set.len(),
            2,
            "预置：多选 {{a, b}}"
        );
    });

    // 从 a 行拖起（按下时 a 在集合内：实时判定保持多选，载荷冻结含两项）。
    cx.simulate_mouse_down(
        point(px(10.), row_y(2)),
        MouseButton::Left,
        gpui::Modifiers::default(),
    );
    cx.simulate_mouse_move(
        point(px(14.), row_y(2)),
        MouseButton::Left,
        gpui::Modifiers::default(),
    );
    cx.simulate_mouse_move(
        point(px(10.), row_y(1)),
        MouseButton::Left,
        gpui::Modifiers::default(),
    );
    cx.run_until_parked();

    // 拖拽移动中清空多选集合（不 notify，使「放下时实时读」的旧实现必然退化为单项）。
    tree.update(cx, |tree, _| {
        let mut state = tree.state.borrow_mut();
        state.toggle_selection(&file_a);
        state.toggle_selection(&file_b);
        assert!(state.selected_set.is_empty(), "预置：拖拽中集合被清空");
    });

    cx.simulate_mouse_up(
        point(px(10.), row_y(1)),
        MouseButton::Left,
        gpui::Modifiers::default(),
    );
    cx.run_until_parked();

    assert!(
        !file_a.exists() && !file_b.exists(),
        "拖拽期间集合被清空，放下仍应移动发起时冻结的载荷，不得退化为单项"
    );
    assert!(
        target.join("a.txt").is_file() && target.join("b.txt").is_file(),
        "多选各项都应落入目标目录"
    );
}

/// 缺陷回归：未聚焦时首次 cmd 点击也应直接执行多选语义，后续拖拽移动全部选中项。
#[gpui::test]
fn drag_after_unfocused_first_cmd_click_moves_all_items(cx: &mut TestAppContext) {
    let (_temp, project, _root, file_a, file_b, _file_c, target) = drag_project(cx);
    let project_for_move = project.clone();
    let (_tree, cx) = cx.add_window_view(move |_, cx| {
        let mut tree = ProjectTreePanel::new(project.clone(), cx);
        tree.set_on_move(Rc::new(
            move |from: PathBuf, to: PathBuf, overwrite: bool, cx: &mut gpui::App| {
                project_for_move.update(cx, |project, cx| {
                    project.move_path(&from, &to, overwrite, cx)
                })
            },
        ));
        tree
    });
    cx.run_until_parked();
    // 不预先聚焦，模拟用户从编辑器转向项目树的真实入口。
    let row_height = zcv_theme::typography::ui_line();
    let row_y = |row: usize| px(f32::from(row_height) * row as f32 + 1.);
    cx.simulate_click(point(px(10.), row_y(2)), gpui::Modifiers::secondary_key());
    cx.simulate_click(point(px(10.), row_y(3)), gpui::Modifiers::secondary_key());
    cx.run_until_parked();

    // 从首项 a（行 2）拖起放到 dst（行 1）：a、b 都应被移动。
    simulate_drag(cx, 2, 1);

    assert!(
        !file_a.exists() && !file_b.exists(),
        "未聚焦首击选中的首项也应参与多选拖拽，不应只移动单项"
    );
    assert!(
        target.join("a.txt").is_file() && target.join("b.txt").is_file(),
        "多选各项都应落入目标目录"
    );
}

/// 缺陷回归：shift 点击建立区间多选后，从区间中部行拖起应移动整个区间。
#[gpui::test]
fn drag_after_shift_range_selection_moves_all_items(cx: &mut TestAppContext) {
    let (_temp, project, _root, file_a, file_b, file_c, target) = drag_project(cx);
    let project_for_move = project.clone();
    let (tree, cx) = cx.add_window_view(move |_, cx| {
        let mut tree = ProjectTreePanel::new(project.clone(), cx);
        tree.set_on_move(Rc::new(
            move |from: PathBuf, to: PathBuf, overwrite: bool, cx: &mut gpui::App| {
                project_for_move.update(cx, |project, cx| {
                    project.move_path(&from, &to, overwrite, cx)
                })
            },
        ));
        tree
    });
    cx.run_until_parked();
    focus_tree(&tree, cx);

    // 普通点击 a（行 2）建立锚点，shift 点击 c（行 4）扩展区间 {a, b, c}。
    let row_height = zcv_theme::typography::ui_line();
    let row_y = |row: usize| px(f32::from(row_height) * row as f32 + 1.);
    cx.simulate_click(point(px(10.), row_y(2)), gpui::Modifiers::default());
    cx.simulate_click(
        point(px(10.), row_y(4)),
        gpui::Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    cx.run_until_parked();

    // 从区间中部 b（行 3）拖起放到 dst（行 1）。
    simulate_drag(cx, 3, 1);

    assert!(
        !file_a.exists() && !file_b.exists() && !file_c.exists(),
        "shift 区间多选各项都应被移动"
    );
    assert!(
        target.join("a.txt").is_file()
            && target.join("b.txt").is_file()
            && target.join("c.txt").is_file(),
        "多选各项都应落入目标目录"
    );
}

/// 缺陷回归（间歇性线索）：首次多选拖拽成功后不重建面板，等待防抖刷新到期重建行模型，
/// 再次多选并拖拽仍应移动全部选中项——验证首次移动后的选区/行模型无状态残留。
#[gpui::test]
fn second_drag_after_successful_move_still_moves_multi_selection(cx: &mut TestAppContext) {
    let (_temp, project, _root, file_a, file_b, _file_c, target) = drag_project(cx);
    let project_for_move = project.clone();
    let (tree, cx) = cx.add_window_view(move |_, cx| {
        let mut tree = ProjectTreePanel::new(project.clone(), cx);
        tree.set_on_move(Rc::new(
            move |from: PathBuf, to: PathBuf, overwrite: bool, cx: &mut gpui::App| {
                project_for_move.update(cx, |project, cx| {
                    project.move_path(&from, &to, overwrite, cx)
                })
            },
        ));
        tree
    });
    cx.run_until_parked();
    focus_tree(&tree, cx);

    let row_height = zcv_theme::typography::ui_line();
    let row_y = |row: usize| px(f32::from(row_height) * row as f32 + 1.);
    // 第一轮：普通点击 a（行 2）+ cmd 点击 b（行 3），拖到 dst（行 1）。
    cx.simulate_click(point(px(10.), row_y(2)), gpui::Modifiers::default());
    cx.simulate_click(point(px(10.), row_y(3)), gpui::Modifiers::secondary_key());
    cx.run_until_parked();
    simulate_drag(cx, 2, 1);
    assert!(
        target.join("a.txt").is_file() && target.join("b.txt").is_file(),
        "首轮多选拖拽应移动全部选中项"
    );

    // 模拟文件监听触发的防抖刷新到期：行模型异步重建，选区键随之迁移/剪枝。
    tree.update(cx, |tree, cx| tree.schedule_refresh(cx));
    cx.executor().advance_clock(REFRESH_DEBOUNCE);
    cx.run_until_parked();

    // 第二轮：行序变为 [根, dst, dst/a.txt, dst/b.txt]（目录子项紧随目录）；
    // 点选两个已移入文件再多选，拖回根目录行（行 0），不得受首轮状态残留影响。
    cx.simulate_click(point(px(10.), row_y(2)), gpui::Modifiers::default());
    cx.simulate_click(point(px(10.), row_y(3)), gpui::Modifiers::secondary_key());
    cx.run_until_parked();
    simulate_drag(cx, 3, 0);

    assert!(
        file_a.is_file() && file_b.is_file(),
        "第二轮多选拖拽后文件应回到项目根，首次移动不得残留干扰状态"
    );
    assert!(
        !target.join("a.txt").exists() && !target.join("b.txt").exists(),
        "第二轮多选各项都应被移动，不应退化为单项"
    );
}

#[gpui::test]
fn drag_directory_into_its_own_subtree_is_rejected(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let root = directory.path().to_path_buf();
    let src = root.join("src");
    let inner = src.join("inner.txt");
    std::fs::create_dir(&src).expect("应创建源目录");
    std::fs::write(&inner, "inner").expect("应创建测试文件");
    let project = cx.new(|cx| Project::new(root.clone(), cx));
    let move_called = Rc::new(Cell::new(false));
    let callback_called = Rc::clone(&move_called);
    let (tree, cx) = cx.add_window_view({
        let src = src.clone();
        move |_, cx| {
            let mut tree = ProjectTreePanel::new(project.clone(), cx);
            tree.set_on_move(Rc::new(move |_, _, _, _| {
                callback_called.set(true);
                Ok(())
            }));
            // 展开 src 使 inner.txt 行可见；
            // 目录切换延迟到 click，拖拽不会触发，因而拖拽全程行序稳定。
            tree.state.borrow_mut().expanded.insert(src);
            tree.rebuild_rows(cx);
            tree
        }
    });
    cx.run_until_parked();

    // 行序 [根, src, src/inner.txt]：拖 src（行 1）放到 inner.txt（行 2）。
    simulate_drag(cx, 1, 2);

    assert!(!move_called.get(), "移入自身子树不应触发移动服务");
    assert!(inner.is_file(), "文件应保持原位");
    cx.read_entity(&tree, |tree, _| {
        assert!(tree.conflict.is_none(), "非法落点不应进入冲突会话");
    });
}

#[gpui::test]
fn drag_hovering_folded_directory_expands_it_after_delay(cx: &mut TestAppContext) {
    let (_temp, project, _root, _file_a, _file_b, target) = paste_project(cx);
    let project_for_move = project.clone();
    let (tree, cx) = cx.add_window_view(move |_, cx| {
        let mut tree = ProjectTreePanel::new(project.clone(), cx);
        tree.set_on_move(Rc::new(
            move |from: PathBuf, to: PathBuf, overwrite: bool, cx: &mut gpui::App| {
                project_for_move.update(cx, |project, cx| {
                    project.move_path(&from, &to, overwrite, cx)
                })
            },
        ));
        tree
    });
    cx.run_until_parked();

    // 拖起 a.txt（行 2）后悬停在折叠的 dst 行（行 1）上不释放。
    let row_height = zcv_theme::typography::ui_line();
    let row_y = |row: usize| px(f32::from(row_height) * row as f32 + 1.);
    cx.simulate_mouse_down(
        point(px(10.), row_y(2)),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.simulate_mouse_move(
        point(px(14.), row_y(2)),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.simulate_mouse_move(
        point(px(10.), row_y(1)),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.run_until_parked();
    assert!(
        cx.read_entity(&tree, |tree, _| tree.hover_expand_task.is_some()),
        "悬停折叠目录行应调度展开计时器"
    );

    // 未到点：保持折叠。
    cx.executor().advance_clock(HOVER_EXPAND_DELAY / 2);
    cx.run_until_parked();
    cx.read_entity(&tree, |tree, _| {
        assert!(
            !tree.state.borrow().expanded.contains(&target),
            "未到延迟时不应展开"
        );
    });

    // 到点：自动展开且计时器清理。
    cx.executor().advance_clock(HOVER_EXPAND_DELAY / 2);
    cx.run_until_parked();
    cx.read_entity(&tree, |tree, _| {
        assert!(
            tree.state.borrow().expanded.contains(&target),
            "悬停约 500ms 后折叠目录应自动展开"
        );
        assert!(tree.hover_expand_task.is_none(), "到期后计时器应清理");
        assert!(tree.drag_hover_path.is_none(), "到期后悬停记录应清理");
    });
}

// ═══ 回归：空白点击仅聚焦、不改选中 ═════════════════════════
//
// 锁定既定语义：行数较少、列表填不满面板时，点击行下方空白区域——
// 面板根 div（track_focus + size_full）获得焦点，这是空白点击唯一的反应；
// 行级 on_mouse_down 依赖每行 hitbox 命中，空白坐标不在任何行内，
// 容器上也没有鼠标按下处理器，故不会写入任何选中状态。

#[gpui::test]
fn clicking_blank_area_below_rows_keeps_selection_and_focuses_panel(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let file = directory.path().join("a.txt");
    std::fs::write(&file, "hello").expect("应创建测试文件");
    let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));
    let (tree, cx) = cx.add_window_view({
        let project = project.clone();
        move |_, cx| {
            let mut tree = ProjectTreePanel::new(project, cx);
            // 展开根目录使文件行可见：仅 [根, a.txt] 两行，填不满面板。
            let root = tree.root.clone().expect("测试项目应包含根目录");
            tree.state.borrow_mut().expanded.insert(root);
            tree.rebuild_rows(cx);
            tree
        }
    });
    cx.run_until_parked();

    // 先显式点击 a.txt（行 1）建立期望选中（首击聚焦并选中该行）。
    let row_height = zcv_theme::typography::ui_line();
    cx.simulate_click(
        point(px(10.), px(f32::from(row_height) + 1.)),
        Modifiers::default(),
    );
    cx.run_until_parked();
    cx.read_entity(&tree, |tree, _| {
        assert_eq!(
            tree.state.borrow().selected.as_deref(),
            Some(file.as_path()),
            "预置：点击行应选中该行"
        );
    });

    // 移走焦点，使「空白点击聚焦面板」的断言有判别力。
    cx.update(|window, cx| window.blur(cx));
    let focused_before = cx.update(|window, cx| tree.read(cx).focus.contains_focused(window, cx));
    assert!(!focused_before, "预置：失焦后面板不应处于聚焦态");
    // 记录点击空白前的选中快照（含多选集合）。
    let (selected_before, set_before) = cx.read_entity(&tree, |tree, _| {
        let state = tree.state.borrow();
        (state.selected.clone(), state.selected_set.clone())
    });

    // 点击行区域以下的空白：y 明显大于 行数(2) × 行高，x 在面板内。
    let blank = point(px(10.), px(f32::from(row_height) * 2. + 10.));
    cx.simulate_click(blank, Modifiers::default());
    cx.run_until_parked();

    // 空白点击唯一的反应：根容器（track_focus）获得焦点。
    let focused_after = cx.update(|window, cx| tree.read(cx).focus.is_focused(window));
    assert!(focused_after, "空白点击应聚焦面板根容器");
    // 选中路径与多选集合保持不变：空白坐标不命中任何行的 hitbox。
    cx.read_entity(&tree, |tree, _| {
        let state = tree.state.borrow();
        assert_eq!(state.selected, selected_before, "空白点击不应改写选中路径");
        assert_eq!(
            state.selected.as_deref(),
            Some(file.as_path()),
            "选中应仍停留在显式点击过的那行"
        );
        assert_eq!(state.selected_set, set_before, "空白点击不应改写多选集合");
    });
}

/// 回归（用户线索）：多选建立后点击行下方空白区域，多选集合不得被清空，
/// 随后的多选拖拽仍应移动全部选中项。空白点击唯一反应是聚焦根容器，
/// 未命中任何行的坐标不得写入任何选中状态。
#[gpui::test]
fn blank_click_between_multi_selection_and_drag_keeps_set(cx: &mut TestAppContext) {
    let (_temp, project, _root, file_a, file_b, _file_c, target) = drag_project(cx);
    let project_for_move = project.clone();
    let (tree, cx) = cx.add_window_view(move |_, cx| {
        let mut tree = ProjectTreePanel::new(project.clone(), cx);
        tree.set_on_move(Rc::new(
            move |from: PathBuf, to: PathBuf, overwrite: bool, cx: &mut gpui::App| {
                project_for_move.update(cx, |project, cx| {
                    project.move_path(&from, &to, overwrite, cx)
                })
            },
        ));
        tree
    });
    cx.run_until_parked();
    focus_tree(&tree, cx);

    // 行序 [根, dst, a, b, c]：普通点击 a（行 2）+ cmd 点击 b（行 3）建立多选。
    let row_height = zcv_theme::typography::ui_line();
    let row_y = |row: usize| px(f32::from(row_height) * row as f32 + 1.);
    cx.simulate_click(point(px(10.), row_y(2)), gpui::Modifiers::default());
    cx.simulate_click(point(px(10.), row_y(3)), gpui::Modifiers::secondary_key());
    cx.run_until_parked();

    // 点击行下方空白（共 5 行，y 明显越过 5×行高）。
    cx.simulate_click(
        point(px(10.), px(f32::from(row_height) * 5. + 10.)),
        gpui::Modifiers::default(),
    );
    cx.run_until_parked();

    cx.read_entity(&tree, |tree, _| {
        let state = tree.state.borrow();
        assert!(
            state.is_in_selection_set(&file_a),
            "空白点击不得清空多选集合（a 项）"
        );
        assert!(
            state.is_in_selection_set(&file_b),
            "空白点击不得清空多选集合（b 项）"
        );
    });

    // 空白点击后从 a（行 2）拖起放到 dst（行 1）：仍应移动全部选中项。
    simulate_drag(cx, 2, 1);
    assert!(
        !file_a.exists() && !file_b.exists(),
        "空白点击后的多选拖拽仍应移动全部选中项"
    );
    assert!(
        target.join("a.txt").is_file() && target.join("b.txt").is_file(),
        "多选各项都应落入目标目录"
    );
}

#[gpui::test]
fn clicking_blank_area_without_prior_row_click_keeps_fallback_selection(cx: &mut TestAppContext) {
    // 子场景：从未显式点击过任何行。行模型建好后，渲染期的 ensure_selected 兜底
    // 已把选中填为首行（根行），「选中为空」在有行面板中不可停留；
    // 本用例锁定：该兜底之外，空白点击自身不产生任何额外的选中写入。
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let file = directory.path().join("a.txt");
    std::fs::write(&file, "hello").expect("应创建测试文件");
    let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));
    let (tree, cx) = cx.add_window_view({
        let project = project.clone();
        move |_, cx| {
            let mut tree = ProjectTreePanel::new(project, cx);
            let root = tree.root.clone().expect("测试项目应包含根目录");
            tree.state.borrow_mut().expanded.insert(root);
            tree.rebuild_rows(cx);
            tree
        }
    });
    cx.run_until_parked();

    // 点击空白前记录选中快照：应为渲染兜底的首行（根行），多选集合为空。
    let root = cx.read_entity(&tree, |tree, _| tree.root.clone());
    let root = root.expect("测试项目应包含根目录");
    let (selected_before, set_before) = cx.read_entity(&tree, |tree, _| {
        let state = tree.state.borrow();
        assert_eq!(
            state.selected.as_deref(),
            Some(root.as_path()),
            "预置：无点击历史时选中应为渲染兜底的首行（根行）"
        );
        (state.selected.clone(), state.selected_set.clone())
    });

    // 点击行区域以下的空白。
    let row_height = zcv_theme::typography::ui_line();
    let blank = point(px(10.), px(f32::from(row_height) * 2. + 10.));
    cx.simulate_click(blank, Modifiers::default());
    cx.run_until_parked();

    cx.read_entity(&tree, |tree, _| {
        let state = tree.state.borrow();
        assert_eq!(state.selected, selected_before, "空白点击不应写入新的选中");
        assert_eq!(state.selected_set, set_before, "空白点击不应写入多选集合");
        assert!(state.selected_set.is_empty(), "全程不应出现多选标记");
    });
}
