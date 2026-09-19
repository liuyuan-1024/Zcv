//! 显示链的装饰投影：把各领域的显示元数据统一投影到显示坐标。
//!
//! 装饰以「领域键 + 组合坐标范围」表达，权威仍属其领域所有者：
//! - diff hunk 与词级变化由 MultiBuffer 的投影提供；
//! - 搜索命中与宿主 hunk 由 Editor 注入锚点范围；
//! - 折叠候选（crease）由 MultiBuffer 的语法折叠投影提供。
//!
//! DisplayMap 在同一显示版本上把输入投影为显示行坐标并随快照保存；
//! EditorElement 只从 DisplaySnapshot 按视口消费，不持有显示坐标副本。

use zcv_multi_buffer::{MultiBufferAnchor, MultiBufferOffset, MultiBufferRange};

use std::ops::Range;
use std::sync::{Arc, Mutex};

use gpui::{Bounds, Pixels, SharedString};
use zcv_buffer_diff::{DiffHunkKind, DiffHunkStaging};
use zcv_multi_buffer::DisplayHunk;
use zcv_text::{ByteOffset, Line, TextRange};

use crate::scrollbar::{ScrollbarMarker, ScrollbarMarkerKind, marker_geometry};

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

/// diff 显示装饰输入：MultiBuffer 投影提供的逻辑 hunk 与展开态。
pub(crate) struct DiffDecorationInput<'a> {
    pub(crate) hunks: &'a [DisplayHunk],
    pub(crate) expanded: Vec<bool>,
    pub(crate) old_display_ranges: &'a [Option<Range<usize>>],
    pub(crate) word_diffs: &'a [Vec<(DiffHunkKind, Range<usize>)>],
}

/// 绑定一条显示快照的全部显示装饰。
///
/// 随 DisplaySnapshot 整体替换、可丢弃、可重建，不跨显示版本解释旧坐标。
#[derive(Clone)]
pub(crate) struct DisplayDecorations {
    diff: Arc<DiffDecorationSnapshot>,
    search: Option<Arc<SearchDecorationSnapshot>>,
    fold_creases: Arc<[Range<MultiBufferAnchor>]>,
}

impl std::fmt::Debug for DisplayDecorations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DisplayDecorations")
            .field("fold_creases", &self.fold_creases.len())
            .finish_non_exhaustive()
    }
}

impl DisplayDecorations {
    /// 无任何装饰的占位值；用于构造投影装饰前的基线显示快照。
    pub(crate) fn empty() -> Self {
        Self {
            diff: Arc::new(DiffDecorationSnapshot::empty()),
            search: None,
            fold_creases: Arc::from([]),
        }
    }

    pub(crate) fn new(
        snapshot: &DisplaySnapshot,
        diff: DiffDecorationInput<'_>,
        search: Option<&SearchDecorationInput>,
        editor_hunks: Arc<[EditorHunk]>,
        fold_creases: Arc<[Range<MultiBufferAnchor>]>,
    ) -> Self {
        let diff = Arc::new(DiffDecorationSnapshot::new(
            snapshot,
            diff.hunks,
            diff.expanded,
            diff.old_display_ranges,
            diff.word_diffs,
            &editor_hunks,
        ));
        let search = search.map(|input| {
            Arc::new(SearchDecorationSnapshot::from_ranges(
                snapshot,
                Arc::clone(&input.ranges),
                input.active_index,
            ))
        });
        Self {
            diff,
            search,
            fold_creases,
        }
    }

    pub(crate) fn diff(&self) -> Arc<DiffDecorationSnapshot> {
        Arc::clone(&self.diff)
    }

    pub(crate) fn search(&self) -> Option<Arc<SearchDecorationSnapshot>> {
        self.search.as_ref().map(Arc::clone)
    }

    pub(crate) fn fold_creases(&self) -> &[Range<MultiBufferAnchor>] {
        &self.fold_creases
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
    pub(crate) word_diff_highlights: Vec<(DiffHunkKind, Range<usize>)>,
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
    scrollbar_markers: ScrollbarMarkerCache,
}

#[derive(Clone, Copy, PartialEq)]
struct ScrollbarMarkerGeometryKey {
    track_top: f32,
    track_height: f32,
    scroll_per_pixel: f32,
    line_height: f32,
}

type ScrollbarMarkerCache =
    Arc<Mutex<Option<(ScrollbarMarkerGeometryKey, Arc<[ScrollbarMarker]>)>>>;

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
            scrollbar_markers: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn new(
        snapshot: &DisplaySnapshot,
        hunks: &[DisplayHunk],
        expanded: Vec<bool>,
        old_display_ranges: &[Option<Range<usize>>],
        word_diffs: &[Vec<(DiffHunkKind, Range<usize>)>],
        editor_hunks: &[EditorHunk],
    ) -> Self {
        let mut rendering =
            hunk_rendering(snapshot, hunks, &expanded, old_display_ranges, word_diffs);
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
            scrollbar_markers: Arc::new(Mutex::new(None)),
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

    pub(crate) fn scrollbar_markers(
        &self,
        track_bounds: Bounds<Pixels>,
        scroll_per_pixel: f32,
        line_height: Pixels,
    ) -> Arc<[ScrollbarMarker]> {
        let key = ScrollbarMarkerGeometryKey {
            track_top: f32::from(track_bounds.top()),
            track_height: f32::from(track_bounds.size.height),
            scroll_per_pixel,
            line_height: f32::from(line_height),
        };
        let mut cache = self
            .scrollbar_markers
            .lock()
            .expect("滚动栏差异标记缓存锁不应中毒");
        if let Some((cached_key, markers)) = &*cache
            && *cached_key == key
        {
            return Arc::clone(markers);
        }

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
        let markers = Arc::from(
            marker_geometry(
                diff_markers.chain(editor_hunk_markers),
                track_bounds,
                scroll_per_pixel,
                line_height,
            )
            .into_boxed_slice(),
        );
        *cache = Some((key, Arc::clone(&markers)));
        markers
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
    hunks: &[DisplayHunk],
    expanded: &[bool],
    old_display_ranges: &[Option<Range<usize>>],
    word_diffs: &[Vec<(DiffHunkKind, Range<usize>)>],
) -> HunkRendering {
    let mut diff_rows = Vec::new();
    let mut strips = Vec::new();
    let mut hit_regions = Vec::new();
    let mut controls = Vec::new();
    let mut expanded_rows = Vec::new();
    let mut hollow_blocks = Vec::new();
    let mut word_diff_highlights = Vec::new();
    for (index, hunk) in hunks.iter().enumerate() {
        let is_expanded = expanded.get(index).copied().unwrap_or(false);
        let staging = hunk.staging;
        let hollow = is_hollow_hunk(staging);
        // 词级背景只出现在展开态：折叠 hunk 不物化旧侧，也没有可着色的行内文本。
        if is_expanded && let Some(diffs) = word_diffs.get(index) {
            word_diff_highlights.extend(diffs.iter().cloned());
        }
        let old_rows = old_display_ranges
            .get(index)
            .and_then(|range| range.as_ref())
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

#[derive(Clone, Copy, PartialEq)]
struct MarkerGeometryKey {
    track_top: f32,
    track_height: f32,
    scroll_per_pixel: f32,
    line_height: f32,
}

/// 绑定搜索状态与显示拓扑版本的不可变装饰快照。
///
/// 视口高亮按字节范围 seek 后连续消费；
/// 滚动栏行投影只在快照建立时计算一次。
pub(crate) struct SearchDecorationSnapshot {
    ranges: Arc<[MultiBufferRange]>,
    active_index: usize,
    projected_rows: Arc<[Range<usize>]>,
    markers: Mutex<Option<(MarkerGeometryKey, Arc<[ScrollbarMarker]>)>>,
}

impl SearchDecorationSnapshot {
    fn from_ranges(
        display: &DisplaySnapshot,
        ranges: Arc<[MultiBufferRange]>,
        active_index: usize,
    ) -> Self {
        let projected_rows = ranges
            .iter()
            .flat_map(|range| display.project_text_range(*range).unwrap_or_default())
            .map(projected_row_range)
            .collect::<Arc<[_]>>();
        Self {
            ranges,
            active_index,
            projected_rows,
            markers: Mutex::new(None),
        }
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

    pub(crate) fn scrollbar_markers(
        &self,
        track_bounds: Bounds<Pixels>,
        scroll_per_pixel: f32,
        line_height: Pixels,
    ) -> Arc<[ScrollbarMarker]> {
        let key = MarkerGeometryKey {
            track_top: f32::from(track_bounds.top()),
            track_height: f32::from(track_bounds.size.height),
            scroll_per_pixel,
            line_height: f32::from(line_height),
        };
        let mut cache = self.markers.lock().expect("搜索标记缓存锁不应中毒");
        if let Some((cached_key, markers)) = &*cache
            && *cached_key == key
        {
            return Arc::clone(markers);
        }
        let markers = Arc::from(
            marker_geometry(
                self.projected_rows
                    .iter()
                    .cloned()
                    .map(|rows| (rows, ScrollbarMarkerKind::Search)),
                track_bounds,
                scroll_per_pixel,
                line_height,
            )
            .into_boxed_slice(),
        );
        *cache = Some((key, Arc::clone(&markers)));
        markers
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
mod tests {
    use super::*;
    use crate::display_map::DisplayMap;
    use gpui::{AppContext, Empty, Entity, TestAppContext, px};
    use zcv_multi_buffer::MultiBufferSnapshot;
    use zcv_text::{Buffer, BufferConfig};

    fn new_display_map(
        cx: &mut impl AppContext,
        snapshot: impl Into<MultiBufferSnapshot>,
    ) -> Entity<DisplayMap> {
        cx.new(|cx| DisplayMap::new(snapshot, cx))
    }

    fn project_display_snapshot(
        cx: &mut impl AppContext,
        snapshot: impl Into<MultiBufferSnapshot>,
    ) -> DisplaySnapshot {
        let map = new_display_map(cx, snapshot);
        cx.read_entity(&map, |map, _| map.snapshot())
    }

    impl SearchDecorationSnapshot {
        pub(crate) fn for_test(
            display: &DisplaySnapshot,
            ranges: &[MultiBufferRange],
            active_index: usize,
        ) -> Self {
            Self::from_ranges(display, Arc::from(ranges), active_index)
        }

        pub(crate) fn projected_rows_for_test(&self) -> &[Range<usize>] {
            &self.projected_rows
        }
    }

    #[gpui::test]
    fn folded_deleted_hunk_anchor_covers_all_wrapped_subrows(cx: &mut TestAppContext) {
        // 删除点逻辑行软换行拆成多个子行时，折叠的纯删除块锚点必须覆盖全部子行：
        // 三角标记落在删除点行尾（最后子行行尾 = 与下一行的边界），点击区域整行可点，而不是只落在第一个子行之间。
        let window = cx.add_window(|_, _| Empty);
        window
            .update(cx, |_, window, cx| {
                let text_system = window.text_system().clone();
                let font = window.text_style().font();
                let font_size = window.text_style().font_size.to_pixels(window.rem_size());
                let lines = (0..24)
                    .map(|i| {
                        if i == 20 {
                            "x".repeat(120)
                        } else {
                            format!("line {i}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let buffer =
                    Buffer::from_text(lines, BufferConfig::default()).expect("应创建测试 Buffer");
                let map = new_display_map(cx, buffer.snapshot());
                assert!(
                    cx.update_entity(&map, |map, cx| map.set_wrap_width(
                        Some(px(100.)),
                        font.clone(),
                        font_size,
                        &text_system,
                        cx
                    )),
                    "第 20 行应产生软换行"
                );
                let snapshot = cx.read_entity(&map, |map, _| map.snapshot());
                let start_row = snapshot
                    .line_to_display_row(Line::new(20))
                    .expect("第 20 行应可映射");
                let end_row = snapshot
                    .line_to_display_row(Line::new(21))
                    .expect("第 21 行应可映射");
                assert!(
                    end_row.get() > start_row.get() + 1,
                    "软换行行应拆成多个子行"
                );

                let hunk = DisplayHunk {
                    range: 20..20,
                    old_range: 20..21,
                    kind: DiffHunkKind::Deleted,
                    staging: DiffHunkStaging::NoStaging,
                };
                let rendered = hunk_rendering(
                    &snapshot,
                    std::slice::from_ref(&hunk),
                    &[false],
                    &[None],
                    &[],
                );
                assert_eq!(
                    rendered.hit_regions,
                    vec![(start_row.get()..end_row.get(), 0, DiffHunkKind::Deleted)],
                    "折叠删除块锚点应覆盖软换行的全部子行（三角落在删除点行尾）"
                );
            })
            .expect("测试窗口应保持可用");
    }

    #[gpui::test]
    fn folded_deleted_hunk_before_wrapped_row_anchors_to_wrapped_first_subrow(
        cx: &mut TestAppContext,
    ) {
        // 删除点行（第 15 行）无软换行、紧跟在它后面的第 16 行软换行：
        // 三角落在删除点行行尾 = 软换行第一子行行首（被删行在软换行之前）。
        let window = cx.add_window(|_, _| Empty);
        window
            .update(cx, |_, window, cx| {
                let text_system = window.text_system().clone();
                let font = window.text_style().font();
                let font_size = window.text_style().font_size.to_pixels(window.rem_size());
                let lines = (0..24)
                    .map(|i| {
                        if i == 16 {
                            "x".repeat(120)
                        } else {
                            format!("line {i}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let buffer =
                    Buffer::from_text(lines, BufferConfig::default()).expect("应创建测试 Buffer");
                let map = new_display_map(cx, buffer.snapshot());
                assert!(
                    cx.update_entity(&map, |map, cx| map.set_wrap_width(
                        Some(px(100.)),
                        font.clone(),
                        font_size,
                        &text_system,
                        cx
                    )),
                    "第 16 行应产生软换行"
                );
                let snapshot = cx.read_entity(&map, |map, _| map.snapshot());
                let del_start = snapshot
                    .line_to_display_row(Line::new(15))
                    .expect("删除点行应可映射")
                    .get();
                let wrapped_first = snapshot
                    .line_to_display_row(Line::new(16))
                    .expect("第 16 行应可映射")
                    .get();
                let wrapped_next = snapshot
                    .line_to_display_row(Line::new(17))
                    .expect("第 17 行应可映射")
                    .get();
                assert!(wrapped_next > wrapped_first + 1, "第 16 行应拆成多个子行");
                assert_eq!(wrapped_first, del_start + 1, "删除点行应为单显示行");

                let hunk = DisplayHunk {
                    range: 15..15,
                    old_range: 15..16,
                    kind: DiffHunkKind::Deleted,
                    staging: DiffHunkStaging::NoStaging,
                };
                let rendered = hunk_rendering(
                    &snapshot,
                    std::slice::from_ref(&hunk),
                    &[false],
                    &[None],
                    &[],
                );
                // 删除点行是单行 [del_start, del_start+1)，三角在该行行尾 = 软换行第一子行行首。
                assert_eq!(
                    rendered.hit_regions,
                    vec![(del_start..del_start + 1, 0, DiffHunkKind::Deleted)],
                    "删除点在软换行之前时锚点应落在软换行第一子行行首"
                );
            })
            .expect("测试窗口应保持可用");
    }

    #[test]
    fn diff_kind_for_row_matches_display_row_ranges() {
        // 输入是 diff_hunk_rows 的输出：Deleted 已从空区间展开为锚定行的单行区间。
        let diff_rows = vec![
            (2..5, DiffHunkKind::Modified, DiffHunkStaging::NoStaging),
            (7..8, DiffHunkKind::Deleted, DiffHunkStaging::NoStaging),
        ];

        assert_eq!(diff_row_for_row(&diff_rows, 1), None);
        assert_eq!(
            diff_row_for_row(&diff_rows, 2),
            Some((DiffHunkKind::Modified, DiffHunkStaging::NoStaging))
        );
        assert_eq!(
            diff_row_for_row(&diff_rows, 4),
            Some((DiffHunkKind::Modified, DiffHunkStaging::NoStaging))
        );
        assert_eq!(diff_row_for_row(&diff_rows, 5), None);
        assert_eq!(
            diff_row_for_row(&diff_rows, 7),
            Some((DiffHunkKind::Deleted, DiffHunkStaging::NoStaging))
        );
        assert_eq!(diff_row_for_row(&diff_rows, 8), None);
        assert_eq!(diff_row_for_row(&[], 0), None);
    }

    #[gpui::test]
    fn every_diff_hunk_exposes_a_control_anchor(cx: &mut TestAppContext) {
        let buffer = Buffer::from_text(
            "line0\nline1\nline2\nline3\nline4\n".into(),
            BufferConfig::default(),
        )
        .expect("应创建测试 Buffer");
        let snapshot = project_display_snapshot(cx, buffer.snapshot());
        let hunks = vec![
            DisplayHunk {
                range: 0..1,
                old_range: 0..0,
                kind: DiffHunkKind::Added,
                staging: DiffHunkStaging::NoStaging,
            },
            DisplayHunk {
                range: 2..3,
                old_range: 2..3,
                kind: DiffHunkKind::Modified,
                staging: DiffHunkStaging::NoStaging,
            },
            DisplayHunk {
                range: 4..4,
                old_range: 4..5,
                kind: DiffHunkKind::Deleted,
                staging: DiffHunkStaging::NoStaging,
            },
        ];

        let rendered = hunk_rendering(
            &snapshot,
            &hunks,
            &[false, false, false],
            &[None, None, None],
            &[],
        );
        assert_eq!(
            rendered
                .controls
                .iter()
                .map(|(rows, hunk)| {
                    let HunkControlTarget::Diff(hunk) = hunk else {
                        unreachable!("普通 diff 渲染不应产生自定义 hunk")
                    };
                    (rows.start, hunk.kind)
                })
                .collect::<Vec<_>>(),
            vec![
                (0, DiffHunkKind::Added),
                (2, DiffHunkKind::Modified),
                (4, DiffHunkKind::Deleted),
            ]
        );
        // 新增块默认折叠：只保留 gutter 竖条，不整行着色。
        assert_eq!(rendered.expanded_rows, Vec::<Range<usize>>::new());
        // 纯新增块也要登记点击区域，否则普通文档既不能展开也无法着色。
        assert!(
            rendered
                .hit_regions
                .iter()
                .any(|(_, index, kind)| *index == 0 && *kind == DiffHunkKind::Added),
            "纯新增块应可点击展开"
        );

        // 差异审阅视图（默认展开）下新增块才整行着色。
        let expanded_added = hunk_rendering(&snapshot, &hunks[..1], &[true], &[None], &[]);
        assert_eq!(expanded_added.expanded_rows, vec![0..1]);
    }

    #[gpui::test]
    fn materialized_modified_hunk_uses_real_old_and_new_document_rows(cx: &mut TestAppContext) {
        let buffer =
            Buffer::from_text("context\nold\nnew\nafter\n".into(), BufferConfig::default())
                .expect("应创建测试 Buffer");
        let snapshot = project_display_snapshot(cx, buffer.snapshot());
        let hunk = DisplayHunk {
            range: 2..3,
            old_range: 10..11,
            kind: DiffHunkKind::Modified,
            staging: DiffHunkStaging::NoStaging,
        };
        let old_ranges = vec![Some(1..2)];

        let rendered = hunk_rendering(
            &snapshot,
            std::slice::from_ref(&hunk),
            &[true],
            &old_ranges,
            &[],
        );

        assert_eq!(
            rendered.diff_rows,
            vec![
                (1..2, DiffHunkKind::Deleted, DiffHunkStaging::NoStaging),
                (2..3, DiffHunkKind::Added, DiffHunkStaging::NoStaging)
            ]
        );
        assert_eq!(
            rendered.strips,
            vec![(1..3, DiffHunkKind::Modified, DiffHunkStaging::NoStaging)]
        );
        assert_eq!(
            rendered.controls,
            vec![(1..3, HunkControlTarget::Diff(hunk))]
        );
        assert_eq!(
            rendered.hit_regions,
            vec![(1..3, 0, DiffHunkKind::Modified)],
            "物化旧侧与普通编辑器共用 gutter 折叠入口"
        );
    }

    #[gpui::test]
    fn word_diff_highlights_only_render_for_expanded_hunks(cx: &mut TestAppContext) {
        // 词级背景只在展开态出现：折叠的修改块没有物化旧侧，也就没有行内变化文本可着色。
        let buffer = Buffer::from_text("old\nnew\n".into(), BufferConfig::default())
            .expect("应创建测试 Buffer");
        let snapshot = project_display_snapshot(cx, buffer.snapshot());
        let hunks = vec![DisplayHunk {
            range: 1..2,
            old_range: 0..1,
            kind: DiffHunkKind::Modified,
            staging: DiffHunkStaging::NoStaging,
        }];
        let word_diffs = vec![vec![
            (DiffHunkKind::Deleted, 0..3),
            (DiffHunkKind::Added, 4..7),
        ]];

        let collapsed = hunk_rendering(&snapshot, &hunks, &[false], &[Some(0..1)], &word_diffs);
        assert_eq!(
            collapsed.word_diff_highlights,
            Vec::<(DiffHunkKind, Range<usize>)>::new()
        );

        let expanded = hunk_rendering(&snapshot, &hunks, &[true], &[Some(0..1)], &word_diffs);
        assert_eq!(expanded.word_diff_highlights, word_diffs[0]);
    }

    #[gpui::test]
    fn staging_drives_hollow_blocks(cx: &mut TestAppContext) {
        // hunk_rendering 把暂存语义透传到行标记与 gutter 竖条，渲染端据此选空心 / 实心。
        let buffer = Buffer::from_text("a\nb\nc\n".into(), BufferConfig::default())
            .expect("应创建测试 Buffer");
        let snapshot = project_display_snapshot(cx, buffer.snapshot());
        let staged = DisplayHunk {
            range: 1..3,
            old_range: 1..3,
            kind: DiffHunkKind::Added,
            staging: DiffHunkStaging::Staged,
        };
        let rendered = hunk_rendering(&snapshot, &[staged], &[true], &[None], &[]);
        assert_eq!(
            rendered.diff_rows,
            vec![(1..3, DiffHunkKind::Added, DiffHunkStaging::Staged)]
        );
        assert_eq!(
            rendered.strips,
            vec![(1..3, DiffHunkKind::Added, DiffHunkStaging::Staged)]
        );
        // 多行 hollow 只产出一个连续块，边框按块边界合并，不会逐行描边叠加。
        assert_eq!(
            rendered.hollow_blocks,
            vec![1..3],
            "多行已暂存 hunk 应合并为单个 hollow 块"
        );
        // 实心（未暂存）不产生任何 hollow 块。
        let unstaged = DisplayHunk {
            range: 1..3,
            old_range: 1..3,
            kind: DiffHunkKind::Added,
            staging: DiffHunkStaging::NoStaging,
        };
        let rendered = hunk_rendering(&snapshot, &[unstaged], &[true], &[None], &[]);
        assert!(rendered.hollow_blocks.is_empty());
    }

    #[gpui::test]
    fn hunk_click_regions_do_not_depend_on_staging(cx: &mut TestAppContext) {
        // 点击展开只由 hunk 类型决定；已暂存 / 未暂存只影响配色，避免形成双轨。
        let buffer = Buffer::from_text("a\nb\nc\n".into(), BufferConfig::default())
            .expect("应创建测试 Buffer");
        let snapshot = project_display_snapshot(cx, buffer.snapshot());
        for staging in [
            DiffHunkStaging::Staged,
            DiffHunkStaging::Unstaged,
            DiffHunkStaging::NoStaging,
        ] {
            let hunk = DisplayHunk {
                range: 0..1,
                old_range: 0..0,
                kind: DiffHunkKind::Added,
                staging,
            };
            let rendered = hunk_rendering(&snapshot, &[hunk], &[true], &[None], &[]);
            assert_eq!(
                rendered.hit_regions,
                vec![(0..1, 0, DiffHunkKind::Added)],
                "点击区域不应随暂存语义变化：{staging:?}"
            );
        }
    }
}
