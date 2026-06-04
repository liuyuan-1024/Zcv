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

use gpui::{AppContext, BorrowAppContext, Context, Entity, FocusHandle};

use crate::app::App;
use crate::focus::{AppFocus, FileTreeFocus, ProjectPickerFocus, SearchField};
use crate::shell::editor::{EditorKernel, EditorViewportSyncHook, TextEditorSlot};
use crate::shell::platform::clipboard::GpuiClipboard;
use crate::shell::surfaces::{SurfaceAnchorRegistry, SurfaceManager, SurfaceShell};
use crate::shell::workbench::PanelHost;
use crate::shell::workbench::controller::WorkbenchController;

use super::ShellView;
use super::features::FeatureRegistry;
use super::focus::FocusProjection;

pub(super) struct ShellRuntime {
    pub(super) app: Rc<RefCell<App>>,
    pub(super) workbench: Rc<RefCell<WorkbenchController>>,
    pub(super) features: FeatureRegistry,
    pub(super) surface_manager: Entity<SurfaceManager>,
    pub(super) surface_shell: Entity<SurfaceShell>,
    pub(super) main_editor_slot: Rc<TextEditorSlot>,
    pub(super) file_tree_slot: Rc<TextEditorSlot>,
    pub(super) search_query_slot: Rc<TextEditorSlot>,
    pub(super) search_replacement_slot: Rc<TextEditorSlot>,
    pub(super) editor_focus: FocusHandle,
    pub(super) focus_projection: FocusProjection,
    pub(super) panel_host: PanelHost,
}

impl ShellRuntime {
    pub(super) fn assemble(app: App, cx: &mut Context<ShellView>) -> Self {
        let app = Rc::new(RefCell::new(app));
        // 让命令派发期间的 copy / cut / paste 走系统剪贴板。
        // headless 单测路径不经过 ShellRuntime::assemble，所以仍是 MockClipboard。
        app.borrow_mut().set_clipboard(Box::new(GpuiClipboard));
        let workbench = Rc::new(RefCell::new(WorkbenchController::new()));
        cx.update_default_global::<SurfaceAnchorRegistry, _>(|_, _| ());
        let surface_manager = cx.new(|_| SurfaceManager::new());
        let editor_focus = cx.focus_handle();

        let features = FeatureRegistry::assemble(&app, cx);

        // 主编辑区内核：多行 + 行号 + 滚动 + 视口写回。
        // 视口钩子在 prepaint 末尾把测得的 ViewportState 推回 view。
        let main_viewport_sync: EditorViewportSyncHook = {
            let app = Rc::clone(&app);
            Rc::new(move |viewport, wrap_map, _cx| {
                app.borrow_mut().set_main_viewport(viewport, wrap_map);
            })
        };
        // 全局软换行 cell 由 App 持有；任何多行内核构造时都从 App 借这份 `Rc`，
        // 一次 toggle 同帧生效到主编辑区与所有嵌入式编辑器。
        let soft_wrap = app.borrow().soft_wrap_handle();
        let main_editor_kernel = EditorKernel::multi_line(soft_wrap.clone())
            .with_gutter()
            .with_vertical_scroll()
            .with_viewport_sync(main_viewport_sync);
        let main_editor_slot = TextEditorSlot::install(
            Rc::clone(&app),
            AppFocus::editor(),
            main_editor_kernel,
            editor_focus.clone(),
            cx,
        );
        let file_tree_slot = TextEditorSlot::install(
            Rc::clone(&app),
            AppFocus::file_tree(FileTreeFocus::NewEntryName),
            EditorKernel::single_line(),
            features.file_tree.focus_handle(),
            cx,
        );
        let project_picker_slot = TextEditorSlot::install(
            Rc::clone(&app),
            AppFocus::project_picker(ProjectPickerFocus::Query),
            EditorKernel::single_line(),
            features.project_picker.focus_handle(),
            cx,
        );
        features
            .project_picker
            .set_slot(Rc::clone(&project_picker_slot));
        let search_query_slot = TextEditorSlot::install(
            Rc::clone(&app),
            AppFocus::search(SearchField::Query),
            EditorKernel::single_line(),
            features.panels.search_query_focus_handle(),
            cx,
        );
        let search_replacement_slot = TextEditorSlot::install(
            Rc::clone(&app),
            AppFocus::search(SearchField::Replacement),
            EditorKernel::single_line(),
            features.panels.search_replacement_focus_handle(),
            cx,
        );
        let settings_toml_slot = TextEditorSlot::install(
            Rc::clone(&app),
            AppFocus::settings(),
            EditorKernel::multi_line(soft_wrap)
                .with_gutter()
                .with_vertical_scroll(),
            features.settings.focus_handle(),
            cx,
        );
        features
            .settings
            .set_toml_slot(Rc::clone(&settings_toml_slot));

        let surface_shell = cx.new(|cx| SurfaceShell::new(surface_manager.clone(), cx));

        let focus_projection = features.focus_projection(editor_focus.clone(), true);

        Self {
            app,
            workbench,
            features,
            surface_manager,
            surface_shell,
            main_editor_slot,
            file_tree_slot,
            search_query_slot,
            search_replacement_slot,
            editor_focus,
            focus_projection,
            panel_host: PanelHost::new(),
        }
    }
}
