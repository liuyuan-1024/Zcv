//! MultiBuffer 的 git diff 投影：把版本化的 BufferDiff 结果物化为 excerpts 与显示坐标。
//!
//! 普通编辑器与多文件投影（Git 差异视图）共用同一套物化：
//! 宿主注入同一工作区源快照对应的 BufferDiff 和已装配的可见 working 行范围；
//! 本层只消费其 BufferDiffSnapshot，按展开状态把旧侧行物化为只读 excerpt，并派生组合坐标显示 hunks。
//!
//! diff 状态（base/working、版本、hunk、pending、操作）全部归 BufferDiff 所有；
//! hunk 身份随输出变换节点（Excerpt）承载，输出坐标由游标推导；
//! 展开/折叠与显示路径归本层所有，不进入 diff 快照。

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, Context, Entity, Subscription};
use sum_tree::{Bias, SumTree};
use zcv_language::{LanguageBuffer, LanguageBufferEvent};
use zcv_text::{Anchor, BufferId, ByteOffset, Line, Snapshot, TextRange};

use crate::{
    DiffTransform, DiffTransformHunkInfo, DiffTransformHunkSide, DiffTransformSummary, Excerpt,
    ExcerptDiffKind, ExcerptIndex, ExcerptItemIndex, ExcerptRange, ExcerptSummary, MappingPosition,
    MultiBuffer, MultiBufferCursor, MultiBufferEvent, MultiBufferSnapshot, PathKey, SourceTexts,
    mapping_count, projection_item_topology_equal, snapshot_range_summary,
};
use zcv_buffer_diff::{
    BufferDiff, BufferDiffEvent, DiffHunk, DiffHunkKind, DiffHunkStaging, DiffRefresh,
};

/// 单个 hunk 的词级变化片段集合：组合文档字节范围 + 新增/删除色。
pub type WordDiffs = Vec<(DiffHunkKind, Range<usize>)>;

/// 编辑器投影使用的显示 hunk（组合文档行坐标）。
///
/// range 与 old_range 都是组合文档中的逻辑行范围；源文本定位由 DiffHunkSource 提供。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DisplayHunk {
    pub range: Range<usize>,
    pub old_range: Range<usize>,
    pub kind: DiffHunkKind,
    /// 相对 index 参照的暂存语义；无 index 参照时为 NoStaging（实心）。
    pub staging: DiffHunkStaging,
}

/// 一个文件的 diff 注入项：预创建的 diff 实体 + 文档视图装配的 excerpt 范围。
///
/// diff 实体由 GitStore 按 (working, base) 共享；显示配置由注入方（视图）持有。
#[derive(Clone)]
pub struct DiffFile {
    /// 权威 diff 实体（GitStore 预创建并共享）。
    pub diff: Entity<BufferDiff>,
    /// 组合文档中的显示路径（文件标题与导航定位）。
    pub display_path: PathBuf,
    /// 由文档视图装配的 working 行范围；本层只在这些范围内物化 diff 变换。
    pub excerpt_ranges: Vec<Range<usize>>,
}

/// 显示 hunk 对应的源定位（hunk 操作与导航用）。
#[derive(Clone)]
pub struct DiffHunkSource {
    /// 权威 diff 实体（操作实现与快照来源）。
    pub diff: Entity<BufferDiff>,
    /// 新侧源文件路径（绝对）。
    pub path: PathBuf,
    /// 当前工作区快照中的稳定操作范围；整文件新增块没有源 hunk。
    pub range: Option<Range<Anchor>>,
}

/// 一个文件的显示状态：diff 配置、展开覆盖、订阅与物化版本。
///
/// diff 结果由 BufferDiff 持有；本结构只持有显示层状态，随文件在 MultiBuffer 中增删而创建销毁。
pub(crate) struct DiffState {
    diff: Entity<BufferDiff>,
    /// 组合文档中的显示路径（文件标题与导航定位）。
    display_path: PathKey,
    /// 调用方装配的 working 行范围；MultiBuffer 不决定 diff 视图的裁剪策略。
    excerpt_ranges: Vec<Range<usize>>,
    /// 显示层拥有的展开/折叠状态，与版本化 diff 结果分离。
    expansion: DiffExpansionState,
    /// BufferDiff 订阅；只作为守卫随 DiffState 生命周期创建销毁，不直接读取。
    _subscription: Subscription,
    /// diff 基线/参照文本的订阅：它们只是 diff 输入，文本变化只触发 BufferDiff 重算，不进入组合源订阅表，因此不会形成第二个组合投影推进入口。
    _input_subscriptions: Vec<Subscription>,
    /// 上次物化时该文件的 diff 版本；None 表示尚未物化进组合文档。
    revision: Option<u64>,
    /// 替换 diff 实体后，新 BufferDiff 的首次后台结果返回前暂存的展开状态。
    pending_expansion_state: Option<DiffExpansionState>,
}

impl DiffState {
    fn new(
        diff: Entity<BufferDiff>,
        display_path: PathKey,
        excerpt_ranges: Vec<Range<usize>>,
        cx: &mut Context<MultiBuffer>,
    ) -> Self {
        let input_subscriptions = Self::subscribe_inputs(&diff, cx);
        let subscription = cx.subscribe(&diff, |this, diff, event, cx| {
            let BufferDiffEvent::DiffChanged {
                refresh,
                changed_range,
            } = event;
            this.diff_changed(diff.entity_id(), *refresh, changed_range.clone(), cx);
        });
        Self {
            diff,
            display_path,
            excerpt_ranges,
            expansion: DiffExpansionState::default(),
            _subscription: subscription,
            _input_subscriptions: input_subscriptions,
            revision: None,
            pending_expansion_state: None,
        }
    }

    /// 订阅 diff 的基线/参照文本，文本变化只触发该 diff 重算。
    fn subscribe_inputs(
        diff: &Entity<BufferDiff>,
        cx: &mut Context<MultiBuffer>,
    ) -> Vec<Subscription> {
        let inputs = {
            let diff = diff.read(cx);
            [diff.base_source().cloned(), diff.index_source().cloned()]
        };
        let mut seen = HashSet::new();
        inputs
            .into_iter()
            .flatten()
            .filter(|source| seen.insert(source.entity_id()))
            .map(|source| {
                let source_id = source.entity_id();
                cx.subscribe(&source, move |this, _, _event: &LanguageBufferEvent, cx| {
                    this.recompute_diff_for_source(source_id, DiffRefresh::RebuildProjection, cx);
                })
            })
            .collect()
    }
}

/// 一个 path 当前投影中的 hunk 身份索引。
///
/// 它是从 `MultiBuffer` 权威投影派生的不可变值，只保存稳定序号需要的身份数据。
/// hunk 行、字节范围和词级片段由当前输出变换树按需解析，不在此处复制几何状态。
#[derive(Clone, Debug, PartialEq)]
struct PathDiffDisplay {
    path: PathKey,
    sources: Arc<[DisplayHunkSource]>,
}

/// 一条已解析为组合绝对坐标的 diff 显示输入。
pub struct ResolvedDiffHunk {
    pub hunk: DisplayHunk,
    pub old_range: Option<Range<usize>>,
    pub expanded: bool,
    pub word_diffs: WordDiffs,
}

/// diff 投影的 hunk 身份索引。
///
/// 坐标和装饰数据直接从不可变组合快照的输出变换树按需读取，不在这里复制第二份几何状态。
#[derive(Clone, Debug)]
pub struct DiffDisplaySnapshot {
    /// hunk 身份集合变化时递增；普通文本编辑只改变坐标，不改变此索引。
    version: u64,
    segments: Arc<[PathDiffDisplay]>,
    hunk_indices: Arc<HashMap<(gpui::EntityId, Option<Anchor>), usize>>,
}

impl Default for DiffDisplaySnapshot {
    fn default() -> Self {
        Self {
            version: 0,
            segments: Arc::from(Vec::<PathDiffDisplay>::new()),
            hunk_indices: Arc::new(HashMap::new()),
        }
    }
}

impl DiffDisplaySnapshot {
    pub fn version(&self) -> u64 {
        self.version
    }

    fn from_segments(version: u64, segments: Vec<PathDiffDisplay>) -> Self {
        let mut hunk_indices = HashMap::new();
        let mut index = 0;
        for segment in &segments {
            for source in segment.sources.iter() {
                hunk_indices.insert((source.working, source.hunk_start), index);
                index += 1;
            }
        }
        Self {
            version,
            segments: Arc::from(segments),
            hunk_indices: Arc::new(hunk_indices),
        }
    }

    fn index_of(&self, working: gpui::EntityId, hunk_start: Option<Anchor>) -> Option<usize> {
        self.hunk_indices.get(&(working, hunk_start)).copied()
    }

    pub fn len(&self) -> usize {
        self.hunk_indices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 按扁平显示 hunk 序号取源定位。
    fn source_at(&self, index: usize) -> Option<DisplayHunkSource> {
        let mut remaining = index;
        for segment in self.segments.iter() {
            if remaining < segment.sources.len() {
                return Some(segment.sources[remaining].clone());
            }
            remaining -= segment.sources.len();
        }
        None
    }
}
impl MultiBufferSnapshot {
    /// 当前组合映射内与逻辑行范围相交的 hunk；只查询视口变换节点及其相邻 hunk 侧。
    pub fn diff_hunks_in_lines(&self, lines: Range<usize>) -> Vec<(usize, ResolvedDiffHunk)> {
        let Some(diff_display) = self.diff_display.as_deref() else {
            return Vec::new();
        };
        resolved_diff_hunks_in_lines(
            &self.excerpts,
            &self.diff_transforms,
            &self.excerpt_sources,
            diff_display,
            lines,
        )
    }

    /// 组合文档完整显示 hunk 数；序号只在当前 diff 投影快照内稳定。
    pub fn diff_hunk_count(&self) -> usize {
        self.diff_display.as_ref().map_or(0, |diff| diff.len())
    }

    /// 按当前权威变换树派生全部 hunk，用于低频的完整列表 API。
    pub fn resolved_diff_hunks(&self) -> Vec<(usize, ResolvedDiffHunk)> {
        self.diff_hunks_in_lines(0..self.line_count().saturating_add(1))
    }
}

type DiffHunkKey = (gpui::EntityId, Option<Anchor>);

fn resolved_diff_hunks_in_lines<S: SourceTexts + ?Sized>(
    excerpts: &SumTree<Excerpt>,
    transforms: &SumTree<DiffTransform>,
    sources: &S,
    diff_display: &DiffDisplaySnapshot,
    lines: Range<usize>,
) -> Vec<(usize, ResolvedDiffHunk)> {
    if lines.is_empty() {
        return Vec::new();
    }

    let mut candidates = HashMap::<DiffHunkKey, Vec<usize>>::new();
    let mut cursor = MultiBufferCursor::new(excerpts, transforms);
    cursor.seek_output_line(lines.start, Bias::Left);
    if cursor.item().is_none() {
        cursor.prev();
    }
    while let Some((_excerpt, transform)) = cursor.item() {
        let item_start = cursor.start().lines;
        if item_start >= lines.end {
            break;
        }
        let item_end = item_start + transform.transform_summary().output.text.lines.max(1);
        if item_end > lines.start {
            for info in transform.hunks() {
                candidates
                    .entry((info.working, info.hunk_start))
                    .or_default()
                    .push(cursor.start().index);
            }
        }
        cursor.next();
    }

    let mut resolved = Vec::with_capacity(candidates.len());
    for (key, item_indices) in candidates {
        let Some(index) = diff_display.index_of(key.0, key.1) else {
            continue;
        };
        let mut accum = None;
        let mut inspected = HashSet::new();
        for item_index in item_indices {
            let first = item_index.saturating_sub(1);
            let last = item_index.saturating_add(1);
            for neighbor in first..=last {
                if !inspected.insert(neighbor) {
                    continue;
                }
                let mut cursor = MultiBufferCursor::new(excerpts, transforms);
                cursor.seek_excerpt_index(neighbor);
                let Some((excerpt, transform)) = cursor.item() else {
                    continue;
                };
                for info in transform
                    .hunks()
                    .iter()
                    .filter(|info| (info.working, info.hunk_start) == key)
                {
                    let accum = accum.get_or_insert_with(|| HunkAccum::new(info));
                    accum.expanded = info.expanded;
                    let at = cursor.start();
                    let content_lines =
                        excerpt.text_summary.lines + usize::from(excerpt.adds_newline);
                    let content_range = at.lines..(at.lines + content_lines).max(at.lines + 1);
                    match info.side {
                        DiffTransformHunkSide::Content => {
                            accum.content_range = Some(content_range);
                            let source_text = sources
                                .source_text(excerpt.source_index)
                                .expect("diff excerpt 必须引用当前源快照");
                            let output_start = at.bytes;
                            let source_start = excerpt.source_range.start().get();
                            accum
                                .new_word_diffs
                                .extend(info.buffer_word_diffs.iter().filter_map(|diff| {
                                    let start = output_start
                                        + diff.start.resolve_in(source_text).ok()?.get()
                                        - source_start;
                                    let end = output_start
                                        + diff.end.resolve_in(source_text).ok()?.get()
                                        - source_start;
                                    Some((DiffHunkKind::Added, start..end))
                                }));
                        }
                        DiffTransformHunkSide::Old => {
                            accum.old_range = Some(content_range);
                            if info.expanded {
                                let output_start = at.bytes;
                                let source_start = excerpt.source_range.start().get();
                                accum.old_word_diffs.extend(info.base_word_diffs.iter().map(
                                    |diff| {
                                        let start =
                                            output_start + info.base_byte_start + diff.start
                                                - source_start;
                                        let end = output_start + info.base_byte_start + diff.end
                                            - source_start;
                                        (DiffHunkKind::Deleted, start..end)
                                    },
                                ));
                            }
                        }
                        DiffTransformHunkSide::BoundaryStart => {
                            accum.boundary_start = Some(at.lines);
                        }
                        DiffTransformHunkSide::BoundaryEnd => {
                            accum.boundary_end = Some(at.lines + content_lines);
                        }
                    }
                }
            }
        }

        let Some(accum) = accum else {
            continue;
        };
        let Some(range) = accum
            .content_range
            .or_else(|| accum.old_range.as_ref().map(|range| range.end..range.end))
            .or_else(|| accum.boundary_start.map(|line| line..line))
            .or_else(|| accum.boundary_end.map(|line| line..line))
        else {
            continue;
        };
        let mut word_diffs = accum.old_word_diffs;
        word_diffs.extend(accum.new_word_diffs);
        resolved.push((
            index,
            ResolvedDiffHunk {
                hunk: DisplayHunk {
                    range,
                    old_range: accum.base_lines,
                    kind: accum.kind,
                    staging: accum.staging,
                },
                old_range: accum.old_range,
                expanded: accum.expanded,
                word_diffs,
            },
        ));
    }
    resolved.sort_by_key(|(index, _)| *index);
    resolved
}

/// 一个文件内用户显式切换过展开状态的 hunk。
///
/// 只保存与展开策略默认值不同的显式覆盖，按「变化类型 + hunk 起点工作区 Anchor」标识；
/// 未覆盖的 hunk 一律采用默认值。新增/修改/删除共用同一份状态，不为类型建立平行集合。
#[derive(Default, Clone)]
struct DiffExpansionState {
    overrides: Vec<HunkExpansionOverride>,
}

#[derive(Clone)]
struct HunkExpansionOverride {
    kind: DiffHunkKind,
    /// hunk 身份：与输出变换节点承载的 hunk 相同的工作区 Anchor。
    ///
    /// 只保存 Anchor，不保存裸偏移；working 版本推进后在当前工作区快照上重新解析再比较，
    /// 同一 hunk 不因工作区编辑而丢失展开状态。
    hunk_start: Anchor,
    expanded: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DisplayHunkSource {
    /// 稳定文件键：working 源实体，不随组合文档序号变化。
    working: gpui::EntityId,
    /// hunk 起点的工作区 Anchor；无 hunk 身份的合成节点为 None。
    hunk_start: Option<Anchor>,
    kind: DiffHunkKind,
}

/// 一个 hunk 在显示层需要的行坐标（从 anchor 与旧侧字节范围派生）。
#[derive(Clone)]
struct ResolvedHunk {
    buffer_range: Range<Anchor>,
    /// working 源行范围。
    buffer_lines: Range<usize>,
    /// base 文本行范围（旧侧物化；不再作为展开状态身份）。
    base_lines: Range<usize>,
    kind: DiffHunkKind,
    staging: DiffHunkStaging,
    /// 旧侧字节范围起点（base_word_diffs 的相对基准）。
    base_byte_start: usize,
    /// 新侧词级变化片段（working 锚点）。
    buffer_word_diffs: Vec<Range<Anchor>>,
    /// 旧侧词级变化片段（相对 diff_base_byte_range.start）。
    base_word_diffs: Vec<Range<usize>>,
}

struct ExistingExcerptGroup {
    start_index: usize,
    end_index: usize,
    lines: Option<Range<usize>>,
}

/// 单次游标遍历中按 hunk 身份聚合的输出范围与词级片段。
struct HunkAccum {
    kind: DiffHunkKind,
    staging: DiffHunkStaging,
    base_lines: Range<usize>,
    expanded: bool,
    content_range: Option<Range<usize>>,
    old_range: Option<Range<usize>>,
    boundary_start: Option<usize>,
    boundary_end: Option<usize>,
    old_word_diffs: WordDiffs,
    new_word_diffs: WordDiffs,
}

impl HunkAccum {
    fn new(info: &DiffTransformHunkInfo) -> Self {
        Self {
            kind: info.kind,
            staging: info.staging,
            base_lines: info.base_lines.clone(),
            expanded: info.expanded,
            content_range: None,
            old_range: None,
            boundary_start: None,
            boundary_end: None,
            old_word_diffs: Vec::new(),
            new_word_diffs: Vec::new(),
        }
    }
}

/// 一个投影片段的裁剪与标注选项。
struct ExcerptShape {
    /// diff 语义；None 表示不标注类型。
    diff_kind: Option<ExcerptDiffKind>,
    /// 是否作为逻辑 excerpt 的起点（每个可见窗口的首个物理片段）。
    starts_logical_excerpt: bool,
    /// 是否允许空片段（空文件占位行、删除点占位行）。
    allow_empty: bool,
}

struct ExcerptMaterializer<'a> {
    excerpts: &'a mut Vec<ExcerptRange>,
    display_path: &'a Path,
    /// 同一 diff 文件的旧/新侧物理来源都属于 working Buffer 的一个逻辑显示实体。
    buffer_id: BufferId,
}

impl ExcerptMaterializer<'_> {
    /// 构造并推入一个投影片段，并标注它承担的 hunk 身份。
    ///
    /// 返回未挂载的 hunk 身份：片段被空行策略跳过时，调用方据此恢复待挂载状态（通常是纯删除边界）。
    fn push(
        &mut self,
        lines: Range<usize>,
        text: &Snapshot,
        source: &Entity<LanguageBuffer>,
        shape: ExcerptShape,
        hunks: Vec<DiffTransformHunkInfo>,
    ) -> Vec<DiffTransformHunkInfo> {
        let Some(mut excerpt) = projected_excerpt(
            source,
            text,
            lines,
            self.display_path,
            self.buffer_id,
            shape,
        ) else {
            return hunks;
        };
        for hunk in hunks {
            excerpt = excerpt.with_diff_hunk(hunk);
        }
        self.excerpts.push(excerpt);
        Vec::new()
    }
}

/// 一个 hunk 节点携带的身份与显示元数据。
fn hunk_info(
    working: gpui::EntityId,
    side: DiffTransformHunkSide,
    hunk: &ResolvedHunk,
    expanded: bool,
) -> DiffTransformHunkInfo {
    DiffTransformHunkInfo {
        working,
        hunk_start: Some(hunk.buffer_range.start),
        side,
        kind: hunk.kind,
        staging: hunk.staging,
        base_lines: hunk.base_lines.clone(),
        base_byte_start: hunk.base_byte_start,
        buffer_word_diffs: hunk.buffer_word_diffs.clone(),
        base_word_diffs: hunk.base_word_diffs.clone(),
        expanded,
    }
}

impl MultiBuffer {
    /// 按路径增量挂接一个文件的 diff。
    ///
    /// 新路径按路径顺序追加；
    /// 同路径的 diff 变化只替换该文件的 excerpts 并迁移展开状态，不重建整份组合文档。
    /// 返回 true 表示组合文档已更新；
    /// diff 仍在后台计算时返回 false，结果到达后自动物化。
    pub fn add_diff(&mut self, file: DiffFile, cx: &mut Context<Self>) -> bool {
        let existing = self.diffs.iter().position(|current| {
            current.display_path.as_path() == file.display_path
                || current.diff.read(cx).working().entity_id()
                    == file.diff.read(cx).working().entity_id()
        });
        if let Some(index) = existing {
            let current = &self.diffs[index];
            if current.diff.entity_id() == file.diff.entity_id()
                && current.display_path.as_path() == file.display_path
                && current.excerpt_ranges == file.excerpt_ranges
            {
                return false;
            }
            return self.replace_diff_file(index, file, cx);
        }
        // 新路径按显示路径顺序插入：位于末尾走增量追加，插到中间按路径 splice。
        let insert_at = self.diffs.partition_point(|current| {
            current.display_path.as_path() < file.display_path.as_path()
        });
        let len = self.diffs.len();
        if insert_at == len {
            return self.append_diff_projection(vec![file], cx);
        }
        self.insert_diff_file(insert_at, file, cx)
    }

    /// 更新 Git diff 视图装配的 excerpt 范围，并按 BufferDiff 事件范围同步投影。
    pub fn update_diff_excerpt_ranges(
        &mut self,
        display_path: &Path,
        excerpt_ranges: Vec<Range<usize>>,
        refresh: DiffRefresh,
        changed_range: Range<Anchor>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(index) = self
            .diffs
            .iter()
            .position(|file| file.display_path.as_path() == display_path)
        else {
            return false;
        };
        let ranges_changed = self.diffs[index].excerpt_ranges != excerpt_ranges;
        self.diffs[index].excerpt_ranges = excerpt_ranges;
        let diff = self.diffs[index].diff.clone();
        if self.diffs[index].revision == Some(diff.read(cx).revision()) {
            if !ranges_changed || refresh == DiffRefresh::PreserveProjection {
                return false;
            }
            self.begin_projection_sync();
            self.sync_pending_sources(cx);
            let updated = self.sync_diff_range(index, &changed_range, cx);
            self.finish_projection_sync(cx);
            return updated;
        }
        self.diff_changed(diff.entity_id(), refresh, Some(changed_range), cx);
        true
    }

    /// 同路径的 diff 实体或视图范围变化：只替换该文件的 excerpts，不重建整份组合文档。
    ///
    /// 展开状态按旧/新 hunk 迁移；新 diff 尚未算完时先登记 pending 迁移，
    /// 保留现有 excerpts，等 DiffChanged 到期后由 diff_changed 增量替换。
    fn replace_diff_file(&mut self, index: usize, file: DiffFile, cx: &mut Context<Self>) -> bool {
        let working_id_matches = self.diffs[index].diff.read(cx).working().entity_id()
            == file.diff.read(cx).working().entity_id();
        let mut next = DiffState::new(
            file.diff,
            PathKey::new(file.display_path),
            file.excerpt_ranges,
            cx,
        );
        if working_id_matches {
            if next.diff.read(cx).is_current_version_calculated(cx) {
                let new_resolved = resolve_file_hunks(&next, cx);
                let working_text = working_snapshot_for(&next, cx);
                migrate_expansion_state(
                    &self.diffs[index].expansion,
                    &new_resolved,
                    &working_text,
                    &mut next.expansion,
                );
            } else {
                next.pending_expansion_state = Some(self.diffs[index].expansion.clone());
            }
        }
        self.diffs[index] = next;
        if !self.diffs[index]
            .diff
            .read(cx)
            .is_current_version_calculated(cx)
        {
            return false;
        }
        self.replace_materialized_file(index, cx);
        true
    }

    /// 在 insert_at 处插入一个 diff 文件，只物化该文件的 excerpts 并按路径 splice。
    fn insert_diff_file(
        &mut self,
        insert_at: usize,
        file: DiffFile,
        cx: &mut Context<Self>,
    ) -> bool {
        let state = DiffState::new(
            file.diff,
            PathKey::new(file.display_path),
            file.excerpt_ranges,
            cx,
        );
        self.diffs.insert(insert_at, state);

        let expanded_by_default = self.diff_expanded_by_default;
        let mut excerpts = Vec::new();
        {
            let file = &self.diffs[insert_at];
            materialize_file(file, cx, expanded_by_default, &mut excerpts);
        }
        self.set_excerpts_for_path(excerpts, cx);
        let diff = self.diffs[insert_at].diff.read(cx);
        self.diffs[insert_at].revision = diff
            .is_current_version_calculated(cx)
            .then_some(diff.revision());
        if insert_at < self.diff_materialized_files {
            self.diff_materialized_files += 1;
        }
        self.refresh_diff_display(cx);
        true
    }

    /// 移除指定显示路径的 diff；用于 Git 状态中不再存在的文件。
    ///
    /// 按路径增量移除该文件的 excerpts（不动其余文件的源订阅），再只重算显示坐标。
    /// 移除最后一个文件时回退到整份清理路径（可能恢复整文件 excerpt）。
    pub fn remove_diff(&mut self, path: &Path, cx: &mut Context<Self>) -> bool {
        if self.diff.is_none() {
            return false;
        }
        let Some(file_index) = self
            .diffs
            .iter()
            .position(|file| file.display_path.as_path() == path)
        else {
            return false;
        };
        if self.diffs.len() == 1 {
            self.set_diff_files(Vec::new(), cx);
            return true;
        }

        // 被移除路径在 excerpt 流中的区间（映射按源路径升序；显示路径可能被裁剪为相对路径）。
        // 身份必须与物化时一致：有文件路径按路径，无路径的匿名 Buffer 用 buffer_id。
        let source_path = {
            let working = self.diffs[file_index].diff.read(cx).working().clone();
            let working = working.read(cx);
            PathKey::for_buffer(working.file_path(), working.buffer_id())
        };
        self.remove_excerpts_for_path(source_path.as_path(), cx);

        // DiffState 持有自己的订阅，移除即取消订阅；hunk 身份随节点消失，无需下标顺延。
        self.diffs.remove(file_index);
        self.refresh_diff_display(cx);
        self.diff_materialized_files = self.diffs.len();
        cx.notify();
        true
    }

    /// 清除全部 diff，使组合文档回到无 diff 状态。
    pub fn clear_diffs(&mut self, cx: &mut Context<Self>) -> bool {
        if self.diff.is_none() && self.singleton_source.is_none() {
            return false;
        }
        self.set_diff_files(Vec::new(), cx)
    }

    /// 用给定文件列表整体替换投影；结构性变化（刷新、展开策略切换）的重建入口。
    ///
    /// 按路径增量更新请使用 Self::add_diff / Self::remove_diff。
    pub fn set_diff_files(&mut self, inputs: Vec<DiffFile>, cx: &mut Context<Self>) -> bool {
        if inputs.is_empty()
            && let Some(source) = self.singleton_source.clone()
        {
            self.diff = None;
            self.diffs.clear();
            let line_count = source.read(cx).text_snapshot().line_count();
            self.clear(cx);
            self.set_excerpts_for_path(
                vec![ExcerptRange::line_range(source, 0..line_count, cx)],
                cx,
            );
            return true;
        }
        // 路径顺序的尾部追加：
        // 已有文件身份与顺序不变时，只登记新增文件，由 diff 计算完成事件增量物化，避免整份组合文档重建。
        let append_from = self.diff.as_ref().and_then(|_| {
            let old_len = self.diffs.len();
            (old_len > 0
                && inputs.len() > old_len
                && self.diffs.iter().zip(inputs.iter()).all(|(old, new)| {
                    old.display_path.as_path() == new.display_path
                        && old.diff.entity_id() == new.diff.entity_id()
                        && old.excerpt_ranges == new.excerpt_ranges
                        && old.diff.read(cx).working().entity_id()
                            == new.diff.read(cx).working().entity_id()
                }))
            .then_some(old_len)
        });
        if let Some(old_len) = append_from {
            let appended = inputs[old_len..].to_vec();
            return self.append_diff_projection(appended, cx);
        }
        let old_files = self.diff.as_mut().map(|_| std::mem::take(&mut self.diffs));
        self.diff
            .get_or_insert_with(|| Arc::new(DiffDisplaySnapshot::default()));

        let mut next_files: Vec<DiffState> = inputs
            .into_iter()
            .map(|file| {
                DiffState::new(
                    file.diff,
                    PathKey::new(file.display_path),
                    file.excerpt_ranges,
                    cx,
                )
            })
            .collect();

        for file in &mut next_files {
            let mut pending_migration = None;
            if let Some(old_files) = old_files.as_deref()
                && let Some(old_file) = old_files.iter().find(|old| {
                    old.diff.read(cx).working().entity_id()
                        == file.diff.read(cx).working().entity_id()
                })
            {
                if file.diff.read(cx).is_current_version_calculated(cx) {
                    let new_resolved = resolve_file_hunks(file, cx);
                    let working_text = working_snapshot_for(file, cx);
                    migrate_expansion_state(
                        &old_file.expansion,
                        &new_resolved,
                        &working_text,
                        &mut file.expansion,
                    );
                } else {
                    pending_migration = Some(old_file.expansion.clone());
                }
            }
            file.pending_expansion_state = pending_migration;
        }
        self.diffs = next_files;
        // 新文件的 hunk 尚未算完时，保留已物化投影，避免先清空再展示结果导致一次
        // Git 刷新产生两次可见重建；各文件的就绪状态互不影响。
        if self
            .diffs
            .iter()
            .any(|file| !file.diff.read(cx).is_current_version_calculated(cx))
        {
            return false;
        }
        self.rebuild_diff_projection(cx)
    }

    /// 设置新 hunk 的初始展开策略；用户之后的显式展开/折叠不受投影刷新覆盖。
    pub fn set_diff_hunks_expanded_by_default(&mut self, expanded: bool, cx: &mut Context<Self>) {
        self.diff
            .get_or_insert_with(|| Arc::new(DiffDisplaySnapshot::default()));
        if self.diff_expanded_by_default == expanded {
            return;
        }
        self.diff_expanded_by_default = expanded;
        // 策略切换不迁移旧状态：按新默认值重新应用（清空全部显式集合）。
        for file in &mut self.diffs {
            file.expansion = DiffExpansionState::default();
        }
        self.rebuild_diff_projection(cx);
        cx.emit(MultiBufferEvent::DiffExpansionChanged);
    }

    /// 按显示 hunk 索引切换展开/折叠（渲染层点击入口）。
    ///
    /// 折叠/展开不改变源，编辑器源锚点选区自然存活，无需返回投影重映射。
    pub fn toggle_diff_hunk_at(&mut self, display_index: usize, cx: &mut Context<Self>) {
        let expanded_by_default = self.diff_expanded_by_default;
        let Some((working, kind, hunk_start)) = self.diff.as_ref().and_then(|diff| {
            let source = diff.source_at(display_index)?;
            Some((source.working, source.kind, source.hunk_start?))
        }) else {
            return;
        };
        let Some(file_index) = self
            .diffs
            .iter()
            .position(|file| file.diff.read(cx).working().entity_id() == working)
        else {
            return;
        };
        let working_text = working_snapshot_for(&self.diffs[file_index], cx);
        self.diffs[file_index].expansion.toggle(
            kind,
            &hunk_start,
            &working_text,
            expanded_by_default,
        );
        // 只重物化该文件所在路径；其余文件及其组合坐标保持不变。
        self.replace_materialized_file(file_index, cx);
        cx.emit(MultiBufferEvent::DiffExpansionChanged);
    }

    /// base 版本变化（HEAD 变化等）后由宿主调用：旧侧坐标空间已失效，按默认策略重置展开状态。
    ///
    /// 折叠/展开不改变源，编辑器源锚点选区自然存活，无需返回投影重映射。
    pub fn reset_diff_hunk_expansion_state(&mut self, cx: &mut Context<Self>) {
        if self.diff.is_none() {
            return;
        }
        for file in &mut self.diffs {
            file.expansion = DiffExpansionState::default();
        }
        self.rebuild_diff_projection(cx);
        cx.emit(MultiBufferEvent::DiffExpansionChanged);
    }

    /// 显示坐标 hunks（组合坐标，跨文件展平）。
    ///
    /// 坐标是当前组合映射下的派生缓存，在所有会改动组合文档的路径上同步刷新；
    /// 单个文件未就绪不会让其他文件的高亮消失。
    pub fn diff_hunks(&self) -> Vec<DisplayHunk> {
        self.current_resolved_diff_hunks()
            .into_iter()
            .map(|(_, resolved)| resolved.hunk)
            .collect()
    }

    /// 已挂接 diff 的显示路径集合（按组合文档顺序）。
    pub fn diff_paths(&self) -> Vec<PathBuf> {
        self.diffs
            .iter()
            .map(|file| file.display_path.as_path().to_path_buf())
            .collect()
    }

    /// 每个 hunk 在组合文档中的旧侧显示行范围。
    pub fn diff_hunk_old_ranges(&self) -> Vec<Option<Range<usize>>> {
        self.current_resolved_diff_hunks()
            .into_iter()
            .map(|(_, resolved)| resolved.old_range)
            .collect()
    }

    /// 与 MultiBuffer::diff_hunks 平行的词级变化片段（组合文档字节范围 + 新增/删除色）。
    pub fn diff_hunk_word_diffs(&self) -> Vec<WordDiffs> {
        self.current_resolved_diff_hunks()
            .into_iter()
            .map(|(_, resolved)| resolved.word_diffs)
            .collect()
    }

    /// 与 MultiBuffer::diff_hunks 平行的展开标志（渲染层按显示 hunk 索引查询）。
    pub fn diff_hunk_expanded(&self) -> Vec<bool> {
        self.current_resolved_diff_hunks()
            .into_iter()
            .map(|(_, resolved)| resolved.expanded)
            .collect()
    }

    fn current_resolved_diff_hunks(&self) -> Vec<(usize, ResolvedDiffHunk)> {
        let Some(diff_display) = self.diff.as_deref() else {
            return Vec::new();
        };
        let end = self
            .state
            .diff_transforms
            .summary()
            .output
            .text
            .lines
            .saturating_add(1);
        resolved_diff_hunks_in_lines(
            &self.state.excerpts,
            &self.state.diff_transforms,
            self.state.sources.as_slice(),
            diff_display,
            0..end,
        )
    }

    /// 显示 hunk 到源定位（hunk 操作与导航用）。
    pub fn buffer_diff_hunk_at(&self, display_index: usize, cx: &App) -> Option<DiffHunkSource> {
        let diff = self.diff.as_ref()?;
        let source = diff.source_at(display_index)?;
        let file = self
            .diffs
            .iter()
            .find(|file| file.diff.read(cx).working().entity_id() == source.working)?;
        let entity = file.diff.clone();
        let is_created = entity.read(cx).is_created();
        let path = entity.read(cx).path().clone();
        if is_created {
            // 整文件新增块没有可供 Git 操作重解析的 hunk。
            return Some(DiffHunkSource {
                diff: entity,
                path,
                range: None,
            });
        }
        let range = source.hunk_start.and_then(|start| {
            let working = entity.read(cx).working();
            let working_text = working.read(cx).text_snapshot();
            let target = start.resolve_in(&working_text).ok()?;
            entity
                .read(cx)
                .snapshot()
                .visible_hunks()
                .iter()
                .find(|hunk| hunk.buffer_range.start.resolve_in(&working_text).ok() == Some(target))
                .map(|hunk| hunk.buffer_range.clone())
        });
        Some(DiffHunkSource {
            diff: entity,
            path,
            range,
        })
    }

    /// 查询指定 diff 文件的 working source 是否有未保存修改。
    ///
    /// dirty 状态由源 Buffer 唯一拥有；组合文档只读取该状态，用于文件级提示。
    pub fn is_diff_file_dirty(&self, path: &Path, cx: &App) -> bool {
        self.diff.as_ref().is_some_and(|_| {
            self.diffs.iter().any(|file| {
                file.diff.read(cx).path() == path
                    && file.diff.read(cx).working().read(cx).is_dirty()
            })
        })
    }

    /// 把打开请求中的 Deleted 片段换算为工作区文件中的合法定位行列（0-based）。
    ///
    /// Deleted 片段的内容来自 Git 修订文本，其字节坐标在打开的工作区文件中不存在；
    /// 经 hunk 把修订侧行号映射到工作区（新侧）行号，列沿用修订行内逻辑列，行与列都按工作区文件文本钳制到有效范围，返回值可直接用于行列导航。
    /// 非 Deleted 片段返回 None（坐标直接可用）。
    pub fn deleted_navigation_target(
        &self,
        location: &crate::ExcerptLocation,
        working_text: &Snapshot,
        cx: &App,
    ) -> Option<(usize, usize)> {
        let snapshot = self.snapshot.clone();
        // 仅处理 Deleted 片段：修订文本坐标需换算，其余片段直接可用。
        let in_deleted_excerpt =
            snapshot
                .excerpts_for_path(location.path.as_path())
                .any(|excerpt| {
                    excerpt.diff_kind() == Some(ExcerptDiffKind::Deleted)
                        && excerpt.source_range().start() <= location.source_range.start()
                        && location.source_range.end() <= excerpt.source_range().end()
                });
        if !in_deleted_excerpt {
            return None;
        }
        // 修订文本行号与列（列按 Unicode scalar 计数，与导航协议一致）。
        self.diff.as_ref()?;
        let file = self
            .diffs
            .iter()
            .find(|file| file.diff.read(cx).path() == &location.path)?;
        let base = file.diff.read(cx).base_source()?.clone();
        let base_text = base.read(cx).text_snapshot();
        let position = base_text
            .byte_to_position(location.source_range.start())
            .ok()?;
        let old_line = position.line().get();
        let column = position.column().get();
        // 包含该修订行的 hunk（旧侧行范围）。
        let resolved = resolve_file_hunks(file, cx);
        let hunk = resolved
            .iter()
            .find(|hunk| hunk.base_lines.contains(&old_line))?;
        // 修改行在 hunk 内按偏移映射；纯删除锚定变更块起点。
        let offset = old_line - hunk.base_lines.start;
        let working_line = if hunk.buffer_lines.is_empty() {
            hunk.buffer_lines.start
        } else {
            (hunk.buffer_lines.start + offset).min(hunk.buffer_lines.end - 1)
        };
        // 行与列钳制到工作区文件有效范围（修改可能让行变短）。
        let line = working_line.min(working_text.line_count().saturating_sub(1));
        let column = clamp_column_to_line(working_text, line, column);
        Some((line, column))
    }

    /// 拥有指定源的 diff；源不属于任何已挂接 diff 时返回 None。
    ///
    /// working、base 与 index 是一个 diff 的全部文本输入，三者的变化都必须回到该 diff，不能按普通组合文档源各自处理。
    fn diff_source(&self, source_id: gpui::EntityId, cx: &App) -> Option<Entity<BufferDiff>> {
        self.diff.as_ref()?;
        self.diffs.iter().find_map(|file| {
            let diff = file.diff.read(cx);
            if diff.working().entity_id() == source_id {
                return Some(file.diff.clone());
            }
            if diff
                .base_source()
                .is_some_and(|source| source.entity_id() == source_id)
                || diff
                    .index_source()
                    .is_some_and(|source| source.entity_id() == source_id)
            {
                return Some(file.diff.clone());
            }
            None
        })
    }

    pub(crate) fn diff_source_sync_ready(&self, source_id: gpui::EntityId, cx: &App) -> bool {
        let Some(diff) = self.diff_source(source_id, cx) else {
            return true;
        };
        let revision = diff.read(cx).revision();
        diff.read(cx).is_current_version_calculated(cx)
            && self.diffs.iter().any(|file| {
                file.diff.entity_id() == diff.entity_id() && file.revision == Some(revision)
            })
    }

    pub(crate) fn recompute_diff_for_source(
        &mut self,
        source_id: gpui::EntityId,
        refresh: DiffRefresh,
        cx: &mut Context<Self>,
    ) {
        let diff = self.diff_source(source_id, cx);
        if let Some(diff) = diff {
            diff.update(cx, |diff, cx| diff.recompute_with_refresh(refresh, cx));
        }
    }

    /// BufferDiff 事件入口：只有当前物化结果落后于 diff 版本时才重建。
    fn diff_changed(
        &mut self,
        diff_id: gpui::EntityId,
        refresh: DiffRefresh,
        changed_range: Option<Range<Anchor>>,
        cx: &mut Context<Self>,
    ) {
        // 对齐 Zed 的 buffer_diff_changed：先把待同步的源快照纳入当前帧，
        // 再更新 diff transform，最后只发布一次组合投影版本。
        self.begin_projection_sync();
        self.sync_pending_sources(cx);
        self.diff_changed_inner(diff_id, refresh, changed_range, cx);
        self.finish_projection_sync(cx);
    }

    fn diff_changed_inner(
        &mut self,
        diff_id: gpui::EntityId,
        refresh: DiffRefresh,
        changed_range: Option<Range<Anchor>>,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self
            .diffs
            .iter()
            .position(|file| file.diff.entity_id() == diff_id)
        else {
            return;
        };
        let diff = self.diffs[index].diff.clone();
        if !diff.read(cx).is_current_version_calculated(cx) {
            return;
        }
        let revision = diff.read(cx).revision();
        if self.diffs[index].revision == Some(revision) {
            return;
        }

        if refresh == DiffRefresh::PreserveProjection {
            self.diffs[index].revision = Some(revision);
            return;
        }
        if self.diffs[index].pending_expansion_state.is_some() {
            let old_state = self.diffs[index]
                .pending_expansion_state
                .take()
                .expect("已检查 pending expansion state 存在");
            let new_hunks = resolve_file_hunks(&self.diffs[index], cx);
            let working_text = working_snapshot_for(&self.diffs[index], cx);
            migrate_expansion_state(
                &old_state,
                &new_hunks,
                &working_text,
                &mut self.diffs[index].expansion,
            );
        }

        if self.diffs[index].revision.is_none() {
            // 新文件或显式替换的 diff 尚未进入投影时，按正常构造/替换生命周期物化。
            if self.diff_materialized_files == 0 {
                self.rebuild_diff_projection(cx);
            } else if index < self.diff_materialized_files {
                self.replace_materialized_file(index, cx);
            } else {
                let calculated_prefix = self
                    .diffs
                    .iter()
                    .take_while(|file| file.diff.read(cx).is_current_version_calculated(cx))
                    .count();
                let from = self.diff_materialized_files.min(self.diffs.len());
                if calculated_prefix > from {
                    self.append_materialized_files(from, calculated_prefix, cx);
                }
            }
            return;
        }

        if let Some(changed_range) = changed_range {
            self.sync_diff_range(index, &changed_range, cx);
        }
        // 缺少范围时只接受 BufferDiff 状态，不同步 diff 投影，也不触发整文件重建。
        // 记录已处理版本，使源文档同步可以完成。
        self.diffs[index].revision = Some(revision);
    }

    /// 按路径顺序登记追加的 diff 文件，并物化其中已计算完成的前缀。
    ///
    /// 尚未计算完成的文件只登记订阅；结果到达后由 diff_changed 增量物化。
    fn append_diff_projection(&mut self, files: Vec<DiffFile>, cx: &mut Context<Self>) -> bool {
        if files.is_empty() {
            return false;
        }
        if self.diff.is_none() {
            return self.set_diff_files(files, cx);
        }
        let next_files: Vec<DiffState> = files
            .into_iter()
            .map(|file| {
                DiffState::new(
                    file.diff,
                    PathKey::new(file.display_path),
                    file.excerpt_ranges,
                    cx,
                )
            })
            .collect();
        self.diffs.extend(next_files);
        let (from, to) = {
            let from = self.diff_materialized_files;
            let to = self
                .diffs
                .iter()
                .take_while(|file| file.diff.read(cx).is_current_version_calculated(cx))
                .count();
            (from, to)
        };
        if to > from {
            self.append_materialized_files(from, to, cx);
        }
        to == self.diffs.len()
    }

    /// 追加指定范围文件的物化结果，按路径插入组合映射，不重建已有片段。
    fn append_materialized_files(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let base_excerpt_count = mapping_count(&self.state.diff_transforms);
        let expanded_by_default = self.diff_expanded_by_default;
        let mut expected_excerpt_count = 0;
        for index in from..to {
            let mut excerpts = Vec::new();
            {
                let file = &self.diffs[index];
                materialize_file(file, cx, expanded_by_default, &mut excerpts);
            }
            expected_excerpt_count += excerpts.len();
            if !excerpts.is_empty() {
                self.set_excerpts_for_path(excerpts, cx);
            }
        }
        assert_eq!(
            mapping_count(&self.state.diff_transforms),
            base_excerpt_count + expected_excerpt_count,
            "追加 diff 物化必须全部建立组合映射"
        );
        for index in from..to {
            self.diffs[index].revision = Some(self.diffs[index].diff.read(cx).revision());
        }
        self.diff_materialized_files = to;
        self.refresh_diff_display(cx);
    }

    /// 原地重物化单个文件，只替换其路径的 excerpts，其余路径保持不变。
    ///
    /// 用于某个文件的 diff 结果发生版本或身份变化时避免整份组合文档重建：
    /// 先按当前 hunk 收敛该文件的展开覆盖，再物化该文件，最后只重算显示坐标。
    fn replace_materialized_file(&mut self, file_index: usize, cx: &mut Context<Self>) {
        let resolved = resolve_file_hunks(&self.diffs[file_index], cx);
        let working_text = working_snapshot_for(&self.diffs[file_index], cx);
        self.diffs[file_index]
            .expansion
            .retain_for_current_hunks(&resolved, &working_text);

        let expanded_by_default = self.diff_expanded_by_default;
        let mut excerpts = Vec::new();
        {
            let file = &self.diffs[file_index];
            materialize_file(file, cx, expanded_by_default, &mut excerpts);
        }
        // 映射树按源路径排序，显示路径可能被裁剪为相对路径。
        // 身份必须与物化时一致：有文件路径按路径，无路径的匿名 Buffer 用 buffer_id。
        let path = {
            let working = self.diffs[file_index].diff.read(cx).working().clone();
            let working = working.read(cx);
            PathKey::for_buffer(working.file_path(), working.buffer_id())
        };

        let mut new_sources = Vec::new();
        let mut seen_source_ids = HashSet::new();
        for excerpt in &excerpts {
            let source_id = excerpt.source.entity_id();
            if !self.state.source_indices.contains_key(&source_id)
                && seen_source_ids.insert(source_id)
            {
                new_sources.push(excerpt.source.clone());
            }
        }
        let subscribe_ids = excerpts
            .iter()
            .filter(|excerpt| excerpt.diff_kind != Some(ExcerptDiffKind::Deleted))
            .map(|excerpt| excerpt.source.entity_id())
            .collect::<HashSet<_>>();
        self.register_sources(new_sources, &subscribe_ids, cx);

        let start_index = {
            let mut cursor = self
                .state
                .diff_transforms
                .cursor::<DiffTransformSummary>(());
            cursor.seek(&path, Bias::Left);
            cursor.start().output.count
        };
        let old_count = {
            let mut cursor = self
                .state
                .diff_transforms
                .cursor::<DiffTransformSummary>(());
            cursor.seek(&path, Bias::Right);
            cursor.start().output.count.saturating_sub(start_index)
        };
        let total = self.state.diff_transforms.summary().output.count - old_count + excerpts.len();
        let entries = self.build_entries_for_excerpts(excerpts, start_index, total, cx);

        // Diff 的新快照也可能只更新 hunk 锚点、暂存状态或词级差异。
        // 这些元数据要进入唯一的输出变换树，但不改变 excerpt 布局或显示行拓扑。
        let topology_unchanged = if entries.len() == old_count {
            let mut cursor =
                MultiBufferCursor::new(&self.state.excerpts, &self.state.diff_transforms);
            cursor.seek_path(&path, Bias::Left);
            entries.iter().all(|(new_excerpt, _)| {
                let Some((old_excerpt, old_transform)) = cursor.item() else {
                    return false;
                };
                let matches = old_excerpt.path == path
                    && projection_item_topology_equal(
                        old_excerpt,
                        old_transform,
                        new_excerpt,
                        new_excerpt.diff_kind == Some(ExcerptDiffKind::Deleted),
                    );
                cursor.next();
                matches
            }) && cursor
                .item()
                .is_none_or(|(excerpt, _)| excerpt.path != path)
        } else {
            false
        };
        if topology_unchanged {
            if old_count > 0 {
                let old_display_version = self.diff.as_ref().map(|display| display.version);
                self.splice_excerpt_entries(&path, entries);
                self.snapshot_dirty = true;
                self.refresh_diff_display_for_path(&path, cx);
                if self.diff.as_ref().map(|display| display.version) == old_display_version {
                    self.notify_if_not_syncing(cx);
                }
            }
        } else if entries.is_empty() {
            // 该文件已无可见 hunk（差异被消除等）时必须移除其路径的 excerpts；
            // set_excerpts_for_path 对空片段集合是空操作，无法表达“清空该路径”。
            self.remove_excerpts_for_path(path.as_path(), cx);
        } else {
            let before = self.projection_trees();
            let old_version = self.state.projection_version;
            self.state.topology_version = self.state.topology_version.wrapping_add(1);
            self.splice_excerpt_entries(&path, entries);
            self.fix_document_tail_newline();
            self.publish_projection_edit(&before, old_version);
            self.emit_projection_changed(cx);
        }
        self.diffs[file_index].revision = Some(self.diffs[file_index].diff.read(cx).revision());
        if !topology_unchanged {
            self.refresh_diff_display(cx);
        }
    }

    /// 按文档投影语义同步 BufferDiff 的受影响范围。
    fn sync_diff_range(
        &mut self,
        file_index: usize,
        changed_range: &Range<Anchor>,
        cx: &mut Context<Self>,
    ) -> bool {
        let working_id = self.diffs[file_index].diff.read(cx).working().entity_id();
        if self
            .singleton_source
            .as_ref()
            .is_some_and(|source| source.entity_id() == working_id)
        {
            self.sync_document_diff_range(file_index, changed_range, cx)
        } else {
            let excerpt_ranges = self.diffs[file_index].excerpt_ranges.clone();
            self.sync_excerpt_ranges(file_index, changed_range, &excerpt_ranges, cx)
        }
    }

    /// 普通文档维持既有完整 excerpt，只替换变更范围内的 diff transforms。
    fn sync_document_diff_range(
        &mut self,
        file_index: usize,
        changed_range: &Range<Anchor>,
        cx: &mut Context<Self>,
    ) -> bool {
        let file = &self.diffs[file_index];
        let working = file.diff.read(cx).working().clone();
        let working_id = working.entity_id();
        let working_text = working.read(cx).text_snapshot();
        let Some(change_start) = changed_range.start.resolve_in(&working_text).ok() else {
            return false;
        };
        let Some(change_end) = changed_range.end.resolve_in(&working_text).ok() else {
            return false;
        };
        let path = {
            let working = working.read(cx);
            PathKey::for_buffer(working.file_path(), working.buffer_id())
        };
        let line_count = working_text.line_count();
        let mut line_start = line_at_or_end(&working_text, change_start).min(line_count);
        let mut line_end = line_at_or_end(&working_text, change_end).min(line_count);
        if line_start == line_end && line_start < line_count {
            line_end += 1;
        } else if line_start == line_end && line_start > 0 {
            line_start -= 1;
        }

        let mut path_entries = Vec::new();
        let mut cursor = MultiBufferCursor::new(&self.state.excerpts, &self.state.diff_transforms);
        cursor.seek_path(&path, Bias::Left);
        while let Some((excerpt, transform)) = cursor.item() {
            if excerpt.path != path {
                break;
            }
            let mut intersects = false;
            if excerpt.source_id == Some(working_id) {
                let Some(source_start) = excerpt.source_range.start.resolve_in(&working_text).ok()
                else {
                    return false;
                };
                let Some(source_end) = excerpt.source_range.end.resolve_in(&working_text).ok()
                else {
                    return false;
                };
                intersects = source_start <= change_end && source_end >= change_start;
            }
            if !intersects {
                intersects = transform.hunks().iter().any(|hunk| {
                    hunk.working == working_id
                        && hunk.hunk_start.is_some_and(|start| {
                            start
                                .resolve_in(&working_text)
                                .is_ok_and(|offset| change_start <= offset && offset <= change_end)
                        })
                });
            }
            path_entries.push((
                cursor.start().index,
                excerpt.clone(),
                transform.clone(),
                intersects,
            ));
            cursor.next();
        }
        drop(cursor);

        let Some(first) = path_entries.iter().position(|entry| entry.3) else {
            return false;
        };
        let last = path_entries.iter().rposition(|entry| entry.3).unwrap();
        let start_index = path_entries[first].0;
        let end_index = path_entries[last].0 + 1;
        let old_entries = path_entries[first..=last]
            .iter()
            .map(|(_, excerpt, transform, _)| (excerpt.clone(), transform.clone()))
            .collect::<Vec<_>>();
        let first_excerpt = &old_entries[0].0;
        let last_excerpt = &old_entries.last().expect("受影响 excerpt 非空").0;
        let prefix = excerpt_slice_outside_range(
            first_excerpt,
            old_entries[0].1.hunks(),
            &self.state.sources[first_excerpt.source_index],
            working_id,
            &working_text,
            line_start,
            true,
        );
        let suffix = excerpt_slice_outside_range(
            last_excerpt,
            old_entries.last().expect("excerpt 存在").1.hunks(),
            &self.state.sources[last_excerpt.source_index],
            working_id,
            &working_text,
            line_end,
            false,
        );
        let resolved = resolve_file_hunks(&self.diffs[file_index], cx);
        self.diffs[file_index]
            .expansion
            .retain_for_current_hunks(&resolved, &working_text);
        let expanded_by_default = self.diff_expanded_by_default;
        let mut excerpts = Vec::new();
        if let Some(prefix) = prefix {
            excerpts.push(prefix);
        }
        materialize_file_in_range(
            &self.diffs[file_index],
            &resolved,
            cx,
            expanded_by_default,
            line_start..line_end,
            excerpts.is_empty(),
            &mut excerpts,
        );
        if let Some(suffix) = suffix {
            excerpts.push(suffix);
        }
        let mut new_sources = Vec::new();
        let mut seen_source_ids = HashSet::new();
        for excerpt in &excerpts {
            let source_id = excerpt.source.entity_id();
            if !self.state.source_indices.contains_key(&source_id)
                && seen_source_ids.insert(source_id)
            {
                new_sources.push(excerpt.source.clone());
            }
        }
        let subscribe_ids = excerpts
            .iter()
            .filter(|excerpt| excerpt.diff_kind != Some(ExcerptDiffKind::Deleted))
            .map(|excerpt| excerpt.source.entity_id())
            .collect::<HashSet<_>>();
        self.register_sources(new_sources, &subscribe_ids, cx);
        let total = self.state.diff_transforms.summary().output.count - (end_index - start_index)
            + excerpts.len();
        let entries = self.build_entries_for_excerpts(excerpts, start_index, total, cx);
        let topology_unchanged = old_entries.len() == entries.len()
            && old_entries.iter().zip(&entries).all(
                |((old_excerpt, old_transform), (new_excerpt, _))| {
                    projection_item_topology_equal(
                        old_excerpt,
                        old_transform,
                        new_excerpt,
                        new_excerpt.diff_kind == Some(ExcerptDiffKind::Deleted),
                    )
                },
            );
        let old_version = self.state.projection_version;
        let before = self.projection_trees();
        if !topology_unchanged {
            self.state.topology_version = self.state.topology_version.wrapping_add(1);
        }
        self.splice_excerpt_entries_range(start_index, end_index, entries);
        self.fix_document_tail_newline();
        self.diffs[file_index].revision = Some(self.diffs[file_index].diff.read(cx).revision());
        if topology_unchanged {
            self.snapshot_dirty = true;
            self.refresh_diff_display_for_path(&path, cx);
            self.notify_if_not_syncing(cx);
        } else {
            self.publish_projection_edit(&before, old_version);
            self.emit_projection_changed(cx);
            self.refresh_diff_display_for_path(&path, cx);
        }
        true
    }

    /// 只替换受影响的 Git diff excerpt 窗口；窗口范围由 Git diff 视图装配。
    fn sync_excerpt_ranges(
        &mut self,
        file_index: usize,
        changed_range: &Range<Anchor>,
        new_ranges: &[Range<usize>],
        cx: &mut Context<Self>,
    ) -> bool {
        let file = &self.diffs[file_index];
        let working = file.diff.read(cx).working().clone();
        let working_id = working.entity_id();
        let working_text = working.read(cx).text_snapshot();
        let Some(change_start) = changed_range.start.resolve_in(&working_text).ok() else {
            return false;
        };
        let Some(change_end) = changed_range.end.resolve_in(&working_text).ok() else {
            return false;
        };
        let path = {
            let working = working.read(cx);
            PathKey::for_buffer(working.file_path(), working.buffer_id())
        };
        let line_count = working_text.line_count();
        let resolved = resolve_file_hunks(&self.diffs[file_index], cx);
        let mut line_start = line_at_or_end(&working_text, change_start).min(line_count);
        let mut line_end = line_at_or_end(&working_text, change_end).min(line_count);
        if line_start == line_end && line_start < line_count {
            line_end += 1;
        } else if line_start == line_end && line_start > 0 {
            line_start -= 1;
        }
        let mut affected_lines = line_start..line_end;

        let mut path_entries = Vec::new();
        let mut cursor = MultiBufferCursor::new(&self.state.excerpts, &self.state.diff_transforms);
        cursor.seek_path(&path, Bias::Left);
        while let Some((excerpt, transform)) = cursor.item() {
            if excerpt.path != path {
                break;
            }
            path_entries.push((cursor.start().index, excerpt.clone(), transform.clone()));
            cursor.next();
        }
        drop(cursor);

        let mut groups = Vec::<ExistingExcerptGroup>::new();
        for (index, excerpt, transform) in &path_entries {
            if groups.is_empty() || excerpt.starts_logical_excerpt {
                groups.push(ExistingExcerptGroup {
                    start_index: *index,
                    end_index: *index + 1,
                    lines: None,
                });
            }
            let group = groups.last_mut().expect("excerpt group was created");
            group.end_index = *index + 1;

            let mut include_lines = |range: Range<usize>| {
                if let Some(lines) = &mut group.lines {
                    lines.start = lines.start.min(range.start);
                    lines.end = lines.end.max(range.end);
                } else {
                    group.lines = Some(range);
                }
            };
            if excerpt.source_id == Some(working_id) {
                let Some(source_start) = excerpt.source_range.start.resolve_in(&working_text).ok()
                else {
                    return false;
                };
                let Some(source_end) = excerpt.source_range.end.resolve_in(&working_text).ok()
                else {
                    return false;
                };
                include_lines(
                    line_at_or_end(&working_text, source_start).min(line_count)
                        ..line_at_or_end(&working_text, source_end).min(line_count),
                );
            }
            for hunk in transform
                .hunks()
                .iter()
                .filter(|hunk| hunk.working == working_id)
            {
                if let Some(start) = hunk.hunk_start {
                    let Ok(offset) = start.resolve_in(&working_text) else {
                        return false;
                    };
                    let line = line_at_or_end(&working_text, offset).min(line_count);
                    include_lines(line..line);
                }
            }
        }

        let mut selected_groups = vec![false; groups.len()];
        let mut selected_ranges = vec![false; new_ranges.len()];
        loop {
            let mut changed = false;
            for (index, group) in groups.iter().enumerate() {
                if selected_groups[index]
                    || group
                        .lines
                        .as_ref()
                        .is_none_or(|lines| !line_ranges_intersect(lines, &affected_lines))
                {
                    continue;
                }
                selected_groups[index] = true;
                if let Some(lines) = &group.lines {
                    affected_lines.start = affected_lines.start.min(lines.start);
                    affected_lines.end = affected_lines.end.max(lines.end);
                }
                changed = true;
            }
            for (index, lines) in new_ranges.iter().enumerate() {
                if selected_ranges[index] || !line_ranges_intersect(lines, &affected_lines) {
                    continue;
                }
                selected_ranges[index] = true;
                affected_lines.start = affected_lines.start.min(lines.start);
                affected_lines.end = affected_lines.end.max(lines.end);
                changed = true;
            }
            if !changed {
                break;
            }
        }

        let (start_index, end_index) =
            if let Some(first_group) = selected_groups.iter().position(|selected| *selected) {
                let last_group = selected_groups
                    .iter()
                    .rposition(|selected| *selected)
                    .expect("first selected group must have a last group");
                (
                    groups[first_group].start_index,
                    groups[last_group].end_index,
                )
            } else {
                let mut insertion_index = {
                    let mut cursor =
                        MultiBufferCursor::new(&self.state.excerpts, &self.state.diff_transforms);
                    cursor.seek_path(&path, Bias::Left);
                    cursor.start().index
                };
                for group in &groups {
                    if group
                        .lines
                        .as_ref()
                        .is_some_and(|lines| lines.start >= affected_lines.start)
                    {
                        insertion_index = group.start_index;
                        break;
                    }
                    insertion_index = group.end_index;
                }
                (insertion_index, insertion_index)
            };

        self.diffs[file_index]
            .expansion
            .retain_for_current_hunks(&resolved, &working_text);
        let expanded_by_default = self.diff_expanded_by_default;
        let mut excerpts = Vec::new();
        for (index, lines) in new_ranges.iter().enumerate() {
            if selected_ranges[index] {
                materialize_file_in_range(
                    &self.diffs[file_index],
                    &resolved,
                    cx,
                    expanded_by_default,
                    lines.clone(),
                    true,
                    &mut excerpts,
                );
            }
        }

        let mut new_sources = Vec::new();
        let mut seen_source_ids = HashSet::new();
        for excerpt in &excerpts {
            let source_id = excerpt.source.entity_id();
            if !self.state.source_indices.contains_key(&source_id)
                && seen_source_ids.insert(source_id)
            {
                new_sources.push(excerpt.source.clone());
            }
        }
        let subscribe_ids = excerpts
            .iter()
            .filter(|excerpt| excerpt.diff_kind != Some(ExcerptDiffKind::Deleted))
            .map(|excerpt| excerpt.source.entity_id())
            .collect::<HashSet<_>>();
        self.register_sources(new_sources, &subscribe_ids, cx);

        let old_entries = path_entries
            .into_iter()
            .filter(|(index, _, _)| *index >= start_index && *index < end_index)
            .map(|(_, excerpt, transform)| (excerpt, transform))
            .collect::<Vec<_>>();
        let total = self.state.diff_transforms.summary().output.count - (end_index - start_index)
            + excerpts.len();
        let entries = self.build_entries_for_excerpts(excerpts, start_index, total, cx);
        let topology_unchanged = old_entries.len() == entries.len()
            && old_entries.iter().zip(&entries).all(
                |((old_excerpt, old_transform), (new_excerpt, _))| {
                    projection_item_topology_equal(
                        old_excerpt,
                        old_transform,
                        new_excerpt,
                        new_excerpt.diff_kind == Some(ExcerptDiffKind::Deleted),
                    )
                },
            );
        let old_version = self.state.projection_version;
        let before = self.projection_trees();
        if !topology_unchanged {
            self.state.topology_version = self.state.topology_version.wrapping_add(1);
        }
        self.splice_excerpt_entries_range(start_index, end_index, entries);
        self.fix_document_tail_newline();
        self.diffs[file_index].revision = Some(self.diffs[file_index].diff.read(cx).revision());
        if topology_unchanged {
            self.snapshot_dirty = true;
            self.refresh_diff_display_for_path(&path, cx);
            self.notify_if_not_syncing(cx);
        } else {
            self.publish_projection_edit(&before, old_version);
            self.emit_projection_changed(cx);
            self.refresh_diff_display_for_path(&path, cx);
        }
        true
    }

    /// 只替换输出变换序号区间，并同步其对应的输入 excerpts 区间。
    fn splice_excerpt_entries_range(
        &mut self,
        start_index: usize,
        end_index: usize,
        entries: Vec<(Excerpt, Vec<DiffTransformHunkInfo>)>,
    ) {
        let mut transform_cursor = self.state.diff_transforms.cursor::<MappingPosition>(());
        let mut next_transforms = transform_cursor.slice(&ExcerptIndex(start_index), Bias::Right);
        let start_input_index = transform_cursor.start().input_item_index;
        let _replaced_transforms = transform_cursor.slice(&ExcerptIndex(end_index), Bias::Right);
        let end_input_index = transform_cursor.start().input_item_index;
        let transform_suffix = transform_cursor.suffix();

        let mut excerpt_cursor = self.state.excerpts.cursor::<ExcerptSummary>(());
        let mut next_excerpts =
            excerpt_cursor.slice(&ExcerptItemIndex(start_input_index), Bias::Right);
        let _replaced_excerpts =
            excerpt_cursor.slice(&ExcerptItemIndex(end_input_index), Bias::Right);
        let excerpt_suffix = excerpt_cursor.suffix();
        drop(excerpt_cursor);
        drop(transform_cursor);

        if !next_excerpts.is_empty() {
            let sources = &self.state.sources;
            next_excerpts.update_last(
                |entry| {
                    let Some((_, ends_with_newline)) = snapshot_range_summary(
                        &sources[entry.source_index].text,
                        entry.source_range.range(),
                    ) else {
                        return;
                    };
                    entry.adds_newline = !ends_with_newline;
                },
                (),
            );
        }
        let last_prefix_excerpt = next_excerpts.iter().last().cloned();
        if !next_transforms.is_empty() {
            let sources = &self.state.sources;
            next_transforms.update_last(
                |transform| {
                    let hunks = transform.hunks().to_vec();
                    match transform {
                        DiffTransform::BufferContent { .. } => {
                            if let Some(excerpt) = last_prefix_excerpt.as_ref() {
                                *transform = DiffTransform::from_excerpt(excerpt, hunks);
                            }
                        }
                        DiffTransform::DeletedHunk { excerpt, .. } => {
                            let ends_with_newline = snapshot_range_summary(
                                &sources[excerpt.source_index].text,
                                excerpt.source_range.range(),
                            )
                            .is_none_or(|(_, ends_with_newline)| ends_with_newline);
                            excerpt.adds_newline = !ends_with_newline;
                            let excerpt = excerpt.clone();
                            *transform = DiffTransform::deleted_hunk(excerpt, hunks);
                        }
                    }
                },
                (),
            );
        }

        next_excerpts.extend(
            entries
                .iter()
                .filter(|(excerpt, _)| excerpt.diff_kind != Some(ExcerptDiffKind::Deleted))
                .map(|(excerpt, _)| excerpt.clone()),
            (),
        );
        next_transforms.extend(
            entries.iter().map(|(excerpt, hunks)| {
                if excerpt.diff_kind == Some(ExcerptDiffKind::Deleted) {
                    DiffTransform::deleted_hunk(excerpt.clone(), hunks.clone())
                } else {
                    DiffTransform::from_excerpt(excerpt, hunks.clone())
                }
            }),
            (),
        );
        next_excerpts.append(excerpt_suffix, ());
        next_transforms.append(transform_suffix, ());
        self.state.excerpts = next_excerpts;
        self.state.diff_transforms = next_transforms;
    }

    /// 按展开状态重建 excerpts，并派生显示坐标 hunks。
    ///
    /// 返回是否推进了投影版本；选区与滚动位置由源 Anchor 在当前快照上重新解析，不经过重建映射。
    pub(crate) fn rebuild_diff_projection(&mut self, cx: &mut Context<Self>) -> bool {
        let before = self.projection_trees();
        self.rebuild_diff_projection_from(before, cx)
    }

    /// 按调用方在 hunk 生命周期变更前冻结的投影树重建 diff 投影。
    ///
    /// 冻结旧投影树用于推导本次结构变化的增量范围（对齐 excerpt 边界），不物化旧输出文本。
    /// 返回是否推进了投影版本。
    pub(crate) fn rebuild_diff_projection_from(
        &mut self,
        before: (SumTree<Excerpt>, SumTree<DiffTransform>),
        cx: &mut Context<Self>,
    ) -> bool {
        self.assert_no_active_transaction("MultiBuffer::rebuild_diff_projection");
        if self.diff.is_none() {
            return false;
        }
        let old_version = self.state.projection_version;
        let expanded_by_default = self.diff_expanded_by_default;
        let mut excerpts = Vec::new();
        for file in &self.diffs {
            materialize_file(file, cx, expanded_by_default, &mut excerpts);
        }
        let expected_excerpt_count = excerpts.len();
        self.replace_all_excerpts(excerpts, cx);
        assert_eq!(
            mapping_count(&self.state.diff_transforms),
            expected_excerpt_count,
            "diff 物化生成的 excerpt 必须全部建立组合映射"
        );
        self.publish_projection_edit(&before, old_version);
        for file in &mut self.diffs {
            let diff = file.diff.read(cx);
            file.revision = diff
                .is_current_version_calculated(cx)
                .then_some(diff.revision());
        }
        // 未就绪文件也有当前 excerpts，但保留 None 版本，结果到达后按单文件替换其投影。
        self.diff_materialized_files = self.diffs.len();
        self.refresh_diff_display(cx);
        self.state.projection_version != old_version
    }

    /// 按 path 顺序遍历组合文档，收集需要建立 hunk 身份索引的路径。
    fn diff_display_paths(&self) -> Vec<PathKey> {
        let mut paths = Vec::new();
        let mut seen = HashSet::new();
        let mut cursor = MultiBufferCursor::new(&self.state.excerpts, &self.state.diff_transforms);
        cursor.seek_output(ByteOffset::ZERO, sum_tree::Bias::Left);
        while let Some((excerpt, _)) = cursor.item() {
            if seen.insert(excerpt.path.clone()) {
                paths.push(excerpt.path.clone());
            }
            cursor.next();
        }
        paths
    }

    /// 从当前组合映射全量派生每个 path 的 hunk 身份索引；只用于低频拓扑变化。
    fn derive_diff_display_segments(&self) -> Vec<PathDiffDisplay> {
        self.diff_display_paths()
            .into_iter()
            .map(|path| self.derive_diff_display_for_path(&path))
            .collect()
    }

    /// 只收集一个 path 的 hunk 身份；显示坐标之后从组合变换树按需派生。
    fn derive_diff_display_for_path(&self, path: &PathKey) -> PathDiffDisplay {
        let mut seen = HashSet::<DiffHunkKey>::new();
        let mut sources = Vec::new();
        let mut cursor = MultiBufferCursor::new(&self.state.excerpts, &self.state.diff_transforms);
        // 用 Left 偏置从边界开始遍历：整份删除的零长度边界节点也必须被访问。
        cursor.seek_path(path, sum_tree::Bias::Left);
        while let Some((excerpt, transform)) = cursor.item() {
            if &excerpt.path != path {
                break;
            }
            for info in transform.hunks() {
                if seen.insert((info.working, info.hunk_start)) {
                    sources.push(DisplayHunkSource {
                        working: info.working,
                        hunk_start: info.hunk_start,
                        kind: info.kind,
                    });
                }
            }
            cursor.next();
        }
        PathDiffDisplay {
            path: path.clone(),
            sources: Arc::from(sources),
        }
    }

    /// diff hunk 身份变化时只替换对应 path 的序号索引；坐标由变换树独立派生。
    pub(crate) fn refresh_diff_display_for_path(&mut self, path: &PathKey, cx: &mut Context<Self>) {
        let Some(current) = self.diff.as_ref() else {
            return;
        };
        let Some(index) = current
            .segments
            .iter()
            .position(|segment| &segment.path == path)
        else {
            // 该 path 不在 excerpt 投影中（如无片段的 base/index 修订源被编辑）。
            // 路径身份索引的增删由 excerpt 物化路径负责。
            return;
        };
        let segment = self.derive_diff_display_for_path(path);
        if current.segments[index] == segment {
            self.snapshot_dirty = true;
            self.notify_if_not_syncing(cx);
            return;
        }
        let version = current.version.wrapping_add(1);
        let mut segments = current.segments.to_vec();
        segments[index] = segment;
        // hunk 身份索引是投影拓扑的派生输入；显示几何由权威变换树按需读取。
        self.diff = Some(Arc::new(DiffDisplaySnapshot::from_segments(
            version, segments,
        )));
        self.snapshot_dirty = true;
        self.notify_if_not_syncing(cx);
    }

    /// 在低频 diff 拓扑变化后按当前组合映射重建 hunk 身份索引。
    ///
    /// 普通源编辑复用身份索引，不进入这里。
    pub(crate) fn refresh_diff_display(&mut self, cx: &mut Context<Self>) {
        let Some(current) = self.diff.as_ref() else {
            return;
        };
        let segments = self.derive_diff_display_segments();
        if current.segments.as_ref() == segments.as_slice() {
            self.snapshot_dirty = true;
            self.notify_if_not_syncing(cx);
            return;
        }
        let version = current.version.wrapping_add(1);
        self.diff = Some(Arc::new(DiffDisplaySnapshot::from_segments(
            version, segments,
        )));
        self.snapshot_dirty = true;
        self.notify_if_not_syncing(cx);
    }
}

/// 解析一个文件的当前 hunk 为显示行坐标。
///
/// 用完整 hunks，不用 pending 抑制后的集合：pending 只表达"正在后台暂存"，
/// 改变的是 hunk 的状态标记，不应改变 excerpt 结构（对齐 Zed 的 hunks_intersecting_range：遍历完整 hunks，pending 只贡献 has_pending）。
fn resolve_file_hunks(file: &DiffState, cx: &App) -> Vec<ResolvedHunk> {
    let entity = file.diff.clone();
    let (working_text, base_text, hunks) = {
        let diff = entity.read(cx);
        let working_text = diff.working().read(cx).text_snapshot();
        let base_text = diff.base_source().map(|base| base.read(cx).text_snapshot());
        let hunks = diff.snapshot().hunks().to_vec();
        (working_text, base_text, hunks)
    };
    hunks
        .iter()
        .map(|hunk| resolve_hunk(hunk, &working_text, base_text.as_ref()))
        .collect()
}

/// 该文件 working 源当前的文本快照。
///
/// hunk 起点 Anchor 与展开覆盖身份都在这份快照上重新解析；它始终是工作区源的最新快照，
/// 因此不早于任何由该源派生的 Anchor 版本。
fn working_snapshot_for(file: &DiffState, cx: &App) -> Snapshot {
    file.diff.read(cx).working().read(cx).text_snapshot()
}

/// 把 anchor hunk 展开为显示层需要的行坐标与旧侧字节范围。
fn resolve_hunk(hunk: &DiffHunk, working: &Snapshot, base: Option<&Snapshot>) -> ResolvedHunk {
    // hunk 定位是工作区 Anchor：
    // 后台 diff 尚未按最新文本重算时它可能落后于当前快照，必须解析到目标快照，不能把创建偏移当作当前行坐标。
    let buffer_start = hunk
        .buffer_range
        .start
        .resolve_in(working)
        .unwrap_or_else(|_| hunk.buffer_range.start.offset());
    let buffer_end = hunk
        .buffer_range
        .end
        .resolve_in(working)
        .unwrap_or_else(|_| hunk.buffer_range.end.offset());
    let buffer_lines = line_at_or_end(working, buffer_start)..line_at_or_end(working, buffer_end);
    let base_lines = base.map_or(0..0, |base| {
        line_at_or_end(base, ByteOffset::new(hunk.diff_base_byte_range.start))
            ..line_at_or_end(base, ByteOffset::new(hunk.diff_base_byte_range.end))
    });
    ResolvedHunk {
        buffer_range: hunk.buffer_range.clone(),
        buffer_lines,
        base_lines,
        kind: hunk.kind,
        staging: hunk.staging,
        base_byte_start: hunk.diff_base_byte_range.start,
        buffer_word_diffs: hunk.buffer_word_diffs.clone(),
        base_word_diffs: hunk.base_word_diffs.clone(),
    }
}

/// 字节偏移所在行；偏移等于文本末尾（或多字节边界之外）时取 line_count。
fn line_at_or_end(text: &Snapshot, offset: ByteOffset) -> usize {
    text.byte_to_line(offset)
        .map_or_else(|_| text.line_count(), |line| line.get())
}

fn line_ranges_intersect(left: &Range<usize>, right: &Range<usize>) -> bool {
    left.start <= right.end && right.start <= left.end
}

fn excerpt_slice_outside_range(
    entry: &Excerpt,
    hunks: &[DiffTransformHunkInfo],
    source: &crate::ExcerptSource,
    working_id: gpui::EntityId,
    working_text: &Snapshot,
    boundary_line: usize,
    prefix: bool,
) -> Option<ExcerptRange> {
    if entry.source_id != Some(working_id) || entry.diff_kind == Some(ExcerptDiffKind::Deleted) {
        return None;
    }

    let source_start = entry.source_range.start.resolve_in(working_text).ok()?;
    let source_end = entry.source_range.end.resolve_in(working_text).ok()?;
    let source_start_line = line_at_or_end(working_text, source_start);
    let source_end_line = line_at_or_end(working_text, source_end);
    let boundary_line = boundary_line.min(working_text.line_count());
    let lines = if prefix {
        source_start_line..source_end_line.min(boundary_line)
    } else {
        source_start_line.max(boundary_line)..source_end_line
    };
    if lines.is_empty() {
        return None;
    }

    let start = working_text.line_start_byte(Line::new(lines.start)).ok()?;
    let end = if lines.end == working_text.line_count() {
        working_text.len_bytes()
    } else {
        working_text.line_start_byte(Line::new(lines.end)).ok()?
    };
    let source_range = TextRange::new(start, end).ok()?;
    let match_ranges = entry
        .match_ranges
        .iter()
        .filter_map(|range| {
            let start = range.start().max(source_range.start());
            let end = range.end().min(source_range.end());
            (start < end).then(|| TextRange::new(start, end).expect("裁剪后的匹配范围有效"))
        })
        .collect();
    let mut excerpt = ExcerptRange::new(source.entity.clone(), source_range, match_ranges)
        .with_display_path(entry.display_path.as_path().to_path_buf())
        .with_buffer_id(entry.buffer_id)
        .with_editable(entry.editable)
        .with_starts_logical_excerpt(prefix && entry.starts_logical_excerpt);
    if let Some(diff_kind) = entry.diff_kind {
        excerpt = excerpt.with_diff_kind(diff_kind);
    }

    let boundary_offset = if boundary_line == working_text.line_count() {
        working_text.len_bytes()
    } else {
        working_text
            .line_start_byte(Line::new(boundary_line))
            .ok()?
    };
    for hunk in hunks.iter().filter(|hunk| {
        hunk.working == working_id
            && hunk.hunk_start.is_some_and(|anchor| {
                anchor.resolve_in(working_text).is_ok_and(|offset| {
                    if prefix {
                        offset < boundary_offset
                    } else {
                        offset >= boundary_offset
                    }
                })
            })
    }) {
        excerpt = excerpt.with_diff_hunk(hunk.clone());
    }
    Some(excerpt)
}

impl DiffExpansionState {
    fn is_expanded(
        &self,
        kind: DiffHunkKind,
        hunk_start: &Anchor,
        working: &Snapshot,
        expanded_by_default: bool,
    ) -> bool {
        self.override_for(kind, hunk_start, working)
            .map_or(expanded_by_default, |over| over.expanded)
    }

    /// 切换展开/折叠；结果作为显式覆盖记录，后续刷新按工作区 Anchor 迁移。
    fn toggle(
        &mut self,
        kind: DiffHunkKind,
        hunk_start: &Anchor,
        working: &Snapshot,
        expanded_by_default: bool,
    ) {
        let expanded = !self.is_expanded(kind, hunk_start, working, expanded_by_default);
        match self
            .overrides
            .iter_mut()
            .find(|over| over.kind == kind && anchor_matches(&over.hunk_start, hunk_start, working))
        {
            Some(over) => over.expanded = expanded,
            None => self.overrides.push(HunkExpansionOverride {
                kind,
                hunk_start: *hunk_start,
                expanded,
            }),
        }
    }

    fn override_for(
        &self,
        kind: DiffHunkKind,
        hunk_start: &Anchor,
        working: &Snapshot,
    ) -> Option<&HunkExpansionOverride> {
        self.overrides
            .iter()
            .find(|over| over.kind == kind && anchor_matches(&over.hunk_start, hunk_start, working))
    }

    /// 只保留仍能对应到当前 hunk 的显式覆盖。
    fn retain_for_current_hunks(&mut self, hunks: &[ResolvedHunk], working: &Snapshot) {
        self.overrides.retain(|over| {
            hunks.iter().any(|hunk| {
                hunk.kind == over.kind
                    && anchor_matches(&over.hunk_start, &hunk.buffer_range.start, working)
            })
        });
    }
}

/// 两个 hunk 起点 Anchor 是否指向同一工作区位置。
///
/// 两个 Anchor 可能来自不同的 working 版本，必须在同一份工作区快照上重新解析后比较；
/// 直接比较版本与偏移会在 working 推进后让同一 hunk 失去身份。任一端无法解析都判为不匹配。
fn anchor_matches(a: &Anchor, b: &Anchor, working: &Snapshot) -> bool {
    match (a.resolve_in(working), b.resolve_in(working)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// 按工作区锚点把显示层展开/折叠状态迁移到新的 diff 结果。
///
/// 显式覆盖只在对应到同一 hunk 时随位置迁移；hunk 身份变化时回落到展开策略默认值。
fn migrate_expansion_state(
    old_expansion: &DiffExpansionState,
    new: &[ResolvedHunk],
    working: &Snapshot,
    expansion: &mut DiffExpansionState,
) {
    for new_hunk in new {
        let Some(over) =
            old_expansion.override_for(new_hunk.kind, &new_hunk.buffer_range.start, working)
        else {
            continue;
        };
        expansion.overrides.push(HunkExpansionOverride {
            kind: new_hunk.kind,
            hunk_start: new_hunk.buffer_range.start,
            expanded: over.expanded,
        });
    }
}

/// 把单个文件的可见行物化为 excerpts，并在 excerpt 上标注 hunk 身份。
///
/// hunk 身份随 excerpt 进入输入树、再派生到输出变换树；输出行范围因此无需在物化期存储，
/// 也无需在源编辑后手工推进第二份源坐标。
fn materialize_file(
    file: &DiffState,
    cx: &App,
    expanded_by_default: bool,
    excerpts: &mut Vec<ExcerptRange>,
) {
    let resolved = resolve_file_hunks(file, cx);
    for range in file.excerpt_ranges.iter().cloned() {
        materialize_file_in_range(
            file,
            &resolved,
            cx,
            expanded_by_default,
            range,
            true,
            excerpts,
        );
    }
}

/// 只物化受影响的 working 行范围；hunk 快照仍由 BufferDiff 唯一持有。
fn materialize_file_in_range(
    file: &DiffState,
    resolved: &[ResolvedHunk],
    cx: &App,
    expanded_by_default: bool,
    lines: Range<usize>,
    starts_logical_excerpt: bool,
    excerpts: &mut Vec<ExcerptRange>,
) {
    let working = file.diff.read(cx).working().clone();
    let base_source = file.diff.read(cx).base_source().cloned();
    let is_created = file.diff.read(cx).is_created();
    let working_text = working.read(cx).text_snapshot();
    let line_count = working_text.line_count();
    let display_path = file.display_path.clone();
    let working_id = working.entity_id();
    let working_buffer_id = working.read(cx).buffer_id();
    let expansion = &file.expansion;
    let mut materializer = ExcerptMaterializer {
        excerpts,
        display_path: display_path.as_path(),
        buffer_id: working_buffer_id,
    };

    // 无文本差异时仍物化调用方明确提供的文档范围；普通编辑器传入其完整 source excerpt，
    // Git diff 视图在无可见 hunk 时传入空范围。
    if resolved.is_empty() && !is_created {
        materializer.push(
            lines,
            &working_text,
            &working,
            ExcerptShape {
                diff_kind: None,
                starts_logical_excerpt,
                allow_empty: true,
            },
            Vec::new(),
        );
        return;
    }
    // Git diff 视图中的整文件新增没有旧侧 hunk，整份新增内容本身就是差异窗口。
    if is_created && resolved.is_empty() {
        materializer.push(
            lines,
            &working_text,
            &working,
            ExcerptShape {
                diff_kind: Some(ExcerptDiffKind::Added),
                starts_logical_excerpt,
                allow_empty: false,
            },
            vec![DiffTransformHunkInfo {
                working: working_id,
                hunk_start: None,
                side: DiffTransformHunkSide::Content,
                kind: DiffHunkKind::Added,
                staging: DiffHunkStaging::NoStaging,
                base_lines: 0..0,
                base_byte_start: 0,
                buffer_word_diffs: Vec::new(),
                base_word_diffs: Vec::new(),
                expanded: true,
            }],
        );
        return;
    }
    if resolved.is_empty() {
        return;
    }

    {
        let mut current = lines.start;
        // 每个可见窗口只由首个物理片段开启一个逻辑 excerpt；窗口内的旧侧/新侧/上下文片段都不再另起边界。
        // 是否绘制实体 header 由 MultiBufferSnapshot::show_headers 决定，不由物化决定。
        let mut starts_logical_excerpt = starts_logical_excerpt;
        // 无旧侧物化的纯删除需要一个相邻内容节点承载边界；挂到后继内容起点，无后继时挂到前驱终点。
        let mut pending_boundary: Option<DiffTransformHunkInfo> = None;
        for hunk in resolved
            .iter()
            .filter(|hunk| hunk_is_inside_excerpt(hunk, &lines, line_count))
        {
            if current < hunk.buffer_lines.start {
                let boundary = pending_boundary.take().into_iter().collect();
                materializer.push(
                    current..hunk.buffer_lines.start,
                    &working_text,
                    &working,
                    ExcerptShape {
                        diff_kind: None,
                        starts_logical_excerpt,
                        allow_empty: false,
                    },
                    boundary,
                );
                starts_logical_excerpt = false;
            }
            let expanded = expansion.is_expanded(
                hunk.kind,
                &hunk.buffer_range.start,
                &working_text,
                expanded_by_default,
            );
            // 旧侧只在展开时物化完整旧行；折叠的纯删除挂到相邻新侧变换边界。
            let mut old_materialized = false;
            if !hunk.base_lines.is_empty()
                && expanded
                && let Some(base) = base_source.as_ref()
            {
                let base_text = base.read(cx).text_snapshot();
                // 边界标记的是 working 侧位置：旧侧是 base 坐标，不承载待挂载边界。
                let hunks = vec![hunk_info(
                    working_id,
                    DiffTransformHunkSide::Old,
                    hunk,
                    expanded,
                )];
                materializer.push(
                    hunk.base_lines.clone(),
                    &base_text,
                    base,
                    ExcerptShape {
                        diff_kind: Some(ExcerptDiffKind::Deleted),
                        starts_logical_excerpt,
                        allow_empty: false,
                    },
                    hunks,
                );
                old_materialized = true;
                starts_logical_excerpt = false;
            }
            // 新侧：可编辑 excerpt；纯删除 hunk 无新侧内容，由旧侧节点或相邻内容节点承载边界。
            if !hunk.buffer_lines.is_empty() {
                let mut hunks = pending_boundary.take().into_iter().collect::<Vec<_>>();
                hunks.push(hunk_info(
                    working_id,
                    DiffTransformHunkSide::Content,
                    hunk,
                    expanded,
                ));
                materializer.push(
                    hunk.buffer_lines.clone(),
                    &working_text,
                    &working,
                    ExcerptShape {
                        diff_kind: Some(ExcerptDiffKind::Added),
                        starts_logical_excerpt,
                        allow_empty: false,
                    },
                    hunks,
                );
                starts_logical_excerpt = false;
            } else if !old_materialized {
                pending_boundary = Some(hunk_info(
                    working_id,
                    DiffTransformHunkSide::BoundaryStart,
                    hunk,
                    expanded,
                ));
            }
            current = hunk.buffer_lines.end;
        }
        if current < lines.end {
            let leftover = materializer.push(
                current..lines.end,
                &working_text,
                &working,
                ExcerptShape {
                    diff_kind: None,
                    starts_logical_excerpt,
                    allow_empty: false,
                },
                pending_boundary.take().into_iter().collect(),
            );
            // 片段被空行策略跳过时恢复边界，交给收尾逻辑挂到零长度节点。
            pending_boundary = leftover.into_iter().next();
        }
        if let Some(mut info) = pending_boundary.take() {
            // 文档末尾（或本投影范围末尾）的纯删除没有后继内容：挂到前驱内容节点的终点。
            info.side = DiffTransformHunkSide::BoundaryEnd;
            if materializer.excerpts.is_empty() {
                // 整份工作区为空且没有任何内容节点：保留一个零长度工作区节点承载边界，
                // 与无差异空文件的占位行语义一致；它不产生输出行，只为 hunk 提供节点。
                materializer.push(
                    lines.start..lines.end.max(lines.start + 1).min(line_count),
                    &working_text,
                    &working,
                    ExcerptShape {
                        diff_kind: None,
                        starts_logical_excerpt: true,
                        allow_empty: true,
                    },
                    Vec::new(),
                );
            }
            if let Some(last) = materializer.excerpts.last_mut() {
                last.diff_hunks.push(info);
            }
        }
    }
}

/// 构造一个投影片段（空行策略由 shape.allow_empty 控制：占位行允许空源范围）。
fn projected_excerpt(
    source: &Entity<LanguageBuffer>,
    text: &Snapshot,
    lines: Range<usize>,
    display_path: &Path,
    buffer_id: BufferId,
    shape: ExcerptShape,
) -> Option<ExcerptRange> {
    if lines.is_empty() && !shape.allow_empty {
        return None;
    }
    let mut excerpt = ExcerptRange::line_range_from_text(source.clone(), text, lines);
    // 空源范围的普通片段没有可显示内容：跳过（deleted 文件的占位上下文等）。
    // 普通文档构造方允许空范围占位；diff 片段（旧侧/新增）始终物化。
    if excerpt.source_range().is_empty() && !shape.allow_empty && shape.diff_kind.is_none() {
        return None;
    }
    excerpt = excerpt
        .with_display_path(display_path.to_path_buf())
        .with_buffer_id(buffer_id)
        .with_starts_logical_excerpt(shape.starts_logical_excerpt)
        .with_editable(shape.diff_kind != Some(ExcerptDiffKind::Deleted));
    if let Some(diff_kind) = shape.diff_kind {
        excerpt = excerpt.with_diff_kind(diff_kind);
    }
    Some(excerpt)
}

fn hunk_is_inside_excerpt(hunk: &ResolvedHunk, excerpt: &Range<usize>, line_count: usize) -> bool {
    if hunk.buffer_lines.is_empty() {
        excerpt.start <= hunk.buffer_lines.start
            && (hunk.buffer_lines.start < excerpt.end
                || (hunk.buffer_lines.start == line_count && excerpt.end == line_count))
    } else {
        hunk.buffer_lines.start >= excerpt.start && hunk.buffer_lines.end <= excerpt.end
    }
}

/// 把列（Unicode scalar 计数）钳制到文本中指定行的有效长度（行 0-based）。
fn clamp_column_to_line(text: &Snapshot, line: usize, column: usize) -> usize {
    let line = line.min(text.line_count().saturating_sub(1));
    let line_chars = text
        .line_content(Line::new(line), None)
        .map_or(0, |content| content.len_chars());
    column.min(line_chars)
}
