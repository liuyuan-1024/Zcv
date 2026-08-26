//! 剪贴板与传输执行：复制/剪切/粘贴、拖拽放置、冲突确认会话与批量移动/复制。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{Context, Window};

use zcv_actions::{TreeCancelConflict, TreeClearClipboard, TreeCopy, TreeCut, TreePaste};
use zcv_ui::ConfirmAnswer;

use super::ProjectTreePanel;
use super::drag::{TreeDrag, drop_target_dir, filter_movable_sources};
use super::transfer::{
    ConflictDecision, ConflictSession, TransferMode, TreeClipboard, paste_target_dir,
    sanitize_selection,
};

/// 拖拽悬停自动展开延迟：拖拽中悬停折叠目录行约 500ms 后展开。
pub(super) const HOVER_EXPAND_DELAY: Duration = Duration::from_millis(500);

impl ProjectTreePanel {
    pub(super) fn handle_tree_copy(
        &mut self,
        _: &TreeCopy,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_clipboard(TreeClipboard::Copied, cx);
    }

    pub(super) fn handle_tree_cut(&mut self, _: &TreeCut, _: &mut Window, cx: &mut Context<Self>) {
        self.set_clipboard(TreeClipboard::Cut, cx);
    }

    pub(super) fn set_clipboard(
        &mut self,
        kind: fn(Vec<PathBuf>) -> TreeClipboard,
        cx: &mut Context<Self>,
    ) {
        if self.edit_state.is_some() || self.conflict.is_some() {
            return;
        }
        let paths = self.sanitized_selection();
        if paths.is_empty() {
            return;
        }
        self.clipboard = Some(kind(paths));
        cx.notify();
    }

    pub(super) fn handle_tree_clear_clipboard(
        &mut self,
        _: &TreeClearClipboard,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.edit_state.is_some() || self.conflict.is_some() {
            return;
        }
        if self.clipboard.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn handle_tree_paste(
        &mut self,
        _: &TreePaste,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 异步复制进行中禁止再次粘贴：双任务会并发写同一目标并产生进度竞态。
        if self.active_transfer.is_some() {
            return;
        }
        if self.edit_state.is_some() || self.conflict.is_some() {
            return;
        }
        let Some(clipboard) = self.clipboard.clone() else {
            return;
        };
        let mode = match &clipboard {
            TreeClipboard::Copied(_) => TransferMode::Copy,
            TreeClipboard::Cut(_) => TransferMode::Move,
        };
        // 目标目录：选中目录→自身、文件→父目录（is_dir 从行模型解析）。
        let (selected, selected_is_dir) = {
            let state = self.state.borrow();
            let is_dir = state
                .selected_idx()
                .map(|index| state.rows[index].is_dir)
                .unwrap_or(false);
            (state.selected.clone(), is_dir)
        };
        let Some(target_dir) = paste_target_dir(selected.as_deref(), selected_is_dir) else {
            return;
        };
        self.begin_transfer(mode, target_dir, clipboard.paths().to_vec(), cx);
    }

    pub(super) fn begin_transfer(
        &mut self,
        mode: TransferMode,
        target_dir: PathBuf,
        sources: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        // 双保险：拖拽放下与冲突决策完成后也经此入口，传输进行中一律拒绝，避免双任务并发写同一目标与进度竞态。
        if self.active_transfer.is_some() {
            return;
        }
        // 展开目标目录：传输产生的新条目在刷新后立即可见。
        self.state.borrow_mut().expanded.insert(target_dir.clone());
        // 目标与源相同（落回原目录）的项为无操作，静默剔除。
        let items: Vec<(PathBuf, PathBuf)> = sources
            .iter()
            .filter_map(|source| {
                let name = source.file_name()?;
                let dest = target_dir.join(name);
                (dest != *source).then(|| (source.clone(), dest))
            })
            .collect();
        if items.is_empty() {
            return;
        }
        // 冲突预检：目标已存在的项进入会话，其余为非冲突项。
        let (conflicts, clean): (Vec<_>, Vec<_>) =
            items.into_iter().partition(|(_, dest)| dest.exists());
        if conflicts.is_empty() {
            self.execute_transfer(mode, clean, &HashSet::new(), cx);
        } else {
            self.pending_clean_items = clean;
            self.conflict = Some(ConflictSession::new(mode, target_dir, conflicts));
            cx.notify();
        }
    }

    pub(super) fn handle_row_drop(
        &mut self,
        dragged: &TreeDrag,
        row_path: &Path,
        row_is_dir: bool,
        cx: &mut Context<Self>,
    ) {
        self.reset_drag_hover();
        if self.edit_state.is_some() || self.conflict.is_some() {
            return;
        }
        let Some(target_dir) = drop_target_dir(row_path, row_is_dir) else {
            return;
        };
        // 放下信任渲染期冻结的载荷：拖拽内容恒等于发起时所见选区，拖拽期间选区如何变化都不影响移动清单；
        // 净化（排除根与项目外路径、祖先后代重叠）在此处与剪切粘贴共用同一套规则（sanitize_selection），过期路径由 execute_move 的存在性检查兜底。
        let mut items = dragged.items();
        if let Some(root) = self.root.as_ref() {
            items = sanitize_selection(items, root);
        }
        let sources = filter_movable_sources(&items, &target_dir);
        if sources.is_empty() {
            return;
        }
        self.begin_transfer(TransferMode::Move, target_dir, sources, cx);
    }

    pub(super) fn handle_drag_hover(
        &mut self,
        path: PathBuf,
        is_dir: bool,
        expanded: bool,
        cx: &mut Context<Self>,
    ) {
        if is_dir && !expanded {
            if self.drag_hover_path.as_ref() == Some(&path) {
                return;
            }
            self.drag_hover_path = Some(path.clone());
            let timer = cx.background_executor().timer(HOVER_EXPAND_DELAY);
            self.hover_expand_task = Some(cx.spawn(async move |this, cx| {
                timer.await;
                let _ = this.update(cx, |tree, cx| {
                    // 到期校验悬停记录仍有效：移开/放下/取消都会先清掉该字段。
                    if tree.drag_hover_path.as_ref() == Some(&path) {
                        tree.reset_drag_hover();
                        tree.state.borrow_mut().expanded.insert(path.clone());
                        tree.rebuild_rows(cx);
                    }
                });
            }));
        } else {
            self.reset_drag_hover();
        }
    }

    pub(super) fn reset_drag_hover(&mut self) {
        self.drag_hover_path = None;
        self.hover_expand_task = None;
    }

    pub(super) fn handle_tree_cancel_conflict(
        &mut self,
        _: &TreeCancelConflict,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.conflict.take().is_some() {
            self.pending_clean_items.clear();
            cx.notify();
        }
    }

    pub(super) fn sanitized_selection(&self) -> Vec<PathBuf> {
        let Some(root) = self.root.clone() else {
            return Vec::new();
        };
        let state = self.state.borrow();
        sanitize_selection(state.effective_selection(), &root)
    }

    pub(super) fn resolve_conflict(&mut self, answer: ConfirmAnswer, cx: &mut Context<Self>) {
        let Some(mut session) = self.conflict.take() else {
            return;
        };
        match answer {
            ConfirmAnswer::Confirm => session.record_decision(ConflictDecision::Overwrite),
            ConfirmAnswer::Skip => session.record_decision(ConflictDecision::Skip),
            ConfirmAnswer::Cancel => {
                self.pending_clean_items.clear();
                cx.notify();
                return;
            }
        }
        if session.is_resolved() {
            let mode = session.mode;
            // 会话可能跨多次渲染：执行前重展开目标目录（幂等），确保新条目可见。
            self.state
                .borrow_mut()
                .expanded
                .insert(session.target_dir.clone());
            let mut items = std::mem::take(&mut self.pending_clean_items);
            let decisions = session.decisions().to_vec();
            let mut overwrite_set: HashSet<PathBuf> = HashSet::new();
            for ((source, dest), decision) in session.items.into_iter().zip(decisions) {
                if decision == ConflictDecision::Overwrite {
                    overwrite_set.insert(source.clone());
                    items.push((source, dest));
                }
            }
            self.execute_transfer(mode, items, &overwrite_set, cx);
        } else {
            self.conflict = Some(session);
            cx.notify();
        }
    }

    pub(super) fn execute_transfer(
        &mut self,
        mode: TransferMode,
        items: Vec<(PathBuf, PathBuf)>,
        overwrite_set: &HashSet<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if items.is_empty() {
            return;
        }
        match mode {
            TransferMode::Move => self.execute_move(items, overwrite_set, cx),
            TransferMode::Copy => self.execute_copy(items, overwrite_set, cx),
        }
    }

    pub(super) fn execute_move(
        &mut self,
        items: Vec<(PathBuf, PathBuf)>,
        overwrite_set: &HashSet<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let Some(on_move) = self.on_move.clone() else {
            eprintln!("项目树移动失败：未配置项目移动服务");
            return;
        };
        let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
        for (source, dest) in items {
            // 剪贴板过期（源已被移动/删除）或无操作项：静默跳过。
            if !source.exists() || source == dest {
                continue;
            }
            let overwrite = overwrite_set.contains(&source);
            match on_move(source.clone(), dest.clone(), overwrite, cx) {
                Ok(()) => moved.push((source, dest)),
                Err(error) => eprintln!("项目树移动失败：{error}"),
            }
        }
        // 批量迁移树状态：translate_path 按路径前缀替换，逐对调用叠加；
        // 行模型统一重建一次（migrate_rename 不触发重建）。
        // 全部失败时无迁移也无需重建。
        if !moved.is_empty() {
            for (from, to) in &moved {
                self.migrate_rename(from, to);
            }
            self.rebuild_rows(cx);
            // 剪切降级为复制（首次粘贴后剪贴板项可再次粘贴）；
            // 全部失败时保持剪切可重试。
            self.clipboard = self.clipboard.take().map(TreeClipboard::into_copied);
        }
        // 游标收拢：apply_rename 已迁移 selected；selected 未落在任何成功目标（含其子路径）时指向首个成功目标。
        // 移动完成一律回到单选态：迁移后的旧多选集合若残留，下一轮「点选 + cmd 点击」会命中集合变成 toggle 移除，多选拖拽随之间歇性退化为单项（首次拖放后的状态残留）。
        {
            let mut state = self.state.borrow_mut();
            let selected_in_moved = state.selected.as_ref().is_some_and(|selected| {
                moved
                    .iter()
                    .any(|(_, to)| selected.starts_with(to.as_path()))
            });
            if !selected_in_moved && let Some((_, first_dest)) = moved.first() {
                state.selected = Some(first_dest.clone());
            }
            state.selected_set.clear();
            state.anchor = state.selected.clone();
        }
        cx.notify();
    }

    pub(super) fn execute_copy(
        &mut self,
        items: Vec<(PathBuf, PathBuf)>,
        overwrite_set: &HashSet<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let total = items.len();
        self.active_transfer = Some((0, total));
        cx.notify();
        let overwrite_set = overwrite_set.clone();
        cx.spawn(async move |this, cx| {
            let mut succeeded: Vec<PathBuf> = Vec::new();
            for (done, (source, dest)) in items.into_iter().enumerate() {
                let overwrite = overwrite_set.contains(&source);
                // UI 线程创建复制任务（同步校验 + 后台复制），await 驱动完成。
                let created = this.update(cx, |tree, cx| {
                    tree.project.update(cx, |project, cx| {
                        project.copy_path(&source, &dest, overwrite, cx)
                    })
                });
                match created {
                    Ok(Ok(task)) => {
                        // 复制本体失败已在数据层记录：这里只收集成功项。
                        if task.await.is_ok() {
                            succeeded.push(dest);
                        }
                    }
                    // 同步校验失败（如源已消失）：数据层未记录，面板侧补日志。
                    Ok(Err(error)) => eprintln!("项目树复制失败：{error}"),
                    Err(error) => {
                        // 面板实体已释放：进度无处更新，直接终止。
                        eprintln!("项目树复制失败：{error}");
                        return;
                    }
                }
                // enumerate 从 0 计数，进度需要已完成数，加 1 后推进。
                let finished = done + 1;
                let _ = this.update(cx, |tree, cx| {
                    tree.active_transfer = Some((finished, total));
                    cx.notify();
                });
            }
            // 全部完成：清进度并选中首个成功目标（select 重置多选集合，回到单选态）。
            let _ = this.update(cx, |tree, cx| {
                tree.active_transfer = None;
                if let Some(first) = succeeded.first() {
                    tree.state.borrow_mut().select(first.clone());
                }
                cx.notify();
            });
        })
        .detach();
    }
}
