//! 普通文件编辑器的 Git 差异投影与冲突标记。

use gpui::{Context, Entity};
use zcv_git::{FileStatus, parse_conflict_regions};
use zcv_language::{LanguageBuffer, LanguageBufferEvent};
use zcv_multi_buffer::{DiffExcerptRanges, DiffFile};
use zcv_project::{GitStore, GitStoreEvent};
use zcv_text::{ByteOffset, TextRange};

use super::{Editor, EditorHunk};

impl Editor {
    pub(crate) fn bind_file_git_diff(
        &mut self,
        working: Entity<LanguageBuffer>,
        store: Entity<GitStore>,
        cx: &mut Context<Self>,
    ) {
        let store_subscription = cx.subscribe(&store, {
            let working = working.clone();
            move |editor, store, event, cx| {
                if matches!(
                    event,
                    GitStoreEvent::Repositories
                        | GitStoreEvent::Statuses
                        | GitStoreEvent::Head
                        | GitStoreEvent::RevisionSupportChanged
                ) {
                    editor.sync_file_conflicts(&working, &store, cx);
                    editor.open_file_git_diff(working.clone(), &store, cx);
                }
            }
        });
        let working_subscription = cx.subscribe(&working, {
            let store = store.clone();
            let mut path = working.read(cx).file_path();
            move |editor, working, event, cx| match event {
                LanguageBufferEvent::TextChanged => {
                    editor.sync_file_conflicts(&working, &store, cx);
                }
                LanguageBufferEvent::MetadataChanged => {
                    let current_path = working.read(cx).file_path();
                    if current_path != path {
                        path = current_path;
                        if !editor.diff_paths(cx).is_empty() {
                            editor.clear_diffs(cx);
                        }
                        editor.sync_file_conflicts(&working, &store, cx);
                        editor.open_file_git_diff(working, &store, cx);
                    }
                }
                _ => {}
            }
        });
        self.file_git_subscriptions = Some([store_subscription, working_subscription]);
        self.sync_file_conflicts(&working, &store, cx);
        self.open_file_git_diff(working, &store, cx);
    }

    fn open_file_git_diff(
        &mut self,
        working: Entity<LanguageBuffer>,
        store: &Entity<GitStore>,
        cx: &mut Context<Self>,
    ) {
        self.file_diff_task = None;
        let Some(path) = working.read(cx).file_path() else {
            return;
        };
        let task = store.update(cx, |store, cx| store.open_uncommitted_diff(working, cx));
        self.file_diff_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |editor, cx| {
                editor.file_diff_task = None;
                if editor.file_path(cx).as_deref() != Some(path.as_path()) {
                    return;
                }
                match result {
                    Ok(Some(diff)) => {
                        editor.add_diff(
                            DiffFile {
                                diff,
                                display_path: path,
                                excerpt_ranges: DiffExcerptRanges::FullFile,
                            },
                            cx,
                        );
                    }
                    Ok(None) if !editor.diff_paths(cx).is_empty() => {
                        editor.clear_diffs(cx);
                    }
                    // 修订加载错误由 GitStore 统一报告，保留已接入的差异。
                    _ => {}
                }
            })
            .ok();
        }));
    }

    fn sync_file_conflicts(
        &mut self,
        working: &Entity<LanguageBuffer>,
        store: &Entity<GitStore>,
        cx: &mut Context<Self>,
    ) {
        let path = working.read(cx).file_path();
        let Some(path) = path.filter(|path| {
            store
                .read(cx)
                .status_for_path(path)
                .is_some_and(|entry| entry.status == FileStatus::Unmerged)
        }) else {
            self.set_editor_hunks(Vec::new(), cx);
            return;
        };
        let snapshot = working.read(cx).text_snapshot();
        let text_range =
            TextRange::new(ByteOffset::ZERO, snapshot.len_bytes()).expect("工作区文本范围必须有效");
        let text = snapshot
            .slice_text(text_range)
            .expect("工作区文本快照必须可切片")
            .to_string();
        let hunks = parse_conflict_regions(&text)
            .into_iter()
            .enumerate()
            .filter_map(|(index, region)| {
                EditorHunk::conflict(
                    format!("{}\n{index}", path.display()),
                    region.outer.clone(),
                    region.theirs.start,
                    |offset| offset,
                )
            })
            .collect();
        self.set_editor_hunks(hunks, cx);
    }
}
