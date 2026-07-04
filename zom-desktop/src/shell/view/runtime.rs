//! Shell 装配产物：把 [`ShellView::new`] 里的构造步骤搬到一个独立的"组合根"。
//!
//! 这里只负责一次性把 App / WorkbenchController / SurfaceManager / 各
//! TextEditorSlot / FeatureRegistry / FocusProjection 拉起来。新增运行期可见
//! 的字段先加在 [`ShellRuntime`]，[`ShellView`] 不再随 feature 增长。
//!
//! [`ShellView`]: super::ShellView
//! [`ShellView::new`]: super::ShellView::new

use std::cell::RefCell;
use std::rc::Rc;

use zom_command::commands::branch_picker as branch_picker_commands;

use gpui::{AppContext, BorrowAppContext, Context, Entity, FocusHandle};

use crate::app::App;
use crate::clipboard::GpuiClipboard;
use crate::editor::{EditorKernel, EditorViewportSyncHook, TextEditorSlot};
use crate::focus::{AppFocus, FileTreeFocus, PanelFocus, SearchField};
use crate::host_intent::{CommandRequest, HostIntent, HostIntentRequest};
use crate::shell::bubble::{BubbleRuntime, BubbleShell};
use crate::shell::surfaces::{SurfaceAnchorRegistry, SurfaceManager, SurfaceShell};
use crate::shell::workbench::PanelHost;
use crate::shell::workbench::controller::WorkbenchController;

use super::config_visuals;
use super::features::FeatureRegistry;
use super::focus::FocusProjection;
use super::{ShellView, actions};

pub(super) struct ShellRuntime {
    pub(super) app: Rc<RefCell<App>>,
    pub(super) workbench: Rc<RefCell<WorkbenchController>>,
    pub(super) features: FeatureRegistry,
    pub(super) surface_manager: Entity<SurfaceManager>,
    pub(super) surface_shell: Entity<SurfaceShell>,
    pub(super) bubble_runtime: Entity<BubbleRuntime>,
    pub(super) bubble_shell: Entity<BubbleShell>,
    pub(super) main_editor_slot: Rc<TextEditorSlot>,
    pub(super) file_tree_new_entry_slot: Rc<TextEditorSlot>,
    pub(super) file_tree_rename_slot: Rc<TextEditorSlot>,
    pub(super) search_query_slot: Rc<TextEditorSlot>,
    pub(super) search_replacement_slot: Rc<TextEditorSlot>,
    pub(super) editor_focus: FocusHandle,
    pub(super) focus_projection: FocusProjection,
    pub(super) host_intent: HostIntentRequest,
    pub(super) panel_host: PanelHost,
}

impl ShellRuntime {
    pub(super) fn assemble(app: App, cx: &mut Context<ShellView>) -> Self {
        let app = Rc::new(RefCell::new(app));
        config_visuals::apply(&app.borrow().config_snapshot(), None);
        // 让命令派发期间的 copy / cut / paste 走系统剪贴板。
        // headless 路径不经过 ShellRuntime::assemble，所以仍是 NoopClipboard。
        app.borrow_mut().set_clipboard(Box::new(GpuiClipboard));
        let workbench = Rc::new(RefCell::new(WorkbenchController::new()));
        cx.update_default_global::<SurfaceAnchorRegistry, _>(|_, _| ());
        let surface_manager = cx.new(|_| SurfaceManager::new());
        let bubble_runtime = cx.new(|_| BubbleRuntime::new());
        let editor_focus = cx.focus_handle();

        let features = FeatureRegistry::assemble(&app, cx);
        let focus_projection = features.focus_projection(editor_focus.clone());
        let text_editor_slots: Rc<RefCell<Vec<Rc<TextEditorSlot>>>> =
            Rc::new(RefCell::new(Vec::new()));
        let text_editor_slots_provider = {
            let text_editor_slots = Rc::clone(&text_editor_slots);
            Rc::new(move || text_editor_slots.borrow().clone())
        };
        let host_intent = actions::bind_host_intent_request(
            Rc::clone(&app),
            Rc::clone(&workbench),
            surface_manager.clone(),
            bubble_runtime.clone(),
            editor_focus.clone(),
            features.clone(),
            focus_projection.clone(),
            text_editor_slots_provider,
        );

        bubble_runtime.update(cx, |runtime, _| {
            runtime.set_intent_request(host_intent.clone());
        });

        // 主编辑区内核：多行 + 行号 + 滚动 + 视口测量回写。
        // 视口钩子在 prepaint 测出本帧 wrap_map 后即时调用，
        // App 侧顺手用新 wrap_map 跑一次 settle，把 settle 后的顶端返回给 element。
        let main_viewport_sync: EditorViewportSyncHook = {
            let app = Rc::clone(&app);
            Rc::new(move |measurement, wrap_map, _cx| {
                app.borrow_mut()
                    .sync_main_viewport_measurement(measurement, wrap_map)
            })
        };
        // 全局软换行 cell 由 App 持有；
        // 多行内核构造时从 App 借这份 `Rc`， 一次 toggle 同帧生效到主编辑区。
        let soft_wrap = app.borrow().soft_wrap_handle();
        let git_handle = app.borrow().git_handle();
        let main_editor_kernel = EditorKernel::multi_line(soft_wrap.clone())
            .with_gutter()
            .with_vertical_scroll()
            .with_viewport_sync(main_viewport_sync)
            .with_git(git_handle);
        let main_editor_slot = TextEditorSlot::install(
            Rc::clone(&app),
            Rc::clone(&host_intent),
            AppFocus::editor(),
            main_editor_kernel,
            editor_focus.clone(),
            cx,
        );
        let file_tree_new_entry_slot = TextEditorSlot::install(
            Rc::clone(&app),
            Rc::clone(&host_intent),
            AppFocus::file_tree(FileTreeFocus::NewEntryName),
            EditorKernel::single_line(),
            features.file_tree.focus_handle(),
            cx,
        );
        let file_tree_rename_slot = TextEditorSlot::install(
            Rc::clone(&app),
            Rc::clone(&host_intent),
            AppFocus::file_tree(FileTreeFocus::RenameEntry),
            EditorKernel::single_line(),
            features.file_tree.focus_handle(),
            cx,
        );
        let project_picker_slot = TextEditorSlot::install(
            Rc::clone(&app),
            Rc::clone(&host_intent),
            AppFocus::project_picker(),
            EditorKernel::single_line(),
            features.project_picker.focus_handle(),
            cx,
        );
        features
            .project_picker
            .set_slot(Rc::clone(&project_picker_slot));
        let search_query_slot = TextEditorSlot::install(
            Rc::clone(&app),
            Rc::clone(&host_intent),
            AppFocus::search(SearchField::Query),
            EditorKernel::single_line(),
            features.search.query_focus_handle(),
            cx,
        );
        let search_replacement_slot = TextEditorSlot::install(
            Rc::clone(&app),
            Rc::clone(&host_intent),
            AppFocus::search(SearchField::Replacement),
            EditorKernel::single_line(),
            features.search.replacement_focus_handle(),
            cx,
        );
        let go_to_line_slot = TextEditorSlot::install(
            Rc::clone(&app),
            Rc::clone(&host_intent),
            AppFocus::go_to_line(),
            EditorKernel::single_line(),
            features.go_to_line.focus_handle(),
            cx,
        );
        features.go_to_line.set_slot(Rc::clone(&go_to_line_slot));
        let branch_picker_slot = TextEditorSlot::install(
            Rc::clone(&app),
            Rc::clone(&host_intent),
            AppFocus::branch_picker(),
            EditorKernel::single_line(),
            features.branch_picker.focus_handle(),
            cx,
        );
        features
            .branch_picker
            .set_slot(Rc::clone(&branch_picker_slot));
        {
            let host_intent = Rc::clone(&host_intent);
            let switch_invocation = branch_picker_commands::switch();
            let switch_request: CommandRequest = Rc::new(move |window, cx| {
                host_intent(HostIntent::Command(switch_invocation.clone()), window, cx);
            });
            features.branch_picker.set_switch_request(switch_request);
        }
        {
            let host_intent = Rc::clone(&host_intent);
            let delete_invocation = branch_picker_commands::delete();
            let delete_request: CommandRequest = Rc::new(move |window, cx| {
                host_intent(HostIntent::Command(delete_invocation.clone()), window, cx);
            });
            features.branch_picker.set_delete_request(delete_request);
        }
        let vc_commit_message_slot = TextEditorSlot::install(
            Rc::clone(&app),
            Rc::clone(&host_intent),
            AppFocus::Panel(PanelFocus::version_control_commit()),
            EditorKernel::multi_line(soft_wrap.clone()).with_vertical_scroll(),
            features.panels.vc_runtime().commit_focus_handle(),
            cx,
        );
        features
            .panels
            .vc_runtime()
            .set_slot(Rc::clone(&vc_commit_message_slot));
        *text_editor_slots.borrow_mut() = vec![
            Rc::clone(&main_editor_slot),
            Rc::clone(&file_tree_new_entry_slot),
            Rc::clone(&file_tree_rename_slot),
            Rc::clone(&project_picker_slot),
            Rc::clone(&search_query_slot),
            Rc::clone(&search_replacement_slot),
            Rc::clone(&go_to_line_slot),
            Rc::clone(&branch_picker_slot),
            Rc::clone(&vc_commit_message_slot),
        ];
        let surface_shell = cx.new(|cx| SurfaceShell::new(surface_manager.clone(), cx));
        let title_lookup: crate::shell::CommandTitleLookup = {
            let app = Rc::clone(&app);
            Rc::new(move |id| app.borrow().command_title_for(id))
        };
        let shortcut_lookup: crate::shell::ShortcutLookup = {
            let app = Rc::clone(&app);
            Rc::new(move |id| app.borrow().shortcuts_for(id))
        };
        let bubble_shell = cx
            .new(|cx| BubbleShell::new(bubble_runtime.clone(), title_lookup, shortcut_lookup, cx));

        Self {
            app,
            workbench,
            features,
            surface_manager,
            surface_shell,
            bubble_runtime,
            bubble_shell,
            main_editor_slot,
            file_tree_new_entry_slot,
            file_tree_rename_slot,
            search_query_slot,
            search_replacement_slot,
            editor_focus,
            focus_projection,
            host_intent,
            panel_host: PanelHost::new(),
        }
    }
}
