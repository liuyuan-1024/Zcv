//! WorkbenchFrame —— 窗口顶层装配。

pub(crate) mod active_buffer_language;
pub(crate) mod cursor_position;
pub(crate) mod diagnostics_button;
pub(crate) mod dock;
pub(crate) mod item;
pub(crate) mod lsp_button;
pub(crate) mod pane;
pub(crate) mod pane_group;
pub(crate) mod panel;
pub(crate) mod panel_buttons;
pub(crate) mod project_picker;
pub(crate) mod project_search_button;
pub(crate) mod project_tree;
mod recent_projects;
pub(crate) mod status_bar;
pub(crate) mod top_bar;
pub(crate) mod window_controls;

use std::cell::RefCell;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    App, Context, Div, Entity, FocusHandle, MouseButton, Render, Subscription, Window, actions,
    div, prelude::*,
};
use zcv_engine::{Buffer, BufferSaveError};

use self::active_buffer_language::ActiveBufferLanguage;
use self::cursor_position::CursorPosition;
use self::diagnostics_button::DiagnosticsButton;
use self::dock::{
    DockArea, LayoutController, LayoutSnapshot, ToggleDebug, ToggleKeyboardShortcuts,
    ToggleOutline, ToggleProjectTree, ToggleTerminal, ToggleVersionControl,
    render_body as render_layout_body,
};
use self::lsp_button::LspButton;
use self::pane::Pane;
use self::pane_group::PaneId;
use self::panel::PanelHandle;
use self::panel_buttons::PanelButtons;
use self::project_picker::OnProjectSelected;
use self::project_search_button::ProjectSearchButton;
use self::project_tree::{OnOpenFile, ProjectTree};
use self::status_bar::StatusBar;
use self::top_bar::TopBar;
use crate::editor::buffer_store::BufferStore;
use crate::keymap;
use crate::theme::{color, typography};

actions!(workspace, [Save]);

pub(crate) struct Workspace {
    pub(crate) focus: FocusHandle,
    pub(crate) layout: Rc<RefCell<LayoutController>>,
    pub(crate) focus_pane: Option<Entity<Pane>>,
    top_bar: Entity<TopBar>,
    status_bar: Entity<StatusBar>,
    project_tree: Entity<ProjectTree>,
    /// 面板 Entity 列表，与 LayoutController.panels index 对应。
    panel_handles: Vec<Arc<dyn PanelHandle>>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    /// 创建所有面板 Entity，返回 (panel_handles, panel_pairs)。
    /// `project_tree` 由调用方先创建并传入，确保只有一个实体。
    fn make_panels(
        project_tree: &Entity<ProjectTree>,
        cx: &mut Context<Self>,
    ) -> (
        Vec<Arc<dyn PanelHandle>>,
        Vec<(Arc<dyn PanelHandle>, DockArea)>,
    ) {
        let version_control = cx.new(|cx| panel::VersionControlPanel::new(cx));
        let outline = cx.new(|cx| panel::OutlinePanel::new(cx));
        let terminal = cx.new(|cx| panel::TerminalPanel::new(cx));
        let debug = cx.new(|cx| panel::DebugPanel::new(cx));
        let keyboard_shortcuts = cx.new(|cx| panel::KeyboardShortcutsPanel::new(cx));

        let mut handles: Vec<Arc<dyn PanelHandle>> = Vec::new();
        let mut pairs: Vec<(Arc<dyn PanelHandle>, DockArea)> = Vec::new();

        macro_rules! reg {
            ($entity:expr, $area:expr) => {{
                let handle: Arc<dyn PanelHandle> = Arc::new($entity);
                handles.push(handle.clone());
                pairs.push((handle, $area));
            }};
        }

        reg!(project_tree.clone(), DockArea::Left);
        reg!(version_control, DockArea::Left);
        reg!(outline, DockArea::Left);
        reg!(terminal, DockArea::Bottom);
        reg!(debug, DockArea::Bottom);
        reg!(keyboard_shortcuts, DockArea::Right);

        (handles, pairs)
    }

    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let weak_self: gpui::WeakEntity<Self> = cx.weak_entity();
        let weak_project_switcher = weak_self.clone();
        let weak_open_file = weak_self.clone();

        let keybindings = keymap::load();
        cx.bind_keys(keybindings.bindings.clone());
        cx.set_global(keybindings);

        let on_project_selected: OnProjectSelected = Rc::new(move |path, window, app| {
            if let Some(ws) = weak_project_switcher.upgrade() {
                ws.update(app, |workspace, cx| {
                    workspace.switch_project(&path, window, cx);
                });
            }
        });

        let top_bar = cx.new(|cx| TopBar::new(on_project_selected, cx));

        let initial_pane = cx.new(|cx| Pane::new(PaneId(1), cx));
        let status_pane = initial_pane.clone();

        // 先创建 ProjectTree（唯一实体），再创建面板
        let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let project_tree: Entity<ProjectTree> = cx.new(|cx| {
            let mut tree = ProjectTree::new(root, cx);
            let on_open_file: OnOpenFile = Rc::new(
                move |path: PathBuf, window: &mut Window, cx: &mut gpui::App| {
                    if let Some(ws) = weak_open_file.upgrade() {
                        ws.update(cx, |ws, cx| {
                            ws.open_path_in_active_pane(path, window, cx);
                        });
                    }
                },
            );
            tree.set_on_open_file(on_open_file);
            tree
        });

        let (all_handles, panel_pairs) = Self::make_panels(&project_tree, cx);

        let layout = Rc::new(RefCell::new(LayoutController::with_initial_pane(
            initial_pane.clone(),
            panel_pairs,
        )));

        // 创建 StatusBar 并注册按钮
        let status_bar = cx.new(|cx| StatusBar::new(status_pane, cx));
        status_bar.update(cx, |bar, cx| {
            let for_area =
                |area: DockArea| -> Vec<(Arc<dyn PanelHandle>, fn(&mut Window, &mut App))> {
                    all_handles
                        .iter()
                        .filter(|h| h.position() == area)
                        .map(|h| {
                            let dispatch: fn(&mut Window, &mut App) = match h.action_name() {
                                "dock::ToggleProjectTree" => {
                                    |w, cx| w.dispatch_action(Box::new(ToggleProjectTree), cx)
                                }
                                "dock::ToggleVersionControl" => {
                                    |w, cx| w.dispatch_action(Box::new(ToggleVersionControl), cx)
                                }
                                "dock::ToggleOutline" => {
                                    |w, cx| w.dispatch_action(Box::new(ToggleOutline), cx)
                                }
                                "dock::ToggleTerminal" => {
                                    |w, cx| w.dispatch_action(Box::new(ToggleTerminal), cx)
                                }
                                "dock::ToggleDebug" => {
                                    |w, cx| w.dispatch_action(Box::new(ToggleDebug), cx)
                                }
                                "dock::ToggleKeyboardShortcuts" => {
                                    |w, cx| w.dispatch_action(Box::new(ToggleKeyboardShortcuts), cx)
                                }
                                _ => unreachable!(),
                            };
                            (h.clone(), dispatch)
                        })
                        .collect()
                };

            bar.add_left_item(cx.new(|_| PanelButtons::new(for_area(DockArea::Left))), cx);
            bar.add_left_item(cx.new(|_| LspButton::new()), cx);
            bar.add_left_item(cx.new(|_| DiagnosticsButton::new()), cx);
            bar.add_left_item(cx.new(|_| ProjectSearchButton::new()), cx);
            bar.add_right_item(cx.new(|_| CursorPosition::new()), cx);
            bar.add_right_item(cx.new(|_| ActiveBufferLanguage::new()), cx);
            bar.add_right_item(
                cx.new(|_| PanelButtons::new(for_area(DockArea::Bottom))),
                cx,
            );
            bar.add_right_item(cx.new(|_| PanelButtons::new(for_area(DockArea::Right))), cx);
        });

        cx.set_global(BufferStore::new());

        Self {
            focus,
            layout,
            focus_pane: Some(initial_pane),
            top_bar,
            status_bar,
            project_tree,
            panel_handles: all_handles,
            _subscriptions: Vec::new(),
        }
    }

    /// 由项目树回调调用的文件打开逻辑。
    fn open_path_in_active_pane(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = match path.canonicalize() {
            Ok(p) => p,
            Err(error) => {
                eprintln!("打开文件失败：{}：{error}", path.display());
                return;
            }
        };
        let Ok(buffer) =
            cx.update_global::<BufferStore, _>(|store, cx| store.open_buffer(&path, cx))
        else {
            return;
        };
        let Some(pane) = self.focus_pane.clone() else {
            return;
        };
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let focus = pane.update(cx, |pane, cx| pane.open_file(path, file_name, buffer, cx));
        window.focus(&focus);
        window.refresh();
    }

    /// 开发构建启动时，沿用正式项目切换链路打开 zcv 工作区。
    #[cfg(debug_assertions)]
    pub(crate) fn open_development_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let project_root = crate_dir.parent().unwrap_or(&crate_dir);
        self.switch_project(&project_root.to_string_lossy(), window, cx);
    }

    fn handle_git_fetch(_: &top_bar::GitFetch, _: &mut Window, _: &mut gpui::App) {
        println!("fetch");
    }

    fn handle_git_pull(_: &top_bar::GitPull, _: &mut Window, _: &mut gpui::App) {
        println!("pull");
    }

    fn handle_git_push(_: &top_bar::GitPush, _: &mut Window, _: &mut gpui::App) {
        println!("push");
    }

    fn handle_open_settings(_: &top_bar::OpenSettings, _: &mut Window, _: &mut gpui::App) {
        println!("设置");
    }

    fn handle_save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.focus_pane.clone() else {
            return;
        };
        let (editor, path) = {
            let pane = pane.read(cx);
            let Some(editor) = pane.active_editor(cx) else {
                return;
            };
            let Some(path) = pane.active_path(cx) else {
                return;
            };
            (editor, path)
        };
        let buffer = editor.read(cx).buffer();

        let result = buffer.update(cx, |buffer, cx| {
            let result = write_buffer_to_path(buffer, &path);
            if result.is_ok() {
                cx.notify();
            }
            result
        });
        if let Err(error) = result {
            eprintln!("保存文件失败（{}）：{error}", path.display());
        }
    }

    /// 按 action 名查找面板在注册表中的 index。
    fn panel_index(&self, action_name: &str) -> Option<usize> {
        self.panel_handles
            .iter()
            .position(|h| h.action_name() == action_name)
    }

    /// 切换面板焦点：若面板已聚焦则隐藏并退焦到编辑区，否则显示并聚焦。
    fn toggle_panel_focus(
        &mut self,
        index: usize,
        focus_handle: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if focus_handle.contains_focused(window, cx) {
            self.layout.borrow_mut().toggle_panel(index, window, cx);
            self.focus_center_pane(window, cx);
        } else {
            if !self.layout.borrow().is_panel_active(index) {
                self.layout.borrow_mut().toggle_panel(index, window, cx);
            }
            window.focus(focus_handle);
        }
        window.refresh();
    }

    /// 聚焦回编辑区。
    fn focus_center_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ref pane_entity) = self.focus_pane {
            let pane = pane_entity.read(cx);
            if let Some(editor) = pane.active_editor(cx) {
                window.focus(&editor.read(cx).focus_handle());
            } else {
                window.focus(&pane.focus);
            }
        }
    }

    /// 注册焦点监听：当指定 Pane 或其子元素获得焦点时更新 StatusBar。
    pub(crate) fn register_pane_focus_listener(
        &mut self,
        pane: &Entity<Pane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = pane.read(cx).focus.clone();
        let pane_entity = pane.clone();
        let sub = cx.on_focus_in(&focus, window, move |this, _window, cx| {
            this.handle_pane_focused(&pane_entity, cx);
        });
        self._subscriptions.push(sub);

        let sub = cx.subscribe_in(pane, window, |_this, _emitter, event, _window, _cx| {
            let _ = event;
        });
        self._subscriptions.push(sub);
    }

    /// 当 Pane 获得焦点时更新 Workspace 和 StatusBar 的焦点 Pane。
    fn handle_pane_focused(&mut self, pane: &Entity<Pane>, cx: &mut Context<Self>) {
        self.focus_pane = Some(pane.clone());
        self.status_bar.update(cx, |bar, cx| {
            bar.set_active_pane(pane, cx);
        });
    }

    fn handle_close_tab(
        &mut self,
        _: &pane::CloseTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane_entity) = self.focus_pane.clone() else {
            return;
        };
        let pane_focus = pane_entity.read(cx).focus.clone();
        if let Some(view_id) = pane_entity.read(cx).active {
            pane_entity.update(cx, |pane, cx| {
                pane.close_tab(view_id);
                cx.emit(pane::PaneEvent::RemovedItem { view_id });
                cx.notify();
            });
            if let Some(editor) = pane_entity.read(cx).active_editor(cx) {
                window.focus(&editor.read(cx).focus_handle());
            } else {
                window.focus(&pane_focus);
            }
            window.refresh();
        }
    }

    fn handle_toggle_project_tree(
        &mut self,
        _: &ToggleProjectTree,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = self.project_tree.read(cx).focus.clone();
        if let Some(i) = self.panel_index("dock::ToggleProjectTree") {
            self.toggle_panel_focus(i, &focus, window, cx);
        }
    }

    fn handle_toggle_panel(
        &mut self,
        action_name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(i) = self.panel_index(action_name) {
            let was_active = self.layout.borrow().is_panel_active(i);
            self.layout.borrow_mut().toggle_panel(i, window, cx);
            if was_active {
                self.focus_center_pane(window, cx);
            } else if let Some(handle) = self.panel_handles.get(i) {
                let focus = handle.focus_handle(cx);
                window.focus(&focus);
            }
            window.refresh();
        }
    }

    fn handle_toggle_project_picker(
        &mut self,
        _: &project_picker::ToggleProjectPicker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.top_bar.update(cx, |bar, cx| {
            bar.project_picker.update(cx, |picker, cx| {
                picker.toggle(window, cx);
            });
        });
    }

    /// 切换到指定目录作为项目根目录。
    fn switch_project(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let root = PathBuf::from(path);
        let label = root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        self.project_tree.update(cx, |tree, cx| {
            tree.set_root(root.clone(), cx);
        });
        self.top_bar.update(cx, |bar, cx| {
            bar.project_picker.update(cx, |picker, _cx| {
                picker.set_current_label(label);
            });
        });
        recent_projects::add_to_recent(path);
        window.refresh();
    }
}

/// 工作台顶层框架组装。
fn render_frame(
    top_bar: &Entity<TopBar>,
    status_bar: &Entity<StatusBar>,
    layout: &LayoutSnapshot,
    panels: &[(Arc<dyn PanelHandle>, DockArea)],
    layout_ctrl: Rc<RefCell<LayoutController>>,
) -> Div {
    div()
        .relative()
        .flex()
        .flex_col()
        .size_full()
        .overflow_hidden()
        .bg(color::current().gray.s[1])
        .font(typography::ui_font())
        .text_size(typography::ui())
        .line_height(typography::ui())
        .text_color(color::current().gray.s[8])
        .child(top_bar.clone())
        .child(render_layout_body(layout, panels, layout_ctrl))
        .child(status_bar.clone())
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.layout.borrow().snapshot();

        let layout_clone1 = self.layout.clone();
        let layout_clone2 = self.layout.clone();

        // 收集面板 pairs 供渲染
        let pairs: Vec<(Arc<dyn PanelHandle>, DockArea)> = self
            .layout
            .borrow()
            .panels
            .iter()
            .map(|(h, a)| (h.clone(), *a))
            .collect();

        div()
            .id("app-view")
            .track_focus(&self.focus)
            .key_context("Workspace")
            .size_full()
            .relative()
            .child(render_frame(
                &self.top_bar,
                &self.status_bar,
                &snapshot,
                &pairs,
                self.layout.clone(),
            ))
            .on_action(window_controls::handle_quit)
            .on_action(window_controls::handle_minimize)
            .on_action(window_controls::handle_toggle_maximize)
            .on_action(cx.listener(Self::handle_close_tab))
            .on_action(Self::handle_git_fetch)
            .on_action(Self::handle_git_pull)
            .on_action(Self::handle_git_push)
            .on_action(Self::handle_open_settings)
            .on_action(cx.listener(Self::handle_save))
            .on_action(cx.listener(Self::handle_toggle_project_tree))
            .on_action(cx.listener(
                |this: &mut Workspace, _: &ToggleVersionControl, window, cx| {
                    this.handle_toggle_panel("dock::ToggleVersionControl", window, cx);
                },
            ))
            .on_action(
                cx.listener(|this: &mut Workspace, _: &ToggleOutline, window, cx| {
                    this.handle_toggle_panel("dock::ToggleOutline", window, cx);
                }),
            )
            .on_action(
                cx.listener(|this: &mut Workspace, _: &ToggleTerminal, window, cx| {
                    this.handle_toggle_panel("dock::ToggleTerminal", window, cx);
                }),
            )
            .on_action(
                cx.listener(|this: &mut Workspace, _: &ToggleDebug, window, cx| {
                    this.handle_toggle_panel("dock::ToggleDebug", window, cx);
                }),
            )
            .on_action(cx.listener(
                |this: &mut Workspace, _: &ToggleKeyboardShortcuts, window, cx| {
                    this.handle_toggle_panel("dock::ToggleKeyboardShortcuts", window, cx);
                },
            ))
            .on_action(cx.listener(Self::handle_toggle_project_picker))
            .on_mouse_move(move |event, window, _cx| {
                let mut ctrl = layout_clone1.borrow_mut();
                if ctrl.is_dragging() {
                    ctrl.drag_to(event.position, window.bounds().size);
                    window.refresh();
                }
            })
            .on_mouse_up(MouseButton::Left, move |_event, window, _cx| {
                layout_clone2.borrow_mut().end_drag();
                window.refresh();
            })
    }
}

fn write_buffer_to_path(buffer: &mut Buffer, path: &Path) -> Result<(), BufferSaveError> {
    let version = buffer.version();
    let mut file = File::create(path)?;
    buffer.write_to(version, &mut file)?;
    file.sync_all()?;
    buffer.mark_saved();
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use zcv_engine::{BufferConfig, ByteOffset};

    use super::*;

    #[test]
    fn saving_buffer_writes_current_version_and_marks_it_clean() {
        let path = test_file_path();
        let mut buffer =
            Buffer::scratch("旧内容".to_owned(), BufferConfig::default()).expect("应创建 Buffer");
        buffer
            .insert(buffer.len_bytes(), " + 新内容")
            .expect("测试编辑应成功");
        assert!(buffer.is_dirty());

        write_buffer_to_path(&mut buffer, &path).expect("保存应成功");

        assert_eq!(
            fs::read_to_string(&path).expect("应读回文件"),
            "旧内容 + 新内容"
        );
        assert!(!buffer.is_dirty());
        fs::remove_file(path).expect("测试文件应可删除");
    }

    #[test]
    fn failed_save_keeps_buffer_dirty() {
        let path = test_file_path().join("missing.txt");
        let mut buffer =
            Buffer::scratch("内容".to_owned(), BufferConfig::default()).expect("应创建 Buffer");
        buffer
            .insert(ByteOffset::ZERO, "未保存")
            .expect("测试编辑应成功");

        assert!(write_buffer_to_path(&mut buffer, &path).is_err());
        assert!(buffer.is_dirty());
    }

    fn test_file_path() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("系统时间应晚于 Unix Epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "zcv-workspace-save-{}-{nonce}.txt",
            std::process::id()
        ))
    }
}
