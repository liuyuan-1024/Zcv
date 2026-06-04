//! 文件树选择与扩选模型。

use std::collections::BTreeSet;
use std::path::PathBuf;

use super::model::FileTreeModel;

/// 一次正在进行的扩选笔画。`anchor` 是笔画起点（按下第一个 Shift+方向时的
/// 焦点）；`items` 是当前 `[anchor, focus]` 区间在可见行里覆盖到的全部行。
#[derive(Clone, Debug)]
pub(super) struct Stroke {
    pub(super) anchor: PathBuf,
    pub(super) items: BTreeSet<PathBuf>,
}

impl FileTreeModel {
    /// 已提交选区与当前 stroke 的并集——所有“对外说选了什么”都看这里。
    pub(super) fn effective_selection(&self) -> BTreeSet<PathBuf> {
        match &self.stroke {
            None => self.selection.clone(),
            Some(stroke) => self.selection.union(&stroke.items).cloned().collect(),
        }
    }

    /// 把当前笔画 `items` 沉淀到 `selection`，并清空 stroke。普通方向键、
    /// PageUp/PageDown 这些“打断笔画”的操作在动焦点前都要先调它一次。
    fn commit_stroke(&mut self) {
        if let Some(stroke) = self.stroke.take() {
            self.selection.extend(stroke.items);
        }
    }

    /// 焦点进入文件树时调用：若尚未选中任何行，默认落到第一行，让边框
    /// 立刻出现，无需用户先按一次 ↓。
    pub(crate) fn ensure_selection_initialized(&mut self) {
        if self.selected.is_some() {
            return;
        }
        let Some(tree) = self.project_tree.as_ref() else {
            return;
        };
        if let Some(first) = tree.visible_rows().first() {
            self.selected = Some(first.path.to_path_buf());
        }
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        let Some(tree) = self.project_tree.as_ref() else {
            return;
        };
        let paths: Vec<PathBuf> = tree
            .visible_rows()
            .into_iter()
            .map(|row| row.path.to_path_buf())
            .collect();
        if paths.is_empty() {
            return;
        }
        // 普通方向键打断当前笔画：把 stroke.items 沉淀到 selection，再动焦点。
        // 这样下一次 Shift+方向 会从新焦点重新起锚，之前的笔画作为已提交选区被保留。
        self.commit_stroke();
        let new_index = match self.selected.as_ref() {
            None => {
                if delta >= 0 {
                    0
                } else {
                    paths.len() - 1
                }
            }
            Some(current) => {
                let cur_idx = paths.iter().position(|p| p == current).unwrap_or(0) as isize;
                (cur_idx + delta).clamp(0, paths.len() as isize - 1) as usize
            }
        };
        self.selected = Some(paths[new_index].clone());
    }

    /// 扩展多选选区——“锚点 + 笔画”模型。
    pub(crate) fn extend_selection(&mut self, delta: isize) {
        let Some(tree) = self.project_tree.as_ref() else {
            return;
        };
        let paths: Vec<PathBuf> = tree
            .visible_rows()
            .into_iter()
            .map(|row| row.path.to_path_buf())
            .collect();
        if paths.is_empty() {
            return;
        }
        if self.selected.is_none() {
            let initial = if delta >= 0 { 0 } else { paths.len() - 1 };
            let path = paths[initial].clone();
            self.selected = Some(path.clone());
            self.stroke = Some(Stroke {
                anchor: path.clone(),
                items: std::iter::once(path).collect(),
            });
            return;
        }
        let cur_focus = self.selected.clone().expect("上面保证非空");
        let cur_idx = paths.iter().position(|p| p == &cur_focus).unwrap_or(0);
        let anchor_idx = self
            .stroke
            .as_ref()
            .and_then(|s| paths.iter().position(|p| p == &s.anchor))
            .unwrap_or(cur_idx);
        let need_reset = self
            .stroke
            .as_ref()
            .map(|s| !paths.iter().any(|p| p == &s.anchor))
            .unwrap_or(true);
        if need_reset {
            self.stroke = Some(Stroke {
                anchor: paths[anchor_idx].clone(),
                items: BTreeSet::new(),
            });
        }
        let new_idx = ((cur_idx as isize) + delta).clamp(0, paths.len() as isize - 1) as usize;
        self.selected = Some(paths[new_idx].clone());
        let (lo, hi) = if anchor_idx <= new_idx {
            (anchor_idx, new_idx)
        } else {
            (new_idx, anchor_idx)
        };
        let items: BTreeSet<PathBuf> = paths[lo..=hi].iter().cloned().collect();
        self.stroke.as_mut().expect("上面已确保 stroke 存在").items = items;
    }

    /// Esc 二段式：已提交选区或当前 stroke 任一非空时，全清并返回 `true`
    /// 表示已消化；都空时返回 `false`，让调用方走“焦点回编辑器”的原有路径。
    pub(crate) fn escape(&mut self) -> bool {
        if self.selection.is_empty() && self.stroke.is_none() {
            false
        } else {
            self.selection.clear();
            self.stroke = None;
            true
        }
    }
}
