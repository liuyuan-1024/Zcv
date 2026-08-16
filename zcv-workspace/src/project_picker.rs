//! ProjectPicker —— 项目选择器。
//!
//! 自含 glyph 按钮 + 浮层，浮层内嵌 `Picker<ProjectPickerDelegate>`。
//! glyph 内联在布局中，浮层用 deferred + anchored 逃逸。
//!
//! 最近项目从 `~/.zcv/recent_projects.json` 读取，"打开本地项目"调用系统文件选择器选择目录。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{
    Action, App, ClickEvent, Context, Corner, Entity, FocusHandle, Focusable, MouseButton,
    PathPromptOptions, Pixels, Render, Window, anchored, deferred, div, point, prelude::*, px,
};
use zcv_actions::{DeleteRecentProject, OpenLocalProject, ToggleProjectPicker};
use zcv_keymap::KeyBindings;
use zcv_picker::{Picker, PickerDelegate, picker_divider};
use zcv_theme::{color, space, typography};
use zcv_ui::Glyph;
use zcv_ui::ListItem;

use crate::recent_projects::{self, ProjectEntry};

const PICKER_WIDTH: Pixels = px(360.0);

// ═══ 回调 ════════════════════════════════════════════════════════

/// 项目选中回调 —— 参数为项目路径。
pub type OnProjectSelected = Rc<dyn Fn(String, &mut Window, &mut App)>;

// ═══ 数据源 ═══════════════════════════════════════════════════════

/// 项目选择器数据源。
struct ProjectPickerDelegate {
    query: String,
    projects: Vec<ProjectEntry>,
    filtered: Vec<usize>,
    selected_index: usize,
    on_selected: OnProjectSelected,
}

impl ProjectPickerDelegate {
    fn new(projects: Vec<ProjectEntry>, on_selected: OnProjectSelected) -> Self {
        let filtered: Vec<usize> = (0..projects.len()).collect();
        let selected_index = projects.iter().position(|p| p.is_current).unwrap_or(0);
        Self {
            query: String::new(),
            projects,
            filtered,
            selected_index,
            on_selected,
        }
    }

    fn do_filter(&mut self) {
        if self.query.is_empty() {
            self.filtered = (0..self.projects.len()).collect();
        } else {
            let q = self.query.to_lowercase();
            self.filtered = self
                .projects
                .iter()
                .enumerate()
                .filter(|(_, p)| {
                    p.label.to_lowercase().contains(&q) || p.path.to_lowercase().contains(&q)
                })
                .map(|(i, _)| i)
                .collect();
        }
        self.selected_index = self
            .selected_index
            .min(self.filtered.len().saturating_sub(1));
    }

    /// 删除 filtered 索引对应的最近项目（落盘 + 内存）。
    fn remove_project(&mut self, ix: usize) {
        let project_ix = self.filtered[ix];
        let path = self.projects[project_ix].path.clone();
        recent_projects::remove_from_recent(&path);
        self.remove_project_in_memory(project_ix);
    }

    /// 从内存列表移除一个项目，并重算过滤结果与选中项。
    fn remove_project_in_memory(&mut self, project_ix: usize) {
        self.projects.remove(project_ix);
        self.do_filter();
    }

    /// 从磁盘重新加载最近项目列表，保留当前搜索 query。
    fn reload_projects(&mut self) {
        self.projects = recent_projects::load_recent_projects();
        if self.projects.is_empty() {
            // 回到退路：当前工作目录
            if let Ok(cwd) = std::env::current_dir() {
                let label = cwd
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                if !label.is_empty() {
                    self.projects.push(ProjectEntry {
                        label,
                        path: cwd.to_string_lossy().to_string(),
                        is_current: true,
                    });
                }
            }
        }
        // 选中当前项目
        self.selected_index = self.projects.iter().position(|p| p.is_current).unwrap_or(0);
        // 重新应用过滤
        self.do_filter();
    }
}

impl PickerDelegate for ProjectPickerDelegate {
    fn match_count(&self) -> usize {
        self.filtered.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(&mut self, ix: usize) {
        self.selected_index = ix;
    }

    fn update_matches(&mut self, query: String) {
        self.query = query;
        self.do_filter();
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut App) {
        if self.filtered.is_empty() {
            return;
        }
        let entry = &self.projects[self.filtered[self.selected_index]];
        // 由回调（Workspace::switch_project）负责持久化最近项目
        let cb = self.on_selected.clone();
        cb(entry.path.clone(), window, cx);
    }

    fn dismissed(&mut self) {}

    fn render_match(
        &self,
        index: usize,
        is_selected: bool,
        cx: &mut Context<Picker<Self>>,
    ) -> gpui::AnyElement {
        let entry = &self.projects[self.filtered[index]];
        let icon_color = color::current(cx).icon_muted;
        let remove = cx.listener(move |this, _: &ClickEvent, window, cx| {
            // 阻止冒泡，避免触发所在行的打开项目行为
            cx.stop_propagation();
            this.delegate_mut().remove_project(index);
            cx.notify();
            window.refresh();
        });
        ListItem::new(index)
            .toggle_state(is_selected)
            .child(
                div()
                    .text_color(color::current(cx).text)
                    .child(entry.label.clone()),
            )
            .subtitle(entry.path.clone())
            .end_slot(
                Glyph::icon(("delete-project", index), "icons/trash.svg")
                    .color(icon_color)
                    .label("移除")
                    .shortcut(&DeleteRecentProject, cx)
                    .on_click(remove),
            )
            .into_any_element()
    }

    fn placeholder_text(&self) -> &str {
        "搜索项目..."
    }
    fn render_footer(&self, _window: &mut Window, cx: &mut App) -> Option<gpui::AnyElement> {
        let shortcut = cx
            .try_global::<KeyBindings>()
            .and_then(|kb| kb.display_shortcut(OpenLocalProject.name()));
        let item = ListItem::new("open-local").child("打开本地项目");
        let item = if let Some(s) = shortcut {
            item.end_slot(
                div()
                    .text_color(color::current(cx).text_placeholder)
                    .text_size(typography::ui())
                    .child(s),
            )
        } else {
            item
        };
        // 整个 footer 区域可点击，派发 OpenLocalProject action
        Some(
            div()
                .id("open-local-footer")
                .child(picker_divider(cx))
                .child(item)
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(OpenLocalProject), cx);
                })
                .into_any_element(),
        )
    }
}

// ═══ Entity ═════════════════════════════════════════════════════

/// 项目选择器 —— 自含 glyph 按钮 + 浮层。
///
/// glyph 显示当前项目名称，无项目时显示「选择项目」。
pub struct ProjectPicker {
    is_open: bool,
    dismiss_flag: Rc<Cell<bool>>,
    focus: FocusHandle,
    picker: Entity<Picker<ProjectPickerDelegate>>,
    /// 异步「打开本地项目」暂存的路径
    pending_path: Rc<RefCell<Option<String>>>,
    /// 项目选中回调
    on_selected: OnProjectSelected,
    /// 当前项目名称（glyph 上显示）
    current_label: String,
}

impl ProjectPicker {
    fn load_projects() -> Vec<ProjectEntry> {
        let mut projects = recent_projects::load_recent_projects();
        // 最近列表为空时，将当前工作目录作为候选
        if projects.is_empty()
            && let Ok(cwd) = std::env::current_dir()
        {
            let label = cwd
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if !label.is_empty() {
                projects.push(ProjectEntry {
                    label,
                    path: cwd.to_string_lossy().to_string(),
                    is_current: true,
                });
            }
        }
        projects
    }

    pub fn new(on_selected: OnProjectSelected, cx: &mut Context<Self>) -> Self {
        let projects = Self::load_projects();
        let current_label = projects
            .iter()
            .find(|p| p.is_current)
            .map(|p| p.label.clone())
            .unwrap_or_default();
        let delegate = ProjectPickerDelegate::new(projects, on_selected.clone());
        let dismiss_flag = Rc::new(Cell::new(false));
        let pending_path = Rc::new(RefCell::new(None));

        let picker = cx.new(|cx| Picker::new(delegate, PICKER_WIDTH, cx));
        let on_dismiss = {
            let df = dismiss_flag.clone();
            Box::new(move |window: &mut Window, _app: &mut App| {
                df.set(true);
                window.refresh();
            })
        };
        picker.update(cx, |picker, cx| {
            picker.init(cx);
            picker.set_on_dismiss(on_dismiss);
        });

        Self {
            is_open: false,
            dismiss_flag,
            focus: cx.focus_handle(),
            picker,
            pending_path,
            on_selected,
            current_label,
        }
    }

    /// 设置当前项目名称。
    pub fn set_current_label(&mut self, label: impl Into<String>) {
        self.current_label = label.into();
    }

    /// 外部切换（快捷键/按钮等）。
    pub fn toggle(&mut self, window: &mut Window, cx: &mut App) {
        self.dismiss_flag.set(false);
        self.is_open = !self.is_open;
        if self.is_open {
            // 打开时从磁盘重新加载最近项目列表
            self.picker.update(cx, |picker, cx| {
                picker.delegate_mut().reload_projects();
                // 清空搜索框文字
                picker.search_input().update(cx, |input, cx| {
                    input.set_text("", cx);
                });
                cx.notify();
            });
            // 同步 glyph 上显示的当前项目名
            let delegate = self.picker.read(cx).delegate();
            if let Some(entry) = delegate.projects.iter().find(|p| p.is_current) {
                self.current_label = entry.label.clone();
            }
            let input = self.picker.read(cx).search_input().clone();
            let focus = input.read(cx).focus_handle(cx);
            window.focus(&focus);
        } else {
            window.focus(&self.focus);
        }
        window.refresh();
    }

    fn handle_toggle(
        &mut self,
        _: &ToggleProjectPicker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle(window, cx);
    }

    /// 删除当前选中的最近项目（快捷键 cmd-backspace，仅 picker 打开时绑定生效）。
    fn handle_delete_recent(
        &mut self,
        _: &DeleteRecentProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picker.update(cx, |picker, cx| {
            if picker.delegate().match_count() == 0 {
                return;
            }
            let ix = picker.delegate().selected_index();
            picker.delegate_mut().remove_project(ix);
            cx.notify();
        });
        window.refresh();
    }

    fn handle_open_local_project(
        &mut self,
        _: &OpenLocalProject,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pending = self.pending_path.clone();

        // 同步触发系统文件选择器，返回一个异步 channel
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("选择项目目录".into()),
        });

        // 通过 foreground executor 处理异步结果
        cx.foreground_executor()
            .spawn(async move {
                if let Ok(inner) = rx.await {
                    match inner {
                        Ok(Some(paths)) => {
                            if let Some(path) = paths.first() {
                                *pending.borrow_mut() = Some(path.to_string_lossy().to_string());
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            eprintln!("文件选择器出错: {e}");
                        }
                    }
                } // Err(_) → channel 被关闭（取消），静默忽略
            })
            .detach();
    }
}

impl Render for ProjectPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 检查是否需要关闭（Escape / 点击外部）
        if self.dismiss_flag.replace(false) {
            self.is_open = false;
            window.focus(&self.focus);
        }

        // 处理异步「打开本地项目」返回的路径
        if let Some(path) = self.pending_path.borrow_mut().take() {
            self.is_open = false;
            window.focus(&self.focus);
            // 从路径提取项目名
            if let Some(file_name) = std::path::Path::new(&path).file_name() {
                self.current_label = file_name.to_string_lossy().to_string();
            }
            let cb = self.on_selected.clone();
            window.defer(cx, move |window, cx| cb(path, window, cx));
        }

        let color_value = if self.is_open {
            color::current(cx).icon_accent
        } else {
            color::current(cx).text
        };

        // glyph 上显示当前项目名称，没有时显示「选择项目」
        let glyph_text: &str = if self.current_label.is_empty() {
            "选择项目"
        } else {
            &self.current_label
        };

        let glyph = Glyph::text("project-picker", glyph_text.to_string())
            .label("项目选择器")
            .shortcut(&ToggleProjectPicker, cx)
            .color(color_value)
            .on_click(|_, window, cx| {
                window.dispatch_action(Box::new(ToggleProjectPicker), cx);
            });

        let mut root = div()
            .track_focus(&self.focus)
            // 复合 context 让 Picker 分组的快捷键与 Editor 同深度竞争
            .key_context("ProjectPicker")
            .on_action(cx.listener(Self::handle_toggle))
            .on_action(cx.listener(Self::handle_delete_recent))
            .on_action(cx.listener(Self::handle_open_local_project))
            .relative()
            .child(glyph);

        // 浮层
        if self.is_open {
            let dismiss = self.dismiss_flag.clone();
            let win_size = window.bounds().size;

            // 全屏点击拦截（优先级 0，垫底）
            root = root
                .child(
                    deferred(
                        div()
                            .absolute()
                            .top(Pixels::ZERO)
                            .left(Pixels::ZERO)
                            .w(win_size.width)
                            .h(win_size.height)
                            .occlude()
                            .on_mouse_down(MouseButton::Left, move |_, window, _cx| {
                                dismiss.set(true);
                                window.refresh();
                            }),
                    )
                    .with_priority(0),
                )
                // Picker 浮层（优先级 1，Local 定位到 glyph 旁边）
                .child(
                    deferred(
                        anchored()
                            .anchor(Corner::TopLeft)
                            .position(point(Pixels::ZERO, Pixels::ZERO))
                            .position_mode(gpui::AnchoredPositionMode::Local)
                            .snap_to_window_with_margin(space::S6)
                            .child(
                                div()
                                    .occlude()
                                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .child(
                                        div()
                                            .bg(color::current(cx).elevated_surface_background)
                                            .border_l_3()
                                            .border_color(color::current(cx).border_focused)
                                            .border_1()
                                            .border_color(color::current(cx).border_variant)
                                            .rounded(px(8.0))
                                            .overflow_hidden()
                                            .child(self.picker.clone()),
                                    ),
                            ),
                    )
                    .with_priority(1),
                );
        }

        root
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use gpui::{Context, div, prelude::*};

    use super::*;

    #[derive(Default)]
    struct TestView;

    impl Render for TestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[gpui::test]
    fn confirm_invokes_on_selected(cx: &mut gpui::TestAppContext) {
        let triggered = Rc::new(Cell::new(None::<String>));
        let on_selected: OnProjectSelected = {
            let triggered = triggered.clone();
            Rc::new(move |path, _window, _cx| triggered.set(Some(path)))
        };
        let mut delegate = ProjectPickerDelegate::new(
            vec![ProjectEntry {
                label: "测试项目".into(),
                path: "/tmp/test-project".into(),
                is_current: true,
            }],
            on_selected,
        );
        let window = cx.add_window(|_window, _cx| TestView);
        let _ = window.update(cx, |_, window, cx| {
            delegate.confirm(window, cx);
        });
        assert_eq!(triggered.take().as_deref(), Some("/tmp/test-project"));
    }

    /// 构造 3 个项目的数据源，第 2 个是当前项目。
    fn test_delegate() -> ProjectPickerDelegate {
        let on_selected: OnProjectSelected = Rc::new(|_, _, _| {});
        ProjectPickerDelegate::new(
            vec![
                ProjectEntry {
                    label: "项目A".into(),
                    path: "/tmp/a".into(),
                    is_current: false,
                },
                ProjectEntry {
                    label: "项目B".into(),
                    path: "/tmp/b".into(),
                    is_current: true,
                },
                ProjectEntry {
                    label: "项目C".into(),
                    path: "/tmp/c".into(),
                    is_current: false,
                },
            ],
            on_selected,
        )
    }

    #[test]
    fn remove_project_drops_entry_and_keeps_filter() {
        let mut delegate = test_delegate();
        delegate.update_matches("项目".into());
        delegate.remove_project_in_memory(2);
        assert_eq!(delegate.projects.len(), 2);
        assert!(delegate.projects.iter().all(|p| p.path != "/tmp/c"));
        assert_eq!(delegate.filtered, vec![0, 1]);
    }

    #[test]
    fn remove_selected_project_selects_the_next_entry() {
        let mut delegate = test_delegate();
        assert_eq!(delegate.selected_index, 1);
        delegate.remove_project_in_memory(1);
        assert_eq!(delegate.projects.len(), 2);
        assert_eq!(delegate.selected_index, 1);
        assert_eq!(delegate.projects[delegate.selected_index].label, "项目C");
    }

    #[test]
    fn remove_last_project_clamps_selection() {
        let mut delegate = test_delegate();
        delegate.remove_project_in_memory(2);
        assert_eq!(delegate.selected_index, 1);
        assert_eq!(delegate.projects[delegate.selected_index].label, "项目B");
    }
}
