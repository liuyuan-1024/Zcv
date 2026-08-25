//! ProjectPicker —— 项目选择器。
//!
//! 自含 glyph 按钮 + 浮层，浮层内嵌 `Picker<ProjectPickerDelegate>`。
//! glyph 内联在布局中，浮层用 deferred + anchored 逃逸。
//!
//! 最近项目从 `~/.zcv/recent_projects.json` 读取，"打开本地项目"调用系统文件选择器选择目录。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    Action, App, ClickEvent, Context, Entity, PathPromptOptions, Render, Window, div, prelude::*,
};
use zcv_actions::{DeleteRecentProject, OpenLocalProject, ToggleProjectPicker};
use zcv_keymap::KeyBindings;
use zcv_picker::{PICKER_WIDTH, Picker, PickerDelegate, PickerHost, picker_divider};
use zcv_theme::{color, typography};
use zcv_ui::Button;
use zcv_ui::ListItem;

use crate::recent_projects::{self, ProjectEntry};

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
        // 列表第一位即最近打开的项目，作为默认选中项
        let selected_index = 0;
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
                    p.label().to_lowercase().contains(&q) || p.path.to_lowercase().contains(&q)
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
        // 列表第一位即最近打开的项目，作为默认选中项
        self.selected_index = 0;
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
                    .child(entry.label()),
            )
            .subtitle(entry.path.clone())
            .end_slot(
                Button::icon(("delete-project", index), "icons/trash.svg")
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
            .and_then(|kb| kb.display_shortcut_named(OpenLocalProject.name()));
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
    host: PickerHost,
    picker: Entity<Picker<ProjectPickerDelegate>>,
    /// 异步「打开本地项目」暂存的路径
    pending_path: Rc<RefCell<Option<String>>>,
    /// 项目选中回调
    on_selected: OnProjectSelected,
    /// 当前项目名称（glyph 上显示）
    current_label: String,
}

impl ProjectPicker {
    pub fn new(
        on_selected: OnProjectSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let projects = recent_projects::load_recent_projects();
        // 列表第一位即最近打开的项目，作为顶栏显示名
        let current_label = projects.first().map(|p| p.label()).unwrap_or_default();
        let delegate = ProjectPickerDelegate::new(projects, on_selected.clone());
        let pending_path = Rc::new(RefCell::new(None));

        let picker = cx.new(|cx| Picker::new(delegate, PICKER_WIDTH, window, cx));
        let host = PickerHost::new(cx.focus_handle());
        picker.update(cx, |picker, _| {
            picker.set_on_dismiss(host.on_dismiss_handler())
        });

        Self {
            host,
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
        if !self.host.is_open() {
            // 打开时从磁盘重新加载最近项目列表
            self.picker.update(cx, |picker, cx| {
                picker.delegate_mut().reload_projects();
                // 清空搜索框文字
                if let Some(input) = picker.search_input() {
                    input.set_text("", cx);
                }
                cx.notify();
            });
            // 同步 glyph 上显示的当前项目名
            let delegate = self.picker.read(cx).delegate();
            if let Some(entry) = delegate.projects.first() {
                self.current_label = entry.label();
            }
        }
        self.host.toggle(&self.picker, window, cx);
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
        if self.host.consume_dismiss() {
            self.host.close_and_refocus(window);
        }

        // 处理异步「打开本地项目」返回的路径
        if let Some(path) = self.pending_path.borrow_mut().take() {
            self.host.close_and_refocus(window);
            // 从路径提取项目名
            if let Some(file_name) = std::path::Path::new(&path).file_name() {
                self.current_label = file_name.to_string_lossy().to_string();
            }
            let cb = self.on_selected.clone();
            window.defer(cx, move |window, cx| cb(path, window, cx));
        }

        let color_value = if self.host.is_open() {
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

        let glyph = Button::text("project-picker", glyph_text.to_string())
            .label("项目选择器")
            .shortcut(&ToggleProjectPicker, cx)
            .color(color_value)
            .on_click(|_, window, cx| {
                window.dispatch_action(Box::new(ToggleProjectPicker), cx);
            });

        let mut root = div()
            .track_focus(&self.host.focus_handle())
            // 复合 context 让 Picker 分组的快捷键与 Editor 同深度竞争
            .key_context("ProjectPicker")
            .on_action(cx.listener(Self::handle_toggle))
            .on_action(cx.listener(Self::handle_delete_recent))
            .on_action(cx.listener(Self::handle_open_local_project))
            .relative()
            .child(glyph);

        // 浮层
        if self.host.is_open() {
            root = root.child(self.host.overlay(window, cx, &self.picker));
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
                path: "/tmp/test-project".into(),
            }],
            on_selected,
        );
        let window = cx.add_window(|_window, _cx| TestView);
        let _ = window.update(cx, |_, window, cx| {
            delegate.confirm(window, cx);
        });
        assert_eq!(triggered.take().as_deref(), Some("/tmp/test-project"));
    }

    /// 构造 3 个项目的数据源，默认选中第一项。
    fn test_delegate() -> ProjectPickerDelegate {
        let on_selected: OnProjectSelected = Rc::new(|_, _, _| {});
        ProjectPickerDelegate::new(
            vec![
                ProjectEntry {
                    path: "/tmp/a".into(),
                },
                ProjectEntry {
                    path: "/tmp/b".into(),
                },
                ProjectEntry {
                    path: "/tmp/c".into(),
                },
            ],
            on_selected,
        )
    }

    #[test]
    fn remove_project_drops_entry_and_keeps_filter() {
        let mut delegate = test_delegate();
        delegate.update_matches("tmp".into());
        delegate.remove_project_in_memory(2);
        assert_eq!(delegate.projects.len(), 2);
        assert!(delegate.projects.iter().all(|p| p.path != "/tmp/c"));
        assert_eq!(delegate.filtered, vec![0, 1]);
    }

    #[test]
    fn remove_selected_project_selects_the_next_entry() {
        let mut delegate = test_delegate();
        delegate.selected_index = 1;
        delegate.remove_project_in_memory(1);
        assert_eq!(delegate.projects.len(), 2);
        assert_eq!(delegate.selected_index, 1);
        assert_eq!(delegate.projects[delegate.selected_index].label(), "c");
    }

    #[test]
    fn remove_last_project_clamps_selection() {
        let mut delegate = test_delegate();
        delegate.selected_index = 1;
        delegate.remove_project_in_memory(2);
        assert_eq!(delegate.selected_index, 1);
        assert_eq!(delegate.projects[delegate.selected_index].label(), "b");
    }
}
