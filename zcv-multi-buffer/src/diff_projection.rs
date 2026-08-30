//! MultiBuffer 的 git diff 投影：hunks 注入、展开状态、锚点迁移与显示坐标派生。
//!
//! 状态与组合文档同属本层：Editor 只消费派生结果，不再由外部用临时列表驱动。
//! 两种注入模式：
//! - 普通编辑器（source 模式）：注入工作区源坐标 hunks + HEAD 全文，本层把展开的旧侧行物化为只读 excerpt 并派生显示坐标；
//! - 多文件投影（materialized 模式）：旧侧已由宿主物化进组合文档，只注入显示坐标 hunks。
//!
//! 展开状态跨 diff 刷新迁移：source 模式按工作区文本锚点（Anchor）匹配新旧 hunk，文本位置未变即视为同一 hunk——编辑移位、HEAD/index 变化都不丢失状态；materialized 模式按旧侧（base 版本）行范围重叠匹配，
//! base 变化时由宿主调用 [`MultiBuffer::reset_diff_hunk_expansion_state`] 重置。

use std::ops::Range;
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity};
use zcv_git::{DiffHunk, DiffHunkKind};
use zcv_text::{Anchor, BufferVersion, Line, PositionMap};

use crate::{MultiBuffer, MultiBufferEvent, MultiBufferExcerpt};

/// 一个 MultiBuffer 的 git diff 投影状态。
#[derive(Default)]
pub(crate) struct DiffProjection {
    /// source 模式注入的源坐标 hunks；`None` 表示 materialized 模式。
    source_hunks: Option<Vec<DiffHunk>>,
    /// 与 source_hunks 并行的工作区源文本锚点（hunk 起点所在行首；随编辑推进）。
    hunk_anchors: Vec<Anchor>,
    /// HEAD 全文（删除/修改块展开时旧行来源；source 模式）。
    head_text: Option<Arc<str>>,
    /// HEAD 修订来源（singleton MultiBuffer）：source 模式物化旧行的数据源。
    head_source: Option<Entity<MultiBuffer>>,
    /// 新 hunk 的初始展开策略；只决定初始状态，不覆盖用户显式切换。
    expanded_by_default: bool,
    /// 默认折叠模式下被用户显式展开的删除/修改 hunk（按旧侧行范围标识）。
    expanded_deleted: Vec<Range<usize>>,
    expanded_modified: Vec<Range<usize>>,
    /// 默认展开模式下被用户显式折叠的 hunk。
    collapsed_deleted: Vec<Range<usize>>,
    collapsed_modified: Vec<Range<usize>>,
    /// 显示坐标 hunks（source 模式由投影计算；materialized 模式直接注入）。
    display_hunks: Vec<DiffHunk>,
    /// 每个 hunk 在组合文档中的旧侧显示行范围；折叠态或 Added hunk 为 `None`。
    display_old_ranges: Vec<Option<Range<usize>>>,
    /// 显示坐标对应的组合文档版本（注入/重建后发生编辑会使坐标失效）。
    display_version: Option<BufferVersion>,
}

impl MultiBuffer {
    /// 普通编辑器：注入工作区源坐标 git hunks。
    ///
    /// `None` 是加载态（新 diff 尚未算完），保留现有 hunks 与用户展开状态；
    /// `Some` 注入后按锚点迁移展开状态并重建投影。
    /// 返回 `true` 表示组合文档被重建（调用方应重置光标）。
    pub fn set_diff_hunks(&mut self, hunks: Option<Vec<DiffHunk>>, cx: &mut Context<Self>) -> bool {
        let Some(hunks) = hunks else {
            return false;
        };
        let (old_hunks, old_anchors) = match &mut self.diff {
            Some(diff) => (
                diff.source_hunks.take(),
                std::mem::take(&mut diff.hunk_anchors),
            ),
            None => (None, Vec::new()),
        };
        let new_anchors = self.diff_anchors(&hunks, cx);
        let diff = self
            .diff
            .get_or_insert_with(|| Box::new(DiffProjection::default()));
        diff.migrate_expansion_state(
            old_hunks.as_deref().unwrap_or(&[]),
            &old_anchors,
            &hunks,
            &new_anchors,
        );
        diff.source_hunks = Some(hunks);
        diff.hunk_anchors = new_anchors;
        self.rebuild_diff_projection(cx)
    }

    /// 多文件投影：注入旧侧已经属于组合文档文本的显示坐标 hunks。
    pub fn set_materialized_diff_hunks(
        &mut self,
        hunks: Vec<crate::MaterializedDiffHunk>,
        cx: &mut Context<Self>,
    ) {
        let (hunks, old_display_ranges): (Vec<_>, Vec<_>) = hunks
            .into_iter()
            .map(|hunk| (hunk.hunk, hunk.old_display_range))
            .unzip();
        let version = self.text_buffer(cx).read(cx).snapshot().version();
        let diff = self
            .diff
            .get_or_insert_with(|| Box::new(DiffProjection::default()));
        if diff.display_hunks == hunks
            && diff.display_old_ranges == old_display_ranges
            && diff.display_version == Some(version)
        {
            return;
        }
        // materialized 模式按旧侧行范围重叠迁移（无文本锚点）。
        // 注入发生在宿主重建组合片段之后，迁移改变了展开状态时必须再通知宿主重建一次。
        let old_hunks = std::mem::take(&mut diff.display_hunks);
        let state_changed = diff.migrate_expansion_state(&old_hunks, &[], &hunks, &[]);
        diff.display_hunks = hunks;
        diff.display_old_ranges = old_display_ranges;
        diff.display_version = Some(version);
        if state_changed {
            cx.emit(MultiBufferEvent::DiffExpansionChanged);
        }
        cx.notify();
    }

    /// 注入 HEAD 全文（source 模式删除/修改块展开的数据源）；到达后重建删除块。
    ///
    /// 返回 `true` 表示组合文档被重建。
    pub fn set_diff_head_text(&mut self, text: Option<Arc<str>>, cx: &mut Context<Self>) -> bool {
        let changed = match &mut self.diff {
            Some(diff) if diff.head_text != text => {
                diff.head_text = text.clone();
                true
            }
            _ => false,
        };
        if !changed {
            return false;
        }
        let path = self.file_path(cx);
        let head_source = text.as_ref().map(|text| {
            let buffer =
                zcv_text::Buffer::from_text(text.to_string(), zcv_text::BufferConfig::default())
                    .expect("HEAD 修订文本必须能创建 Buffer");
            let buffer = cx.new(|_| buffer);
            let language = cx.new(|cx| zcv_language::LanguageBuffer::new(buffer, path.clone(), cx));
            cx.new(|cx| MultiBuffer::singleton(language, cx))
        });
        if let Some(diff) = &mut self.diff {
            diff.head_source = head_source;
        }
        self.rebuild_diff_projection(cx)
    }

    /// 设置新 hunk 的初始展开策略；用户之后的显式展开/折叠不受投影刷新覆盖。
    ///
    /// 返回 `true` 表示组合文档被重建。
    pub fn set_diff_hunks_expanded_by_default(
        &mut self,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let diff = self
            .diff
            .get_or_insert_with(|| Box::new(DiffProjection::default()));
        if diff.expanded_by_default == expanded {
            return false;
        }
        diff.expanded_by_default = expanded;
        diff.collapsed_deleted.clear();
        diff.collapsed_modified.clear();
        // 策略切换不迁移旧状态：按新默认值重新应用。
        let display_hunks = std::mem::take(&mut diff.display_hunks);
        diff.migrate_expansion_state(&[], &[], &display_hunks, &[]);
        diff.display_hunks = display_hunks;
        let rebuilt = self.rebuild_diff_projection(cx);
        cx.emit(MultiBufferEvent::DiffExpansionChanged);
        rebuilt
    }

    /// 展开/折叠指定删除或修改 hunk（按旧侧行范围标识）。
    ///
    /// 返回 `true` 表示组合文档被重建（source 模式物化/撤销旧侧 excerpt）。
    pub fn toggle_diff_hunk(
        &mut self,
        kind: DiffHunkKind,
        old_range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(diff) = &mut self.diff else {
            return false;
        };
        let is_expanded = diff.is_expanded(kind, &old_range);
        match kind {
            DiffHunkKind::Deleted => {
                if is_expanded {
                    diff.expanded_deleted.retain(|range| range != &old_range);
                    if diff.expanded_by_default && !diff.collapsed_deleted.contains(&old_range) {
                        diff.collapsed_deleted.push(old_range.clone());
                    }
                } else {
                    diff.collapsed_deleted.retain(|range| range != &old_range);
                    if !diff.expanded_deleted.contains(&old_range) {
                        diff.expanded_deleted.push(old_range);
                    }
                }
            }
            DiffHunkKind::Modified => {
                if is_expanded {
                    diff.expanded_modified.retain(|range| range != &old_range);
                    if diff.expanded_by_default && !diff.collapsed_modified.contains(&old_range) {
                        diff.collapsed_modified.push(old_range.clone());
                    }
                } else {
                    diff.collapsed_modified.retain(|range| range != &old_range);
                    if !diff.expanded_modified.contains(&old_range) {
                        diff.expanded_modified.push(old_range);
                    }
                }
            }
            DiffHunkKind::Added => return false,
        }
        let rebuilt = self.rebuild_diff_projection(cx);
        cx.emit(MultiBufferEvent::DiffExpansionChanged);
        rebuilt
    }

    /// base 版本变化（HEAD 变化等）后由宿主调用：旧侧坐标空间已失效（materialized 模式），按默认策略重置展开状态；
    /// source 模式锚点仍有效，本调用等价于用户全部折叠。
    ///
    /// 返回 `true` 表示组合文档被重建。
    pub fn reset_diff_hunk_expansion_state(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(diff) = &mut self.diff else {
            return false;
        };
        diff.expanded_deleted.clear();
        diff.expanded_modified.clear();
        diff.collapsed_deleted.clear();
        diff.collapsed_modified.clear();
        let rebuilt = self.rebuild_diff_projection(cx);
        cx.emit(MultiBufferEvent::DiffExpansionChanged);
        rebuilt
    }

    /// 与当前组合文档版本匹配的显示坐标 hunks；未注入、加载态或注入后发生编辑时返回空。
    pub fn diff_hunks<'a>(&'a self, cx: &'a App) -> &'a [DiffHunk] {
        match self.display_state(cx) {
            Some(diff) => &diff.display_hunks,
            None => &[],
        }
    }

    /// 每个 hunk 在组合文档中的旧侧显示行范围（与 [`MultiBuffer::diff_hunks`] 同门控）。
    pub fn diff_hunk_old_ranges<'a>(&'a self, cx: &'a App) -> &'a [Option<Range<usize>>] {
        match self.display_state(cx) {
            Some(diff) => &diff.display_old_ranges,
            None => &[],
        }
    }

    /// 显示坐标只在组合文档未被后续编辑时有效（版本门控）。
    fn display_state<'a>(&'a self, cx: &'a App) -> Option<&'a DiffProjection> {
        let diff = self.diff.as_ref()?;
        (diff.display_version == Some(self.text_buffer(cx).read(cx).snapshot().version()))
            .then_some(diff)
    }

    /// hunk 的当前展开状态；尚未进入投影的新 hunk 直接采用默认策略。
    pub fn is_diff_hunk_expanded(&self, kind: DiffHunkKind, old_range: &Range<usize>) -> bool {
        self.diff
            .as_ref()
            .is_some_and(|diff| diff.is_expanded(kind, old_range))
    }

    /// 已展开的删除 hunk（按旧侧行范围标识；渲染背景色用）。
    pub fn expanded_deleted_hunks(&self) -> &[Range<usize>] {
        self.diff
            .as_ref()
            .map(|diff| diff.expanded_deleted.as_slice())
            .unwrap_or(&[])
    }

    /// 已展开的修改 hunk（按旧侧行范围标识；渲染背景色用）。
    pub fn expanded_modified_hunks(&self) -> &[Range<usize>] {
        self.diff
            .as_ref()
            .map(|diff| diff.expanded_modified.as_slice())
            .unwrap_or(&[])
    }

    /// 工作区源编辑后推进 hunk 锚点。
    ///
    /// 组合编辑在 [`MultiBuffer::edit`] 内同步调用（用本次编辑的 PositionMap）；
    /// 外部编辑由 source_changed 调用（用消费到的文本变化 patch）。
    pub(crate) fn map_diff_hunk_anchors(
        &mut self,
        position_map: &PositionMap,
        new_version: BufferVersion,
    ) {
        let Some(diff) = &mut self.diff else {
            return;
        };
        for anchor in &mut diff.hunk_anchors {
            *anchor = anchor
                .map_through_position_map(new_version, position_map)
                .value();
        }
    }

    /// 把 hunk 新侧起点行号转为工作区源文本锚点（source 模式注入时使用）。
    fn diff_anchors(&self, hunks: &[DiffHunk], cx: &App) -> Vec<Anchor> {
        let Some(working) = &self.working_source else {
            return Vec::new();
        };
        let snapshot = working.read(cx).snapshot(cx);
        let version = snapshot.text().version();
        let text = snapshot.text();
        hunks
            .iter()
            .map(|hunk| {
                let offset = text
                    .line_start_byte(Line::new(hunk.range.start))
                    .unwrap_or(text.len_bytes());
                Anchor::new(version, offset)
            })
            .collect()
    }

    /// source 模式投影重建：把展开状态物化为 excerpts 并派生显示坐标。
    ///
    /// 返回 `true` 表示组合文档文本版本发生变化。
    fn rebuild_diff_projection(&mut self, cx: &mut Context<Self>) -> bool {
        let (source_hunks, head_source, expanded_deleted, expanded_modified) = {
            let Some(diff) = &self.diff else {
                return false;
            };
            let Some(source_hunks) = diff.source_hunks.clone() else {
                return false;
            };
            (
                source_hunks,
                diff.head_source.clone(),
                diff.expanded_deleted.clone(),
                diff.expanded_modified.clone(),
            )
        };
        // 工作区源：组合文档（from_working_source）用独立源；
        // singleton 形态下 excerpts 引用自身（set_excerpts 首次转换时改写为独立工作区源），文本快照从组合投影读取。
        let working_source = self.working_source.clone();
        let working = working_source.clone().unwrap_or_else(|| cx.entity());
        let working_text = match &working_source {
            Some(source) => source.read(cx).snapshot(cx).text().clone(),
            None => self.text_buffer(cx).read(cx).snapshot(),
        };
        let working_excerpt = |lines: std::ops::Range<usize>| {
            MultiBufferExcerpt::line_range_from_text(working.clone(), &working_text, lines)
        };
        let old_version = self.text_buffer(cx).read(cx).snapshot().version();
        let line_count = working_text.line_count();
        let mut excerpts = Vec::new();
        let mut displayed_hunks = Vec::with_capacity(source_hunks.len());
        let mut old_display_ranges = Vec::with_capacity(source_hunks.len());
        let had_materialized_old_rows = self
            .diff
            .as_ref()
            .is_some_and(|diff| diff.display_old_ranges.iter().any(Option::is_some));
        let mut current = 0usize;
        let mut materialized_old_lines = 0usize;
        for hunk in &source_hunks {
            let expanded = head_source.is_some()
                && match hunk.kind {
                    DiffHunkKind::Deleted => expanded_deleted.contains(&hunk.old_range),
                    DiffHunkKind::Modified => expanded_modified.contains(&hunk.old_range),
                    DiffHunkKind::Added => false,
                };
            let old_display_range = if expanded && !hunk.old_range.is_empty() {
                if current < hunk.range.start {
                    excerpts.push(
                        working_excerpt(current..hunk.range.start).with_starts_new_excerpt(false),
                    );
                }
                let start = hunk.range.start + materialized_old_lines;
                let end = start + hunk.old_range.len();
                excerpts.push(
                    MultiBufferExcerpt::line_range(
                        head_source
                            .as_ref()
                            .expect("展开 hunk 必须具有 HEAD 来源")
                            .clone(),
                        hunk.old_range.clone(),
                        cx,
                    )
                    .with_diff_kind(crate::ExcerptDiffKind::Deleted)
                    .with_editable(false)
                    .with_starts_new_excerpt(false),
                );
                current = hunk.range.start;
                materialized_old_lines += hunk.old_range.len();
                Some(start..end)
            } else {
                None
            };
            let displayed_range =
                hunk.range.start + materialized_old_lines..hunk.range.end + materialized_old_lines;
            displayed_hunks.push(DiffHunk {
                range: displayed_range,
                old_range: hunk.old_range.clone(),
                kind: hunk.kind,
            });
            old_display_ranges.push(old_display_range);
        }

        if materialized_old_lines == 0 {
            if had_materialized_old_rows {
                self.set_excerpts(
                    vec![working_excerpt(0..line_count).with_starts_new_excerpt(false)],
                    cx,
                );
            }
        } else if current < line_count {
            excerpts.push(working_excerpt(current..line_count).with_starts_new_excerpt(false));
            self.set_excerpts(excerpts, cx);
        }
        let new_version = self.text_buffer(cx).read(cx).snapshot().version();
        let diff = self.diff.as_mut().expect("投影重建前 diff 状态必须存在");
        diff.display_hunks = displayed_hunks;
        diff.display_old_ranges = old_display_ranges;
        diff.display_version = Some(new_version);
        cx.notify();
        new_version != old_version
    }
}

impl DiffProjection {
    /// hunk 的当前展开状态；尚未进入投影的新 hunk 直接采用默认策略。
    fn is_expanded(&self, kind: DiffHunkKind, old_range: &Range<usize>) -> bool {
        match kind {
            DiffHunkKind::Deleted => {
                if self.expanded_by_default {
                    !self.collapsed_deleted.contains(old_range)
                } else {
                    self.expanded_deleted.contains(old_range)
                }
            }
            DiffHunkKind::Modified => {
                if self.expanded_by_default {
                    !self.collapsed_modified.contains(old_range)
                } else {
                    self.expanded_modified.contains(old_range)
                }
            }
            DiffHunkKind::Added => true,
        }
    }

    /// 按新 hunk 列表重建展开/折叠集合。
    ///
    /// 显式状态从旧 hunk 迁移：source 模式按工作区文本锚点匹配（编辑移位、base 变化都保持识别），materialized 模式按旧侧（base 版本）行范围重叠匹配；
    /// 未匹配到旧 hunk 的新 hunk 采用默认策略，真正消失的 hunk 状态被清理。
    /// 返回是否有集合发生变化。
    fn migrate_expansion_state(
        &mut self,
        old_hunks: &[DiffHunk],
        old_anchors: &[Anchor],
        new_hunks: &[DiffHunk],
        new_anchors: &[Anchor],
    ) -> bool {
        let use_anchors = !old_anchors.is_empty() && !new_anchors.is_empty();
        let matches = |old_index: usize, new_index: usize| {
            if use_anchors {
                old_anchors[old_index] == new_anchors[new_index]
            } else {
                let old = &old_hunks[old_index];
                let new = &new_hunks[new_index];
                old.kind == new.kind
                    && old.old_range.start < new.old_range.end
                    && new.old_range.start < old.old_range.end
            }
        };
        let mut expanded_deleted = Vec::new();
        let mut expanded_modified = Vec::new();
        let mut collapsed_deleted = Vec::new();
        let mut collapsed_modified = Vec::new();
        for (new_index, hunk) in new_hunks.iter().enumerate() {
            let explicitly_expanded = |kind_set: &[Range<usize>]| {
                old_hunks.iter().enumerate().any(|(old_index, old)| {
                    matches(old_index, new_index) && kind_set.contains(&old.old_range)
                })
            };
            match hunk.kind {
                DiffHunkKind::Deleted => {
                    let was_expanded = explicitly_expanded(&self.expanded_deleted);
                    let was_collapsed = explicitly_expanded(&self.collapsed_deleted);
                    if self.expanded_by_default {
                        if was_collapsed {
                            collapsed_deleted.push(hunk.old_range.clone());
                        } else {
                            expanded_deleted.push(hunk.old_range.clone());
                        }
                    } else if was_expanded {
                        expanded_deleted.push(hunk.old_range.clone());
                    }
                }
                DiffHunkKind::Modified => {
                    let was_expanded = explicitly_expanded(&self.expanded_modified);
                    let was_collapsed = explicitly_expanded(&self.collapsed_modified);
                    if self.expanded_by_default {
                        if was_collapsed {
                            collapsed_modified.push(hunk.old_range.clone());
                        } else {
                            expanded_modified.push(hunk.old_range.clone());
                        }
                    } else if was_expanded {
                        expanded_modified.push(hunk.old_range.clone());
                    }
                }
                DiffHunkKind::Added => {}
            }
        }
        let changed = self.expanded_deleted != expanded_deleted
            || self.expanded_modified != expanded_modified
            || self.collapsed_deleted != collapsed_deleted
            || self.collapsed_modified != collapsed_modified;
        self.expanded_deleted = expanded_deleted;
        self.expanded_modified = expanded_modified;
        self.collapsed_deleted = collapsed_deleted;
        self.collapsed_modified = collapsed_modified;
        changed
    }
}
