//! ProjectPicker —— 项目选择器。
//!
//! 自含 glyph 按钮 + 浮层，浮层内嵌 `Picker<ProjectPickerDelegate>`。
//! glyph 内联在布局中，浮层用 deferred + anchored 逃逸。

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    Action, AnyElement, App, Context, Corner, Entity, FocusHandle, MouseButton, Render, Window,
    actions, anchored, deferred, div, point, prelude::*, px,
};

use crate::keymap::KeyBindings;
use crate::shared::Glyph;
use crate::shared::list_item::{ListItem, list_item_two_line};
use crate::shared::picker::{Picker, PickerDelegate, picker_divider};
use crate::theme::{color, space, typography};

actions!(project_picker, [TogglePicker, OpenLocalProject]);

// ═══ 数据 ════════════════════════════════════════════════════════

struct ProjectEntry {
    label: String,
    path: String,
    is_current: bool,
}

/// 项目选择器数据源。
struct ProjectPickerDelegate {
    query: String,
    projects: Vec<ProjectEntry>,
    filtered: Vec<usize>,
    selected_index: usize,
}

impl ProjectPickerDelegate {
    fn new(projects: Vec<ProjectEntry>) -> Self {
        let filtered: Vec<usize> = (0..projects.len()).collect();
        let selected = projects.iter().position(|p| p.is_current).unwrap_or(0);
        Self {
            query: String::new(),
            projects,
            filtered,
            selected_index: selected,
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

    fn confirm(&mut self, _window: &mut Window, _cx: &mut App) {
        if self.filtered.is_empty() {
            return;
        }
        let entry = &self.projects[self.filtered[self.selected_index]];
        println!("打开项目: {} ({})", entry.label, entry.path);
    }

    fn dismissed(&mut self) {}
    fn render_match(&self, index: usize, is_selected: bool) -> AnyElement {
        let entry = &self.projects[self.filtered[index]];
        ListItem::new(index)
            .toggle_state(is_selected)
            .child(list_item_two_line(
                div()
                    .text_color(color::current().gray.s[8])
                    .child(entry.label.clone()),
                entry.path.clone(),
            ))
            .into_any_element()
    }

    fn placeholder_text(&self) -> &str {
        "搜索项目..."
    }
    fn render_footer(&self, _window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        let shortcut = cx
            .try_global::<KeyBindings>()
            .and_then(|kb| kb.display_shortcut(OpenLocalProject.name()));
        let item = ListItem::new("open-local").child("打开本地项目");
        let item = if let Some(s) = shortcut {
            item.end_slot(
                div()
                    .text_color(color::current().gray.s[5])
                    .text_size(typography::ui())
                    .child(s),
            )
        } else {
            item
        };
        Some(div().child(picker_divider()).child(item).into_any_element())
    }
}

// ═══ Entity ═════════════════════════════════════════════════════

/// 项目选择器 —— 自含 glyph 按钮 + 浮层。
pub(crate) struct ProjectPicker {
    is_open: bool,
    dismiss_flag: Rc<Cell<bool>>,
    focus: FocusHandle,
    picker: Entity<Picker<ProjectPickerDelegate>>,
}

impl ProjectPicker {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let delegate = ProjectPickerDelegate::new(mock_projects());
        let dismiss_flag = Rc::new(Cell::new(false));

        let picker = cx.new(|cx| Picker::new(delegate, px(360.0), cx));
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
        }
    }

    /// 外部切换（快捷键/按钮等）。
    pub(crate) fn toggle(&mut self, window: &mut Window, cx: &mut App) {
        self.dismiss_flag.set(false);
        self.is_open = !self.is_open;
        if self.is_open {
            let editor = self.picker.read(cx).editor().clone();
            let focus = editor.update(cx, |e, _| e.focus_handle());
            window.focus(&focus);
        } else {
            window.focus(&self.focus);
        }
        window.refresh();
    }

    fn handle_toggle(&mut self, _: &TogglePicker, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle(window, cx);
    }

    fn handle_open_local_project(
        &mut self,
        _: &OpenLocalProject,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        println!("打开本地项目: 未实现");
    }
}

impl Render for ProjectPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 检查是否需要关闭（Escape / 点击外部）
        if self.dismiss_flag.replace(false) {
            self.is_open = false;
            window.focus(&self.focus);
        }

        let color_value = if self.is_open {
            color::glyph_active()
        } else {
            color::glyph_default()
        };

        let glyph = Glyph::text("project-picker", "打开项目")
            .label("项目选择器")
            .shortcut(&TogglePicker, cx)
            .color(color_value)
            .on_click(|window, cx| {
                window.dispatch_action(Box::new(TogglePicker), cx);
            });

        let mut root = div()
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::handle_toggle))
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
                            .top(px(0.0))
                            .left(px(0.0))
                            .w(win_size.width)
                            .h(win_size.height)
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
                            .position(point(px(0.0), px(0.0)))
                            .position_mode(gpui::AnchoredPositionMode::Local)
                            .snap_to_window_with_margin(space::S8)
                            .child(
                                div()
                                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .child(
                                        div()
                                            .border_l_3()
                                            .border_color(color::glyph_active())
                                            .border_1()
                                            .border_color(color::current().gray.s[4])
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

fn mock_projects() -> Vec<ProjectEntry> {
    vec![
        ProjectEntry {
            label: "zcv".into(),
            path: "/Users/liuyuan/project/liuyuan/zcv".into(),
            is_current: true,
        },
        ProjectEntry {
            label: "zed".into(),
            path: "/Users/liuyuan/code/zed".into(),
            is_current: false,
        },
        ProjectEntry {
            label: "gpui".into(),
            path: "/Users/liuyuan/code/gpui".into(),
            is_current: false,
        },
        ProjectEntry {
            label: "zcv".into(),
            path: "/Users/liuyuan/project/zcv".into(),
            is_current: false,
        },
        ProjectEntry {
            label: "rust-analyzer".into(),
            path: "/Users/liuyuan/code/rust-analyzer".into(),
            is_current: false,
        },
    ]
}
