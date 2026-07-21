//! BranchPicker —— 分支选择器浮面。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{AnyElement, App, Window, div, prelude::*, px};

use crate::shared::picker::{PickerDelegate, picker_row, picker_search_box, render_picker};
use crate::surface::{SurfaceAnchor, SurfaceRequest};

/// 分支选择器数据源。
struct BranchPickerDelegate {
    query: String,
    branches: Vec<(String, bool)>,
    filtered: Vec<usize>,
    selected_index: usize,
}

impl BranchPickerDelegate {
    fn new(branches: Vec<(String, bool)>) -> Self {
        let filtered: Vec<usize> = (0..branches.len()).collect();
        let selected = branches.iter().position(|(_, cur)| *cur).unwrap_or(0);
        Self {
            query: String::new(),
            branches,
            filtered,
            selected_index: selected,
        }
    }

    fn do_filter(&mut self) {
        if self.query.is_empty() {
            self.filtered = (0..self.branches.len()).collect();
        } else {
            let q = self.query.to_lowercase();
            self.filtered = self
                .branches
                .iter()
                .enumerate()
                .filter(|(_, (name, _))| name.to_lowercase().contains(&q))
                .map(|(i, _)| i)
                .collect();
        }
        self.selected_index = self
            .selected_index
            .min(self.filtered.len().saturating_sub(1));
    }
}

impl PickerDelegate for BranchPickerDelegate {
    fn placeholder(&self) -> &str {
        "搜索分支..."
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
        println!(
            "切换分支: {}",
            self.branches[self.filtered[self.selected_index]].0
        );
    }

    fn render_item(&self, index: usize, is_selected: bool) -> AnyElement {
        let (name, is_current) = &self.branches[self.filtered[index]];
        let prefix = if *is_current { "✓ " } else { "  " };
        picker_row(div().child(format!("{}{}", prefix, name)), is_selected)
    }
}

// ── 公共 API ──

pub(crate) fn open(anchor: SurfaceAnchor, window: &mut Window, cx: &mut gpui::App) {
    let branches = mock_branches();
    let delegate = Rc::new(RefCell::new(BranchPickerDelegate::new(branches)));
    let render_delegate = Rc::clone(&delegate);

    let request = SurfaceRequest {
        id: crate::surface::SurfaceId::BranchPicker,
        anchor,
        focus_on_open: None,
        render: Rc::new(move || {
            render_picker(px(320.0), render_delegate.clone(), picker_search_box(div()))
        }),
    };

    crate::surface::open_surface(request, window, cx);
}

fn mock_branches() -> Vec<(String, bool)> {
    vec![
        ("main".into(), true),
        ("develop".into(), false),
        ("feature/surface".into(), false),
        ("feature/picker".into(), false),
        ("fix/keyboard".into(), false),
        ("release/v1.0".into(), false),
    ]
}
