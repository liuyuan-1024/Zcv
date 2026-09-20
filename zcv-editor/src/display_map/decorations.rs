//! 显示链的装饰投影：把各领域的显示元数据统一投影到显示坐标。
//!
//! 装饰以「领域键 + 组合坐标范围」表达，权威仍属其领域所有者：
//! - diff hunk 与词级变化由 MultiBuffer 的投影提供；
//! - 搜索命中与宿主 hunk 由 Editor 注入锚点范围。
//!
//! 折叠候选是独立的 `CreaseMap` 快照，不经过本模块。
//! DisplayMap 在同一显示版本上把输入投影为显示行坐标并随快照保存；
//! EditorElement 只从 DisplaySnapshot 按视口消费，不持有显示坐标副本。

use zcv_multi_buffer::{MultiBufferOffset, MultiBufferRange};

use std::ops::Range;
use std::sync::{Arc, OnceLock};

use gpui::SharedString;
use zcv_buffer_diff::{DiffHunkKind, DiffHunkStaging};
use zcv_multi_buffer::{DiffDisplaySnapshot, DisplayHunk, ResolvedDiffHunk, WordDiffs};
use zcv_text::{ByteOffset, Line, TextRange};

use crate::scrollbar::ScrollbarMarkerKind;

use super::{DisplayRange, DisplaySnapshot};

/// 宿主注入的 hunk 展示数据。
/// 文本内容仍由 MultiBuffer 持有，Editor 只消费范围和视觉语义。
#[derive(Clone, Debug, PartialEq)]
pub struct EditorHunk {
    pub id: SharedString,
    pub range: MultiBufferRange,
    pub parts: Arc<[EditorHunkPart]>,
}

impl EditorHunk {
    /// 由一段 Git 冲突标记构造未解决的冲突 hunk。
    ///
    /// `outer` 是包含冲突标记的完整源范围，`theirs_start` 是传入侧正文的起始偏移；
    /// `map_offset` 把源字节偏移映射到目标文档坐标（普通编辑器为恒等映射，项目差异视图使用 excerpt 输出偏移）。范围无效时返回 `None`。
    pub fn conflict(
        id: impl Into<SharedString>,
        outer: Range<usize>,
        theirs_start: usize,
        map_offset: impl Fn(usize) -> usize,
    ) -> Option<Self> {
        let range = TextRange::new(
            ByteOffset::new(map_offset(outer.start)),
            ByteOffset::new(map_offset(outer.end)),
        )
        .ok()?;
        let ours = TextRange::new(
            ByteOffset::new(map_offset(outer.start)),
            ByteOffset::new(map_offset(theirs_start)),
        )
        .ok()?;
        let theirs = TextRange::new(
            ByteOffset::new(map_offset(theirs_start)),
            ByteOffset::new(map_offset(outer.end)),
        )
        .ok()?;
        Some(Self {
            id: id.into(),
            range: range.into(),
            parts: vec![
                EditorHunkPart {
                    range: ours.into(),
                    content_kind: DiffHunkKind::Deleted,
                    marker_kind: EditorHunkMarkerKind::Conflict,
                },
                EditorHunkPart {
                    range: theirs.into(),
                    content_kind: DiffHunkKind::Added,
                    marker_kind: EditorHunkMarkerKind::Conflict,
                },
            ]
            .into(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorHunkPart {
    pub range: MultiBufferRange,
    pub content_kind: DiffHunkKind,
    pub marker_kind: EditorHunkMarkerKind,
}

/// Git hunk 的外围状态标记。
///
/// 内容背景由 `content_kind` 决定；
/// gutter 和滚动条则由这里的状态决定，因此冲突可以复用新增/删除的内容背景，同时保留冲突专有的标记颜色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorHunkMarkerKind {
    /// 普通 Git diff hunk，颜色由具体差异类型决定。
    Diff(DiffHunkKind),
    /// 未解决的 Git 冲突，使用主题中的冲突颜色。
    Conflict,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HunkControlTarget {
    Diff(DisplayHunk),
    Editor(EditorHunk),
}

/// 搜索命中的显示装饰输入：已解析到组合坐标的匹配范围与活动序号。
///
/// 搜索会话（query、活动匹配、替换）仍由 Editor 的搜索状态拥有；
/// 这里只承载供显示链投影的范围输入，不构成第二份搜索权威。
#[derive(Clone, Debug)]
pub(crate) struct SearchDecorationInput {
    pub(crate) ranges: Arc<[MultiBufferRange]>,
    pub(crate) active_index: usize,
}

impl SearchDecorationInput {
    pub(crate) fn new(ranges: Arc<[MultiBufferRange]>, active_index: usize) -> Self {
        Self {
            ranges,
            active_index,
        }
    }
}

/// 绑定一条显示快照的全部显示装饰。
///
/// 随 DisplaySnapshot 整体替换、可丢弃、可重建，不跨显示版本解释旧坐标。
#[derive(Clone)]
pub(crate) struct DisplayDecorations {
    diff: Arc<DiffDecorationSnapshot>,
    search: Option<Arc<SearchDecorationSnapshot>>,
}

impl std::fmt::Debug for DisplayDecorations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DisplayDecorations").finish_non_exhaustive()
    }
}

impl DisplayDecorations {
    /// 无任何装饰的占位值；用于构造投影装饰前的基线显示快照。
    pub(crate) fn empty() -> Self {
        Self {
            diff: Arc::new(DiffDecorationSnapshot::empty()),
            search: None,
        }
    }

    pub(crate) fn new(
        snapshot: &DisplaySnapshot,
        diff: Option<&DiffDisplaySnapshot>,
        search: Option<&SearchDecorationInput>,
        editor_hunks: Arc<[EditorHunk]>,
        cached_diff: Option<Arc<DiffDecorationSnapshot>>,
    ) -> Self {
        let diff = cached_diff.unwrap_or_else(|| {
            Arc::new(DiffDecorationSnapshot::new(snapshot, diff, &editor_hunks))
        });
        let search = search.map(|input| {
            Arc::new(SearchDecorationSnapshot::from_ranges(
                Arc::clone(&input.ranges),
                input.active_index,
            ))
        });
        Self { diff, search }
    }

    pub(crate) fn diff(&self) -> Arc<DiffDecorationSnapshot> {
        Arc::clone(&self.diff)
    }

    pub(crate) fn search(&self) -> Option<Arc<SearchDecorationSnapshot>> {
        self.search.as_ref().map(Arc::clone)
    }

    /// 只替换 diff 域，搜索域保持原快照。
    pub(crate) fn with_diff(&self, diff: Arc<DiffDecorationSnapshot>) -> Self {
        Self {
            diff,
            search: self.search.clone(),
        }
    }

    /// 只替换搜索域，diff 域保持原快照。
    pub(crate) fn with_search(&self, search: Option<Arc<SearchDecorationSnapshot>>) -> Self {
        Self {
            diff: Arc::clone(&self.diff),
            search,
        }
    }
}

/// hunks 的单遍渲染数据：行标记 / 竖条 / 点击区域共用同一份行区间计算。
#[derive(Clone)]
pub(crate) struct HunkRendering {
    /// 行标记：显示行区间、行级色、以及 hunk 相对 index 的暂存语义。
    pub(crate) diff_rows: Vec<(Range<usize>, DiffHunkKind, DiffHunkStaging)>,
    /// gutter 竖条：与 diff_rows 同源，按暂存语义区分空心 / 实心。
    pub(crate) strips: Vec<(Range<usize>, DiffHunkKind, DiffHunkStaging)>,
    pub(crate) hit_regions: Vec<(Range<usize>, usize, DiffHunkKind)>,
    /// hunk 操作栏的锚定显示范围；控件取范围起点作为右上角所在行。
    pub(crate) controls: Vec<(Range<usize>, HunkControlTarget)>,
    /// 宿主注入的 hunk 行范围与视觉语义。
    pub(crate) editor_hunks: Vec<(Range<usize>, EditorHunk)>,
    pub(crate) editor_hunk_parts: Vec<(Range<usize>, DiffHunkKind, EditorHunkMarkerKind)>,
    /// 需要整行差异背景的显示行区间；只有展开态包含（新增块的展开态由注入方决定）。
    pub(crate) expanded_rows: Vec<Range<usize>>,
    /// 展开的 hollow（已暂存）连续块；边框只在块首 / 末行按该行背景色绘制，相邻行之间不画线。
    pub(crate) hollow_blocks: Vec<Range<usize>>,
    /// 展开 hunk 的词级变化片段（组合文档字节范围 + 新增/删除色）。
    pub(crate) word_diff_highlights: WordDiffs,
}

/// 绑定一条显示快照的 diff 装饰派生状态。
///
/// 逻辑 hunk 只在显示映射版本变化时投影一次。
/// 滚动帧通过 `viewport` 和 `visible_word_diff_highlights` 消费已有的显示行数据，不重新访问逻辑 hunk 或重新执行逻辑坐标到显示坐标的转换。
#[derive(Clone)]
pub(crate) struct DiffDecorationSnapshot {
    rendering: HunkRendering,
    expanded: Vec<bool>,
    projected_word_diff_highlights: Vec<(DiffHunkKind, DisplayRange)>,
    scrollbar_diff_markers: Vec<(Range<usize>, DiffHunkKind)>,
}

impl DiffDecorationSnapshot {
    fn empty() -> Self {
        Self {
            rendering: HunkRendering {
                diff_rows: Vec::new(),
                strips: Vec::new(),
                hit_regions: Vec::new(),
                controls: Vec::new(),
                editor_hunks: Vec::new(),
                editor_hunk_parts: Vec::new(),
                expanded_rows: Vec::new(),
                hollow_blocks: Vec::new(),
                word_diff_highlights: Vec::new(),
            },
            expanded: Vec::new(),
            projected_word_diff_highlights: Vec::new(),
            scrollbar_diff_markers: Vec::new(),
        }
    }

    pub(crate) fn new(
        snapshot: &DisplaySnapshot,
        diff: Option<&DiffDisplaySnapshot>,
        editor_hunks: &[EditorHunk],
    ) -> Self {
        let resolved: Vec<ResolvedDiffHunk> =
            diff.into_iter().flat_map(|diff| diff.resolved()).collect();
        Self::from_resolved(snapshot, resolved, editor_hunks)
    }

    /// 由已解析的组合绝对坐标输入构建装饰快照（生产与渲染单元测试共用入口）。
    pub(crate) fn from_resolved(
        snapshot: &DisplaySnapshot,
        resolved: Vec<ResolvedDiffHunk>,
        editor_hunks: &[EditorHunk],
    ) -> Self {
        let expanded: Vec<bool> = resolved.iter().map(|hunk| hunk.expanded).collect();
        let mut rendering = hunk_rendering(snapshot, resolved.into_iter());
        rendering.editor_hunks = editor_hunk_rendering(snapshot, editor_hunks);
        rendering.controls.extend(
            rendering
                .editor_hunks
                .iter()
                .map(|(rows, hunk)| (rows.clone(), HunkControlTarget::Editor(hunk.clone()))),
        );
        rendering
            .controls
            .sort_by_key(|(rows, _)| (rows.start, rows.end));
        rendering.editor_hunk_parts = editor_hunk_part_rendering(snapshot, editor_hunks);
        let projected_word_diff_highlights = rendering
            .word_diff_highlights
            .iter()
            .filter_map(|(kind, range)| {
                let text_range = MultiBufferRange::new(
                    MultiBufferOffset::new(range.start),
                    MultiBufferOffset::new(range.end),
                )
                .ok()?;
                Some(
                    snapshot
                        .project_text_range(text_range)
                        .ok()?
                        .into_iter()
                        .map(|range| (*kind, range)),
                )
            })
            .flatten()
            .collect();

        let mut scrollbar_diff_markers = rendering
            .diff_rows
            .iter()
            .map(|(rows, kind, _)| (rows.clone(), *kind))
            .collect::<Vec<_>>();
        for (rows, index, kind) in &rendering.hit_regions {
            if *kind == DiffHunkKind::Deleted && !expanded.get(*index).copied().unwrap_or(false) {
                scrollbar_diff_markers.push((rows.clone(), DiffHunkKind::Deleted));
            }
        }

        Self {
            rendering,
            expanded,
            projected_word_diff_highlights,
            scrollbar_diff_markers,
        }
    }

    pub(crate) fn rendering_for_viewport(&self, viewport: Range<usize>) -> HunkRendering {
        HunkRendering {
            diff_rows: visible_triples(&self.rendering.diff_rows, &viewport),
            strips: visible_triples(&self.rendering.strips, &viewport),
            hit_regions: visible_triples(&self.rendering.hit_regions, &viewport),
            controls: Vec::new(),
            editor_hunks: visible_pairs(&self.rendering.editor_hunks, &viewport),
            editor_hunk_parts: visible_triples(&self.rendering.editor_hunk_parts, &viewport),
            expanded_rows: visible_ranges(&self.rendering.expanded_rows, &viewport),
            hollow_blocks: visible_ranges(&self.rendering.hollow_blocks, &viewport),
            word_diff_highlights: Vec::new(),
        }
    }

    pub(crate) fn visible_controls(
        &self,
        viewport: &Range<usize>,
    ) -> Vec<(usize, Range<usize>, HunkControlTarget)> {
        let start = self
            .rendering
            .controls
            .partition_point(|(rows, _)| rows.end <= viewport.start);
        self.rendering.controls[start..]
            .iter()
            .enumerate()
            .take_while(|(_, (rows, _))| rows.start < viewport.end)
            .filter(|(_, (rows, _))| ranges_overlap(rows, viewport))
            .map(|(index, (rows, target))| (start + index, rows.clone(), target.clone()))
            .collect()
    }

    pub(crate) fn visible_word_diff_highlights<'a>(
        &'a self,
        viewport: &'a Range<usize>,
    ) -> impl Iterator<Item = (DiffHunkKind, DisplayRange)> + 'a {
        let start = self
            .projected_word_diff_highlights
            .partition_point(|(_, range)| {
                range.end().row().get().max(range.start().row().get() + 1) <= viewport.start
            });
        self.projected_word_diff_highlights[start..]
            .iter()
            .take_while(|(_, range)| range.start().row().get() < viewport.end)
            .filter(|(_, range)| {
                let range_start = range.start().row().get();
                let range_end = range.end().row().get().max(range_start + 1);
                range_start < viewport.end && range_end > viewport.start
            })
            .copied()
    }

    pub(crate) fn is_expanded(&self, index: usize) -> bool {
        self.expanded.get(index).copied().unwrap_or(false)
    }

    /// 滚动条 diff 标记的显示行范围；几何换算由 Editor 的后台任务完成。
    pub(crate) fn scrollbar_marker_ranges(
        &self,
    ) -> impl Iterator<Item = (Range<usize>, ScrollbarMarkerKind)> + '_ {
        let diff_markers = self.scrollbar_diff_markers.iter().map(|(rows, kind)| {
            (
                rows.clone(),
                ScrollbarMarkerKind::Git(EditorHunkMarkerKind::Diff(*kind)),
            )
        });
        let editor_hunk_markers = self
            .rendering
            .editor_hunk_parts
            .iter()
            .map(|(rows, _, marker)| (rows.clone(), ScrollbarMarkerKind::Git(*marker)));
        diff_markers.chain(editor_hunk_markers)
    }
}

fn ranges_overlap(left: &Range<usize>, right: &Range<usize>) -> bool {
    left.start < right.end && right.start < left.end
}

fn visible_pairs<T: Clone>(
    items: &[(Range<usize>, T)],
    viewport: &Range<usize>,
) -> Vec<(Range<usize>, T)> {
    let start = items.partition_point(|(range, _)| range.end <= viewport.start);
    items[start..]
        .iter()
        .take_while(|(range, _)| range.start < viewport.end)
        .filter(|(range, _)| ranges_overlap(range, viewport))
        .map(|(range, value)| (range.clone(), value.clone()))
        .collect()
}

fn visible_ranges(items: &[Range<usize>], viewport: &Range<usize>) -> Vec<Range<usize>> {
    let start = items.partition_point(|range| range.end <= viewport.start);
    items[start..]
        .iter()
        .take_while(|range| range.start < viewport.end)
        .filter(|range| ranges_overlap(range, viewport))
        .cloned()
        .collect()
}

fn visible_triples<T: Clone, U: Clone>(
    items: &[(Range<usize>, T, U)],
    viewport: &Range<usize>,
) -> Vec<(Range<usize>, T, U)> {
    let start = items.partition_point(|(range, _, _)| range.end <= viewport.start);
    items[start..]
        .iter()
        .take_while(|(range, _, _)| range.start < viewport.end)
        .filter(|(range, _, _)| ranges_overlap(range, viewport))
        .map(|(range, first, second)| (range.clone(), first.clone(), second.clone()))
        .collect()
}

/// hunks（逻辑行）→ 行级渲染数据，单遍遍历产出五份视图：
///
/// - `diff_rows`：行标记（gutter 指示，wrap 下行映射出的全部显示行都覆盖）
/// - `strips`：竖条范围与状态色（竖条颜色不随展开变化）
/// - `hit_regions`：可点击色带区域（显示行范围 + 点击目标 old_range + 类型）
/// - `expanded_rows`：整行差异背景数据源（只有展开态着色；折叠 hunk 仅保留 gutter 竖条）
/// - `hollow_blocks`：展开的已暂存（hollow）连续块（边框按块边界合并绘制，不做逐行描边）
/// - `word_diff_highlights`：展开态 hunk 的词级变化片段（组合文档字节范围；折叠态无）
///
/// 覆盖终点取 hunk 之后第一行的行首显示行（end 行首显示行 − 1 即 hunk 最后一个显示行，左闭右开区间 [start, end) 恰好盖住全部 wrap 片段）；
/// hunk 到达文件末尾时以显示快照行数为终点。
/// 纯删除 hunk（空范围）：折叠时行内不做标记（gutter 红色三角提示），展开后标记物化的旧侧行；
/// 修改 hunk 展开后：旧侧行按删除色、修改行按新增色（base 旧行红、新行绿）。
/// 映射失败（越界等）跳过该 hunk。
pub(crate) fn hunk_rendering(
    snapshot: &DisplaySnapshot,
    resolved: impl Iterator<Item = ResolvedDiffHunk>,
) -> HunkRendering {
    let mut diff_rows = Vec::new();
    let mut strips = Vec::new();
    let mut hit_regions = Vec::new();
    let mut controls = Vec::new();
    let mut expanded_rows = Vec::new();
    let mut hollow_blocks = Vec::new();
    let mut word_diff_highlights = Vec::new();
    for (index, resolved) in resolved.enumerate() {
        let hunk = resolved.hunk;
        let is_expanded = resolved.expanded;
        let staging = hunk.staging;
        let hollow = is_hollow_hunk(staging);
        // 词级背景只出现在展开态：折叠 hunk 不物化旧侧，也没有可着色的行内文本。
        if is_expanded {
            word_diff_highlights.extend(resolved.word_diffs.iter().cloned());
        }
        let old_rows = resolved
            .old_range
            .as_ref()
            .and_then(|range| logical_rows(snapshot, range));
        let new_rows = logical_rows(snapshot, &hunk.range);
        match hunk.kind {
            DiffHunkKind::Added => {
                if let Some(rows) = new_rows {
                    diff_rows.push((rows.clone(), DiffHunkKind::Added, staging));
                    strips.push((rows.clone(), DiffHunkKind::Added, staging));
                    hit_regions.push((rows.clone(), index, DiffHunkKind::Added));
                    if is_expanded {
                        expanded_rows.push(rows.clone());
                        if hollow {
                            hollow_blocks.push(rows.clone());
                        }
                    }
                    controls.push((rows, HunkControlTarget::Diff(hunk.clone())));
                }
            }
            DiffHunkKind::Deleted => {
                if is_expanded && let Some(rows) = old_rows {
                    diff_rows.push((rows.clone(), DiffHunkKind::Deleted, staging));
                    strips.push((rows.clone(), DiffHunkKind::Deleted, staging));
                    expanded_rows.push(rows.clone());
                    if hollow {
                        hollow_blocks.push(rows.clone());
                    }
                    hit_regions.push((rows.clone(), index, DiffHunkKind::Deleted));
                    controls.push((rows, HunkControlTarget::Diff(hunk.clone())));
                } else if let Some(rows) =
                    old_rows.or_else(|| logical_anchor_rows(snapshot, hunk.range.start))
                {
                    hit_regions.push((rows.clone(), index, DiffHunkKind::Deleted));
                    controls.push((rows, HunkControlTarget::Diff(hunk.clone())));
                }
            }
            DiffHunkKind::Modified => {
                if is_expanded && let (Some(old_rows), Some(new_rows)) = (&old_rows, &new_rows) {
                    diff_rows.push((old_rows.clone(), DiffHunkKind::Deleted, staging));
                    diff_rows.push((new_rows.clone(), DiffHunkKind::Added, staging));
                    expanded_rows.push(old_rows.clone());
                    expanded_rows.push(new_rows.clone());
                    let rows = old_rows.start.min(new_rows.start)..old_rows.end.max(new_rows.end);
                    strips.push((rows.clone(), DiffHunkKind::Modified, staging));
                    if hollow {
                        hollow_blocks.push(rows.clone());
                    }
                    hit_regions.push((rows.clone(), index, DiffHunkKind::Modified));
                    controls.push((rows, HunkControlTarget::Diff(hunk.clone())));
                } else if let Some(rows) = new_rows {
                    diff_rows.push((rows.clone(), DiffHunkKind::Modified, staging));
                    strips.push((rows.clone(), DiffHunkKind::Modified, staging));
                    hit_regions.push((rows.clone(), index, DiffHunkKind::Modified));
                    controls.push((rows, HunkControlTarget::Diff(hunk.clone())));
                }
            }
        }
    }
    HunkRendering {
        diff_rows,
        strips,
        hit_regions,
        controls,
        expanded_rows,
        hollow_blocks,
        word_diff_highlights,
        editor_hunks: Vec::new(),
        editor_hunk_parts: Vec::new(),
    }
}

pub(crate) fn editor_hunk_rendering(
    snapshot: &DisplaySnapshot,
    hunks: &[EditorHunk],
) -> Vec<(Range<usize>, EditorHunk)> {
    hunks
        .iter()
        .filter_map(|hunk| {
            let projected = snapshot
                .project_text_range(hunk.range)
                .ok()?
                .into_iter()
                .next()?;
            let start = projected.start().row().get();
            let end = projected.end().row().get();
            let end = end.max(start + 1);
            Some((start..end, hunk.clone()))
        })
        .collect()
}

pub(crate) fn editor_hunk_part_rendering(
    snapshot: &DisplaySnapshot,
    hunks: &[EditorHunk],
) -> Vec<(Range<usize>, DiffHunkKind, EditorHunkMarkerKind)> {
    hunks
        .iter()
        .flat_map(|hunk| hunk.parts.iter())
        .filter_map(|part| {
            let projected = snapshot
                .project_text_range(part.range)
                .ok()?
                .into_iter()
                .next()?;
            let start = projected.start().row().get();
            let end = projected.end().row().get().max(start + 1);
            Some((start..end, part.content_kind, part.marker_kind))
        })
        .collect()
}

fn logical_rows(snapshot: &DisplaySnapshot, range: &Range<usize>) -> Option<Range<usize>> {
    if range.is_empty() {
        return None;
    }
    let start = snapshot.line_to_display_row(Line::new(range.start))?.get();
    let end = snapshot
        .line_to_display_row(Line::new(range.end))
        .map_or_else(|| snapshot.line_count(), |row| row.get());
    Some(start..end.max(start + 1))
}

fn logical_anchor_rows(snapshot: &DisplaySnapshot, line: usize) -> Option<Range<usize>> {
    // 折叠的纯删除块锚定到删除点逻辑行：
    // 软换行拆出的全部子行都属于该行，范围延伸到下一逻辑行的行首（与 logical_rows 的终点换算一致），点击区域与三角标记因此覆盖整行而不是仅第一个子行。
    let start = snapshot.line_to_display_row(Line::new(line))?.get();
    let end = snapshot
        .line_to_display_row(Line::new(line + 1))
        .map_or_else(|| snapshot.line_count(), |row| row.get());
    Some(start..end.max(start + 1))
}

/// 查询显示行所属的 diff 类型与暂存语义（gutter 与内容背景共用；线性扫描，hunks 数量级小）。
pub(crate) fn diff_row_for_row(
    diff_rows: &[(Range<usize>, DiffHunkKind, DiffHunkStaging)],
    row: usize,
) -> Option<(DiffHunkKind, DiffHunkStaging)> {
    diff_rows
        .iter()
        .find(|(range, _, _)| range.contains(&row))
        .map(|(_, kind, staging)| (*kind, *staging))
}

/// 该 hunk 是否用空心色条 + 透明行背景（已暂存）。
///
/// 只有完全进入 index 的 hunk 是空心；未暂存 / 部分暂存 / 无 index 参照一律实心。
pub(crate) fn is_hollow_hunk(staging: DiffHunkStaging) -> bool {
    matches!(staging, DiffHunkStaging::Staged)
}

/// 绑定搜索状态与显示拓扑版本的不可变装饰快照。
///
/// 视口高亮按字节范围 seek 后连续消费；
/// 滚动栏行投影只在需要绘制滚动条标记时惰性构建，
/// 组合文档不渲染搜索标记，因此不触发整份命中的行投影。
pub(crate) struct SearchDecorationSnapshot {
    ranges: Arc<[MultiBufferRange]>,
    active_index: usize,
    projected_rows: OnceLock<Arc<[Range<usize>]>>,
}

impl SearchDecorationSnapshot {
    pub(crate) fn from_ranges(ranges: Arc<[MultiBufferRange]>, active_index: usize) -> Self {
        Self {
            ranges,
            active_index,
            projected_rows: OnceLock::new(),
        }
    }

    /// 显示行投影；只服务滚动轴标记，按需构建并随快照缓存。
    fn projected_rows(&self, display: &DisplaySnapshot) -> &Arc<[Range<usize>]> {
        self.projected_rows.get_or_init(|| {
            self.ranges
                .iter()
                .flat_map(|range| display.project_text_range(*range).unwrap_or_default())
                .map(projected_row_range)
                .collect::<Arc<[_]>>()
        })
    }

    pub(crate) fn visible_ranges(
        &self,
        viewport: Range<usize>,
    ) -> impl Iterator<Item = (usize, MultiBufferRange)> + '_ {
        let start = self
            .ranges
            .partition_point(|range| range.end().get() <= viewport.start);
        self.ranges[start..]
            .iter()
            .enumerate()
            .take_while(move |(_, range)| range.start().get() < viewport.end)
            .map(move |(index, range)| (start + index, *range))
    }

    pub(crate) fn is_active(&self, index: usize) -> bool {
        index == self.active_index
    }

    /// 搜索命中滚动条标记的显示行范围；仅在单文档编辑器上消费。
    pub(crate) fn scrollbar_marker_ranges<'a>(
        &'a self,
        display: &'a DisplaySnapshot,
    ) -> impl Iterator<Item = (Range<usize>, ScrollbarMarkerKind)> + 'a {
        self.projected_rows(display)
            .iter()
            .cloned()
            .map(|rows| (rows, ScrollbarMarkerKind::Search))
    }
}

fn projected_row_range(range: DisplayRange) -> Range<usize> {
    let start = range.start();
    let end = range.end();
    let end_line = if end.row() == start.row() || end.column().get() != 0 {
        end.row().get().saturating_add(1)
    } else {
        end.row().get()
    };
    start.row().get()..end_line
}

#[cfg(test)]
#[path = "test/decorations_tests.rs"]
mod tests;
