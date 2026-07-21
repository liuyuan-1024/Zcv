//! ProjectPicker —— 项目选择器浮面。
//!
//! 验证 Picker + Surface 全链路的试点。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{AnyElement, App, Entity, Global, Window, div, prelude::*, px};

use crate::editor::EditableText;
use crate::shared::picker::{
    PickerDelegate, picker_footer, picker_footer_row, picker_row, picker_search_box,
    picker_two_line, render_picker,
};
use crate::surface::{SurfaceAnchor, SurfaceRequest};
use crate::theme::color;

/// 项目选择器搜索框编辑器全局句柄。
pub(crate) struct ProjectSearchEditor(pub(crate) Entity<EditableText>);
impl Global for ProjectSearchEditor {}

/// 项目条目。
struct ProjectEntry {
    id: String,
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
    fn placeholder(&self) -> &str {
        "搜索项目..."
    }

    fn query(&self) -> &str {
        &self.query
    }

    fn set_query(&mut self, query: &str) {
        self.query = query.to_string();
        self.do_filter();
    }

    fn item_count(&self) -> usize {
        self.filtered.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn move_selection(&mut self, delta: isize) {
        let count = self.filtered.len();
        if count == 0 {
            return;
        }
        let new = self.selected_index as isize + delta;
        self.selected_index = new.rem_euclid(count as isize) as usize;
    }

    fn confirm(&self, _window: &mut Window, _cx: &mut App) {
        if self.filtered.is_empty() {
            return;
        }
        let entry = &self.projects[self.filtered[self.selected_index]];
        println!("打开项目: {} ({})", entry.label, entry.path);
    }

    fn render_item(&self, index: usize, is_selected: bool) -> AnyElement {
        let entry = &self.projects[self.filtered[index]];
        let prefix = if entry.is_current { "✓ " } else { "  " };
        picker_row(
            picker_two_line(
                div()
                    .text_color(color::current().gray.s[8])
                    .child(format!("{}{}", prefix, entry.label)),
                entry.path.clone(),
            ),
            is_selected,
        )
    }

    fn render_footer(&self) -> Option<AnyElement> {
        Some(
            picker_footer(vec![
                picker_footer_row("打开本地项目"),
                picker_footer_row("克隆 Git 仓库"),
            ])
            .into_any_element(),
        )
    }
}

// ── 公共 API ──

pub(crate) fn open(anchor: SurfaceAnchor, window: &mut Window, cx: &mut gpui::App) {
    let projects = mock_projects();
    let delegate = Rc::new(RefCell::new(ProjectPickerDelegate::new(projects)));
    let render_delegate = Rc::clone(&delegate);

    let Some(editor) = cx.try_global::<ProjectSearchEditor>().map(|g| g.0.clone()) else {
        return;
    };

    // 编辑器文本变更 → 更新 delegate 的搜索过滤
    {
        let delegate = Rc::clone(&delegate);
        editor.update(cx, |e, _cx| {
            e.set_on_change(move |text, _window, _cx| {
                delegate.borrow_mut().set_query(text);
            });
        });
    }

    let focus = editor.update(cx, |e, _cx| e.focus_handle());
    let editor_entity = editor.clone();

    let request = SurfaceRequest {
        id: crate::surface::SurfaceId::ProjectPicker,
        anchor,
        focus_on_open: Some(focus),
        render: Rc::new(move || {
            render_picker(
                px(420.0),
                render_delegate.clone(),
                picker_search_box(editor_entity.clone()),
            )
        }),
    };

    crate::surface::open_surface(request, window, cx);
}

fn mock_projects() -> Vec<ProjectEntry> {
    vec![
        ProjectEntry {
            id: "1".into(),
            label: "zcv".into(),
            path: "/Users/liuyuan/project/liuyuan/zcv".into(),
            is_current: true,
        },
        ProjectEntry {
            id: "2".into(),
            label: "zed".into(),
            path: "/Users/liuyuan/code/zed".into(),
            is_current: false,
        },
        ProjectEntry {
            id: "3".into(),
            label: "gpui".into(),
            path: "/Users/liuyuan/code/gpui".into(),
            is_current: false,
        },
        ProjectEntry {
            id: "4".into(),
            label: "zcv".into(),
            path: "/Users/liuyuan/project/zcv".into(),
            is_current: false,
        },
        ProjectEntry {
            id: "5".into(),
            label: "rust-analyzer".into(),
            path: "/Users/liuyuan/code/rust-analyzer".into(),
            is_current: false,
        },
    ]
}
