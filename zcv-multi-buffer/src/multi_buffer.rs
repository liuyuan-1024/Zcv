//! Editor 与具体文本 Buffer 之间的组合文档边界。
//!
//! 普通组合文档按自然路径顺序组织 excerpts；项目差异组合文档按目录优先的树形自然顺序组织。
//! 两种顺序由创建入口固定，组合坐标到源文件坐标的映射仍归同一模型维护。
//! 普通编辑器是「整文件单 excerpt」的组合文档；差异投影在稳定 excerpts 上叠加输出变换。
//! Editor 始终只消费本层，不感知来源数量。
//! diff 显示拓扑（git hunks、展开状态、跟踪区间与显示坐标）见 [`diff_projection`]。

mod buffer_construct;
mod buffer_edit;
mod buffer_excerpts;
mod buffer_projection;
mod buffer_query;
mod buffer_source_sync;
mod diff_projection;
mod diff_transform_sync;
mod path_key;
mod snapshot_anchor;
mod snapshot_indent;
mod snapshot_outline;
mod snapshot_syntax;
mod snapshot_text;
mod snapshot_topology;

pub use diff_projection::{
    DiffDisplaySnapshot, DiffExcerptRanges, DiffFile, DiffHunkSource, DisplayHunk,
    ResolvedDiffHunk, WordDiffs,
};
pub(crate) use path_key::{PathKey, PathKeyIndex, PathOrder};

use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::{HashMap, HashSet, hash_map::Entry},
    iter, mem,
    ops::Range,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
};

use gpui::{App, Context, Entity, EntityId, EventEmitter, Subscription};
use sum_tree::{Bias, ContextLessSummary, Cursor, Dimension, Item, SeekTarget, SumTree, TreeMap};
use unicode_segmentation::UnicodeSegmentation;
use zcv_buffer_diff::{DiffHunkKind, DiffHunkStaging};
use zcv_language::{
    BracketPair, HighlightCache, HighlightSpan, IndentGuideSettings, InputScope, LanguageBuffer,
    LanguageBufferEvent, LanguageBufferSnapshot, LanguageRegistry, LanguageSettings, LocalBinding,
    NewlineIndent, OutlineItem, SyntaxNode, SyntaxSnapshot,
};
use zcv_text::{
    Affinity, Anchor, Buffer, BufferConfig, BufferId, BufferVersion, ByteOffset, CharOffset,
    CoordinateError, Edit, Line, LineEndingStyle, LineIndent, LogicalColumn, MovementDirection,
    MovementUnit, Position, PositionMap, Snapshot, Stickiness, StorageError, TextChangeBatch,
    TextError, TextRange, TextRead, TextResult, TextSubscription, TransactionError, TransactionId,
    TransactionMetadata, Utf16Offset, WordBoundaryPolicy,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndentGuide {
    pub buffer_id: BufferId,
    pub start_row: Line,
    pub end_row: Line,
    pub depth: usize,
    pub tab_size: usize,
    pub settings: IndentGuideSettings,
}

impl IndentGuide {
    pub fn indent_level(&self) -> usize {
        self.depth * self.tab_size
    }
}

#[derive(Clone)]
struct IndentLine {
    row: Line,
    indent: LineIndent,
    buffer_id: BufferId,
    tab_size: usize,
    settings: IndentGuideSettings,
}

/// 组合文档中的一个源片段。
#[derive(Clone)]
pub struct ExcerptRange {
    source: Entity<LanguageBuffer>,
    source_range: TextRange,
    match_ranges: Vec<TextRange>,
    display_path: Option<PathKey>,
    editable: bool,
}

/// 一次尾部片段追加产生的匹配范围及其组合文档版本。
pub struct AppendedExcerpts {
    pub base_version: BufferVersion,
    pub version: BufferVersion,
    pub match_ranges: Vec<MultiBufferRange>,
}

/// 组合投影片段在统一 diff 中承担的文本侧别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExcerptDiffKind {
    Added,
    Deleted,
}

/// 组合输出行的差异状态，由当前输出变换直接提供给显示行游标。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffRowStatus {
    pub kind: DiffHunkKind,
    pub staging: DiffHunkStaging,
    pub expanded: bool,
}

impl ExcerptRange {
    pub fn new(
        source: Entity<LanguageBuffer>,
        source_range: TextRange,
        match_ranges: Vec<TextRange>,
    ) -> Self {
        Self {
            source,
            source_range,
            match_ranges,
            display_path: None,
            editable: true,
        }
    }

    pub fn with_display_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.display_path = Some(PathKey::new(path.into()));
        self
    }

    /// 取源文档中一个 0-based、左闭右开的完整逻辑行范围。
    ///
    /// 范围终点可以等于 `line_count`；
    /// 空文件的 `0..1` 会得到零字节源范围，按组合尾换行不变式在非末尾片段时占一个组合边界行。
    pub fn line_range(source: Entity<LanguageBuffer>, lines: Range<usize>, cx: &App) -> Self {
        let text = source.read(cx).text_snapshot();
        Self::line_range_from_text(source, &text, lines)
    }

    /// 从已读取的源文本快照取行范围。
    ///
    /// `line_range` 与 diff 文档的 working 窗口共用此入口。
    pub(crate) fn line_range_from_text(
        source: Entity<LanguageBuffer>,
        text: &Snapshot,
        lines: Range<usize>,
    ) -> Self {
        assert!(lines.start <= lines.end, "excerpt 行范围必须正序");
        assert!(lines.end <= text.line_count(), "excerpt 行范围不能越界");
        let start = text
            .line_start_byte(Line::new(lines.start))
            .expect("excerpt 起始行必须有效");
        let end = if lines.end == text.line_count() {
            text.len_bytes()
        } else {
            text.line_start_byte(Line::new(lines.end))
                .expect("excerpt 终止行必须有效")
        };
        Self::new(
            source,
            TextRange::new(start, end).expect("excerpt 行范围必须有效"),
            Vec::new(),
        )
    }

    pub fn match_count(&self) -> usize {
        self.match_ranges.len()
    }

    pub fn source_range(&self) -> TextRange {
        self.source_range
    }
}

/// 组合坐标对应的源文件位置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExcerptLocation {
    pub path: PathBuf,
    pub source_range: TextRange,
}

/// 组合文档中的稳定位置。
///
/// 组合 Anchor：要么是文档边界，要么绑定一个源片段身份与源文本 Anchor。
/// 稳定位置不保存裸偏移；解析时按当前快照用源 Anchor 推进，再按 excerpt 身份投影到组合坐标。
/// 文件退出投影时按当前路径顺序解析到最近的后继，没有后继时回到前驱。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MultiBufferAnchor {
    /// 始终解析到组合文档开头。
    Min,
    /// 绑定具体源片段的组合位置。
    Excerpt(ExcerptAnchor),
    /// 始终解析到组合文档末尾。
    Max,
}

/// 绑定源片段身份与源文本 Anchor 的组合位置。
///
/// `source_id` 为 `None` 表示纯文本派生快照（没有工作区源实体）；
/// `text_anchor` 承载源内位置与插入点吸附方向（affinity）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExcerptAnchor {
    path: PathKeyIndex,
    source_id: Option<EntityId>,
    text_anchor: Anchor,
    diff_base_anchor: Option<DiffBaseAnchor>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct DiffBaseAnchor {
    source_id: EntityId,
    text_anchor: Anchor,
}

impl MultiBufferAnchor {
    fn excerpt(path: PathKeyIndex, source_id: Option<EntityId>, text_anchor: Anchor) -> Self {
        Self::Excerpt(ExcerptAnchor {
            path,
            source_id,
            text_anchor,
            diff_base_anchor: None,
        })
    }

    /// 空投影中的边界锚点：文首取 Min，其余取 Max。
    fn boundary(offset: MultiBufferOffset) -> Self {
        if offset == MultiBufferOffset::ZERO {
            Self::Min
        } else {
            Self::Max
        }
    }

    /// 按稳定身份比较组合 Anchor，不解析文本坐标。
    ///
    /// 对齐 `ExcerptAnchor::cmp`：先按路径顺序，再按源身份，最后按源内插入身份的稳定文档序。
    /// `Min`/`Max` 边界位于所有源锚点两侧。折叠树据此保持稳定顺序，编辑后无需按解析结果重排。
    pub fn cmp(&self, other: &Self, snapshot: &MultiBufferSnapshot) -> Ordering {
        match (self, other) {
            (Self::Min, Self::Min) | (Self::Max, Self::Max) => Ordering::Equal,
            (Self::Min, _) => Ordering::Less,
            (_, Self::Min) => Ordering::Greater,
            (Self::Max, _) => Ordering::Greater,
            (_, Self::Max) => Ordering::Less,
            (Self::Excerpt(left), Self::Excerpt(right)) => {
                Ord::cmp(left.path_key(snapshot), right.path_key(snapshot))
                    .then_with(|| left.source_id.cmp(&right.source_id))
                    .then_with(|| stable_excerpt_text_cmp(snapshot, left, right))
            }
        }
    }
}

impl ExcerptAnchor {
    fn path_key<'a>(&self, snapshot: &'a MultiBufferSnapshot) -> &'a PathKey {
        &snapshot.path_keys[self.path.get() as usize]
    }
}

/// 一个源文档的去重共享状态：文本、语法与 capture 映射各保存一份，
/// 该源的所有 excerpt 映射只引用 `source_index`，避免同一文件大量搜索片段重复克隆。
#[derive(Clone, Debug)]
struct ExcerptSource {
    /// 源语言 Buffer 实体（更新时按 id 定位）。
    entity: Entity<LanguageBuffer>,
    /// 源文件路径身份；注册或路径变化时算一次，快照构造不再逐源读实体。
    path: PathKey,
    /// 源 Buffer 的稳定身份；无文件路径时参与 PathKey 与显示块分类。
    buffer_id: BufferId,
    text: Snapshot,
    syntax: SyntaxSnapshot,
    /// 派生高亮缓存句柄；随源快照版本变化整体替换。
    highlight_cache: Arc<HighlightCache>,
    /// 源语言解析后的编辑器设置。
    settings: Arc<LanguageSettings>,
    capture_map: Arc<[u32]>,
    is_dirty: bool,
}

fn path_key_for_source(source: &LanguageBuffer, order: PathOrder) -> PathKey {
    PathKey::for_buffer(source.file_path(), source.buffer_id(), order)
}

/// 不可变快照帧中的源状态（不携带实体引用）。
#[derive(Clone, Debug)]
struct ExcerptSourceSnapshot {
    text: Snapshot,
    syntax: SyntaxSnapshot,
    highlight_cache: Arc<HighlightCache>,
    settings: Arc<LanguageSettings>,
    capture_map: Arc<[u32]>,
    is_dirty: bool,
}

/// 输出变换节点携带的 diff hunk 身份与显示元数据。
///
/// 身份绑定 working 源与 hunk 起点的工作区 Anchor，不随组合文档序号、`visible_hunks` 下标或源范围变化。
/// 输出行/字节范围由游标推导；源操作范围是 BufferDiff 快照的只读派生数据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiffTransformHunkInfo {
    working: EntityId,
    /// 完整工作区 Anchor 范围；整文件新增等无源 hunk 的合成节点为 None。
    buffer_range: Option<Range<Anchor>>,
    /// 源文件无 base 时，Git 操作以整文件为单位；hunk 范围仍服务展开身份。
    is_created: bool,
    side: DiffTransformHunkSide,
    kind: DiffHunkKind,
    staging: DiffHunkStaging,
    base_lines: Range<usize>,
    base_byte_start: usize,
    buffer_word_diffs: Vec<Range<Anchor>>,
    base_word_diffs: Vec<Range<usize>>,
    expanded: bool,
}

impl DiffTransformHunkInfo {
    fn hunk_start(&self) -> Option<Anchor> {
        self.buffer_range.as_ref().map(|range| range.start)
    }
}

/// 一个 hunk 在输出变换树中的节点角色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiffTransformHunkSide {
    /// 新侧内容节点；hunk 主范围取本节点内容行。
    Content,
    /// 旧侧删除节点；提供旧侧显示范围。
    Old,
    /// 无旧侧物化的纯删除挂到后继内容节点；主范围取本节点起点。
    BoundaryStart,
}

/// 输入侧 excerpts 树的 item：位置无关的源片段数据。
///
/// 只承载源坐标与内容摘要；
/// 绝对输出坐标由 `DiffTransformSummary` 的 `MultiBufferOffset` 维度在游标上累加得到，因此按路径 splice 不需要重算下游 item。
#[derive(Clone, Debug)]
struct Excerpt {
    path: PathKey,
    /// 路径身份索引；锚点解析通过它取得当前路径排序键。
    path_index: PathKeyIndex,
    display_path: PathKey,
    /// 实体 header / excerpt divider 归属的逻辑 Buffer。
    buffer_id: BufferId,
    /// 片段在源文档中的锚点范围；权威是源 Anchor，裸 TextRange 只是当前解析缓存。
    source_range: ExcerptContext,
    source_start_line: usize,
    /// 片段真实内容的多维摘要（不含分隔用的合成换行）。
    text_summary: MBTextSummary,
    /// 结构分隔换行，与源末尾是否已有换行无关；仅 excerpt 增删或重排时改变。
    adds_newline: bool,
    /// 该片段内真实内容匹配的源范围（搜索高亮用；diff 片段为空）。
    match_ranges: Arc<[TextRange]>,
    /// 指向源表（`ExcerptState::sources` / 快照的 `excerpt_sources`）的索引。
    source_index: usize,
    /// 该片段所属的工作区源实体；纯文本派生快照没有实体源，因此为 `None`。
    source_id: Option<EntityId>,
    editable: bool,
}

/// 输入 excerpt 与输出变换联合解析出的短期源读取区域。
/// 只读视图不进入输入树；hunk 变化不会反向修改逻辑窗口。
#[derive(Clone, Debug)]
struct OutputRegion {
    path: PathKey,
    /// 路径身份索引；锚点解析用它做整数比较。
    path_index: PathKeyIndex,
    display_path: PathKey,
    /// 实体 header / excerpt divider 归属的逻辑 Buffer。
    buffer_id: BufferId,
    /// 片段在源文档中的锚点范围；权威是源 Anchor，裸 TextRange 只是当前解析缓存。
    source_range: ExcerptContext,
    source_start_line: usize,
    /// 片段真实内容的多维摘要（不含分隔用的合成换行）。
    text_summary: MBTextSummary,
    /// 片段末尾是否为分隔补出了一个合成换行；决定该 item 在组合文本中的输出长度。
    adds_newline: bool,
    /// 该片段内真实内容匹配的源范围（搜索高亮用；diff 片段为空）。
    match_ranges: Arc<[TextRange]>,
    /// 指向源表（`ExcerptState::sources` / 快照的 `excerpt_sources`）的索引。
    source_index: usize,
    /// 该片段所属的工作区源实体；纯文本派生快照没有实体源，因此为 `None`。
    source_id: Option<EntityId>,
    editable: bool,
    diff_kind: Option<ExcerptDiffKind>,
}

/// 删除变换拥有的基线读取描述；不具备输入 excerpt 身份或编辑状态。
#[derive(Clone, Debug)]
struct DeletedHunkRegion {
    source_index: usize,
    source_id: EntityId,
    source_range: ExcerptContext,
    source_start_line: usize,
    text_summary: MBTextSummary,
    adds_newline: bool,
}

/// excerpt 在源文档中的锚点范围。
///
/// 权威是成对的源 Anchor；resolved 是它们在锚点版本下解析出的当前坐标缓存，
/// 随锚点一起推进，读取方按 TextRange 使用（Deref）。
/// 源推进后必须经 mapped 用源快照的版本化编辑日志重新解析，不能复用旧裸偏移。
#[derive(Clone, Debug)]
struct ExcerptContext {
    start: Anchor,
    end: Anchor,
}

impl ExcerptContext {
    /// 在指定源快照版本上创建锚点范围；outside 让边界插入纳入范围。
    fn new(snapshot: &Snapshot, range: TextRange, outside: bool) -> Self {
        let (start_affinity, end_affinity) = if outside {
            (Affinity::Before, Affinity::After)
        } else {
            (Affinity::After, Affinity::Before)
        };
        Self {
            start: snapshot.anchor_with_affinity(range.start(), start_affinity),
            end: snapshot.anchor_with_affinity(range.end(), end_affinity),
        }
    }

    /// 输出读取区域的短期切片；持久窗口仍保留原来的插入身份。
    fn slice(&self, range: TextRange) -> Self {
        Self {
            start: Anchor::new(self.version(), range.start()).with_affinity(Affinity::After),
            end: Anchor::new(self.version(), range.end()).with_affinity(Affinity::Before),
        }
    }

    fn version(&self) -> BufferVersion {
        self.start.version()
    }

    fn range(&self) -> TextRange {
        TextRange::new(self.start.offset(), self.end.offset()).expect("锚点范围必须正序")
    }

    fn start(&self) -> ByteOffset {
        self.start.offset()
    }

    fn end(&self) -> ByteOffset {
        self.end.offset()
    }

    fn len(&self) -> usize {
        self.range().len()
    }

    fn is_empty(&self) -> bool {
        self.start.offset() == self.end.offset()
    }

    /// 用目标源快照的不衰减坐标索引把锚点范围推进到当前坐标。
    ///
    /// 起止 affinity 分别决定边界处的插入归属；坐标索引不衰减，不会因编辑日志预算裁剪而失效。
    fn mapped(
        &self,
        snapshot: &Snapshot,
        start_affinity: Affinity,
        end_affinity: Affinity,
    ) -> TextResult<Self> {
        let range = self.range();
        // 位置推进必须用“自锚点版本以来的全部坐标增量”：范围之前的编辑同样会平移它。
        let map = snapshot.position_map_since(self.version())?;
        let mapped_start = map
            .map_old_position_with_affinity(range.start(), start_affinity)
            .value();
        let mapped_end = map
            .map_old_position_with_affinity(range.end(), end_affinity)
            .value();
        let mapped = if mapped_start <= mapped_end {
            TextRange::new(mapped_start, mapped_end)
        } else {
            TextRange::new(mapped_end, mapped_end)
        }
        .expect("映射后的 excerpt 范围必须有序");
        Ok(Self {
            start: snapshot.anchor_with_affinity(mapped.start(), start_affinity),
            end: snapshot.anchor_with_affinity(mapped.end(), end_affinity),
        })
    }
}

impl PartialEq for ExcerptContext {
    fn eq(&self, other: &Self) -> bool {
        self.range() == other.range()
    }
}

impl Eq for ExcerptContext {}

/// 由树维度推导出的组合映射视图：在位置无关 item 之上附加绝对输出坐标。
///
/// 树本身是权威；该视图只在快照/派生读取时短暂存在，不反向写回。
#[derive(Clone, Debug)]
struct ExcerptMapping {
    entry: OutputRegion,
    /// 在组合文档中的顺序位置（由树序推导，不存储在 item 上）。
    excerpt_index: usize,
    output_range: MultiBufferRange,
}

#[derive(Clone, Copy)]
struct ExcerptCoordinates {
    source_index: usize,
    source_range: TextRange,
    source_start_line: usize,
    adds_newline: bool,
}

impl std::ops::Deref for ExcerptMapping {
    type Target = OutputRegion;

    fn deref(&self) -> &OutputRegion {
        &self.entry
    }
}

/// 组合投影树内的一个 diff transform：工作区内容或删除块。
///
/// 只保存输入/输出摘要和变换类型。
/// 源片段的路径、源范围和编辑属性始终从输入侧 `Excerpt` 树读取，不在输出树中复制。
#[derive(Clone, Debug)]
enum DiffTransform {
    BufferContent {
        summary: DiffTransformSummary,
        /// 本输出节点承载的 hunk 身份与显示元数据；普通 excerpt 为空。
        hunks: Vec<DiffTransformHunkInfo>,
    },
    /// 删除块只存在于输出坐标：它不消费输入坐标；
    /// 被删文本由本节点自带的只读描述承载，因此不进入输入 excerpt 树。
    DeletedHunk {
        summary: DiffTransformSummary,
        hunks: Vec<DiffTransformHunkInfo>,
        /// 被删文本的描述：源是 diff 基线，只用于读取文本与显示坐标，不参与输入坐标。
        region: DeletedHunkRegion,
    },
}

/// 源内容与结构分隔换行共同构成的输出摘要。
fn text_with_separator(mut output_text: MBTextSummary, adds_newline: bool) -> MBTextSummary {
    if adds_newline {
        output_text += MBTextSummary::newline();
    }
    output_text
}

impl DiffTransform {
    /// 从输入侧 excerpt 构造内容变换摘要，并接收本节点的 hunk 身份。
    fn from_excerpt(excerpt: &Excerpt, hunks: Vec<DiffTransformHunkInfo>) -> Self {
        let text = excerpt.output_text_summary();
        Self::BufferContent {
            summary: DiffTransformSummary {
                input: text,
                output: text,
                count: 1,
            },
            hunks,
        }
    }

    /// 由删除 hunk 的描述构造输出变换：不消费输入坐标，被删文本随节点承载。
    fn deleted_hunk(region: DeletedHunkRegion, hunks: Vec<DiffTransformHunkInfo>) -> Self {
        let text = text_with_separator(region.text_summary, region.adds_newline);
        Self::DeletedHunk {
            summary: DiffTransformSummary {
                input: MBTextSummary::default(),
                output: text,
                count: 1,
            },
            hunks,
            region,
        }
    }

    /// 本输出节点携带的 hunk 身份与显示元数据。
    fn hunks(&self) -> &[DiffTransformHunkInfo] {
        match self {
            Self::BufferContent { hunks, .. } | Self::DeletedHunk { hunks, .. } => hunks,
        }
    }

    fn transform_summary(&self) -> &DiffTransformSummary {
        match self {
            Self::BufferContent { summary, .. } | Self::DeletedHunk { summary, .. } => summary,
        }
    }

    fn buffer_content(
        excerpt: &Excerpt,
        content: MBTextSummary,
        completes_excerpt: bool,
        hunks: Vec<DiffTransformHunkInfo>,
    ) -> Self {
        let text = text_with_separator(content, completes_excerpt && excerpt.adds_newline);
        Self::BufferContent {
            summary: DiffTransformSummary {
                input: text,
                output: text,
                count: 1,
            },
            hunks,
        }
    }
}

impl OutputRegion {
    fn output_text_summary(&self) -> MBTextSummary {
        text_with_separator(self.text_summary, self.adds_newline)
    }

    /// 在给定输出坐标起点上派生对外片段快照；快照只是树的只读视图，不反向写回。
    fn to_snapshot(&self, at: MappingPosition) -> ExcerptSnapshot {
        let output = self.output_text_summary();
        ExcerptSnapshot {
            path: self.path.clone(),
            display_path: self.display_path.clone(),
            buffer_id: self.buffer_id,
            output_range: MultiBufferRange::new(
                MultiBufferOffset::new(at.bytes),
                MultiBufferOffset::new(at.bytes + output.len),
            )
            .expect("组合片段输出范围必须正序"),
            source_range: self.source_range.range(),
            output_start_line: at.lines,
            output_end_line: at.lines + output.lines,
            source_start_line: self.source_start_line,
            source_index: self.source_index,
            editable: self.editable,
            diff_kind: self.diff_kind,
        }
    }

    /// 在给定游标位置（输出字节 + 输出行起点）上构造派生视图。
    fn to_mapping(&self, at: MappingPosition) -> ExcerptMapping {
        let output = self.output_text_summary();
        ExcerptMapping {
            entry: self.clone(),
            excerpt_index: at.input_item_index,
            output_range: MultiBufferRange::new(
                MultiBufferOffset::new(at.bytes),
                MultiBufferOffset::new(at.bytes + output.len),
            )
            .expect("组合片段输出范围必须正序"),
        }
    }
}

impl Excerpt {
    fn output_text_summary(&self) -> MBTextSummary {
        text_with_separator(self.text_summary, self.adds_newline)
    }

    fn output_region(&self) -> OutputRegion {
        OutputRegion {
            path: self.path.clone(),
            path_index: self.path_index,
            display_path: self.display_path.clone(),
            buffer_id: self.buffer_id,
            source_range: self.source_range.clone(),
            source_start_line: self.source_start_line,
            text_summary: self.text_summary,
            adds_newline: self.adds_newline,
            match_ranges: Arc::clone(&self.match_ranges),
            source_index: self.source_index,
            source_id: self.source_id,
            editable: self.editable,
            diff_kind: None,
        }
    }
}

/// 组合文本的多维长度摘要，对应 MBTextSummary。
///
/// 字节、Unicode scalar、UTF-16 code unit 与逻辑行来自同一份文本；
/// 组合文档据此在 O(log n) 内完成 seek 与坐标换算。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MBTextSummary {
    /// UTF-8 字节数。
    pub len: usize,
    /// Unicode scalar 数。
    pub chars: usize,
    /// UTF-16 code unit 数。
    pub len_utf16: usize,
    /// 逻辑行数（换行符数）。
    pub lines: usize,
    /// 第一行的 Unicode scalar 数。
    pub first_line_chars: usize,
    /// 最后一行的字节列；摘要拼接时不与总长度混用。
    pub last_line_len: usize,
    /// 最后一行的 Unicode scalar 列。
    pub last_line_chars: usize,
    /// 最长行的相对行号；并列时取更早的行。
    pub longest_row: usize,
    /// 最长行的 Unicode scalar 数。
    pub longest_row_chars: usize,
}

impl MBTextSummary {
    const fn newline() -> Self {
        Self {
            len: 1,
            chars: 1,
            len_utf16: 1,
            lines: 1,
            first_line_chars: 0,
            last_line_len: 0,
            last_line_chars: 0,
            longest_row: 0,
            longest_row_chars: 0,
        }
    }
}

impl std::ops::AddAssign for MBTextSummary {
    /// 并接两个摘要；语义与 zcv_text::TextSummary 的 join 完全一致。
    ///
    /// 拼接点两侧的行会合并成候选最长行；行宽维度不可通过减法得到，只能由两端摘要派生。
    fn add_assign(&mut self, other: Self) {
        let joined_chars = self.last_line_chars + other.first_line_chars;
        if joined_chars > self.longest_row_chars {
            self.longest_row = self.lines;
            self.longest_row_chars = joined_chars;
        }
        if other.longest_row_chars > self.longest_row_chars {
            self.longest_row = self.lines + other.longest_row;
            self.longest_row_chars = other.longest_row_chars;
        }
        if self.lines == 0 {
            self.first_line_chars += other.first_line_chars;
        }
        if other.lines == 0 {
            self.last_line_len += other.last_line_len;
            self.last_line_chars += other.last_line_chars;
        } else {
            self.last_line_len = other.last_line_len;
            self.last_line_chars = other.last_line_chars;
        }
        self.len += other.len;
        self.chars += other.chars;
        self.len_utf16 += other.len_utf16;
        self.lines += other.lines;
    }
}

/// 输入（未删除拼接）坐标偏移，对应 `ExcerptOffset`。
///
/// 它度量基础 excerpts 拼接文本中的位置，与输出 `MultiBufferOffset` 是不同坐标空间；
/// 删除 hunk 只存在于输出，不能把两者当作同一个 `usize` 混用。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct ExcerptOffset(usize);

impl ExcerptOffset {
    const fn new(value: usize) -> Self {
        Self(value)
    }

    const fn get(self) -> usize {
        self.0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ExcerptSummary {
    text: MBTextSummary,
    count: usize,
    path_key: PathKey,
    max_anchor: Option<(Option<EntityId>, Anchor)>,
}

impl ContextLessSummary for ExcerptSummary {
    fn zero() -> Self {
        Self::default()
    }

    fn add_summary(&mut self, summary: &Self) {
        self.text += summary.text;
        self.count += summary.count;
        self.path_key = summary.path_key.clone();
        self.max_anchor = summary.max_anchor;
    }
}

/// 输入窗口与输出文本独立分段的变换摘要。
/// 删除变换的输入摘要为空，内容变换的输入、输出摘要相同。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct DiffTransformSummary {
    input: MBTextSummary,
    output: MBTextSummary,
    count: usize,
}

impl ContextLessSummary for DiffTransformSummary {
    fn zero() -> Self {
        Self::default()
    }

    fn add_summary(&mut self, summary: &Self) {
        self.input += summary.input;
        self.output += summary.output;
        self.count += summary.count;
    }
}

impl Item for Excerpt {
    type Summary = ExcerptSummary;

    /// 输入摘要包含源内容和稳定的结构分隔换行；内容变换原样传递完整输入。
    fn summary(&self, _cx: ()) -> Self::Summary {
        ExcerptSummary {
            text: self.output_text_summary(),
            count: 1,
            path_key: self.path.clone(),
            max_anchor: Some((self.source_id, self.source_range.end)),
        }
    }
}

impl Item for DiffTransform {
    type Summary = DiffTransformSummary;

    fn summary(&self, _cx: ()) -> Self::Summary {
        // 用节点自身的内容摘要（内容长度 + 是否补合成换行）描述子树，不依赖其在文档中的绝对输出坐标；
        // 按路径 splice 时下游节点无需重算。
        self.transform_summary().clone()
    }
}

impl Dimension<'_, ExcerptSummary> for PathKey {
    fn zero(_: ()) -> Self {
        Self::min()
    }

    fn add_summary(&mut self, summary: &ExcerptSummary, _: ()) {
        *self = summary.path_key.clone();
    }
}

impl SeekTarget<'_, ExcerptSummary, ExcerptSummary> for PathKey {
    fn cmp(&self, cursor_location: &ExcerptSummary, _: ()) -> Ordering {
        Ord::cmp(self, &cursor_location.path_key)
    }
}

/// 组合文本的字节偏移。
///
/// 与源文档的 ByteOffset 是不同坐标空间：
/// 源偏移由各源快照解释，组合偏移由 MultiBufferSnapshot 解释，二者不得互相直接传递。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MultiBufferOffset(pub usize);

impl MultiBufferOffset {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: usize) -> Self {
        Self(value)
    }

    pub const fn get(self) -> usize {
        self.0
    }

    pub fn checked_add(self, rhs: usize) -> Option<Self> {
        self.0.checked_add(rhs).map(Self)
    }

    pub fn checked_sub(self, rhs: usize) -> Option<Self> {
        self.0.checked_sub(rhs).map(Self)
    }

    pub fn saturating_add(self, rhs: usize) -> Self {
        Self(self.0.saturating_add(rhs))
    }

    pub fn saturating_sub(self, rhs: usize) -> Self {
        Self(self.0.saturating_sub(rhs))
    }
}

impl From<usize> for MultiBufferOffset {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl From<ByteOffset> for MultiBufferOffset {
    fn from(value: ByteOffset) -> Self {
        Self(value.get())
    }
}

impl From<MultiBufferOffset> for ByteOffset {
    fn from(value: MultiBufferOffset) -> Self {
        ByteOffset::new(value.get())
    }
}

impl From<MultiBufferOffset> for usize {
    fn from(value: MultiBufferOffset) -> Self {
        value.get()
    }
}

impl Dimension<'_, DiffTransformSummary> for MultiBufferOffset {
    fn zero(_: ()) -> Self {
        Self(0)
    }

    fn add_summary(&mut self, summary: &DiffTransformSummary, _: ()) {
        self.0 += summary.output.len;
    }
}

/// 组合文本中的半开字节区间。
///
/// 与源文档的 TextRange 是不同坐标空间；两者只能通过显式转换跨越协议边界。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MultiBufferRange {
    start: MultiBufferOffset,
    end: MultiBufferOffset,
}

impl MultiBufferRange {
    pub fn new(
        start: impl Into<MultiBufferOffset>,
        end: impl Into<MultiBufferOffset>,
    ) -> TextResult<Self> {
        let (start, end) = (start.into(), end.into());
        if start > end {
            return Err(CoordinateError::InvalidRange {
                start: ByteOffset::new(start.get()),
                end: ByteOffset::new(end.get()),
            }
            .into());
        }
        Ok(Self { start, end })
    }

    pub const fn start(self) -> MultiBufferOffset {
        self.start
    }

    pub const fn end(self) -> MultiBufferOffset {
        self.end
    }

    pub fn len(self) -> usize {
        self.end.get() - self.start.get()
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn contains(self, point: MultiBufferOffset) -> bool {
        self.start <= point && point < self.end
    }
}

impl From<MultiBufferRange> for TextRange {
    fn from(range: MultiBufferRange) -> Self {
        TextRange::new(
            ByteOffset::new(range.start().get()),
            ByteOffset::new(range.end().get()),
        )
        .expect("组合范围必须正序")
    }
}

impl From<TextRange> for MultiBufferRange {
    fn from(range: TextRange) -> Self {
        Self {
            start: MultiBufferOffset::new(range.start().get()),
            end: MultiBufferOffset::new(range.end().get()),
        }
    }
}

impl From<Range<MultiBufferOffset>> for MultiBufferRange {
    fn from(range: Range<MultiBufferOffset>) -> Self {
        Self {
            start: range.start,
            end: range.end,
        }
    }
}

/// 组合坐标的算术：与裸长度运算只在明确坐标维度内进行。
macro_rules! impl_offset_ops {
    ($name:ident) => {
        impl std::ops::Add<usize> for $name {
            type Output = Self;
            fn add(self, rhs: usize) -> Self {
                Self(self.0 + rhs)
            }
        }
        impl std::ops::Sub<usize> for $name {
            type Output = Self;
            fn sub(self, rhs: usize) -> Self {
                Self(self.0 - rhs)
            }
        }
        impl std::ops::Sub for $name {
            type Output = usize;
            fn sub(self, rhs: Self) -> usize {
                self.0 - rhs.0
            }
        }
        impl std::ops::AddAssign<usize> for $name {
            fn add_assign(&mut self, rhs: usize) {
                self.0 += rhs;
            }
        }
        impl std::ops::SubAssign<usize> for $name {
            fn sub_assign(&mut self, rhs: usize) {
                self.0 -= rhs;
            }
        }
    };
}

impl_offset_ops!(MultiBufferOffset);
impl_offset_ops!(MultiBufferCharOffset);
impl_offset_ops!(MultiBufferOffsetUtf16);

/// 组合输出 Unicode scalar 偏移维度。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct MultiBufferCharOffset(usize);

impl Dimension<'_, DiffTransformSummary> for MultiBufferCharOffset {
    fn zero(_: ()) -> Self {
        Self(0)
    }

    fn add_summary(&mut self, summary: &DiffTransformSummary, _: ()) {
        self.0 += summary.output.chars;
    }
}

/// 组合输出 UTF-16 code unit 偏移维度。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct MultiBufferOffsetUtf16(usize);

impl Dimension<'_, DiffTransformSummary> for MultiBufferOffsetUtf16 {
    fn zero(_: ()) -> Self {
        Self(0)
    }

    fn add_summary(&mut self, summary: &DiffTransformSummary, _: ()) {
        self.0 += summary.output.len_utf16;
    }
}

/// 同时组合输入、输出的文本坐标；
/// 节点序号只用于变换遍历，逻辑身份来自 excerpt 游标。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct MappingPosition {
    bytes: usize,
    chars: usize,
    utf16: usize,
    lines: usize,
    column_bytes: usize,
    column_chars: usize,
    index: usize,
    input_item_index: usize,
    input_offset: ExcerptOffset,
    input_lines: usize,
    input_text: MBTextSummary,
}

impl Dimension<'_, DiffTransformSummary> for MappingPosition {
    fn zero(_: ()) -> Self {
        Self::default()
    }

    fn add_summary(&mut self, summary: &DiffTransformSummary, _: ()) {
        self.bytes += summary.output.len;
        self.chars += summary.output.chars;
        self.utf16 += summary.output.len_utf16;
        if summary.output.lines > 0 {
            self.column_bytes = summary.output.last_line_len;
            self.column_chars = summary.output.last_line_chars;
        } else {
            self.column_bytes += summary.output.last_line_len;
            self.column_chars += summary.output.last_line_chars;
        }
        self.lines += summary.output.lines;
        self.index += summary.count;
        self.input_lines += summary.input.lines;
        self.input_text += summary.input;
        self.input_offset = ExcerptOffset::new(self.input_offset.get() + summary.input.len);
    }
}

impl SeekTarget<'_, DiffTransformSummary, MappingPosition> for MultiBufferOffset {
    fn cmp(&self, cursor_location: &MappingPosition, _: ()) -> Ordering {
        Ord::cmp(&self.0, &cursor_location.bytes)
    }
}

impl SeekTarget<'_, ExcerptSummary, ExcerptSummary> for Position {
    fn cmp(&self, at: &ExcerptSummary, _: ()) -> Ordering {
        Ord::cmp(
            &(self.line().get(), self.column().get()),
            &(at.text.lines, at.text.last_line_chars),
        )
    }
}
impl SeekTarget<'_, ExcerptSummary, ExcerptSummary> for MultiBufferCharOffset {
    fn cmp(&self, at: &ExcerptSummary, _: ()) -> Ordering {
        Ord::cmp(&self.0, &at.text.chars)
    }
}
impl SeekTarget<'_, ExcerptSummary, ExcerptSummary> for MultiBufferOffsetUtf16 {
    fn cmp(&self, at: &ExcerptSummary, _: ()) -> Ordering {
        Ord::cmp(&self.0, &at.text.len_utf16)
    }
}

impl SeekTarget<'_, DiffTransformSummary, MappingPosition> for Position {
    fn cmp(&self, at: &MappingPosition, _: ()) -> Ordering {
        Ord::cmp(
            &(self.line().get(), self.column().get()),
            &(at.lines, at.column_chars),
        )
    }
}

impl SeekTarget<'_, DiffTransformSummary, MappingPosition> for MultiBufferCharOffset {
    fn cmp(&self, cursor_location: &MappingPosition, _: ()) -> Ordering {
        Ord::cmp(&self.0, &cursor_location.chars)
    }
}

impl SeekTarget<'_, DiffTransformSummary, MappingPosition> for MultiBufferOffsetUtf16 {
    fn cmp(&self, cursor_location: &MappingPosition, _: ()) -> Ordering {
        Ord::cmp(&self.0, &cursor_location.utf16)
    }
}

impl SeekTarget<'_, DiffTransformSummary, MappingPosition> for ExcerptOffset {
    fn cmp(&self, cursor_location: &MappingPosition, _: ()) -> Ordering {
        Ord::cmp(&self.get(), &cursor_location.input_offset.get())
    }
}

fn intern_path(
    path_keys: &mut Vec<PathKey>,
    path_key_indices: &mut HashMap<PathKey, PathKeyIndex>,
    path: &PathKey,
) -> PathKeyIndex {
    if let Some(index) = path_key_indices.get(path) {
        return *index;
    }
    let index = PathKeyIndex::new(path_keys.len() as u64);
    path_keys.push(path.clone());
    path_key_indices.insert(path.clone(), index);
    index
}

type ProjectionTrees = (SumTree<Excerpt>, SumTree<DiffTransform>);

/// 比较源文本片段而非物理节点边界：hunk 装饰拆分内容节点不产生文本编辑。
fn matching_projection_text(
    old: &OutputRegion,
    old_offset: usize,
    new: &OutputRegion,
    new_offset: usize,
    reverse: bool,
) -> usize {
    if old.path != new.path || old.buffer_id != new.buffer_id {
        return 0;
    }
    let old_len = old.text_summary.len;
    let new_len = new.text_summary.len;
    let old_newline = if reverse {
        old_offset > old_len
    } else {
        old_offset == old_len
    };
    let new_newline = if reverse {
        new_offset > new_len
    } else {
        new_offset == new_len
    };
    if old_newline || new_newline {
        return usize::from(old_newline && new_newline && old.adds_newline && new.adds_newline);
    }
    if old.source_id != new.source_id
        || old.source_range.start.version() != new.source_range.start.version()
        || old.source_range.start().get() + old_offset
            != new.source_range.start().get() + new_offset
    {
        return 0;
    }
    if reverse {
        old_offset.min(new_offset)
    } else {
        (old_len - old_offset).min(new_len - new_offset)
    }
}

/// 判断一次局部变换重算是否真的改变了组合文本，而不把 hunk 元数据变化当作编辑。
fn projection_text_equal_in_ranges(
    before: &ProjectionTrees,
    after: &ProjectionTrees,
    sources: Sources<'_>,
    old_range: TextRange,
    new_range: TextRange,
) -> bool {
    if old_range.len() != new_range.len() {
        return false;
    }
    let mut old_cursor = MultiBufferCursor::new(&before.0, &before.1, sources);
    let mut new_cursor = MultiBufferCursor::new(&after.0, &after.1, sources);
    old_cursor.seek(old_range.start(), Bias::Right);
    new_cursor.seek(new_range.start(), Bias::Right);
    let mut old_offset = old_range
        .start()
        .get()
        .saturating_sub(old_cursor.start().bytes);
    let mut new_offset = new_range
        .start()
        .get()
        .saturating_sub(new_cursor.start().bytes);
    let mut remaining = old_range.len();
    while remaining > 0 {
        let Some((old, _)) = old_cursor.item_counts() else {
            return false;
        };
        let Some((new, _)) = new_cursor.item_counts() else {
            return false;
        };
        if old_offset == old.text_summary.len + usize::from(old.adds_newline) {
            old_cursor.next();
            old_offset = 0;
            continue;
        }
        if new_offset == new.text_summary.len + usize::from(new.adds_newline) {
            new_cursor.next();
            new_offset = 0;
            continue;
        }
        let matched =
            matching_projection_text(&old, old_offset, &new, new_offset, false).min(remaining);
        if matched == 0 {
            return false;
        }
        remaining -= matched;
        old_offset += matched;
        new_offset += matched;
    }
    true
}

/// 沿变换树比较同版本源片段的公共文本前后缀，不物化全文。
/// 源内部编辑由 TextChangeBatch 投影；结构同步只发布实际插入／移除的文本。
fn projection_changed_ranges(
    before: &ProjectionTrees,
    after: &ProjectionTrees,
    sources: Sources<'_>,
) -> (TextRange, TextRange) {
    let old_len = before.1.summary().output.len;
    let new_len = after.1.summary().output.len;
    let limit = old_len.min(new_len);
    let mut old_cursor = MultiBufferCursor::new(&before.0, &before.1, sources);
    let mut new_cursor = MultiBufferCursor::new(&after.0, &after.1, sources);
    old_cursor.seek_transform_index(0);
    new_cursor.seek_transform_index(0);
    let mut prefix = 0;
    let mut old_offset = 0;
    let mut new_offset = 0;
    while prefix < limit {
        let Some((old, _)) = old_cursor.item_counts() else {
            break;
        };
        let Some((new, _)) = new_cursor.item_counts() else {
            break;
        };
        if old_offset == old.text_summary.len + usize::from(old.adds_newline) {
            old_cursor.next();
            old_offset = 0;
            continue;
        }
        if new_offset == new.text_summary.len + usize::from(new.adds_newline) {
            new_cursor.next();
            new_offset = 0;
            continue;
        }
        let matched =
            matching_projection_text(&old, old_offset, &new, new_offset, false).min(limit - prefix);
        if matched == 0 {
            break;
        }
        prefix += matched;
        old_offset += matched;
        new_offset += matched;
    }

    old_cursor.seek(ByteOffset::new(old_len), Bias::Left);
    new_cursor.seek(ByteOffset::new(new_len), Bias::Left);
    let mut old_remaining = old_cursor.item_counts().map_or(0, |(region, _)| {
        region.text_summary.len + usize::from(region.adds_newline)
    });
    let mut new_remaining = new_cursor.item_counts().map_or(0, |(region, _)| {
        region.text_summary.len + usize::from(region.adds_newline)
    });
    let mut suffix = 0;
    while prefix + suffix < limit {
        while old_remaining == 0 && old_cursor.item_counts().is_some() {
            old_cursor.prev();
            old_remaining = old_cursor.item_counts().map_or(0, |(region, _)| {
                region.text_summary.len + usize::from(region.adds_newline)
            });
        }
        while new_remaining == 0 && new_cursor.item_counts().is_some() {
            new_cursor.prev();
            new_remaining = new_cursor.item_counts().map_or(0, |(region, _)| {
                region.text_summary.len + usize::from(region.adds_newline)
            });
        }
        let Some((old, _)) = old_cursor.item_counts() else {
            break;
        };
        let Some((new, _)) = new_cursor.item_counts() else {
            break;
        };
        let matched = matching_projection_text(&old, old_remaining, &new, new_remaining, true)
            .min(limit - prefix - suffix);
        if matched == 0 {
            break;
        }
        suffix += matched;
        old_remaining -= matched;
        new_remaining -= matched;
    }
    (
        TextRange::new(ByteOffset::new(prefix), ByteOffset::new(old_len - suffix))
            .expect("旧投影文本范围必须正序"),
        TextRange::new(ByteOffset::new(prefix), ByteOffset::new(new_len - suffix))
            .expect("新投影文本范围必须正序"),
    )
}

/// 输出变换节点序号，与逻辑窗口序号分离。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct TransformIndex(usize);

impl SeekTarget<'_, DiffTransformSummary, MappingPosition> for TransformIndex {
    fn cmp(&self, cursor_location: &MappingPosition, _: ()) -> Ordering {
        Ord::cmp(&self.0, &cursor_location.index)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct ExcerptIndex(usize);

impl SeekTarget<'_, ExcerptSummary, ExcerptSummary> for ExcerptIndex {
    fn cmp(&self, cursor_location: &ExcerptSummary, _: ()) -> Ordering {
        Ord::cmp(&self.0, &cursor_location.count)
    }
}

/// 同时维护输入 excerpts 与输出 diff transforms 的双坐标游标。
///
/// 输出游标负责组合文档坐标；
/// 输入游标始终跟随当前 transform 指向其权威源 excerpt。
/// 连续输出定位同时向前推进两棵树，不逐点重新定位源 excerpt。
#[derive(Clone)]
struct MultiBufferCursor<'a> {
    excerpts_tree: &'a SumTree<Excerpt>,
    excerpts: Cursor<'a, 'static, Excerpt, ExcerptSummary>,
    diff_transforms: Cursor<'a, 'static, DiffTransform, MappingPosition>,
    /// 组合偏移落在片段内部时，区间摘要必须回到源快照读取；
    /// 游标持有源视图，而不是从两个前缀摘要相减推导最长行等不可减维度。
    sources: Sources<'a>,
}

impl<'a> MultiBufferCursor<'a> {
    fn new(
        excerpts: &'a SumTree<Excerpt>,
        diff_transforms: &'a SumTree<DiffTransform>,
        sources: Sources<'a>,
    ) -> Self {
        Self {
            excerpts_tree: excerpts,
            excerpts: excerpts.cursor::<ExcerptSummary>(()),
            diff_transforms: diff_transforms.cursor::<MappingPosition>(()),
            sources,
        }
    }

    fn sync_excerpts_at_input(&mut self, input: ExcerptOffset, bias: Bias) {
        self.excerpts.seek(&input, bias);
        if self.excerpts.item().is_none() {
            self.excerpts.prev();
        }
    }

    fn input_offset_at_output(&self, offset: ByteOffset) -> ExcerptOffset {
        let Some(transform) = self.diff_transforms.item() else {
            return self.diff_transforms.start().input_offset;
        };
        let at = self.diff_transforms.start();
        let input = at.input_offset.get()
            + match transform {
                DiffTransform::BufferContent { summary, .. } => {
                    offset.get().saturating_sub(at.bytes).min(summary.input.len)
                }
                DiffTransform::DeletedHunk { .. } => 0,
            };
        ExcerptOffset::new(input)
    }

    fn seek(&mut self, offset: ByteOffset, bias: Bias) {
        self.diff_transforms
            .seek(&MultiBufferOffset(offset.get()), bias);
        self.sync_excerpts_at_input(self.input_offset_at_output(offset), bias);
    }

    fn seek_forward(&mut self, offset: ByteOffset, bias: Bias) {
        self.diff_transforms
            .seek_forward(&MultiBufferOffset(offset.get()), bias);
        let input = self.input_offset_at_output(offset);
        self.excerpts.seek_forward(&input, bias);
        if self.excerpts.item().is_none() {
            self.excerpts.prev();
        }
    }

    fn sync_output_position(&mut self, position: Position, bias: Bias) {
        let Some(transform) = self.diff_transforms.item() else {
            return;
        };
        let at = self.diff_transforms.start();
        if matches!(transform, DiffTransform::DeletedHunk { .. }) {
            self.sync_excerpts_at_input(at.input_offset, Bias::Right);
        } else {
            let rows = position.line().get().saturating_sub(at.lines);
            let column = if rows == 0 {
                at.input_text.last_line_chars
                    + position.column().get().saturating_sub(at.column_chars)
            } else {
                position.column().get()
            };
            let input = Position::new(
                Line::new(at.input_text.lines + rows),
                LogicalColumn::new(column),
            );
            self.excerpts.seek(&input, bias);
            if self.excerpts.item().is_none() {
                self.excerpts.prev();
            }
        }
    }

    fn seek_output_line(&mut self, line: usize, bias: Bias) {
        self.seek_output_position(Position::new(Line::new(line), LogicalColumn::ZERO), bias);
    }

    fn seek_output_position(&mut self, position: Position, bias: Bias) {
        self.diff_transforms.seek(&position, bias);
        self.sync_output_position(position, bias);
    }

    fn seek_output_char(&mut self, offset: CharOffset, bias: Bias) {
        self.diff_transforms
            .seek(&MultiBufferCharOffset(offset.get()), bias);
        if let Some(transform) = self.diff_transforms.item() {
            let at = self.diff_transforms.start();
            let input = if matches!(transform, DiffTransform::BufferContent { .. }) {
                at.input_text.chars + offset.get().saturating_sub(at.chars)
            } else {
                at.input_text.chars
            };
            self.excerpts.seek(&MultiBufferCharOffset(input), bias);
            if self.excerpts.item().is_none() {
                self.excerpts.prev();
            }
        }
    }

    fn seek_output_utf16(&mut self, offset: Utf16Offset, bias: Bias) {
        self.diff_transforms
            .seek(&MultiBufferOffsetUtf16(offset.get()), bias);
        if let Some(transform) = self.diff_transforms.item() {
            let at = self.diff_transforms.start();
            let input = if matches!(transform, DiffTransform::BufferContent { .. }) {
                at.input_text.len_utf16 + offset.get().saturating_sub(at.utf16)
            } else {
                at.input_text.len_utf16
            };
            self.excerpts.seek(&MultiBufferOffsetUtf16(input), bias);
            if self.excerpts.item().is_none() {
                self.excerpts.prev();
            }
        }
    }

    fn seek_path(&mut self, path: &PathKey, bias: Bias) {
        self.excerpts.seek(path, bias);
        let input = ExcerptOffset::new(self.excerpts.start().text.len);
        self.diff_transforms.seek(&input, bias);
        if bias == Bias::Left {
            while let Some(transform) = self.diff_transforms.item() {
                let len = transform.transform_summary().input.len;
                if len == 0 || self.diff_transforms.start().input_offset.get() + len > input.get() {
                    break;
                }
                self.diff_transforms.next();
            }
        }
        self.sync_excerpts_at_input(input, Bias::Right);
    }

    fn seek_transform_index(&mut self, index: usize) {
        self.diff_transforms
            .seek(&TransformIndex(index), Bias::Right);
        if self.diff_transforms.item().is_some() {
            self.sync_excerpts_at_input(self.diff_transforms.start().input_offset, Bias::Right);
        }
    }

    fn next(&mut self) {
        if let (Some(logical), Some(DiffTransform::BufferContent { summary, .. })) =
            (self.excerpts.item(), self.diff_transforms.item())
        {
            let logical_end = self.excerpts.start().text.len + logical.output_text_summary().len;
            let transform_end = self.diff_transforms.start().input_offset.get() + summary.input.len;
            if logical_end < transform_end {
                self.excerpts.next();
                return;
            }
        }
        self.diff_transforms.next();
        if self.diff_transforms.item().is_some() {
            self.sync_excerpts_at_input(self.diff_transforms.start().input_offset, Bias::Right);
        }
    }

    fn prev(&mut self) {
        if matches!(
            self.diff_transforms.item(),
            Some(DiffTransform::BufferContent { .. })
        ) && self.excerpts.start().text.len > self.diff_transforms.start().input_offset.get()
        {
            self.excerpts.prev();
            return;
        }
        self.diff_transforms.prev();
        if let Some(transform) = self.diff_transforms.item() {
            let start = self.diff_transforms.start().input_offset.get();
            let end = start + transform.transform_summary().input.len;
            let input = if end > start { end - 1 } else { start };
            self.sync_excerpts_at_input(ExcerptOffset::new(input), Bias::Right);
        }
    }

    fn item_counts(&self) -> Option<(OutputRegion, &DiffTransform)> {
        let transform = self.diff_transforms.item()?;
        let region = match transform {
            DiffTransform::DeletedHunk {
                region: deleted, ..
            } => {
                let logical = self.excerpts.item()?;
                let mut region = logical.output_region();
                region.source_index = deleted.source_index;
                region.source_id = Some(deleted.source_id);
                region.source_range = deleted.source_range.clone();
                region.source_start_line = deleted.source_start_line;
                region.text_summary = deleted.text_summary;
                region.adds_newline = deleted.adds_newline;
                region.match_ranges = Arc::from([]);
                region.editable = false;
                region.diff_kind = Some(ExcerptDiffKind::Deleted);
                region
            }
            DiffTransform::BufferContent { summary, hunks } => {
                let logical = self.excerpts.item()?;
                let at = self.diff_transforms.start();
                let logical_start = self.excerpts.start().text;
                let mut logical_end = logical_start;
                logical_end += logical.output_text_summary();
                let mut transform_end = at.input_text;
                transform_end += summary.input;
                let start_summary = if logical_start.len > at.input_text.len {
                    logical_start
                } else {
                    at.input_text
                };
                let end_summary = if logical_end.len < transform_end.len {
                    logical_end
                } else {
                    transform_end
                };
                let relative = start_summary.len - logical_start.len;
                let adds_newline = logical.adds_newline && end_summary.len == logical_end.len;
                let content_end = if adds_newline {
                    let mut content_end = logical_start;
                    content_end += logical.text_summary;
                    content_end
                } else {
                    end_summary
                };
                // 计数层只做可减长度维度的前缀相减；最长行等 max 维度在区域摘要层补齐。
                let content = counts_between(start_summary, content_end);
                let start = ByteOffset::new(logical.source_range.start().get() + relative);
                let end = ByteOffset::new(start.get() + content.len);
                let mut region = logical.output_region();
                region.source_range = logical
                    .source_range
                    .slice(TextRange::new(start, end).expect("内容变换必须位于逻辑 excerpt 内"));
                region.source_start_line =
                    logical.source_start_line + start_summary.lines - logical_start.lines;
                region.text_summary = content;
                region.adds_newline = adds_newline;
                if hunks
                    .iter()
                    .any(|hunk| hunk.side == DiffTransformHunkSide::Content)
                {
                    region.diff_kind = Some(ExcerptDiffKind::Added);
                }
                region
            }
        };
        Some((region, transform))
    }

    /// 当前输出区域；行宽维度由区域源范围补齐。
    ///
    /// 可减长度维度来自计数层；最长行等 max 维度不可减，必须回到源摘要读取。
    fn item(&self) -> Option<(OutputRegion, &DiffTransform)> {
        let (mut region, transform) = self.item_counts()?;
        if matches!(transform, DiffTransform::BufferContent { .. }) {
            region.text_summary = self.region_summary(&region);
        }
        Some((region, transform))
    }

    /// 区域源范围的完整摘要；整片段直接取已存摘要，局部区间回到源快照。
    fn region_summary(&self, region: &OutputRegion) -> MBTextSummary {
        if let Some(logical) = self.excerpts.item()
            && region.source_range.start() == logical.source_range.start()
            && region.source_range.end() == logical.source_range.end()
        {
            return logical.text_summary;
        }
        let source = self
            .sources
            .source_text(region.source_index)
            .expect("区域必须引用当前源快照");
        snapshot_range_summary(source, region.source_range.range())
            .expect("区域源范围必须属于源快照")
            .0
    }

    fn start(&self) -> MappingPosition {
        let mut at = self.diff_transforms.start().clone();
        at.input_item_index = self.excerpts.start().count;
        if matches!(
            self.diff_transforms.item(),
            Some(DiffTransform::BufferContent { .. })
        ) && self.excerpts.start().text.len > at.input_text.len
        {
            let prefix = input_range_summary(
                self.excerpts_tree,
                &self.sources,
                at.input_offset,
                ExcerptOffset::new(self.excerpts.start().text.len),
            );
            at.bytes += prefix.len;
            at.chars += prefix.chars;
            at.utf16 += prefix.len_utf16;
            if prefix.lines > 0 {
                at.column_bytes = prefix.last_line_len;
                at.column_chars = prefix.last_line_chars;
            } else {
                at.column_bytes += prefix.last_line_len;
                at.column_chars += prefix.last_line_chars;
            }
            at.lines += prefix.lines;
            at.input_text += prefix;
            at.input_offset = ExcerptOffset::new(at.input_text.len);
            at.input_lines = at.input_text.lines;
        }
        at
    }

    fn mapping(&self) -> Option<ExcerptMapping> {
        let (excerpt, _) = self.item()?;
        Some(excerpt.to_mapping(self.start()))
    }

    fn seek_output_line_forward(&mut self, line: usize, bias: Bias) {
        let position = Position::new(Line::new(line), LogicalColumn::ZERO);
        self.diff_transforms.seek_forward(&position, bias);
        let Some(transform) = self.diff_transforms.item() else {
            return;
        };
        let at = self.diff_transforms.start();
        if matches!(transform, DiffTransform::DeletedHunk { .. }) {
            self.excerpts.seek_forward(&at.input_offset, Bias::Right);
        } else {
            let rows = position.line().get().saturating_sub(at.lines);
            let column = if rows == 0 {
                at.input_text.last_line_chars
                    + position.column().get().saturating_sub(at.column_chars)
            } else {
                position.column().get()
            };
            let input = Position::new(
                Line::new(at.input_text.lines + rows),
                LogicalColumn::new(column),
            );
            self.excerpts.seek_forward(&input, bias);
        }
        if self.excerpts.item().is_none() {
            self.excerpts.prev();
        }
    }
}

/// 按非递减组合字节偏移读取逻辑位置与源锚点，并复用 excerpt 与 diff 变换树游标。
pub struct MultiBufferCoordinateCursor<'a> {
    snapshot: &'a MultiBufferSnapshot,
    cursor: MultiBufferCursor<'a>,
    last_offset: Option<MultiBufferOffset>,
}

impl<'a> MultiBufferCoordinateCursor<'a> {
    pub fn new(snapshot: &'a MultiBufferSnapshot) -> Self {
        Self {
            snapshot,
            cursor: MultiBufferCursor::new(
                &snapshot.excerpts,
                &snapshot.diff_transforms,
                Sources::Snapshot(&snapshot.excerpt_sources),
            ),
            last_offset: None,
        }
    }

    pub fn reset(&mut self) {
        self.cursor = MultiBufferCursor::new(
            &self.snapshot.excerpts,
            &self.snapshot.diff_transforms,
            Sources::Snapshot(&self.snapshot.excerpt_sources),
        );
        self.last_offset = None;
    }

    pub fn byte_to_position(&mut self, offset: MultiBufferOffset) -> TextResult<Position> {
        if self.last_offset.is_some_and(|last| offset <= last) {
            self.reset();
        }
        if self.last_offset.is_some() {
            self.cursor.seek_forward(offset.into(), Bias::Right);
        } else {
            self.cursor.seek(offset.into(), Bias::Right);
        }
        self.last_offset = Some(offset);
        self.snapshot
            .byte_to_position_with_cursor(&mut self.cursor, offset)
    }

    pub fn anchor_at(
        &mut self,
        offset: MultiBufferOffset,
        affinity: Affinity,
    ) -> MultiBufferAnchor {
        if self.last_offset.is_some_and(|last| offset <= last) {
            self.reset();
        }
        if self.last_offset.is_some() {
            self.cursor.seek_forward(offset.into(), Bias::Right);
        } else {
            self.cursor.seek(offset.into(), Bias::Right);
        }
        self.last_offset = Some(offset);
        anchor_at_cursor(
            &self.snapshot.excerpts,
            &self.snapshot.diff_transforms,
            Sources::Snapshot(&self.snapshot.excerpt_sources),
            &mut self.cursor,
            offset.into(),
            affinity,
        )
        .unwrap_or_else(|| MultiBufferAnchor::boundary(offset))
    }
}

/// 按逻辑行顺序推进的组合行游标。
///
/// 显示层逐可见行读取组合行内容时，如果每行都走无状态的
/// line_start_byte/line_content_byte_range，就会对映射树做一次整树 seek；
/// 该 seek 的成本随目标在树中的名次增长，于是滚动越深帧越慢。
/// 本游标一次定位后只向前推进，行内容范围由当前片段与源行直接换算，不再逐行重建映射。
pub struct MultiBufferLineCursor<'a> {
    snapshot: &'a MultiBufferSnapshot,
    cursor: MultiBufferCursor<'a>,
    line: Line,
    at_bytes: usize,
    at_lines: usize,
    output_line_span: usize,
    source_range_start: usize,
    source_range_end: usize,
    source_start_line: usize,
    source: Option<&'a ExcerptSourceSnapshot>,
}

impl<'a> MultiBufferLineCursor<'a> {
    /// 定位到 start 行；之后用 [Self::seek] 向前推进。
    pub fn new(snapshot: &'a MultiBufferSnapshot, start: Line) -> Option<Self> {
        if start.get() >= snapshot.line_count() {
            return None;
        }
        let mut cursor = MultiBufferCursor::new(
            &snapshot.excerpts,
            &snapshot.diff_transforms,
            Sources::Snapshot(&snapshot.excerpt_sources),
        );
        cursor.seek_output_line(start.get(), Bias::Right);
        if cursor.item().is_none() {
            cursor.prev();
        }
        let mut this = Self {
            snapshot,
            cursor,
            line: start,
            at_bytes: 0,
            at_lines: 0,
            output_line_span: 0,
            source_range_start: 0,
            source_range_end: 0,
            source_start_line: 0,
            source: None,
        };
        if !this.refresh() {
            return None;
        }
        Some(this)
    }

    /// 定位到指定行；只支持向前，向后会重建游标。
    pub fn seek(&mut self, target: Line) -> bool {
        if target.get() >= self.snapshot.line_count() {
            return false;
        }
        if target == self.line {
            return true;
        }
        if target.get() < self.line.get() || target.get() < self.at_lines {
            return Self::new(self.snapshot, target).is_some_and(|next| {
                *self = next;
                true
            });
        }
        if target.get() - self.at_lines < self.output_line_span {
            self.line = target;
            return true;
        }
        self.cursor
            .seek_output_line_forward(target.get(), Bias::Right);
        if self.cursor.item().is_none() {
            self.cursor.prev();
        }
        if !self.refresh() {
            return false;
        }
        self.line = target;
        true
    }

    /// 当前行的内容范围：(组合行内容起点, 内容字节长度)，不含行尾换行符。
    ///
    /// 一行可以跨越多个输出变换；变换边界不截断渲染读取范围。
    pub fn line_content_range(&self) -> Option<(usize, usize)> {
        // 没有 excerpt 的组合文档仍有一行；它没有源映射，内容范围为空。
        let Some(source) = self.source else {
            return Some((0, 0));
        };
        let offset = self.line.get().checked_sub(self.at_lines)?;
        let source_line = Line::new(self.source_start_line + offset);
        let Some(content) = source.text.line_content(source_line, None).ok() else {
            return (self.line.get() + 1 == self.snapshot.line_count())
                .then_some((self.snapshot.len_bytes().get(), 0));
        };
        let text_range = content.text_range();
        let start = text_range.start().get().max(self.source_range_start);
        let end = text_range.end().get().min(self.source_range_end);
        let start = start.min(end);
        let output_start = self.at_bytes + start - self.source_range_start;
        let mut output_end = self.at_bytes + end - self.source_range_start;
        let (region, _) = self.cursor.item()?;
        if end == self.source_range_end && !region.adds_newline {
            let mut cursor = self.cursor.clone();
            cursor.next();
            while let Some((region, _)) = cursor.item() {
                let at = cursor.start();
                if at.lines > self.line.get() {
                    break;
                }
                let source = self.snapshot.source_snapshot(region.source_index)?;
                let source_line = Line::new(region.source_start_line + self.line.get() - at.lines);
                let content = source
                    .text
                    .line_content(source_line, None)
                    .ok()?
                    .text_range();
                let end = content.end().min(region.source_range.end());
                output_end = at.bytes + end.get() - region.source_range.start().get();
                if end < region.source_range.end() || region.adds_newline {
                    break;
                }
                cursor.next();
            }
        }
        Some((output_start, output_end - output_start))
    }

    /// 当前组合行所属的 excerpt；显示层按行消费时复用同一个映射游标。
    pub fn excerpt_snapshot(&self) -> Option<ExcerptSnapshot> {
        let (excerpt, _) = self.cursor.item()?;
        Some(excerpt.to_snapshot(self.cursor.start().clone()))
    }

    /// 当前行的差异状态与文本从同一个组合变换节点读取。
    pub fn diff_row_status(&self) -> Option<DiffRowStatus> {
        let (region, transform) = self.cursor.item()?;
        let at = self.cursor.start();
        let info = transform.hunks().iter().find(|info| {
            matches!(
                info.side,
                DiffTransformHunkSide::Content | DiffTransformHunkSide::Old
            )
        })?;
        let (line_start, _) = self.line_content_range()?;
        if line_start >= at.bytes + region.output_text_summary().len {
            return None;
        }
        let kind = match info.side {
            DiffTransformHunkSide::Old => DiffHunkKind::Deleted,
            DiffTransformHunkSide::Content
                if info.kind == DiffHunkKind::Modified && info.expanded =>
            {
                DiffHunkKind::Added
            }
            DiffTransformHunkSide::Content => info.kind,
            DiffTransformHunkSide::BoundaryStart => return None,
        };
        Some(DiffRowStatus {
            kind,
            staging: info.staging,
            expanded: info.expanded,
        })
    }

    /// 当前行内容起点的源映射；复用行游标的位置，不重新定位组合树。
    pub fn source(&self) -> Option<MultiBufferSource<'a>> {
        let (output_start, _) = self.line_content_range()?;
        let mapping = self.cursor.mapping()?;
        let source_offset = ByteOffset::new(
            mapping.source_range.start().get() + output_start - mapping.output_range.start().get(),
        );
        Some(MultiBufferSource {
            snapshot: self.snapshot,
            mapping,
            source_offset,
        })
    }

    /// 当前组合行在所属源片段中的行首缩进；只读取行首到首个非空白字节。
    fn line_indent(&self) -> Option<LineIndent> {
        let source = self.source()?;
        let (_, output_len) = self.line_content_range()?;
        let source_end = source
            .source_offset
            .get()
            .saturating_add(output_len)
            .min(source.mapping.source_range.end().get());
        Some(
            source
                .text()
                .line_indent_in_range(source.source_offset, ByteOffset::new(source_end))
                .expect("行游标的源范围必须有效"),
        )
    }

    fn refresh(&mut self) -> bool {
        let (source_index, at_bytes, at_lines, span, range_start, range_end, source_start_line) = {
            let Some((excerpt, _)) = self.cursor.item() else {
                return self.snapshot.len_bytes() == MultiBufferOffset::ZERO;
            };
            let at = self.cursor.start();
            let range = excerpt.source_range.range();
            (
                excerpt.source_index,
                at.bytes,
                at.lines,
                excerpt.text_summary.lines + usize::from(excerpt.adds_newline),
                range.start().get(),
                range.end().get(),
                excerpt.source_start_line,
            )
        };
        let Some(source) = self.snapshot.source_snapshot(source_index) else {
            return false;
        };
        self.at_bytes = at_bytes;
        self.at_lines = at_lines;
        self.output_line_span = span;
        self.source_range_start = range_start;
        self.source_range_end = range_end;
        self.source_start_line = source_start_line;
        self.source = Some(source);
        true
    }
}

/// 遍历本次输出编辑覆盖的源区域；节点拆分与合并不改变编辑语义。
fn mappings_in_output_range(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    sources: Sources<'_>,
    start: ExcerptMapping,
    end: ByteOffset,
) -> Vec<ExcerptMapping> {
    let mut cursor = MultiBufferCursor::new(excerpts, tree, sources);
    cursor.seek(
        ByteOffset::new(start.output_range.start().get()),
        Bias::Right,
    );
    let mut mappings = Vec::new();
    while cursor.item().is_some() {
        let mapping = cursor.mapping().expect("输出区域必须有对应源映射");
        if !mappings.is_empty() && mapping.output_range.start().get() >= end.get() {
            break;
        }
        let done = mapping.output_range.end().get() >= end.get();
        mappings.push(mapping);
        if done {
            break;
        }
        cursor.next();
    }
    if mappings.is_empty() {
        mappings.push(start);
    }
    mappings
}

/// 用 `entries` 替换输入 excerpts 树上 `path` 区间的全部 item；其余路径的子树原样保留。
///
/// item 不存储绝对输出坐标，因此 splice 不需要触碰下游 item。
/// 读取一个源范围的长度和换行摘要，不构造组合字符串。
fn snapshot_range_is_valid(text: &Snapshot, range: TextRange) -> bool {
    let len = text.len_bytes();
    range.start() <= range.end()
        && range.end() <= len
        && (range.start() == len || text.chunk_at_byte(range.start()).is_ok())
        && (range.end() == len || text.chunk_at_byte(range.end()).is_ok())
}

fn snapshot_range_summary(text: &Snapshot, range: TextRange) -> Option<(MBTextSummary, bool)> {
    if !snapshot_range_is_valid(text, range) {
        return None;
    }
    if range.start() == range.end() {
        return Some((MBTextSummary::default(), false));
    }
    // 宽字段与计数字段统一由源快照的行宽摘要索引给出（O(log n)，只重测首尾不完整行）；
    // 不再分别差分字符、UTF-16 与行数，保证同一范围只有一份语义来源。
    let summary = text.text_summary_for_range(range).ok()?;
    let ends_with_newline = text
        .slice_byte_range(
            ByteOffset::new(range.end().get().saturating_sub(1)),
            range.end(),
        )
        .is_ok_and(|text| text.as_str() == "\n");
    Some((
        MBTextSummary {
            len: summary.len,
            chars: summary.chars,
            len_utf16: summary.len_utf16,
            lines: summary.lines,
            first_line_chars: summary.first_line_chars,
            // 字节列不在行宽摘要索引中；单行范围的长度就是最后一行的字节列。
            last_line_len: if summary.lines == 0 {
                range.len()
            } else {
                text.byte_to_point(range.end()).ok()?.1
            },
            last_line_chars: summary.last_line_chars,
            longest_row: summary.longest_row,
            longest_row_chars: summary.longest_row_chars,
        },
        ends_with_newline,
    ))
}

/// 两个同坐标空间摘要前缀之间的可减计数。
///
/// 只计算字节、Unicode scalar、UTF-16 与行相关维度；
/// 最长行等 max 维度不可减，调用方必须从区域源范围摘要取得，因此这里不填充这些字段。
fn counts_between(start: MBTextSummary, end: MBTextSummary) -> MBTextSummary {
    MBTextSummary {
        len: end.len - start.len,
        chars: end.chars - start.chars,
        len_utf16: end.len_utf16 - start.len_utf16,
        lines: end.lines - start.lines,
        last_line_len: if end.lines == start.lines {
            end.last_line_len - start.last_line_len
        } else {
            end.last_line_len
        },
        last_line_chars: if end.lines == start.lines {
            end.last_line_chars - start.last_line_chars
        } else {
            end.last_line_chars
        },
        ..MBTextSummary::default()
    }
}

/// 输入 excerpts 拼接文本中左闭右开区间的完整摘要。
///
/// 每个 excerpt 的内容区间只测量与查询区间相交的部分，结构分隔换行按边界补入；
/// 端点落在片段内部时回到源快照聚合，因此最长行等 max 维度不会丢失。
fn input_range_summary<S: SourceTexts + ?Sized>(
    excerpts: &SumTree<Excerpt>,
    sources: &S,
    start: ExcerptOffset,
    end: ExcerptOffset,
) -> MBTextSummary {
    let mut summary = MBTextSummary::default();
    if start.get() >= end.get() {
        return summary;
    }
    let mut cursor = excerpts.cursor::<ExcerptSummary>(());
    cursor.seek(&start, Bias::Right);
    if let Some(excerpt) = cursor.item() {
        let excerpt_start = cursor.start().text.len;
        if start.get() > excerpt_start {
            let excerpt_end = excerpt_start + excerpt.output_text_summary().len;
            summary += partial_excerpt_summary(
                excerpt,
                excerpt_start,
                start.get(),
                end.get().min(excerpt_end),
                sources,
            );
            if end.get() <= excerpt_end {
                return summary;
            }
            cursor.next();
        }
    }
    if end.get() == excerpts.summary().text.len {
        summary += cursor.suffix().summary().text;
        return summary;
    }
    let mut end_cursor = excerpts.cursor::<ExcerptSummary>(());
    end_cursor.seek(&end, Bias::Right);
    let end_index = end_cursor.start().count;
    if cursor.start().count < end_index {
        summary += cursor
            .slice(&ExcerptIndex(end_index), Bias::Right)
            .summary()
            .text;
    }
    if let Some(excerpt) = end_cursor.item() {
        let excerpt_start = end_cursor.start().text.len;
        if end.get() > excerpt_start {
            summary +=
                partial_excerpt_summary(excerpt, excerpt_start, excerpt_start, end.get(), sources);
        }
    }
    summary
}

fn partial_excerpt_summary<S: SourceTexts + ?Sized>(
    excerpt: &Excerpt,
    excerpt_start: usize,
    from: usize,
    to: usize,
    sources: &S,
) -> MBTextSummary {
    let mut summary = MBTextSummary::default();
    let content_end = excerpt_start + excerpt.text_summary.len;
    let content_to = to.min(content_end);
    if from < content_to {
        let source = sources
            .source_text(excerpt.source_index)
            .expect("输入范围必须属于源快照");
        let byte_start = excerpt.source_range.start().get() + (from - excerpt_start);
        let byte_end = excerpt.source_range.start().get() + (content_to - excerpt_start);
        summary += snapshot_range_summary(
            source,
            TextRange::new(ByteOffset::new(byte_start), ByteOffset::new(byte_end))
                .expect("输入子范围必须正序"),
        )
        .expect("输入子范围必须属于源快照")
        .0;
    }
    if to > content_end {
        summary += MBTextSummary::newline();
    }
    summary
}

/// 按源重建 capture 映射（源局部 capture index → 组合全局 index）。
fn rebuild_capture_table(sources: &mut [ExcerptSource]) -> Arc<[Arc<str>]> {
    let mut capture_names = Vec::<Arc<str>>::new();
    let mut capture_indices = HashMap::<Arc<str>, u32>::new();
    for source in sources {
        source.capture_map = source
            .syntax
            .capture_names()
            .iter()
            .map(|name| {
                if let Some(index) = capture_indices.get(name) {
                    *index
                } else {
                    let index = capture_names.len() as u32;
                    capture_names.push(Arc::clone(name));
                    capture_indices.insert(Arc::clone(name), index);
                    index
                }
            })
            .collect();
    }
    Arc::from(capture_names)
}

/// 仅为新增源扩展组合 capture 表，保留已有源的 capture 索引。
fn extend_capture_table(
    sources: &mut [ExcerptSource],
    first_new_source: usize,
    capture_names: &mut Vec<Arc<str>>,
) {
    let mut capture_indices = capture_names
        .iter()
        .enumerate()
        .map(|(index, name)| (Arc::clone(name), index as u32))
        .collect::<HashMap<_, _>>();
    for source in &mut sources[first_new_source..] {
        source.capture_map = source
            .syntax
            .capture_names()
            .iter()
            .map(|name| {
                if let Some(index) = capture_indices.get(name) {
                    *index
                } else {
                    let index = capture_names.len() as u32;
                    capture_names.push(Arc::clone(name));
                    capture_indices.insert(Arc::clone(name), index);
                    index
                }
            })
            .collect();
    }
}

/// 单次源编辑换算到组合坐标所需的信息。
struct SourceIncremental {
    batch: TextChangeBatch,
}

/// 源编辑前后逻辑 excerpt 的输入坐标。
struct SourceExcerptRecord {
    input_offset: ExcerptOffset,
    source_range: TextRange,
}

/// 同一源编辑前后配对的 excerpt 坐标记录。
#[derive(Default)]
struct SourceEditRecords {
    old: Vec<SourceExcerptRecord>,
    new: Vec<SourceExcerptRecord>,
}

/// 一个组合同步帧内的投影事务。
///
/// 同步源 Buffer 与 diff transform 时只对外提交一帧快照；
/// 这里累积帧内依次落地的精确投影编辑，由帧结束时一次性发布净增量与对外版本。
/// 结构增删的区间由各结构操作自身的树比较产出并同样进入累积，帧末不再做兜底比较。
struct ProjectionSync {
    /// 帧内依次落地的投影编辑；各段旧坐标基于上一段落地后的状态，按顺序组合。
    changes: TextChangeBatch,
    changed: bool,
    /// 只更新了快照可见的显示元数据，没有组合文本 edit。
    snapshot_changed: bool,
    /// 差异结果已安装；帧提交时再通知显示层，避免消费者读取未完成的投影。
    diff_changed: bool,
    waiting_for_diff_sources: HashSet<EntityId>,
}

/// 多文件文档中一个可见片段的一帧元数据。
///
/// 文本仍通过组合投影供编辑器的折叠、换行和命中测试使用；
/// 路径、源行号、语法与边界保持为一等数据，不能编码进投影文本。
/// Editor 的通用文件标题和片段分隔块只消费本结构。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExcerptSnapshot {
    path: PathKey,
    display_path: PathKey,
    buffer_id: BufferId,
    output_range: MultiBufferRange,
    source_range: TextRange,
    output_start_line: usize,
    output_end_line: usize,
    source_start_line: usize,
    /// 指向快照 `excerpt_sources` 的索引；内部坐标换算用，不在公开 API 暴露。
    source_index: usize,
    editable: bool,
    diff_kind: Option<ExcerptDiffKind>,
}

impl ExcerptSnapshot {
    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    pub fn display_path(&self) -> &Path {
        self.display_path.as_path()
    }

    /// 当前逻辑 excerpt 的稳定 Buffer 身份。
    pub fn buffer_id(&self) -> BufferId {
        self.buffer_id
    }

    pub fn output_range(&self) -> MultiBufferRange {
        self.output_range
    }

    pub fn source_range(&self) -> TextRange {
        self.source_range
    }

    pub fn output_start_line(&self) -> usize {
        self.output_start_line
    }

    pub fn output_end_line(&self) -> usize {
        self.output_end_line
    }

    /// 源文件内的起始行，1 起始（gutter、光标行列与文件标题的显示约定）。
    ///
    /// 内部坐标换算使用 0 起始字段；显示层不得直接读字段。
    pub fn source_start_line(&self) -> usize {
        self.source_start_line + 1
    }

    pub fn diff_kind(&self) -> Option<ExcerptDiffKind> {
        self.diff_kind
    }

    pub fn source_line_for_output_line(&self, output_line: usize) -> Option<usize> {
        (self.diff_kind != Some(ExcerptDiffKind::Deleted) && output_line >= self.output_start_line)
            .then(|| self.source_start_line + 1 + output_line - self.output_start_line)
    }
}

/// 当前组合快照中两个相邻逻辑 excerpt 之间的边界。
///
/// 这对应 `MultiBufferSnapshot::excerpt_boundaries_in_range` 提供给 `BlockMap` 的领域事实。
/// 边界只来自逻辑 excerpt；删除块与内容变换的拆分、合并不会创建边界。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExcerptBoundary {
    previous: Option<ExcerptSnapshot>,
    next: ExcerptSnapshot,
}

impl ExcerptBoundary {
    pub fn previous(&self) -> Option<&ExcerptSnapshot> {
        self.previous.as_ref()
    }

    pub fn next(&self) -> &ExcerptSnapshot {
        &self.next
    }

    /// 边界是否进入了一个新的 Buffer。
    ///
    /// 这是 `ExcerptBoundary::starts_new_buffer` 的同一语义：只比较相邻逻辑 excerpt 的 Buffer 身份，与显示策略无关。
    pub fn starts_new_buffer(&self) -> bool {
        match self.previous.as_ref() {
            None => true,
            Some(previous) => previous.buffer_id() != self.next.buffer_id(),
        }
    }
}

/// 组合文档中的一条大纲条目：符号所属文件身份 + 符号本体。
///
/// 单文件文档只有一个文件身份；组合文档按 `display_path` 区分文件，
/// 供大纲面板构建「目录 → 文件 → 符号」树。
#[derive(Clone, Debug)]
pub struct OutlineEntry {
    /// 组合文档中的显示路径（文件身份）。
    pub display_path: PathBuf,
    /// 输出坐标中的符号。
    pub item: OutlineItem<MultiBufferAnchor>,
}

/// 一帧组合文档的不可变快照。
#[derive(Clone, Debug)]
pub struct MultiBufferSnapshot {
    projection_version: BufferVersion,
    topology_version: u64,
    path_order: PathOrder,
    /// 输入侧 excerpts 的权威快照；源坐标由自身 Summary 派生。
    excerpts: SumTree<Excerpt>,
    /// 由输入 excerpts 派生的输出变换树；输出坐标由累积 Summary 派生。
    diff_transforms: SumTree<DiffTransform>,
    /// 路径索引表：PathKeyIndex 对应的路径，供锚点解析按路径 seek。
    path_keys: Arc<[PathKey]>,
    /// 按源去重的源快照表（映射经 `source_index` 引用）。
    ///
    /// 文本与语法属于同一源快照；
    /// `metadata_version` 只随非文本状态（语法安装、元数据变化）推进，
    /// 纯文本编辑由 `projection_version` 表达；显示层据此替换只读附属数据而不重建显示拓扑。
    excerpt_sources: TreeMap<usize, ExcerptSourceSnapshot>,
    /// 源实体到快照源索引的派生索引，供源级元数据增量直接定位。
    source_indices: Arc<HashMap<EntityId, usize>>,
    file_sources: TreeMap<BufferId, FileSource>,
    dirty_source_count: usize,
    capture_names: Arc<[Arc<str>]>,
    metadata_version: u64,
    /// diff 显示输入与文本／语言元数据分离：它只驱动装饰替换，不触发显示拓扑同步。
    diff_display: Option<Arc<DiffDisplaySnapshot>>,
    /// 单文件组合文档：此语义下不产生任何 excerpt 边界。
    singleton: bool,
    /// 单文件文档的权威工作区源索引；`None` 表示真正的多来源组合文档。
    ///
    /// 单文件大纲必须来自该源的完整语法快照，不能按 excerpt/diff 片段裁剪。
    singleton_source_index: Option<usize>,
}

/// 当前逻辑窗口引用的去重文件源；只在窗口拓扑变化时重建。
#[derive(Clone, Debug)]
struct FileSource {
    path: PathKey,
    source_index: usize,
    editable: bool,
}

/// 组合输出位置关联的源快照与坐标映射。
///
/// `MultiBuffer` 只提供源／组合坐标转换；
/// Tree-sitter 查询由显示层在需要某一行时执行，结果留在语法快照的派生缓存中，不物化进组合快照。
pub struct MultiBufferSource<'a> {
    snapshot: &'a MultiBufferSnapshot,
    mapping: ExcerptMapping,
    source_offset: ByteOffset,
}

impl<'a> MultiBufferSource<'a> {
    pub fn text(&self) -> &'a Snapshot {
        &self
            .snapshot
            .source_snapshot(self.mapping.source_index)
            .expect("源映射必须引用当前快照中的源")
            .text
    }

    pub fn syntax(&self) -> &'a SyntaxSnapshot {
        &self
            .snapshot
            .source_snapshot(self.mapping.source_index)
            .expect("源映射必须引用当前快照中的源")
            .syntax
    }

    /// 当前组合位置在源文本中的字节偏移。
    pub fn source_offset(&self) -> ByteOffset {
        self.source_offset
    }

    /// 将当前 excerpt 内的源点重新投影到组合坐标，供事务回放恢复源内光标。
    pub fn project_point(&self, offset: ByteOffset) -> Option<MultiBufferOffset> {
        let start = self.mapping.source_range.start().get();
        let end = self.mapping.source_range.end().get();
        (start <= offset.get() && offset.get() <= end).then(|| {
            MultiBufferOffset::new(self.mapping.output_range.start().get() + offset.get() - start)
        })
    }

    /// 将连续可见的工作区源范围投影为当前快照的组合坐标范围。
    ///
    /// 范围可以跨同一源的连续 excerpt；
    /// 若中间有未展示的源区间，则不能投影。范围只在本快照内有效；
    /// 需要长期保存的位置由消费方显式创建组合 Anchor。
    pub fn project_range(&self, range: Range<ByteOffset>) -> Option<Range<MultiBufferOffset>> {
        if range.start >= range.end {
            return None;
        }
        source_mapping_range(
            &self.snapshot.excerpts,
            &self.snapshot.diff_transforms,
            Sources::Snapshot(&self.snapshot.excerpt_sources),
            &self.mapping,
            range.start.get(),
            range.end.get(),
        )
    }
}

/// 虚拟组合文本的一段连续借用。
///
/// `text` 永远直接借用某个源 Buffer，或借用 excerpt 间的静态换行边界；
/// 它不来自任何组合文本物化。
/// `output_range` 保留该段在组合坐标中的位置，使后续 DisplayMap 游标无需回退到整份物化快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiBufferChunk<'a> {
    pub text: &'a str,
    pub output_range: Range<MultiBufferOffset>,
}

/// 在 excerpt 映射与源快照之间向前推进的组合文本游标。
///
/// 输入范围必须位于组合快照内；游标只向前移动，跨 excerpt 时不会拷贝或拼接文本。
pub struct MultiBufferBytes<'a> {
    snapshot: &'a MultiBufferSnapshot,
    range: Range<ByteOffset>,
    cursor: MultiBufferCursor<'a>,
    offset: ByteOffset,
}

/// Editor 对 MultiBuffer 虚拟投影的独立订阅。
#[derive(Debug, Clone)]
pub struct MultiBufferSubscription {
    state: Arc<Mutex<ProjectionSubscriptionState>>,
}

impl MultiBufferSubscription {
    pub fn consume(&self) -> TextChangeBatch {
        let mut state = self
            .state
            .lock()
            .expect("组合投影订阅锁不应在持锁期间 panic");
        let Some(_) = state.pending_old_version.take() else {
            return TextChangeBatch::default();
        };
        state
            .pending_batch
            .take()
            .expect("存在待消费投影版本时必须同时存在连续的输出编辑批次")
    }
}

#[derive(Debug, Clone)]
struct ProjectionSubscriptionState {
    current_version: BufferVersion,
    pending_old_version: Option<BufferVersion>,
    /// 单次源编辑换算到组合坐标后的增量批次。
    ///
    /// 订阅协议始终保留可组合的输出编辑；
    /// 版本连续性是本层不变量，不能以缺失批次退化为无坐标的整体替换。
    pending_batch: Option<TextChangeBatch>,
}

#[derive(Default)]
struct ProjectionChangeTopic {
    subscriptions: Mutex<Vec<Weak<Mutex<ProjectionSubscriptionState>>>>,
}

impl ProjectionChangeTopic {
    fn subscribe(&self, version: BufferVersion) -> MultiBufferSubscription {
        let state = Arc::new(Mutex::new(ProjectionSubscriptionState {
            current_version: version,
            pending_old_version: None,
            pending_batch: None,
        }));
        self.subscriptions
            .lock()
            .expect("组合投影订阅锁不应在持锁期间 panic")
            .push(Arc::downgrade(&state));
        MultiBufferSubscription { state }
    }

    fn publish(
        &self,
        old_version: BufferVersion,
        new_version: BufferVersion,
        batch: TextChangeBatch,
    ) {
        self.subscriptions
            .lock()
            .expect("组合投影订阅锁不应在持锁期间 panic")
            .retain(|subscription| {
                let Some(subscription) = subscription.upgrade() else {
                    return false;
                };
                let mut state = subscription
                    .lock()
                    .expect("组合投影订阅锁不应在持锁期间 panic");
                if state.pending_old_version.is_none() {
                    // 首个待消费变化：记录净变化起点并保留其增量批次。
                    state.pending_old_version = Some(old_version);
                    state.pending_batch = Some(batch.clone());
                } else {
                    // 同一读取前的多次变化必须组合为一段连续增量。
                    // 投影版本由本层依次发布，版本不连续说明内部发布协议被破坏。
                    state.pending_batch = state
                        .pending_batch
                        .as_ref()
                        .and_then(|pending| pending.compose(&batch))
                        .or_else(|| {
                            panic!("组合投影订阅收到了不连续的版本批次");
                        });
                }
                state.current_version = new_version;
                true
            });
    }
}

#[derive(Clone, Debug)]
pub struct MultiBufferHistoryOutcome {
    transaction_id: TransactionId,
    position_map: PositionMap,
    old_version: BufferVersion,
    new_version: BufferVersion,
}

impl MultiBufferHistoryOutcome {
    pub fn transaction_id(&self) -> TransactionId {
        self.transaction_id
    }

    pub fn position_map(&self) -> &PositionMap {
        &self.position_map
    }

    pub fn old_version(&self) -> BufferVersion {
        self.old_version
    }

    pub fn new_version(&self) -> BufferVersion {
        self.new_version
    }
}

struct CompositeHistoryEntry {
    id: TransactionId,
    buffers: Vec<(Entity<LanguageBuffer>, TransactionId)>,
}

/// MultiBuffer 拥有的源文本增量游标。
///
/// GPUI 事件只负责唤醒；
/// 连续版本和 Patch 必须由该独立订阅拉取，避免把延迟派发的旧事件当成当前坐标事实。
struct SourceSubscription {
    source: Entity<LanguageBuffer>,
    /// Deleted 旧侧的文本由 BufferDiff 推进，组合文档只跟踪其语法元数据。
    text: Option<TextSubscription>,
    /// 文本与语法事件共用源生命周期，但按各自所有权处理。
    _event: Subscription,
}

/// MultiBuffer 对消费方公开的文本与语法更新边界。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MultiBufferEvent {
    TextChanged,
    /// excerpts / 组合变换拓扑发生变化，消费方需要收敛源集合。
    ProjectionChanged,
    /// 只在文尾追加 excerpts；已有组合坐标和锚点范围保持不变。
    ProjectionAppended,
    Reparsed(EntityId),
    MetadataChanged,
    /// 差异状态或显示输入变化；组合文本可能保持不变。
    DiffChanged,
    /// diff 展开/折叠状态变化；输出变换已同步，逻辑窗口保持不变。
    DiffExpansionChanged,
}

/// `MultiBufferSnapshot` 是连续文本视图，而不是由临时 `Buffer` 复制出的影子文本。
///
/// 对外实现 `zcv_text::TextRead` 后，搜索等跨文本算法直接消费 excerpt 游标；
/// 文本所有权仍然只存在于各源 Buffer 中。
impl TextRead for MultiBufferSnapshot {
    fn slice_text(&self, range: TextRange) -> TextResult<Cow<'_, str>> {
        Ok(Cow::Owned(self.text_for_range(range.into())?))
    }

    fn chunks(&self, range: TextRange) -> TextResult<impl Iterator<Item = &str> + '_> {
        self.ensure_output_boundary(range.start().into())?;
        self.ensure_output_boundary(range.end().into())?;
        Ok(self
            .bytes_in_range(range.start().into()..range.end().into())
            .map(|chunk| chunk.text))
    }

    fn len_bytes(&self) -> ByteOffset {
        self.len_bytes().into()
    }

    fn len_chars(&self) -> CharOffset {
        self.byte_to_char(self.len_bytes())
            .expect("组合文本末端必须是有效字符边界")
    }

    fn line_count(&self) -> usize {
        self.line_count()
    }

    fn line_start(&self, line: Line) -> TextResult<ByteOffset> {
        self.line_start_byte(line).map(Into::into)
    }

    fn byte_to_position(&self, offset: ByteOffset) -> TextResult<Position> {
        self.byte_to_position(offset.into())
    }

    fn byte_to_line(&self, offset: ByteOffset) -> TextResult<Line> {
        self.byte_to_line(offset.into())
    }

    fn position_to_byte(&self, position: Position) -> TextResult<ByteOffset> {
        self.position_to_byte(position).map(Into::into)
    }

    fn char_to_position(&self, offset: CharOffset) -> TextResult<Position> {
        self.byte_to_position(self.char_to_byte(offset)?)
    }

    fn position_to_char(&self, position: Position) -> TextResult<CharOffset> {
        self.byte_to_char(self.position_to_byte(position)?)
    }

    fn char_at(&self, offset: CharOffset) -> Option<char> {
        self.char_to_byte(offset)
            .ok()
            .and_then(|offset| self.char_at_byte(offset.into()))
    }

    fn char_at_byte(&self, offset: ByteOffset) -> Option<char> {
        (offset < self.len_bytes().into())
            .then(|| self.chunk_at_byte(offset.into()).ok())
            .flatten()
            .and_then(|(chunk, start)| chunk[offset.get() - start.get()..].chars().next())
    }

    fn byte_to_char(&self, offset: ByteOffset) -> TextResult<CharOffset> {
        self.byte_to_char(offset.into())
    }

    fn char_to_byte(&self, offset: CharOffset) -> TextResult<ByteOffset> {
        self.char_to_byte(offset).map(Into::into)
    }

    fn byte_to_utf16_cu(&self, offset: ByteOffset) -> TextResult<Utf16Offset> {
        self.byte_to_utf16_cu(offset.into())
    }

    fn utf16_cu_to_byte(&self, offset: Utf16Offset) -> TextResult<ByteOffset> {
        self.utf16_cu_to_byte(offset).map(Into::into)
    }

    fn is_grapheme_boundary(&self, offset: ByteOffset) -> TextResult<bool> {
        self.ensure_output_boundary(offset.into())?;
        if offset == ByteOffset::ZERO || offset == self.len_bytes().into() {
            return Ok(true);
        }
        Ok(self
            .bytes_in_range(ByteOffset::ZERO.into()..self.len_bytes())
            .flat_map(|chunk| {
                chunk
                    .text
                    .grapheme_indices(true)
                    .map(move |(index, _)| ByteOffset::new(chunk.output_range.start.get() + index))
            })
            .any(|boundary| boundary == offset))
    }

    fn previous_grapheme_boundary(&self, offset: ByteOffset) -> TextResult<ByteOffset> {
        self.ensure_output_boundary(offset.into())?;
        Ok(self
            .bytes_in_range(ByteOffset::ZERO.into()..offset.into())
            .flat_map(|chunk| {
                chunk
                    .text
                    .grapheme_indices(true)
                    .map(move |(index, _)| ByteOffset::new(chunk.output_range.start.get() + index))
            })
            .last()
            .unwrap_or(ByteOffset::ZERO))
    }

    fn next_grapheme_boundary(&self, offset: ByteOffset) -> TextResult<ByteOffset> {
        self.ensure_output_boundary(offset.into())?;
        let mut saw_current = false;
        for chunk in self.bytes_in_range(offset.into()..self.len_bytes()) {
            for (index, _) in chunk.text.grapheme_indices(true) {
                let boundary = ByteOffset::new(chunk.output_range.start.get() + index);
                if boundary > offset || saw_current {
                    return Ok(boundary);
                }
                saw_current = true;
            }
        }
        Ok(self.len_bytes().into())
    }

    fn line_ending_style(&self) -> LineEndingStyle {
        let mut saw_lf = false;
        let mut saw_crlf = false;
        let mut saw_lone_cr = false;
        let mut previous_was_cr = false;
        for chunk in self.bytes_in_range(ByteOffset::ZERO.into()..self.len_bytes()) {
            for byte in chunk.text.bytes() {
                match byte {
                    b'\n' if previous_was_cr => saw_crlf = true,
                    b'\n' => saw_lf = true,
                    b'\r' => {
                        if previous_was_cr {
                            saw_lone_cr = true;
                        }
                    }
                    _ if previous_was_cr => saw_lone_cr = true,
                    _ => {}
                }
                previous_was_cr = byte == b'\r';
            }
        }
        if previous_was_cr {
            saw_lone_cr = true;
        }
        match (saw_lf, saw_crlf, saw_lone_cr) {
            (false, false, false) => LineEndingStyle::None,
            (true, false, false) => LineEndingStyle::Lf,
            (false, true, false) => LineEndingStyle::Crlf,
            _ => LineEndingStyle::Mixed,
        }
    }
}

impl<'a> Iterator for MultiBufferBytes<'a> {
    type Item = MultiBufferChunk<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.offset < self.range.end {
            let (entry, _) = self.cursor.item()?;
            let at = self.cursor.start().clone();
            let separator = entry.adds_newline as usize;
            let output_start = ByteOffset::new(at.bytes);
            let output_end = ByteOffset::new(at.bytes + entry.text_summary.len + separator);
            if self.offset >= output_end {
                self.cursor.next();
                continue;
            }

            let source_output_end = ByteOffset::new(output_start.get() + entry.source_range.len());

            if self.offset >= source_output_end {
                // 非末尾 excerpt 为保持组合行边界而补出的换行不属于任何源文本。
                let end = self.range.end.min(output_end);
                if end <= self.offset {
                    self.cursor.next();
                    continue;
                }
                let start = self.offset;
                self.offset = end;
                return Some(MultiBufferChunk {
                    text: "\n",
                    output_range: start.into()..end.into(),
                });
            }

            let source = self.snapshot.source_snapshot(entry.source_index)?;
            let source_offset = ByteOffset::new(
                entry.source_range.start().get() + self.offset.get() - output_start.get(),
            );
            let (chunk, chunk_start) = source.text.chunk_at_byte(source_offset).ok()?;
            let start = source_offset.get() - chunk_start.get();
            let source_end = entry.source_range.end().get();
            let end = (source_end - chunk_start.get())
                .min(chunk.len())
                .min(start + (self.range.end.get() - self.offset.get()));
            if start >= end || !chunk.is_char_boundary(start) || !chunk.is_char_boundary(end) {
                return None;
            }
            let output_chunk_start = self.offset;
            self.offset = ByteOffset::new(self.offset.get() + end - start);
            return Some(MultiBufferChunk {
                text: &chunk[start..end],
                output_range: output_chunk_start.into()..self.offset.into(),
            });
        }
        None
    }
}

fn project_outline_item(
    item: &OutlineItem,
    source_range: TextRange,
    output_range: MultiBufferRange,
    source_text: &Snapshot,
) -> Option<OutlineItem<usize>> {
    let project = |range: &Range<Anchor>| {
        let range = resolve_outline_range(range, source_text)?;
        (source_range.start().get() <= range.start && range.end <= source_range.end().get()).then(
            || {
                let source_start = source_range.start().get();
                let output_start = output_range.start().get();
                (output_start + range.start - source_start)
                    ..(output_start + range.end - source_start)
            },
        )
    };
    Some(OutlineItem {
        range: project(&item.range)?,
        name_range: project(&item.name_range)?,
        source_range_for_text: project(&item.source_range_for_text)?,
        name: item.name.clone(),
        text: item.text.clone(),
        highlight_ranges: item.highlight_ranges.clone(),
        name_ranges: item.name_ranges.clone(),
        kind: item.kind.clone(),
        depth: item.depth,
        language: item.language,
        language_depth: item.language_depth,
        body_range: item.body_range.as_ref().and_then(project),
        annotation_range: item.annotation_range.as_ref().and_then(project),
    })
}

fn resolve_outline_range(range: &Range<Anchor>, source_text: &Snapshot) -> Option<Range<usize>> {
    Some(
        range.start.resolve_in(source_text).ok()?.get()
            ..range.end.resolve_in(source_text).ok()?.get(),
    )
}

fn project_range_contained(
    segments: &[(TextRange, MultiBufferRange)],
    range: &Range<usize>,
) -> Option<Range<usize>> {
    segments.iter().find_map(|(source, output)| {
        let source_start = source.start().get();
        let source_end = source.end().get();
        (source_start <= range.start && range.end <= source_end).then(|| {
            let output_start = output.start().get();
            (output_start + range.start - source_start)..(output_start + range.end - source_start)
        })
    })
}

fn project_range_spanning(
    segments: &[(TextRange, MultiBufferRange)],
    range: &Range<usize>,
) -> Option<Range<usize>> {
    let start = map_source_offset(segments, range.start, false)?;
    let end = map_source_offset(segments, range.end, true)?;
    (start <= end).then_some(start..end)
}

fn map_source_offset(
    segments: &[(TextRange, MultiBufferRange)],
    offset: usize,
    include_segment_end: bool,
) -> Option<usize> {
    for (source, output) in segments {
        let source_start = source.start().get();
        let source_end = source.end().get();
        if offset >= source_start
            && (offset < source_end || (include_segment_end && offset == source_end))
        {
            return Some(output.start().get() + (offset - source_start));
        }
    }
    None
}

fn project_syntax_node(
    node: &SyntaxNode,
    source_range: TextRange,
    output_range: MultiBufferRange,
) -> Option<SyntaxNode> {
    Some(SyntaxNode {
        version: node.version,
        range: project_range(node.range.clone(), source_range, output_range)?,
        kind: node.kind.clone(),
        language: node.language,
        language_depth: node.language_depth,
        is_named: node.is_named,
        is_error: node.is_error,
        is_missing: node.is_missing,
    })
}

fn project_range(
    range: Range<usize>,
    source_range: TextRange,
    output_range: MultiBufferRange,
) -> Option<Range<usize>> {
    let source_start = source_range.start().get();
    (source_start <= range.start && range.end <= source_range.end().get()).then(|| {
        let output_start = output_range.start().get();
        (output_start + range.start - source_start)..(output_start + range.end - source_start)
    })
}

/// 纯文本派生快照：把整段文本表示为一个不可编辑的单 excerpt，语法为空表。
///
/// placeholder 等独立文本因此与真实组合文档共用同一套 excerpt 游标语义，
/// 不再维护第二份纯文本坐标实现。
impl From<Snapshot> for MultiBufferSnapshot {
    fn from(text: Snapshot) -> Self {
        let syntax = SyntaxSnapshot::empty(text.version());
        let capture_names = syntax.capture_names();
        let range =
            TextRange::new(ByteOffset::ZERO, text.len_bytes()).expect("纯文本快照范围必须有效");
        let (text_summary, _) =
            snapshot_range_summary(&text, range).expect("纯文本快照范围必须有效");
        let path = PathKey::min();
        let excerpt = Excerpt {
            path: path.clone(),
            path_index: PathKeyIndex::new(0),
            display_path: path,
            buffer_id: BufferId::new(0),
            source_range: ExcerptContext::new(&text, range, false),
            source_start_line: 0,
            text_summary,
            adds_newline: false,
            match_ranges: Arc::from([]),
            source_index: 0,
            source_id: None,
            editable: true,
        };
        let diff_transforms =
            SumTree::from_iter([DiffTransform::from_excerpt(&excerpt, Vec::new())], ());
        Self {
            projection_version: text.version(),
            topology_version: 0,
            path_order: PathOrder::Natural,
            excerpts: SumTree::from_iter([excerpt], ()),
            diff_transforms,
            path_keys: Arc::from([PathKey::min()]),
            excerpt_sources: TreeMap::from_ordered_entries([(
                0,
                ExcerptSourceSnapshot {
                    text,
                    syntax,
                    highlight_cache: Arc::new(HighlightCache::new()),
                    settings: Arc::new(LanguageSettings::default()),
                    capture_map: Arc::from([]),
                    is_dirty: false,
                },
            )]),
            source_indices: Arc::new(HashMap::new()),
            file_sources: TreeMap::default(),
            dirty_source_count: 0,
            capture_names,
            metadata_version: 0,
            diff_display: None,
            singleton: true,
            singleton_source_index: Some(0),
        }
    }
}

impl MultiBufferSnapshot {
    fn empty() -> Self {
        let text = Buffer::from_text(String::new(), BufferConfig::default())
            .expect("空组合快照的临时文本必须可以创建")
            .snapshot();
        let mut snapshot = Self::from(text);
        snapshot.excerpts = SumTree::new(());
        snapshot.diff_transforms = SumTree::new(());
        snapshot.path_keys = Arc::from([]);
        snapshot.excerpt_sources = TreeMap::default();
        snapshot.singleton = false;
        snapshot.singleton_source_index = None;
        snapshot
    }
}

/// 取源快照按语言解析后的设置。
fn snapshot_settings(snapshot: &LanguageBufferSnapshot) -> Arc<LanguageSettings> {
    Arc::clone(&snapshot.settings)
}

struct ExcerptState {
    source_subscriptions: Vec<SourceSubscription>,
    /// 输入侧 excerpts 的唯一权威树。
    excerpts: SumTree<Excerpt>,
    /// 从输入 excerpts 派生的输出变换树。
    diff_transforms: SumTree<DiffTransform>,
    /// 按源去重的 (text, syntax, capture_map) 表。
    sources: Vec<ExcerptSource>,
    /// 源实体到 `sources` 索引的派生索引，供增量追加按身份查找源状态。
    source_indices: HashMap<EntityId, usize>,
    /// 路径身份表：索引一经分配不再变化，锚点用索引做紧凑表示。
    path_keys: Vec<PathKey>,
    path_key_indices: HashMap<PathKey, PathKeyIndex>,
    capture_names: Arc<[Arc<str>]>,
    projection_version: BufferVersion,
    topology_version: u64,
    /// 非文本状态（语法安装、捕获表等）版本；纯文本编辑不推进它。
    metadata_epoch: u64,
    /// 源事件只登记脏源；组合快照读取时才消费这些源的订阅。
    pending_source_syncs: HashMap<EntityId, PendingSourceSync>,
    projection_changes: ProjectionChangeTopic,
    next_transaction_id: TransactionId,
    active_transaction: Option<TransactionId>,
    active_source_transactions: Vec<Entity<LanguageBuffer>>,
    undo_stack: Vec<CompositeHistoryEntry>,
    redo_stack: Vec<CompositeHistoryEntry>,
}

#[derive(Clone, Copy, Default)]
struct PendingSourceSync {
    refresh_metadata: bool,
}

/// 组合文档的写入能力。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Capability {
    /// 只读：拒绝组合编辑，仅用于不可直接编辑的数据投影（索引、暂存 diff 视图等）。
    ReadOnly,
    /// 可读写。
    ReadWrite,
}

impl Capability {
    pub(crate) fn is_read_only(self) -> bool {
        matches!(self, Capability::ReadOnly)
    }
}

/// Editor 持有的组合文档模型。
///
/// 恒为 excerpts 形态；普通编辑器是整文件单 excerpt，与多文件文档共用同一套显示、编辑与历史链路。
pub struct MultiBuffer {
    state: ExcerptState,
    capability: Capability,
    path_order: PathOrder,
    /// 普通整文件文档的稳定角色与权威源。
    ///
    /// excerpts 会因 diff 展开而改变形状，不能据此推断文档角色；
    /// 历史、配置、重命名与保存等文件级事实按本字段委托给底层 LanguageBuffer。
    /// `None` 表示真正的多来源组合文档。
    singleton_source: Option<Entity<LanguageBuffer>>,
    /// 按显示路径排序的每文件 diff 状态（diff 实体、显示配置与展开覆盖）。
    diffs: Vec<diff_projection::DiffState>,
    /// git 行级 diff 显示输入（hunks、跟踪区间与显示坐标）；`None` = 无 diff 需求。
    /// 每次真实几何变化以写时复制替换，已发布快照继续持有旧输入。
    diff: Option<Arc<DiffDisplaySnapshot>>,
    /// 新 hunk 的初始展开策略；只决定初始状态，不覆盖用户显式切换。
    diff_expanded_by_default: bool,
    /// 当前一致的组合快照；只由 MultiBuffer 的同步入口替换。
    snapshot: MultiBufferSnapshot,
    /// 当前快照是否需要从可变组合状态重新对齐。
    snapshot_dirty: bool,
    /// `None` 表示源集合拓扑变化，需要重建源表；`Some` 只记录待替换的源索引。
    /// 源表仍会与 excerpts、diff transforms 在同一快照帧提交。
    snapshot_source_updates: Option<HashSet<usize>>,
    /// 当前是否处于源与 diff 的同一同步帧；存在时不提前发布投影版本。
    projection_sync: Option<ProjectionSync>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HistoryOwner {
    /// 单文件编辑器的多个视图共享同一个工作区源，因此历史归源 Buffer 所有。
    SourceBuffer,
    /// 组合文档同时编辑多个源，由 MultiBuffer 记录一次复合事务。
    MultiBuffer,
}

impl EventEmitter<MultiBufferEvent> for MultiBuffer {}

/// 按真实 source 内容定位组合偏移。
///
/// 非末尾 excerpt 为分隔而补出的换行不属于任何 source；位于该换行上的光标按编辑语义落到后继 excerpt。
/// 用累积输出字节游标定位组合偏移所属的映射（O(log n)）。
///
/// 语义与旧的线性扫描一致：命中映射内容的偏移；空片段命中其起点；
/// 合成换行区域的偏移落到下一个映射；文档末尾命中最后一个映射。
fn mapping_at_tree(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    sources: Sources<'_>,
    offset: ByteOffset,
) -> Option<(ExcerptMapping, MappingPosition)> {
    let mut cursor = MultiBufferCursor::new(excerpts, tree, sources);
    cursor.seek(offset, Bias::Right);
    mapping_at_cursor(excerpts, tree, sources, &mut cursor, offset)
}

fn mapping_at_cursor(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    sources: Sources<'_>,
    cursor: &mut MultiBufferCursor<'_>,
    offset: ByteOffset,
) -> Option<(ExcerptMapping, MappingPosition)> {
    if offset.get() == tree.summary().output.len {
        let logical = excerpts.last()?;
        let mut cursor = excerpts.cursor::<ExcerptSummary>(());
        cursor.seek(&ExcerptIndex(excerpts.summary().count - 1), Bias::Right);
        let output = tree.summary().output;
        let at = MappingPosition {
            bytes: output.len,
            chars: output.chars,
            utf16: output.len_utf16,
            lines: output.lines,
            column_bytes: output.last_line_len,
            column_chars: output.last_line_chars,
            input_item_index: cursor.start().count,
            input_text: excerpts.summary().text,
            input_offset: ExcerptOffset::new(excerpts.summary().text.len),
            ..MappingPosition::default()
        };
        let mut region = logical.output_region();
        let end = logical.source_range.end();
        region.source_range = logical
            .source_range
            .slice(TextRange::new(end, end).expect("源末端范围必须有效"));
        region.source_start_line += logical.text_summary.lines;
        region.text_summary = MBTextSummary::default();
        region.adds_newline = false;
        return Some((region.to_mapping(at.clone()), at));
    }
    if cursor.item().is_none() {
        // 偏移在文档末尾（或之后）：命中最后一个映射。
        let mut last = MultiBufferCursor::new(excerpts, tree, sources);
        // Right bias 保留末尾零长度 excerpt；它们仍然拥有自己的文件身份和边界行。
        last.seek(ByteOffset::new(tree.summary().output.len), Bias::Right);
        if last.item().is_none() {
            last.prev();
        }
        let at = last.start().clone();
        return last
            .item()
            .and_then(|_| last.mapping().map(|mapping| (mapping, at)));
    }
    let (mapping, content_end, is_empty, at) = {
        let at = cursor.start().clone();
        let mapping = cursor.mapping()?;
        let content_end = ByteOffset::new(at.bytes + mapping.source_range.len());
        let is_empty = mapping.source_range.is_empty();
        (mapping, content_end, is_empty, at)
    };
    if offset < content_end || (is_empty && offset == ByteOffset::new(at.bytes)) {
        return Some((mapping, at));
    }
    // 落在合成换行区域：优先落到下一个映射，没有下一个则命中最后一个。
    cursor.next();
    if cursor.item().is_some() {
        let at = cursor.start().clone();
        return cursor.mapping().map(|mapping| (mapping, at));
    }
    let mut last = MultiBufferCursor::new(excerpts, tree, sources);
    last.seek(ByteOffset::new(tree.summary().output.len), Bias::Right);
    if last.item().is_none() {
        last.prev();
    }
    let at = last.start().clone();
    last.item()
        .and_then(|_| last.mapping().map(|mapping| (mapping, at)))
}

/// 按逻辑行定位所在片段（O(log n)）。
///
/// 与按输出字节定位同构：用累积输出行游标命中第一个跨越目标行的片段；
/// 目标行恰为最后一条内容行时（其行区间终点等于目标行），退回最后一个片段。
fn mapping_at_output_line(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    sources: Sources<'_>,
    line: usize,
) -> Option<(ExcerptMapping, MappingPosition)> {
    let mut cursor = MultiBufferCursor::new(excerpts, tree, sources);
    cursor.seek_output_line(line, Bias::Right);
    if cursor.item().is_some() {
        let at = cursor.start().clone();
        return cursor.mapping().map(|mapping| (mapping, at));
    }
    cursor.seek_output_line(tree.summary().output.lines, Bias::Left);
    let at = cursor.start().clone();
    cursor
        .item()
        .and_then(|_| cursor.mapping().map(|mapping| (mapping, at)))
}

/// 从查询位置所属的映射出发，投影同一源的连续工作区内容。
///
/// 仅遍历候选实际跨越的片段；删除节点不消费工作区源坐标。
fn source_mapping_range(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    sources: Sources<'_>,
    origin: &ExcerptMapping,
    source_start: usize,
    source_end: usize,
) -> Option<Range<MultiBufferOffset>> {
    // 基线删除片段不属于可折叠的工作区源覆盖范围。
    if origin.diff_kind == Some(ExcerptDiffKind::Deleted) {
        return None;
    }
    if origin.source_range.start().get() <= source_start
        && source_end <= origin.source_range.end().get()
    {
        let output_start = origin.output_range.start().get();
        let source_offset = origin.source_range.start().get();
        return Some(
            MultiBufferOffset::new(output_start + source_start - source_offset)
                ..MultiBufferOffset::new(output_start + source_end - source_offset),
        );
    }

    let mut cursor = MultiBufferCursor::new(excerpts, tree, sources);
    cursor.seek(origin.output_range.start().into(), Bias::Right);
    if cursor.item().is_none() {
        cursor.prev();
    }
    if cursor.item()?.0.diff_kind == Some(ExcerptDiffKind::Deleted) {
        source_content_neighbor(&mut cursor, origin, false)?;
    }
    while source_start < cursor.item()?.0.source_range.start().get() {
        let next_start = cursor.item()?.0.source_range.start();
        source_content_neighbor(&mut cursor, origin, false)?;
        if cursor.item()?.0.source_range.end() != next_start {
            return None;
        }
    }
    while source_start >= cursor.item()?.0.source_range.end().get() {
        let previous_end = cursor.item()?.0.source_range.end();
        source_content_neighbor(&mut cursor, origin, true)?;
        if cursor.item()?.0.source_range.start() != previous_end {
            return None;
        }
    }
    let output_start =
        cursor.start().bytes + source_start - cursor.item()?.0.source_range.start().get();
    while source_end > cursor.item()?.0.source_range.end().get() {
        let previous_end = cursor.item()?.0.source_range.end();
        source_content_neighbor(&mut cursor, origin, true)?;
        if cursor.item()?.0.source_range.start() != previous_end {
            return None;
        }
    }
    let output_end =
        cursor.start().bytes + source_end - cursor.item()?.0.source_range.start().get();
    Some(MultiBufferOffset::new(output_start)..MultiBufferOffset::new(output_end))
}

fn source_content_neighbor(
    cursor: &mut MultiBufferCursor<'_>,
    origin: &ExcerptMapping,
    forward: bool,
) -> Option<()> {
    loop {
        if forward {
            cursor.next();
        } else {
            cursor.prev();
        }
        let (excerpt, transform) = cursor.item()?;
        if excerpt.path != origin.path {
            return None;
        }
        if matches!(transform, DiffTransform::DeletedHunk { .. }) {
            continue;
        }
        return (excerpt.source_index == origin.source_index).then_some(());
    }
}

/// 解析组合锚点时读取源文本快照的统一入口。
///
/// 组合锚点保存源 `zcv_text::Anchor`，解析时必须用当前源快照把锚点版本推进到当前坐标。
trait SourceTexts {
    fn source_text(&self, source_index: usize) -> Option<&Snapshot>;
}

impl SourceTexts for [ExcerptSource] {
    fn source_text(&self, source_index: usize) -> Option<&Snapshot> {
        self.get(source_index).map(|source| &source.text)
    }
}

impl SourceTexts for TreeMap<usize, ExcerptSourceSnapshot> {
    fn source_text(&self, source_index: usize) -> Option<&Snapshot> {
        self.get(&source_index).map(|source| &source.text)
    }
}

/// 组合游标可访问的源文本视图：工作区源表或不可变快照的源帧。
///
/// 组合偏移对应的读取区域可能只覆盖一个 excerpt 的一部分，其行宽摘要没有现成 item 承载，
/// 必须从源快照聚合；游标因此持有源视图。
#[derive(Clone, Copy)]
enum Sources<'a> {
    Workspace(&'a [ExcerptSource]),
    Snapshot(&'a TreeMap<usize, ExcerptSourceSnapshot>),
}

impl<'a> Sources<'a> {
    fn source_text(self, source_index: usize) -> Option<&'a Snapshot> {
        match self {
            Self::Workspace(sources) => sources.get(source_index).map(|source| &source.text),
            Self::Snapshot(sources) => sources.get(&source_index).map(|source| &source.text),
        }
    }
}

impl SourceTexts for Sources<'_> {
    fn source_text(&self, source_index: usize) -> Option<&Snapshot> {
        (*self).source_text(source_index)
    }
}

/// 在给定投影→源映射中把投影偏移锚定到源坐标。
///
/// 供 [`MultiBuffer::anchor_at`] 与 [`MultiBufferSnapshot::anchor_at`] 共用。
fn anchor_in_mappings(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    sources: Sources<'_>,
    offset: ByteOffset,
    affinity: Affinity,
) -> Option<MultiBufferAnchor> {
    let mut cursor = MultiBufferCursor::new(excerpts, tree, sources);
    cursor.seek(offset, Bias::Right);
    anchor_at_cursor(excerpts, tree, sources, &mut cursor, offset, affinity)
}

fn anchor_at_cursor(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    sources: Sources<'_>,
    cursor: &mut MultiBufferCursor<'_>,
    offset: ByteOffset,
    affinity: Affinity,
) -> Option<MultiBufferAnchor> {
    // 左吸附在删除段末端属于旧侧末端，需保留基线 Anchor；
    // 若只锚到同点的工作区起点，解析时会按左吸附落到删除段起点，扩大后续编辑的选区。
    let deleted_end = if affinity == Affinity::Before && cursor.start().bytes == offset.get() {
        let mut previous = cursor.clone();
        previous.prev();
        if matches!(
            previous.item(),
            Some((_, DiffTransform::DeletedHunk { .. }))
        ) && previous
            .mapping()
            .is_some_and(|mapping| mapping.output_range.end().get() == offset.get())
        {
            *cursor = previous;
            true
        } else {
            false
        }
    } else {
        false
    };
    let (mapping, at) = if deleted_end {
        (cursor.mapping()?, cursor.start().clone())
    } else {
        mapping_at_cursor(excerpts, tree, sources, cursor, offset)?
    };
    let source_offset = ByteOffset::new(
        (mapping.source_range.start().get() + offset.get().saturating_sub(at.bytes))
            .min(mapping.source_range.end().get()),
    );
    let text_anchor = match sources.source_text(mapping.source_index) {
        Some(text) => text.anchor_with_affinity(source_offset, affinity),
        None => Anchor::new(mapping.source_range.version(), source_offset).with_affinity(affinity),
    };
    if mapping.diff_kind == Some(ExcerptDiffKind::Deleted) {
        if !deleted_end {
            cursor.seek(offset, Bias::Right);
        }
        let (_, transform) = cursor.item()?;
        let logical = cursor.excerpts.item()?;
        let working_text = sources.source_text(logical.source_index)?;
        let hunk_start = transform.hunks().first()?.hunk_start()?;
        let working_offset = hunk_start.resolve_in(working_text).ok()?;
        return Some(MultiBufferAnchor::Excerpt(ExcerptAnchor {
            path: logical.path_index,
            source_id: logical.source_id,
            text_anchor: working_text.anchor_before(working_offset),
            diff_base_anchor: Some(DiffBaseAnchor {
                source_id: mapping.source_id.expect("删除变换必须拥有基线源"),
                text_anchor,
            }),
        }));
    }
    Some(MultiBufferAnchor::excerpt(
        mapping.path_index,
        mapping.source_id,
        text_anchor,
    ))
}

/// 稳定锚点在当前投影中的解析结果。
///
/// `Detached` 是一次结构变化后的正常状态：位置仍有明确的组合文档边界，但不再属于可见源片段。
/// 源 Anchor 版本链错误由解析入口返回 `TextError`，不能猜测坐标。
enum AnchorResolution {
    Projected(ByteOffset),
    Detached(ByteOffset),
}

impl AnchorResolution {
    fn offset(self) -> ByteOffset {
        match self {
            Self::Projected(offset) | Self::Detached(offset) => offset,
        }
    }

    fn projected_offset(self) -> Option<ByteOffset> {
        match self {
            Self::Projected(offset) => Some(offset),
            Self::Detached(_) => None,
        }
    }
}

/// 路径与源 Anchor 共同构成输入树的查询键；摘要保存该子树的最后一个源 Anchor。
struct AnchorSeekTarget<'a> {
    path_key: &'a PathKey,
    source_id: Option<EntityId>,
    anchor: Anchor,
    source: Option<&'a Snapshot>,
}

impl SeekTarget<'_, ExcerptSummary, ExcerptSummary> for AnchorSeekTarget<'_> {
    fn cmp(&self, summary: &ExcerptSummary, _: ()) -> Ordering {
        Ord::cmp(self.path_key, &summary.path_key).then_with(|| {
            let Some((source_id, max_anchor)) = summary.max_anchor else {
                return Ordering::Greater;
            };
            self.source_id.cmp(&source_id).then_with(|| {
                self.source.map_or(Ordering::Greater, |source| {
                    source.stable_anchor_cmp(&self.anchor, &max_anchor)
                })
            })
        })
    }
}

fn source_for_id<'a, S: SourceTexts + ?Sized>(
    sources: &'a S,
    indices: &HashMap<EntityId, usize>,
    id: Option<EntityId>,
) -> Option<&'a Snapshot> {
    let index = match id {
        Some(id) => *indices.get(&id)?,
        None => 0,
    };
    sources.source_text(index)
}

fn stable_excerpt_text_cmp(
    snapshot: &MultiBufferSnapshot,
    left: &ExcerptAnchor,
    right: &ExcerptAnchor,
) -> Ordering {
    let Some(source) = source_for_id(
        &snapshot.excerpt_sources,
        &snapshot.source_indices,
        left.source_id,
    ) else {
        return Ordering::Equal;
    };
    let cmp = source.stable_anchor_cmp(&left.text_anchor, &right.text_anchor);
    if cmp != Ordering::Equal {
        return cmp;
    }
    match (left.diff_base_anchor, right.diff_base_anchor) {
        (Some(left), Some(right)) => left.source_id.cmp(&right.source_id).then_with(|| {
            source_for_id(
                &snapshot.excerpt_sources,
                &snapshot.source_indices,
                Some(left.source_id),
            )
            .map_or(Ordering::Equal, |source| {
                source.stable_anchor_cmp(&left.text_anchor, &right.text_anchor)
            })
        }),
        (Some(_), None) => {
            if right.text_anchor.affinity() == Affinity::Before {
                Ordering::Greater
            } else {
                Ordering::Less
            }
        }
        (None, Some(_)) => {
            if left.text_anchor.affinity() == Affinity::Before {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        }
        (None, None) => Ordering::Equal,
    }
}

/// 先沿输入树解析工作区 Anchor，再在对应输入位置定位输出变换。
/// 删除侧只额外携带基线 Anchor，不改变逻辑窗口的身份或索引。
fn resolve_anchor_in_mappings<S: SourceTexts + ?Sized>(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    path_keys: &[PathKey],
    sources: &S,
    source_indices: &HashMap<EntityId, usize>,
    anchor: &MultiBufferAnchor,
) -> TextResult<AnchorResolution> {
    let anchor = match anchor {
        MultiBufferAnchor::Min => return Ok(AnchorResolution::Projected(ByteOffset::ZERO)),
        MultiBufferAnchor::Max => {
            return Ok(AnchorResolution::Projected(ByteOffset::new(
                tree.summary().output.len,
            )));
        }
        MultiBufferAnchor::Excerpt(anchor) => anchor,
    };
    let path_key =
        path_keys
            .get(anchor.path.get() as usize)
            .ok_or_else(|| TextError::InvariantViolation {
                location: "MultiBuffer::anchor_offset",
                detail: "组合 Anchor 的路径索引不属于当前 MultiBuffer".into(),
            })?;
    let source = source_for_id(sources, source_indices, anchor.source_id);
    let source_offset = source
        .map(|source| anchor.text_anchor.resolve_in(source))
        .transpose()?;
    let target = AnchorSeekTarget {
        path_key,
        source_id: anchor.source_id,
        anchor: anchor.text_anchor,
        source,
    };
    let mut cursor = excerpts.cursor::<ExcerptSummary>(());
    cursor.seek(&target, Bias::Right);
    // 文尾与文件末端仍归最后一个窗口；半开区间共享端点优先归后继窗口。
    let mut projected = cursor.item().is_some_and(|excerpt| {
        &excerpt.path == path_key
            && excerpt.source_id == anchor.source_id
            && source_offset.is_some_and(|offset| {
                excerpt.source_range.start() <= offset && offset <= excerpt.source_range.end()
            })
    });
    if !projected {
        let mut previous = cursor.clone();
        previous.prev();
        if previous.item().is_some_and(|excerpt| {
            &excerpt.path == path_key
                && excerpt.source_id == anchor.source_id
                && source_offset.is_some_and(|offset| offset >= excerpt.source_range.end())
        }) {
            cursor = previous;
            projected = true;
        }
    }
    let mut input = cursor.start().text.len;
    if projected {
        let excerpt = cursor.item().expect("已投影 Anchor 必须命中窗口");
        let offset = source_offset.expect("已投影 Anchor 必须有源坐标");
        projected = offset <= excerpt.source_range.end();
        input +=
            offset.get().min(excerpt.source_range.end().get()) - excerpt.source_range.start().get();
    }
    let mut output = tree.cursor::<MappingPosition>(());
    output.seek(&ExcerptOffset::new(input), Bias::Left);
    let base = anchor
        .diff_base_anchor
        .and_then(|base| {
            source_for_id(sources, source_indices, Some(base.source_id))
                .map(|source| (base, source))
        })
        .map(|(base, source)| {
            base.text_anchor
                .resolve_in(source)
                .map(|offset| (base.source_id, offset))
        })
        .transpose()?;
    loop {
        let at = output.start();
        let Some(transform) = output.item() else {
            let offset = ByteOffset::new(at.bytes);
            return Ok(if projected && anchor.diff_base_anchor.is_none() {
                AnchorResolution::Projected(offset)
            } else {
                AnchorResolution::Detached(offset)
            });
        };
        let end_input = at.input_offset.get() + transform.transform_summary().input.len;
        match transform {
            DiffTransform::DeletedHunk { region, .. } => {
                if projected
                    && let Some((source_id, offset)) = base
                    && source_id == region.source_id
                    && region.source_range.start() <= offset
                    && offset <= region.source_range.end()
                {
                    return Ok(AnchorResolution::Projected(ByteOffset::new(
                        at.bytes + offset.get() - region.source_range.start().get(),
                    )));
                }
                if projected
                    && anchor.diff_base_anchor.is_none()
                    && anchor.text_anchor.affinity() == Affinity::Before
                {
                    return Ok(AnchorResolution::Projected(ByteOffset::new(at.bytes)));
                }
                if at.input_offset.get() == input
                    && (anchor.diff_base_anchor.is_none()
                        || base.is_some_and(|(_, offset)| offset > region.source_range.end()))
                {
                    output.next();
                    continue;
                }
                return Ok(AnchorResolution::Detached(ByteOffset::new(at.bytes)));
            }
            DiffTransform::BufferContent { .. } => {
                if end_input == input && output.next_item().is_some() {
                    output.next();
                    continue;
                }
                let offset =
                    ByteOffset::new(at.bytes + input.saturating_sub(at.input_offset.get()));
                return Ok(if projected && anchor.diff_base_anchor.is_none() {
                    AnchorResolution::Projected(offset)
                } else {
                    AnchorResolution::Detached(offset)
                });
            }
        }
    }
}

#[cfg(test)]
#[path = "test/multi_buffer_tests.rs"]
mod tests;
