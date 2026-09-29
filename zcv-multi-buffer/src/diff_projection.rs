//! MultiBuffer 的 git diff 投影：把版本化的 BufferDiff 结果投影为输出变换与显示坐标。
//!
//! 普通编辑器与多文件投影（Git 差异视图）共用同一套物化：
//! 宿主注入同一工作区源快照对应的 BufferDiff 和已装配的可见 working 行范围；
//! 本层只消费其 BufferDiffSnapshot，按展开状态把旧侧行插入只读删除变换，并派生组合坐标显示 hunks。
//!
//! diff 状态（base/working、版本、hunk、pending、操作）全部归 BufferDiff 所有；
//! hunk 身份随输出变换节点承载，输出坐标由游标推导；
//! 展开/折叠与显示路径归本层所有，不进入 diff 快照。

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, Context, Entity, Subscription};
use sum_tree::{Bias, SumTree};
use zcv_text::{Anchor, BufferId, ByteOffset, Line, Snapshot, TextChangeBatch, TextRange};

use crate::{
    DeletedHunkRegion, DiffTransform, DiffTransformHunkInfo, DiffTransformHunkSide, Excerpt,
    ExcerptContext, ExcerptDiffKind, ExcerptRange, ExcerptSummary, MBTextSummary, MultiBuffer,
    MultiBufferCursor, MultiBufferEvent, MultiBufferSnapshot, OutputRegion, PathKey,
    SourceIncremental, SourceTexts, diff_output_text, snapshot_range_summary,
};
use zcv_buffer_diff::{
    BufferDiff, BufferDiffEvent, DiffHunk, DiffHunkKind, DiffHunkStaging, diff_line_boundary,
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
    /// 由文档视图装配的 working 范围；整文件范围会随源快照增长而保持整文件语义。
    pub excerpt_ranges: DiffExcerptRanges,
}

/// diff 文档的 working 可见范围。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffExcerptRanges {
    /// 普通编辑器显示完整工作区源文件。
    FullFile,
    /// 组合 Git diff 视图装配的可见行窗口。
    Windows(Vec<Range<usize>>),
}

impl DiffExcerptRanges {
    fn iter(&self, line_count: usize) -> impl Iterator<Item = Range<usize>> + '_ {
        let windows = match self {
            Self::FullFile => &[][..],
            Self::Windows(ranges) => ranges.as_slice(),
        };
        matches!(self, Self::FullFile)
            .then_some(0..line_count)
            .into_iter()
            .chain(windows.iter().cloned())
    }
}

/// 随可见 hunk 传递的源身份与操作范围，不依赖组合文档中的位置或序号。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffHunkSource {
    /// 工作区 Buffer 身份；旧侧和新侧片段共用此身份。
    pub buffer_id: BufferId,
    /// 源 diff 快照提供的 Anchor 范围；整文件新增块显式以整文件为操作目标。
    /// 消费方在执行操作时于当前工作区快照解析，不把显示几何当作操作坐标。
    pub range: Option<Range<Anchor>>,
}

/// 一个文件的显示状态：diff 配置、展开覆盖、订阅与物化版本。
///
/// diff 结果由 BufferDiff 持有；本结构只持有显示层状态，随文件在 MultiBuffer 中增删而创建销毁。
pub(crate) struct DiffState {
    pub(super) diff: Entity<BufferDiff>,
    /// 组合文档中的显示路径（文件标题与导航定位）。
    display_path: PathKey,
    /// 调用方装配的 working 范围；MultiBuffer 不决定 diff 视图的裁剪策略。
    excerpt_ranges: DiffExcerptRanges,
    /// 显示层拥有的展开/折叠状态，与版本化 diff 结果分离。
    expansion: DiffExpansionState,
    /// BufferDiff 订阅；只作为守卫随 DiffState 生命周期创建销毁，不直接读取。
    _subscription: Subscription,
    /// 上次物化时该文件的 diff 版本；None 表示尚未物化进组合文档。
    revision: Option<u64>,
    /// 替换 diff 实体后，新 BufferDiff 的首次后台结果返回前暂存的展开状态。
    pending_expansion_state: Option<DiffExpansionState>,
}

impl DiffState {
    fn new(
        diff: Entity<BufferDiff>,
        display_path: PathKey,
        excerpt_ranges: DiffExcerptRanges,
        cx: &mut Context<MultiBuffer>,
    ) -> Self {
        let subscription = cx.subscribe(&diff, |this, diff, event, cx| {
            let BufferDiffEvent::DiffChanged { changed_range } = event;
            this.diff_changed(diff.entity_id(), changed_range.clone(), cx);
        });
        Self {
            diff,
            display_path,
            excerpt_ranges,
            expansion: DiffExpansionState::default(),
            _subscription: subscription,
            revision: None,
            pending_expansion_state: None,
        }
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
    pub source: DiffHunkSource,
    pub old_range: Option<Range<usize>>,
    pub expanded: bool,
    pub word_diffs: WordDiffs,
}

/// diff 投影的 hunk 身份索引。
///
/// 坐标和装饰数据直接从不可变组合快照的输出变换树按需读取，不在这里复制第二份几何状态。
#[derive(Clone, Debug)]
pub struct DiffDisplaySnapshot {
    /// hunk 身份或显示状态变化时递增；纯坐标变化复用当前索引版本。
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

    fn with_version(&self, version: u64) -> Self {
        Self {
            version,
            segments: Arc::clone(&self.segments),
            hunk_indices: Arc::clone(&self.hunk_indices),
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
    /// 当前组合映射内与逻辑行范围相交的 hunk；词级差异也裁剪到该范围。
    pub fn diff_hunks_in_lines(&self, lines: Range<usize>) -> Vec<(usize, ResolvedDiffHunk)> {
        let Some(diff_display) = self.diff_display.as_deref() else {
            return Vec::new();
        };
        let word_diff_range = self.byte_range_for_lines(lines.clone());
        resolved_diff_hunks_in_lines(
            &self.excerpts,
            &self.diff_transforms,
            &self.excerpt_sources,
            diff_display,
            lines,
            word_diff_range,
        )
    }

    fn byte_range_for_lines(&self, lines: Range<usize>) -> Range<usize> {
        let line_count = self.line_count();
        let byte_at = |line: usize| {
            if line >= line_count {
                self.len_bytes().get()
            } else {
                self.line_start_byte(Line::new(line))
                    .map_or_else(|_| self.len_bytes().get(), |offset| offset.get())
            }
        };
        byte_at(lines.start)..byte_at(lines.end)
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
    word_diff_range: Range<usize>,
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
        let item_end = item_start + transform.transform_summary().output.lines.max(1);
        if item_end > lines.start {
            for info in transform.hunks() {
                candidates
                    .entry((info.working, info.hunk_start()))
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
            let mut cursor = MultiBufferCursor::new(excerpts, transforms);
            cursor.seek_transform_index(item_index);
            loop {
                let mut previous = cursor.clone();
                previous.prev();
                if previous.item().is_none_or(|(_, transform)| {
                    !transform
                        .hunks()
                        .iter()
                        .any(|info| (info.working, info.hunk_start()) == key)
                }) {
                    break;
                }
                cursor = previous;
            }
            while let Some((excerpt, transform)) = cursor.item() {
                if !transform
                    .hunks()
                    .iter()
                    .any(|info| (info.working, info.hunk_start()) == key)
                {
                    break;
                }
                let at = cursor.start();
                if !inspected.insert((at.index, at.input_item_index)) {
                    cursor.next();
                    continue;
                }
                for info in transform
                    .hunks()
                    .iter()
                    .filter(|info| (info.working, info.hunk_start()) == key)
                {
                    let accum =
                        accum.get_or_insert_with(|| HunkAccum::new(info, excerpt.buffer_id));
                    accum.expanded = info.expanded;
                    let at = cursor.start();
                    let content_lines = excerpt.text_summary.lines
                        + usize::from(
                            excerpt.adds_newline && info.side == DiffTransformHunkSide::Old,
                        );
                    let content_range = at.lines..(at.lines + content_lines).max(at.lines + 1);
                    match info.side {
                        DiffTransformHunkSide::Content => {
                            accum.content_range = Some(match accum.content_range.take() {
                                Some(previous) => {
                                    previous.start.min(content_range.start)
                                        ..previous.end.max(content_range.end)
                                }
                                None => content_range,
                            });
                            let source_text = sources
                                .source_text(excerpt.source_index)
                                .expect("diff excerpt 必须引用当前源快照");
                            let output_start = at.bytes;
                            let source_start = excerpt.source_range.start().get();
                            if let Some(visible) =
                                visible_source_bytes(&excerpt, output_start, &word_diff_range)
                            {
                                // 词级范围由 BufferDiff 按源文档顺序生成；源编辑保持锚点顺序。
                                let first = info.buffer_word_diffs.partition_point(|diff| {
                                    diff.end
                                        .resolve_in(source_text)
                                        .expect("词级 diff 锚点必须属于当前工作区版本链")
                                        .get()
                                        <= visible.start
                                });
                                for diff in &info.buffer_word_diffs[first..] {
                                    let start = diff
                                        .start
                                        .resolve_in(source_text)
                                        .expect("词级 diff 锚点必须属于当前工作区版本链")
                                        .get();
                                    if start >= visible.end {
                                        break;
                                    }
                                    let end = diff
                                        .end
                                        .resolve_in(source_text)
                                        .expect("词级 diff 锚点必须属于当前工作区版本链")
                                        .get();
                                    let start = start.max(source_start);
                                    let end = end.min(excerpt.source_range.end().get());
                                    if start < end {
                                        accum.new_word_diffs.push((
                                            DiffHunkKind::Added,
                                            (output_start + start - source_start)
                                                ..(output_start + end - source_start),
                                        ));
                                    }
                                }
                            }
                        }
                        DiffTransformHunkSide::Old => {
                            accum.old_range = Some(match accum.old_range.take() {
                                Some(previous) => {
                                    previous.start.min(content_range.start)
                                        ..previous.end.max(content_range.end)
                                }
                                None => content_range,
                            });
                            let output_start = at.bytes;
                            if info.expanded
                                && let Some(visible) =
                                    visible_source_bytes(&excerpt, output_start, &word_diff_range)
                            {
                                let source_start = excerpt.source_range.start().get();
                                let first = info.base_word_diffs.partition_point(|diff| {
                                    info.base_byte_start + diff.end <= visible.start
                                });
                                for diff in &info.base_word_diffs[first..] {
                                    let start = info.base_byte_start + diff.start;
                                    if start >= visible.end {
                                        break;
                                    }
                                    let start = start.max(source_start);
                                    let end = (info.base_byte_start + diff.end)
                                        .min(excerpt.source_range.end().get());
                                    if start < end {
                                        accum.old_word_diffs.push((
                                            DiffHunkKind::Deleted,
                                            (output_start + start - source_start)
                                                ..(output_start + end - source_start),
                                        ));
                                    }
                                }
                            }
                        }
                        DiffTransformHunkSide::BoundaryStart => {
                            accum.boundary_start = Some(at.lines);
                        }
                    }
                }
                cursor.next();
            }
        }

        let Some(accum) = accum else {
            continue;
        };
        let Some(range) = accum
            .content_range
            .or_else(|| accum.old_range.as_ref().map(|range| range.end..range.end))
            .or_else(|| accum.boundary_start.map(|line| line..line))
        else {
            continue;
        };
        let mut word_diffs = accum.old_word_diffs;
        word_diffs.extend(accum.new_word_diffs);
        resolved.push((
            index,
            ResolvedDiffHunk {
                source: accum.source,
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

/// 先将视口与当前投影片段相交，再换算为该片段的源字节范围。
fn visible_source_bytes(
    excerpt: &OutputRegion,
    output_start: usize,
    visible_output: &Range<usize>,
) -> Option<Range<usize>> {
    let start = output_start.max(visible_output.start);
    let end = (output_start + excerpt.source_range.len()).min(visible_output.end);
    (start < end).then(|| {
        let source_start = excerpt.source_range.start().get();
        (source_start + start - output_start)..(source_start + end - output_start)
    })
}

/// 一个文件内用户显式切换过展开状态的 hunk。
///
/// 显式选择按 hunk 起点工作区 Anchor 标识；文件身份由外层 DiffState 持有。
/// 未覆盖的 hunk 采用默认值；变化类型是派生事实，不参与展开状态的身份判断。
#[derive(Default, Clone)]
struct DiffExpansionState {
    overrides: Vec<HunkExpansionOverride>,
}

#[derive(Clone)]
struct HunkExpansionOverride {
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

/// 单次游标遍历中按 hunk 身份聚合的输出范围与词级片段。
struct HunkAccum {
    source: DiffHunkSource,
    kind: DiffHunkKind,
    staging: DiffHunkStaging,
    base_lines: Range<usize>,
    expanded: bool,
    content_range: Option<Range<usize>>,
    old_range: Option<Range<usize>>,
    boundary_start: Option<usize>,
    old_word_diffs: WordDiffs,
    new_word_diffs: WordDiffs,
}

impl HunkAccum {
    fn new(info: &DiffTransformHunkInfo, buffer_id: BufferId) -> Self {
        Self {
            source: DiffHunkSource {
                buffer_id,
                range: if info.is_created {
                    None
                } else {
                    info.buffer_range.clone()
                },
            },
            kind: info.kind,
            staging: info.staging,
            base_lines: info.base_lines.clone(),
            expanded: info.expanded,
            content_range: None,
            old_range: None,
            boundary_start: None,
            old_word_diffs: Vec::new(),
            new_word_diffs: Vec::new(),
        }
    }
}

/// 一个 hunk 节点携带的身份与显示元数据。
fn hunk_info(
    working: gpui::EntityId,
    side: DiffTransformHunkSide,
    hunk: &ResolvedHunk,
    expanded: bool,
    is_created: bool,
) -> DiffTransformHunkInfo {
    DiffTransformHunkInfo {
        working,
        buffer_range: Some(hunk.buffer_range.clone()),
        is_created,
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
    /// 新路径按路径顺序插入；
    /// 同路径的 diff 变化只替换该文件的 excerpts 并迁移展开状态，不重建整份组合文档。
    /// excerpts 按调用方给出的可见范围同步物化，后台 diff 结果只补充输出变换（旧侧行与词级范围），不决定该文件能否进入投影。
    /// 返回 true 表示这次调用新增或替换了该路径的登记；完全相同的登记返回 false。
    pub fn add_diff(&mut self, file: DiffFile, cx: &mut Context<Self>) -> bool {
        if self.diff.is_none() {
            // 空投影下等价于首次建立：先初始化显示快照并登记该文件。
            self.set_diff_files(vec![file], cx);
            return true;
        }
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
            self.replace_diff_file(index, file, cx);
            return true;
        }
        let insert_at = self.diffs.partition_point(|current| {
            current.display_path.as_path() < file.display_path.as_path()
        });
        self.insert_diff_file(insert_at, file, cx);
        true
    }

    /// 更新 Git diff 视图装配的 excerpt 范围，并按 BufferDiff 事件范围同步投影。
    pub fn update_diff_excerpt_ranges(
        &mut self,
        display_path: &Path,
        excerpt_ranges: DiffExcerptRanges,
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
        if ranges_changed {
            self.begin_projection_sync();
            self.sync_pending_sources(cx);
            self.replace_materialized_file(index, cx);
            self.finish_projection_sync(cx);
            return true;
        }
        let diff = self.diffs[index].diff.clone();
        if self.diffs[index].revision == Some(diff.read(cx).revision()) {
            if !ranges_changed {
                return false;
            }
            self.begin_projection_sync();
            self.sync_pending_sources(cx);
            let updated = self.sync_diff_range(index, &changed_range, cx);
            self.finish_projection_sync(cx);
            return updated;
        }
        self.diff_changed(diff.entity_id(), Some(changed_range), cx);
        true
    }

    /// 同路径的 diff 实体或视图范围变化：只替换该文件的 excerpts，不重建整份组合文档。
    ///
    /// 展开状态按旧/新 hunk 迁移；新 diff 尚未算完时先登记 pending 迁移，
    /// 但 excerpts 仍按本次调用给出的可见范围同步替换，等 DiffChanged 到期后再补充输出变换。
    fn replace_diff_file(&mut self, index: usize, file: DiffFile, cx: &mut Context<Self>) {
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
                let working_text = working_snapshot_for(&next, cx);
                let new_hunks = next.diff.read(cx).snapshot().hunks().collect::<Vec<_>>();
                migrate_expansion_state(
                    &self.diffs[index].expansion,
                    &new_hunks,
                    &working_text,
                    &mut next.expansion,
                );
            } else {
                next.pending_expansion_state = Some(self.diffs[index].expansion.clone());
            }
        }
        self.diffs[index] = next;
        self.replace_materialized_file(index, cx);
    }

    /// 在 insert_at 处登记一个 diff 文件，并按其可见范围同步建立该路径的 excerpts。
    ///
    /// 后台结果就绪前 excerpts 只包含可见工作区文本（没有旧侧行）；
    /// 结果到达后由 diff_changed 复用同一路径替换，不需要第二次结构登记。
    fn insert_diff_file(&mut self, insert_at: usize, file: DiffFile, cx: &mut Context<Self>) {
        let state = DiffState::new(
            file.diff,
            PathKey::new(file.display_path),
            file.excerpt_ranges,
            cx,
        );
        self.diffs.insert(insert_at, state);
        self.replace_materialized_file(insert_at, cx);
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
            let old_version = self.state.projection_version;
            for file in inputs.into_iter().skip(old_len) {
                self.insert_diff_file(self.diffs.len(), file, cx);
            }
            return self.state.projection_version != old_version;
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
                    let working_text = working_snapshot_for(file, cx);
                    let new_hunks = file.diff.read(cx).snapshot().hunks().collect::<Vec<_>>();
                    migrate_expansion_state(
                        &old_file.expansion,
                        &new_hunks,
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
            let paths = self
                .diffs
                .iter()
                .map(|file| {
                    let working = file.diff.read(cx).working().read(cx);
                    crate::path_key_for_source(working)
                })
                .collect::<HashSet<_>>();
            let removed_paths = self
                .diff_display_paths()
                .into_iter()
                .filter(|path| !paths.contains(path))
                .collect::<Vec<_>>();
            for path in removed_paths {
                self.remove_excerpts_for_path(path.as_path(), cx);
            }
            for index in 0..self.diffs.len() {
                if self.diffs[index]
                    .diff
                    .read(cx)
                    .is_current_version_calculated(cx)
                {
                    self.replace_materialized_file(index, cx);
                }
            }
            self.refresh_diff_display(cx);
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
        self.rebuild_diff_transforms_for_expansion(cx);
        cx.emit(MultiBufferEvent::DiffExpansionChanged);
    }

    /// 按显示 hunk 索引切换展开/折叠（渲染层点击入口）。
    ///
    /// 折叠/展开不改变源，编辑器源锚点选区自然存活，无需返回投影重映射。
    pub fn toggle_diff_hunk_at(&mut self, display_index: usize, cx: &mut Context<Self>) {
        let expanded_by_default = self.diff_expanded_by_default;
        let Some((working, hunk_start)) = self.diff.as_ref().and_then(|diff| {
            let source = diff.source_at(display_index)?;
            Some((source.working, source.hunk_start?))
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
        self.diffs[file_index]
            .expansion
            .toggle(&hunk_start, &working_text, expanded_by_default);
        let range = self.diffs[file_index]
            .diff
            .read(cx)
            .snapshot()
            .hunks()
            .find(|hunk| anchor_matches(&hunk.buffer_range.start, &hunk_start, &working_text))
            .expect("显示 hunk 必须属于当前 diff")
            .buffer_range
            .clone();
        self.sync_diff_range(file_index, &range, cx);
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
        self.rebuild_diff_transforms_for_expansion(cx);
        cx.emit(MultiBufferEvent::DiffExpansionChanged);
    }

    /// 展开策略切换只重建输出变换，不改变源窗口、Anchor 或源订阅。
    fn rebuild_diff_transforms_for_expansion(&mut self, cx: &mut Context<Self>) {
        self.assert_no_active_transaction("MultiBuffer::rebuild_diff_transforms_for_expansion");
        let before = self.projection_trees();
        let old_version = self.state.projection_version;
        self.prepare_diff_sources(cx);
        self.rebuild_all_diff_transforms(cx);
        self.publish_projection_edit(&before, old_version);
        self.refresh_diff_display_after_hunk_update(cx);
    }

    /// 显示坐标 hunks（组合坐标，跨文件展平）。
    ///
    /// 从当前变换树全量派生，仅供低频调用；帧路径使用快照的可见范围查询。
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
            .lines
            .saturating_add(1);
        resolved_diff_hunks_in_lines(
            &self.state.excerpts,
            &self.state.diff_transforms,
            self.state.sources.as_slice(),
            diff_display,
            0..end,
            0..self.state.diff_transforms.summary().output.len,
        )
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
                .regions_for_path(location.path.as_path())
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

    /// BufferDiff 事件入口：只有当前物化结果落后于 diff 版本时才重建。
    fn diff_changed(
        &mut self,
        diff_id: gpui::EntityId,
        changed_range: Option<Range<Anchor>>,
        cx: &mut Context<Self>,
    ) {
        // 对齐 Zed 的 buffer_diff_changed：先把待同步的源快照纳入当前帧，
        // 再更新 diff transform，最后只发布一次组合投影版本。
        self.begin_projection_sync();
        self.sync_pending_sources(cx);
        self.diff_changed_inner(diff_id, changed_range, cx);
        self.finish_projection_sync(cx);
    }

    fn diff_changed_inner(
        &mut self,
        diff_id: gpui::EntityId,
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

        if self.diffs[index].pending_expansion_state.is_some() {
            let old_state = self.diffs[index]
                .pending_expansion_state
                .take()
                .expect("已检查 pending expansion state 存在");
            let working_text = working_snapshot_for(&self.diffs[index], cx);
            let new_hunks = self.diffs[index]
                .diff
                .read(cx)
                .snapshot()
                .hunks()
                .collect::<Vec<_>>();
            migrate_expansion_state(
                &old_state,
                &new_hunks,
                &working_text,
                &mut self.diffs[index].expansion,
            );
        }

        if self.diffs[index].revision.is_none() {
            self.replace_materialized_file(index, cx);
            return;
        }

        if let Some(changed_range) = changed_range {
            let working = working_snapshot_for(&self.diffs[index], cx);
            let start = changed_range
                .start
                .resolve_in(&working)
                .expect("diff 变化起点必须属于工作区版本链");
            let end = changed_range
                .end
                .resolve_in(&working)
                .expect("diff 变化终点必须属于工作区版本链");
            let current_starts = diff
                .read(cx)
                .snapshot()
                .hunks_intersecting_working_range(start..end, &working)
                .map(|hunk| hunk.buffer_range.start)
                .collect::<Vec<_>>();
            self.diffs[index].expansion.overrides.retain(|over| {
                let offset = over
                    .hunk_start
                    .resolve_in(&working)
                    .expect("展开状态必须属于工作区版本链");
                offset < start
                    || offset > end
                    || current_starts
                        .iter()
                        .any(|current| anchor_matches(&over.hunk_start, current, &working))
            });
            self.sync_diff_range(index, &changed_range, cx);
        }
        // 缺少范围时只接受 BufferDiff 状态，不同步 diff 投影，也不触发整文件重建。
        // 记录已处理版本，使源文档同步可以完成。
        self.diffs[index].revision = Some(revision);
    }

    /// 显式替换 diff 实体或可见窗口时，重建该路径的逻辑 excerpts。
    fn replace_materialized_file(&mut self, file_index: usize, cx: &mut Context<Self>) {
        let mut excerpts = Vec::new();
        materialize_file(&self.diffs[file_index], cx, &mut excerpts);
        let working = self.diffs[file_index].diff.read(cx).working().clone();
        let path = crate::path_key_for_source(working.read(cx));
        self.prepare_diff_sources(cx);
        if excerpts.is_empty() {
            self.remove_excerpts_for_path(path.as_path(), cx);
        } else {
            self.set_excerpts_for_path(excerpts, cx);
        }
        // revision 只标记已采用后台 hunk 结果的物化；
        // 调用方给的范围先行建立 excerpts 时保持 None，等 DiffChanged 到期后由 diff_changed 复用同一路径重建输出变换。
        let calculated = self.diffs[file_index]
            .diff
            .read(cx)
            .is_current_version_calculated(cx);
        let revision = self.diffs[file_index].diff.read(cx).revision();
        self.diffs[file_index].revision = calculated.then_some(revision);
        self.refresh_diff_display_after_hunk_update(cx);
    }

    /// hunk 更新只重算相交的输出变换，不修改逻辑窗口。
    fn sync_diff_range(
        &mut self,
        file_index: usize,
        changed_range: &Range<Anchor>,
        cx: &mut Context<Self>,
    ) -> bool {
        self.prepare_diff_sources(cx);
        let working = self.diffs[file_index].diff.read(cx).working().clone();
        let source_id = working.entity_id();
        let text = &self.state.sources[self.state.source_indices[&source_id]].text;
        let start = changed_range
            .start
            .resolve_in(text)
            .expect("diff 增量起点必须属于工作区版本链");
        let end = changed_range
            .end
            .resolve_in(text)
            .expect("diff 增量终点必须属于工作区版本链");
        let path = crate::path_key_for_source(working.read(cx));
        let mut cursor = self.state.excerpts.cursor::<ExcerptSummary>(());
        cursor.seek(&path, Bias::Left);
        let mut edits = Vec::new();
        while let Some(excerpt) = cursor.item() {
            if excerpt.path != path {
                break;
            }
            let from = start.max(excerpt.source_range.start());
            let to = end.min(excerpt.source_range.end());
            if from <= to {
                let offset = cursor.start().text.len;
                let from = offset + from.get() - excerpt.source_range.start().get();
                let mut to = offset + to.get() - excerpt.source_range.start().get();
                if to == offset + excerpt.text_summary.len {
                    to += usize::from(excerpt.adds_newline);
                }
                edits.push(crate::diff_transform_sync::InputEdit::new(
                    from..to,
                    from..to,
                ));
            }
            cursor.next();
        }
        drop(cursor);
        let before = self.projection_trees();
        let old_version = self.state.projection_version;
        let output = self.sync_diff_transforms(&before, edits, None, cx);
        if !output.is_empty() {
            self.publish_projection_change(SourceIncremental {
                batch: TextChangeBatch::from_edits(old_version, old_version, output),
            });
            self.emit_projection_changed(cx);
        }
        self.refresh_diff_display_for_path(&path, cx);
        self.state.projection_version != old_version
    }

    /// 旧侧源由 BufferDiff 唯一拥有；本层只固定用于读取的快照，不订阅其文本编辑。
    pub(super) fn prepare_diff_sources(&mut self, cx: &mut Context<Self>) {
        let bases = self
            .diffs
            .iter()
            .filter_map(|file| file.diff.read(cx).base_source().cloned())
            .collect::<Vec<_>>();
        let new = bases
            .iter()
            .filter(|source| !self.state.source_indices.contains_key(&source.entity_id()))
            .cloned()
            .collect();
        self.register_sources(new, &HashSet::new(), cx);
        for source in bases {
            let index = self.state.source_indices[&source.entity_id()];
            if self.state.sources[index].text.version() != source.read(cx).version() {
                self.install_source_snapshot(source.entity_id(), source.read(cx).snapshot());
            }
        }
    }

    /// 从逻辑 excerpt 的输入子区间派生内容与旧侧变换。
    /// 输入摘要包含稳定分隔符；旧侧摘要的输入恒为零。
    pub(super) fn diff_transforms_for_excerpt(
        &self,
        excerpt: &Excerpt,
        range: Range<usize>,
        cx: &App,
    ) -> Vec<DiffTransform> {
        let source = &self.state.sources[excerpt.source_index];
        let text = &source.text;
        let start = excerpt.source_range.start().get() + range.start.min(excerpt.text_summary.len);
        let end = excerpt.source_range.start().get() + range.end.min(excerpt.text_summary.len);
        let completes = range.end == diff_output_text(excerpt).len;
        let content_summary = |start: usize, end: usize| {
            snapshot_range_summary(
                text,
                TextRange::new(ByteOffset::new(start), ByteOffset::new(end))
                    .expect("变换源范围必须正序"),
            )
            .expect("变换源范围必须属于 excerpt 快照")
            .0
        };
        let Some(file) = self
            .diffs
            .iter()
            .find(|file| file.diff.read(cx).working().entity_id() == source.entity.entity_id())
        else {
            return vec![DiffTransform::buffer_content(
                excerpt,
                content_summary(start, end),
                completes,
                Vec::new(),
            )];
        };
        let diff = file.diff.read(cx);
        let created = diff.is_created();
        let resolved = resolve_file_hunks_in_working_range(
            file,
            ByteOffset::new(start)..ByteOffset::new(end),
            cx,
        );
        let working_id = source.entity.entity_id();
        if created && resolved.is_empty() {
            return vec![DiffTransform::buffer_content(
                excerpt,
                content_summary(start, end),
                completes,
                vec![DiffTransformHunkInfo {
                    working: working_id,
                    buffer_range: None,
                    is_created: true,
                    side: DiffTransformHunkSide::Content,
                    kind: DiffHunkKind::Added,
                    staging: DiffHunkStaging::NoStaging,
                    base_lines: 0..0,
                    base_byte_start: 0,
                    buffer_word_diffs: Vec::new(),
                    base_word_diffs: Vec::new(),
                    expanded: true,
                }],
            )];
        }
        let mut transforms = Vec::new();
        let mut current = start;
        for hunk in &resolved {
            let hunk_start = hunk
                .buffer_range
                .start
                .resolve_in(text)
                .expect("hunk 起点必须属于工作区版本链")
                .get();
            let hunk_end = hunk
                .buffer_range
                .end
                .resolve_in(text)
                .expect("hunk 终点必须属于工作区版本链")
                .get();
            if hunk_start > end || hunk_end < start {
                continue;
            }
            let from = hunk_start.max(start).min(end);
            let to = hunk_end.min(end).max(from);
            if current < from {
                transforms.push(DiffTransform::buffer_content(
                    excerpt,
                    content_summary(current, from),
                    false,
                    Vec::new(),
                ));
            }
            let expanded = file.expansion.is_expanded(
                &hunk.buffer_range.start,
                text,
                self.diff_expanded_by_default,
            );
            let owns_boundary = hunk_start >= start
                && (hunk_start < end || (hunk_start == end && (completes || range.is_empty())));
            let mut old_visible = false;
            if owns_boundary
                && expanded
                && !hunk.base_lines.is_empty()
                && let Some(base) = diff.base_source()
            {
                let base_index = self.state.source_indices[&base.entity_id()];
                let base_text = &self.state.sources[base_index].text;
                let bytes = working_byte_range_for_lines(base_text, hunk.base_lines.clone());
                let base_range = TextRange::new(bytes.start, bytes.end).expect("旧侧范围必须正序");
                let (summary, ends_newline) = snapshot_range_summary(base_text, base_range)
                    .expect("旧侧范围必须属于基线快照");
                let deleted = DeletedHunkRegion {
                    source_index: base_index,
                    source_id: base.entity_id(),
                    source_range: ExcerptContext::new(base_text.version(), base_range, false),
                    source_start_line: base_text
                        .byte_to_line(bytes.start)
                        .expect("旧侧起点必须有效")
                        .get(),
                    text_summary: summary,
                    adds_newline: !ends_newline,
                };
                transforms.push(DiffTransform::deleted_hunk(
                    deleted,
                    vec![hunk_info(
                        working_id,
                        DiffTransformHunkSide::Old,
                        hunk,
                        expanded,
                        created,
                    )],
                ));
                old_visible = true;
            }
            if from < to {
                transforms.push(DiffTransform::buffer_content(
                    excerpt,
                    content_summary(from, to),
                    false,
                    vec![hunk_info(
                        working_id,
                        DiffTransformHunkSide::Content,
                        hunk,
                        expanded,
                        created,
                    )],
                ));
            } else if owns_boundary && !old_visible {
                transforms.push(DiffTransform::buffer_content(
                    excerpt,
                    MBTextSummary::default(),
                    false,
                    vec![hunk_info(
                        working_id,
                        DiffTransformHunkSide::BoundaryStart,
                        hunk,
                        expanded,
                        created,
                    )],
                ));
            }
            current = to;
        }
        if current < end || (completes && excerpt.adds_newline) || transforms.is_empty() {
            transforms.push(DiffTransform::buffer_content(
                excerpt,
                content_summary(current, end),
                completes,
                Vec::new(),
            ));
        }
        transforms
    }

    /// 显式重建 diff 文档的逻辑窗口与输出变换，并派生显示坐标 hunks。
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
        let mut excerpts = Vec::new();
        for file in &self.diffs {
            materialize_file(file, cx, &mut excerpts);
        }
        let expected_excerpt_count = excerpts.len();
        self.replace_all_excerpts(excerpts, cx);
        assert_eq!(
            self.state.excerpts.summary().count,
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
        self.refresh_diff_display_after_hunk_update(cx);
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
                if seen.insert((info.working, info.hunk_start())) {
                    sources.push(DisplayHunkSource {
                        working: info.working,
                        hunk_start: info.hunk_start(),
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

    /// 刷新已变化路径的 diff 显示输入；hunk 元数据变化也推进专属版本。
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
        let version = current.version.wrapping_add(1);
        self.diff = Some(if current.segments[index] == segment {
            Arc::new(current.with_version(version))
        } else {
            let mut segments = current.segments.to_vec();
            segments[index] = segment;
            Arc::new(DiffDisplaySnapshot::from_segments(version, segments))
        });
        self.snapshot_dirty = true;
        self.notify_if_not_syncing(cx);
    }

    /// 在低频 diff 拓扑变化后按当前组合映射重建 hunk 身份索引。
    ///
    /// 普通源编辑复用身份索引，不进入这里。
    pub(crate) fn refresh_diff_display(&mut self, cx: &mut Context<Self>) {
        self.refresh_diff_display_inner(false, cx);
    }

    fn refresh_diff_display_after_hunk_update(&mut self, cx: &mut Context<Self>) {
        self.refresh_diff_display_inner(true, cx);
    }

    fn refresh_diff_display_inner(&mut self, hunk_display_changed: bool, cx: &mut Context<Self>) {
        let Some(current) = self.diff.as_ref() else {
            return;
        };
        let segments = self.derive_diff_display_segments();
        let identities_unchanged = current.segments.as_ref() == segments.as_slice();
        if identities_unchanged && !hunk_display_changed {
            self.snapshot_dirty = true;
            self.notify_if_not_syncing(cx);
            return;
        }
        let version = current.version.wrapping_add(1);
        self.diff = Some(if identities_unchanged {
            Arc::new(current.with_version(version))
        } else {
            Arc::new(DiffDisplaySnapshot::from_segments(version, segments))
        });
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
        let hunks = diff.snapshot().hunks().collect::<Vec<_>>();
        (working_text, base_text, hunks)
    };
    hunks
        .iter()
        .map(|hunk| resolve_hunk(hunk, &working_text, base_text.as_ref()))
        .collect()
}

fn resolve_file_hunks_in_working_range(
    file: &DiffState,
    range: Range<ByteOffset>,
    cx: &App,
) -> Vec<ResolvedHunk> {
    let diff = file.diff.read(cx);
    let working_text = diff.working().read(cx).text_snapshot();
    let base_text = diff.base_source().map(|base| base.read(cx).text_snapshot());
    diff.snapshot()
        .hunks_intersecting_working_range(range, &working_text)
        .map(|hunk| resolve_hunk(hunk, &working_text, base_text.as_ref()))
        .collect()
}

fn working_byte_range_for_lines(working: &Snapshot, lines: Range<usize>) -> Range<ByteOffset> {
    let line_count = working.line_count();
    let line_start = |line| {
        if line >= line_count {
            working.len_bytes()
        } else {
            working
                .line_start_byte(Line::new(line))
                .unwrap_or(working.len_bytes())
        }
    };
    line_start(lines.start)..line_start(lines.end)
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
        .expect("hunk 起点必须属于工作区版本链");
    let buffer_end = hunk
        .buffer_range
        .end
        .resolve_in(working)
        .expect("hunk 终点必须属于工作区版本链");
    let buffer_lines =
        diff_line_boundary(working, buffer_start)..diff_line_boundary(working, buffer_end);
    let base_lines = base.map_or(0..0, |base| {
        diff_line_boundary(base, ByteOffset::new(hunk.diff_base_byte_range.start))
            ..diff_line_boundary(base, ByteOffset::new(hunk.diff_base_byte_range.end))
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

impl DiffExpansionState {
    fn is_expanded(
        &self,
        hunk_start: &Anchor,
        working: &Snapshot,
        expanded_by_default: bool,
    ) -> bool {
        self.override_for(hunk_start, working)
            .map_or(expanded_by_default, |over| over.expanded)
    }

    /// 切换展开/折叠；结果作为显式覆盖记录，后续刷新按工作区 Anchor 迁移。
    fn toggle(&mut self, hunk_start: &Anchor, working: &Snapshot, expanded_by_default: bool) {
        let expanded = !self.is_expanded(hunk_start, working, expanded_by_default);
        match self
            .overrides
            .iter_mut()
            .find(|over| anchor_matches(&over.hunk_start, hunk_start, working))
        {
            Some(over) => over.expanded = expanded,
            None => self.overrides.push(HunkExpansionOverride {
                hunk_start: *hunk_start,
                expanded,
            }),
        }
    }

    fn override_for(
        &self,
        hunk_start: &Anchor,
        working: &Snapshot,
    ) -> Option<&HunkExpansionOverride> {
        self.overrides
            .iter()
            .find(|over| anchor_matches(&over.hunk_start, hunk_start, working))
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
    new: &[&DiffHunk],
    working: &Snapshot,
    expansion: &mut DiffExpansionState,
) {
    for new_hunk in new {
        let Some(over) = old_expansion.override_for(&new_hunk.buffer_range.start, working) else {
            continue;
        };
        expansion.overrides.push(HunkExpansionOverride {
            hunk_start: new_hunk.buffer_range.start,
            expanded: over.expanded,
        });
    }
}

/// 一个可见窗口对应一个稳定 working excerpt，hunk 拆分只由输出变换承担。
fn materialize_file(file: &DiffState, cx: &App, excerpts: &mut Vec<ExcerptRange>) {
    let working = file.diff.read(cx).working().clone();
    let text = working.read(cx).text_snapshot();
    for range in file.excerpt_ranges.iter(text.line_count()) {
        excerpts.push(
            ExcerptRange::line_range_from_text(working.clone(), &text, range)
                .with_display_path(file.display_path.as_path().to_path_buf()),
        );
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
