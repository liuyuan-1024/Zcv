//! 装配层 —— 创建 Workspace，注入顶栏/面板/状态项，接线项目与设置订阅。
//!
//! Workspace 框架（Pane/Dock/命令分发）在 zcv-workspace；
//! 本模块只做 binary 侧的具体装配：面板（项目树/版本控制）、状态栏按钮、git/settings 订阅与 diff hunks 推送。

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use std::time::Duration;

use gpui::{
    App, AsyncApp, Context, Entity, FocusHandle, Focusable, Render, TitlebarOptions, WeakEntity,
    Window, WindowBounds, WindowOptions, div, point, prelude::*, px, size,
};
use zcv_actions::{
    DecreaseFontSize, DecreaseUiFontSize, GitFetch, GitPull, GitPush, IncreaseFontSize,
    IncreaseUiFontSize, NewTerminal, ResetFontSize, ResetUiFontSize, SelectGitBranch,
    ToggleHarnessMode, ToggleProjectPicker,
};
use zcv_editor::Editor;
use zcv_git::{DiffBase, GitRevision};
use zcv_project::{
    ActiveProjectRoot, DiffRequest, GitOperationKind, GitOperationOutcome, GitStoreEvent, Project,
};
use zcv_search::ProjectSearchButton;
use zcv_settings::SettingsStore;
use zcv_theme::{ThemeChoice, color, typography};
use zcv_workspace::{
    ActivityIndicator, Dock, DockPosition, FileToolbarControls, GitBranchAction, OnBranchSelected,
    OnProjectSelected, Pane, PaneEvent, Panel, PanelButtons, PanelEvent, PanelHandle, ToastAction,
    ToastKind, TopBar, Workspace, add_to_recent, load_window_bounds, save_window_bounds,
};

use crate::active_buffer_language::ActiveBufferLanguage;
use crate::breadcrumbs::Breadcrumbs;
use crate::cursor_position::CursorPosition;
use crate::harness::HarnessButton;
use crate::project_diff;
use crate::version_control::{OnOpenGitDiff, VersionControlPanel};
use zcv_project_tree::{OnCreate, OnMove, OnOpenFile, OnRename, OnTrash, ProjectTreePanel};
use zcv_terminal::TerminalPanel;

/// 构造打开文件回调（两个面板共用同一契约）。
fn on_open_file_callback(weak: &WeakEntity<Workspace>) -> OnOpenFile {
    let weak = weak.clone();
    Rc::new(
        move |path: PathBuf, focus_opened_item: bool, window: &mut Window, cx: &mut gpui::App| {
            if let Some(ws) = weak.upgrade() {
                ws.update(cx, |ws, cx| {
                    ws.open_path(path, focus_opened_item, window, cx);
                });
            }
        },
    )
}

/// 构造版本管理面板的项目差异回调。
fn on_open_git_diff_callback(weak: &WeakEntity<Workspace>) -> OnOpenGitDiff {
    let weak = weak.clone();
    Rc::new(
        move |kind,
              path: PathBuf,
              focus_opened_item: bool,
              window: &mut Window,
              cx: &mut gpui::App| {
            if let Some(workspace) = weak.upgrade() {
                workspace.update(cx, |workspace, cx| {
                    project_diff::deploy_at(workspace, kind, path, focus_opened_item, window, cx);
                });
            }
        },
    )
}

/// 以类型擦除句柄注册面板；同时让所属 dock 订阅面板事件（Dock 统一处理面板请求）。
fn register_panel<P: Panel>(
    workspace: &mut Workspace,
    entity: Entity<P>,
    position: DockPosition,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let dock = match position {
        DockPosition::Left => workspace.left_dock.clone(),
        DockPosition::Right => workspace.right_dock.clone(),
        DockPosition::Bottom => workspace.bottom_dock.clone(),
    };
    let workspace_for_events = cx.weak_entity();
    dock.update(cx, |dock, cx| {
        let subscription = cx.subscribe_in(
            &entity,
            window,
            move |dock, _, event: &PanelEvent, window, cx| {
                match event {
                    // 面板请求关闭（如终端最后一个会话关闭）时折叠 dock。
                    PanelEvent::Close => dock.set_open(false, window, cx),
                    // 面板自持状态变化（如终端会话增删）时保存布局。
                    PanelEvent::StateChanged => {
                        if let Some(workspace) = workspace_for_events.upgrade() {
                            workspace.update(cx, |workspace, cx| {
                                workspace.schedule_layout_save(window, cx);
                            });
                        }
                    }
                }
            },
        );
        dock.add_subscription(subscription);
    });
    let handle: Arc<dyn PanelHandle> = Arc::new(entity);
    workspace.register_panel(handle, position, window, cx);
}

/// 「切换项目」回调：在同一窗口内替换工作区根，窗口本体（尺寸/位置）保持不变。
fn switch_project_callback() -> OnProjectSelected {
    Rc::new(move |path, window, app| {
        let Ok(root) = canonical_project_root(PathBuf::from(&path)) else {
            eprintln!("打开项目失败（{path}）：路径无效");
            return; // 窗口保持原样。
        };
        add_to_recent(&root.to_string_lossy());
        // 先保存当前窗口边界（全局默认 + 旧项目记录）；随后窗口不重建，尺寸自然保持。
        save_window_bounds(window, app);
        // 旧工作区根即将被替换销毁，节流中的布局保存会随实体释放而丢失，先冲刷落盘。
        if let Some(Some(workspace)) = window.root::<Workspace>() {
            workspace.update(app, |workspace, cx| workspace.flush_layout(cx));
        }
        // 新项目根先于替换注册为全局显示基准（breadcrumbs 相对化查询按新根）。
        app.set_global(ActiveProjectRoot(Some(root.clone())));
        window.replace_root(app, |window, cx| build_workspace(&Some(root), window, cx));
    })
}

/// 规范化项目路径：相对路径（如 `zcv .`）归一为绝对路径，无效路径返回错误。
fn canonical_project_root(root: PathBuf) -> anyhow::Result<PathBuf> {
    root.canonicalize()
        .ok()
        .filter(|path| path.is_dir() && path.file_name().is_some())
        .ok_or_else(|| anyhow::anyhow!("项目路径不是有效目录：{}", root.display()))
}

/// 打开一个项目窗口（CLI 启动入口）。
pub(crate) fn open_project_window(root: PathBuf, cx: &mut App) -> anyhow::Result<()> {
    let root = canonical_project_root(root)?;
    add_to_recent(&root.to_string_lossy());
    open_workspace_window(Some(root), cx)
}

/// 打开不绑定任何目录的空工作区。
pub(crate) fn open_empty_workspace(cx: &mut App) -> anyhow::Result<()> {
    open_workspace_window(None, cx)
}

/// 项目与空工作区共用同一条窗口创建路径；差异只在 Project 是否含 worktree。
fn open_workspace_window(root: Option<PathBuf>, cx: &mut App) -> anyhow::Result<()> {
    // 项目根作为全局显示基准注册（breadcrumbs 相对化查询；RootChanged 时更新）。
    cx.set_global(ActiveProjectRoot(root.clone()));
    // 窗口边界恢复：项目记录 → 全局默认 → 初始居中。
    let (window_bounds, display_id) =
        load_window_bounds(root.as_deref(), cx).unwrap_or_else(|| {
            (
                WindowBounds::centered(size(px(1200.0), px(900.0)), cx),
                None,
            )
        });

    cx.open_window(
        WindowOptions {
            window_bounds: Some(window_bounds),
            display_id,
            titlebar: Some(TitlebarOptions {
                title: Some("".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(-100.0), px(-100.0))),
            }),
            ..Default::default()
        },
        |window, cx| cx.new(|cx| build_workspace(&root, window, cx)),
    )?;
    Ok(())
}

/// 在给定窗口内创建并装配工作区；窗口创建与「切换项目」的根替换共用（须在 cx.new 闭包内调用）。
fn build_workspace(
    root: &Option<PathBuf>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Workspace {
    apply_theme(&SettingsStore::get(cx).theme, cx, Some(window));
    // 全局字号经 window rem 基准设置：
    // 字体与行高仍在元素上显式设置（见 Workspace 根元素与 tooltip）。
    window.set_rem_size(typography::ui());
    let mut workspace = match root {
        Some(root) => Workspace::new(root.clone(), window, cx),
        None => Workspace::new_empty(window, cx),
    };
    // 装配不区分空/项目工作区：面板无条件注册，空态由各面板自行渲染。
    initialize_workspace(&mut workspace, window, cx);
    // 焦点延后到首帧渲染完成后：track_focus 元素未挂载前 focus 会静默丢失，导致启动后 keymap dispatch 无焦点链，快捷键不生效，直到用户点击界面（焦点链建立）才恢复。
    let focus = workspace.focus.clone();
    window.defer(cx, move |window, _cx| {
        window.focus(&focus);
    });
    workspace
}

/// 所有工作区共享的面板、状态栏和编辑器工具栏。
///
/// 这些 UI 不以 worktree 是否存在为条件；各状态项在没有活动编辑器时自行显示空态。
fn initialize_common_workspace(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let outline = cx.new(OutlinePanel::new);
    let terminal = cx.new(TerminalPanel::new);

    let terminal_for_new = terminal.clone();
    register_panel(workspace, outline, DockPosition::Left, window, cx);
    register_panel(workspace, terminal, DockPosition::Bottom, window, cx);

    // 新建终端：先创建再确保面板可见，避免面板激活时的懒创建重复生成终端。
    workspace.register_action(move |workspace, _: &NewTerminal, window, cx| {
        terminal_for_new.update(cx, |panel, cx| {
            panel.new_terminal(window, cx);
        });
        let bottom_dock = workspace.bottom_dock.clone();
        bottom_dock.update(cx, |dock, cx| {
            let Some(index) = dock.panel_index_for_persistent_name("terminal") else {
                return;
            };
            if !dock.is_panel_active(index) {
                dock.activate_panel(index, window, cx);
                dock.set_open(true, window, cx);
            }
        });
    });

    zcv_search::install(workspace, window, cx);

    let status_bar = workspace.status_bar().clone();
    let left_dock = workspace.left_dock.clone();
    let bottom_dock = workspace.bottom_dock.clone();
    let workspace_entity = cx.weak_entity();
    status_bar.update(cx, |bar, cx| {
        bar.add_left_item(
            cx.new(|cx| PanelButtons::new(left_dock.clone(), workspace_entity.clone(), cx)),
            cx,
        );
        bar.add_left_item(cx.new(|_| ProjectSearchButton::new()), cx);
        bar.add_right_item(cx.new(|_| CursorPosition::new()), cx);
        bar.add_right_item(cx.new(|_| ActiveBufferLanguage::new()), cx);
        bar.add_right_item(
            cx.new(|cx| PanelButtons::new(bottom_dock.clone(), workspace_entity.clone(), cx)),
            cx,
        );
        let harness_button = cx.new(|_| HarnessButton::new());
        bar.add_right_item(harness_button.clone(), cx);
        workspace.register_action(move |_workspace, _: &ToggleHarnessMode, _window, cx| {
            harness_button.update(cx, |button, cx| button.toggle(cx));
        });
    });

    // 编辑器字号缩放（会话内生效，不写配置文件）。
    // 字号是 typography 的运行时状态：直接调整并强制重绘，不改 SettingsStore。
    workspace.register_action(move |_workspace, _: &IncreaseFontSize, window, _cx| {
        let editor = f32::from(typography::editor());
        typography::set_typography(Some(editor + 1.), None, None);
        window.refresh();
    });
    workspace.register_action(move |_workspace, _: &DecreaseFontSize, window, _cx| {
        let editor = f32::from(typography::editor());
        typography::set_typography(Some((editor - 1.).max(8.)), None, None);
        window.refresh();
    });
    workspace.register_action(move |_workspace, _: &ResetFontSize, window, cx| {
        let settings = SettingsStore::get(cx);
        typography::set_typography(Some(settings.font_size), None, None);
        window.refresh();
    });

    // UI 字号缩放（全局可用，会话内生效）。
    // UI 字号缩放（cmd-shift-= 等，全局可用，会话内生效）：只调 UI 字号，编辑器不动。
    // UI 字号是窗口 rem 基准：字号变化必须同步更新rem_size，否则基于 rem 的文本/布局沿用旧基准，与放大后的字形错位导致截断。
    workspace.register_action(move |_workspace, _: &IncreaseUiFontSize, window, _cx| {
        let ui = f32::from(typography::ui());
        typography::set_typography(None, Some(ui + 1.), None);
        window.set_rem_size(typography::ui());
        window.refresh();
    });
    workspace.register_action(move |_workspace, _: &DecreaseUiFontSize, window, _cx| {
        let ui = f32::from(typography::ui());
        typography::set_typography(None, Some((ui - 1.).max(8.)), None);
        window.set_rem_size(typography::ui());
        window.refresh();
    });
    workspace.register_action(move |_workspace, _: &ResetUiFontSize, window, cx| {
        let settings = SettingsStore::get(cx);
        typography::set_typography(None, Some(settings.ui_font_size), None);
        window.set_rem_size(typography::ui());
        window.refresh();
    });

    let pane = workspace.pane().clone();
    pane.update(cx, |pane, cx| {
        let toolbar = pane.toolbar().clone();
        toolbar.update(cx, |toolbar, cx| {
            toolbar.add_item(cx.new(|_| Breadcrumbs::new()), window, cx);
            toolbar.add_item(cx.new(|_| FileToolbarControls::new()), window, cx);
        });
    });

    for dock in [
        workspace.left_dock.clone(),
        workspace.right_dock.clone(),
        workspace.bottom_dock.clone(),
    ] {
        dock.update(cx, |dock: &mut Dock, cx: &mut Context<Dock>| {
            let focus = dock.focus_handle(cx);
            let sub = cx.on_focus(
                &focus,
                window,
                |dock: &mut Dock, window: &mut Window, cx: &mut Context<Dock>| {
                    if let Some(panel) = dock.visible_panel() {
                        window.focus(&panel.focus_handle(cx));
                    }
                },
            );
            dock.add_subscription(sub);
        });
    }
}

/// 装配 Workspace：顶栏注入、面板/状态项注册、订阅接线。
///
/// 必须在 `Workspace::update` 闭包内调用（workspace 为 &mut），内部不得再对同一实体嵌套 update。
/// 所有工作区（含无 worktree 的空工作区）走同一条装配路径。
/// 后台执行 git 操作（fetch/pull/push）：等待结果后直接弹提示（成功/失败+错误详情）。
///
/// 命令编排与结果文案属于产品层，框架 workspace 不解释 git 领域语义，因此这里在装配层统一注册。
fn run_git_operation(
    workspace: &mut Workspace,
    operation: GitOperationKind,
    cx: &mut Context<Workspace>,
) {
    let Some(git_store) = workspace.project().read(cx).try_git_store() else {
        return;
    };
    let task = git_store.update(cx, |store, cx| store.run_operation(operation, cx));
    let name = match operation {
        GitOperationKind::Fetch => "拉取",
        GitOperationKind::Pull => "合并拉取",
        GitOperationKind::Push => "推送",
    };
    cx.spawn(move |this: WeakEntity<Workspace>, asynccx: &mut AsyncApp| {
        let mut cx = asynccx.clone();
        async move {
            let result = task.await;
            let failure = match &result {
                Ok(GitOperationOutcome::Failed(error)) => Some(error.clone()),
                Err(error) => Some(format!("{error:#}")),
                _ => None,
            };
            let (kind, message, action) = if let Some(error) = failure {
                // 失败提示带重试按钮：点击重新执行同一操作（弱引用，不持有 Workspace）。
                let weak = this.clone();
                (
                    ToastKind::Error,
                    format!("{name}失败：{error}"),
                    Some(ToastAction::new("重试", move |_window, cx| {
                        if let Some(workspace) = weak.upgrade() {
                            // App 上下文的 Entity::update 直接返回闭包结果（实体经 upgrade 已确认存在），无 Result 包装。
                            workspace.update(cx, |workspace, cx| {
                                run_git_operation(workspace, operation, cx);
                            });
                        }
                    })),
                )
            } else {
                match result.expect("失败分支已在上方处理") {
                    GitOperationOutcome::Completed => {
                        (ToastKind::Success, format!("{name}完成"), None)
                    }
                    GitOperationOutcome::Cancelled => {
                        (ToastKind::Info, format!("{name}已取消"), None)
                    }
                    GitOperationOutcome::CompletedBeforeCancellation => {
                        (ToastKind::Success, format!("{name}已在取消前完成"), None)
                    }
                    GitOperationOutcome::CancellationUnconfirmed(detail) => (
                        ToastKind::Error,
                        format!("{name}已停止，但暂时无法确认远端状态：{detail}"),
                        None,
                    ),
                    GitOperationOutcome::Failed(_) => unreachable!(),
                }
            };
            if let Some(this) = this.upgrade() {
                this.update(&mut cx, |workspace, cx| {
                    workspace.show_toast(kind, message, action, Some(Duration::from_secs(5)), cx);
                })
                .ok();
            }
        }
    })
    .detach();
}

fn initialize_workspace(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    // ═══ 顶栏注入 ═══════════════════════════════════════════════════

    let weak_self: gpui::WeakEntity<Workspace> = cx.weak_entity();
    let weak_branch = weak_self.clone();
    let on_branch: OnBranchSelected = Rc::new(move |action, _window, app| {
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

    let top_bar = cx.new(|cx| TopBar::new(switch_project_callback(), on_branch, window, cx));
    if let Some(root) = workspace.project().read(cx).root() {
        let label = root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        top_bar.update(cx, |bar, cx| {
            bar.project_picker.update(cx, |picker, _| {
                picker.set_current_label(label);
            });
        });
    }
    workspace.set_titlebar(top_bar.clone().into(), cx);
    // TopBar 组件不在主焦点链上：把选择器的命令 handler 注册到 Workspace 根节点，全局可达。
    let project_picker = top_bar.read(cx).project_picker.clone();
    workspace.register_action(move |_workspace, _: &ToggleProjectPicker, window, cx| {
        project_picker.update(cx, |picker, cx| picker.toggle(window, cx));
    });
    let branch_picker = top_bar.read(cx).branch_picker.clone();
    workspace.register_action(move |_workspace, _: &SelectGitBranch, window, cx| {
        branch_picker.update(cx, |picker, cx| picker.toggle(window, cx));
    });
    workspace.set_open_settings_provider(Box::new(|_cx| {
        zcv_settings::ensure_user_settings_file()
            .ok()
            .map(|path| path.to_path_buf())
    }));

    // ═══ 面板创建与注册 ═══════════════════════════════════════════

    let project = workspace.project().clone();

    let project_tree: Entity<ProjectTreePanel> = cx.new(|cx| {
        let mut tree = ProjectTreePanel::new(project.clone(), cx);
        tree.set_on_open_file(on_open_file_callback(&weak_self));
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
        let weak_move = weak_self.clone();
        let on_move: OnMove = Rc::new(move |from, to, overwrite, cx| {
            let Some(workspace) = weak_move.upgrade() else {
                anyhow::bail!("工作区已关闭");
            };
            workspace.update(cx, |workspace, cx| {
                workspace.move_path(&from, &to, overwrite, cx)
            })
        });
        tree.set_on_move(on_move);
        tree
    });

    let version_control: Entity<VersionControlPanel> = cx.new(|cx| {
        let mut panel = VersionControlPanel::new(project.clone(), cx);
        panel.set_on_open_file(on_open_git_diff_callback(&weak_self));
        panel
    });

    register_panel(
        workspace,
        project_tree.clone(),
        DockPosition::Left,
        window,
        cx,
    );
    register_panel(workspace, version_control, DockPosition::Left, window, cx);
    initialize_common_workspace(workspace, window, cx);

    // ═══ 状态栏注册 ═══════════════════════════════════════════════

    let status_bar = workspace.status_bar().clone();
    status_bar.update(cx, |bar, cx| {
        bar.add_left_item(
            cx.new(|cx| ActivityIndicator::new(project.read(cx).git_store(), cx)),
            cx,
        );
    });

    // ═══ 订阅接线 ═════════════════════════════════════════════════

    let pane = workspace.pane().clone();

    let git_store = project.read(cx).git_store();
    let git_subscription = cx.subscribe(&git_store, move |workspace, store, event, cx| {
        let branch = store.read(cx).current_branch().map(str::to_string);
        let head_commit = store.read(cx).current_head_commit().map(str::to_string);
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
            bar.set_head_commit(head_commit, cx);
            bar.set_branches(branch_list, cx);
            bar.set_has_repositories(has_repositories);
            bar.set_remote_operation_state(remote_operation_state);
            cx.notify();
        });
        // 任务事件只更新任务界面，不能反向触发差异业务；其余状态事件同步当前结果。
        if matches!(
            event,
            GitStoreEvent::Repositories
                | GitStoreEvent::Statuses
                | GitStoreEvent::HunksChanged
                | GitStoreEvent::Head
        ) {
            push_diff_hunks(workspace.pane(), workspace.project(), cx);
        }
    });

    let project_tree_for_pane = project_tree.clone();
    let pane_subscription = cx.subscribe(&pane, move |workspace, pane, event, cx| {
        if matches!(
            event,
            PaneEvent::ActivateItem { .. } | PaneEvent::RemovedItem { .. }
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
    });

    // 项目事件订阅：根重命名与文件树变化驱动项目树刷新。
    let project_tree_for_project = project_tree.clone();
    let project_subscription =
        cx.subscribe(
            &project,
            move |_workspace, _project, event, cx| match event {
                zcv_project::ProjectEvent::RootChanged(root) => {
                    cx.set_global(ActiveProjectRoot(Some(root.clone())));
                    project_tree_for_project.update(cx, |tree, cx| {
                        tree.set_root(root.clone(), cx);
                    });
                }
                zcv_project::ProjectEvent::EntriesChanged => {
                    project_tree_for_project.update(cx, |tree, cx| tree.schedule_refresh(cx));
                }
            },
        );

    let project_tree_for_settings = project_tree.clone();
    let settings_subscription =
        cx.observe_global_in::<SettingsStore>(window, move |_workspace, window, cx| {
            let settings = SettingsStore::get(cx);
            zcv_theme::typography::set_typography(
                Some(settings.font_size),
                Some(settings.ui_font_size),
                Some(settings.line_height),
            );
            apply_theme(&settings.theme, cx, Some(window));
            project_tree_for_settings.update(cx, |tree, cx| tree.refresh(cx));
            cx.notify();
        });

    let appearance_subscription = window.observe_window_appearance(|window, cx| {
        let settings = SettingsStore::get(cx);
        apply_theme(&settings.theme, cx, Some(window));
        window.refresh();
    });

    // git 操作（fetch/pull/push）：编排与文案在装配层。
    workspace.register_action(move |workspace, _: &GitFetch, _window, cx| {
        run_git_operation(workspace, GitOperationKind::Fetch, cx);
    });
    workspace.register_action(move |workspace, _: &GitPull, _window, cx| {
        run_git_operation(workspace, GitOperationKind::Pull, cx);
    });
    workspace.register_action(move |workspace, _: &GitPush, _window, cx| {
        run_git_operation(workspace, GitOperationKind::Push, cx);
    });

    for subscription in [
        git_subscription,
        pane_subscription,
        project_subscription,
        settings_subscription,
        appearance_subscription,
    ] {
        workspace.add_subscription(subscription);
    }
}

/// 将设置层的文本主题 id 解析并应用为主题运行时状态。
fn apply_theme(theme: &str, cx: &mut App, window: Option<&Window>) {
    ThemeChoice::from_config(theme).apply(cx, window);
}

/// 把 GitStore 快照中的行级 diff hunks 推送给打开的 Editor。
///
/// 打开文件集合是差异协调器的唯一需求来源；本函数同时把当前终态结果推送给编辑器。
///
/// 不接收 Workspace 实体：订阅注册时的初始回调发生在 Workspace 更新期间，
/// 读取自身实体会触发 double-lease panic。
fn push_diff_hunks(pane: &Entity<Pane>, project: &Entity<Project>, cx: &mut App) {
    // 所有打开 Item 的真实源文件共同构成差异需求；
    // 组合文档不能再被单个 item_path 隐式代表，否则项目差异 Item 只会请求当前文件而丢失其余来源。
    let mut interested_requests: Vec<DiffRequest> = pane
        .read(cx)
        .tabs()
        .iter()
        .filter(|item| item.act_as::<project_diff::ProjectDiffView>(cx).is_none())
        .filter_map(|item| item.multi_buffer(cx))
        .flat_map(|multi_buffer| {
            multi_buffer
                .read(cx)
                .file_buffers(cx)
                .into_iter()
                .map(|(_, path)| DiffRequest::new(DiffBase::Head, path))
        })
        .collect();
    // 项目差异在 hunk 首次加载完成前还没有 excerpts，不能从空 MultiBuffer 反推需求；
    // 直接读取该 Item 由 Git 状态派生的路径集合，确保等待态不会被精确需求刷新清掉。
    interested_requests.extend(
        pane.read(cx)
            .tabs()
            .iter()
            .filter_map(|item| item.act_as::<project_diff::ProjectDiffView>(cx))
            .flat_map(|view| view.read(cx).diff_requests().collect::<Vec<_>>()),
    );
    // 单文件 Editor 仍直接消费自身 hunk；组合 Item 负责把各源 hunk 投影到组合坐标。
    let opened: Vec<(Entity<Editor>, PathBuf)> = pane
        .read(cx)
        .tabs()
        .iter()
        .filter_map(|item| {
            if item.act_as::<project_diff::ProjectDiffView>(cx).is_some() {
                return None;
            }
            let editor = item.act_as::<Editor>(cx)?;
            let path = item.item_path(cx)?;
            Some((editor, path))
        })
        .collect();
    let store = project.read(cx).git_store();
    store.update(cx, |store, cx| {
        store.set_hunk_interests(&interested_requests, cx)
    });
    for (editor, path) in &opened {
        let store = project.read(cx).git_store();
        // 等待态先清空旧标记；结果到达后由 HunksChanged 精确补回。
        let hunks: Vec<zcv_git::DiffHunk> = store
            .read(cx)
            .hunks_for_path(DiffBase::Head, path)
            .map(|hunks| hunks.to_vec())
            .unwrap_or_default();
        editor.update(cx, |editor, cx| editor.set_diff_hunks(hunks, cx));
    }
    // 预取 HEAD 文本：含 Deleted hunk 的文件展开删除块需要。
    // 每路径每 HEAD 一次，缓存命中后不再重复加载；HEAD 变化时 git_store 自动清缓存。
    for (editor, path) in &opened {
        let store = project.read(cx).git_store();
        // 删除块与修改块展开都需要 HEAD 文本（被删行/旧行来源）。
        let needs_head_text = store
            .read(cx)
            .hunks_for_path(DiffBase::Head, path)
            .is_some_and(|hunks| {
                hunks.iter().any(|hunk| {
                    matches!(
                        hunk.kind,
                        zcv_git::DiffHunkKind::Deleted | zcv_git::DiffHunkKind::Modified
                    )
                })
            });
        if !needs_head_text {
            continue;
        }
        if let Some(text) = store.read(cx).revision_text(GitRevision::Head, path) {
            editor.update(cx, |editor, cx| {
                editor.set_deleted_hunk_text(Some(text), cx)
            });
        } else {
            // 加载结果由 GitStore 自行回填缓存，这里只消费返回值。
            let task = store
                .read(cx)
                .load_revision_text(GitRevision::Head, path, cx);
            let editor = editor.clone();
            cx.spawn(async move |cx| {
                if let Some(text) = task.await {
                    cx.update(|app| {
                        editor.update(app, |editor, cx| {
                            editor.set_deleted_hunk_text(Some(Arc::from(text)), cx);
                        });
                    })
                    .ok();
                }
            })
            .detach();
        }
    }
}

// ── 内部类型 ────────────────────────────────────────────────────────

/// 占位面板：大纲/调试（后续接入真实功能）。
macro_rules! make_placeholder_panel {
    ($name:ident, $persistent:expr, $icon:expr, $label:expr) => {
        struct $name {
            focus: FocusHandle,
        }

        impl $name {
            fn new(cx: &mut Context<Self>) -> Self {
                Self {
                    focus: cx.focus_handle(),
                }
            }
        }

        impl gpui::EventEmitter<zcv_workspace::PanelEvent> for $name {}

        impl Panel for $name {
            fn icon() -> &'static str {
                $icon
            }
            fn label() -> &'static str {
                $label
            }
            fn persistent_name() -> &'static str {
                $persistent
            }
            fn focus_handle(&self, _cx: &App) -> FocusHandle {
                self.focus.clone()
            }
        }

        impl Render for $name {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .track_focus(&self.focus)
                    .key_context($persistent)
                    .tab_index(0)
                    .text_color(color::current(cx).text_placeholder)
                    .child($label)
            }
        }
    };
}

make_placeholder_panel!(OutlinePanel, "outline", "icons/list_tree.svg", "大纲");

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;

    use gpui::{AppContext, TestAppContext};
    use zcv_workspace::Item as _;

    use super::{Workspace, build_workspace};
    use crate::project_diff::{self, ProjectDiffView};

    /// 空工作区与项目工作区走同一条装配路径：全部面板无条件注册，空态由面板自行渲染。
    #[gpui::test]
    fn empty_workspace_installs_all_panels(cx: &mut TestAppContext) {
        cx.update(|cx| {
            zcv_settings::init(cx);
            zcv_editor::init(cx);
        });
        let (workspace, cx) = cx.add_window_view(|window, cx| build_workspace(&None, window, cx));

        cx.read_entity(&workspace, |workspace, cx| {
            assert_eq!(workspace.left_dock.read(cx).panel_count(), 3);
            assert_eq!(workspace.bottom_dock.read(cx).panel_count(), 1);
            // 右 dock 当前无面板：原快捷键面板已由 harness 状态标记按钮取代。
            assert_eq!(workspace.right_dock.read(cx).panel_count(), 0);
        });
    }

    /// 切换项目在同一窗口内替换工作区根：窗口本体不变，根实体换新。
    #[gpui::test]
    fn switching_replaces_root_in_same_window(cx: &mut TestAppContext) {
        cx.update(|cx| {
            zcv_settings::init(cx);
            zcv_editor::init(cx);
        });
        let (old_workspace, cx) =
            cx.add_window_view(|window, cx| build_workspace(&None, window, cx));
        let old_id = old_workspace.entity_id();

        cx.update(|window, app| {
            window.replace_root(app, |window, cx| build_workspace(&None, window, cx));
        });

        // 新根已就位，且不是旧工作区实体。
        cx.update(|window, _| {
            let new_root = window.root::<Workspace>().flatten().expect("新根应已就位");
            assert_ne!(new_root.entity_id(), old_id);
        });
    }

    #[gpui::test]
    fn project_diff_keeps_hunk_interest_while_its_multibuffer_is_empty(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时仓库");
        let root = directory.path().canonicalize().expect("应规范化仓库路径");
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

        cx.update(|cx| {
            zcv_settings::init(cx);
            zcv_editor::init(cx);
        });
        let project_root = Some(root);
        let (workspace, cx) =
            cx.add_window_view(|window, cx| build_workspace(&project_root, window, cx));
        cx.run_until_parked();

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| {
                project_diff::deploy_at(
                    workspace,
                    project_diff::ProjectDiffKind::Unstaged,
                    path.clone(),
                    false,
                    window,
                    cx,
                )
            });
        });
        cx.run_until_parked();
        cx.run_until_parked();

        let view = cx.read_entity(&workspace, |workspace, cx| {
            workspace
                .pane()
                .read(cx)
                .tabs()
                .iter()
                .find_map(|item| item.act_as::<ProjectDiffView>(cx))
                .expect("应打开项目差异视图")
        });
        cx.read_entity(&view, |view, cx| {
            let multi_buffer = view.multi_buffer(cx).expect("项目差异应提供组合文档");
            let text = String::from_utf8(multi_buffer.read(cx).snapshot(cx).text_bytes())
                .expect("投影文本应为 UTF-8");
            assert_eq!(text, "line1\nline2\n原内容\n新内容\nline4\nline5\n");
        });
    }

    fn run_in(directory: &Path, arguments: &[&str]) {
        let output = Command::new(arguments[0])
            .args(&arguments[1..])
            .current_dir(directory)
            .output()
            .expect("应执行 Git 命令");
        assert!(
            output.status.success(),
            "命令 {arguments:?} 失败：{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
