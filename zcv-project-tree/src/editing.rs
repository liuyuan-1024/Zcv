//! 行名称编辑态：重命名与新建条目（名称编辑器、确认/取消、错误提示）。

use std::path::{Path, PathBuf};

use gpui::{Context, Window};

use zcv_actions::{TreeCancelEdit, TreeConfirmEdit, TreeNewEntry, TreeRename};
use zcv_project::{new_entry_destination, rename_destination};

use super::ProjectTreePanel;

#[derive(Clone, Debug)]
pub(super) struct EditState {
    pub(super) operation: EditOperation,
    pub(super) validation_error: Option<String>,
}

impl EditState {
    pub(super) fn matches_row(&self, row: &super::ProjectTreeRow) -> bool {
        match &self.operation {
            EditOperation::Rename { source, is_dir } => {
                !row.is_new && row.path == *source && row.is_dir == *is_dir
            }
            EditOperation::Create { parent } => row.is_new && row.path == *parent,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) enum EditOperation {
    Rename { source: PathBuf, is_dir: bool },
    Create { parent: PathBuf },
}

impl ProjectTreePanel {
    pub(super) fn handle_tree_rename(
        &mut self,
        _: &TreeRename,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.edit_state.is_some() {
            return;
        }
        let row = {
            let state = self.state.borrow();
            state.selected_idx().map(|index| state.rows[index].clone())
        };
        let Some(row) = row else {
            return;
        };

        let selection_end = if row.is_dir {
            row.name.len()
        } else {
            Path::new(&row.name)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map_or(row.name.len(), str::len)
        };
        self.begin_edit(
            EditOperation::Rename {
                source: row.path,
                is_dir: row.is_dir,
            },
            &row.name,
            0..selection_end,
            window,
            cx,
        );
    }

    pub(super) fn handle_tree_new_entry(
        &mut self,
        _: &TreeNewEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.begin_create(window, cx);
    }

    pub(super) fn begin_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit_state.is_some() {
            return;
        }
        let row = {
            let state = self.state.borrow();
            state.selected_idx().map(|index| state.rows[index].clone())
        };
        let Some(row) = row else {
            return;
        };
        let parent = if row.is_dir {
            {
                let mut state = self.state.borrow_mut();
                state.expanded.insert(row.path.clone());
            }
            // 展开父目录产生新行，重建时现查 git 状态。
            self.rebuild_rows(cx);
            row.path
        } else {
            let Some(parent) = row.path.parent() else {
                return;
            };
            parent.to_path_buf()
        };
        self.begin_edit(EditOperation::Create { parent }, "", 0..0, window, cx);
    }

    pub(super) fn begin_edit(
        &mut self,
        operation: EditOperation,
        name: &str,
        selection: std::ops::Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.entry_name_editor.update(cx, |editor, cx| {
            editor.set_text(name, cx);
            editor.select_byte_range(selection, cx);
        });
        self.edit_state = Some(EditState {
            operation,
            validation_error: None,
        });
        let focus = self.entry_name_editor.read(cx).focus_handle();
        window.focus(&focus);
        cx.notify();
    }

    pub(super) fn handle_tree_confirm_edit(
        &mut self,
        _: &TreeConfirmEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(edit_state) = self.edit_state.clone() else {
            return;
        };
        let name = self.entry_name_editor.read(cx).text(cx);
        match edit_state.operation {
            EditOperation::Rename { source, .. } => {
                let destination = match rename_destination(&source, &name) {
                    Ok(destination) => destination,
                    Err(error) => return self.set_edit_error(error, cx),
                };
                if destination == source {
                    self.finish_edit(window, cx);
                    return;
                }
                let Some(on_rename) = self.on_rename.clone() else {
                    return self.set_edit_error(anyhow::anyhow!("未配置项目重命名服务"), cx);
                };
                if let Err(error) = on_rename(source.clone(), destination.clone(), cx) {
                    return self.set_edit_error(error, cx);
                }
                self.apply_rename(&source, &destination, cx);
            }
            EditOperation::Create { parent } => {
                let new_entry = match new_entry_destination(&parent, &name) {
                    Ok(destination) => destination,
                    Err(error) => return self.set_edit_error(error, cx),
                };
                let Some(on_create) = self.on_create.clone() else {
                    return self.set_edit_error(anyhow::anyhow!("未配置项目新建服务"), cx);
                };
                if let Err(error) = on_create(new_entry.path.clone(), new_entry.is_dir, cx) {
                    return self.set_edit_error(error, cx);
                }
                let mut state = self.state.borrow_mut();
                let mut ancestor = new_entry.path.parent();
                while let Some(directory) = ancestor.filter(|path| path.starts_with(&parent)) {
                    state.expanded.insert(directory.to_path_buf());
                    if directory == parent {
                        break;
                    }
                    ancestor = directory.parent();
                }
                drop(state);
                self.rebuild_rows(cx);
                self.state.borrow_mut().selected = Some(new_entry.path.clone());
                if !new_entry.is_dir
                    && let Some(on_open_file) = &self.on_open_file
                {
                    on_open_file(new_entry.path, true, window, cx);
                }
            }
        }
        self.finish_edit(window, cx);
    }

    pub(super) fn handle_tree_cancel_edit(
        &mut self,
        _: &TreeCancelEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.edit_state.is_some() {
            self.finish_edit(window, cx);
        }
    }

    pub(super) fn set_edit_error(&mut self, error: anyhow::Error, cx: &mut Context<Self>) {
        eprintln!("项目树名称编辑失败：{error}");
        if let Some(edit_state) = &mut self.edit_state {
            edit_state.validation_error = Some(error.to_string());
        }
        cx.notify();
    }

    pub(super) fn finish_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_state = None;
        window.focus(&self.focus);
        cx.notify();
    }
}
