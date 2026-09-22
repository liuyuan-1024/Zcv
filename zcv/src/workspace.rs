//! 装配层 —— 创建 Workspace，注入顶栏/面板/状态项，接线项目与设置订阅。
//!
//! 工作区（Pane/Dock/命令分发）在 zcv-workspace；
//! 本模块只做 binary 侧的具体装配：面板（项目树/版本控制）、状态栏按钮、git/settings 订阅与 diff hunks 推送。

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use gpui::{
    App, AsyncApp, Context, Entity, Focusable, PromptLevel, TitlebarOptions, WeakEntity, Window,
    WindowBounds, WindowOptions, point, prelude::*, px, size,
};
use zcv_actions::{
    DecreaseContentFontSize, DecreaseUiFontSize, GitFetch, GitPull, GitPush,
    IncreaseContentFontSize, IncreaseUiFontSize, NewTerminal, ResetContentFontSize,
    ResetUiFontSize, RestartToUpdate, SelectGitBranch, ToggleHarnessMode, ToggleProjectPicker,
};
use zcv_editor::{Editor, EditorEvent};
use zcv_language::LanguageRegistry;
use zcv_project::{
    FileWatcherError, FileWatcherOperation, GitOperationKind, GitOperationOutcome, GitStoreEvent,
    ProjectEvent,
};
use zcv_settings::{GlobalSettingsErrorReporter, SettingsStore};
use zcv_theme::{ThemeChoice, typography};
use zcv_workspace::{
    ActivityIndicator, Dock, DockPosition, GitBranchAction, OnBranchSelected, OnProjectSelected,
    PaneEvent, PanelButtons, PreviewToolbar, ToastAction, ToastKind, TopBar, TopBarCallbacks,
    Workspace, add_to_recent, load_window_bounds, save_window_bounds,
};

use crate::auto_update::{UpdateButton, UpdateManager};
use crate::harness::HarnessButton;
use zcv_outline::OutlinePanel;
use zcv_path::AbsolutePathBuf;
use zcv_project_tree::{OnCreate, OnMove, OnOpenFile, OnRename, OnTrash, ProjectTreePanel};
use zcv_terminal::TerminalPanel;
use zcv_version_control::{
    OnOpenGitDiff, OnOpenGitGraph, VersionControlPanel, deploy_git_graph, deploy_project_diff,
    install as install_version_control, refresh_pane_git_projection,
};

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
                    deploy_project_diff(workspace, kind, path, focus_opened_item, window, cx);
                });
            }
        },
    )
}

/// 构造版本管理面板打开版本控制图的回调。
fn on_open_git_graph_callback(weak: &WeakEntity<Workspace>) -> OnOpenGitGraph {
    let weak = weak.clone();
    Rc::new(move |window: &mut Window, cx: &mut gpui::App| {
        if let Some(workspace) = weak.upgrade() {
            workspace.update(cx, |workspace, cx| {
                deploy_git_graph(workspace, window, cx);
            });
        }
    })
}

/// 「切换项目」回调：在同一窗口内替换工作区根，窗口本体（尺寸/位置）保持不变。
///
/// 替换后的工作区必须继续使用应用级语言注册表，因此回调捕获同一份 Arc。
fn switch_project_callback(languages: Arc<LanguageRegistry>) -> OnProjectSelected {
    Rc::new(move |path, window, app| {
        let Ok(root) = canonical_project_root(PathBuf::from(&path)) else {
            if let Some(workspace) = window.root::<Workspace>().flatten() {
                workspace.update(app, |workspace, cx| {
                    workspace.show_toast(
                        ToastKind::Error,
                        format!("打开项目失败（{path}）：路径无效"),
                        None,
                        Some(Duration::from_secs(5)),
                        cx,
                    );
                });
            }
            return; // 窗口保持原样。
        };
        let recent_error = add_to_recent(&root.to_string_lossy())
            .err()
            .map(|error| format!("更新最近项目列表失败：{error:#}"));
        // 先保存当前窗口边界（全局默认 + 旧项目记录）；随后窗口不重建，尺寸自然保持。
        let current_root = window.root::<Workspace>().flatten().and_then(|workspace| {
            workspace
                .read(app)
                .project()
                .read(app)
                .root()
                .map(Path::to_path_buf)
        });
        save_window_bounds(current_root.as_deref(), window, app);
        // 旧工作区根即将被替换销毁，节流中的布局保存会随实体释放而丢失，先冲刷落盘。
        if let Some(Some(workspace)) = window.root::<Workspace>() {
            workspace.update(app, |workspace, cx| workspace.flush_layout(cx));
        }
        let languages = Arc::clone(&languages);
        window.replace_root(app, move |window, cx| {
            let workspace = build_workspace(&Some(root), languages, window, cx);
            if let Some(message) = recent_error {
                workspace.show_toast(
                    ToastKind::Error,
                    message,
                    None,
                    Some(Duration::from_secs(5)),
                    cx,
                );
            }
            workspace
        });
    })
}

/// 规范化项目路径：相对路径（如 `zcv .`）归一为绝对路径，无效路径返回错误。
fn canonical_project_root(root: PathBuf) -> anyhow::Result<PathBuf> {
    let root = AbsolutePathBuf::canonicalize(&root)
        .with_context(|| format!("无法规范化项目路径：{}", root.display()))?
        .into_path_buf();
    if root.is_dir() && root.file_name().is_some() {
        Ok(root)
    } else {
        Err(anyhow::anyhow!("项目路径不是有效目录：{}", root.display()))
    }
}

/// 打开一个项目窗口（CLI 启动入口）。
pub(crate) fn open_project_window(
    root: PathBuf,
    languages: Arc<LanguageRegistry>,
    cx: &mut App,
) -> anyhow::Result<()> {
    let root = canonical_project_root(root)?;
    let startup_error = add_to_recent(&root.to_string_lossy())
        .err()
        .map(|error| format!("更新最近项目列表失败：{error:#}"));
    open_workspace_window(Some(root), languages, startup_error, cx)
}

/// 打开不绑定任何目录的空工作区。
pub(crate) fn open_empty_workspace(
    languages: Arc<LanguageRegistry>,
    cx: &mut App,
) -> anyhow::Result<()> {
    open_workspace_window(None, languages, None, cx)
}

pub(crate) fn open_empty_workspace_with_error(
    message: String,
    languages: Arc<LanguageRegistry>,
    cx: &mut App,
) -> anyhow::Result<()> {
    open_workspace_window(None, languages, Some(message), cx)
}

/// 项目与空工作区共用同一条窗口创建路径；差异只在 Project 是否含 worktree。
fn open_workspace_window(
    root: Option<PathBuf>,
    languages: Arc<LanguageRegistry>,
    startup_error: Option<String>,
    cx: &mut App,
) -> anyhow::Result<()> {
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
        |window, cx| {
            cx.new(|cx| {
                let workspace = build_workspace(&root, languages, window, cx);
                if let Some(message) = startup_error {
                    workspace.show_toast(
                        ToastKind::Error,
                        message,
                        None,
                        Some(Duration::from_secs(8)),
                        cx,
                    );
                }
                workspace
            })
        },
    )?;
    Ok(())
}

/// 在给定窗口内创建并装配工作区；窗口创建与「切换项目」的根替换共用（须在 cx.new 闭包内调用）。
fn build_workspace(
    root: &Option<PathBuf>,
    languages: Arc<LanguageRegistry>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Workspace {
    let workspace = match root {
        Some(root) => Workspace::new(root.clone(), Arc::clone(&languages), window, cx),
        None => Workspace::new_empty(Arc::clone(&languages), window, cx),
    };
    finish_build_workspace(workspace, languages, window, cx)
}

/// 将工作区状态装配为应用界面。
fn finish_build_workspace(
    mut workspace: Workspace,
    languages: Arc<LanguageRegistry>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Workspace {
    apply_theme(&SettingsStore::get(cx).theme, cx, Some(window));
    // UI 字号是当前工作区的窗口基准；临时缩放不会影响其他窗口。
    window.set_rem_size(workspace.typography().ui_size());
    // 装配不区分空/项目工作区：面板无条件注册，空态由各面板自行渲染。
    initialize_workspace(&mut workspace, languages, window, cx);
    // 焦点延后到首帧渲染完成后：track_focus 元素未挂载前 focus 会静默丢失，导致启动后 keymap dispatch 无焦点链，快捷键不生效，直到用户点击界面（焦点链建立）才恢复。
    let focus = workspace.focus_handle().clone();
    window.defer(cx, move |window, cx| {
        window.focus(&focus, cx);
    });
    workspace
}

/// 所有工作区共享的面板、状态栏和编辑器内容装配。
///
/// 这些 UI 不以 worktree 是否存在为条件；各状态项在没有活动编辑器时自行显示空态。
fn initialize_common_workspace(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let outline = cx.new(|cx| {
        OutlinePanel::new(
            workspace.pane().clone(),
            workspace.project().read(cx).language_registry(),
            cx,
        )
    });
    let terminal_project = workspace.project().clone();
    let terminal = cx.new(|cx| TerminalPanel::new(terminal_project, cx));

    let terminal_for_new = terminal.clone();
    workspace.register_panel(outline.clone(), DockPosition::Left, window, cx);
    workspace.register_panel(terminal, DockPosition::Bottom, window, cx);

    // 新建终端：先创建再确保面板可见，避免面板激活时的懒创建重复生成终端。
    workspace.register_action(move |workspace, _: &NewTerminal, window, cx| {
        terminal_for_new.update(cx, |panel, cx| {
            panel.new_terminal(window, cx);
        });
        let bottom_dock = workspace.dock(DockPosition::Bottom).clone();
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

    workspace.register_action(|workspace, _: &RestartToUpdate, window, cx| {
        let has_unsaved_items = workspace
            .pane()
            .read(cx)
            .tabs()
            .iter()
            .any(|item| item.is_dirty(cx));
        if has_unsaved_items {
            drop(window.prompt(
                PromptLevel::Warning,
                "存在未保存的文件",
                Some("请先保存或关闭未保存的文件，再重启完成更新。"),
                &["知道了"],
                cx,
            ));
            return;
        }

        let result = UpdateManager::get(cx)
            .context("自动更新管理器未初始化")
            .and_then(|manager| manager.update(cx, |manager, _| manager.launch_helper()));
        if let Err(error) = result {
            workspace.show_toast(
                ToastKind::Error,
                format!("无法开始更新：{error:#}"),
                None,
                Some(Duration::from_secs(8)),
                cx,
            );
            return;
        }
        let root = workspace.project().read(cx).root().map(Path::to_path_buf);
        save_window_bounds(root.as_deref(), window, cx);
        workspace.flush_layout(cx);
        cx.quit();
    });

    let status_bar = workspace.status_bar().clone();
    let left_dock = workspace.dock(DockPosition::Left).clone();
    let bottom_dock = workspace.dock(DockPosition::Bottom).clone();
    let workspace_entity = cx.weak_entity();
    zcv_editor::install_status_items(workspace, cx);
    status_bar.update(cx, |bar, cx| {
        bar.add_left_item(
            cx.new(|cx| PanelButtons::new(left_dock.clone(), workspace_entity.clone(), cx)),
            cx,
        );
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

    // 内容字号缩放（当前工作区内生效，不写配置文件）。
    workspace.register_action(|workspace, _: &IncreaseContentFontSize, window, cx| {
        workspace.increase_content_font_size(1., cx);
        window.refresh();
    });
    workspace.register_action(|workspace, _: &DecreaseContentFontSize, window, cx| {
        workspace.increase_content_font_size(-1., cx);
        window.refresh();
    });
    workspace.register_action(|workspace, _: &ResetContentFontSize, window, cx| {
        workspace.reset_content_font_size(cx);
        window.refresh();
    });

    // 工作区 UI 字号缩放（当前窗口内生效）：同步更新窗口 rem 基准。
    workspace.register_action(|workspace, _: &IncreaseUiFontSize, window, cx| {
        workspace.increase_ui_font_size(1., cx);
        window.set_rem_size(workspace.typography().ui_size());
        window.refresh();
    });
    workspace.register_action(|workspace, _: &DecreaseUiFontSize, window, cx| {
        workspace.increase_ui_font_size(-1., cx);
        window.set_rem_size(workspace.typography().ui_size());
        window.refresh();
    });
    workspace.register_action(|workspace, _: &ResetUiFontSize, window, cx| {
        workspace.reset_ui_font_size(cx);
        window.set_rem_size(workspace.typography().ui_size());
        window.refresh();
    });

    zcv_search::install(workspace, window, cx);
    install_version_control(workspace, window, cx);

    let pane = workspace.pane().clone();
    let preview_toolbar = cx.new(|cx| PreviewToolbar::new(pane.downgrade(), cx));
    pane.update(cx, |pane, cx| {
        pane.toolbar().update(cx, |toolbar, cx| {
            toolbar.add_item(preview_toolbar, window, cx);
        });
    });

    for dock in [
        workspace.dock(DockPosition::Left).clone(),
        workspace.dock(DockPosition::Right).clone(),
        workspace.dock(DockPosition::Bottom).clone(),
    ] {
        dock.update(cx, |dock: &mut Dock, cx: &mut Context<Dock>| {
            let focus = dock.focus_handle(cx);
            let sub = cx.on_focus(
                &focus,
                window,
                |dock: &mut Dock, window: &mut Window, cx: &mut Context<Dock>| {
                    if let Some(panel) = dock.visible_panel() {
                        window.focus(&panel.focus_handle(cx), cx);
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
    let name = operation.display_name();
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
                });
            }
        }
    })
    .detach();
}

fn show_file_watcher_error(
    workspace: &mut Workspace,
    error: &FileWatcherError,
    cx: &mut Context<Workspace>,
) {
    let operation = match error.operation {
        FileWatcherOperation::Add => "监听项目路径",
        FileWatcherOperation::Remove => "停止监听项目路径",
    };
    workspace.show_toast(
        ToastKind::Error,
        format!(
            "{operation}失败（{}）：{}",
            error.path.display(),
            error.error
        ),
        None,
        Some(Duration::from_secs(8)),
        cx,
    );
}

fn initialize_workspace(
    workspace: &mut Workspace,
    languages: Arc<LanguageRegistry>,
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
                        let task = store
                            .update(cx, |store, cx| store.checkout_branch_with_result(name, cx));
                        let weak = ws.downgrade();
                        cx.spawn(
                            move |_this: WeakEntity<Workspace>, asynccx: &mut AsyncApp| {
                                let mut cx = asynccx.clone();
                                async move {
                                    let result = task.await;
                                    if let Err(error) = result.and_then(|outcome| match outcome {
                                        GitOperationOutcome::Failed(error) => {
                                            Err(anyhow::anyhow!(error))
                                        }
                                        _ => Ok(()),
                                    }) && let Some(this) = weak.upgrade()
                                    {
                                        this.update(&mut cx, |workspace, cx| {
                                            workspace.show_toast(
                                                ToastKind::Error,
                                                format!("切换分支失败：{error:#}"),
                                                None,
                                                Some(Duration::from_secs(5)),
                                                cx,
                                            );
                                        });
                                    }
                                }
                            },
                        )
                        .detach();
                    }
                    GitBranchAction::Create(name) => {
                        store.update(cx, |store, cx| store.create_branch(name, cx));
                    }
                    GitBranchAction::Delete(name) => {
                        let is_current = store
                            .read(cx)
                            .current_branch()
                            .is_some_and(|current| current == name);
                        if is_current {
                            workspace.show_toast(
                                ToastKind::Error,
                                "无法删除当前分支",
                                None,
                                Some(Duration::from_secs(5)),
                                cx,
                            );
                        } else {
                            store.update(cx, |store, cx| store.delete_branch(name, cx));
                        }
                    }
                }
            });
        }
    });

    let git_store = workspace.project().read(cx).git_store();
    let top_bar = cx.new(|cx| {
        let on_git_fetch = {
            let workspace = weak_self.clone();
            Rc::new(move |_window: &mut Window, cx: &mut App| {
                workspace
                    .update(cx, |workspace, cx| {
                        run_git_operation(workspace, GitOperationKind::Fetch, cx);
                    })
                    .ok();
            })
        };
        let on_git_pull = {
            let workspace = weak_self.clone();
            Rc::new(move |_window: &mut Window, cx: &mut App| {
                workspace
                    .update(cx, |workspace, cx| {
                        run_git_operation(workspace, GitOperationKind::Pull, cx);
                    })
                    .ok();
            })
        };
        let on_git_push = {
            let workspace = weak_self.clone();
            Rc::new(move |_window: &mut Window, cx: &mut App| {
                workspace
                    .update(cx, |workspace, cx| {
                        run_git_operation(workspace, GitOperationKind::Push, cx);
                    })
                    .ok();
            })
        };
        TopBar::new(
            switch_project_callback(languages),
            weak_self.clone(),
            git_store.clone(),
            on_branch,
            TopBarCallbacks {
                on_git_fetch,
                on_git_pull,
                on_git_push,
            },
            window,
            cx,
        )
    });
    let update_workspace = weak_self.clone();
    let update_button = cx.new(|cx| UpdateButton::new(update_workspace, cx));
    top_bar.update(cx, |bar, cx| {
        bar.set_update_control(update_button.into(), cx);
    });
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
        zcv_settings::ensure_user_settings_file().map(Path::to_path_buf)
    }));

    // ═══ 面板创建与注册 ═══════════════════════════════════════════

    let project = workspace.project().clone();

    let project_tree: Entity<ProjectTreePanel> = cx.new(|cx| {
        let mut tree = ProjectTreePanel::new(project.clone(), cx);
        tree.set_on_open_file(on_open_file_callback(&weak_self));
        tree.set_workspace(weak_self.clone());
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
        panel.set_on_open_graph(on_open_git_graph_callback(&weak_self));
        panel
    });

    workspace.register_panel(project_tree.clone(), DockPosition::Left, window, cx);
    workspace.register_panel(version_control, DockPosition::Left, window, cx);
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
    let git_subscription = cx.subscribe(&git_store, move |workspace, _store, event, cx| {
        if let GitStoreEvent::UncommitFailed(error) = event {
            workspace.show_toast(
                ToastKind::Error,
                format!("撤销提交失败：{error}"),
                None,
                Some(Duration::from_secs(5)),
                cx,
            );
        }
        // 任务事件只更新任务界面，不能反向触发差异业务；其余状态事件同步当前结果。
        // 展开状态按工作区文本跟踪区间跨刷新迁移（HEAD 变化不重置，见 diff_projection 模块说明）。
        if matches!(
            event,
            GitStoreEvent::Repositories
                | GitStoreEvent::Statuses
                | GitStoreEvent::Head
                | GitStoreEvent::IndexText { .. }
        ) {
            refresh_pane_git_projection(workspace.pane(), workspace.project(), cx);
        }
    });

    let project_tree_for_pane = project_tree.clone();
    let pane_subscription = cx.subscribe(&pane, move |workspace, pane, event, cx| {
        if let PaneEvent::ItemError { message } = event {
            workspace.show_toast(
                ToastKind::Error,
                message.clone(),
                None,
                Some(Duration::from_secs(5)),
                cx,
            );
            return;
        }
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
        if let PaneEvent::AddItem { item_id } = event
            && let Some(editor) = pane
                .read(cx)
                .tabs()
                .iter()
                .find(|item| item.item_id() == *item_id)
                .and_then(|item| item.act_as::<Editor>(cx))
        {
            subscribe_to_editor_events(workspace, editor, cx);
        }
        // 打开/激活编辑器时推送 git diff hunks（打开即有快照里的现成数据）。
        refresh_pane_git_projection(workspace.pane(), workspace.project(), cx);
    });

    // 项目事件订阅：根重命名与文件树变化驱动项目树刷新。
    let project_tree_for_project = project_tree.clone();
    let project_subscription =
        cx.subscribe(
            &project,
            move |_workspace, _project, event, cx| match event {
                ProjectEvent::RootChanged(root) => {
                    project_tree_for_project.update(cx, |tree, cx| {
                        tree.set_root(root.clone(), cx);
                    });
                }
                ProjectEvent::EntriesChanged => {
                    project_tree_for_project.update(cx, |tree, cx| tree.schedule_refresh(cx));
                }
                ProjectEvent::FileWatcherError(error) => {
                    show_file_watcher_error(_workspace, error, cx);
                }
            },
        );

    let pending_file_watcher_errors =
        project.update(cx, |project, _| project.take_pending_file_watcher_errors());
    for error in &pending_file_watcher_errors {
        show_file_watcher_error(workspace, error, cx);
    }

    let project_tree_for_settings = project_tree.clone();
    let settings_subscription =
        cx.observe_global_in::<SettingsStore>(window, move |workspace, window, cx| {
            let settings = SettingsStore::get(cx);
            typography::set_base_typography(
                cx,
                Some(settings.content_font_size),
                Some(settings.ui_font_size),
                Some(settings.content_line_height),
            );
            workspace.apply_typography_settings(&settings, cx);
            window.set_rem_size(workspace.typography().ui_size());
            apply_theme(&settings.theme, cx, Some(window));
            project_tree_for_settings.update(cx, |tree, cx| tree.refresh(cx));
            cx.notify();
        });

    let error_reporter = cx.global::<GlobalSettingsErrorReporter>().0.clone();
    let error_subscription = cx.subscribe(&error_reporter, |workspace, _, event, cx| {
        workspace.show_toast(
            ToastKind::Error,
            event.0.clone(),
            None,
            Some(Duration::from_secs(8)),
            cx,
        );
    });
    if let Some(error) = error_reporter.update(cx, |reporter, _| reporter.take_pending()) {
        workspace.show_toast(
            ToastKind::Error,
            error,
            None,
            Some(Duration::from_secs(8)),
            cx,
        );
    }

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
        error_subscription,
        appearance_subscription,
    ] {
        workspace.add_subscription(subscription);
    }
    let pane = workspace.pane().clone();
    let editors = pane
        .read(cx)
        .tabs()
        .iter()
        .filter_map(|item| item.act_as::<Editor>(cx))
        .collect::<Vec<_>>();
    for editor in editors {
        subscribe_to_editor_events(&mut *workspace, editor, cx);
    }
    refresh_pane_git_projection(&pane, workspace.project(), cx);
}

/// 将设置层的文本主题 id 解析并应用为主题运行时状态。
fn apply_theme(theme: &str, cx: &mut App, window: Option<&Window>) {
    ThemeChoice::from_config(theme).apply(cx, window);
}

/// 普通编辑器的文本变化只同步工作区文本上的冲突标记；
/// 其余 Git diff 投影由 zcv-version-control 的拥有域能力承载。
fn subscribe_to_editor_events(
    workspace: &mut Workspace,
    editor: Entity<Editor>,
    cx: &mut Context<Workspace>,
) {
    let project = workspace.project().clone();
    workspace.add_subscription(cx.subscribe(
        &editor,
        move |_workspace, editor, event: &EditorEvent, cx| {
            if matches!(event, EditorEvent::Edited { .. }) {
                let path = editor.read(cx).file_path(cx);
                if let Some(path) = path {
                    zcv_version_control::sync_editor_conflict_hunks(&editor, &path, &project, cx);
                }
            }
        },
    ));
}

// ── 内部类型 ────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "test/workspace_tests.rs"]
mod tests;
