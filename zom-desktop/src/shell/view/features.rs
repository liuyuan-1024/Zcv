//! 把 shell feature runtime 折叠成单一 registry。
//!
//! ShellView 不再为每个 feature 同时持字段、传参数、抄一遍焦点投影。
//! 新增 feature 只需在 [`FeatureRegistry`] 添字段并在 [`assemble`] / [`install_listeners`] / [`focus_projection`] 各添一行；
//! callsite（actions / key_request）拿 `&FeatureRegistry` 就够。
//!
//! [`assemble`]: FeatureRegistry::assemble
//! [`install_listeners`]: FeatureRegistry::install_listeners
//! [`focus_projection`]: FeatureRegistry::focus_projection

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Context, Entity, FocusHandle, Window};

use crate::app::App;
use crate::shell::features::language_servers::LanguageServersRuntime;
use crate::shell::features::panels::PanelRuntimes;
use crate::shell::features::panels::file_tree::FileTreeRuntime;
use crate::shell::features::panels::search::{SearchEditObserver, SearchFramePump};
use crate::shell::features::project_picker::{ProjectPickerRuntime, RecentProjects};
use crate::shell::features::settings::SettingsRuntime;
use crate::shell::surfaces::SurfaceManager;

use super::focus::{FocusProjection, projection_from_runtimes};

#[derive(Clone)]
pub(super) struct FeatureRegistry {
    pub(super) panels: PanelRuntimes,
    pub(super) file_tree: FileTreeRuntime,
    pub(super) project_picker: ProjectPickerRuntime,
    pub(super) language_servers: LanguageServersRuntime,
    pub(super) settings: SettingsRuntime,
}

impl FeatureRegistry {
    /// 构造全部 feature runtime，并把它们要 owner / search 接入 router 的事一次性做完。
    /// 生产路径用 [`RecentProjects::default_path`]；headless 单测不经过 ShellView::new。
    pub(super) fn assemble<T>(app: &Rc<RefCell<App>>, cx: &mut Context<T>) -> Self {
        let panels = PanelRuntimes::new(cx);
        let file_tree = FileTreeRuntime::new(cx);
        let project_picker = ProjectPickerRuntime::new(cx, RecentProjects::default_path());
        let language_servers = LanguageServersRuntime::new(cx);
        let settings = SettingsRuntime::new(cx);

        {
            let mut app = app.borrow_mut();
            app.install_editor_owner(file_tree.owner_handle());
            app.install_editor_owner(project_picker.owner_handle());
            // SearchModel 同时承载 query / replacement 两个输入框，按 focus 内部分派；
            // 通过同一个 install_editor_owner 注册进 router，TextTargetRuntime 不为它单走特殊分支。
            app.install_editor_owner(panels.search_owner_handle());
            // 编辑后同步与每帧后台命中收割走通用端口注册——BackgroundPumps
            // 不认 search feature，由它两个 trait 实现自报家门。
            let search_handle = panels.search_runtime_handle();
            app.install_post_edit_observer(Box::new(SearchEditObserver::new(search_handle)));
            app.install_frame_pump(Box::new(SearchFramePump));
        }

        Self {
            panels,
            file_tree,
            project_picker,
            language_servers,
            settings,
        }
    }

    pub(super) fn install_listeners<T: 'static>(
        &self,
        app: Rc<RefCell<App>>,
        surfaces: Entity<SurfaceManager>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        self.file_tree
            .install_listeners(Rc::clone(&app), window, cx);
        self.project_picker
            .install_listeners(Rc::clone(&app), surfaces.clone(), window, cx);
        self.language_servers
            .install_listeners(surfaces.clone(), window, cx);
        self.settings.install_listeners(surfaces, window, cx);
        self.panels.install_listeners(app, window, cx);
    }

    /// 组合给定 editor focus 与各 feature 的 focus handle 成 `AppFocus <-> FocusHandle` 投影表。
    pub(super) fn focus_projection(&self, editor: FocusHandle) -> FocusProjection {
        projection_from_runtimes(
            editor,
            &self.panels,
            &self.file_tree,
            self.project_picker.focus_handle(),
            Some(self.settings.focus_handle()),
        )
    }
}
