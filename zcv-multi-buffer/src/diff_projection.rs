//! MultiBuffer 的 git diff 投影：把版本化的 BufferDiff 结果物化为 excerpts 与显示坐标。
//!
//! 普通编辑器与多文件投影（Git 差异视图）共用同一套物化：
//! 宿主注入同一工作区源快照对应的 BufferDiff，本层只消费其 BufferDiffSnapshot，
//! 按展开状态把旧侧行物化为只读 excerpt、按显示策略裁剪可见行，并派生组合坐标显示 hunks。
//!
//! diff 状态（base/working、版本、hunk、pending、操作）全部归 BufferDiff 所有；
//! hunk 身份随输出变换节点（Excerpt）承载，输出坐标由游标推导；
//! 展开/折叠、显示路径与上下文裁剪归本层所有，不进入 diff 快照。

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, Context, Entity, Subscription};
use sum_tree::SumTree;
use zcv_language::{LanguageBuffer, LanguageBufferEvent};
use zcv_text::{Anchor, ByteOffset, Line, Snapshot};

use crate::{
    DiffTransform, DiffTransformHunkInfo, DiffTransformHunkSide, Excerpt, ExcerptDiffKind,
    ExcerptRange, MBTextSummary, MultiBuffer, MultiBufferCursor, MultiBufferEvent, PathKey,
    mapping_count, output_summary_for_path,
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

/// 一个文件的 diff 注入项：预创建的 diff 实体 + 显示配置。
///
/// diff 实体由 GitStore 按 (working, base) 共享；显示配置由注入方（视图）持有。
#[derive(Clone)]
pub struct DiffFile {
    /// 权威 diff 实体（GitStore 预创建并共享）。
    pub diff: Entity<BufferDiff>,
    /// 组合文档中的显示路径（文件标题与导航定位）。
    pub display_path: PathBuf,
    /// 显示策略：None 显示整个新侧文件（普通编辑器）；
    /// Some(n) 只显示 hunk 周围 n 行上下文（多文件投影）。
    pub context_lines: Option<usize>,
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
    /// 显示策略：None 显示整个新侧文件；Some(n) 只显示 hunk 周围 n 行上下文。
    context_lines: Option<usize>,
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
        context_lines: Option<usize>,
        cx: &mut Context<MultiBuffer>,
    ) -> Self {
        let input_subscriptions = Self::subscribe_inputs(&diff, cx);
        let subscription = cx.subscribe(&diff, |this, _, event, cx| {
            let BufferDiffEvent::DiffChanged { refresh } = event;
            this.diff_changed(*refresh, cx);
        });
        Self {
            diff,
            display_path,
            context_lines,
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

/// 一个 MultiBuffer 的 git diff 投影状态。
/// 绑定一份组合快照的 diff 显示输入。
///
/// 它是由 `MultiBuffer` 权威投影派生出的不可变值：
/// 显示层只消费该快照，不再把 diff 几何变更混入语言和设置使用的 `metadata_version`。
/// 一个 path 的 diff 显示缓存；坐标相对该 path 的组合输出起点。
///
/// 未变化 path 的缓存在源编辑时以同一 `Arc` 原样保留，只替换受影响 path 的 segment，
/// 因此单路径更新不复制、不排序其它 path 的 hunk。
#[derive(Clone, Debug, PartialEq)]
struct PathDiffDisplay {
    path: PathKey,
    /// 该 path 在组合输出中的起始行与起始字节；随前序 path 的输出长度变化。
    output_start_line: usize,
    output_start_byte: usize,
    hunks: Arc<[DisplayHunk]>,
    old_ranges: Arc<[Option<Range<usize>>]>,
    sources: Arc<[DisplayHunkSource]>,
    expanded: Arc<[bool]>,
    word_diffs: Arc<[WordDiffs]>,
}

impl PathDiffDisplay {
    /// 把 path 内的绝对显示坐标转成相对该 path 输出起点的缓存。
    fn from_absolute(
        path: PathKey,
        output_start_line: usize,
        output_start_byte: usize,
        display: DiffDisplay,
    ) -> Self {
        let shift_line = |range: Range<usize>| subtract_offset(range, output_start_line);
        let hunks = display
            .hunks
            .into_iter()
            .map(|hunk| DisplayHunk {
                range: shift_line(hunk.range),
                old_range: hunk.old_range,
                kind: hunk.kind,
                staging: hunk.staging,
            })
            .collect::<Vec<_>>();
        let old_ranges = display
            .old_ranges
            .into_iter()
            .map(|range| range.map(shift_line))
            .collect::<Vec<_>>();
        let word_diffs = display
            .word_diffs
            .into_iter()
            .map(|diffs| {
                diffs
                    .into_iter()
                    .map(|(kind, range)| (kind, subtract_offset(range, output_start_byte)))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        Self {
            path,
            output_start_line,
            output_start_byte,
            hunks: Arc::from(hunks),
            old_ranges: Arc::from(old_ranges),
            sources: Arc::from(display.sources),
            expanded: Arc::from(display.expanded),
            word_diffs: Arc::from(word_diffs),
        }
    }
}

/// 一个 path 的 diff 显示分段视图；坐标相对该 path 的输出起点。
///
/// 消费方按段遍历并叠加 output_start_*，不整体展平，也不缓存扁平副本。
pub struct DiffSegment<'a> {
    pub output_start_line: usize,
    pub output_start_byte: usize,
    pub hunks: &'a [DisplayHunk],
    pub old_ranges: &'a [Option<Range<usize>>],
    pub expanded: &'a [bool],
    pub word_diffs: &'a [WordDiffs],
}

/// 一条已解析为组合绝对坐标的 diff 显示输入。
pub struct ResolvedDiffHunk {
    pub hunk: DisplayHunk,
    pub old_range: Option<Range<usize>>,
    pub expanded: bool,
    pub word_diffs: WordDiffs,
}

/// diff 投影的显示缓存。
///
/// 以 path 为持久化分段：源编辑只替换受影响 path 的 PathDiffDisplay，未变化 path 复用同一 Arc；
/// 消费方按段读取，按显示 hunk 序号访问时做一次线性定位。
#[derive(Clone, Debug)]
pub struct DiffDisplaySnapshot {
    version: u64,
    segments: Arc<[PathDiffDisplay]>,
}

impl Default for DiffDisplaySnapshot {
    fn default() -> Self {
        Self {
            version: 0,
            segments: Arc::from(Vec::<PathDiffDisplay>::new()),
        }
    }
}

impl DiffDisplaySnapshot {
    pub fn version(&self) -> u64 {
        self.version
    }

    /// 按 path 顺序遍历分段；坐标为相对该 path 输出起点的值。
    pub fn segments(&self) -> impl Iterator<Item = DiffSegment<'_>> + '_ {
        self.segments.iter().map(|segment| DiffSegment {
            output_start_line: segment.output_start_line,
            output_start_byte: segment.output_start_byte,
            hunks: &segment.hunks,
            old_ranges: &segment.old_ranges,
            expanded: &segment.expanded,
            word_diffs: &segment.word_diffs,
        })
    }

    pub fn len(&self) -> usize {
        self.segments
            .iter()
            .map(|segment| segment.hunks.len())
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 按扁平显示 hunk 序号取绝对坐标 hunk。
    pub fn hunk_at(&self, index: usize) -> Option<DisplayHunk> {
        let (segment, local) = self.locate(index)?;
        let hunk = &segment.hunks[local];
        Some(DisplayHunk {
            range: add_offset(hunk.range.clone(), segment.output_start_line),
            old_range: hunk.old_range.clone(),
            kind: hunk.kind,
            staging: hunk.staging,
        })
    }

    /// 按扁平显示 hunk 序号取源定位。
    fn source_at(&self, index: usize) -> Option<DisplayHunkSource> {
        let (segment, local) = self.locate(index)?;
        Some(segment.sources[local].clone())
    }

    /// 扁平展开标志（不携带坐标，直接拼接）。
    pub fn expanded_flags(&self) -> Vec<bool> {
        self.resolved().map(|hunk| hunk.expanded).collect()
    }

    /// 扁平绝对坐标 hunk 列表（按需查询，不缓存）。
    pub fn hunks_absolute(&self) -> Vec<DisplayHunk> {
        self.resolved().map(|hunk| hunk.hunk).collect()
    }

    /// 扁平旧侧显示行范围（按需查询，不缓存）。
    pub fn old_ranges_absolute(&self) -> Vec<Option<Range<usize>>> {
        self.resolved().map(|hunk| hunk.old_range).collect()
    }

    /// 扁平词级变化片段（绝对字节范围，按需查询，不缓存）。
    pub fn word_diffs_absolute(&self) -> Vec<WordDiffs> {
        self.resolved().map(|hunk| hunk.word_diffs).collect()
    }

    /// 按 path 分段解析为组合绝对坐标的显示输入。
    ///
    /// 显示层按此迭代，不展平、不缓存跨 path 的扁平副本。
    pub fn resolved(&self) -> impl Iterator<Item = ResolvedDiffHunk> + '_ {
        self.segments().flat_map(|segment| {
            let line = segment.output_start_line;
            let byte = segment.output_start_byte;
            (0..segment.hunks.len()).map(move |local| ResolvedDiffHunk {
                hunk: DisplayHunk {
                    range: add_offset(segment.hunks[local].range.clone(), line),
                    old_range: segment.hunks[local].old_range.clone(),
                    kind: segment.hunks[local].kind,
                    staging: segment.hunks[local].staging,
                },
                old_range: segment.old_ranges[local]
                    .clone()
                    .map(|range| add_offset(range, line)),
                expanded: segment.expanded[local],
                word_diffs: segment.word_diffs[local]
                    .iter()
                    .map(|(kind, range)| (*kind, add_offset(range.clone(), byte)))
                    .collect(),
            })
        })
    }

    /// 扁平显示 hunk 序号转 (所属分段, 段内序号)。
    fn locate(&self, index: usize) -> Option<(&PathDiffDisplay, usize)> {
        let mut offset = index;
        for segment in self.segments.iter() {
            if offset < segment.hunks.len() {
                return Some((segment, offset));
            }
            offset -= segment.hunks.len();
        }
        None
    }
}

fn add_offset(range: Range<usize>, offset: usize) -> Range<usize> {
    range.start.saturating_add(offset)..range.end.saturating_add(offset)
}

fn subtract_offset(range: Range<usize>, offset: usize) -> Range<usize> {
    range.start.saturating_sub(offset)..range.end.saturating_sub(offset)
}

fn shift_offset(value: usize, delta: isize) -> usize {
    (value as isize + delta).max(0) as usize
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
    /// hunk 所属的组合路径，用于源编辑后的局部缓存更新。
    path: PathKey,
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

/// 一次物化派生出的显示坐标；由输出变换树单次游标遍历推导。
struct DiffDisplay {
    hunks: Vec<DisplayHunk>,
    old_ranges: Vec<Option<Range<usize>>>,
    sources: Vec<DisplayHunkSource>,
    expanded: Vec<bool>,
    word_diffs: Vec<WordDiffs>,
}

/// 单次游标遍历中按 hunk 身份聚合的输出范围与词级片段。
struct HunkAccum {
    working: gpui::EntityId,
    hunk_start: Option<Anchor>,
    path: PathKey,
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
    fn new(info: &DiffTransformHunkInfo, path: &PathKey) -> Self {
        Self {
            working: info.working,
            hunk_start: info.hunk_start,
            path: path.clone(),
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
    buffer_id: zcv_text::BufferId,
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
                && current.context_lines == file.context_lines
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

    /// 同路径的 diff 实体或显示配置变化：只替换该文件的 excerpts，不重建整份组合文档。
    ///
    /// 展开状态按旧/新 hunk 迁移；新 diff 尚未算完时先登记 pending 迁移，
    /// 保留现有 excerpts，等 DiffChanged 到期后由 diff_changed 增量替换。
    fn replace_diff_file(&mut self, index: usize, file: DiffFile, cx: &mut Context<Self>) -> bool {
        let working_id_matches = self.diffs[index].diff.read(cx).working().entity_id()
            == file.diff.read(cx).working().entity_id();
        let mut next = DiffState::new(
            file.diff,
            PathKey::new(file.display_path),
            file.context_lines,
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
            file.context_lines,
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
        self.diffs[insert_at].revision = Some(self.diffs[insert_at].diff.read(cx).revision());
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
                    file.context_lines,
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
            let hunk = diff.hunk_at(display_index)?;
            Some((source.working, hunk.kind, source.hunk_start?))
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
        self.diff
            .as_ref()
            .map_or_else(Vec::new, |diff| diff.hunks_absolute())
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
        self.diff
            .as_ref()
            .map_or_else(Vec::new, |diff| diff.old_ranges_absolute())
    }

    /// 与 MultiBuffer::diff_hunks 平行的词级变化片段（组合文档字节范围 + 新增/删除色）。
    pub fn diff_hunk_word_diffs(&self) -> Vec<WordDiffs> {
        self.diff
            .as_ref()
            .map_or_else(Vec::new, |diff| diff.word_diffs_absolute())
    }

    /// 与 MultiBuffer::diff_hunks 平行的展开标志（渲染层按显示 hunk 索引查询）。
    pub fn diff_hunk_expanded(&self) -> Vec<bool> {
        self.diff
            .as_ref()
            .map_or_else(Vec::new, |diff| diff.expanded_flags())
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
    fn diff_changed(&mut self, refresh: DiffRefresh, cx: &mut Context<Self>) {
        // 对齐 Zed 的 buffer_diff_changed：先把待同步的源快照纳入当前帧，
        // 再更新 diff transform，最后只发布一次组合投影版本。
        self.begin_projection_sync();
        self.sync_pending_sources(cx);
        self.diff_changed_inner(refresh, cx);
        self.finish_projection_sync(cx);
    }

    fn diff_changed_inner(&mut self, refresh: DiffRefresh, cx: &mut Context<Self>) {
        {
            let Some(diff) = &self.diff else {
                return;
            };
            if refresh == DiffRefresh::PreserveProjection {
                return;
            }
            let working_is_dirty = self
                .diffs
                .iter()
                .any(|file| file.diff.read(cx).working().read(cx).is_dirty());
            if working_is_dirty
                && !diff
                    .segments()
                    .any(|segment| segment.expanded.iter().any(|&expanded| expanded))
            {
                // 折叠态组合文档的 excerpt 是用户当前正在编辑的稳定窗口。
                // 没有展开 hunk 时只更新 BufferDiff 快照，等保存/重新注入后再提交新的窗口；
                // 展开态则必须跟随新的 working 快照重物化，保证可见 hunk 与正文一致。
                return;
            }
        }
        for index in 0..self.diffs.len() {
            if self.diffs[index].pending_expansion_state.is_none()
                || !self.diffs[index]
                    .diff
                    .read(cx)
                    .is_current_version_calculated(cx)
            {
                continue;
            }
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
        let (calculated_prefix, materialized, any_materialized, prefix_changed) = {
            let calculated_prefix = self
                .diffs
                .iter()
                .take_while(|file| file.diff.read(cx).is_current_version_calculated(cx))
                .count();
            let materialized = self.diff_materialized_files.min(self.diffs.len());
            let any_materialized = self.diffs[..materialized]
                .iter()
                .any(|file| file.revision.is_some());
            let prefix_changed = self.diff_materialized_files > self.diffs.len()
                || !any_materialized
                || self.diffs[..materialized]
                    .iter()
                    .any(|file| file.revision != Some(file.diff.read(cx).revision()));
            (
                calculated_prefix,
                materialized,
                any_materialized,
                prefix_changed,
            )
        };
        if prefix_changed {
            // 尚无任何已物化文件（首个 diff 结果到达）或文件集合与已物化前缀不一致时整体重建。
            if !any_materialized || self.diff_materialized_files > self.diffs.len() {
                self.rebuild_diff_projection(cx);
                return;
            }
            // 身份或版本变化的文件逐个原地重物化，只替换对应路径的 excerpts；
            // 未变化的路径其组合坐标与展开状态保持不变。
            let changed = (0..materialized)
                .filter(|&index| {
                    self.diffs[index].revision != Some(self.diffs[index].diff.read(cx).revision())
                })
                .collect::<Vec<_>>();
            for index in changed {
                self.replace_materialized_file(index, cx);
            }
        }
        // 尾部新就绪的文件只增量追加，不重建已物化的前缀。
        if calculated_prefix > materialized {
            self.append_materialized_files(materialized, calculated_prefix, cx);
        }
    }

    /// 按路径顺序登记追加的 diff 文件，并物化其中已计算完成的前缀。
    ///
    /// 尚未计算完成的文件只登记订阅；结果到达后由 diff_changed 增量物化。
    pub fn append_diff_projection(&mut self, files: Vec<DiffFile>, cx: &mut Context<Self>) -> bool {
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
                    file.context_lines,
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
        // 该文件已无可见 hunk（差异被消除等）时必须移除其路径的 excerpts；
        // set_excerpts_for_path 对空片段集合是空操作，无法表达“清空该路径”。
        if excerpts.is_empty() {
            self.remove_excerpts_for_path(path.as_path(), cx);
        } else {
            self.set_excerpts_for_path(excerpts, cx);
        }
        self.diffs[file_index].revision = Some(self.diffs[file_index].diff.read(cx).revision());
        self.refresh_diff_display(cx);
    }

    /// 按展开状态与显示策略重建可见 excerpts，并派生显示坐标 hunks。
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
            file.revision = Some(file.diff.read(cx).revision());
        }
        // 整体重建会物化全部文件（未就绪文件按空 hunk 投影），因此前缀直接取文件总数。
        self.diff_materialized_files = self.diffs.len();
        self.refresh_diff_display(cx);
        self.state.projection_version != old_version
    }

    /// 单次 cursor 遍历输出变换树，从节点携带的 hunk 身份派生显示坐标。
    ///
    /// 输出范围由游标位置推导，不再为每个 hunk 反查 excerpt，也不保留源坐标副本。
    /// 按 path 顺序遍历组合文档，收集拥有显示缓存的源路径。
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

    /// 从当前组合映射全量派生每个 path 的显示缓存；只用于低频拓扑变化。
    fn derive_diff_display_segments(&self) -> Vec<PathDiffDisplay> {
        let mut segments = Vec::new();
        let mut output_start_line = 0usize;
        let mut output_start_byte = 0usize;
        for path in self.diff_display_paths() {
            let summary = output_summary_for_path(&self.state.diff_transforms, &path);
            let display = self.derive_diff_display_for_path(&path);
            segments.push(PathDiffDisplay::from_absolute(
                path,
                output_start_line,
                output_start_byte,
                display,
            ));
            output_start_line += summary.lines;
            output_start_byte += summary.len;
        }
        segments
    }

    /// 只遍历一个 path 的输出变换，用于源编辑后的局部显示缓存更新。
    fn derive_diff_display_for_path(&self, path: &PathKey) -> DiffDisplay {
        let mut index_of: HashMap<(gpui::EntityId, Option<Anchor>), usize> = HashMap::new();
        let mut accums: Vec<HunkAccum> = Vec::new();
        let mut cursor = MultiBufferCursor::new(&self.state.excerpts, &self.state.diff_transforms);
        // 用 Left 偏置从边界开始遍历：整份删除的零长度边界节点也必须被访问。
        cursor.seek_path(path, sum_tree::Bias::Left);
        while let Some((excerpt, transform)) = cursor.item() {
            if &excerpt.path != path {
                break;
            }
            if !transform.hunks().is_empty() {
                let start = cursor.start().clone();
                let content_lines = excerpt.text_summary.lines + excerpt.adds_newline as usize;
                let range = start.lines..(start.lines + content_lines).max(start.lines + 1);
                for info in transform.hunks() {
                    let key = (info.working, info.hunk_start);
                    let index = *index_of.entry(key).or_insert_with(|| {
                        accums.push(HunkAccum::new(info, &excerpt.path));
                        accums.len() - 1
                    });
                    let accum = &mut accums[index];
                    accum.expanded = info.expanded;
                    match info.side {
                        DiffTransformHunkSide::Content => {
                            accum.content_range = Some(range.clone());
                            let output_start = start.bytes;
                            let source_start = excerpt.source_range.start().get();
                            accum.new_word_diffs = info
                                .buffer_word_diffs
                                .iter()
                                .map(|diff| {
                                    let start =
                                        output_start + diff.start.offset().get() - source_start;
                                    let end = output_start + diff.end.offset().get() - source_start;
                                    (DiffHunkKind::Added, start..end)
                                })
                                .collect();
                        }
                        DiffTransformHunkSide::Old => {
                            accum.old_range = Some(range.clone());
                            if info.expanded {
                                let output_start = start.bytes;
                                let source_start = excerpt.source_range.start().get();
                                accum.old_word_diffs = info
                                    .base_word_diffs
                                    .iter()
                                    .map(|diff| {
                                        let start =
                                            output_start + info.base_byte_start + diff.start
                                                - source_start;
                                        let end = output_start + info.base_byte_start + diff.end
                                            - source_start;
                                        (DiffHunkKind::Deleted, start..end)
                                    })
                                    .collect();
                            }
                        }
                        DiffTransformHunkSide::BoundaryStart => {
                            accum.boundary_start = Some(start.lines);
                        }
                        DiffTransformHunkSide::BoundaryEnd => {
                            accum.boundary_end = Some(start.lines + content_lines);
                        }
                    }
                }
            }
            cursor.next();
        }

        let mut hunks = Vec::with_capacity(accums.len());
        let mut old_ranges = Vec::with_capacity(accums.len());
        let mut sources = Vec::with_capacity(accums.len());
        let mut expanded = Vec::with_capacity(accums.len());
        let mut word_diffs = Vec::with_capacity(accums.len());
        for accum in accums {
            let range = accum
                .content_range
                .or_else(|| accum.old_range.as_ref().map(|range| range.end..range.end))
                .or_else(|| accum.boundary_start.map(|line| line..line))
                .or_else(|| accum.boundary_end.map(|line| line..line))
                .expect("diff hunk 必须挂到输出变换节点");
            let mut combined = accum.old_word_diffs;
            combined.extend(accum.new_word_diffs);
            hunks.push(DisplayHunk {
                range,
                old_range: accum.base_lines,
                kind: accum.kind,
                staging: accum.staging,
            });
            old_ranges.push(accum.old_range);
            sources.push(DisplayHunkSource {
                working: accum.working,
                hunk_start: accum.hunk_start,
                path: accum.path,
            });
            expanded.push(accum.expanded);
            word_diffs.push(combined);
        }
        DiffDisplay {
            hunks,
            old_ranges,
            sources,
            expanded,
            word_diffs,
        }
    }

    /// 源编辑后只替换受影响 path 的显示缓存，并平移其后的 path 输出起点。
    ///
    /// path 内的变换需要重新读取 hunk 身份与词级范围；其它 path 的 segment 以同一
    /// `Arc` 原样保留，只调整绝对输出起点，不复制、不排序其它 path 的 hunk。
    pub(crate) fn refresh_diff_display_for_path(
        &mut self,
        path: &PathKey,
        old_summary: MBTextSummary,
        new_summary: MBTextSummary,
        cx: &mut Context<Self>,
    ) {
        let Some(current) = self.diff.as_ref() else {
            return;
        };
        let Some(index) = current
            .segments
            .iter()
            .position(|segment| &segment.path == path)
        else {
            // 该 path 没有显示缓存：它不在 excerpt 投影中（如无片段的 base/index 修订源被编辑）。
            // 显示缓存的增删由 excerpt 物化路径负责，这里没有可增量更新的内容。
            return;
        };
        let line_delta = new_summary.lines as isize - old_summary.lines as isize;
        let byte_delta = new_summary.len as isize - old_summary.len as isize;
        let output_start_line = current.segments[index].output_start_line;
        let output_start_byte = current.segments[index].output_start_byte;
        let display = self.derive_diff_display_for_path(path);
        let segment = PathDiffDisplay::from_absolute(
            path.clone(),
            output_start_line,
            output_start_byte,
            display,
        );
        if line_delta == 0 && byte_delta == 0 && current.segments[index] == segment {
            return;
        }
        let version = current.version.wrapping_add(1);
        let mut segments = current.segments.to_vec();
        segments[index] = segment;
        for segment in segments.iter_mut().skip(index + 1) {
            segment.output_start_line = shift_offset(segment.output_start_line, line_delta);
            segment.output_start_byte = shift_offset(segment.output_start_byte, byte_delta);
        }
        // diff 显示几何是组合投影的局部派生输入，而不是语言／设置元数据。
        // 快照整体替换使 DisplayMap 能按独立显示版本更新装饰，同时不重建显示拓扑。
        self.diff = Some(Arc::new(DiffDisplaySnapshot {
            version,
            segments: Arc::from(segments),
        }));
        self.snapshot_dirty = true;
        self.notify_if_not_syncing(cx);
    }

    /// 在低频拓扑或 diff 物化变化后按当前组合映射重算全部 path 的显示缓存。
    ///
    /// 普通源编辑使用 `refresh_diff_display_for_path`，不会进入这里。
    pub(crate) fn refresh_diff_display(&mut self, cx: &mut Context<Self>) {
        if self.diff.is_none() {
            return;
        }
        let segments = self.derive_diff_display_segments();
        if let Some(current) = self.diff.as_ref()
            && current.segments.as_ref() == segments.as_slice()
        {
            return;
        }
        let version = self
            .diff
            .as_ref()
            .map_or(0, |current| current.version.wrapping_add(1));
        self.diff = Some(Arc::new(DiffDisplaySnapshot {
            version,
            segments: Arc::from(segments),
        }));
        self.snapshot_dirty = true;
        self.notify_if_not_syncing(cx);
    }
}

/// 解析一个文件当前的可见 hunk（pending 抑制后）为显示行坐标。
fn resolve_file_hunks(file: &DiffState, cx: &App) -> Vec<ResolvedHunk> {
    let entity = file.diff.clone();
    let (working_text, base_text, hunks) = {
        let diff = entity.read(cx);
        let working_text = diff.working().read(cx).text_snapshot();
        let base_text = diff.base_source().map(|base| base.read(cx).text_snapshot());
        (working_text, base_text, diff.snapshot().visible_hunks())
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
    let buffer_lines = line_at_or_end(working, hunk.buffer_range.start.offset())
        ..line_at_or_end(working, hunk.buffer_range.end.offset());
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
    let working = file.diff.read(cx).working().clone();
    let base_source = file.diff.read(cx).base_source().cloned();
    let is_created = file.diff.read(cx).is_created();
    let working_text = working.read(cx).text_snapshot();
    let line_count = working_text.line_count();
    let display_path = file.display_path.clone();
    let context_lines = file.context_lines;
    let working_id = working.entity_id();
    let working_buffer_id = working.read(cx).buffer_id();
    let expansion = &file.expansion;
    let mut materializer = ExcerptMaterializer {
        excerpts,
        display_path: display_path.as_path(),
        buffer_id: working_buffer_id,
    };

    // 整文件新增：整个新侧文件作为 Added 显示（无旧侧）。
    if is_created && resolved.is_empty() {
        materializer.push(
            0..line_count,
            &working_text,
            &working,
            ExcerptShape {
                diff_kind: Some(ExcerptDiffKind::Added),
                starts_logical_excerpt: true,
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
    // 无行级差异：整文件模式显示整个新侧文件（空文件保留占位行），裁剪模式不显示。
    if resolved.is_empty() {
        if context_lines.is_none() {
            materializer.push(
                0..line_count,
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
        return;
    }

    let visible = match context_lines {
        None => std::iter::once(0..line_count).collect::<Vec<_>>(),
        Some(context) => excerpt_line_ranges(&resolved, line_count, context),
    };
    for context_range in visible {
        let mut current = context_range.start;
        // 每个可见窗口只由首个物理片段开启一个逻辑 excerpt；窗口内的旧侧/新侧/上下文片段都不再另起边界。
        // 是否绘制实体 header 由 MultiBufferSnapshot::show_headers 决定，不由物化决定。
        let mut starts_logical_excerpt = true;
        // 无旧侧物化的纯删除需要一个相邻内容节点承载边界；挂到后继内容起点，无后继时挂到前驱终点。
        let mut pending_boundary: Option<DiffTransformHunkInfo> = None;
        for hunk in resolved
            .iter()
            .filter(|hunk| hunk_is_inside_excerpt(hunk, &context_range))
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
            // 旧侧：展开时物化完整旧行；裁剪模式折叠时用空占位行标记删除点。
            let mut old_materialized = false;
            if !hunk.base_lines.is_empty() {
                if expanded && let Some(base) = base_source.as_ref() {
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
                } else if context_lines.is_some() {
                    // 折叠占位行：空 Deleted 片段（组合文档为它保留一个显示行）。
                    let base = base_source.as_ref().expect("删除点占位需要 base 来源");
                    let base_text = base.read(cx).text_snapshot();
                    let hunks = vec![hunk_info(
                        working_id,
                        DiffTransformHunkSide::Old,
                        hunk,
                        false,
                    )];
                    materializer.push(
                        hunk.base_lines.start..hunk.base_lines.start,
                        &base_text,
                        base,
                        ExcerptShape {
                            diff_kind: Some(ExcerptDiffKind::Deleted),
                            starts_logical_excerpt,
                            allow_empty: true,
                        },
                        hunks,
                    );
                    old_materialized = true;
                    starts_logical_excerpt = false;
                }
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
        if current < context_range.end {
            let leftover = materializer.push(
                current..context_range.end,
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
                    0..line_count.max(1),
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
    buffer_id: zcv_text::BufferId,
    shape: ExcerptShape,
) -> Option<ExcerptRange> {
    if lines.is_empty() && !shape.allow_empty {
        return None;
    }
    let mut excerpt = ExcerptRange::line_range_from_text(source.clone(), text, lines);
    // 空源范围的普通片段没有可显示内容：跳过（deleted 文件的占位上下文等）。
    // 整文件显示（allow_empty）保留占位行，diff 片段（旧侧/新增）始终物化。
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

fn excerpt_line_ranges(
    hunks: &[ResolvedHunk],
    line_count: usize,
    context_lines: usize,
) -> Vec<Range<usize>> {
    let max_line = line_count.saturating_sub(1);
    let mut ranges = hunks
        .iter()
        .map(|hunk| {
            let start = hunk
                .buffer_lines
                .start
                .min(max_line)
                .saturating_sub(context_lines);
            // Zcv 的行范围右开；
            // 非空 hunk 先换算为最后一条变更行，才能得到真正的后两行上下文。
            let changed_end_line = if hunk.buffer_lines.is_empty() {
                hunk.buffer_lines.start
            } else {
                hunk.buffer_lines.end.saturating_sub(1)
            };
            let end_line = changed_end_line.saturating_add(context_lines).min(max_line);
            start..end_line + 1
        })
        .collect::<Vec<_>>();
    ranges.sort_by_key(|range| range.start);

    let mut merged = Vec::<Range<usize>>::new();
    for range in ranges {
        if let Some(previous) = merged.last_mut()
            && range.start <= previous.end
        {
            previous.end = previous.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

fn hunk_is_inside_excerpt(hunk: &ResolvedHunk, excerpt: &Range<usize>) -> bool {
    if hunk.buffer_lines.is_empty() {
        excerpt.contains(&hunk.buffer_lines.start)
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
