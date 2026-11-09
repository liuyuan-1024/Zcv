//! 剪贴板与传输执行：复制/剪切/粘贴、拖拽放置、冲突确认会话与批量移动/复制。

use std::{collections::HashSet, time::Duration};

use gpui::{ClipboardEntry, ClipboardItem, Context, ExternalPaths, Window};

use zcv_actions::{
    TreeCancelConflict, TreeClearClipboard, TreeCopy, TreeCopyAbsolutePath, TreeCopyRelativePath,
    TreeCut, TreePaste,
};
use zcv_path::AbsolutePathBuf;
use zcv_ui::ConfirmAnswer;

use super::{
    ProjectTreePanel,
    drag::{TreeDrag, resolve_internal_drop},
    transfer::{
        ConflictDecision, ConflictSession, TransferMode, TransferOrigin, TransferRequest,
        TreeClipboard, paste_target_dir, sanitize_selection,
    },
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

    pub(super) fn handle_tree_copy_absolute_path(
        &mut self,
        _: &TreeCopyAbsolutePath,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_paths_as_text(|path, _| path.to_string_lossy().into_owned(), cx);
    }

    pub(super) fn handle_tree_copy_relative_path(
        &mut self,
        _: &TreeCopyRelativePath,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_paths_as_text(
            |path, root| {
                path.strip_prefix(root)
                    .expect("净化后的选中路径必在其所属根下")
                    .to_string_lossy()
                    .into_owned()
            },
            cx,
        );
    }

    /// 把净化选中集的路径文本写入系统剪贴板，每行一条。
    ///
    /// 只写文本，不进入文件复制/剪切会话（面板剪贴板状态不变）。
    fn copy_paths_as_text(
        &mut self,
        render: impl Fn(&std::path::Path, &std::path::Path) -> String,
        cx: &mut Context<Self>,
    ) {
        if self.edit_state.is_some() || self.conflict.is_some() {
            return;
        }
        if self.roots.is_empty() {
            return;
        }
        let paths = self.sanitized_selection();
        if paths.is_empty() {
            return;
        }
        let text = paths
            .iter()
            .map(|path| {
                // 相对化基准 = 路径所属的根（嵌套根取最内层）。
                let root = self
                    .owning_root(path)
                    .expect("净化后的选中路径必在某个工作区根下");
                render(path.as_path(), root.as_path())
            })
            .collect::<Vec<_>>()
            .join(
                "
",
            );
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    pub(super) fn handle_tree_cut(&mut self, _: &TreeCut, _: &mut Window, cx: &mut Context<Self>) {
        self.set_clipboard(TreeClipboard::Cut, cx);
    }

    pub(super) fn set_clipboard(
        &mut self,
        kind: fn(Vec<AbsolutePathBuf>) -> TreeClipboard,
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
        if let Some(clipboard) = &self.clipboard {
            let paths = clipboard
                .paths()
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            cx.write_to_clipboard(ClipboardItem::new_string(paths.join("\n")));
        }
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
        let external = cx.read_from_clipboard().and_then(|item| {
            item.entries().iter().find_map(|entry| match entry {
                ClipboardEntry::ExternalPaths(paths) if !paths.paths().is_empty() => {
                    Some(paths.clone())
                }
                _ => None,
            })
        });
        // 目标目录：选中目录→自身、文件→父目录（is_dir 从行模型解析）。
        let (selected, selected_is_dir) = {
            let state = self.state.borrow();
            let is_dir = state
                .selected_idx()
                .map(|index| state.rows[index].is_dir)
                .unwrap_or(false);
            (state.selected.clone(), is_dir)
        };
        // 无选中时回退到主根（首个根）。
        let Some(target_dir) = paste_target_dir(selected.as_ref(), selected_is_dir)
            .or_else(|| self.roots.first().cloned())
        else {
            return;
        };
        if external.is_some() || self.clipboard.is_some() {
            // 粘贴主动展示结果；拖拽与传输层不修改用户的目录展开意图。
            if self.state.borrow_mut().expanded.insert(target_dir.clone()) {
                self.rebuild_rows(cx);
            }
        }
        if let Some(paths) = external {
            self.import_external_paths(&paths, target_dir, TransferOrigin::Paste, cx);
        } else if let Some(clipboard) = self.clipboard.clone() {
            let mode = match &clipboard {
                TreeClipboard::Copied(_) => TransferMode::Copy,
                TreeClipboard::Cut(_) => TransferMode::Move,
            };
            self.begin_transfer(
                TransferRequest {
                    mode,
                    origin: TransferOrigin::Paste,
                    target_dir,
                    sources: clipboard.paths().to_vec(),
                },
                cx,
            );
        }
    }

    pub(super) fn import_external_paths(
        &mut self,
        paths: &ExternalPaths,
        target_dir: AbsolutePathBuf,
        origin: TransferOrigin,
        cx: &mut Context<Self>,
    ) {
        if self.edit_state.is_some() || self.conflict.is_some() || self.active_transfer.is_some() {
            return;
        }
        let sources: anyhow::Result<Vec<_>> = paths
            .paths()
            .iter()
            .map(|path| {
                AbsolutePathBuf::new(path.clone())
                    .map_err(|error| anyhow::anyhow!("{}：{error}", path.display()))
            })
            .collect();
        let sources = match sources {
            Ok(sources) => sources,
            Err(error) => {
                self.report_error(format!("项目树导入失败：{error:#}"), cx);
                return;
            }
        };
        self.begin_transfer(
            TransferRequest {
                mode: TransferMode::CopyExternal,
                origin,
                target_dir,
                sources,
            },
            cx,
        );
    }

    pub(super) fn begin_transfer(&mut self, request: TransferRequest, cx: &mut Context<Self>) {
        // 双保险：拖拽放下与冲突决策完成后也经此入口，传输进行中一律拒绝，避免双任务并发写同一目标与进度竞态。
        if self.active_transfer.is_some() {
            return;
        }
        // 目标与源相同（落回原目录）的项为无操作，静默剔除。
        let plan = request.prepare();
        if plan.items.is_empty() {
            return;
        }
        // 冲突预检：目标已存在的项进入会话，其余为非冲突项。
        let (conflicts, clean): (Vec<_>, Vec<_>) =
            plan.items.into_iter().partition(|(_, dest)| dest.exists());
        if conflicts.is_empty() {
            self.execute_transfer(plan.mode, plan.origin, clean, &HashSet::new(), cx);
        } else {
            self.conflict = Some(ConflictSession::new(
                plan.mode,
                plan.origin,
                clean,
                conflicts,
            ));
            cx.notify();
        }
    }

    pub(super) fn handle_row_drop(
        &mut self,
        dragged: &TreeDrag,
        row_path: &AbsolutePathBuf,
        row_is_dir: bool,
        cx: &mut Context<Self>,
    ) {
        self.finish_drag_session(cx);
        if self.edit_state.is_some() || self.conflict.is_some() {
            return;
        }
        let Some(candidate) = resolve_internal_drop(dragged, row_path, row_is_dir, &self.roots)
        else {
            return;
        };
        self.begin_transfer(
            TransferRequest {
                mode: TransferMode::Move,
                origin: TransferOrigin::InternalDrag,
                target_dir: candidate.target_dir,
                sources: candidate.sources,
            },
            cx,
        );
    }

    pub(super) fn handle_drag_hover(
        &mut self,
        path: AbsolutePathBuf,
        is_dir: bool,
        expanded: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.drag_session.as_mut() else {
            return;
        };
        let droppable =
            resolve_internal_drop(&session.dragged, &path, is_dir, &self.roots).is_some();
        if is_dir && !expanded && droppable {
            if session.hovered_directory.as_ref() == Some(&path) {
                return;
            }
            session.clear_hover();
            session.hovered_directory = Some(path.clone());
            let timer = cx.background_executor().timer(HOVER_EXPAND_DELAY);
            session.hover_expand_task = Some(cx.spawn(async move |this, cx| {
                timer.await;
                let _ = this.update(cx, |tree, cx| {
                    if let Some(session) = tree.drag_session.as_mut()
                        && session.hovered_directory.as_ref() == Some(&path)
                    {
                        session.clear_hover();
                        session.temporary_expanded.insert(path.clone());
                        tree.rebuild_rows(cx);
                    }
                });
            }));
        } else {
            self.reset_drag_hover();
        }
    }

    pub(super) fn reset_drag_hover(&mut self) {
        if let Some(session) = self.drag_session.as_mut() {
            session.clear_hover();
        }
    }

    pub(super) fn handle_tree_cancel_conflict(
        &mut self,
        _: &TreeCancelConflict,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.conflict.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn sanitized_selection(&self) -> Vec<AbsolutePathBuf> {
        if self.roots.is_empty() {
            return Vec::new();
        }
        let state = self.state.borrow();
        sanitize_selection(state.effective_selection(), &self.roots)
    }

    pub(super) fn resolve_conflict(&mut self, answer: ConfirmAnswer, cx: &mut Context<Self>) {
        let Some(mut session) = self.conflict.take() else {
            return;
        };
        match answer {
            ConfirmAnswer::Confirm => session.record_decision(ConflictDecision::Overwrite),
            ConfirmAnswer::Skip => session.record_decision(ConflictDecision::Skip),
            ConfirmAnswer::Cancel => {
                cx.notify();
                return;
            }
        }
        if session.is_resolved() {
            let mode = session.mode;
            let origin = session.origin;
            let decisions = session.decisions().to_vec();
            let mut items = session.clean_items;
            let mut overwrite_set: HashSet<AbsolutePathBuf> = HashSet::new();
            for ((source, dest), decision) in session.items.into_iter().zip(decisions) {
                if decision == ConflictDecision::Overwrite {
                    overwrite_set.insert(source.clone());
                    items.push((source, dest));
                }
            }
            self.execute_transfer(mode, origin, items, &overwrite_set, cx);
        } else {
            self.conflict = Some(session);
            cx.notify();
        }
    }

    pub(super) fn execute_transfer(
        &mut self,
        mode: TransferMode,
        origin: TransferOrigin,
        items: Vec<(AbsolutePathBuf, AbsolutePathBuf)>,
        overwrite_set: &HashSet<AbsolutePathBuf>,
        cx: &mut Context<Self>,
    ) {
        if items.is_empty() {
            return;
        }
        match mode {
            TransferMode::Move => self.execute_move(origin, items, overwrite_set, cx),
            TransferMode::Copy | TransferMode::CopyExternal => {
                self.execute_copy(mode, origin, items, overwrite_set, cx)
            }
        }
    }

    pub(super) fn execute_move(
        &mut self,
        origin: TransferOrigin,
        items: Vec<(AbsolutePathBuf, AbsolutePathBuf)>,
        overwrite_set: &HashSet<AbsolutePathBuf>,
        cx: &mut Context<Self>,
    ) {
        let Some(on_move) = self.on_move.clone() else {
            self.report_error("项目树移动失败：未配置项目移动服务".into(), cx);
            return;
        };
        let mut moved: Vec<(AbsolutePathBuf, AbsolutePathBuf)> = Vec::new();
        for (source, dest) in items {
            // 剪贴板过期（源已被移动/删除）或无操作项：静默跳过。
            if !source.exists() || source == dest {
                continue;
            }
            let overwrite = overwrite_set.contains(&source);
            match on_move(
                source.clone().into_path_buf(),
                dest.clone().into_path_buf(),
                overwrite,
                cx,
            ) {
                Ok(()) => moved.push((source, dest)),
                Err(error) => self.report_error(format!("项目树移动失败：{error:#}"), cx),
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
            if origin == TransferOrigin::Paste {
                self.clipboard = self.clipboard.take().map(TreeClipboard::into_copied);
            }
        }
        self.present_transfer_result(
            origin,
            moved
                .into_iter()
                .map(|(_, dest)| dest)
                .collect::<Vec<_>>()
                .as_slice(),
        );
        cx.notify();
    }

    fn present_transfer_result(
        &mut self,
        origin: TransferOrigin,
        destinations: &[AbsolutePathBuf],
    ) {
        let Some(first) = destinations.first() else {
            return;
        };
        if origin == TransferOrigin::ExternalDrop {
            return;
        }
        let mut state = self.state.borrow_mut();
        let selected = if origin == TransferOrigin::InternalDrag {
            state
                .selected
                .as_ref()
                .filter(|selected| {
                    destinations
                        .iter()
                        .any(|dest| selected.starts_with(dest.as_path()))
                })
                .cloned()
                .unwrap_or_else(|| first.clone())
        } else {
            first.clone()
        };
        let target = selected.parent().map(|parent| {
            AbsolutePathBuf::new(parent.to_path_buf()).expect("目标目录必须为绝对路径")
        });
        let selected = match target {
            Some(parent) if !state.expanded.contains(&parent) => parent,
            _ => selected,
        };
        state.select(selected);
    }

    pub(super) fn execute_copy(
        &mut self,
        mode: TransferMode,
        origin: TransferOrigin,
        items: Vec<(AbsolutePathBuf, AbsolutePathBuf)>,
        overwrite_set: &HashSet<AbsolutePathBuf>,
        cx: &mut Context<Self>,
    ) {
        let total = items.len();
        self.active_transfer = Some((0, total));
        cx.notify();
        let overwrite_set = overwrite_set.clone();
        cx.spawn(async move |this, cx| {
            let mut succeeded: Vec<AbsolutePathBuf> = Vec::new();
            for (done, (source, dest)) in items.into_iter().enumerate() {
                let overwrite = overwrite_set.contains(&source);
                // UI 线程创建复制任务（同步校验 + 后台复制），await 驱动完成。
                let created = this.update(cx, |tree, cx| {
                    tree.project.update(cx, |project, cx| match mode {
                        TransferMode::Copy => project.copy_path(&source, &dest, overwrite, cx),
                        TransferMode::CopyExternal => {
                            project.copy_external_path(&source, &dest, overwrite, cx)
                        }
                        TransferMode::Move => unreachable!("移动不进入复制执行路径"),
                    })
                });
                match created {
                    Ok(Ok(task)) => {
                        // 复制本体失败已在数据层记录：这里只收集成功项。
                        match task.await {
                            Ok(()) => succeeded.push(dest),
                            Err(error) => {
                                let _ = this.update(cx, |tree, cx| {
                                    tree.report_error(format!("项目树复制失败：{error:#}"), cx);
                                });
                            }
                        }
                    }
                    // 同步校验失败（如源已消失）：数据层未记录，面板侧补日志。
                    Ok(Err(error)) => {
                        let _ = this.update(cx, |tree, cx| {
                            tree.report_error(format!("项目树复制失败：{error:#}"), cx);
                        });
                    }
                    Err(error) => {
                        // 面板实体已释放：进度无处更新，直接终止。
                        let _ = this.update(cx, |tree, cx| {
                            tree.report_error(format!("项目树复制失败：{error:#}"), cx);
                        });
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
                tree.present_transfer_result(origin, &succeeded);
                cx.notify();
            });
        })
        .detach();
    }
}
