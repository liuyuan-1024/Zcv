//! Editor 与具体文本 Buffer 之间的组合文档边界。
//!
//! 组合文档按调用方给出的顺序组织多个来源的 excerpts，并保留组合坐标到源文件坐标的映射。
//! 普通编辑器是「整文件单 excerpt」的组合文档；差异投影在稳定 excerpts 上叠加输出变换。
//! Editor 始终只消费本层，不感知来源数量。
//! diff 显示拓扑（git hunks、展开状态、跟踪区间与显示坐标）见 [`diff_projection`]。

mod diff_projection;
mod diff_transform_sync;
mod path_key;

pub use diff_projection::{
    DiffDisplaySnapshot, DiffExcerptRanges, DiffFile, DiffHunkSource, DisplayHunk,
    ResolvedDiffHunk, WordDiffs,
};
pub(crate) use path_key::{PathKey, PathKeyIndex};

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, hash_map::Entry};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use gpui::{App, Context, Entity, EventEmitter, Subscription};
use sum_tree::{Bias, ContextLessSummary, Cursor, Dimension, Item, SeekTarget, SumTree, TreeMap};
use unicode_segmentation::UnicodeSegmentation;
use zcv_buffer_diff::{DiffHunkKind, DiffHunkStaging};
use zcv_language::{
    AutoClosePair, BracketPair, HighlightCache, HighlightSpan, LanguageBuffer, LanguageBufferEvent,
    LanguageBufferSnapshot, LanguageRegistry, LanguageSettings, LocalBinding, NewlineIndent,
    OutlineItem, OutlineTextRange, SyntaxNode, SyntaxSnapshot,
};
use zcv_text::{
    Affinity, Anchor, Buffer, BufferConfig, BufferId, BufferVersion, ByteOffset, CharOffset,
    CoordinateError, Edit, Line, LineEndingStyle, LogicalColumn, MovementDirection, MovementUnit,
    Position, PositionMap, Snapshot, Stickiness, StorageError, TextChangeBatch, TextError,
    TextRange, TextRead, TextResult, TextSubscription, TransactionError, TransactionId,
    TransactionMetadata, Utf16Offset, Utf16Position, WordBoundaryPolicy,
};

/// 组合文档中的一个源片段。
#[derive(Clone)]
pub struct ExcerptRange {
    source: Entity<LanguageBuffer>,
    source_range: TextRange,
    match_ranges: Vec<TextRange>,
    display_path: Option<PathKey>,
    editable: bool,
}

/// 组合投影片段在统一 diff 中承担的文本侧别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExcerptDiffKind {
    Added,
    Deleted,
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

    /// 标记该源片段是否接受组合编辑。
    pub fn with_editable(mut self, editable: bool) -> Self {
        self.editable = editable;
        self
    }

    /// 取源文档中一个 0-based、左闭右开的完整逻辑行范围。
    ///
    /// 范围终点可以等于 `line_count`；
    /// 空文件的 `0..1` 会得到零字节源范围，按组合尾换行不变式在非末尾片段时占一个组合边界行。
    pub fn line_range(
        source: Entity<LanguageBuffer>,
        lines: std::ops::Range<usize>,
        cx: &App,
    ) -> Self {
        let text = source.read(cx).text_snapshot();
        Self::line_range_from_text(source, &text, lines)
    }

    /// 从已读取的源文本快照取行范围。
    ///
    /// `line_range` 与 diff 文档的 working 窗口共用此入口。
    pub(crate) fn line_range_from_text(
        source: Entity<LanguageBuffer>,
        text: &Snapshot,
        lines: std::ops::Range<usize>,
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
/// 对齐 Zed 的组合 Anchor：要么是文档边界，要么绑定一个源片段身份与源文本 Anchor。
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
    source_id: Option<gpui::EntityId>,
    text_anchor: Anchor,
}

impl MultiBufferAnchor {
    fn excerpt(path: PathKeyIndex, source_id: Option<gpui::EntityId>, text_anchor: Anchor) -> Self {
        Self::Excerpt(ExcerptAnchor {
            path,
            source_id,
            text_anchor,
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
    /// 对齐 Zed 的 `ExcerptAnchor::cmp`：先按路径顺序，再按源身份，最后按源内插入身份的稳定文档序。
    /// `Min`/`Max` 边界位于所有源锚点两侧。折叠树据此保持稳定顺序，编辑后无需按解析结果重排。
    pub fn cmp(&self, other: &Self, snapshot: &MultiBufferSnapshot) -> Ordering {
        match (self, other) {
            (Self::Min, Self::Min) | (Self::Max, Self::Max) => Ordering::Equal,
            (Self::Min, _) => Ordering::Less,
            (_, Self::Min) => Ordering::Greater,
            (Self::Max, _) => Ordering::Greater,
            (_, Self::Max) => Ordering::Less,
            (Self::Excerpt(left), Self::Excerpt(right)) => left
                .path
                .cmp(&right.path)
                .then_with(|| left.source_id.cmp(&right.source_id))
                .then_with(|| stable_excerpt_text_cmp(snapshot, left, right)),
        }
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
    /// 源语言的词边界策略（对齐 Zed 的 per-language word_characters）。
    word_boundary: WordBoundaryPolicy,
    /// 源语言解析后的编辑器设置。
    settings: Arc<LanguageSettings>,
    capture_map: Arc<[u32]>,
}

fn path_key_for_source(source: &LanguageBuffer) -> PathKey {
    PathKey::for_buffer(source.file_path(), source.buffer_id())
}

/// 不可变快照帧中的源状态（不携带实体引用）。
#[derive(Clone, Debug)]
struct ExcerptSourceSnapshot {
    text: Snapshot,
    syntax: SyntaxSnapshot,
    highlight_cache: Arc<HighlightCache>,
    word_boundary: WordBoundaryPolicy,
    settings: Arc<LanguageSettings>,
    capture_map: Arc<[u32]>,
}

/// 输出变换节点携带的 diff hunk 身份与显示元数据。
///
/// 身份绑定 working 源与 hunk 起点的工作区 Anchor，不随组合文档序号、`visible_hunks` 下标或源范围变化。
/// 输出行/字节范围由游标推导；源操作范围是 BufferDiff 快照的只读派生数据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiffTransformHunkInfo {
    working: gpui::EntityId,
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
    /// 结构分隔换行，与源末尾是否已有换行无关；仅 excerpt 增删或重排时改变。
    adds_newline: bool,
    /// 该片段内真实内容匹配的源范围（搜索高亮用；diff 片段为空）。
    match_ranges: Arc<[TextRange]>,
    /// 指向源表（`ExcerptState::sources` / 快照的 `excerpt_sources`）的索引。
    source_index: usize,
    /// 该片段所属的工作区源实体；纯文本派生快照没有实体源，因此为 `None`。
    source_id: Option<gpui::EntityId>,
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
    source_id: Option<gpui::EntityId>,
    editable: bool,
    diff_kind: Option<ExcerptDiffKind>,
}

/// 删除变换拥有的基线读取描述；不具备输入 excerpt 身份或编辑状态。
#[derive(Clone, Debug)]
struct DeletedHunkRegion {
    source_index: usize,
    source_id: gpui::EntityId,
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
    fn new(version: BufferVersion, range: TextRange, outside: bool) -> Self {
        let (start_affinity, end_affinity) = if outside {
            (Affinity::Before, Affinity::After)
        } else {
            (Affinity::After, Affinity::Before)
        };
        Self {
            start: Anchor::new(version, range.start()).with_affinity(start_affinity),
            end: Anchor::new(version, range.end()).with_affinity(end_affinity),
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
            start: Anchor::new(snapshot.version(), mapped.start()).with_affinity(start_affinity),
            end: Anchor::new(snapshot.version(), mapped.end()).with_affinity(end_affinity),
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

/// 输入窗口的完整摘要：源内容与窗口拥有的结构分隔换行。
fn diff_output_text(excerpt: &Excerpt) -> MBTextSummary {
    let mut output_text = excerpt.text_summary;
    if excerpt.adds_newline {
        output_text += MBTextSummary::newline();
    }
    output_text
}

impl DiffTransform {
    /// 从输入侧 excerpt 构造内容变换摘要，并接收本节点的 hunk 身份。
    fn from_excerpt(excerpt: &Excerpt, hunks: Vec<DiffTransformHunkInfo>) -> Self {
        let text = diff_output_text(excerpt);
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
        let mut text = region.text_summary;
        if region.adds_newline {
            text += MBTextSummary::newline();
        }
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
        let mut text = content;
        if completes_excerpt && excerpt.adds_newline {
            text += MBTextSummary::newline();
        }
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
    /// 在给定输出坐标起点上派生对外片段快照；快照只是树的只读视图，不反向写回。
    fn to_snapshot(&self, at: MappingPosition) -> ExcerptSnapshot {
        let separator = self.adds_newline as usize;
        let len = self.text_summary.len + separator;
        ExcerptSnapshot {
            path: self.path.clone(),
            display_path: self.display_path.clone(),
            buffer_id: self.buffer_id,
            output_range: MultiBufferRange::new(
                MultiBufferOffset::new(at.bytes),
                MultiBufferOffset::new(at.bytes + len),
            )
            .expect("组合片段输出范围必须正序"),
            source_range: self.source_range.range(),
            output_start_line: at.lines,
            output_end_line: at.lines + self.text_summary.lines + separator,
            source_start_line: self.source_start_line,
            source_index: self.source_index,
            editable: self.editable,
            diff_kind: self.diff_kind,
        }
    }

    /// 在给定游标位置（输出字节 + 输出行起点）上构造派生视图。
    fn to_mapping(&self, at: MappingPosition) -> ExcerptMapping {
        let separator = self.adds_newline as usize;
        let len = self.text_summary.len + separator;
        ExcerptMapping {
            entry: self.clone(),
            excerpt_index: at.input_item_index,
            output_range: MultiBufferRange::new(
                MultiBufferOffset::new(at.bytes),
                MultiBufferOffset::new(at.bytes + len),
            )
            .expect("组合片段输出范围必须正序"),
        }
    }
}

impl Excerpt {
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

/// 组合文本的多维长度摘要，对应 Zed 的 MBTextSummary。
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
    /// 最后一行的字节列；摘要拼接时不与总长度混用。
    pub last_line_len: usize,
    /// 最后一行的 Unicode scalar 列。
    pub last_line_chars: usize,
    /// 最后一行的 UTF-16 code unit 列。
    pub last_line_len_utf16: usize,
}

impl MBTextSummary {
    const fn newline() -> Self {
        Self {
            len: 1,
            chars: 1,
            len_utf16: 1,
            lines: 1,
            last_line_len: 0,
            last_line_chars: 0,
            last_line_len_utf16: 0,
        }
    }
}

impl std::ops::AddAssign for MBTextSummary {
    fn add_assign(&mut self, other: Self) {
        self.len += other.len;
        self.chars += other.chars;
        self.len_utf16 += other.len_utf16;
        if other.lines > 0 {
            self.last_line_len = other.last_line_len;
            self.last_line_chars = other.last_line_chars;
            self.last_line_len_utf16 = other.last_line_len_utf16;
        } else {
            self.last_line_len += other.last_line_len;
            self.last_line_chars += other.last_line_chars;
            self.last_line_len_utf16 += other.last_line_len_utf16;
        }
        self.lines += other.lines;
    }
}

/// 输入（未删除拼接）坐标偏移，对应 Zed 的 `ExcerptOffset`。
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
}

impl ContextLessSummary for ExcerptSummary {
    fn zero() -> Self {
        Self::default()
    }

    fn add_summary(&mut self, summary: &Self) {
        self.text += summary.text;
        self.count += summary.count;
        self.path_key = summary.path_key.clone();
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
            text: diff_output_text(self),
            count: 1,
            path_key: self.path.clone(),
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
    column_utf16: usize,
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
            self.column_utf16 = summary.output.last_line_len_utf16;
        } else {
            self.column_bytes += summary.output.last_line_len;
            self.column_chars += summary.output.last_line_chars;
            self.column_utf16 += summary.output.last_line_len_utf16;
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

#[derive(Clone, Copy, Debug, Default)]
struct MultiBufferCoordinates {
    chars: usize,
    utf16: usize,
    line: usize,
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

/// 两个同坐标空间摘要前缀之间的文本摘要。
fn summary_between(start: MBTextSummary, end: MBTextSummary) -> MBTextSummary {
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
        last_line_len_utf16: if end.lines == start.lines {
            end.last_line_len_utf16 - start.last_line_len_utf16
        } else {
            end.last_line_len_utf16
        },
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

/// 沿变换树比较同版本源片段的公共文本前后缀，不物化全文。
/// 源内部编辑由 TextChangeBatch 投影；结构同步只发布实际插入／移除的文本。
fn projection_changed_ranges(
    before: &ProjectionTrees,
    after: &ProjectionTrees,
) -> (TextRange, TextRange) {
    let old_len = before.1.summary().output.len;
    let new_len = after.1.summary().output.len;
    let limit = old_len.min(new_len);
    let mut old_cursor = MultiBufferCursor::new(&before.0, &before.1);
    let mut new_cursor = MultiBufferCursor::new(&after.0, &after.1);
    old_cursor.seek_transform_index(0);
    new_cursor.seek_transform_index(0);
    let mut prefix = 0;
    let mut old_offset = 0;
    let mut new_offset = 0;
    while prefix < limit {
        let Some((old, _)) = old_cursor.item() else {
            break;
        };
        let Some((new, _)) = new_cursor.item() else {
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

    old_cursor.seek_output(ByteOffset::new(old_len), Bias::Left);
    new_cursor.seek_output(ByteOffset::new(new_len), Bias::Left);
    let mut old_remaining = old_cursor.item().map_or(0, |(region, _)| {
        region.text_summary.len + usize::from(region.adds_newline)
    });
    let mut new_remaining = new_cursor.item().map_or(0, |(region, _)| {
        region.text_summary.len + usize::from(region.adds_newline)
    });
    let mut suffix = 0;
    while prefix + suffix < limit {
        while old_remaining == 0 && old_cursor.item().is_some() {
            old_cursor.prev();
            old_remaining = old_cursor.item().map_or(0, |(region, _)| {
                region.text_summary.len + usize::from(region.adds_newline)
            });
        }
        while new_remaining == 0 && new_cursor.item().is_some() {
            new_cursor.prev();
            new_remaining = new_cursor.item().map_or(0, |(region, _)| {
                region.text_summary.len + usize::from(region.adds_newline)
            });
        }
        let Some((old, _)) = old_cursor.item() else {
            break;
        };
        let Some((new, _)) = new_cursor.item() else {
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
/// 读取路径通过此类型完成坐标联合，避免每次查询重新从 transform 序号创建输入树游标。
#[derive(Clone)]
struct MultiBufferCursor<'a> {
    excerpts: Cursor<'a, 'static, Excerpt, ExcerptSummary>,
    diff_transforms: Cursor<'a, 'static, DiffTransform, MappingPosition>,
}

impl<'a> MultiBufferCursor<'a> {
    fn new(excerpts: &'a SumTree<Excerpt>, diff_transforms: &'a SumTree<DiffTransform>) -> Self {
        Self {
            excerpts: excerpts.cursor::<ExcerptSummary>(()),
            diff_transforms: diff_transforms.cursor::<MappingPosition>(()),
        }
    }

    fn sync_excerpts_at_input(&mut self, input: ExcerptOffset, bias: Bias) {
        self.excerpts.seek(&input, bias);
        if self.excerpts.item().is_none() {
            self.excerpts.prev();
        }
    }

    fn sync_output_offset(&mut self, offset: ByteOffset, bias: Bias) {
        let Some(transform) = self.diff_transforms.item() else {
            return;
        };
        let at = self.diff_transforms.start();
        let input = at.input_offset.get()
            + match transform {
                DiffTransform::BufferContent { summary, .. } => {
                    offset.get().saturating_sub(at.bytes).min(summary.input.len)
                }
                DiffTransform::DeletedHunk { .. } => 0,
            };
        self.sync_excerpts_at_input(ExcerptOffset::new(input), bias);
    }

    fn seek_output(&mut self, offset: ByteOffset, bias: Bias) {
        self.diff_transforms
            .seek(&MultiBufferOffset(offset.get()), bias);
        self.sync_output_offset(offset, bias);
    }

    fn seek_output_forward(&mut self, offset: ByteOffset, bias: Bias) {
        self.diff_transforms
            .seek_forward(&MultiBufferOffset(offset.get()), bias);
        self.sync_output_offset(offset, bias);
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
            let logical_end = self.excerpts.start().text.len + diff_output_text(logical).len;
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

    fn item(&self) -> Option<(OutputRegion, &DiffTransform)> {
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
                logical_end += diff_output_text(logical);
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
                let content = summary_between(start_summary, content_end);
                let start = ByteOffset::new(logical.source_range.start().get() + relative);
                let end = ByteOffset::new(start.get() + content.len);
                let mut region = logical.output_region();
                region.source_range = ExcerptContext::new(
                    logical.source_range.version(),
                    TextRange::new(start, end).expect("内容变换必须位于逻辑 excerpt 内"),
                    false,
                );
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

    fn start(&self) -> MappingPosition {
        let mut at = self.diff_transforms.start().clone();
        at.input_item_index = self.excerpts.start().count;
        if matches!(
            self.diff_transforms.item(),
            Some(DiffTransform::BufferContent { .. })
        ) && self.excerpts.start().text.len > at.input_text.len
        {
            let prefix = summary_between(at.input_text, self.excerpts.start().text);
            at.bytes += prefix.len;
            at.chars += prefix.chars;
            at.utf16 += prefix.len_utf16;
            if prefix.lines > 0 {
                at.column_bytes = prefix.last_line_len;
                at.column_chars = prefix.last_line_chars;
                at.column_utf16 = prefix.last_line_len_utf16;
            } else {
                at.column_bytes += prefix.last_line_len;
                at.column_chars += prefix.last_line_chars;
                at.column_utf16 += prefix.last_line_len_utf16;
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
        self.sync_output_position(position, bias);
    }
}

/// 按非递减组合字节偏移转换逻辑位置，并复用 excerpt 与 diff 变换树游标。
pub struct MultiBufferPositionCursor<'a> {
    snapshot: &'a MultiBufferSnapshot,
    cursor: MultiBufferCursor<'a>,
    last_offset: Option<MultiBufferOffset>,
}

impl<'a> MultiBufferPositionCursor<'a> {
    pub fn new(snapshot: &'a MultiBufferSnapshot) -> Self {
        Self {
            snapshot,
            cursor: MultiBufferCursor::new(&snapshot.excerpts, &snapshot.diff_transforms),
            last_offset: None,
        }
    }

    pub fn reset(&mut self) {
        self.cursor =
            MultiBufferCursor::new(&self.snapshot.excerpts, &self.snapshot.diff_transforms);
        self.last_offset = None;
    }

    pub fn byte_to_position(&mut self, offset: MultiBufferOffset) -> TextResult<Position> {
        if self.last_offset.is_some_and(|last| offset < last) {
            self.reset();
        }
        if self.last_offset.is_some() {
            self.cursor.seek_output_forward(offset.into(), Bias::Right);
        } else {
            self.cursor.seek_output(offset.into(), Bias::Right);
        }
        self.last_offset = Some(offset);
        self.snapshot
            .byte_to_position_with_cursor(&mut self.cursor, offset)
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
        let mut cursor = MultiBufferCursor::new(&snapshot.excerpts, &snapshot.diff_transforms);
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
    start: ExcerptMapping,
    end: ByteOffset,
) -> Vec<ExcerptMapping> {
    let mut cursor = MultiBufferCursor::new(excerpts, tree);
    cursor.seek_output(
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
    // 字符、UTF-16 与逻辑行数由源快照自身的坐标查询差分得到（O(log n)），
    // 不逐个 chunk 扫描源文本；三者必须来自同一次源范围语义。
    let chars = text
        .byte_to_char(range.end())
        .ok()?
        .get()
        .saturating_sub(text.byte_to_char(range.start()).ok()?.get());
    let utf16 = text
        .byte_to_utf16_cu(range.end())
        .ok()?
        .get()
        .saturating_sub(text.byte_to_utf16_cu(range.start()).ok()?.get());
    let lines = text
        .byte_to_line(range.end())
        .ok()?
        .get()
        .saturating_sub(text.byte_to_line(range.start()).ok()?.get());
    let ends_with_newline = text
        .slice_byte_range(
            ByteOffset::new(range.end().get().saturating_sub(1)),
            range.end(),
        )
        .is_ok_and(|text| text.as_str() == "\n");
    Some((
        MBTextSummary {
            len: range.len(),
            chars,
            len_utf16: utf16,
            lines,
            last_line_len: if lines == 0 {
                range.len()
            } else {
                text.byte_to_point(range.end()).ok()?.1
            },
            last_line_chars: if lines == 0 {
                chars
            } else {
                text.byte_to_position(range.end()).ok()?.column().get()
            },
            last_line_len_utf16: if lines == 0 {
                utf16
            } else {
                text.byte_to_utf16_position(range.end())
                    .ok()?
                    .character()
                    .get()
            },
        },
        ends_with_newline,
    ))
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
/// Zed 在同步源 Buffer 与 diff transform 时只对外提交一帧快照；
/// 这里累积帧内依次落地的精确投影编辑，由帧结束时一次性发布净增量与对外版本。
/// 结构增删的区间由各结构操作自身的树比较产出并同样进入累积，帧末不再做兜底比较。
struct ProjectionSync {
    /// 帧内依次落地的投影编辑；各段旧坐标基于上一段落地后的状态，按顺序组合。
    changes: TextChangeBatch,
    changed: bool,
    /// 只更新了快照可见的显示元数据，没有组合文本 edit。
    snapshot_changed: bool,
    waiting_for_diff_sources: HashSet<gpui::EntityId>,
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
/// 这对应 Zed `MultiBufferSnapshot::excerpt_boundaries_in_range` 提供给 `BlockMap` 的领域事实。
/// 边界只来自逻辑 excerpt；删除块与内容变换的拆分、合并不会创建边界。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExcerptBoundary {
    previous: Option<ExcerptSnapshot>,
    next: ExcerptSnapshot,
    next_index: usize,
}

impl ExcerptBoundary {
    pub fn previous(&self) -> Option<&ExcerptSnapshot> {
        self.previous.as_ref()
    }

    pub fn next(&self) -> &ExcerptSnapshot {
        &self.next
    }

    /// `next` 在当前快照 `excerpts()` 序列中的下标。
    pub fn next_index(&self) -> usize {
        self.next_index
    }

    /// 边界是否进入了一个新的 Buffer。
    ///
    /// 这是 Zed `ExcerptBoundary::starts_new_buffer` 的同一语义：只比较相邻逻辑 excerpt 的 Buffer 身份，与显示策略无关。
    pub fn starts_new_buffer(&self) -> bool {
        match self.previous.as_ref() {
            None => true,
            Some(previous) => previous.buffer_id() != self.next.buffer_id(),
        }
    }
}

/// 一帧组合文档的不可变快照。
#[derive(Clone, Debug)]
pub struct MultiBufferSnapshot {
    projection_version: BufferVersion,
    topology_version: u64,
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
    source_indices: Arc<HashMap<gpui::EntityId, usize>>,
    capture_names: Arc<[Arc<str>]>,
    metadata_version: u64,
    /// diff 显示输入与文本／语言元数据分离：它只驱动装饰替换，不触发显示拓扑同步。
    diff_display: Option<Arc<DiffDisplaySnapshot>>,
    /// 整个组合文档的显示策略：是否为新 Buffer 绘制实体 header。
    /// 单文件文档（`singleton`）不产生边界；该策略只决定多文件边界画 header 还是 divider。
    show_headers: bool,
    /// 单文件组合文档：Zed 语义下不产生任何 excerpt 边界。
    singleton: bool,
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
    /// Zed 的订阅协议始终保留可组合的输出编辑；
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
    Reparsed(gpui::EntityId),
    MetadataChanged,
    /// diff 展开/折叠状态变化；输出变换已同步，逻辑窗口保持不变。
    DiffExpansionChanged,
}

impl MultiBufferSnapshot {
    fn source_snapshot(&self, source_index: usize) -> Option<&ExcerptSourceSnapshot> {
        self.excerpt_sources.get(&source_index)
    }

    fn first_source_snapshot(&self) -> Option<&ExcerptSourceSnapshot> {
        self.excerpt_sources.first().map(|(_, source)| source)
    }

    /// 主源语言的词边界策略；无源时返回默认。
    ///
    /// 全文搜索等不携带具体位置的消费方使用它；按位置消费方用 `word_boundary_at`。
    pub fn word_boundary(&self) -> WordBoundaryPolicy {
        self.first_source_snapshot()
            .map_or_else(WordBoundaryPolicy::default, |source| source.word_boundary)
    }

    /// 指定组合偏移所属源语言的词边界策略。
    fn word_boundary_at(&self, offset: MultiBufferOffset) -> WordBoundaryPolicy {
        self.source_point(offset.into())
            .map_or_else(WordBoundaryPolicy::default, |(_, source, _)| {
                source.word_boundary
            })
    }

    /// 主源语言解析后的编辑器设置（对齐 Zed `LanguageSettings::for_buffer`）。
    pub fn language_settings(&self) -> Arc<LanguageSettings> {
        self.first_source_snapshot().map_or_else(
            || Arc::new(LanguageSettings::default()),
            |source| Arc::clone(&source.settings),
        )
    }

    /// 指定组合偏移所属源语言解析后的编辑器设置。
    pub fn language_settings_at(&self, offset: MultiBufferOffset) -> Arc<LanguageSettings> {
        self.source_point(offset.into()).map_or_else(
            || Arc::new(LanguageSettings::default()),
            |(_, source, _)| Arc::clone(&source.settings),
        )
    }

    /// 当前源快照元数据版本。
    ///
    /// 该版本独立于组合文本投影版本：语法重解析不改变文本坐标，但必须让显示层替换其持有的源快照。
    pub fn metadata_version(&self) -> u64 {
        self.metadata_version
    }

    /// 返回当前组合文档的完整 UTF-8 内容，供预览等只读消费者使用。
    pub fn text_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.len_bytes().get());
        for chunk in self.bytes_in_range(ByteOffset::ZERO.into()..self.len_bytes()) {
            bytes.extend_from_slice(chunk.text.as_bytes());
        }
        bytes
    }

    /// 组合文本总字节数。
    /// 由变换树的输出摘要决定，不读取或复制组合文本。
    pub fn len_bytes(&self) -> MultiBufferOffset {
        ByteOffset::new(self.diff_transforms.summary().output.len).into()
    }

    /// 虚拟组合文本的逻辑行数。
    ///
    /// excerpt 映射在建立时已经累计了输出行边界，因此这里不扫描、更不拼接所有源文本。
    pub fn line_count(&self) -> usize {
        // 末尾片段的输出行终点等于 Summary 累积行数；总行数比终点多一行。
        self.diff_transforms.summary().output.lines + 1
    }

    /// 把组合偏移转换为逻辑行。
    pub fn byte_to_line(&self, offset: MultiBufferOffset) -> TextResult<Line> {
        if offset == self.len_bytes() {
            return Ok(Line::new(self.diff_transforms.summary().output.lines));
        }
        let (entry, at, source, source_offset) = self.source_point_at_byte(offset)?;
        let source_line = source.text.byte_to_line(source_offset)?.get();
        Ok(Line::new(
            at.lines + source_line.saturating_sub(entry.source_start_line),
        ))
    }

    /// 返回组合逻辑行的起始字节偏移。
    pub fn line_start_byte(&self, target: Line) -> TextResult<MultiBufferOffset> {
        if target.get() >= self.line_count() {
            return Err(CoordinateError::LineOutOfBounds(target).into());
        }
        self.position_to_byte(Position::new(target, LogicalColumn::ZERO))
    }

    /// 组合逻辑行的完整字节范围（含行尾换行符）；行号越界返回 None。
    ///
    /// 最后一行以组合文本末尾为终点。
    pub fn line_byte_range(&self, line: Line) -> Option<Range<MultiBufferOffset>> {
        let start = self.line_start_byte(line).ok()?;
        let end = if line.get() + 1 < self.line_count() {
            self.line_start_byte(Line::new(line.get() + 1)).ok()?
        } else {
            self.len_bytes()
        };
        Some(start..end)
    }

    /// 组合逻辑行内容的字节范围（不含行尾 `\r`/`\n`）。
    pub fn line_content_byte_range(&self, line: Line) -> Option<Range<MultiBufferOffset>> {
        let range = self.line_byte_range(line)?;
        let mut end = range.end;
        while end > range.start {
            let last = MultiBufferOffset::new(end.get() - 1);
            let is_line_break = self
                .bytes_in_range(last..end)
                .next()
                .and_then(|chunk| chunk.text.as_bytes().first().copied())
                .is_some_and(|byte| byte == b'\n' || byte == b'\r');
            if !is_line_break {
                break;
            }
            end = last;
        }
        Some(range.start..end)
    }

    /// 组合逻辑行的文本（含行尾换行符）；单块时借用源文本切片，跨块时拼接。
    ///
    /// 行号越界返回 None。
    pub fn line_text(&self, line: Line) -> Option<Cow<'_, str>> {
        let range = self.line_byte_range(line)?;
        if range.is_empty() {
            return Some(Cow::Borrowed(""));
        }
        let mut chunks = self.bytes_in_range(range);
        let first = chunks.next()?;
        if let Some(second) = chunks.next() {
            let mut text = String::from(first.text);
            text.push_str(second.text);
            text.extend(chunks.map(|chunk| chunk.text));
            Some(Cow::Owned(text))
        } else {
            Some(Cow::Borrowed(first.text))
        }
    }

    /// 组合逻辑行内容的字节长度与字符数（不含行尾换行符）。
    pub fn line_content_metrics(&self, line: Line) -> Option<(usize, usize)> {
        let content = self.line_content_byte_range(line)?;
        let content_len = content.end.get() - content.start.get();
        let mut byte = content.start.get();
        let mut chars = 0;
        while byte < content.end.get() {
            let (chunk, chunk_start) = self.chunk_at_byte(MultiBufferOffset::new(byte)).ok()?;
            let start = byte - chunk_start.get();
            let end = (content.end.get() - chunk_start.get()).min(chunk.len());
            chars += chunk[start..end].chars().count();
            byte = chunk_start.get() + end;
        }
        Some((content_len, chars))
    }

    /// 定位到起始行的前向行游标。
    ///
    /// 显示层逐可见行读取组合行内容时必须走它，而不是逐行调用无状态的
    /// line_start_byte/line_content_byte_range：后者每次都对映射树整树 seek，
    /// 成本随目标名次增长，会让滚动帧随文档深度变慢。
    pub fn line_cursor(&self, start: Line) -> Option<MultiBufferLineCursor<'_>> {
        MultiBufferLineCursor::new(self, start)
    }

    fn coordinates_at_byte(&self, offset: MultiBufferOffset) -> TextResult<MultiBufferCoordinates> {
        if offset == self.len_bytes() {
            let text = &self.diff_transforms.summary().output;
            return Ok(MultiBufferCoordinates {
                chars: text.chars,
                utf16: text.len_utf16,
                line: text.lines,
            });
        }
        let (entry, at, source, source_offset) = self.source_point_at_byte(offset)?;
        let source_line = source.text.byte_to_line(source_offset)?.get();
        let source_char = source.text.byte_to_char(source_offset)?.get();
        let source_utf16 = source.text.byte_to_utf16_cu(source_offset)?.get();
        let source_start_line = source.text.byte_to_line(entry.source_range.start())?.get();
        let source_start_char = source.text.byte_to_char(entry.source_range.start())?.get();
        let source_start_utf16 = source
            .text
            .byte_to_utf16_cu(entry.source_range.start())?
            .get();
        Ok(MultiBufferCoordinates {
            chars: at.chars + source_char - source_start_char,
            utf16: at.utf16 + source_utf16 - source_start_utf16,
            line: at.lines + source_line.saturating_sub(source_start_line),
        })
    }

    /// 把组合字节偏移转换为按 Unicode scalar value 计数的逻辑位置。
    pub fn byte_to_position(&self, offset: MultiBufferOffset) -> TextResult<Position> {
        let mut cursor = MultiBufferCursor::new(&self.excerpts, &self.diff_transforms);
        cursor.seek_output(offset.into(), Bias::Right);
        self.byte_to_position_with_cursor(&mut cursor, offset)
    }

    fn byte_to_position_with_cursor(
        &self,
        cursor: &mut MultiBufferCursor<'_>,
        offset: MultiBufferOffset,
    ) -> TextResult<Position> {
        if offset == self.len_bytes() && self.excerpts.is_empty() {
            return Ok(Position::new(Line::ZERO, LogicalColumn::ZERO));
        }
        let (entry, at, source, source_offset) = self.source_point_at_cursor(cursor, offset)?;
        let source_start = source.text.byte_to_position(entry.source_range.start())?;
        let source_position = source.text.byte_to_position(source_offset)?;
        let row_delta = source_position.line().get() - entry.source_start_line;
        if offset == self.len_bytes() && entry.adds_newline {
            return Ok(Position::new(
                Line::new(self.diff_transforms.summary().output.lines),
                LogicalColumn::ZERO,
            ));
        }
        let column = if row_delta == 0 {
            at.column_chars + source_position.column().get() - source_start.column().get()
        } else {
            source_position.column().get()
        };
        Ok(Position::new(
            Line::new(at.lines + row_delta),
            LogicalColumn::new(column),
        ))
    }

    /// 返回组合文本中的 Tree-sitter 风格字节坐标。
    pub fn byte_to_point(&self, offset: MultiBufferOffset) -> TextResult<(Line, usize)> {
        if offset == self.len_bytes() && self.excerpts.is_empty() {
            return Ok((Line::ZERO, 0));
        }
        let (entry, at, source, source_offset) = self.source_point_at_byte(offset)?;
        let source_start = source.text.byte_to_point(entry.source_range.start())?;
        let source_point = source.text.byte_to_point(source_offset)?;
        let row_delta = source_point.0.get() - entry.source_start_line;
        if offset == self.len_bytes() && entry.adds_newline {
            return Ok((Line::new(self.diff_transforms.summary().output.lines), 0));
        }
        let column = if row_delta == 0 {
            at.column_bytes + source_point.1 - source_start.1
        } else {
            source_point.1
        };
        Ok((Line::new(at.lines + row_delta), column))
    }

    /// 组合范围的文本多维摘要。
    ///
    /// 两个边界各定位一次组合树，避免分别查询字符、UTF-16 和行时重复从树根 seek。
    pub fn text_summary_for_range(&self, range: MultiBufferRange) -> TextResult<MBTextSummary> {
        let start = self.coordinates_at_byte(range.start())?;
        let end = self.coordinates_at_byte(range.end())?;
        let chars = end.chars.saturating_sub(start.chars);
        let len_utf16 = end.utf16.saturating_sub(start.utf16);
        let lines = end.line.saturating_sub(start.line);
        Ok(MBTextSummary {
            len: range.len(),
            chars,
            len_utf16,
            lines,
            last_line_len: if lines == 0 {
                range.len()
            } else {
                self.byte_to_point(range.end())?.1
            },
            last_line_chars: if lines == 0 {
                chars
            } else {
                self.byte_to_position(range.end())?.column().get()
            },
            last_line_len_utf16: if lines == 0 {
                len_utf16
            } else {
                TextRead::byte_to_utf16_position(self, range.end().into())?
                    .character()
                    .get()
            },
        })
    }

    /// 读取指定组合范围；结果只在调用方需要跨源拼接时短暂存在。
    pub fn text_for_range(&self, range: MultiBufferRange) -> TextResult<String> {
        Ok(self
            .bytes_in_range(range.start()..range.end())
            .map(|chunk| chunk.text)
            .collect())
    }

    /// 返回**包含**组合偏移的文本块及其组合坐标起点。
    ///
    /// 注意与 `bytes_in_range(offset..)` 的区别：后者从偏移处切开块，本方法返回偏移所在的完整块。
    pub fn chunk_at_byte(
        &self,
        offset: MultiBufferOffset,
    ) -> TextResult<(&str, MultiBufferOffset)> {
        let (entry, at, source, source_offset) = self.source_point_at_byte(offset)?;
        let content_start = ByteOffset::new(at.bytes);
        let content_end = ByteOffset::new(content_start.get() + entry.source_range.len());
        if content_start <= offset.into() && offset < content_end.into() {
            let (chunk, source_chunk_start) = source.text.chunk_at_byte(source_offset)?;
            // 源 chunk 可能起始于片段之前（或越过片段末尾）：裁剪到片段范围内，
            // 组合文本只投影片段内容。
            let chunk_end = ByteOffset::new(source_chunk_start.get() + chunk.len());
            let slice_start = source_chunk_start.max(entry.source_range.start());
            let slice_end = chunk_end.min(entry.source_range.end());
            let text = &chunk[slice_start.get() - source_chunk_start.get()
                ..slice_end.get() - source_chunk_start.get()];
            let output_chunk_start = MultiBufferOffset::new(
                content_start.get() + (slice_start.get() - entry.source_range.start().get()),
            );
            return Ok((text, output_chunk_start));
        }
        // 片段之间的静态换行块或文档末尾：退回按偏移切分的块。
        self.bytes_in_range(offset..self.len_bytes())
            .next()
            .map(|chunk| (chunk.text, chunk.output_range.start))
            .ok_or(CoordinateError::OutOfBounds(offset.into()).into())
    }

    /// 把组合逻辑位置转换为字节偏移。
    ///
    /// 按完整行列定位输出变换与逻辑窗口，再由同版本源快照解析区域内的列。
    pub fn position_to_byte(&self, position: Position) -> TextResult<MultiBufferOffset> {
        if position.line().get() >= self.line_count() {
            return Err(CoordinateError::LineOutOfBounds(position.line()).into());
        }
        if self.excerpts.is_empty() {
            if position == Position::new(Line::ZERO, LogicalColumn::ZERO) {
                return Ok(ByteOffset::ZERO.into());
            }
            return Err(CoordinateError::OutOfBounds(ByteOffset::ZERO).into());
        }

        let mut cursor = MultiBufferCursor::new(&self.excerpts, &self.diff_transforms);
        cursor.seek_output_position(position, Bias::Right);
        if cursor.item().is_none() {
            cursor.prev();
        }
        let (excerpt, _) = cursor
            .item()
            .ok_or(CoordinateError::LineOutOfBounds(position.line()))?;
        let at = cursor.start();
        let row_delta = position.line().get() - at.lines;
        if position.line().get() == self.diff_transforms.summary().output.lines
            && position.column() == LogicalColumn::ZERO
            && excerpt.adds_newline
            && row_delta == excerpt.text_summary.lines + 1
        {
            return Ok(self.len_bytes());
        }
        let source = self
            .source_snapshot(excerpt.source_index)
            .ok_or(CoordinateError::LineOutOfBounds(position.line()))?;
        let source_line = excerpt.source_start_line + row_delta;
        let source_column = if row_delta == 0 {
            let source_start = source.text.byte_to_position(excerpt.source_range.start())?;
            source_start.column().get() + position.column().get() - at.column_chars
        } else {
            position.column().get()
        };
        let source_offset = source.text.position_to_byte(Position::new(
            Line::new(source_line),
            LogicalColumn::new(source_column),
        ))?;
        if source_offset > excerpt.source_range.end() {
            return Err(CoordinateError::OutOfBounds(self.len_bytes().into()).into());
        }
        Ok(
            ByteOffset::new(at.bytes + source_offset.get() - excerpt.source_range.start().get())
                .into(),
        )
    }

    pub fn byte_to_char(&self, offset: MultiBufferOffset) -> TextResult<CharOffset> {
        if offset == self.len_bytes() {
            return Ok(CharOffset::new(self.diff_transforms.summary().output.chars));
        }
        let (entry, at, source, source_offset) = self.source_point_at_byte(offset)?;
        let source_start = source.text.byte_to_char(entry.source_range.start())?;
        let source_char = source.text.byte_to_char(source_offset)?;
        Ok(CharOffset::new(
            at.chars + source_char.get() - source_start.get(),
        ))
    }

    pub fn char_to_byte(&self, target: CharOffset) -> TextResult<MultiBufferOffset> {
        let total = self.diff_transforms.summary().output.chars;
        if target.get() > total {
            return Err(CoordinateError::CharOutOfBounds(target).into());
        }
        if target.get() == total {
            return Ok(self.len_bytes());
        }
        let mut cursor = MultiBufferCursor::new(&self.excerpts, &self.diff_transforms);
        cursor.seek_output_char(target, Bias::Right);
        let (excerpt, _) = cursor
            .item()
            .ok_or(CoordinateError::CharOutOfBounds(target))?;
        let at = cursor.start();
        let source = self
            .source_snapshot(excerpt.source_index)
            .ok_or(CoordinateError::CharOutOfBounds(target))?;
        let source_start = source.text.byte_to_char(excerpt.source_range.start())?;
        let source_char = source_start.get() + target.get() - at.chars;
        let source_offset = source.text.char_to_byte(CharOffset::new(source_char))?;
        if source_offset > excerpt.source_range.end() {
            return Err(CoordinateError::CharOutOfBounds(target).into());
        }
        Ok(
            ByteOffset::new(at.bytes + source_offset.get() - excerpt.source_range.start().get())
                .into(),
        )
    }

    pub fn movement_boundary(
        &self,
        offset: CharOffset,
        direction: MovementDirection,
        unit: MovementUnit,
    ) -> TextResult<CharOffset> {
        // 与单 Buffer 共用同一份文本移动语义，组合文档不得另实现一套边界规则。
        let byte = self.char_to_byte(offset)?;
        let policy = self.word_boundary_at(byte);
        zcv_text::movement_boundary_in_text(self, policy, offset, direction, unit)
    }

    /// 返回包含当前位置的词边界。组合文本沿连续 chunk 读取，不构造临时字符串。
    pub fn surrounding_word(&self, offset: CharOffset) -> TextResult<(CharOffset, CharOffset)> {
        let offset = self.char_to_byte(offset)?;
        let policy = self.word_boundary_at(offset);
        let is_word = |byte: ByteOffset| {
            self.char_at_byte(byte)
                .is_some_and(|character| policy.is_identifier_continue(character))
        };
        let mut start = offset;
        while start > ByteOffset::ZERO.into() {
            let previous = self.previous_grapheme_boundary(start.into())?;
            if !is_word(previous) {
                break;
            }
            start = previous.into();
        }
        let mut end = offset;
        while end < self.len_bytes() && is_word(end.into()) {
            end = self.next_grapheme_boundary(end.into())?.into();
        }
        Ok((self.byte_to_char(start)?, self.byte_to_char(end)?))
    }

    pub fn is_inside_word(&self, offset: CharOffset) -> TextResult<bool> {
        let offset = self.char_to_byte(offset)?;
        let policy = self.word_boundary_at(offset);
        Ok(self
            .char_at_byte(offset.into())
            .is_some_and(|character| policy.is_identifier_continue(character)))
    }

    pub fn byte_to_utf16_cu(&self, offset: MultiBufferOffset) -> TextResult<Utf16Offset> {
        if offset == self.len_bytes() {
            return Ok(Utf16Offset::new(
                self.diff_transforms.summary().output.len_utf16,
            ));
        }
        let (entry, at, source, source_offset) = self.source_point_at_byte(offset)?;
        let source_start = source.text.byte_to_utf16_cu(entry.source_range.start())?;
        let source_units = source.text.byte_to_utf16_cu(source_offset)?;
        Ok(Utf16Offset::new(
            at.utf16 + source_units.get() - source_start.get(),
        ))
    }

    pub fn utf16_cu_to_byte(&self, target: Utf16Offset) -> TextResult<MultiBufferOffset> {
        let total = self.diff_transforms.summary().output.len_utf16;
        if target.get() > total {
            return Err(
                CoordinateError::Utf16PositionOutOfBounds(Utf16Position::new(Line::ZERO, target))
                    .into(),
            );
        }
        if target.get() == total {
            return Ok(self.len_bytes());
        }
        let mut cursor = MultiBufferCursor::new(&self.excerpts, &self.diff_transforms);
        cursor.seek_output_utf16(target, Bias::Right);
        let (excerpt, _) = cursor
            .item()
            .ok_or(CoordinateError::Utf16PositionOutOfBounds(
                Utf16Position::new(Line::ZERO, target),
            ))?;
        let at = cursor.start();
        let source = self.source_snapshot(excerpt.source_index).ok_or(
            CoordinateError::Utf16PositionOutOfBounds(Utf16Position::new(Line::ZERO, target)),
        )?;
        let source_start = source.text.byte_to_utf16_cu(excerpt.source_range.start())?;
        let source_units = source_start.get() + target.get() - at.utf16;
        let source_offset = source
            .text
            .utf16_cu_to_byte(Utf16Offset::new(source_units))?;
        if source_offset > excerpt.source_range.end() {
            return Err(
                CoordinateError::Utf16PositionOutOfBounds(Utf16Position::new(Line::ZERO, target))
                    .into(),
            );
        }
        Ok(
            ByteOffset::new(at.bytes + source_offset.get() - excerpt.source_range.start().get())
                .into(),
        )
    }

    /// 从组合偏移范围连续借用文本块。
    ///
    /// 虚拟 MultiBuffer 文本入口：
    /// 消费者按输出坐标请求范围，游标通过 excerpt 映射定位源快照，再直接返回 Rope chunk 的子切片。
    /// 片段间为保持行边界注入的换行也以静态借用块返回。
    pub fn bytes_in_range(&self, range: Range<MultiBufferOffset>) -> MultiBufferBytes<'_> {
        let end = range.end.min(self.len_bytes());
        let start = range.start.min(end);
        let mut cursor = MultiBufferCursor::new(&self.excerpts, &self.diff_transforms);
        cursor.seek_output(start.into(), Bias::Right);
        MultiBufferBytes {
            snapshot: self,
            range: start.into()..end.into(),
            cursor,
            offset: start.into(),
        }
    }

    fn source_point_at_byte(
        &self,
        offset: MultiBufferOffset,
    ) -> TextResult<(
        ExcerptCoordinates,
        MappingPosition,
        &ExcerptSourceSnapshot,
        ByteOffset,
    )> {
        if offset > self.len_bytes() {
            return Err(CoordinateError::OutOfBounds(offset.into()).into());
        }
        let mut cursor = MultiBufferCursor::new(&self.excerpts, &self.diff_transforms);
        cursor.seek_output(offset.into(), Bias::Right);
        self.source_point_at_cursor(&mut cursor, offset)
    }

    fn source_point_at_cursor(
        &self,
        cursor: &mut MultiBufferCursor<'_>,
        offset: MultiBufferOffset,
    ) -> TextResult<(
        ExcerptCoordinates,
        MappingPosition,
        &ExcerptSourceSnapshot,
        ByteOffset,
    )> {
        if offset > self.len_bytes() {
            return Err(CoordinateError::OutOfBounds(offset.into()).into());
        }
        if cursor.item().is_none() {
            cursor.prev();
        }
        let (excerpt, _) = cursor
            .item()
            .ok_or(CoordinateError::OutOfBounds(offset.into()))?;
        let entry = ExcerptCoordinates {
            source_index: excerpt.source_index,
            source_range: excerpt.source_range.range(),
            source_start_line: excerpt.source_start_line,
            adds_newline: excerpt.adds_newline,
        };
        let at = cursor.start().clone();
        let source = self
            .source_snapshot(entry.source_index)
            .ok_or(CoordinateError::OutOfBounds(offset.into()))?;
        let relative = offset
            .get()
            .saturating_sub(at.bytes)
            .min(entry.source_range.len());
        let source_offset = ByteOffset::new(entry.source_range.start().get() + relative);
        source
            .text
            .chunk_at_byte(source_offset)
            .map_err(|_| CoordinateError::InvalidByteBoundary(offset.into()))?;
        Ok((entry, at, source, source_offset))
    }

    fn ensure_output_boundary(&self, offset: MultiBufferOffset) -> TextResult<()> {
        if offset > self.len_bytes() {
            return Err(CoordinateError::OutOfBounds(offset.into()).into());
        }
        if offset == self.len_bytes()
            || (offset == ByteOffset::ZERO.into() && self.excerpts.is_empty())
        {
            return Ok(());
        }
        self.source_point_at_byte(offset).map(|_| ())
    }

    pub fn version(&self) -> BufferVersion {
        self.projection_version
    }

    /// 当前组合快照绑定的 diff 显示输入。
    ///
    /// `None` 表示普通文档没有 diff 装饰；调用方不得为此建立空的并列缓存。
    pub fn diff_display(&self) -> Option<&Arc<DiffDisplaySnapshot>> {
        self.diff_display.as_ref()
    }

    /// 组合片段边界的拓扑版本。
    ///
    /// 源文本编辑只推进 `version`；只有增删片段或改变逻辑 excerpt 边界时才推进本版本。
    /// 显示块据此复用边界分类，不把普通行内编辑误当成组合结构变化。
    pub fn topology_version(&self) -> u64 {
        self.topology_version
    }

    /// 从逻辑窗口与输出变换联合派生源读取区域；输出坐标由累积摘要提供。
    ///
    /// 组合坐标查询直接用树游标；本方法只在需要随机访问或移交所有权时调用。
    pub fn regions(&self) -> impl Iterator<Item = ExcerptSnapshot> + '_ {
        let mut cursor = MultiBufferCursor::new(&self.excerpts, &self.diff_transforms);
        // 用 Left 偏置从零长度边界节点开始遍历：空文件的零长度 excerpt 不能被跳过。
        cursor.seek_output(ByteOffset::ZERO, Bias::Left);
        std::iter::from_fn(move || {
            let (excerpt, _) = cursor.item()?;
            let snapshot = excerpt.to_snapshot(cursor.start().clone());
            cursor.next();
            Some(snapshot)
        })
    }

    /// 按输入顺序遍历稳定逻辑 excerpts；diff 展开不会增删这些窗口。
    pub fn excerpts(&self) -> impl Iterator<Item = ExcerptSnapshot> + '_ {
        let mut cursor = self.excerpts.cursor::<ExcerptSummary>(());
        cursor.next();
        std::iter::from_fn(move || {
            let excerpt = cursor.item()?;
            let start = cursor.start().text.len;
            let snapshot = self.logical_excerpt_snapshot(excerpt, start);
            cursor.next();
            Some(snapshot)
        })
    }

    fn logical_excerpt_snapshot(&self, excerpt: &Excerpt, input_start: usize) -> ExcerptSnapshot {
        let mut start_cursor = self.diff_transforms.cursor::<MappingPosition>(());
        start_cursor.seek(&ExcerptOffset::new(input_start), Bias::Left);
        let start = start_cursor.start().bytes
            + input_start.saturating_sub(start_cursor.start().input_offset.get());
        let end = if excerpt.adds_newline {
            let input_end = input_start + diff_output_text(excerpt).len;
            let mut end_cursor = self.diff_transforms.cursor::<MappingPosition>(());
            end_cursor.seek(&ExcerptOffset::new(input_end), Bias::Left);
            end_cursor.start().bytes
                + input_end.saturating_sub(end_cursor.start().input_offset.get())
        } else {
            // 最后一个窗口延伸到输出文尾，包含输入文尾的零长度删除变换。
            self.len_bytes().get()
        };
        let mut snapshot = excerpt.output_region().to_snapshot(MappingPosition {
            bytes: start,
            lines: self
                .byte_to_line(MultiBufferOffset::new(start))
                .expect("excerpt 输出起点必须有效")
                .get(),
            ..MappingPosition::default()
        });
        snapshot.output_range =
            MultiBufferRange::new(start, end).expect("逻辑 excerpt 输出范围必须正序");
        snapshot.output_end_line = self
            .byte_to_line(MultiBufferOffset::new(end))
            .expect("excerpt 输出终点必须有效")
            .get();
        snapshot
    }

    /// 整个组合文档的显示策略：新 Buffer 边界绘制实体 header 还是 divider。
    ///
    /// 它是快照级策略，不进入任何 excerpt 的身份或边界判定；
    /// `BlockMap` 是当前唯一消费者。
    pub fn show_headers(&self) -> bool {
        self.show_headers
    }

    /// 按组合顺序遍历逻辑 excerpt 边界。
    ///
    /// Header、divider 与整文件折叠都必须消费此边界流；
    /// 边界由权威逻辑窗口派生，与输出变换的分段无关。
    /// 单文件文档没有边界，缺口由 `singleton` 直接关闭。
    pub fn excerpt_boundaries(&self) -> impl Iterator<Item = ExcerptBoundary> + '_ {
        let singleton = self.singleton;
        let mut previous = None;
        self.excerpts()
            .enumerate()
            .filter_map(move |(next_index, next)| {
                if singleton {
                    return None;
                }
                let boundary = ExcerptBoundary {
                    previous: previous.clone(),
                    next: next.clone(),
                    next_index,
                };
                previous = Some(next);
                Some(boundary)
            })
    }

    /// 按逻辑路径定位输出读取区域，不扫描其它文件。
    pub fn regions_for_path(&self, path: &Path) -> impl Iterator<Item = ExcerptSnapshot> {
        let path_key = PathKey::new(path);
        let mut cursor = MultiBufferCursor::new(&self.excerpts, &self.diff_transforms);
        cursor.seek_path(&path_key, Bias::Left);
        let mut snapshots = Vec::new();
        while let Some((excerpt, _)) = cursor.item() {
            if excerpt.path != path_key {
                break;
            }
            snapshots.push(excerpt.to_snapshot(cursor.start().clone()));
            cursor.next();
        }
        snapshots.into_iter()
    }

    /// 按路径定位逻辑窗口，不扫描其它文件。
    pub fn excerpts_for_path(&self, path: &Path) -> impl Iterator<Item = ExcerptSnapshot> {
        let path = PathKey::new(path);
        let mut cursor = self.excerpts.cursor::<ExcerptSummary>(());
        cursor.seek(&path, Bias::Left);
        let mut snapshots = Vec::new();
        while let Some(excerpt) = cursor.item() {
            if excerpt.path != path {
                break;
            }
            snapshots.push(self.logical_excerpt_snapshot(excerpt, cursor.start().text.len));
            cursor.next();
        }
        snapshots.into_iter()
    }

    pub fn excerpt_at_index(&self, index: usize) -> Option<ExcerptSnapshot> {
        let mut cursor = self.excerpts.cursor::<ExcerptSummary>(());
        cursor.seek(&ExcerptIndex(index), Bias::Right);
        let excerpt = cursor.item()?;
        Some(self.logical_excerpt_snapshot(excerpt, cursor.start().text.len))
    }

    /// 组合输出偏移所在的 excerpt（用累积输出字节的偏移游标在路径有序树上定位）。
    pub fn region_at_output_offset(&self, offset: MultiBufferOffset) -> Option<ExcerptSnapshot> {
        let mut cursor = MultiBufferCursor::new(&self.excerpts, &self.diff_transforms);
        cursor.seek_output(offset.into(), Bias::Right);
        let (excerpt, _) = cursor.item()?;
        Some(excerpt.to_snapshot(cursor.start().clone()))
    }

    /// 把快照内的组合偏移锚定到底层源坐标（Editor 源锚点选区：投影→源）。
    pub fn anchor_at(
        &self,
        offset: impl Into<MultiBufferOffset>,
        affinity: Affinity,
    ) -> MultiBufferAnchor {
        let offset: MultiBufferOffset = offset.into();
        anchor_in_mappings(
            &self.excerpts,
            &self.diff_transforms,
            &self.excerpt_sources,
            offset.into(),
            affinity,
        )
        .unwrap_or_else(|| MultiBufferAnchor::boundary(offset))
    }

    /// 把稳定锚点定位到当前组合坐标。
    ///
    /// 这是位置状态（选择、滚动）的总解析：
    /// 源或 excerpt 退出当前投影时，按路径顺序定位到重建后结构中的相邻边界；空投影则落在文首。
    /// 源 Anchor 版本无法推进到当前源快照仍是版本链错误，必须显式处理。
    pub fn anchor_offset(&self, anchor: &MultiBufferAnchor) -> TextResult<MultiBufferOffset> {
        resolve_anchor_in_mappings(
            &self.excerpts,
            &self.diff_transforms,
            &self.path_keys,
            &self.excerpt_sources,
            anchor,
        )
        .map(|resolution| resolution.offset().into())
    }

    /// 只在锚点仍属于当前可见源片段时返回组合坐标。
    ///
    /// 折叠、搜索命中和自动闭合等附属状态不能在源退出投影后迁移到相邻文件，因此与位置状态使用不同的解析语义。
    pub fn projected_anchor_offset(
        &self,
        anchor: &MultiBufferAnchor,
    ) -> TextResult<Option<MultiBufferOffset>> {
        resolve_anchor_in_mappings(
            &self.excerpts,
            &self.diff_transforms,
            &self.path_keys,
            &self.excerpt_sources,
            anchor,
        )
        .map(|resolution| resolution.projected_offset().map(Into::into))
    }
    pub fn capture_names(&self) -> Arc<[Arc<str>]> {
        Arc::clone(&self.capture_names)
    }

    /// 查询组合坐标中的语法高亮，并把每个源 Buffer 的 capture index 映射到本快照的统一表。
    ///
    pub fn highlights(&self, range: std::ops::Range<usize>) -> Vec<HighlightSpan> {
        let mut spans = Vec::new();
        // 片段按输出顺序排列，内容结束位置单调递增；
        // 先用输出字节游标定位第一个可能重叠的片段，避免每个视口范围都扫描整份多文件结果。
        let mut cursor = MultiBufferCursor::new(&self.excerpts, &self.diff_transforms);
        cursor.seek_output(ByteOffset::new(range.start), Bias::Right);
        while let Some((excerpt, _)) = cursor.item() {
            let output_start = cursor.start().bytes;
            if output_start >= range.end {
                break;
            }
            // 非末尾 excerpt 可能为显示边界补一个换行；该字节不属于 source，
            // 不能越过 source_range 去查询下一段源文本的语法。
            let output_end = output_start + excerpt.source_range.len();
            let start = range.start.max(output_start);
            let end = range.end.min(output_end);
            if start < end {
                let source = self
                    .source_snapshot(excerpt.source_index)
                    .expect("excerpt 必须引用已注册的源快照");
                let source_start = excerpt.source_range.start().get() + start - output_start;
                let source_end = excerpt.source_range.start().get() + end - output_start;
                let source_offset = excerpt.source_range.start().get();
                spans.extend(
                    source
                        .syntax
                        .highlights(
                            source_start..source_end,
                            &source.text,
                            &source.highlight_cache,
                        )
                        .into_iter()
                        .filter_map(|span| {
                            let capture = *source.capture_map.get(span.capture as usize)?;
                            Some(HighlightSpan {
                                range: (output_start + span.range.start - source_offset)
                                    ..(output_start + span.range.end - source_offset),
                                capture,
                            })
                        }),
                );
            }
            cursor.next();
        }
        spans
    }

    /// 返回组合输出位置对应的源快照与坐标映射。
    ///
    /// 删除 hunk 对应只读基线源；excerpt 间补充换行按边界规则关联相邻源。
    pub fn source_at(&self, offset: impl Into<MultiBufferOffset>) -> Option<MultiBufferSource<'_>> {
        let (mapping, _, source_offset) =
            self.source_point(ByteOffset::new(offset.into().get()))?;
        Some(MultiBufferSource {
            snapshot: self,
            mapping,
            source_offset,
        })
    }

    pub fn bracket_pairs_at(&self, offset: impl Into<MultiBufferOffset>) -> Vec<BracketPair> {
        let offset: MultiBufferOffset = offset.into();
        let offset = ByteOffset::new(offset.get());
        let Some((mapping, source, source_offset)) = self.source_point(offset) else {
            return Vec::new();
        };
        let excerpt_start = mapping.source_range.start().get();
        let excerpt_end = mapping.source_range.end().get();
        let query_start = source_offset.get().saturating_sub(1).max(excerpt_start);
        let query_end = source_offset.get().saturating_add(1).min(excerpt_end);
        source
            .syntax
            .bracket_pairs(query_start..query_end, &source.text)
            .into_iter()
            .filter(|pair| {
                pair.open.start >= excerpt_start
                    && pair.close.end <= excerpt_end
                    && pair.open.start < pair.open.end
                    && pair.close.start < pair.close.end
            })
            .map(|pair| {
                let output_start = mapping.output_range.start().get();
                BracketPair {
                    open: (output_start + pair.open.start - excerpt_start)
                        ..(output_start + pair.open.end - excerpt_start),
                    close: (output_start + pair.close.start - excerpt_start)
                        ..(output_start + pair.close.end - excerpt_start),
                }
            })
            .collect()
    }

    /// 查询组合坐标中光标所在 source 的换行缩进建议。
    pub fn suggested_newline_indent(
        &self,
        offset: impl Into<MultiBufferOffset>,
    ) -> TextResult<NewlineIndent> {
        let offset: MultiBufferOffset = offset.into();
        let offset = ByteOffset::new(offset.get());
        let Some((_, source, source_offset)) = self.source_point(offset) else {
            return Ok(NewlineIndent {
                base_indent: String::new(),
                additional_levels: 0,
            });
        };
        source
            .syntax
            .suggested_newline_indent(source_offset, &source.text)
    }

    /// 查询严格包围组合范围的最小 source 语法节点，并映射回组合坐标。
    pub fn ancestor_range(&self, range: std::ops::Range<usize>) -> Option<std::ops::Range<usize>> {
        self.expand_selection_range(range)
    }

    /// 返回组合坐标中光标所在的最深语法节点。
    pub fn node_at(&self, offset: impl Into<MultiBufferOffset>) -> Option<SyntaxNode> {
        let offset: MultiBufferOffset = offset.into();
        let offset = ByteOffset::new(offset.get());
        let (mapping, source, source_offset) = self.source_point(offset)?;
        let node = source.syntax.node_at(source_offset.get(), &source.text)?;
        project_syntax_node(&node, mapping.source_range.range(), mapping.output_range)
    }

    /// 返回组合坐标中选区所在语法层的节点链，顺序为最小节点到语法根节点。
    pub fn node_ancestors(&self, range: std::ops::Range<usize>) -> Vec<SyntaxNode> {
        let Some((mapping, source, source_range)) = self.source_range(range.clone()) else {
            return Vec::new();
        };
        source
            .syntax
            .node_ancestors(source_range, &source.text)
            .into_iter()
            .filter_map(|node| {
                project_syntax_node(&node, mapping.source_range.range(), mapping.output_range)
            })
            .collect()
    }

    /// 将选区扩展到当前语法层中严格包围它的下一个节点。
    pub fn expand_selection_range(
        &self,
        range: std::ops::Range<usize>,
    ) -> Option<std::ops::Range<usize>> {
        let (mapping, source, source_range) = self.source_range(range.clone())?;
        let ancestor = source
            .syntax
            .expand_selection_range(source_range, &source.text)?;
        project_range(ancestor, mapping.source_range.range(), mapping.output_range)
    }

    /// 返回当前组合文档中可见源范围内的文件大纲项。
    ///
    /// 大纲先从每个源的 `SyntaxSnapshot` 计算，再只投影完整落在 excerpt 内的定义；
    /// 这样不会把跨未展示内容的语法节点误投影到差异或搜索组合文档中。
    pub fn outline_items(&self) -> Vec<OutlineItem> {
        let source_outlines = self
            .excerpt_sources
            .values()
            .map(|source| {
                source
                    .syntax
                    .outline(0..source.text.len_bytes().get(), &source.text)
            })
            .collect::<Vec<_>>();
        let mut projected = Vec::new();
        let mut cursor = MultiBufferCursor::new(&self.excerpts, &self.diff_transforms);
        cursor.seek_output(ByteOffset::ZERO, Bias::Right);
        while let Some((excerpt, _)) = cursor.item() {
            let mapping = excerpt.to_mapping(cursor.start());
            if let Some(outlines) = source_outlines.get(mapping.source_index) {
                for item in outlines.iter().filter_map(|item| {
                    project_outline_item(item, mapping.source_range.range(), mapping.output_range)
                }) {
                    projected.push(item);
                }
            }
            cursor.next();
        }
        projected.sort_unstable_by_key(|item| (item.range.start, item.range.end));
        projected.dedup_by(|left, right| {
            left.range == right.range
                && left.name_range == right.name_range
                && left.name == right.name
                && left.language == right.language
        });
        projected
    }

    /// 返回普通单文件组合文档中的局部绑定。
    ///
    /// 局部绑定仍以源文件字节范围为语义；
    /// 多文件 excerpt 可能只展示作用域的一部分，因而这里不返回不完整的绑定，避免编辑器把组合坐标误当成源坐标执行重命名。
    pub fn local_bindings(&self) -> Vec<LocalBinding> {
        if self.excerpts.summary().count != 1 {
            return Vec::new();
        }
        let Some(excerpt) = self.excerpts.first() else {
            return Vec::new();
        };
        let Some(source) = self.source_snapshot(excerpt.source_index) else {
            return Vec::new();
        };
        let source_len = source.text.len_bytes().get();
        if excerpt.source_range.start().get() != 0 || excerpt.source_range.end().get() != source_len
        {
            return Vec::new();
        }
        source.syntax.local_bindings(0..source_len, &source.text)
    }

    fn source_point(
        &self,
        offset: ByteOffset,
    ) -> Option<(ExcerptMapping, &ExcerptSourceSnapshot, ByteOffset)> {
        let (mapping, at) = mapping_at_tree(&self.excerpts, &self.diff_transforms, offset)?;
        let source = self.source_snapshot(mapping.source_index)?;
        let delta = offset
            .get()
            .saturating_sub(at.bytes)
            .min(mapping.source_range.len());
        let source_offset = ByteOffset::new(mapping.source_range.start().get() + delta);
        Some((mapping, source, source_offset))
    }

    fn source_range(
        &self,
        range: std::ops::Range<usize>,
    ) -> Option<(
        ExcerptMapping,
        &ExcerptSourceSnapshot,
        std::ops::Range<usize>,
    )> {
        let (mapping, at) = mapping_at_tree(
            &self.excerpts,
            &self.diff_transforms,
            ByteOffset::new(range.start),
        )?;
        let output_start = at.bytes;
        let content_end = output_start + mapping.source_range.len();
        if range.start < output_start || range.end > content_end {
            return None;
        }
        let source = self.source_snapshot(mapping.source_index)?;
        let source_start = mapping.source_range.start().get() + range.start - output_start;
        let source_end = mapping.source_range.start().get() + range.end - output_start;
        Some((mapping, source, source_start..source_end))
    }

    pub fn excerpt_for_output_line(&self, line: usize) -> Option<ExcerptSnapshot> {
        let (entry, at) = mapping_at_output_line(&self.excerpts, &self.diff_transforms, line)?;
        let excerpt = entry.entry.to_snapshot(at);
        ((excerpt.output_start_line <= line && line < excerpt.output_end_line)
            || (excerpt.output_start_line == excerpt.output_end_line
                && line == excerpt.output_start_line))
            .then_some(excerpt)
    }
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

    fn byte_to_utf16_position(&self, offset: ByteOffset) -> TextResult<Utf16Position> {
        if offset == self.len_bytes().into() {
            let text = self.diff_transforms.summary().output;
            return Ok(Utf16Position::new(
                Line::new(text.lines),
                Utf16Offset::new(text.last_line_len_utf16),
            ));
        }
        let (entry, at, source, source_offset) = self.source_point_at_byte(offset.into())?;
        let start = source
            .text
            .byte_to_utf16_position(entry.source_range.start())?;
        let position = source.text.byte_to_utf16_position(source_offset)?;
        let rows = position.line().get() - entry.source_start_line;
        let column = if rows == 0 {
            at.column_utf16 + position.character().get() - start.character().get()
        } else {
            position.character().get()
        };
        Ok(Utf16Position::new(
            Line::new(at.lines + rows),
            Utf16Offset::new(column),
        ))
    }

    fn utf16_position_to_byte(&self, position: Utf16Position) -> TextResult<ByteOffset> {
        let start = self.line_start_byte(position.line())?;
        let units = self.byte_to_utf16_cu(start)?.get() + position.character().get();
        let offset = self.utf16_cu_to_byte(Utf16Offset::new(units))?;
        if TextRead::byte_to_utf16_position(self, offset.into())? == position {
            Ok(offset.into())
        } else {
            Err(CoordinateError::Utf16PositionOutOfBounds(position).into())
        }
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
) -> Option<OutlineItem> {
    let project = |range: &std::ops::Range<usize>| {
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
        version: item.version,
        range: project(&item.range)?,
        name_range: project(&item.name_range)?,
        name: item.name.clone(),
        text: item.text.clone(),
        text_ranges: item
            .text_ranges
            .iter()
            .map(|part| {
                Some(OutlineTextRange {
                    text_range: part.text_range.clone(),
                    source_range: project(&part.source_range)?,
                })
            })
            .collect::<Option<Vec<_>>>()?,
        kind: item.kind.clone(),
        depth: item.depth,
        language: item.language,
        language_depth: item.language_depth,
        body_range: item.body_range.as_ref().and_then(project),
        annotation_range: item.annotation_range.as_ref().and_then(project),
    })
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
    range: std::ops::Range<usize>,
    source_range: TextRange,
    output_range: MultiBufferRange,
) -> Option<std::ops::Range<usize>> {
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
            source_range: ExcerptContext::new(text.version(), range, false),
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
            excerpts: SumTree::from_iter([excerpt], ()),
            diff_transforms,
            path_keys: Arc::from([PathKey::min()]),
            excerpt_sources: TreeMap::from_ordered_entries([(
                0,
                ExcerptSourceSnapshot {
                    text,
                    syntax,
                    highlight_cache: Arc::new(HighlightCache::new()),
                    word_boundary: WordBoundaryPolicy::default(),
                    settings: Arc::new(LanguageSettings::default()),
                    capture_map: Arc::from([]),
                },
            )]),
            source_indices: Arc::new(HashMap::new()),
            capture_names,
            metadata_version: 0,
            diff_display: None,
            show_headers: true,
            singleton: true,
        }
    }
}

impl MultiBufferSnapshot {
    fn empty() -> Self {
        let text = Buffer::from_text(String::new(), BufferConfig::default())
            .expect("空组合快照的临时文本必须可以创建")
            .snapshot();
        Self::from(text)
    }
}

/// 取源快照对应语言的词边界策略；未识别语言回退默认策略。
fn snapshot_word_boundary(snapshot: &LanguageBufferSnapshot) -> WordBoundaryPolicy {
    snapshot
        .language
        .as_ref()
        .map_or_else(WordBoundaryPolicy::default, |language| {
            language.word_boundary()
        })
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
    source_indices: HashMap<gpui::EntityId, usize>,
    /// 路径身份表：索引一经分配不再变化，锚点用索引做紧凑表示。
    path_keys: Vec<PathKey>,
    path_key_indices: HashMap<PathKey, PathKeyIndex>,
    capture_names: Arc<[Arc<str>]>,
    projection_version: BufferVersion,
    topology_version: u64,
    /// 非文本状态（语法安装、捕获表等）版本；纯文本编辑不推进它。
    metadata_epoch: u64,
    /// 源事件只登记脏源；组合快照读取时才消费这些源的订阅。
    pending_source_syncs: HashMap<gpui::EntityId, PendingSourceSync>,
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
    pub fn is_read_only(self) -> bool {
        matches!(self, Capability::ReadOnly)
    }
}

/// Editor 持有的组合文档模型。
///
/// 恒为 excerpts 形态；普通编辑器是整文件单 excerpt，与多文件文档共用同一套显示、编辑与历史链路。
pub struct MultiBuffer {
    state: ExcerptState,
    capability: Capability,
    /// 普通整文件文档的稳定角色与权威源。
    ///
    /// excerpts 会因 diff 展开而改变形状，不能据此推断文档角色；
    /// 历史、配置、重命名与保存等文件级事实按本字段委托给底层 LanguageBuffer。
    /// `None` 表示真正的多来源组合文档。
    singleton_source: Option<Entity<LanguageBuffer>>,
    /// 显式标题；`None` 时由文档身份派生（当前文件路径的文件名）。
    title: Option<String>,
    /// 按显示路径排序的每文件 diff 状态（diff 实体、显示配置与展开覆盖）。
    diffs: Vec<diff_projection::DiffState>,
    /// git 行级 diff 显示输入（hunks、跟踪区间与显示坐标）；`None` = 无 diff 需求。
    /// 每次真实几何变化以写时复制替换，已发布快照继续持有旧输入。
    diff: Option<Arc<DiffDisplaySnapshot>>,
    /// 新 hunk 的初始展开策略；只决定初始状态，不覆盖用户显式切换。
    diff_expanded_by_default: bool,
    /// 已物化进组合文档的前导文件数量（diff 以路径顺序登记，就绪前缀之外的文件尚未物化）。
    diff_materialized_files: usize,
    /// 当前一致的组合快照；只由 MultiBuffer 的同步入口替换。
    snapshot: MultiBufferSnapshot,
    /// 当前快照是否需要从可变组合状态重新对齐。
    snapshot_dirty: bool,
    /// `None` 表示源集合拓扑变化，需要重建源表；`Some` 只记录待替换的源索引。
    /// 源表仍会与 excerpts、diff transforms 在同一快照帧提交。
    snapshot_source_updates: Option<HashSet<usize>>,
    /// 当前是否处于源与 diff 的同一同步帧；存在时不提前发布投影版本。
    projection_sync: Option<ProjectionSync>,
    /// 整个组合文档的显示策略：新 Buffer 边界绘制实体 header 还是 divider。
    /// 它只由构造入口确定，不属于任何单个 diff 文件的物化条件。
    show_headers: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HistoryOwner {
    /// 单文件编辑器的多个视图共享同一个工作区源，因此历史归源 Buffer 所有。
    SourceBuffer,
    /// 组合文档同时编辑多个源，由 MultiBuffer 记录一次复合事务。
    MultiBuffer,
}

impl EventEmitter<MultiBufferEvent> for MultiBuffer {}

impl MultiBuffer {
    /// 统一选择文本历史的所有者。
    ///
    /// 同一个文档模型根据源数量选择历史协调者：
    /// 单源文档复用共享源历史，多源文档由自身协调多个源的历史。
    fn history_owner(&self) -> HistoryOwner {
        if self.singleton_source.is_some() {
            HistoryOwner::SourceBuffer
        } else {
            HistoryOwner::MultiBuffer
        }
    }

    /// 创建空的可编辑组合文档；调用方可重复设置 ordered excerpts。
    pub fn empty(cx: &mut Context<Self>) -> Self {
        Self::empty_with_capability(Capability::ReadWrite, cx)
    }

    /// 从工作区源构建独立的组合文档（整文件可编辑 excerpt）。
    ///
    /// 普通编辑器的文档统一经此构造：项目共享 LanguageBuffer 只作为工作区源（source），展开 diff hunk 时的 set_excerpts_for_path 只影响本组合文档，不污染项目共享文档。
    /// 单文件文档没有多文件边界，快照据此标记 singleton，显示层不绘制 header/divider。
    pub fn singleton(source: Entity<LanguageBuffer>, cx: &mut Context<Self>) -> Self {
        let line_count = source.read(cx).text_snapshot().line_count();
        let mut multi_buffer = Self::empty(cx);
        multi_buffer.singleton_source = Some(source.clone());
        multi_buffer.set_excerpts_for_path(
            vec![ExcerptRange::line_range(source.clone(), 0..line_count, cx)],
            cx,
        );
        multi_buffer
    }

    /// 创建空的只读组合文档；用于 index 等不可直接编辑的数据投影。
    pub fn empty_read_only(cx: &mut Context<Self>) -> Self {
        Self::empty_with_capability(Capability::ReadOnly, cx)
    }

    fn empty_with_capability(capability: Capability, cx: &mut Context<Self>) -> Self {
        Self {
            state: Self::empty_excerpt_state(cx),
            capability,
            singleton_source: None,
            title: None,
            diffs: Vec::new(),
            diff: None,
            diff_expanded_by_default: false,
            diff_materialized_files: 0,
            snapshot: MultiBufferSnapshot::empty(),
            snapshot_dirty: true,
            snapshot_source_updates: None,
            projection_sync: None,
            show_headers: true,
        }
    }

    /// 空组合状态只保存 source 与其投影映射；组合文本不拥有第二份 Buffer。
    fn empty_excerpt_state(_cx: &mut Context<Self>) -> ExcerptState {
        ExcerptState {
            source_subscriptions: Vec::new(),
            diff_transforms: SumTree::new(()),
            excerpts: SumTree::new(()),
            sources: Vec::new(),
            source_indices: HashMap::new(),
            path_keys: Vec::new(),
            path_key_indices: HashMap::new(),
            capture_names: Arc::from([]),
            projection_version: BufferVersion::INITIAL,
            topology_version: 0,
            metadata_epoch: 0,
            pending_source_syncs: HashMap::new(),
            projection_changes: ProjectionChangeTopic::default(),
            next_transaction_id: TransactionId::INITIAL,
            active_transaction: None,
            active_source_transactions: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// 登记一段投影变化：同步帧内只累积净编辑，帧末一次性发布；帧外立即发布。
    fn publish_projection_change(&mut self, incremental: SourceIncremental) {
        self.snapshot_dirty = true;
        if let Some(sync) = &mut self.projection_sync {
            // 累积精确输出增量，帧末统一重定基到投影版本；不以树比较粗范围替代。
            sync.changes = sync.changes.compose_projection_edits(&incremental.batch);
            sync.changed = true;
            return;
        }
        self.publish_projection_change_now(incremental);
    }

    fn publish_projection_change_now(&mut self, incremental: SourceIncremental) {
        let old_version = self.state.projection_version;
        let new_version = old_version
            .next()
            .expect("组合投影版本不应溢出；溢出时必须创建新文档生命周期");
        self.state.projection_version = new_version;
        let batch = incremental.batch.rebased_to(old_version, new_version);
        self.state
            .projection_changes
            .publish(old_version, new_version, batch);
    }

    /// 开始一个把源快照和 diff 投影一起推进的同步帧。
    pub(crate) fn begin_projection_sync(&mut self) {
        if self.projection_sync.is_none() {
            self.projection_sync = Some(ProjectionSync {
                changes: TextChangeBatch::default(),
                changed: false,
                snapshot_changed: false,
                waiting_for_diff_sources: HashSet::new(),
            });
        }
    }

    /// 提交同步帧，使源映射、diff transform 和投影版本同时对外可见。
    pub(crate) fn finish_projection_sync(&mut self, cx: &mut Context<Self>) {
        let Some(sync) = self.projection_sync.as_ref() else {
            return;
        };
        if sync
            .waiting_for_diff_sources
            .iter()
            .any(|source_id| !self.diff_source_sync_ready(*source_id, cx))
        {
            return;
        }
        let sync = self.projection_sync.take().expect("同步帧在检查后仍应存在");
        if !sync.changed {
            if sync.snapshot_changed {
                cx.notify();
            }
            return;
        }
        self.publish_projection_change_now(SourceIncremental {
            batch: sync.changes,
        });
        self.emit_projection_changed(cx);
    }

    pub(crate) fn emit_text_changed(&mut self, cx: &mut Context<Self>) {
        if self.projection_sync.is_none() {
            cx.emit(MultiBufferEvent::TextChanged);
            cx.notify();
        }
    }

    pub(crate) fn emit_projection_changed(&mut self, cx: &mut Context<Self>) {
        if self.projection_sync.is_none() {
            cx.emit(MultiBufferEvent::ProjectionChanged);
            cx.notify();
        }
    }

    pub(crate) fn notify_if_not_syncing(&mut self, cx: &mut Context<Self>) {
        if let Some(sync) = &mut self.projection_sync {
            sync.snapshot_changed = true;
        } else {
            cx.notify();
        }
    }

    /// 用编辑前冻结的投影树与当前投影树按游标推导结构变化范围，并发布增量批次。
    ///
    /// 结构变化不物化组合文本：前后两棵树按同版本源片段比对公共前后缀，
    /// 不把 hunk 装饰造成的节点拆分当成文本编辑。源内部编辑由 TextChangeBatch 单独投影。
    fn publish_projection_edit(&mut self, before: &ProjectionTrees, old_version: BufferVersion) {
        let after = self.projection_trees();
        let (old_range, new_range) = projection_changed_ranges(before, &after);
        let batch =
            TextChangeBatch::from_edits(old_version, old_version, vec![(old_range, new_range)]);
        self.publish_projection_change(SourceIncremental { batch });
    }

    /// 冻结当前输入/输出投影树；供结构变化前保存旧坐标、变化后推导增量范围。
    pub(crate) fn projection_trees(&self) -> ProjectionTrees {
        (
            self.state.excerpts.clone(),
            self.state.diff_transforms.clone(),
        )
    }

    /// 把源版本化编辑投影到稳定逻辑 excerpts 的输入坐标。
    fn source_input_edits(
        &self,
        source_change: &TextChangeBatch,
        records: &SourceEditRecords,
    ) -> Vec<diff_transform_sync::InputEdit> {
        let mut edits = Vec::new();
        for (old, new) in records.old.iter().zip(&records.new) {
            for edit in source_change.patch().edits() {
                let start = edit.old_range().start().max(old.source_range.start());
                let end = edit.old_range().end().min(old.source_range.end());
                if start > end {
                    continue;
                }
                let new_start = edit.new_range().start().max(new.source_range.start());
                let new_end = edit.new_range().end().min(new.source_range.end());
                if new_start > new_end {
                    continue;
                }
                let old_range = (old.input_offset.get() + start.get()
                    - old.source_range.start().get())
                    ..(old.input_offset.get() + end.get() - old.source_range.start().get());
                let new_range = (new.input_offset.get() + new_start.get()
                    - new.source_range.start().get())
                    ..(new.input_offset.get() + new_end.get() - new.source_range.start().get());
                if old_range.is_empty() && new_range.is_empty() {
                    continue;
                }
                edits.push(diff_transform_sync::InputEdit::new(old_range, new_range));
            }
        }
        diff_transform_sync::merge_input_edits(edits)
    }

    /// 结构变更（excerpt 增删、diff 展开折叠）必须在文本事务之外进行。
    ///
    /// 事务期间改变拓扑会让组合事务身份与坐标基准错配（M-8）；
    /// 这里显式失败，而不是让 end_transaction 在已变化的拓扑上静默收尾。
    fn assert_no_active_transaction(&self, entry: &'static str) {
        assert!(
            self.state.active_transaction.is_none(),
            "{entry} 必须在文本事务之外调用（M-8）"
        );
    }

    fn subscribe_source(
        source: Entity<LanguageBuffer>,
        track_text: bool,
        cx: &mut Context<Self>,
    ) -> SourceSubscription {
        let source_id = source.entity_id();
        let event = cx.subscribe(&source, move |this, _, event, cx| match event {
            LanguageBufferEvent::TextChanged if track_text => {
                this.state
                    .pending_source_syncs
                    .entry(source_id)
                    .or_default();
                cx.emit(MultiBufferEvent::TextChanged);
                cx.notify();
            }
            LanguageBufferEvent::TextChanged => {}
            LanguageBufferEvent::Reparsed => {
                this.state
                    .pending_source_syncs
                    .entry(source_id)
                    .or_default()
                    .refresh_metadata = true;
                cx.emit(MultiBufferEvent::Reparsed(source_id));
                cx.notify();
            }
            LanguageBufferEvent::MetadataChanged => {
                this.state
                    .pending_source_syncs
                    .entry(source_id)
                    .or_default()
                    .refresh_metadata = true;
                cx.emit(MultiBufferEvent::MetadataChanged);
                cx.notify();
            }
        });
        let text = track_text.then(|| source.read(cx).subscribe());
        SourceSubscription {
            source,
            text,
            _event: event,
        }
    }

    /// 整篇重建组合文档的内部入口；只供 diff 投影重建与 clear 使用。
    ///
    /// 顺序权威归 MultiBuffer：内部按 PathKey 稳定排序后建树，调用方传入顺序不进入文档语义。
    fn replace_all_excerpts(&mut self, excerpts: Vec<ExcerptRange>, cx: &mut Context<Self>) {
        self.assert_no_active_transaction("MultiBuffer::replace_all_excerpts");
        self.snapshot_dirty = true;
        self.snapshot_source_updates = None;
        self.state.topology_version = self.state.topology_version.wrapping_add(1);
        let mut unique_sources = Vec::<Entity<LanguageBuffer>>::new();
        let mut unique_source_ids = HashSet::new();
        for excerpt in &excerpts {
            if unique_source_ids.insert(excerpt.source.entity_id()) {
                unique_sources.push(excerpt.source.clone());
            }
        }
        for file in &self.diffs {
            if let Some(base) = file.diff.read(cx).base_source()
                && unique_source_ids.insert(base.entity_id())
            {
                unique_sources.push(base.clone());
            }
        }
        // Deleted 源只由 BufferDiff 推进文本投影，但它的语法与语言元数据仍属于组合快照。
        let text_sources = excerpts
            .iter()
            .map(|excerpt| excerpt.source.entity_id())
            .collect::<HashSet<_>>();
        // 结构重建只替换拓扑：仍存活的源复用已有连接，不为一次重排重建全部订阅。
        let mut previous_subscriptions = std::mem::take(&mut self.state.source_subscriptions)
            .into_iter()
            .map(|subscription| (subscription.source.entity_id(), subscription))
            .collect::<HashMap<_, _>>();
        let mut next_source_subscriptions = Vec::with_capacity(unique_sources.len());
        for source in unique_sources {
            let source_id = source.entity_id();
            let track_text = text_sources.contains(&source_id);
            if let Some(subscription) = previous_subscriptions.remove(&source_id)
                && subscription.text.is_some() == track_text
            {
                // 新 excerpts 已采用当前源快照；
                // 丢弃复用订阅里已被快照吸收的待消费编辑，否则下一帧会按旧版本重放同一批编辑。
                if let Some(text) = &subscription.text {
                    text.consume();
                }
                next_source_subscriptions.push(subscription);
            } else {
                next_source_subscriptions.push(Self::subscribe_source(source, track_text, cx));
            }
        }
        // 折叠候选在源级缓存：结构重建时复用同一源已缓存的锚点，只有 reparse 才重算。
        let ExcerptState {
            source_subscriptions,
            excerpts: authoritative_excerpts,
            diff_transforms,
            sources,
            source_indices,
            path_keys,
            path_key_indices,
            capture_names: composite_capture_names,
            ..
        } = &mut self.state;

        // 按源去重构建 (text, syntax) 表：同一文件的大量片段共享一份源状态。
        let mut next_sources: Vec<ExcerptSource> = Vec::new();
        let mut next_source_indices = HashMap::new();
        struct PreparedExcerpt {
            excerpt: ExcerptRange,
            path: PathKey,
            source_index: usize,
            source_id: gpui::EntityId,
            start_line: usize,
        }
        let mut prepared = Vec::with_capacity(excerpts.len());
        for excerpt in excerpts {
            let source = excerpt.source.read(cx);
            let path = path_key_for_source(source);
            let buffer_id = source.buffer_id();
            let source_id = excerpt.source.entity_id();
            let source_index = match next_source_indices.get(&source_id).copied() {
                Some(index) => index,
                None => {
                    let snapshot = source.snapshot();
                    next_sources.push(ExcerptSource {
                        entity: excerpt.source.clone(),
                        path: path.clone(),
                        buffer_id,
                        word_boundary: snapshot_word_boundary(&snapshot),
                        settings: snapshot_settings(&snapshot),
                        text: snapshot.text,
                        syntax: snapshot.syntax,
                        highlight_cache: snapshot.highlight_cache,
                        capture_map: Arc::from([]),
                    });
                    let index = next_sources.len() - 1;
                    next_source_indices.insert(source_id, index);
                    index
                }
            };
            let source_snapshot = &next_sources[source_index];
            if !snapshot_range_is_valid(&source_snapshot.text, excerpt.source_range) {
                continue;
            }
            let start_line = source_snapshot
                .text
                .byte_to_line(excerpt.source_range.start())
                .map_or(0, |line| line.get());
            prepared.push(PreparedExcerpt {
                excerpt,
                path,
                source_index,
                source_id,
                start_line,
            });
        }

        for subscription in &next_source_subscriptions {
            let entity = &subscription.source;
            if let Entry::Vacant(entry) = next_source_indices.entry(entity.entity_id()) {
                let source = entity.read(cx);
                let snapshot = source.snapshot();
                let index = next_sources.len();
                entry.insert(index);
                next_sources.push(ExcerptSource {
                    entity: entity.clone(),
                    path: path_key_for_source(source),
                    buffer_id: source.buffer_id(),
                    word_boundary: snapshot_word_boundary(&snapshot),
                    settings: snapshot_settings(&snapshot),
                    text: snapshot.text,
                    syntax: snapshot.syntax,
                    highlight_cache: snapshot.highlight_cache,
                    capture_map: Arc::from([]),
                });
            }
        }
        // 组合文档的顺序由 PathKey 决定，不由调用方传入顺序决定：按路径稳定排序后建树。
        prepared.sort_by(|a, b| Ord::cmp(&a.path, &b.path));

        // 非末尾逻辑窗口各自拥有一个结构分隔换行，源文本已有的换行仍原样保留。
        // 分隔标志只随窗口结构变化，不随源文本编辑变化。
        let prepared_count = prepared.len();
        let mut next_excerpts = Vec::with_capacity(prepared_count);
        for (position, item) in prepared.into_iter().enumerate() {
            let display_path = item
                .excerpt
                .display_path
                .clone()
                .unwrap_or_else(|| item.path.clone());
            let Some((text_summary, _)) = snapshot_range_summary(
                &next_sources[item.source_index].text,
                item.excerpt.source_range,
            ) else {
                continue;
            };
            let adds_newline = position + 1 < prepared_count;
            let path_index = intern_path(path_keys, path_key_indices, &item.path);
            let excerpt = Excerpt {
                path: item.path,
                path_index,
                display_path,
                buffer_id: next_sources[item.source_index].buffer_id,
                source_range: ExcerptContext::new(
                    next_sources[item.source_index].text.version(),
                    item.excerpt.source_range,
                    false,
                ),
                source_start_line: item.start_line,
                text_summary,
                adds_newline,
                match_ranges: Arc::from(item.excerpt.match_ranges),
                source_index: item.source_index,
                source_id: Some(item.source_id),
                editable: item.excerpt.editable,
            };
            next_excerpts.push(excerpt);
        }

        *source_subscriptions = next_source_subscriptions;
        *authoritative_excerpts = SumTree::from_iter(next_excerpts, ());
        *diff_transforms = SumTree::new(());
        *sources = next_sources;
        *source_indices = sources
            .iter()
            .enumerate()
            .map(|(index, source)| (source.entity.entity_id(), index))
            .collect();
        *composite_capture_names = rebuild_capture_table(sources);
        self.rebuild_all_diff_transforms(cx);
        self.emit_projection_changed(cx);
    }

    /// 为一组同路径片段构建位置无关映射项；`start_index` 是它们在文档中的起始序号。
    fn build_entries_for_excerpts(
        &mut self,
        excerpts: Vec<ExcerptRange>,
        start_index: usize,
        total: usize,
    ) -> Vec<Excerpt> {
        let mut path_keys = std::mem::take(&mut self.state.path_keys);
        let mut path_key_indices = std::mem::take(&mut self.state.path_key_indices);
        let mut entries = Vec::with_capacity(excerpts.len());
        for (position, excerpt) in excerpts.into_iter().enumerate() {
            let source_id = excerpt.source.entity_id();
            let Some(&source_index) = self.state.source_indices.get(&source_id) else {
                continue;
            };
            let source = &self.state.sources[source_index];
            let path = source.path.clone();
            let display_path = excerpt.display_path.clone().unwrap_or_else(|| path.clone());
            let Some((text_summary, _)) =
                snapshot_range_summary(&source.text, excerpt.source_range)
            else {
                continue;
            };
            let adds_newline = start_index + position + 1 < total;
            let start_line = source
                .text
                .byte_to_line(excerpt.source_range.start())
                .map_or(0, |line| line.get());
            let path_index = intern_path(&mut path_keys, &mut path_key_indices, &path);
            entries.push(Excerpt {
                path,
                path_index,
                display_path,
                buffer_id: source.buffer_id,
                source_range: ExcerptContext::new(
                    source.text.version(),
                    excerpt.source_range,
                    false,
                ),
                source_start_line: start_line,
                text_summary,
                adds_newline,
                match_ranges: Arc::from(excerpt.match_ranges),
                source_index,
                source_id: Some(source_id),
                editable: excerpt.editable,
            });
        }
        self.state.path_keys = path_keys;
        self.state.path_key_indices = path_key_indices;
        entries
    }

    /// 源编辑只推进逻辑 excerpt 的 Anchor、内容摘要与匹配范围。
    /// 分隔换行的生命周期由 excerpt 插入、移除维护。
    fn splice_source_path(
        &mut self,
        source_id: gpui::EntityId,
        source_position_map: &PositionMap,
        expanded_excerpts: Option<&HashSet<usize>>,
        _cx: &App,
    ) -> SourceEditRecords {
        let source_index = self.state.source_indices[&source_id];
        let source = &self.state.sources[source_index];
        let path = source.path.clone();
        let mut cursor = self.state.excerpts.cursor::<ExcerptSummary>(());
        let mut next = cursor.slice(&path, Bias::Left);
        let start_index = cursor.start().count;
        let mut old_input = cursor.start().text.len;
        let mut new_input = old_input;
        let removed = cursor.slice(&path, Bias::Right);
        let suffix = cursor.suffix();
        drop(cursor);
        let source_count = removed
            .iter()
            .filter(|entry| entry.source_id == Some(source_id))
            .count();
        let mut source_position = 0;
        let mut records = SourceEditRecords::default();
        let entries = removed
            .iter()
            .enumerate()
            .map(|(local, original)| {
                let mut entry = original.clone();
                if entry.source_id == Some(source_id) {
                    records.old.push(SourceExcerptRecord {
                        input_offset: ExcerptOffset::new(old_input),
                        source_range: entry.source_range.range(),
                    });
                    source_position += 1;
                    let (start_affinity, end_affinity) = match expanded_excerpts {
                        None => (
                            Affinity::Before,
                            if entry.source_range.is_empty() || source_position == source_count {
                                Affinity::After
                            } else {
                                Affinity::Before
                            },
                        ),
                        Some(expanded) if expanded.contains(&(start_index + local)) => {
                            (Affinity::Before, Affinity::After)
                        }
                        Some(_) => (Affinity::After, Affinity::Before),
                    };
                    entry.source_range = entry
                        .source_range
                        .mapped(&source.text, start_affinity, end_affinity)
                        .expect("源编辑必须属于 excerpt 的版本链");
                    entry.match_ranges = entry
                        .match_ranges
                        .iter()
                        .map(|range| {
                            source_position_map
                                .map_old_range_with_stickiness(*range, Stickiness::Never)
                                .value()
                        })
                        .collect::<Vec<_>>()
                        .into();
                    entry.text_summary =
                        snapshot_range_summary(&source.text, entry.source_range.range())
                            .expect("推进后的 excerpt 范围必须有效")
                            .0;
                    entry.source_start_line = source
                        .text
                        .byte_to_line(entry.source_range.start())
                        .expect("excerpt 起点必须属于源快照")
                        .get();
                    records.new.push(SourceExcerptRecord {
                        input_offset: ExcerptOffset::new(new_input),
                        source_range: entry.source_range.range(),
                    });
                }
                old_input += diff_output_text(original).len;
                new_input += diff_output_text(&entry).len;
                entry
            })
            .collect::<Vec<_>>();
        next.extend(entries, ());
        next.append(suffix, ());
        self.state.excerpts = next;
        records
    }

    /// 显式替换一个路径的逻辑 excerpts，并把结构编辑交给统一变换同步器。
    fn splice_excerpt_entries(&mut self, path: &PathKey, entries: Vec<Excerpt>, cx: &App) {
        let before = self.projection_trees();
        let mut cursor = before.0.cursor::<ExcerptSummary>(());
        let mut next = cursor.slice(path, Bias::Left);
        let old_start = cursor.start().text.len;
        cursor.seek(path, Bias::Right);
        let old_end = cursor.start().text.len;
        let suffix = cursor.suffix();
        drop(cursor);
        next.extend(entries, ());
        next.append(suffix, ());
        let mut separator_edits = Vec::new();
        let old_tail = before.0.last();
        // 追加路径时，原文档尾获得一个结构分隔符。
        if let Some(old_tail) = old_tail
            && old_tail.path < *path
            && next.summary().count > before.0.summary().count
        {
            let mut cursor = next.cursor::<ExcerptSummary>(());
            let mut prefix = cursor.slice(&old_tail.path, Bias::Left);
            let mut previous_path = cursor.slice(&old_tail.path, Bias::Right);
            let input_start = prefix.summary().text.len;
            let footer_start = input_start + previous_path.summary().text.len;
            let suffix = cursor.suffix();
            drop(cursor);
            previous_path.update_last(|entry| entry.adds_newline = true, ());
            prefix.append(previous_path, ());
            prefix.append(suffix, ());
            next = prefix;
            separator_edits.push(diff_transform_sync::InputEdit::new(
                footer_start..footer_start,
                footer_start..footer_start + 1,
            ));
        }
        // 移除尾路径时，新的文档尾释放原有分隔符。
        if next.last().is_some_and(|entry| entry.adds_newline) {
            let old_footer = before.0.summary().text.len - (old_end - old_start) - 1;
            let new_footer = next.summary().text.len - 1;
            next.update_last(|entry| entry.adds_newline = false, ());
            separator_edits.push(diff_transform_sync::InputEdit::new(
                old_footer..old_footer + 1,
                new_footer..new_footer,
            ));
        }
        self.state.excerpts = next;
        let mut cursor = self.state.excerpts.cursor::<ExcerptSummary>(());
        cursor.seek(path, Bias::Left);
        let new_start = cursor.start().text.len;
        cursor.seek(path, Bias::Right);
        let new_end = cursor.start().text.len;
        drop(cursor);
        separator_edits.push(diff_transform_sync::InputEdit::new(
            old_start..old_end,
            new_start..new_end,
        ));
        self.sync_diff_transforms(&before, separator_edits, None, cx);
    }

    /// 从当前映射树派生组合坐标下的搜索匹配范围。
    ///
    /// excerpt 的 match_ranges 是权威数据；文档级列表按需派生，不另存可写副本。
    fn match_ranges_from_tree(&self) -> Vec<MultiBufferRange> {
        self.project_match_ranges(None)
    }

    fn match_ranges_for_path(&self, path: &PathKey) -> Vec<TextRange> {
        self.project_match_ranges(Some(path))
            .into_iter()
            .map(|range| {
                TextRange::new(range.start().into(), range.end().into()).expect("匹配范围必须正序")
            })
            .collect()
    }

    fn project_match_ranges(&self, path: Option<&PathKey>) -> Vec<MultiBufferRange> {
        let mut ranges = Vec::new();
        let mut cursor = MultiBufferCursor::new(&self.state.excerpts, &self.state.diff_transforms);
        if let Some(path) = path {
            cursor.seek_path(path, Bias::Left);
        } else {
            cursor.seek_output(ByteOffset::ZERO, Bias::Right);
        }
        while let Some((excerpt, _)) = cursor.item() {
            if path.is_some_and(|path| excerpt.path != *path) {
                break;
            }
            let mapping = excerpt.to_mapping(cursor.start());
            let output_start = mapping.output_range.start().get();
            let source_start = mapping.source_range.start().get();
            for matched in mapping.match_ranges.iter() {
                if matched.start() < mapping.source_range.start()
                    || matched.end() > mapping.source_range.end()
                {
                    continue;
                }
                let start = output_start + matched.start().get() - source_start;
                let end = output_start + matched.end().get() - source_start;
                if let Ok(range) = MultiBufferRange::new(
                    MultiBufferOffset::new(start),
                    MultiBufferOffset::new(end),
                ) {
                    ranges.push(range);
                }
            }
            cursor.next();
        }
        ranges
    }

    /// 移除指定源路径的全部 excerpts；其余片段的组合坐标自动顺延。
    ///
    /// 只重算映射与匹配范围，不重建源订阅；路径不存在时返回 false。
    pub fn remove_excerpts_for_path(&mut self, path: &Path, cx: &mut Context<Self>) -> bool {
        let path_key = PathKey::new(path.to_path_buf());
        let removed = {
            let mut cursor =
                MultiBufferCursor::new(&self.state.excerpts, &self.state.diff_transforms);
            cursor.seek_path(&path_key, Bias::Left);
            cursor
                .item()
                .is_some_and(|(entry, _)| entry.path == path_key)
        };
        if !removed {
            return false;
        }
        let before = self.projection_trees();
        let old_version = self.state.projection_version;
        self.state.topology_version = self.state.topology_version.wrapping_add(1);
        self.splice_excerpt_entries(&path_key, Vec::new(), cx);
        self.publish_projection_edit(&before, old_version);
        self.emit_projection_changed(cx);
        true
    }

    /// 注册尚未跟踪的源：文本/语法快照、变更订阅与捕获表；返回新增源的起始索引。
    fn register_sources(
        &mut self,
        new_sources: Vec<Entity<LanguageBuffer>>,
        subscribe_ids: &HashSet<gpui::EntityId>,
        cx: &mut Context<Self>,
    ) -> usize {
        if !new_sources.is_empty() {
            self.snapshot_dirty = true;
            self.snapshot_source_updates = None;
        }
        let first_new_source = self.state.sources.len();
        if new_sources.is_empty() {
            return first_new_source;
        }
        // 每个可见源都接收语法元数据事件；只有组合文本的权威输入订阅文本增量。
        let new_source_subscriptions = new_sources
            .iter()
            .map(|source| {
                Self::subscribe_source(
                    source.clone(),
                    subscribe_ids.contains(&source.entity_id()),
                    cx,
                )
            })
            .collect::<Vec<_>>();
        self.state.sources.extend(new_sources.iter().map(|source| {
            let language_buffer = source.read(cx);
            let snapshot = language_buffer.snapshot();
            let path = path_key_for_source(language_buffer);
            let buffer_id = language_buffer.buffer_id();
            ExcerptSource {
                entity: source.clone(),
                path,
                buffer_id,
                word_boundary: snapshot_word_boundary(&snapshot),
                settings: snapshot_settings(&snapshot),
                text: snapshot.text,
                syntax: snapshot.syntax,
                highlight_cache: snapshot.highlight_cache,
                capture_map: Arc::from([]),
            }
        }));
        self.state
            .source_subscriptions
            .extend(new_source_subscriptions);
        for (index, source) in self.state.sources.iter().enumerate().skip(first_new_source) {
            self.state
                .source_indices
                .insert(source.entity.entity_id(), index);
        }
        let mut capture_names = self.state.capture_names.iter().cloned().collect::<Vec<_>>();
        extend_capture_table(
            &mut self.state.sources,
            first_new_source,
            &mut capture_names,
        );
        self.state.capture_names = Arc::from(capture_names);
        first_new_source
    }

    /// 片段源文件路径；源必须已注册。
    fn excerpt_path(&self, excerpt: &ExcerptRange, _cx: &App) -> PathKey {
        let source_index = self.state.source_indices[&excerpt.source.entity_id()];
        self.state.sources[source_index].path.clone()
    }

    /// 用给定 excerpts 替换其源路径现有的全部 excerpts（按路径有序插入）。
    ///
    /// 路径由片段源的 file_path 确定；同一调用内的片段必须属于同一路径。
    /// 新增源自动注册，其余路径的片段与其组合坐标保持不变。
    /// 替换某路径的 excerpts，保持其余路径不变并维持路径升序。
    ///
    /// 返回该路径新片段在输出坐标中的匹配范围；调用方据此更新搜索高亮。
    pub fn set_excerpts_for_path(
        &mut self,
        excerpts: Vec<ExcerptRange>,
        cx: &mut Context<Self>,
    ) -> Vec<TextRange> {
        self.assert_no_active_transaction("MultiBuffer::set_excerpts_for_path");
        if excerpts.is_empty() {
            return Vec::new();
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
            .map(|excerpt| excerpt.source.entity_id())
            .collect::<HashSet<_>>();
        self.register_sources(new_sources, &subscribe_ids, cx);
        let path = self.excerpt_path(&excerpts[0], cx);
        debug_assert!(
            excerpts
                .iter()
                .all(|excerpt| self.excerpt_path(excerpt, cx) == path),
            "set_excerpts_for_path 的片段必须属于同一路径"
        );
        self.prepare_diff_sources(cx);
        self.apply_excerpts_for_path(path, excerpts, cx)
    }

    /// 用给定片段替换某路径的 excerpts，保持其余路径不变并维持路径升序。
    fn apply_excerpts_for_path(
        &mut self,
        path: PathKey,
        excerpts: Vec<ExcerptRange>,
        cx: &mut Context<Self>,
    ) -> Vec<TextRange> {
        let before = self.projection_trees();
        let old_version = self.state.projection_version;
        self.state.topology_version = self.state.topology_version.wrapping_add(1);
        let (start_index, old_count) = {
            let mut cursor = self.state.excerpts.cursor::<ExcerptSummary>(());
            cursor.seek(&path, Bias::Left);
            let start = cursor.start().count;
            cursor.seek(&path, Bias::Right);
            (start, cursor.start().count - start)
        };
        let total = self.state.excerpts.summary().count - old_count + excerpts.len();
        let entries = self.build_entries_for_excerpts(excerpts, start_index, total);
        self.splice_excerpt_entries(&path, entries, cx);
        let match_ranges = self.match_ranges_for_path(&path);
        self.publish_projection_edit(&before, old_version);
        self.emit_projection_changed(cx);
        match_ranges
    }

    /// 从 MultiBuffer 自己拥有的源订阅拉取下一段连续变化并推进投影。
    ///
    /// `LanguageBufferEvent` 只是唤醒信号；直接编辑、外部编辑和历史回放都经过此入口，
    /// 因而同一源只有一个增量游标，不会重放已消费的旧事件。
    fn synchronize_source_change(
        &mut self,
        source_id: gpui::EntityId,
        expanded_excerpts: Option<&HashSet<usize>>,
        refresh_metadata: bool,
        cx: &mut Context<Self>,
    ) -> Option<TextChangeBatch> {
        let tracks_text = self
            .state
            .source_subscriptions
            .iter()
            .find(|state| state.source.entity_id() == source_id)
            .map(|state| state.text.is_some())?;
        if !tracks_text {
            if refresh_metadata {
                self.refresh_source_metadata_if_current(source_id, cx);
            }
            return None;
        }
        let source_change = self
            .state
            .source_subscriptions
            .iter()
            .find(|state| state.source.entity_id() == source_id)
            .and_then(|state| state.text.as_ref())
            .map(TextSubscription::consume)?;
        if source_change.is_empty() {
            // 文本变化已被主动同步；延迟到达的事件只需刷新语法快照。
            self.refresh_source_snapshot(source_id, cx);
            return None;
        }
        let stored_version = self
            .state
            .sources
            .iter()
            .find(|source| source.entity.entity_id() == source_id)
            .map(|source| source.text.version())?;
        assert_eq!(
            source_change.old_version(),
            Some(stored_version),
            "MultiBuffer 必须按自己的源订阅连续消费文本变化"
        );
        let position_map = source_change.position_map();
        // 普通编辑只更新受影响 source 的派生坐标；BufferDiff 仍独立维护 hunk 拓扑。
        self.apply_source_change(
            source_id,
            &position_map,
            &source_change,
            expanded_excerpts,
            cx,
        );
        if refresh_metadata {
            self.refresh_source_snapshot(source_id, cx);
        }
        Some(source_change)
    }

    fn refresh_source_snapshot(&mut self, source_id: gpui::EntityId, cx: &App) {
        let Some(source) = self
            .state
            .sources
            .iter()
            .find(|source| source.entity.entity_id() == source_id)
            .map(|source| source.entity.clone())
        else {
            return;
        };
        let snapshot = source.read(cx).snapshot();
        self.install_source_snapshot(source_id, snapshot);
    }

    /// Deleted 源的文本由 BufferDiff 推进；其语法只能同步到当前投影已采用的同一文本版本。
    fn refresh_source_metadata_if_current(&mut self, source_id: gpui::EntityId, cx: &App) {
        let Some((source, text_version)) = self
            .state
            .sources
            .iter()
            .find(|source| source.entity.entity_id() == source_id)
            .map(|source| (source.entity.clone(), source.text.version()))
        else {
            return;
        };
        let snapshot = source.read(cx).snapshot();
        if snapshot.text.version() != text_version {
            return;
        }
        self.install_source_snapshot(source_id, snapshot);
    }

    fn install_source_snapshot(
        &mut self,
        source_id: gpui::EntityId,
        snapshot: LanguageBufferSnapshot,
    ) {
        self.snapshot_dirty = true;
        self.state.metadata_epoch = self.state.metadata_epoch.wrapping_add(1);
        if let Some(excerpt_source) = self
            .state
            .sources
            .iter_mut()
            .find(|source| source.entity.entity_id() == source_id)
        {
            excerpt_source.text = snapshot.text.clone();
            excerpt_source.syntax = snapshot.syntax.clone();
            excerpt_source.highlight_cache = Arc::clone(&snapshot.highlight_cache);
            excerpt_source.word_boundary = snapshot_word_boundary(&snapshot);
            excerpt_source.settings = snapshot_settings(&snapshot);
        }
        self.mark_source_snapshot_changed(source_id);
        let previous_capture_names = Arc::clone(&self.state.capture_names);
        self.state.capture_names = rebuild_capture_table(&mut self.state.sources);
        if self.state.capture_names != previous_capture_names {
            self.mark_all_source_snapshots_changed();
        }
    }

    fn mark_source_snapshot_changed(&mut self, source_id: gpui::EntityId) {
        let Some(source_index) = self.state.source_indices.get(&source_id).copied() else {
            return;
        };
        if let Some(updates) = &mut self.snapshot_source_updates {
            updates.insert(source_index);
        }
    }

    fn mark_all_source_snapshots_changed(&mut self) {
        if let Some(updates) = &mut self.snapshot_source_updates {
            updates.extend(0..self.state.sources.len());
        }
    }

    /// 把源 Buffer 的版本化编辑同步到组合投影。
    fn apply_source_change(
        &mut self,
        source_id: gpui::EntityId,
        source_position_map: &PositionMap,
        source_change: &TextChangeBatch,
        expanded_excerpts: Option<&HashSet<usize>>,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self
            .state
            .sources
            .iter()
            .find(|source| source.entity.entity_id() == source_id)
            .map(|source| source.entity.clone())
        else {
            return;
        };
        let before = self.projection_trees();
        let old_text = self.state.sources[self.state.source_indices[&source_id]]
            .text
            .clone();
        let snapshot = source.read(cx).snapshot();
        // capture 表只在源语法捕获集合变化时重建；纯文本编辑保持已有全局索引不变。
        let captures_changed = self
            .state
            .sources
            .iter()
            .find(|source| source.entity.entity_id() == source_id)
            .is_some_and(|source| source.syntax.capture_names() != snapshot.syntax.capture_names());

        if let Some(excerpt_source) = self
            .state
            .sources
            .iter_mut()
            .find(|source| source.entity.entity_id() == source_id)
        {
            excerpt_source.word_boundary = snapshot_word_boundary(&snapshot);
            excerpt_source.settings = snapshot_settings(&snapshot);
            excerpt_source.text = snapshot.text;
            excerpt_source.syntax = snapshot.syntax;
            excerpt_source.highlight_cache = snapshot.highlight_cache;
        }
        self.mark_source_snapshot_changed(source_id);
        if captures_changed {
            self.state.capture_names = rebuild_capture_table(&mut self.state.sources);
            self.mark_all_source_snapshots_changed();
        }
        // 绝对输出坐标由树摘要推导：源范围变化只 splice 受影响路径的 item，其余路径不变。
        let records =
            self.splice_source_path(source_id, source_position_map, expanded_excerpts, cx);
        let edits = self.source_input_edits(source_change, &records);
        let output_edits =
            self.sync_diff_transforms(&before, edits, Some((source_id, &old_text)), cx);
        self.publish_projection_change(SourceIncremental {
            batch: source_change.projected_from(output_edits),
        });
        self.emit_text_changed(cx);
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.assert_no_active_transaction("MultiBuffer::clear");
        let before = self.projection_trees();
        let old_version = self.state.projection_version;
        self.replace_all_excerpts(Vec::new(), cx);
        self.publish_projection_edit(&before, old_version);
    }

    /// 将 MultiBuffer 坐标中的编辑拆分到各个底层 Buffer。
    ///
    /// 组合文本只通过映射与游标读取，不创建可变的第二份逻辑文档。
    /// 同一 excerpt 直接映射；跨 excerpt 替换只在起始 excerpt 插入新文本，
    /// 并删除起始尾段、中间 excerpt 和结束首段。
    ///
    /// 编辑本身不返回投影重映射：选区以源 Anchor 为唯一权威，调用方在编辑后按当前快照重新锚定即可。
    pub fn edit(
        &mut self,
        edits: Vec<Edit>,
        metadata: TransactionMetadata,
        cx: &mut Context<Self>,
    ) -> TextResult<()> {
        if self.capability.is_read_only() {
            return Err(StorageError::ReadOnly.into());
        }

        // 源实体按 source_index 稳定索引；供闭包查源而不借用整个 state。
        let source_entities = self
            .state
            .sources
            .iter()
            .map(|source| source.entity.clone())
            .collect::<Vec<_>>();
        // 映射的源范围只在建立映射的那份源快照上有效；提交前用它校验源没有被并发推进。
        let source_versions = self
            .state
            .sources
            .iter()
            .map(|source| source.text.version())
            .collect::<Vec<_>>();

        let mut grouped: Vec<(Entity<LanguageBuffer>, BufferVersion, Vec<Edit>)> = Vec::new();
        let mut edited_excerpts = HashSet::new();
        let push_source_edit =
            |mapping: &ExcerptMapping,
             source_range: TextRange,
             replacement: String,
             grouped: &mut Vec<(Entity<LanguageBuffer>, BufferVersion, Vec<Edit>)>,
             edited_excerpts: &mut HashSet<usize>| {
                edited_excerpts.insert(mapping.excerpt_index);
                let source = source_entities[mapping.source_index].clone();
                let source_edit = Edit::replace(source_range, replacement);
                if let Some((_, _, source_edits)) = grouped
                    .iter_mut()
                    .find(|(candidate, _, _)| candidate.entity_id() == source.entity_id())
                {
                    source_edits.push(source_edit);
                } else {
                    grouped.push((
                        source,
                        source_versions[mapping.source_index],
                        vec![source_edit],
                    ));
                }
            };
        for edit in edits {
            let range = edit.range();
            let start_mapping = mapping_at_tree(
                &self.state.excerpts,
                &self.state.diff_transforms,
                range.start(),
            )
            .map(|(mapping, _)| mapping)
            .ok_or_else(|| TextError::InvariantViolation {
                location: "MultiBuffer::edit",
                detail: "编辑起点不在可见 excerpt 中".to_string(),
            })?;
            let mappings = mappings_in_output_range(
                &self.state.excerpts,
                &self.state.diff_transforms,
                start_mapping,
                range.end(),
            );
            if mappings.iter().any(|mapping| !mapping.editable) {
                return Err(StorageError::ReadOnly.into());
            }
            let start_mapping = mappings.first().expect("编辑起始 excerpt 必须存在");
            let end_mapping = mappings.last().expect("编辑终止 excerpt 必须存在");
            let source_start = ByteOffset::new(
                (start_mapping.source_range.start().get()
                    + range
                        .start()
                        .get()
                        .saturating_sub(start_mapping.output_range.start().get()))
                .min(start_mapping.source_range.end().get()),
            );
            let source_end = ByteOffset::new(
                (end_mapping.source_range.start().get()
                    + range
                        .end()
                        .get()
                        .saturating_sub(end_mapping.output_range.start().get()))
                .min(end_mapping.source_range.end().get()),
            );
            if mappings.len() == 1 {
                push_source_edit(
                    start_mapping,
                    TextRange::new(source_start, source_end).expect("已验证的源范围必须有效"),
                    edit.replacement().to_owned(),
                    &mut grouped,
                    &mut edited_excerpts,
                );
            } else {
                push_source_edit(
                    start_mapping,
                    TextRange::new(source_start, start_mapping.source_range.end())
                        .expect("起始 excerpt 尾段必须有效"),
                    edit.replacement().to_owned(),
                    &mut grouped,
                    &mut edited_excerpts,
                );
                for mapping in mappings
                    .iter()
                    .skip(1)
                    .take(mappings.len().saturating_sub(2))
                {
                    push_source_edit(
                        mapping,
                        mapping.source_range.range(),
                        String::new(),
                        &mut grouped,
                        &mut edited_excerpts,
                    );
                }
                push_source_edit(
                    end_mapping,
                    TextRange::new(end_mapping.source_range.start(), source_end)
                        .expect("结束 excerpt 首段必须有效"),
                    String::new(),
                    &mut grouped,
                    &mut edited_excerpts,
                );
            }
        }

        // 预检：任一源在映射建立后推进过版本，就说明源范围不再对应当前文本。
        // 必须在任何源提交前一次性判定，否则先提交的源会留下部分提交文本
        // （单个源 Buffer 的提交是原子的，跨源提交不是）。
        for (source, base_version, _) in &grouped {
            let actual = source.read(cx).version();
            if actual != *base_version {
                return Err(TransactionError::VersionMismatch {
                    expected: *base_version,
                    actual,
                }
                .into());
            }
            // 只读源同样必须在任何源提交前拒绝，否则先提交的源会留下部分提交文本。
            if source.read(cx).is_read_only() {
                return Err(StorageError::ReadOnly.into());
            }
        }

        let mut edited_source_ids = Vec::with_capacity(grouped.len());
        for (source, _, source_edits) in grouped {
            source.update(cx, |source, cx| {
                source.edit(source_edits, metadata.clone(), cx)
            })?;
            edited_source_ids.push(source.entity_id());
        }

        // 组合编辑写回工作区源后，立即从 MultiBuffer 拥有的订阅拉取同一批变化。
        // 稍后到达的 LanguageBuffer 事件只负责唤醒，不再携带或重放增量。
        // hunk 变化由 BufferDiffEvent::DiffChanged 异步驱动物化；
        // 本轮回传的映射即编辑后、重物化前的坐标系，选区落位不依赖 diff 重建时机。
        for source_id in edited_source_ids {
            self.synchronize_source_change(source_id, Some(&edited_excerpts), false, cx)
                .ok_or_else(|| TextError::InvariantViolation {
                    location: "MultiBuffer::edit",
                    detail: "源 Buffer 已提交编辑但 MultiBuffer 订阅未收到变化".to_string(),
                })?;
        }
        Ok(())
    }

    /// 源文件路径变化后，按各源当前路径重建映射项（低频操作，允许整体重排）。
    fn rebuild_display(&mut self, cx: &mut Context<Self>) {
        let before = self.projection_trees();
        let old_version = self.state.projection_version;
        let mut path_keys = std::mem::take(&mut self.state.path_keys);
        let mut path_key_indices = std::mem::take(&mut self.state.path_key_indices);
        let mut entries = self.state.excerpts.iter().cloned().collect::<Vec<_>>();
        for excerpt in &mut entries {
            let source = &mut self.state.sources[excerpt.source_index];
            let path = path_key_for_source(source.entity.read(cx));
            source.path = path.clone();
            excerpt.path = path.clone();
            excerpt.display_path = path.clone();
            excerpt.path_index = intern_path(&mut path_keys, &mut path_key_indices, &path);
        }
        entries.sort_by(|a, b| Ord::cmp(&a.path, &b.path));
        let total = entries.len();
        for (index, excerpt) in entries.iter_mut().enumerate() {
            excerpt.adds_newline = index + 1 < total;
        }
        self.state.path_keys = path_keys;
        self.state.path_key_indices = path_key_indices;
        self.state.excerpts = SumTree::from_iter(entries, ());
        self.rebuild_all_diff_transforms(cx);
        self.publish_projection_edit(&before, old_version);
        self.emit_projection_changed(cx);
    }

    pub fn start_transaction(&mut self, cx: &mut Context<Self>) -> TextResult<TransactionId> {
        if self.history_owner() == HistoryOwner::SourceBuffer {
            let source = self
                .singleton_source
                .as_ref()
                .expect("共享源历史必须有工作区源");
            return source
                .update(cx, |source, _| source.start_transaction())?
                .ok_or_else(|| TextError::InvariantViolation {
                    location: "MultiBuffer::start_transaction",
                    detail: "工作区源 Buffer 已有活动事务".to_string(),
                });
        }
        // 先收集需要开启底层事务的源，避免与后续对 state 的可变借用冲突。
        let sources = {
            let mut cursor =
                MultiBufferCursor::new(&self.state.excerpts, &self.state.diff_transforms);
            cursor.seek_output(ByteOffset::ZERO, Bias::Right);
            let mut sources = Vec::new();
            while let Some((excerpt, _)) = cursor.item() {
                if excerpt.editable {
                    sources.push(self.state.sources[excerpt.source_index].entity.clone());
                }
                cursor.next();
            }
            sources
        };
        let ExcerptState {
            next_transaction_id,
            active_transaction,
            active_source_transactions,
            ..
        } = &mut self.state;
        if active_transaction.is_some() {
            return Err(TextError::InvariantViolation {
                location: "MultiBuffer::start_transaction",
                detail: "MultiBuffer 不允许嵌套事务".to_string(),
            });
        }
        *next_transaction_id =
            next_transaction_id
                .next()
                .ok_or_else(|| TextError::InvariantViolation {
                    location: "MultiBuffer::start_transaction",
                    detail: "MultiBuffer 事务 ID 溢出".to_string(),
                })?;
        let id = *next_transaction_id;
        for source in sources {
            if active_source_transactions
                .iter()
                .any(|candidate| candidate.entity_id() == source.entity_id())
            {
                continue;
            }
            source
                .update(cx, |source, _| source.start_transaction())?
                .ok_or_else(|| TextError::InvariantViolation {
                    location: "MultiBuffer::start_transaction",
                    detail: "excerpt 底层 Buffer 已有活动事务".to_string(),
                })?;
            active_source_transactions.push(source);
        }
        *active_transaction = Some(id);
        Ok(id)
    }

    pub fn end_transaction(&mut self, cx: &mut Context<Self>) -> Option<TransactionId> {
        if self.history_owner() == HistoryOwner::SourceBuffer {
            let source = self
                .singleton_source
                .as_ref()
                .expect("共享源历史必须有工作区源");
            return source
                .update(cx, |source, _| source.end_transaction())
                .ok()
                .flatten();
        }
        let ExcerptState {
            active_transaction,
            active_source_transactions,
            undo_stack,
            redo_stack,
            ..
        } = &mut self.state;
        let id = active_transaction.take()?;
        let mut transactions = Vec::new();
        for source in active_source_transactions.drain(..) {
            if let Some(transaction_id) = source
                .update(cx, |source, _| source.end_transaction())
                .ok()
                .flatten()
            {
                transactions.push((source, transaction_id));
            }
        }
        if transactions.is_empty() {
            return None;
        }
        // 源历史合并进前一节点（如输入法组合的 MergeWithPrevious）时，组合历史同样并入前一条目，
        // 保持「一次组合会话 = 一个撤销步」；此时返回被并入条目的身份，宿主据此清理本次会话的孤儿选区记录。
        if let Some(previous) = undo_stack.last()
            && previous.buffers.len() == transactions.len()
            && previous.buffers.iter().all(|(buffer, previous_id)| {
                transactions.iter().any(|(candidate, candidate_id)| {
                    candidate.entity_id() == buffer.entity_id() && candidate_id == previous_id
                })
            })
        {
            redo_stack.clear();
            return Some(previous.id);
        }
        undo_stack.push(CompositeHistoryEntry {
            id,
            buffers: transactions,
        });
        redo_stack.clear();
        Some(id)
    }

    /// 当前历史节点的组合事务身份；无历史时为 `None`。
    ///
    /// 撤销后回退到前一条目，编辑合并进前节点时保持不变，与源 Buffer 的当前节点语义一致。
    pub fn current_history_transaction(&self, cx: &App) -> Option<TransactionId> {
        if self.history_owner() == HistoryOwner::SourceBuffer {
            let source = self
                .singleton_source
                .as_ref()
                .expect("共享源历史必须有工作区源");
            return source.read(cx).current_history_transaction_id();
        }
        self.state.undo_stack.last().map(|entry| entry.id)
    }

    pub fn undo(
        &mut self,
        cx: &mut Context<Self>,
    ) -> TextResult<Option<MultiBufferHistoryOutcome>> {
        self.replay_history(false, cx)
    }

    pub fn redo(
        &mut self,
        cx: &mut Context<Self>,
    ) -> TextResult<Option<MultiBufferHistoryOutcome>> {
        self.replay_history(true, cx)
    }

    fn replay_history(
        &mut self,
        redo: bool,
        cx: &mut Context<Self>,
    ) -> TextResult<Option<MultiBufferHistoryOutcome>> {
        if self.history_owner() == HistoryOwner::SourceBuffer {
            let source = self
                .singleton_source
                .clone()
                .expect("共享源历史必须有工作区源");
            let old_version = self.snapshot(cx).version();
            let outcome = source.update(cx, |source, cx| {
                if redo {
                    source.redo(cx)
                } else {
                    source.undo(cx)
                }
            })?;
            let Some(outcome) = outcome else {
                return Ok(None);
            };
            let source_change = self
                .synchronize_source_change(source.entity_id(), None, false, cx)
                .ok_or_else(|| TextError::InvariantViolation {
                    location: "MultiBuffer::replay_history",
                    detail: "源 Buffer 已回放历史但 MultiBuffer 订阅未收到变化".to_string(),
                })?;
            let source_map = source_change.position_map();
            return Ok(Some(MultiBufferHistoryOutcome {
                transaction_id: outcome.transaction_id(),
                // 回放结果必须携带源事务的坐标映射：调用方用它推进锚定在投影坐标
                // 上的派生状态（自动闭合区域等）。空映射会让这些锚点停留在旧偏移。
                position_map: source_map.clone(),
                old_version,
                new_version: self.snapshot(cx).version(),
            }));
        }
        let (entry, old_version) = {
            let ExcerptState {
                undo_stack,
                redo_stack,
                ..
            } = &mut self.state;
            let entry = if redo {
                redo_stack.pop()
            } else {
                undo_stack.pop()
            };
            let Some(entry) = entry else {
                return Ok(None);
            };
            (entry, self.snapshot(cx).version())
        };

        let mut replayed_source_ids = Vec::new();
        for (buffer, expected_transaction) in &entry.buffers {
            if !redo
                && buffer
                    .read(cx)
                    .current_history_transaction_id()
                    .is_none_or(|transaction_id| transaction_id != *expected_transaction)
            {
                return Err(TextError::InvariantViolation {
                    location: "MultiBuffer::undo",
                    detail: "底层 Buffer 历史已在 MultiBuffer 外部分叉".to_string(),
                });
            }
            let mut cursor =
                MultiBufferCursor::new(&self.state.excerpts, &self.state.diff_transforms);
            cursor.seek_output(ByteOffset::ZERO, Bias::Right);
            let source = loop {
                let Some((excerpt, _)) = cursor.item() else {
                    break None;
                };
                if self.state.sources[excerpt.source_index].entity.entity_id() == buffer.entity_id()
                {
                    break Some(self.state.sources[excerpt.source_index].entity.clone());
                }
                cursor.next();
            };
            let Some(source) = source else {
                return Err(TextError::InvariantViolation {
                    location: "MultiBuffer::replay_history",
                    detail: "历史 Buffer 不再属于当前组合文档源".to_string(),
                });
            };
            let outcome = source.update(cx, |source, cx| {
                if redo {
                    source.redo(cx)
                } else {
                    source.undo(cx)
                }
            })?;
            if outcome.is_none() {
                return Err(TextError::InvariantViolation {
                    location: "MultiBuffer::replay_history",
                    detail: "底层 Buffer 缺少对应的历史节点".to_string(),
                });
            }
            // 历史条目记录的是 excerpt 源的内层 Buffer；
            // source id 与 diff 文件（按 working LanguageBuffer 索引）和 excerpt 源统一身份口径。
            let source_id = source.entity_id();
            replayed_source_ids.push(source_id);
        }
        for source_id in replayed_source_ids {
            self.synchronize_source_change(source_id, None, false, cx)
                .ok_or_else(|| TextError::InvariantViolation {
                    location: "MultiBuffer::replay_history",
                    detail: "源 Buffer 已回放历史但 MultiBuffer 订阅未收到变化".to_string(),
                })?;
        }
        let position_map = PositionMap::default();
        let new_version = self.snapshot(cx).version();
        let transaction_id = entry.id;
        if redo {
            self.state.undo_stack.push(entry);
        } else {
            self.state.redo_stack.push(entry);
        }
        Ok(Some(MultiBufferHistoryOutcome {
            transaction_id,
            position_map,
            old_version,
            new_version,
        }))
    }

    /// 快照入口：源事件只置脏，读取时按源订阅批量推进组合状态。
    ///
    /// 组合快照由 MultiBuffer 持有。
    /// 普通源编辑只替换受影响源的源快照节点和受影响的 excerpts / diff transforms 树；
    /// 源集合或路径拓扑变化才重建源表。
    pub fn snapshot(&mut self, cx: &mut Context<Self>) -> MultiBufferSnapshot {
        self.sync_pending_sources(cx);
        self.finish_projection_sync(cx);
        self.sync_snapshot_from_state();
        self.snapshot.clone()
    }

    fn sync_pending_sources(&mut self, cx: &mut Context<Self>) {
        if self.state.pending_source_syncs.is_empty() {
            return;
        }
        let source_syncs = std::mem::take(&mut self.state.pending_source_syncs);
        for (source_id, sync) in source_syncs {
            self.synchronize_source_change(source_id, None, sync.refresh_metadata, cx);
        }
    }

    fn sync_snapshot_from_state(&mut self) {
        if !self.snapshot_dirty {
            return;
        }

        // Zed 的 MultiBufferSnapshot 同时拥有 excerpts、变换树和每个源的
        // BufferSnapshot。这里必须整帧替换，不能先更新源表、再等待 excerpts
        // 树在下一次读取时补齐，否则一个快照会把旧范围解析到新文本上。
        let source_snapshot = |source: &ExcerptSource| ExcerptSourceSnapshot {
            text: source.text.clone(),
            syntax: source.syntax.clone(),
            highlight_cache: Arc::clone(&source.highlight_cache),
            word_boundary: source.word_boundary,
            settings: Arc::clone(&source.settings),
            capture_map: Arc::clone(&source.capture_map),
        };
        let source_updates = self.snapshot_source_updates.take();
        let (excerpt_sources, source_indices) = match source_updates {
            None => (
                TreeMap::from_ordered_entries(
                    self.state
                        .sources
                        .iter()
                        .enumerate()
                        .map(|(index, source)| (index, source_snapshot(source)))
                        .collect::<Vec<_>>(),
                ),
                Arc::new(
                    self.state
                        .sources
                        .iter()
                        .enumerate()
                        .map(|(index, source)| (source.entity.entity_id(), index))
                        .collect(),
                ),
            ),
            Some(updates) => {
                let mut excerpt_sources = self.snapshot.excerpt_sources.clone();
                for source_index in updates {
                    let source = &self.state.sources[source_index];
                    excerpt_sources
                        .update(&source_index, |current| *current = source_snapshot(source));
                }
                (excerpt_sources, Arc::clone(&self.snapshot.source_indices))
            }
        };
        self.snapshot = MultiBufferSnapshot {
            projection_version: self.state.projection_version,
            topology_version: self.state.topology_version,
            excerpts: self.state.excerpts.clone(),
            diff_transforms: self.state.diff_transforms.clone(),
            path_keys: Arc::from(self.state.path_keys.clone()),
            excerpt_sources,
            source_indices,
            capture_names: Arc::clone(&self.state.capture_names),
            metadata_version: self.state.metadata_epoch,
            diff_display: self.diff.clone(),
            show_headers: self.show_headers,
            singleton: self.singleton_source.is_some(),
        };
        self.snapshot_dirty = false;
    }

    /// 普通编辑器的工作区源（展开 diff 时作为新侧输入）。
    pub fn singleton_source(&self) -> Option<Entity<LanguageBuffer>> {
        self.singleton_source.clone()
    }

    /// 组合文档首个源的语言注册表；空组合文档返回 None。
    ///
    /// 需要创建关联语言 Buffer 的消费方复用本组合文档已经使用的注册表。
    pub fn language_registry(&self, cx: &App) -> Option<Arc<LanguageRegistry>> {
        self.state
            .sources
            .first()
            .map(|source| source.entity.read(cx).language_registry())
    }

    pub fn is_read_only(&self) -> bool {
        self.capability.is_read_only()
    }

    pub fn is_dirty(&self, cx: &App) -> bool {
        self.state.excerpts.iter().any(|excerpt| {
            excerpt.editable
                && self.state.sources[excerpt.source_index]
                    .entity
                    .read(cx)
                    .is_dirty()
        })
    }

    /// 文档实际引用的、可落盘的底层文件 Buffer，按源实体去重。
    pub fn file_buffers(&self, cx: &App) -> Vec<(Entity<LanguageBuffer>, PathBuf)> {
        let mut buffers = Vec::<(Entity<LanguageBuffer>, PathBuf)>::new();
        for entry in self.state.excerpts.iter().filter(|entry| entry.editable) {
            let source = self.state.sources[entry.source_index].entity.clone();
            let Some(path) = source.read(cx).file_path() else {
                continue;
            };
            if !buffers
                .iter()
                .any(|(existing, _)| existing.entity_id() == source.entity_id())
            {
                buffers.push((source, path));
            }
        }
        buffers
    }

    pub fn subscribe_and_snapshot(
        &mut self,
        cx: &mut Context<Self>,
    ) -> (MultiBufferSubscription, MultiBufferSnapshot) {
        let snapshot = self.snapshot(cx);
        let subscription = self.state.projection_changes.subscribe(snapshot.version());
        (subscription, snapshot)
    }

    /// 组合文档标题。
    ///
    /// 显式标题优先；未设置时由文档身份派生（当前文件路径的文件名）。
    /// 标题归文档模型所有，展示层直接消费。
    pub fn title(&self, cx: &App) -> Option<String> {
        self.title.clone().or_else(|| {
            self.file_path(cx).and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
        })
    }

    /// 设置显式标题；`None` 恢复按文档身份派生。
    pub fn set_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        if self.title == title {
            return;
        }
        self.title = title;
        cx.notify();
    }

    pub fn file_path(&self, cx: &App) -> Option<PathBuf> {
        // 普通编辑器：从工作区源推导文件路径；
        // 无工作区源时退回第一个可编辑片段（DiffView 等组合视图）。
        self.singleton_source
            .as_ref()
            .and_then(|source| source.read(cx).file_path())
            .or_else(|| {
                self.state
                    .excerpts
                    .iter()
                    .filter(|entry| entry.editable)
                    .find_map(|entry| {
                        self.state.sources[entry.source_index]
                            .entity
                            .read(cx)
                            .file_path()
                    })
            })
    }

    pub fn location_for_offset(
        &self,
        offset: impl Into<MultiBufferOffset>,
    ) -> Option<ExcerptLocation> {
        let offset: MultiBufferOffset = offset.into();
        let range = MultiBufferRange::new(offset, offset).expect("同点组合范围必须有效");
        self.location_for_range(range)
    }

    /// 把当前组合偏移锚定到底层源坐标；affinity 决定同点插入的吸附方向。
    pub fn anchor_at(
        &self,
        offset: impl Into<MultiBufferOffset>,
        affinity: Affinity,
    ) -> MultiBufferAnchor {
        let offset: MultiBufferOffset = offset.into();
        anchor_in_mappings(
            &self.state.excerpts,
            &self.state.diff_transforms,
            self.state.sources.as_slice(),
            offset.into(),
            affinity,
        )
        .unwrap_or_else(|| MultiBufferAnchor::boundary(offset))
    }

    /// 把稳定锚点定位到当前组合坐标。
    ///
    /// 源或 excerpt 退出当前投影时按路径顺序定位到相邻结构边界；
    /// 空投影定位到文首。源 Anchor 的版本链无法推进时返回错误。
    pub fn anchor_offset(&self, anchor: &MultiBufferAnchor) -> TextResult<MultiBufferOffset> {
        resolve_anchor_in_mappings(
            &self.state.excerpts,
            &self.state.diff_transforms,
            &self.state.path_keys,
            self.state.sources.as_slice(),
            anchor,
        )
        .map(|resolution| resolution.offset().into())
    }

    /// 只在锚点仍属于当前可见源片段时返回组合坐标。
    pub fn projected_anchor_offset(
        &self,
        anchor: &MultiBufferAnchor,
    ) -> TextResult<Option<MultiBufferOffset>> {
        resolve_anchor_in_mappings(
            &self.state.excerpts,
            &self.state.diff_transforms,
            &self.state.path_keys,
            self.state.sources.as_slice(),
            anchor,
        )
        .map(|resolution| resolution.projected_offset().map(Into::into))
    }

    /// 把组合文档中的选区映射回同一个源片段；跨片段选区没有单一源位置。
    pub fn location_for_range(&self, range: MultiBufferRange) -> Option<ExcerptLocation> {
        let (mapping, _) = mapping_at_tree(
            &self.state.excerpts,
            &self.state.diff_transforms,
            range.start().into(),
        )?;
        let starts_inside = range.start() >= mapping.output_range.start();
        let ends_inside = range.end() <= mapping.output_range.end();
        let empty_point_inside = !range.is_empty()
            || range.start() < mapping.output_range.end()
            || mapping.output_range.end()
                == MultiBufferOffset::new(self.state.diff_transforms.summary().output.len);
        if !(starts_inside && ends_inside && empty_point_inside) {
            return None;
        }
        let source_start = ByteOffset::new(
            (mapping.source_range.start().get()
                + range
                    .start()
                    .get()
                    .saturating_sub(mapping.output_range.start().get()))
            .min(mapping.source_range.end().get()),
        );
        let source_end = ByteOffset::new(
            (mapping.source_range.start().get()
                + range
                    .end()
                    .get()
                    .saturating_sub(mapping.output_range.start().get()))
            .min(mapping.source_range.end().get()),
        );
        Some(ExcerptLocation {
            path: mapping.path.as_path().to_path_buf(),
            source_range: TextRange::new(source_start, source_end).expect("源范围必须有效"),
        })
    }

    /// 当前 ordered excerpts 中真实内容匹配在组合坐标中的范围。
    ///
    /// 从 excerpt 树的权威 match_ranges 按需派生，不维护文档级扁平副本。
    pub fn match_ranges(&self) -> Vec<MultiBufferRange> {
        self.match_ranges_from_tree()
    }

    /// 更新工作区源的文件路径并重建投影（路径参与 excerpt 元数据与锚点解析）。
    pub fn set_file_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(source) = self.singleton_source.clone() else {
            return;
        };
        source.update(cx, |source, cx| source.set_file_path(path, cx));
        self.rebuild_display(cx);
    }

    /// `offset` 处所在 excerpt 的源语言名（组合文档按光标所在源文件显示语言）。
    pub fn language_at(&self, offset: MultiBufferOffset, cx: &App) -> Option<&'static str> {
        let offset: MultiBufferOffset = offset;
        let mapping = self.mapping_at(ByteOffset::new(offset.get()))?;
        self.state
            .sources
            .get(mapping.source_index)?
            .entity
            .read(cx)
            .language_name()
    }

    /// 定位组合偏移所属的映射；最后一个映射的结束偏移视为命中（光标位于文档末尾）。
    fn mapping_at(&self, offset: ByteOffset) -> Option<ExcerptMapping> {
        mapping_at_tree(&self.state.excerpts, &self.state.diff_transforms, offset)
            .map(|(mapping, _)| mapping)
    }

    /// `offset` 处所在 excerpt 源语言的自动闭合对。
    pub fn auto_close_pairs(
        &self,
        offset: ByteOffset,
        cx: &App,
    ) -> Option<&'static [AutoClosePair]> {
        let mapping = self.mapping_at(offset)?;
        let source = self.state.sources.get(mapping.source_index)?;
        Some(source.entity.read(cx).language()?.auto_close_pairs())
    }
}

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
            column_utf16: output.last_line_len_utf16,
            input_item_index: cursor.start().count,
            input_text: excerpts.summary().text,
            input_offset: ExcerptOffset::new(excerpts.summary().text.len),
            ..MappingPosition::default()
        };
        let mut region = logical.output_region();
        let end = logical.source_range.end();
        region.source_range = ExcerptContext::new(
            logical.source_range.version(),
            TextRange::new(end, end).expect("源末端范围必须有效"),
            false,
        );
        region.source_start_line += logical.text_summary.lines;
        region.text_summary = MBTextSummary::default();
        region.adds_newline = false;
        return Some((region.to_mapping(at.clone()), at));
    }
    let mut cursor = MultiBufferCursor::new(excerpts, tree);
    cursor.seek_output(offset, Bias::Right);
    if cursor.item().is_none() {
        // 偏移在文档末尾（或之后）：命中最后一个映射。
        let mut last = MultiBufferCursor::new(excerpts, tree);
        // Right bias 保留末尾零长度 excerpt；它们仍然拥有自己的文件身份和边界行。
        last.seek_output(ByteOffset::new(tree.summary().output.len), Bias::Right);
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
    let mut last = MultiBufferCursor::new(excerpts, tree);
    last.seek_output(ByteOffset::new(tree.summary().output.len), Bias::Right);
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
    line: usize,
) -> Option<(ExcerptMapping, MappingPosition)> {
    let mut cursor = MultiBufferCursor::new(excerpts, tree);
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

    let mut cursor = MultiBufferCursor::new(excerpts, tree);
    cursor.seek_output(origin.output_range.start().into(), Bias::Right);
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

impl SourceTexts for [ExcerptSourceSnapshot] {
    fn source_text(&self, source_index: usize) -> Option<&Snapshot> {
        self.get(source_index).map(|source| &source.text)
    }
}

impl SourceTexts for TreeMap<usize, ExcerptSourceSnapshot> {
    fn source_text(&self, source_index: usize) -> Option<&Snapshot> {
        self.get(&source_index).map(|source| &source.text)
    }
}

/// 在给定投影→源映射中把投影偏移锚定到源坐标。
///
/// 供 [`MultiBuffer::anchor_at`] 与 [`MultiBufferSnapshot::anchor_at`] 共用。
fn anchor_in_mappings<S: SourceTexts + ?Sized>(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    sources: &S,
    offset: ByteOffset,
    affinity: Affinity,
) -> Option<MultiBufferAnchor> {
    let (mapping, at) = mapping_at_tree(excerpts, tree, offset)?;
    let source_offset = ByteOffset::new(
        (mapping.source_range.start().get() + offset.get().saturating_sub(at.bytes))
            .min(mapping.source_range.end().get()),
    );
    let anchor = Anchor::new(mapping.source_range.version(), source_offset).with_affinity(affinity);
    let text_anchor = match sources.source_text(mapping.source_index) {
        Some(text) => text.attach_insertion(anchor, source_offset),
        None => anchor,
    };
    Some(MultiBufferAnchor::excerpt(
        mapping.path_index,
        mapping.source_id,
        text_anchor,
    ))
}

/// 稳定锚点在当前投影中的解析结果。
///
/// `Detached` 是一次结构变化后的正常状态：位置仍有明确的组合文档边界，但不再属于可见源片段。
/// `Invalid` 则是源 Anchor 版本链损坏，不能猜测坐标。
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

/// 源锚点解析结果：区分“路径退出投影”（允许定位结构边界）与“锚点版本失效”（禁止猜测坐标）。
enum SourceAnchorResolution {
    /// 锚点仍绑定在投影中的源上，已解析到源偏移。
    Mapped(ByteOffset),
    /// 锚点绑定的路径或源已退出投影。
    PathNotProjected,
    /// 锚点版本无法映射到目标快照。
    Invalid(TextError),
}

/// 把锚点绑定的源 Anchor 按当前源文本快照推进到源坐标。
fn excerpt_anchor_source_offset<S: SourceTexts + ?Sized>(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    path_keys: &[PathKey],
    sources: &S,
    anchor: &ExcerptAnchor,
) -> SourceAnchorResolution {
    let Some(path_key) = path_keys.get(anchor.path.get() as usize) else {
        return SourceAnchorResolution::PathNotProjected;
    };
    // 删除 hunk 只存在于输出变换：锚点绑定的旧侧源也必须经输出投影查找。
    let mut cursor = MultiBufferCursor::new(excerpts, tree);
    cursor.seek_path(path_key, Bias::Left);
    while let Some((excerpt, _)) = cursor.item() {
        if &excerpt.path != path_key {
            break;
        }
        if excerpt.source_id == anchor.source_id {
            let Some(text) = sources.source_text(excerpt.source_index) else {
                return SourceAnchorResolution::PathNotProjected;
            };
            return match anchor.text_anchor.resolve_in(text) {
                Ok(offset) => SourceAnchorResolution::Mapped(offset),
                Err(error) => SourceAnchorResolution::Invalid(error),
            };
        }
        cursor.next();
    }
    SourceAnchorResolution::PathNotProjected
}

/// 找出锚点所属的源文本快照；路径或源已退出投影时返回 None。
fn source_snapshot_for_anchor<'a, S: SourceTexts + ?Sized>(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    path_keys: &[PathKey],
    sources: &'a S,
    anchor: &ExcerptAnchor,
) -> Option<&'a Snapshot> {
    let path_key = path_keys.get(anchor.path.get() as usize)?;
    let mut cursor = MultiBufferCursor::new(excerpts, tree);
    cursor.seek_path(path_key, Bias::Left);
    while let Some((excerpt, _)) = cursor.item() {
        if &excerpt.path != path_key {
            break;
        }
        if excerpt.source_id == anchor.source_id {
            return sources.source_text(excerpt.source_index);
        }
        cursor.next();
    }
    None
}
/// 用源文本的稳定插入身份比较同路径同源的锚点。
///
/// 源已退出当前投影时回退到版本 + 偏移 + affinity 的稳定身份，仍然不解析组合坐标。
fn stable_excerpt_text_cmp(
    snapshot: &MultiBufferSnapshot,
    left: &ExcerptAnchor,
    right: &ExcerptAnchor,
) -> Ordering {
    // 调用方已保证 path 与 source_id 相同，因此两个锚点必属同一源快照。
    let source = source_snapshot_for_anchor(
        &snapshot.excerpts,
        &snapshot.diff_transforms,
        &snapshot.path_keys,
        &snapshot.excerpt_sources,
        left,
    );
    match source {
        Some(source) => source.stable_anchor_cmp(&left.text_anchor, &right.text_anchor),
        None => left
            .text_anchor
            .version()
            .cmp(&right.text_anchor.version())
            .then_with(|| left.text_anchor.offset().cmp(&right.text_anchor.offset()))
            .then_with(|| {
                left.text_anchor
                    .affinity()
                    .cmp(&right.text_anchor.affinity())
            }),
    }
}

/// 锚点绑定路径退出投影后，按当前路径顺序定位到相邻结构边界。
///
/// 实现 Zed `summary_for_anchor` 的 `Missing` 语义：
/// 没有后继则取前驱末尾；整个投影为空时，组合文档唯一的稳定边界就是文首。
fn structural_offset_for_detached_anchor(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    path_keys: &[PathKey],
    path: PathKeyIndex,
) -> TextResult<ByteOffset> {
    let anchor_key =
        path_keys
            .get(path.get() as usize)
            .ok_or_else(|| TextError::InvariantViolation {
                location: "MultiBuffer::anchor_offset",
                detail: "组合 Anchor 的路径索引不属于当前 MultiBuffer".to_string(),
            })?;
    let mut following: Option<&PathKey> = None;
    let mut preceding: Option<&PathKey> = None;
    for key in path_keys {
        if key == anchor_key || first_mapping_for_path(excerpts, tree, key).is_none() {
            continue;
        }
        if key > anchor_key {
            if following.is_none_or(|current| key < current) {
                following = Some(key);
            }
        } else if preceding.is_none_or(|current| key > current) {
            preceding = Some(key);
        }
    }
    if let Some(path_key) = following
        && let Some((output_start, _)) = first_mapping_for_path(excerpts, tree, path_key)
    {
        return Ok(ByteOffset::new(output_start));
    }
    if let Some(path_key) = preceding
        && let Some((output_end, _)) = last_mapping_for_path(excerpts, tree, path_key)
    {
        return Ok(ByteOffset::new(output_end));
    }
    Ok(ByteOffset::ZERO)
}

/// 源坐标仍位于当前可见片段时，把它投影为组合输出偏移。
///
/// 半开区间的边界归后一个片段；最后一个片段的末尾仍归它自身。
fn projected_output_offset_for_source(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    path_keys: &[PathKey],
    path: PathKeyIndex,
    source_id: Option<gpui::EntityId>,
    source_offset: ByteOffset,
) -> Option<ByteOffset> {
    let path_key = path_keys.get(path.get() as usize)?;
    let mut cursor = MultiBufferCursor::new(excerpts, tree);
    cursor.seek_path(path_key, Bias::Left);
    let mut last_matching = None;
    while let Some((excerpt, _)) = cursor.item() {
        if &excerpt.path != path_key {
            break;
        }
        if source_id.is_none_or(|source_id| excerpt.source_id == Some(source_id)) {
            let mapping = cursor.mapping().expect("双坐标游标必须有对应映射");
            if mapping.source_range.start() <= source_offset
                && source_offset < mapping.source_range.end()
            {
                return Some(ByteOffset::new(
                    cursor.start().bytes
                        + source_offset
                            .get()
                            .saturating_sub(mapping.source_range.start().get()),
                ));
            }
            last_matching = Some((cursor.start().bytes, mapping));
        }
        cursor.next();
    }
    let (output_start, mapping) = last_matching?;
    (mapping.source_range.end() == source_offset).then(|| {
        ByteOffset::new(
            output_start
                + source_offset
                    .get()
                    .saturating_sub(mapping.source_range.start().get()),
        )
    })
}

fn nearest_output_offset_for_source(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    path_keys: &[PathKey],
    path: PathKeyIndex,
    source_id: Option<gpui::EntityId>,
    source_offset: ByteOffset,
) -> Option<ByteOffset> {
    let path_key = path_keys.get(path.get() as usize)?;
    // 按路径 seek 时同时累加输出字节，直接得到每个 item 的输出起点。
    let mut cursor = MultiBufferCursor::new(excerpts, tree);
    cursor.seek_path(path_key, Bias::Left);
    let mut matching: Vec<(usize, ExcerptMapping)> = Vec::new();
    while let Some((excerpt, _)) = cursor.item() {
        if &excerpt.path != path_key {
            break;
        }
        if source_id.is_none_or(|source_id| excerpt.source_id == Some(source_id)) {
            matching.push((
                cursor.start().bytes,
                cursor.mapping().expect("双坐标游标必须有对应映射"),
            ));
        }
        cursor.next();
    }

    // 源位置仍在可见 excerpt 内时，不应进入“最近”逻辑。
    // 半开区间的边界属于后一段；
    // 只有最后一段的结束位置仍归最后一段，保证删除前一行后光标落在下一行开头，而不是回跳到前一段。
    if let Some((output_start, mapping)) = matching
        .iter()
        .find(|(_, mapping)| {
            mapping.source_range.start() <= source_offset
                && source_offset < mapping.source_range.end()
        })
        .map(|(output_start, mapping)| (*output_start, mapping.clone()))
        .or_else(|| {
            matching
                .last()
                .map(|(output_start, mapping)| (*output_start, mapping.clone()))
                .filter(|(_, mapping)| mapping.source_range.end() == source_offset)
        })
    {
        return Some(ByteOffset::new(
            output_start
                + source_offset
                    .get()
                    .saturating_sub(mapping.source_range.start().get()),
        ));
    }

    matching
        .into_iter()
        .map(|(output_start, mapping)| {
            let clamped = ByteOffset::new(
                source_offset
                    .get()
                    .max(mapping.source_range.start().get())
                    .min(mapping.source_range.end().get()),
            );
            let distance = source_offset.get().abs_diff(clamped.get());
            // 回退时边界并列仍优先「以该源偏移为起点」的片段。
            let at_end_only = source_offset.get() == mapping.source_range.end().get()
                && source_offset.get() != mapping.source_range.start().get();
            let output = ByteOffset::new(
                output_start
                    + clamped
                        .get()
                        .saturating_sub(mapping.source_range.start().get()),
            );
            (distance, at_end_only, output)
        })
        .min_by_key(|(distance, at_end_only, _)| (*distance, *at_end_only))
        .map(|(_, _, output)| output)
}

/// 在给定投影→源映射中把源锚点解析回投影偏移。
///
/// 同一文件仍存在时优先解析到源位置；
/// 源位置已离开可见 excerpt 或路径退出投影时，按当前结构定位相邻边界。
/// 源 Anchor 版本无法映射时显式失败，不进入结构定位。
/// [`MultiBuffer::anchor_offset`]（当前映射）与 [`MultiBufferSnapshot::anchor_offset`]（快照映射）共用此解析逻辑。
fn resolve_anchor_in_mappings<S: SourceTexts + ?Sized>(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    path_keys: &[PathKey],
    sources: &S,
    anchor: &MultiBufferAnchor,
) -> TextResult<AnchorResolution> {
    let excerpt_anchor = match anchor {
        MultiBufferAnchor::Min => return Ok(AnchorResolution::Projected(ByteOffset::ZERO)),
        MultiBufferAnchor::Max => {
            return Ok(AnchorResolution::Projected(ByteOffset::new(
                tree.summary().output.len,
            )));
        }
        MultiBufferAnchor::Excerpt(excerpt_anchor) => excerpt_anchor,
    };
    match excerpt_anchor_source_offset(excerpts, tree, path_keys, sources, excerpt_anchor) {
        SourceAnchorResolution::Mapped(source_offset) => {
            if let Some(offset) = projected_output_offset_for_source(
                excerpts,
                tree,
                path_keys,
                excerpt_anchor.path,
                excerpt_anchor.source_id,
                source_offset,
            ) {
                return Ok(AnchorResolution::Projected(offset));
            }
            if let Some(offset) = nearest_output_offset_for_source(
                excerpts,
                tree,
                path_keys,
                excerpt_anchor.path,
                excerpt_anchor.source_id,
                source_offset,
            ) {
                return Ok(AnchorResolution::Detached(offset));
            }
            if let Some(offset) = nearest_output_offset_for_source(
                excerpts,
                tree,
                path_keys,
                excerpt_anchor.path,
                None,
                source_offset,
            ) {
                return Ok(AnchorResolution::Detached(offset));
            }
            structural_offset_for_detached_anchor(excerpts, tree, path_keys, excerpt_anchor.path)
                .map(AnchorResolution::Detached)
        }
        // 路径退出投影：按当前路径顺序定位结构边界。
        SourceAnchorResolution::PathNotProjected => {
            structural_offset_for_detached_anchor(excerpts, tree, path_keys, excerpt_anchor.path)
                .map(AnchorResolution::Detached)
        }
        // 锚点版本无法映射：禁止进入结构定位猜测坐标。
        SourceAnchorResolution::Invalid(error) => Err(error),
    }
}

/// 路径区间内的第一个映射及其输出起点（路径不存在时 None）。
fn first_mapping_for_path(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    path_key: &PathKey,
) -> Option<(usize, ExcerptMapping)> {
    let mut cursor = MultiBufferCursor::new(excerpts, tree);
    cursor.seek_path(path_key, Bias::Left);
    let (excerpt, _) = cursor.item()?;
    (&excerpt.path == path_key).then(|| {
        (
            cursor.start().bytes,
            cursor.mapping().expect("双坐标游标必须有对应映射"),
        )
    })
}

/// 路径区间内的最后一个映射及其输出终点（路径不存在时 None）。
fn last_mapping_for_path(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    path_key: &PathKey,
) -> Option<(usize, ExcerptMapping)> {
    let mut cursor = MultiBufferCursor::new(excerpts, tree);
    // Right 定位到路径区间之后（或末尾），回退一个即该路径的最后一个映射。
    cursor.seek_path(path_key, Bias::Right);
    cursor.prev();
    let (excerpt, _) = cursor.item()?;
    if &excerpt.path != path_key {
        return None;
    }
    let end = cursor.start().bytes + excerpt.text_summary.len + excerpt.adds_newline as usize;
    Some((end, excerpt.to_mapping(cursor.start().clone())))
}

#[cfg(test)]
#[path = "test/multi_buffer_tests.rs"]
mod tests;
