//! 装配层 —— 创建 Workspace，注入顶栏/面板/状态项，接线项目与设置订阅。
//!
//! Workspace 框架（Pane/Dock/命令分发）在 zcv-workspace；
//! 本模块只做 binary 侧的具体装配：面板（项目树/版本控制/占位面板）、状态栏按钮、git/settings 订阅与 diff hunks 推送。

mod placeholder_panels;

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    App, Bounds, Context, Entity, TitlebarOptions, Window, WindowBounds, WindowOptions, point,
    prelude::*, px, size,
};
use zcv_actions::{SelectGitBranch, ToggleProjectPicker};
use zcv_editor::{Editor, SoftWrap};
use zcv_project::Project;
use zcv_workspace::{
    Dock, DockPosition, FileToolbarControls, GitBranchAction, OnProjectSelected, OnSelectBranch,
    Pane, PaneEvent, PanelButtons, PanelHandle, TopBar, Workspace, add_to_recent,
};

use self::placeholder_panels::{DebugPanel, KeyboardShortcutsPanel, OutlinePanel, TerminalPanel};
use crate::active_buffer_language::ActiveBufferLanguage;
use crate::breadcrumbs::Breadcrumbs;
use crate::cursor_position::CursorPosition;
use crate::diagnostics::DiagnosticsButton;
use crate::language_tools::LspButton;
use crate::project_search::ProjectSearchButton;
use crate::project_tree::{OnCreate, OnOpenFile, OnRename, OnTrash, ProjectTree};
use crate::settings::SettingsStore;
use crate::version_control::VersionControlPanel;

pub(crate) use zcv_workspace::{
    Panel, StatusItemView, ToggleDiagnostics, ToggleLanguageServer, ToggleProjectSearch,
    ToggleProjectTree, ToggleVersionControl, ToolbarItemEvent, ToolbarItemLocation,
    ToolbarItemView,
};

/// 以类型擦除句柄注册面板。
fn register_panel<P: Panel>(
    workspace: &mut Workspace,
    entity: Entity<P>,
    position: DockPosition,
    cx: &mut Context<Workspace>,
) {
    let handle: Arc<dyn PanelHandle> = Arc::new(entity);
    workspace.register_panel(handle, position, cx);
}

/// 打开一个项目窗口（主窗口与「切换项目」的新窗口入口共用）。
pub(crate) fn open_project_window(root: PathBuf, cx: &mut App) -> anyhow::Result<()> {
    add_to_recent(&root.to_string_lossy());

    let bounds = Bounds::centered(None, size(px(1200.0), px(900.0)), cx);

    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(-100.0), px(-100.0))),
            }),
            ..Default::default()
        },
        |window, cx| {
            SettingsStore::get(cx).theme.apply(cx, Some(window));
            let workspace = cx.new(|cx| Workspace::new(root.clone(), window, cx));
            workspace.update(cx, |workspace, cx| {
                initialize_workspace(workspace, root, window, cx);
            });
            // 焦点延后到首帧渲染完成后：track_focus 元素未挂载前 focus 会静默丢失，导致启动后 keymap dispatch 无焦点链，快捷键（如 cmd-shift-p 打开项目选择器）不生效，直到用户点击界面（焦点链建立）才恢复。
            let focus = workspace.read(cx).focus.clone();
            window.defer(cx, move |window, _cx| {
                window.focus(&focus);
            });
            workspace
        },
    )?;
    Ok(())
}

/// 装配 Workspace：顶栏注入、面板/状态项注册、订阅接线。
///
/// 必须在 `Workspace::update` 闭包内调用（workspace 为 &mut），内部不得再对同一实体嵌套 update。
fn initialize_workspace(
    workspace: &mut Workspace,
    root: PathBuf,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    // ═══ 顶栏注入 ═══════════════════════════════════════════════════

    let weak_self: gpui::WeakEntity<Workspace> = cx.weak_entity();
    let on_project_selected: OnProjectSelected = Rc::new(move |path, window, app| {
        // 切换项目：新窗口打开目标项目，成功后关闭当前窗口。
        if let Err(error) = open_project_window(PathBuf::from(path.clone()), app) {
            eprintln!("打开项目失败（{path}）：{error}");
            return;
        }
        window.remove_window();
    });
    let weak_branch = weak_self.clone();
    let on_branch: OnSelectBranch = Rc::new(move |action, _window, app| {
        if let Some(ws) = weak_branch.upgrade() {
            ws.update(app, |workspace, cx| {
                let store = workspace.project().read(cx).git_store();
                match action {
                    GitBranchAction::Checkout(name) => {
                        store.update(cx, |store, cx| store.checkout_branch(name, cx));
                    }
                    GitBranchAction::Create(name) => {
                        store.update(cx, |store, cx| store.create_branch(name, cx));
                    }
                }
            });
        }
    });

    let top_bar = cx.new(|cx| TopBar::new(on_project_selected, on_branch, cx));
    top_bar.update(cx, |bar, cx| {
        let label = root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        bar.project_picker.update(cx, |picker, _| {
            picker.set_current_label(label);
        });
    });
    workspace.set_titlebar(top_bar.clone().into(), cx);
    // TopBar 组件不在主焦点链上：把选择器的命令 handler 注册到 Workspace 根节点（对齐 Zed register_action），全局可达。
    let project_picker = top_bar.read(cx).project_picker.clone();
    workspace.register_action(move |_workspace, _: &ToggleProjectPicker, window, cx| {
        project_picker.update(cx, |picker, cx| picker.toggle(window, cx));
    });
    let branch_picker = top_bar.read(cx).branch_picker.clone();
    workspace.register_action(move |_workspace, _: &SelectGitBranch, window, cx| {
        branch_picker.update(cx, |picker, cx| picker.toggle(window, cx));
    });
    workspace.set_open_settings_provider(Box::new(|_cx| {
        crate::settings::ensure_user_settings_file()
            .ok()
            .map(|path| path.to_path_buf())
    }));

    // ═══ 面板创建与注册 ═══════════════════════════════════════════

    let project = workspace.project().clone();

    let project_tree: Entity<ProjectTree> = cx.new(|cx| {
        let mut tree = ProjectTree::new(root.clone(), project.clone(), cx);
        let weak_open = weak_self.clone();
        let on_open_file: OnOpenFile = Rc::new(
            move |path: PathBuf,
                  focus_opened_item: bool,
                  window: &mut Window,
                  cx: &mut gpui::App| {
                if let Some(ws) = weak_open.upgrade() {
                    ws.update(cx, |ws, cx| {
                        ws.open_path(path, focus_opened_item, window, cx);
                    });
                }
            },
        );
        tree.set_on_open_file(on_open_file);
        let weak_rename = weak_self.clone();
        let on_rename: OnRename = Rc::new(move |from, to, cx| {
            let Some(workspace) = weak_rename.upgrade() else {
                anyhow::bail!("工作区已关闭");
            };
            workspace.update(cx, |workspace, cx| workspace.rename_path(&from, &to, cx))
        });
        tree.set_on_rename(on_rename);
        let weak_create = weak_self.clone();
        let on_create: OnCreate = Rc::new(move |path, is_dir, cx| {
            let Some(workspace) = weak_create.upgrade() else {
                anyhow::bail!("工作区已关闭");
            };
            workspace.update(cx, |workspace, cx| workspace.create_path(&path, is_dir, cx))
        });
        tree.set_on_create(on_create);
        let weak_trash = weak_self.clone();
        let on_trash: OnTrash = Rc::new(move |path, window, cx| {
            let Some(workspace) = weak_trash.upgrade() else {
                anyhow::bail!("工作区已关闭");
            };
            workspace.update(cx, |workspace, cx| workspace.trash_path(&path, window, cx))
        });
        tree.set_on_trash(on_trash);
        tree
    });

    let version_control: Entity<VersionControlPanel> = cx.new(|cx| {
        let mut panel = VersionControlPanel::new(root.clone(), project.clone(), cx);
        let weak_open = weak_self.clone();
        let on_open_file: OnOpenFile = Rc::new(
            move |path: PathBuf,
                  focus_opened_item: bool,
                  window: &mut Window,
                  cx: &mut gpui::App| {
                if let Some(ws) = weak_open.upgrade() {
                    ws.update(cx, |ws, cx| {
                        ws.open_path(path, focus_opened_item, window, cx);
                    });
                }
            },
        );
        panel.set_on_open_file(on_open_file);
        panel
    });

    let outline = cx.new(OutlinePanel::new);
    let terminal = cx.new(TerminalPanel::new);
    let debug = cx.new(DebugPanel::new);
    let keyboard_shortcuts = cx.new(KeyboardShortcutsPanel::new);

    register_panel(workspace, project_tree.clone(), DockPosition::Left, cx);
    register_panel(workspace, version_control, DockPosition::Left, cx);
    register_panel(workspace, outline, DockPosition::Left, cx);
    register_panel(workspace, terminal, DockPosition::Bottom, cx);
    register_panel(workspace, debug, DockPosition::Bottom, cx);
    register_panel(workspace, keyboard_shortcuts, DockPosition::Right, cx);

    // ═══ 状态栏注册 ═══════════════════════════════════════════════

    let status_bar = workspace.status_bar().clone();
    let left_dock = workspace.left_dock.clone();
    let bottom_dock = workspace.bottom_dock.clone();
    let right_dock = workspace.right_dock.clone();
    status_bar.update(cx, |bar, cx| {
        bar.add_left_item(cx.new(|cx| PanelButtons::new(left_dock.clone(), cx)), cx);
        bar.add_left_item(cx.new(|_| LspButton::new()), cx);
        bar.add_left_item(cx.new(|_| DiagnosticsButton::new()), cx);
        bar.add_left_item(cx.new(|_| ProjectSearchButton::new()), cx);
        bar.add_right_item(cx.new(|_| CursorPosition::new()), cx);
        bar.add_right_item(cx.new(|_| ActiveBufferLanguage::new()), cx);
        bar.add_right_item(cx.new(|cx| PanelButtons::new(bottom_dock.clone(), cx)), cx);
        bar.add_right_item(cx.new(|cx| PanelButtons::new(right_dock.clone(), cx)), cx);
    });

    // ═══ Toolbar 子项 ═════════════════════════════════════════════

    let pane = workspace.pane().clone();
    pane.update(cx, |pane, cx| {
        let toolbar = pane.toolbar().clone();
        toolbar.update(cx, |toolbar, cx| {
            toolbar.add_item(cx.new(|_| Breadcrumbs::new()), window, cx);
            toolbar.add_item(cx.new(|_| FileToolbarControls::new()), window, cx);
        });
    });

    // ═══ Dock 焦点转发（面板获得焦点时把焦点转发给面板内容）══════

    let docks = [
        workspace.left_dock.clone(),
        workspace.right_dock.clone(),
        workspace.bottom_dock.clone(),
    ];
    for dock in &docks {
        dock.update(cx, |dock: &mut Dock, cx: &mut Context<Dock>| {
            let focus = dock.focus.clone();
            let sub = cx.on_focus(
                &focus,
                window,
                |d: &mut Dock, w: &mut Window, c: &mut Context<Dock>| {
                    if let Some(panel) = d.visible_panel() {
                        w.focus(&panel.focus_handle(c));
                    }
                },
            );
            dock._subscriptions.push(sub);
        });
    }

    // ═══ 订阅接线 ═════════════════════════════════════════════════

    let git_store = project.read(cx).git_store();
    let git_subscription = cx.subscribe(&git_store, move |workspace, store, _event, cx| {
        let branch = store.read(cx).current_branch().map(str::to_string);
        let has_repositories = store.read(cx).has_repositories();
        let remote_operation_state = store.read(cx).remote_operation_state();
        // 分支列表随事件推送（活动仓库；选择器打开时同步渲染，无加载态）。
        let branch_list = store
            .read(cx)
            .active_branch_list()
            .map(|branches| branches.to_vec())
            .unwrap_or_default();
        top_bar.update(cx, |bar, cx| {
            bar.set_branch(branch, cx);
            bar.set_branches(branch_list, cx);
            bar.set_has_repositories(has_repositories);
            bar.set_remote_operation_state(remote_operation_state);
            cx.notify();
        });
        // hunks 查询完成（Statuses 事件）后补推给打开的编辑器；缺失路径按需请求。
        push_diff_hunks(workspace.pane(), workspace.project(), cx);
    });

    let project_tree_for_pane = project_tree.clone();
    let pane_subscription = cx.subscribe(&pane, move |workspace, pane, event, cx| {
        if matches!(
            event,
            PaneEvent::Activate { .. } | PaneEvent::Removed { .. }
        ) {
            let active_path = pane.read(cx).active_path(cx);
            // 活动仓库跟随焦点文件（最长前缀匹配）：打开/切换子项目文件后，
            // 分支显示与 fetch/pull/push 自动指向其所属仓库。
            if let Some(path) = &active_path {
                workspace.project().update(cx, |project, cx| {
                    project.git_store().update(cx, |store, cx| {
                        store.set_active_repository_for_path(path, cx);
                    });
                });
            }
            project_tree_for_pane.update(cx, |tree, cx| {
                tree.reveal_active_path(active_path, cx);
            });
        }
        // 打开/激活编辑器时推送 git diff hunks（打开即有快照里的现成数据）。
        push_diff_hunks(workspace.pane(), workspace.project(), cx);
        // 新打开文件应用当前换行设置。
        let settings = SettingsStore::get(cx);
        apply_soft_wrap(
            workspace.pane(),
            settings.soft_wrap,
            settings.preferred_line_length,
            cx,
        );
    });

    // 项目事件订阅：根重命名与文件树变化驱动项目树刷新。
    let project_tree_for_project = project_tree.clone();
    let project_subscription =
        cx.subscribe(
            &project,
            move |_workspace, _project, event, cx| match event {
                zcv_project::ProjectEvent::RootChanged(root) => {
                    project_tree_for_project.update(cx, |tree, cx| {
                        tree.set_root(root.clone(), cx);
                    });
                }
                zcv_project::ProjectEvent::EntriesChanged => {
                    project_tree_for_project.update(cx, |tree, cx| tree.refresh(cx));
                }
            },
        );

    let project_tree_for_settings = project_tree.clone();
    let settings_subscription =
        cx.observe_global_in::<SettingsStore>(window, move |workspace, window, cx| {
            let settings = SettingsStore::get(cx);
            settings.theme.apply(cx, Some(window));
            apply_soft_wrap(
                workspace.pane(),
                settings.soft_wrap,
                settings.preferred_line_length,
                cx,
            );
            // 扫描排除名单变化时重建项目树行模型。
            project_tree_for_settings.update(cx, |tree, cx| tree.refresh(cx));
            cx.notify();
        });

    let appearance_subscription = window.observe_window_appearance(|window, cx| {
        let settings = SettingsStore::get(cx);
        settings.theme.apply(cx, Some(window));
        window.refresh();
    });

    workspace._subscriptions = vec![
        git_subscription,
        pane_subscription,
        project_subscription,
        settings_subscription,
        appearance_subscription,
    ];
}

/// 把换行设置应用到所有打开的编辑器。
///
/// 预览 Item 经 act_as 暴露同一个源 Editor；按 EntityId 去重后更新。
fn apply_soft_wrap(
    pane: &Entity<Pane>,
    soft_wrap: SoftWrap,
    preferred_line_length: usize,
    cx: &mut App,
) {
    let mut updated = std::collections::HashSet::new();
    let mut editors = Vec::new();
    for item in pane.read(cx).tabs() {
        let Some(editor) = item.act_as::<Editor>(cx) else {
            continue;
        };
        if updated.insert(editor.entity_id()) {
            editors.push(editor);
        }
    }
    for editor in editors {
        editor.update(cx, |editor, cx| {
            editor.set_soft_wrap(soft_wrap, preferred_line_length, cx)
        });
    }
}

/// 把 GitStore 快照中的行级 diff hunks 推送给打开的 Editor。
///
/// 路径尚未查询（打开文件后首次、或全量扫描后）时按需发起后台查询，
/// 完成后经 GitStoreEvent::Statuses 回到本函数补齐（无死循环：查询完成即 Some）。
///
/// 不接收 Workspace 实体：订阅注册时的初始回调发生在 Workspace 更新期间，
/// 读取自身实体会触发 double-lease panic。
fn push_diff_hunks(pane: &Entity<Pane>, project: &Entity<Project>, cx: &mut App) {
    // 先收集打开的编辑器 (editor, path)，避免持 pane 借用时再可变借用 cx。
    let opened: Vec<(Entity<Editor>, PathBuf)> = pane
        .read(cx)
        .tabs()
        .iter()
        .filter_map(|item| {
            let editor = item.act_as::<Editor>(cx)?;
            let path = item.file_path(cx)?;
            Some((editor, path))
        })
        .collect();
    let mut missing = Vec::new();
    for (editor, path) in &opened {
        let store = project.read(cx).git_store();
        let Some(hunks) = store.read(cx).hunks_for_path(path) else {
            // 尚未查询：按需请求，事件回来再补。
            missing.push(path.clone());
            continue;
        };
        let hunks: Vec<zcv_git::DiffHunk> = hunks.to_vec();
        editor.update(cx, |editor, cx| editor.set_diff_hunks(hunks, cx));
    }
    if !missing.is_empty() {
        let store = project.read(cx).git_store();
        store.update(cx, |store, cx| store.request_hunks(&missing, cx));
    }
    // 预取 HEAD 文本：含 Deleted hunk 的文件展开删除块需要
    // （每路径每 HEAD 一次，缓存命中后不再重复加载；HEAD 变化时 git_store 自动清缓存）。
    for (editor, path) in &opened {
        let store = project.read(cx).git_store();
        // 删除块与修改块展开都需要 HEAD 文本（被删行/旧行来源）。
        let needs_head_text = store.read(cx).hunks_for_path(path).is_some_and(|hunks| {
            hunks.iter().any(|hunk| {
                matches!(
                    hunk.kind,
                    zcv_git::DiffHunkKind::Deleted | zcv_git::DiffHunkKind::Modified
                )
            })
        });
        if needs_head_text && store.read(cx).committed_text(path).is_none() {
            let task = store.read(cx).load_committed_text(path);
            let editor = editor.clone();
            let path = path.clone();
            let store = store.clone();
            cx.spawn(async move |cx| {
                if let Some(text) = task.await {
                    cx.update(|app| {
                        store.update(app, |store, _| {
                            store.cache_committed_text(&path, Arc::from(text));
                        });
                        editor.update(app, |editor, cx| {
                            editor.set_deleted_hunk_text(store.read(cx).committed_text(&path), cx);
                        });
                    })
                    .ok();
                }
            })
            .detach();
        }
    }
}
