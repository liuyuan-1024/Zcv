//! ShellView 的命令动作与 HostEffect 解释。
//!
//! 此文件只承担 view 层壳：命令派发入口、HostEffect 总调度、跨 feature 的窗口 / surface 管理。
//! 每个 feature 自己的 HostEffect 处理都在 `features/<feature>/effects.rs` 里，由 [`apply_host_effects_with_settings`] 按顺序问询。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{Entity, FocusHandle, Window};
use zom_command::{
    BubbleEffect, BubbleRequest, EditorEffect, GitEffect, HostEffect, Invocation, PanelEffect,
    SettingsChangeRequest, SurfaceEffect, WindowEffect,
};

use crate::app::App;
use crate::clipboard::GpuiClipboardScope;
use crate::config::SettingsChange;
use crate::editor::TextEditorSlot;
use crate::focus::AppFocus;
use crate::git_service::GitService;
use crate::host_intent::{CommandRequest, KeyRequest};
use crate::host_intent::{HostIntent, HostIntentOutcome, HostIntentRequest};
use crate::shell::bubble::BubbleRuntime;
use crate::shell::features::branch_picker;
use crate::shell::features::go_to_line;
use crate::shell::features::language_servers;
use crate::shell::features::panels::file_tree;
use crate::shell::features::panels::version_control;
use crate::shell::features::project_picker;
use crate::shell::features::search;
use crate::shell::features::settings;
use crate::shell::platform::window as platform_window;
use crate::shell::surfaces::{SurfaceManager, SurfaceRequest};
use crate::shell::workbench::controller::WorkbenchController;
use crate::ui_id::SurfaceId;

use super::config_visuals;
use super::features::FeatureRegistry;
use super::focus::{FocusProjection, panel_default_focus};

pub(super) fn bind_command_request(
    host_intent: HostIntentRequest,
    invocation: Invocation,
) -> CommandRequest {
    Rc::new(move |window, cx| {
        host_intent(HostIntent::Command(invocation.clone()), window, cx);
    })
}

pub(super) fn bind_key_request(host_intent: HostIntentRequest) -> KeyRequest {
    Rc::new(move |chord, window, cx| host_intent(HostIntent::KeyChord(chord), window, cx).consumed)
}

pub(super) fn bind_host_intent_request(
    app: Rc<RefCell<App>>,
    workbench: Rc<RefCell<WorkbenchController>>,
    surfaces: Entity<SurfaceManager>,
    bubbles: Entity<BubbleRuntime>,
    editor_focus_fallback: FocusHandle,
    features: FeatureRegistry,
    focus_projection: FocusProjection,
    text_editor_slots: Rc<dyn Fn() -> Vec<Rc<TextEditorSlot>>>,
) -> HostIntentRequest {
    let last_projected_focus: Rc<Cell<Option<AppFocus>>> = Rc::new(Cell::new(None));

    Rc::new(move |intent, window, cx| {
        let dispatch = {
            let _clip = GpuiClipboardScope::enter(cx);
            match intent {
                HostIntent::Command(invocation) => app
                    .borrow_mut()
                    .dispatch_command(invocation)
                    .map(|effects| (effects, HostIntentOutcome::consumed(), true)),
                HostIntent::KeyChord(chord) => {
                    let projected = focus_projection.current_focus(window);
                    // VC 面板和提交编辑器各自持有独立 FocusHandle，分别投影为
                    // Navigate 和 CommitMessage。只在投影值真正变化时才刷新 AppFocus，
                    // 避免同一 handle 内每次按键都触发无意义的 focus 同步。
                    if last_projected_focus.replace(Some(projected)) != Some(projected) {
                        let mut app = app.borrow_mut();
                        app.request_focus_from_shell(projected);
                    }
                    let mut app = app.borrow_mut();
                    app.dispatch_key(chord).map(|outcome| {
                        (
                            outcome.effects,
                            HostIntentOutcome {
                                consumed: outcome.consumed,
                            },
                            outcome.consumed,
                        )
                    })
                }
                HostIntent::Ime(intent) => app
                    .borrow_mut()
                    .dispatch_command(intent.into_invocation())
                    .map(|effects| (effects, HostIntentOutcome::consumed(), true)),
                HostIntent::Interaction(intent) => app
                    .borrow_mut()
                    .dispatch_interaction(intent)
                    .map(|effects| (effects, HostIntentOutcome::consumed(), true)),
            }
        };
        let (effects, outcome, should_refresh) = match dispatch {
            Ok(dispatch) => dispatch,
            Err(error) => {
                bubbles.update(cx, |runtime, cx| {
                    runtime.push(
                        BubbleRequest::error(format!("命令执行失败：{error}")).dedupe("cmd.error"),
                        cx,
                    );
                });
                return HostIntentOutcome::passed_through();
            }
        };

        let text_editor_slots = text_editor_slots();
        apply_host_effects_with_settings(
            effects,
            &app,
            &workbench,
            &surfaces,
            &bubbles,
            &editor_focus_fallback,
            &features,
            &text_editor_slots,
            window,
            cx,
        );
        if should_refresh {
            window.refresh();
        }
        outcome
    })
}

pub(super) fn apply_host_effects_with_settings(
    effects: Vec<HostEffect>,
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    surfaces: &Entity<SurfaceManager>,
    bubbles: &Entity<BubbleRuntime>,
    editor_focus_fallback: &FocusHandle,
    features: &FeatureRegistry,
    text_editor_slots: &[Rc<TextEditorSlot>],
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let focus = features.focus_projection(editor_focus_fallback.clone());
    for effect in effects {
        if settings::try_apply_effect(
            &effect,
            app,
            surfaces,
            editor_focus_fallback,
            &features.settings,
            window,
            cx,
        ) {
            continue;
        }
        dispatch_single_effect(
            &effect,
            app,
            workbench,
            surfaces,
            bubbles,
            &focus,
            editor_focus_fallback,
            features,
            text_editor_slots,
            window,
            cx,
        );
    }
}

// ── 单 effect 派发（两个 apply_host_effects* 的共享实现）──────────────────

/// 注册表模式：按顺序问询每个 feature 的 effect handler；
/// 第一个认领的返回 true，跳过余下。新 feature 只需加一行 `try_apply!`。
///
/// 用宏消除逐个 if-continue 的线性链；每个 handler 闭包捕获它需要的依赖。
/// `FnMut` 是因为 `window` / `cx` 是 `&mut` 引用。
fn dispatch_single_effect(
    effect: &HostEffect,
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    surfaces: &Entity<SurfaceManager>,
    bubbles: &Entity<BubbleRuntime>,
    focus: &FocusProjection,
    editor_focus_fallback: &FocusHandle,
    features: &FeatureRegistry,
    text_editor_slots: &[Rc<TextEditorSlot>],
    window: &mut Window,
    cx: &mut gpui::App,
) {
    macro_rules! try_apply {
        ($handler:expr) => {
            if ($handler)(effect) {
                return;
            }
        };
    }

    try_apply!(|effect: &HostEffect| -> bool {
        version_control::try_apply_effect(
            features.panels.vc_runtime(),
            effect,
            app,
            bubbles,
            window,
            cx,
        )
        .is_some()
    });
    try_apply!(|effect: &HostEffect| -> bool {
        file_tree::try_apply_effect(
            effect,
            app,
            workbench,
            &features.file_tree,
            focus,
            bubbles,
            window,
            cx,
        )
    });
    try_apply!(|effect: &HostEffect| -> bool {
        search::try_apply_effect(effect, app, focus, window)
    });
    try_apply!(|effect: &HostEffect| -> bool {
        go_to_line::try_apply_effect(
            effect,
            app,
            focus,
            surfaces,
            editor_focus_fallback,
            &features.go_to_line,
            window,
            cx,
        )
    });
    try_apply!(|effect: &HostEffect| -> bool {
        branch_picker::try_apply_effect(
            effect,
            app,
            focus,
            surfaces,
            editor_focus_fallback,
            &features.branch_picker,
            bubbles,
            window,
            cx,
        )
    });
    try_apply!(|effect: &HostEffect| -> bool {
        project_picker::try_apply_effect(
            effect,
            app,
            workbench,
            surfaces,
            editor_focus_fallback,
            &features.file_tree,
            &features.project_picker,
            bubbles,
            window,
            cx,
        )
    });
    try_apply!(|effect: &HostEffect| -> bool {
        language_servers::try_apply_effect(
            effect,
            app,
            surfaces,
            editor_focus_fallback,
            &features.language_servers,
            window,
            cx,
        )
    });

    apply_shell_effect(
        effect,
        app,
        workbench,
        surfaces,
        bubbles,
        focus,
        text_editor_slots,
        window,
        cx,
    );
}

/// 没有归属到具体 feature 的"壳"级变体：窗口控制、TogglePanel、DismissSurface、未实现的占位。
fn apply_shell_effect(
    effect: &HostEffect,
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    surfaces: &Entity<SurfaceManager>,
    bubbles: &Entity<BubbleRuntime>,
    focus: &FocusProjection,
    text_editor_slots: &[Rc<TextEditorSlot>],
    window: &mut Window,
    cx: &mut gpui::App,
) {
    match effect {
        HostEffect::Window(WindowEffect::Quit) => platform_window::quit(cx),
        HostEffect::Window(WindowEffect::Minimize) => platform_window::minimize(window),
        HostEffect::Window(WindowEffect::ToggleMaximize) => {
            platform_window::toggle_maximize(window)
        }
        HostEffect::Bubble(BubbleEffect::Show(request)) => {
            bubbles.update(cx, |runtime, cx| runtime.push(request.clone(), cx));
            window.refresh();
        }
        HostEffect::Surface(SurfaceEffect::OpenSettingsToml) => {
            let opened = app.borrow_mut().apply_open_config_file_from_effect();
            for request in app.borrow_mut().take_session_bubbles() {
                bubbles.update(cx, |runtime, cx| runtime.push(request, cx));
            }
            if opened {
                request_focus(app, focus, AppFocus::editor(), window);
            }
            window.refresh();
        }
        HostEffect::Surface(SurfaceEffect::ApplySettingsChange(change)) => {
            let config = {
                let mut app = app.borrow_mut();
                app.apply_settings_change_from_effect(settings_change(*change));
                app.config_snapshot()
            };
            config_visuals::apply(&config, Some(window));
            window.refresh();
        }
        HostEffect::Panel(PanelEffect::Toggle(panel, via_pointer)) => {
            let panel = *panel;
            if *via_pointer {
                // 鼠标点击：纯 toggle，不判断焦点归属。
                if workbench.borrow().is_panel_active(panel) {
                    workbench.borrow_mut().hide_panel(panel);
                    request_focus(app, focus, AppFocus::editor(), window);
                } else {
                    workbench.borrow_mut().show_panel(panel);
                    request_focus(app, focus, panel_default_focus(panel), window);
                }
            } else {
                // 键盘：已显示且焦点在面板上才收起。
                let visible = workbench.borrow().is_panel_active(panel);
                if visible && focus.is_at_panel(panel, window) {
                    workbench.borrow_mut().hide_panel(panel);
                    request_focus(app, focus, AppFocus::editor(), window);
                } else {
                    workbench.borrow_mut().show_panel(panel);
                    request_focus(app, focus, panel_default_focus(panel), window);
                }
            }
            window.refresh();
        }
        HostEffect::Editor(EditorEffect::ToggleSoftWrap) => {
            app.borrow_mut().toggle_soft_wrap();
            window.refresh();
        }
        HostEffect::Editor(EditorEffect::SelectTab(view_id)) => {
            app.borrow_mut().activate_view_tab(*view_id);
            window.refresh();
        }
        HostEffect::Editor(EditorEffect::CancelPointerSelection) => {
            for slot in text_editor_slots {
                slot.cancel_pointer_selection();
            }
        }
        HostEffect::Git(GitEffect::Fetch) => spawn_git_op(
            app,
            bubbles,
            window,
            cx,
            ("正在获取远程更新…", "git.fetch_status"),
            ("fetch", &[] as &[&str]),
            |git, app| {
                let remote = git.remote_ahead_count();
                let local = git.local_ahead_count();
                app.set_remote_ahead_count(remote);
                app.set_local_ahead_count(local);
                if remote > 0 || local > 0 {
                    let mut parts = Vec::new();
                    if remote > 0 {
                        parts.push(format!("远程 {remote} 个新提交"));
                    }
                    if local > 0 {
                        parts.push(format!("本地 {local} 个未推送"));
                    }
                    BubbleRequest::success(parts.join("，"))
                } else {
                    BubbleRequest::success("已是最新")
                }
            },
            "获取远程更新失败",
        ),
        HostEffect::Git(GitEffect::Pull) => spawn_git_op(
            app,
            bubbles,
            window,
            cx,
            ("正在拉取远程提交…", "git.pull_status"),
            ("merge", &["--ff-only", "@{upstream}"]),
            |git, app| {
                let local = git.local_ahead_count();
                app.set_remote_ahead_count(0);
                app.set_local_ahead_count(local);
                BubbleRequest::success("拉取成功")
            },
            "拉取远程提交失败",
        ),
        HostEffect::Git(GitEffect::Push) => spawn_git_op(
            app,
            bubbles,
            window,
            cx,
            ("正在推送本地提交…", "git.push_status"),
            ("push", &[]),
            |git, app| {
                let local = git.local_ahead_count();
                app.set_local_ahead_count(local);
                BubbleRequest::success("推送成功")
            },
            "推送失败",
        ),
        HostEffect::Surface(SurfaceEffect::Dismiss) => {
            if surfaces.read_with(cx, |manager, _| manager.is_active(SurfaceId::ProjectPicker)) {
                app.borrow_mut().project_picker_deactivate();
            }
            dismiss_surface(surfaces, window, cx);
        }
        other => {
            bubbles.update(cx, |runtime, cx| {
                runtime.push(
                    BubbleRequest::error(format!("未实现的功能：{other:?}"))
                        .dedupe("shell.unhandled"),
                    cx,
                );
            });
        }
    }
}

/// 在后台线程执行 git 网络操作，主线程推占位气泡 + 结果气泡。
///
/// `on_ok` 在 git 成功后、主线程 `cx.update` 内执行，负责更新 App 状态并返回结果气泡。
fn spawn_git_op(
    app: &Rc<RefCell<App>>,
    bubbles: &Entity<BubbleRuntime>,
    window: &mut Window,
    cx: &mut gpui::App,
    placeholder: (&str, &str),
    git_cmd: (&str, &[&str]),
    on_ok: impl FnOnce(&GitService, &mut App) -> BubbleRequest + 'static,
    error_prefix: &str,
) {
    let git_handle = app.borrow().git_handle();
    if !git_handle.borrow().is_git_repo() {
        let (_, dedupe_key) = placeholder;
        bubbles.update(cx, |runtime, cx| {
            runtime.push(
                BubbleRequest::error("不在 Git 仓库中").dedupe(dedupe_key),
                cx,
            );
        });
        window.refresh();
        return;
    }

    let repo_root = git_handle.borrow().repo_root_path().to_path_buf();
    let app = app.clone();
    let bubbles = bubbles.clone();
    let ph_key = placeholder.1.to_string();
    let git_command = git_cmd.0.to_string();
    let git_args: Vec<String> = git_cmd.1.iter().map(|s| s.to_string()).collect();
    let error_prefix = error_prefix.to_string();

    bubbles.update(cx, |runtime, cx| {
        runtime.push(BubbleRequest::info(placeholder.0).dedupe(&ph_key), cx);
    });
    window.refresh();

    let ph_key_for_spawn = ph_key.clone();
    let git_cmd_name = git_command.clone();
    window
        .spawn(cx, async move |cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut cmd = std::process::Command::new("git");
                    cmd.arg(&git_command);
                    for arg in &git_args {
                        cmd.arg(arg);
                    }
                    let output = cmd
                        .current_dir(&repo_root)
                        .output()
                        .map_err(|e| format!("无法执行 git {git_cmd_name}：{e}"))?;
                    if !output.status.success() {
                        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
                    }
                    Ok(())
                })
                .await;
            let _ = cx.update(|window, cx| {
                let git = git_handle.borrow();
                match result {
                    Ok(()) => {
                        let bubble = on_ok(&git, &mut app.borrow_mut());
                        bubbles.update(cx, |runtime, cx| {
                            runtime.push(bubble.dedupe(&ph_key_for_spawn), cx);
                        });
                    }
                    Err(e) => {
                        bubbles.update(cx, |runtime, cx| {
                            runtime.push(
                                BubbleRequest::error(format!("{error_prefix}：{e}"))
                                    .dedupe(&ph_key_for_spawn),
                                cx,
                            );
                        });
                    }
                }
                window.refresh();
            });
        })
        .detach();
}

fn settings_change(change: SettingsChangeRequest) -> SettingsChange {
    match change {
        SettingsChangeRequest::AdjustUiFont(delta) => SettingsChange::AdjustUiFont(delta),
        SettingsChangeRequest::AdjustEditorFont(delta) => SettingsChange::AdjustEditorFont(delta),
        SettingsChangeRequest::ToggleEditorSoftWrap => SettingsChange::ToggleEditorSoftWrap,
        SettingsChangeRequest::CycleEditorTabSize => SettingsChange::CycleEditorTabSize,
        SettingsChangeRequest::CycleTheme => SettingsChange::CycleTheme,
    }
}

pub(crate) fn request_focus(
    app: &Rc<RefCell<App>>,
    projection: &FocusProjection,
    focus: AppFocus,
    window: &mut Window,
) {
    app.borrow_mut().request_focus(focus);
    let current = app.borrow().focus().current();
    projection.apply(current, window);
}

pub(crate) fn dismiss_surface(
    surfaces: &Entity<SurfaceManager>,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let Some(focus_to_restore) = surfaces.update(cx, |surfaces, cx| surfaces.dismiss(cx)) else {
        return;
    };
    window.focus(&focus_to_restore);
    window.refresh();
}

pub(crate) fn open_surface(
    request: SurfaceRequest,
    surfaces: &Entity<SurfaceManager>,
    editor_focus_fallback: &FocusHandle,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    // 手册 21.7：关闭时焦点回到"先前 focus 目标"——open 这一帧 window 里实际聚焦的元素。
    // 查不到（窗口刚启动等）退回 editor 焦点，避免关闭后焦点悬空。
    let focus_to_restore = window
        .focused(cx)
        .unwrap_or_else(|| editor_focus_fallback.clone());
    let focus_on_open = request.focus_on_open.clone();
    surfaces.update(cx, |surfaces, cx| {
        surfaces.open(request, focus_to_restore, cx);
    });
    if let Some(focus) = focus_on_open {
        window.focus(&focus);
    }
    window.refresh();
}
