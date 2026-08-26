//! 项目树键盘导航与行操作动作：选中移动、展开/折叠、删除。行级鼠标交互见 render.rs。

use std::collections::HashSet;
use std::path::PathBuf;

use gpui::{Context, Window};

use zcv_actions::{
    TreeCollapse, TreeExpand, TreeSelectNext, TreeSelectNextExtend, TreeSelectPrev,
    TreeSelectPrevExtend, TreeTrash,
};

use super::transfer::sanitize_selection;
use super::{ProjectTreePanel, ProjectTreeRow};

impl ProjectTreePanel {
    pub(super) fn handle_tree_select_prev(
        &mut self,
        _: &TreeSelectPrev,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.state.borrow_mut().select_up();
        self.scroll_to_selection();
        window.refresh();
    }

    pub(super) fn handle_tree_select_next(
        &mut self,
        _: &TreeSelectNext,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.state.borrow_mut().select_down();
        self.scroll_to_selection();
        window.refresh();
    }

    pub(super) fn handle_tree_select_prev_extend(
        &mut self,
        _: &TreeSelectPrevExtend,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        if self.state.borrow_mut().extend_up() {
            self.scroll_to_selection();
            window.refresh();
        }
    }

    pub(super) fn handle_tree_select_next_extend(
        &mut self,
        _: &TreeSelectNextExtend,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        if self.state.borrow_mut().extend_down() {
            self.scroll_to_selection();
            window.refresh();
        }
    }

    pub(super) fn handle_tree_collapse(
        &mut self,
        _: &TreeCollapse,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rebuild = self.state.borrow_mut().collapse_selection();
        if rebuild {
            self.rebuild_rows(cx);
        }
        self.scroll_to_selection();
        window.refresh();
    }

    pub(super) fn handle_tree_expand(
        &mut self,
        _: &TreeExpand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rebuild = self.state.borrow_mut().expand_selection();
        if rebuild {
            self.rebuild_rows(cx);
        }
        self.scroll_to_selection();
        window.refresh();
    }

    pub(super) fn handle_tree_trash(
        &mut self,
        _: &TreeTrash,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let (targets, neighbor_path) = {
            let state = self.state.borrow();
            let targets = sanitize_selection(state.effective_selection(), &root);
            // 集合为空且游标在根：sanitize 剔除根后为空，保持“根行不可删除”。
            if targets.is_empty() {
                return;
            }
            let target_set: HashSet<&PathBuf> = targets.iter().collect();
            let first_index = state
                .rows
                .iter()
                .position(|row| target_set.contains(&row.path));
            let neighbor_path = first_index.and_then(|index| {
                let survivors: Vec<&ProjectTreeRow> = state
                    .rows
                    .iter()
                    .filter(|row| !target_set.contains(&row.path))
                    .collect();
                survivors
                    .get(index)
                    .or_else(|| survivors.last())
                    .map(|row| row.path.clone())
            });
            (targets, neighbor_path)
        };
        let Some(on_trash) = self.on_trash.clone() else {
            eprintln!("项目树删除失败：未配置项目删除服务");
            return;
        };
        for path in &targets {
            if let Err(error) = on_trash(path.clone(), window, cx) {
                eprintln!("项目树删除失败：{error}");
            }
        }
        // 游标收拢到预计算的邻居路径；重建后路径仍在，replace_rows 保留选中。
        if let Some(neighbor) = neighbor_path {
            self.state.borrow_mut().selected = Some(neighbor);
        }
        self.rebuild_rows(cx);
    }
}
