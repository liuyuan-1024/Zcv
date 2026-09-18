//! Editor 与具体文本 Buffer 之间的组合文档边界。
//!
//! 组合文档按调用方给出的顺序组织多个来源的 excerpts，并保留组合坐标到源文件坐标的映射。
//! 普通编辑器是「整文件单 excerpt」的组合文档；多文件差异视图在此重排显示 excerpts。
//! Editor 始终只消费本层，不感知来源数量。
//! diff 显示拓扑（git hunks、展开状态、跟踪区间与显示坐标）只服务需要重排 excerpts 的组合文档，见 [`diff_projection`]。

mod diff_projection;
mod path_key;

pub use diff_projection::{DiffFile, DiffHunkSource, DisplayHunk};
pub use path_key::{PathKey, PathKeyIndex};

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use gpui::{App, Context, Entity, EventEmitter, Subscription};
use sum_tree::{Bias, ContextLessSummary, Cursor, Dimension, Item, SeekTarget, SumTree};
use unicode_segmentation::UnicodeSegmentation;
use zcv_buffer_diff::{DiffHunkKind, DiffHunkStaging, DiffRefresh};
use zcv_language::{
    AutoClosePair, BracketPair, HighlightCache, HighlightSpan, LanguageBuffer, LanguageBufferEvent,
    LanguageBufferSnapshot, LanguageRegistry, LanguageSettings, LocalBinding, NewlineIndent,
    OutlineItem, OutlineTextRange, SyntaxNode, SyntaxSnapshot,
};
use zcv_text::{
    Affinity, Anchor, Buffer, BufferVersion, ByteOffset, CharOffset, CoordinateError, Edit, Line,
    LineEndingStyle, LogicalColumn, MovementDirection, MovementUnit, Position, PositionMap,
    Snapshot, Stickiness, StorageError, TextChangeBatch, TextError, TextRange, TextRead,
    TextResult, TextSubscription, TransactionId, TransactionMetadata, Utf16Offset, Utf16Position,
    WordBoundaryPolicy,
};

/// 组合文档中的一个源片段。
#[derive(Clone)]
pub struct ExcerptRange {
    source: Entity<LanguageBuffer>,
    source_range: TextRange,
    match_ranges: Vec<TextRange>,
    display_path: Option<PathKey>,
    editable: bool,
    starts_new_excerpt: bool,
    diff_kind: Option<ExcerptDiffKind>,
    /// diff 投影物化时标注的 hunk 身份；普通 excerpt 为空。
    /// 同一节点可同时承担「前驱纯删除的边界」与「自身内容 hunk」，因此按 hunk 顺序保存。
    diff_hunks: Vec<DiffTransformHunkInfo>,
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
            starts_new_excerpt: true,
            diff_kind: None,
            diff_hunks: Vec::new(),
        }
    }

    pub fn with_display_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.display_path = Some(PathKey::new(path.into()));
        self
    }

    /// 标记该片段是否接受组合编辑；diff 旧侧片段只参与选择、复制和导航。
    pub fn with_editable(mut self, editable: bool) -> Self {
        self.editable = editable;
        self
    }

    /// 同一可见 excerpt 可由多个连续来源片段构成，只有首片段创建文件标题或 excerpt 分隔块。
    pub fn with_starts_new_excerpt(mut self, starts_new_excerpt: bool) -> Self {
        self.starts_new_excerpt = starts_new_excerpt;
        self
    }

    pub fn with_diff_kind(mut self, diff_kind: ExcerptDiffKind) -> Self {
        self.diff_kind = Some(diff_kind);
        self
    }

    /// diff 投影物化时追加该片段承担的 hunk 身份；普通 excerpt 不携带。
    pub(crate) fn with_diff_hunk(mut self, hunk: DiffTransformHunkInfo) -> Self {
        self.diff_hunks.push(hunk);
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
        let text = source.read(cx).buffer().read(cx).snapshot();
        Self::line_range_from_text(source, &text, lines)
    }

    /// 从已读取的源文本快照取行范围。
    ///
    /// `line_range` 内部复用；diff 投影物化旧侧行时也经此构造 excerpt。
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
}

/// 一个源文档的去重共享状态：文本、语法与 capture 映射各保存一份，
/// 该源的所有 excerpt 映射只引用 `source_index`，避免同一文件大量搜索片段重复克隆。
#[derive(Clone, Debug)]
struct ExcerptSource {
    /// 源语言 Buffer 实体（更新时按 id 定位）。
    entity: Entity<LanguageBuffer>,
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
/// 身份绑定 working 源与 `BufferDiffSnapshot::visible_hunks()` 下标，不随组合文档序号或源范围变化。
/// 输出行/字节范围由 `derive_diff_display` 的游标推导；节点不保存绝对输出坐标，也不保存源坐标副本。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiffTransformHunkInfo {
    working: gpui::EntityId,
    hunk_index: Option<usize>,
    side: DiffTransformHunkSide,
    kind: DiffHunkKind,
    staging: DiffHunkStaging,
    base_lines: Range<usize>,
    base_byte_start: usize,
    buffer_word_diffs: Vec<Range<Anchor>>,
    base_word_diffs: Vec<Range<usize>>,
    expanded: bool,
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
    /// 无后继内容节点时挂到前驱内容节点；主范围取本节点终点。
    BoundaryEnd,
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
    /// 片段在源文档中的锚点范围；权威是源 Anchor，裸 TextRange 只是当前解析缓存。
    source_range: ExcerptContext,
    source_start_line: usize,
    /// 片段真实内容的多维摘要（不含分隔用的合成换行）。
    text_summary: MBTextSummary,
    /// 片段末尾是否为分隔补出了一个合成换行；决定该 item 在组合文本中的输出长度。
    adds_newline: bool,
    /// 该片段内真实内容匹配的源范围（搜索高亮用；diff 片段为空）。
    match_ranges: Vec<TextRange>,
    /// 指向源表（`ExcerptState::sources` / 快照的 `excerpt_sources`）的索引。
    source_index: usize,
    /// 该片段所属的工作区源实体；纯文本派生快照没有实体源，因此为 `None`。
    source_id: Option<gpui::EntityId>,
    editable: bool,
    starts_new_excerpt: bool,
    diff_kind: Option<ExcerptDiffKind>,
    /// diff 投影物化时标注的 hunk 身份；普通 excerpt 为空。
    diff_hunks: Vec<DiffTransformHunkInfo>,
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

    /// 用目标源快照的版本化编辑日志把锚点范围推进到当前坐标。
    ///
    /// outside 决定边界插入是否纳入；锚点版本已被日志裁剪时返回 None。
    fn mapped(&self, snapshot: &Snapshot, outside: bool) -> Option<Self> {
        let range = self.range();
        // 位置推进必须用“自锚点版本以来的全部编辑”：范围之前的编辑同样会平移它，
        // 只取范围相交编辑（edits_since_in_range）会漏掉这些位移。
        let batch = snapshot.edits_since(self.version()).ok()?;
        let mapped = batch
            .position_map()
            .map_old_range_with_stickiness(
                range,
                if outside {
                    Stickiness::Expand
                } else {
                    Stickiness::Never
                },
            )
            .value();
        Some(Self::new(snapshot.version(), mapped, outside))
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
    entry: Excerpt,
    /// 在组合文档中的顺序位置（由树序推导，不存储在 item 上）。
    excerpt_index: usize,
    output_range: MultiBufferRange,
}

impl std::ops::Deref for ExcerptMapping {
    type Target = Excerpt;

    fn deref(&self) -> &Excerpt {
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
    },
    /// 删除块只存在于输出坐标：它不消费输入 excerpt，自带删除内容以便物化。
    DeletedHunk {
        summary: DiffTransformSummary,
        excerpt: Box<Excerpt>,
    },
}

impl DiffTransform {
    /// 从输入侧 excerpt 构造独立的输出变换摘要。
    fn from_excerpt(excerpt: &Excerpt) -> Self {
        let mut output_text = excerpt.text_summary;
        if excerpt.adds_newline {
            output_text += MBTextSummary::newline();
        }
        let output = ExcerptSummary {
            text: output_text,
            count: 1,
            path_key: excerpt.path.clone(),
        };
        let input = if excerpt.diff_kind == Some(ExcerptDiffKind::Deleted) {
            // 删除块不占输入坐标：输入摘要必须为零，否则输入游标会被它推进。
            ExcerptSummary {
                path_key: excerpt.path.clone(),
                ..ExcerptSummary::default()
            }
        } else {
            excerpt.summary(())
        };
        let summary = DiffTransformSummary { input, output };
        if excerpt.diff_kind == Some(ExcerptDiffKind::Deleted) {
            Self::DeletedHunk {
                summary,
                excerpt: Box::new(excerpt.clone()),
            }
        } else {
            Self::BufferContent { summary }
        }
    }

    fn transform_summary(&self) -> &DiffTransformSummary {
        match self {
            Self::BufferContent { summary, .. } | Self::DeletedHunk { summary, .. } => summary,
        }
    }

    /// 输出侧片段内容；只有 BufferContent 从输入树读取。
    fn excerpt<'a>(&'a self, input: Option<&'a Excerpt>) -> Option<&'a Excerpt> {
        match self {
            Self::BufferContent { .. } => input,
            Self::DeletedHunk { excerpt, .. } => Some(excerpt),
        }
    }
}

impl Excerpt {
    /// 在给定输出坐标起点上派生对外片段快照；快照只是树的只读视图，不反向写回。
    fn to_snapshot(&self, at: MappingPosition) -> ExcerptSnapshot {
        let separator = self.adds_newline as usize;
        let len = self.text_summary.len + separator;
        ExcerptSnapshot {
            path: self.path.clone(),
            display_path: self.display_path.clone(),
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
            starts_new_excerpt: self.starts_new_excerpt,
            diff_kind: self.diff_kind,
        }
    }

    /// 在给定游标位置（输出字节 + 输出行起点）上构造派生视图。
    fn to_mapping(&self, at: MappingPosition) -> ExcerptMapping {
        let separator = self.adds_newline as usize;
        let len = self.text_summary.len + separator;
        ExcerptMapping {
            entry: self.clone(),
            excerpt_index: at.index,
            output_range: MultiBufferRange::new(
                MultiBufferOffset::new(at.bytes),
                MultiBufferOffset::new(at.bytes + len),
            )
            .expect("组合片段输出范围必须正序"),
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
}

impl MBTextSummary {
    const fn newline() -> Self {
        Self {
            len: 1,
            chars: 1,
            len_utf16: 1,
            lines: 1,
        }
    }
}

impl std::ops::AddAssign for MBTextSummary {
    fn add_assign(&mut self, other: Self) {
        self.len += other.len;
        self.chars += other.chars;
        self.len_utf16 += other.len_utf16;
        self.lines += other.lines;
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

impl ExcerptSummary {
    fn add(&mut self, summary: &Self) {
        debug_assert!(
            summary.path_key >= self.path_key,
            "变换节点必须按路径升序排列：{:?} 之后出现了 {:?}",
            self.path_key,
            summary.path_key,
        );
        self.text += summary.text;
        self.count += summary.count;
        self.path_key = summary.path_key.clone();
    }
}

/// Zed 式输入坐标到输出坐标的变换摘要。
///
/// `input` 是基础 excerpts 的坐标长度，`output` 是 Editor 实际消费的坐标长度。
/// 普通内容节点两者相同；删除 hunk 只存在于 output，因此 input 为空而 output
/// 仍保留删除文本。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct DiffTransformSummary {
    input: ExcerptSummary,
    output: ExcerptSummary,
}

impl ContextLessSummary for DiffTransformSummary {
    fn zero() -> Self {
        Self::default()
    }

    fn add_summary(&mut self, summary: &Self) {
        self.input.add(&summary.input);
        self.output.add(&summary.output);
    }
}

impl Item for Excerpt {
    type Summary = ExcerptSummary;

    /// 输入 excerpts 树只描述源内容坐标；
    /// 片段间为显示边界补出的合成换行属于输出变换，不在输入摘要中重复计入。
    /// 删除块只在输出坐标存在，输入贡献为零。
    fn summary(&self, _cx: ()) -> Self::Summary {
        if self.diff_kind == Some(ExcerptDiffKind::Deleted) {
            ExcerptSummary {
                text: MBTextSummary::default(),
                count: 0,
                path_key: self.path.clone(),
            }
        } else {
            ExcerptSummary {
                text: self.text_summary,
                count: 1,
                path_key: self.path.clone(),
            }
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

impl Dimension<'_, DiffTransformSummary> for PathKey {
    fn zero(_: ()) -> Self {
        Self::min()
    }

    fn add_summary(&mut self, summary: &DiffTransformSummary, _: ()) {
        *self = summary.output.path_key.clone();
    }
}

impl SeekTarget<'_, DiffTransformSummary, DiffTransformSummary> for PathKey {
    fn cmp(&self, cursor_location: &DiffTransformSummary, _: ()) -> Ordering {
        Ord::cmp(self, &cursor_location.output.path_key)
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
        self.0 += summary.output.text.len;
    }
}

/// 组合文本的逻辑行号。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MultiBufferRow(pub usize);

impl MultiBufferRow {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: usize) -> Self {
        Self(value)
    }

    pub const fn get(self) -> usize {
        self.0
    }
}

/// 组合文本中的一个点：逻辑行 + 行内字节列。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MultiBufferPoint {
    pub row: MultiBufferRow,
    pub column: usize,
}

/// 输入 excerpts 空间的字节偏移；删除块不占该空间。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExcerptOffset(pub usize);

impl ExcerptOffset {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: usize) -> Self {
        Self(value)
    }

    pub const fn get(self) -> usize {
        self.0
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

/// 组合层与文本协议之间的区间坐标转换。
///
/// 两者都是半开字节区间，只是坐标空间不同；跨空间必须显式调用。
pub trait RangeOffsetExt {
    fn into_multi_buffer_range(self) -> Range<MultiBufferOffset>;
    fn into_byte_range(self) -> Range<ByteOffset>;
}

impl RangeOffsetExt for Range<ByteOffset> {
    fn into_multi_buffer_range(self) -> Range<MultiBufferOffset> {
        MultiBufferOffset::new(self.start.get())..MultiBufferOffset::new(self.end.get())
    }

    fn into_byte_range(self) -> Range<ByteOffset> {
        self
    }
}

impl RangeOffsetExt for Range<MultiBufferOffset> {
    fn into_multi_buffer_range(self) -> Range<MultiBufferOffset> {
        self
    }

    fn into_byte_range(self) -> Range<ByteOffset> {
        ByteOffset::new(self.start.get())..ByteOffset::new(self.end.get())
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
impl_offset_ops!(MultiBufferRow);
impl_offset_ops!(MultiBufferCharOffset);
impl_offset_ops!(MultiBufferOffsetUtf16);
impl_offset_ops!(ExcerptOffset);

/// 在路径有序树上按累积输出行 seek。
///
/// 供逻辑行坐标查询直接读权威树，不再依赖快照内另存的片段数组。
impl Dimension<'_, DiffTransformSummary> for MultiBufferRow {
    fn zero(_: ()) -> Self {
        Self(0)
    }

    fn add_summary(&mut self, summary: &DiffTransformSummary, _: ()) {
        self.0 += summary.output.text.lines;
    }
}

/// 组合输出 Unicode scalar 偏移维度。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct MultiBufferCharOffset(usize);

impl Dimension<'_, DiffTransformSummary> for MultiBufferCharOffset {
    fn zero(_: ()) -> Self {
        Self(0)
    }

    fn add_summary(&mut self, summary: &DiffTransformSummary, _: ()) {
        self.0 += summary.output.text.chars;
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
        self.0 += summary.output.text.len_utf16;
    }
}

/// 同时累加输出字节、输出行、顺序位置与路径的游标维度。
///
/// 以 `MultiBufferOffset` 为 seek 目标可读回该处的坐标/序号起点；
/// 以 `PathKey` 为 seek 目标可按路径定位区间。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct MappingPosition {
    bytes: usize,
    chars: usize,
    utf16: usize,
    lines: usize,
    index: usize,
    input_index: usize,
    path: PathKey,
}

impl Dimension<'_, DiffTransformSummary> for MappingPosition {
    fn zero(_: ()) -> Self {
        Self::default()
    }

    fn add_summary(&mut self, summary: &DiffTransformSummary, _: ()) {
        self.bytes += summary.output.text.len;
        self.chars += summary.output.text.chars;
        self.utf16 += summary.output.text.len_utf16;
        self.lines += summary.output.text.lines;
        self.index += summary.output.count;
        self.input_index += summary.input.count;
        self.path = summary.output.path_key.clone();
    }
}

impl SeekTarget<'_, DiffTransformSummary, MappingPosition> for MultiBufferOffset {
    fn cmp(&self, cursor_location: &MappingPosition, _: ()) -> Ordering {
        Ord::cmp(&self.0, &cursor_location.bytes)
    }
}

impl SeekTarget<'_, DiffTransformSummary, MappingPosition> for MultiBufferRow {
    fn cmp(&self, cursor_location: &MappingPosition, _: ()) -> Ordering {
        Ord::cmp(&self.0, &cursor_location.lines)
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

impl SeekTarget<'_, DiffTransformSummary, MappingPosition> for PathKey {
    fn cmp(&self, cursor_location: &MappingPosition, _: ()) -> Ordering {
        Ord::cmp(self, &cursor_location.path)
    }
}

/// 取路径的稳定索引；首次出现时登记，之后永不变更。
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

fn mapping_count(mappings: &SumTree<DiffTransform>) -> usize {
    mappings.summary().output.count
}

type ProjectionTrees = (SumTree<Excerpt>, SumTree<DiffTransform>);

fn projection_items_equal(
    old_excerpt: &Excerpt,
    old_transform: &DiffTransform,
    new_excerpt: &Excerpt,
    new_transform: &DiffTransform,
) -> bool {
    old_excerpt.path == new_excerpt.path
        && old_excerpt.path_index == new_excerpt.path_index
        && old_excerpt.display_path == new_excerpt.display_path
        && old_excerpt.source_range == new_excerpt.source_range
        && old_excerpt.source_start_line == new_excerpt.source_start_line
        && old_excerpt.text_summary == new_excerpt.text_summary
        && old_excerpt.adds_newline == new_excerpt.adds_newline
        && old_excerpt.match_ranges == new_excerpt.match_ranges
        && old_excerpt.source_id == new_excerpt.source_id
        && old_excerpt.editable == new_excerpt.editable
        && old_excerpt.starts_new_excerpt == new_excerpt.starts_new_excerpt
        && old_excerpt.diff_kind == new_excerpt.diff_kind
        && old_excerpt.diff_hunks == new_excerpt.diff_hunks
        && matches!(
            (old_transform, new_transform),
            (
                DiffTransform::BufferContent { .. },
                DiffTransform::BufferContent { .. }
            ) | (
                DiffTransform::DeletedHunk { .. },
                DiffTransform::DeletedHunk { .. }
            )
        )
}

/// 通过前后两棵投影树的公共前缀/后缀推导结构编辑范围。
///
/// 比较只沿 transform 游标前后移动，不物化全文或扁平映射数组；返回范围始终对齐
/// excerpt 边界。源文本内部的精确编辑仍由 `TextChangeBatch` 单独投影。
fn projection_changed_ranges(
    before: &ProjectionTrees,
    after: &ProjectionTrees,
) -> (TextRange, TextRange) {
    let old_count = mapping_count(&before.1);
    let new_count = mapping_count(&after.1);
    let common_limit = old_count.min(new_count);
    let mut common_prefix = 0usize;
    let mut old_start = 0usize;
    let mut new_start = 0usize;
    let mut old_cursor = MultiBufferCursor::new(&before.0, &before.1);
    let mut new_cursor = MultiBufferCursor::new(&after.0, &after.1);
    old_cursor.seek_excerpt_index(0);
    new_cursor.seek_excerpt_index(0);
    while common_prefix < common_limit {
        let Some((old_excerpt, old_transform)) = old_cursor.item() else {
            break;
        };
        let Some((new_excerpt, new_transform)) = new_cursor.item() else {
            break;
        };
        if !projection_items_equal(old_excerpt, old_transform, new_excerpt, new_transform) {
            break;
        }
        common_prefix += 1;
        old_cursor.next();
        new_cursor.next();
        old_start = old_cursor.start().bytes;
        new_start = new_cursor.start().bytes;
    }

    let mut common_suffix = 0usize;
    while common_prefix + common_suffix < common_limit {
        let old_index = old_count - common_suffix - 1;
        let new_index = new_count - common_suffix - 1;
        old_cursor.seek_excerpt_index(old_index);
        new_cursor.seek_excerpt_index(new_index);
        let Some((old_excerpt, old_transform)) = old_cursor.item() else {
            break;
        };
        let Some((new_excerpt, new_transform)) = new_cursor.item() else {
            break;
        };
        if !projection_items_equal(old_excerpt, old_transform, new_excerpt, new_transform) {
            break;
        }
        common_suffix += 1;
    }

    let old_end = if common_suffix == 0 {
        before.1.summary().output.text.len
    } else {
        old_cursor.seek_excerpt_index(old_count - common_suffix);
        old_cursor.start().bytes
    };
    let new_end = if common_suffix == 0 {
        after.1.summary().output.text.len
    } else {
        new_cursor.seek_excerpt_index(new_count - common_suffix);
        new_cursor.start().bytes
    };
    (
        TextRange::new(ByteOffset::new(old_start), ByteOffset::new(old_end))
            .expect("旧投影结构编辑范围必须正序"),
        TextRange::new(ByteOffset::new(new_start), ByteOffset::new(new_end))
            .expect("新投影结构编辑范围必须正序"),
    )
}

/// 以输出游标遍历指定源的映射，避免为一次源编辑拍平整棵组合树。
fn mappings_for_source(
    excerpts: &SumTree<Excerpt>,
    entries: &SumTree<DiffTransform>,
    source_id: gpui::EntityId,
) -> Vec<ExcerptMapping> {
    let mut cursor = MultiBufferCursor::new(excerpts, entries);
    cursor.seek_output(ByteOffset::ZERO, Bias::Right);
    let mut mappings = Vec::new();
    while let Some((excerpt, _)) = cursor.item() {
        if excerpt.source_id == Some(source_id) {
            mappings.push(cursor.mapping().expect("双坐标游标必须有对应映射"));
        }
        cursor.next();
    }
    mappings
}

/// 按显示 excerpt 序号从 transform 树定位映射；序号由树摘要累计得到。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct ExcerptIndex(usize);

impl SeekTarget<'_, DiffTransformSummary, MappingPosition> for ExcerptIndex {
    fn cmp(&self, cursor_location: &MappingPosition, _: ()) -> Ordering {
        Ord::cmp(&self.0, &cursor_location.index)
    }
}

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

    fn sync_excerpts(&mut self) {
        if self.diff_transforms.item().is_none() {
            return;
        }
        self.excerpts.seek(
            &ExcerptIndex(self.diff_transforms.start().input_index),
            Bias::Right,
        );
        // 删除块不占输入坐标；输入游标只停在真实消费输入的 excerpt 上。
        while self
            .excerpts
            .item()
            .is_some_and(|excerpt| excerpt.diff_kind == Some(ExcerptDiffKind::Deleted))
        {
            self.excerpts.next();
        }
    }

    fn seek_output(&mut self, offset: ByteOffset, bias: Bias) {
        self.diff_transforms
            .seek(&MultiBufferOffset(offset.get()), bias);
        self.sync_excerpts();
    }

    fn seek_output_line(&mut self, line: usize, bias: Bias) {
        self.diff_transforms.seek(&MultiBufferRow(line), bias);
        self.sync_excerpts();
    }

    fn seek_output_char(&mut self, offset: CharOffset, bias: Bias) {
        self.diff_transforms
            .seek(&MultiBufferCharOffset(offset.get()), bias);
        self.sync_excerpts();
    }

    fn seek_output_utf16(&mut self, offset: Utf16Offset, bias: Bias) {
        self.diff_transforms
            .seek(&MultiBufferOffsetUtf16(offset.get()), bias);
        self.sync_excerpts();
    }

    fn seek_path(&mut self, path: &PathKey, bias: Bias) {
        self.diff_transforms.seek(path, bias);
        self.sync_excerpts();
    }

    fn seek_excerpt_index(&mut self, index: usize) {
        self.diff_transforms.seek(&ExcerptIndex(index), Bias::Right);
        self.sync_excerpts();
    }

    fn next(&mut self) {
        self.diff_transforms.next();
        self.sync_excerpts();
    }

    fn prev(&mut self) {
        self.diff_transforms.prev();
        self.sync_excerpts();
    }

    fn item(&self) -> Option<(&Excerpt, &DiffTransform)> {
        let transform = self.diff_transforms.item()?;
        let excerpt = transform.excerpt(self.excerpts.item())?;
        Some((excerpt, transform))
    }

    fn start(&self) -> &MappingPosition {
        self.diff_transforms.start()
    }

    fn mapping(&self) -> Option<ExcerptMapping> {
        let (excerpt, _) = self.item()?;
        Some(excerpt.to_mapping(self.start().clone()))
    }
}

fn mapping_at_excerpt_index(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    index: usize,
) -> Option<ExcerptMapping> {
    let mut cursor = MultiBufferCursor::new(excerpts, tree);
    cursor.seek_excerpt_index(index);
    cursor.mapping()
}

/// 返回编辑范围末端实际覆盖的 excerpt。
///
/// 输出偏移落在 excerpt 之间的合成换行时，编辑仍应结束于前一个源 excerpt，
/// 不能把后一个 excerpt 的首字符误判为编辑终点。
fn mapping_at_edit_end(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    end: ByteOffset,
) -> Option<ExcerptMapping> {
    let target = ByteOffset::new(end.get().checked_sub(1)?);
    let mapping = mapping_at_tree(excerpts, tree, target).map(|(mapping, _)| mapping)?;
    if mapping.output_range.start().get() > target.get() {
        mapping
            .excerpt_index
            .checked_sub(1)
            .and_then(|index| mapping_at_excerpt_index(excerpts, tree, index))
    } else {
        Some(mapping)
    }
}

/// 只物化一次编辑范围覆盖的 transform 节点；不会创建整份组合映射数组。
fn mappings_between_indices(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    start: usize,
    end: usize,
) -> Vec<ExcerptMapping> {
    let mut cursor = MultiBufferCursor::new(excerpts, tree);
    cursor.seek_excerpt_index(start);
    let mut mappings = Vec::with_capacity(end.saturating_sub(start) + 1);
    while cursor.item().is_some() {
        if cursor.start().index > end {
            break;
        }
        mappings.push(cursor.mapping().expect("双坐标游标必须有对应映射"));
        cursor.next();
    }
    mappings
}

/// 在权威映射树上物化对外片段快照，输出坐标由累积 Summary 派生。
///
/// 仅在需要随机访问或向显示层移交所有权时调用；组合坐标查询直接用树游标，不经过本函数。
fn snapshot_excerpts(
    source_excerpts: &SumTree<Excerpt>,
    entries: &SumTree<DiffTransform>,
) -> Vec<ExcerptSnapshot> {
    let mut at = MappingPosition::default();
    let mut excerpts = Vec::with_capacity(entries.summary().output.count);
    let mut cursor = MultiBufferCursor::new(source_excerpts, entries);
    cursor.seek_output(ByteOffset::ZERO, Bias::Right);
    while let Some((excerpt, transform)) = cursor.item() {
        excerpts.push(excerpt.to_snapshot(at.clone()));
        at.add_summary(&transform.summary(()), ());
        cursor.next();
    }
    excerpts
}

/// 用 `entries` 替换输入 excerpts 树上 `path` 区间的全部 item；其余路径的子树原样保留。
///
/// item 不存储绝对输出坐标，因此 splice 不需要触碰下游 item。
fn splice_excerpt_entries(
    tree: &SumTree<Excerpt>,
    path: &PathKey,
    entries: Vec<Excerpt>,
) -> SumTree<Excerpt> {
    let mut cursor = tree.cursor::<ExcerptSummary>(());
    let mut new_tree = cursor.slice(path, Bias::Left);
    cursor.seek(path, Bias::Right);
    new_tree.extend(entries, ());
    new_tree.append(cursor.suffix(), ());
    new_tree
}

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
    // 空片段没有源内容可显示，但必须在组合文档中占一个空行；
    // 否则删除点占位行会被相邻行吸收，折叠后的删除块不可见。
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

/// 多文件文档中一个可见片段的一帧元数据。
///
/// 文本仍通过组合投影供编辑器的折叠、换行和命中测试使用；
/// 路径、源行号、语法与边界保持为一等数据，不能编码进投影文本。
/// Editor 的通用文件标题和片段分隔块只消费本结构。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExcerptSnapshot {
    path: PathKey,
    display_path: PathKey,
    output_range: MultiBufferRange,
    source_range: TextRange,
    output_start_line: usize,
    output_end_line: usize,
    source_start_line: usize,
    /// 指向快照 `excerpt_sources` 的索引；内部坐标换算用，不在公开 API 暴露。
    source_index: usize,
    editable: bool,
    starts_new_excerpt: bool,
    diff_kind: Option<ExcerptDiffKind>,
}

impl ExcerptSnapshot {
    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    pub fn display_path(&self) -> &Path {
        self.display_path.as_path()
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

    pub fn is_editable(&self) -> bool {
        self.editable
    }

    pub fn starts_new_excerpt(&self) -> bool {
        self.starts_new_excerpt
    }

    pub fn diff_kind(&self) -> Option<ExcerptDiffKind> {
        self.diff_kind
    }

    pub fn source_line_for_output_line(&self, output_line: usize) -> Option<usize> {
        (self.diff_kind != Some(ExcerptDiffKind::Deleted) && output_line >= self.output_start_line)
            .then(|| self.source_start_line + 1 + output_line - self.output_start_line)
    }
}

/// 一帧组合文档的不可变快照。
#[derive(Clone, Debug)]
pub struct MultiBufferSnapshot {
    projection_version: BufferVersion,
    /// 输入侧 excerpts 的权威快照；源坐标由自身 Summary 派生。
    excerpts: SumTree<Excerpt>,
    /// 由输入 excerpts 派生的输出变换树；输出坐标由累积 Summary 派生。
    diff_transforms: SumTree<DiffTransform>,
    /// 由权威树惰性物化的片段视图；同一版本内多次读取共用一份派生结果。
    excerpts_cache: Arc<OnceLock<Arc<[ExcerptSnapshot]>>>,
    /// 路径索引表：PathKeyIndex 对应的路径，供锚点解析按路径 seek。
    path_keys: Arc<[PathKey]>,
    /// 按源去重的源快照表（映射经 `source_index` 引用）。
    ///
    /// 文本与语法属于同一源快照；
    /// `metadata_version` 只随非文本状态（语法安装、元数据变化）推进，
    /// 纯文本编辑由 `projection_version` 表达；显示层据此替换只读附属数据而不重建显示拓扑。
    excerpt_sources: Arc<[ExcerptSourceSnapshot]>,
    capture_names: Arc<[Arc<str>]>,
    metadata_version: u64,
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
        let Some(old_version) = state.pending_old_version.take() else {
            return TextChangeBatch::default();
        };
        state
            .pending_batch
            .take()
            .unwrap_or_else(|| TextChangeBatch::reset(old_version, state.current_version))
    }
}

#[derive(Debug, Clone)]
struct ProjectionSubscriptionState {
    current_version: BufferVersion,
    pending_old_version: Option<BufferVersion>,
    /// 单次源编辑换算到组合坐标后的增量批次；多次变化合并或整体重建时为 None。
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
        batch: Option<TextChangeBatch>,
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
                    // 首个待消费变化可携带增量批次；多次变化合并为整体重载。
                    state.pending_old_version = Some(old_version);
                    state.pending_batch = batch.clone();
                } else {
                    state.pending_batch = None;
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
    buffers: Vec<(Entity<Buffer>, TransactionId)>,
}

/// MultiBuffer 拥有的源文本增量游标。
///
/// GPUI 事件只负责唤醒；
/// 连续版本和 Patch 必须由该独立订阅拉取，避免把延迟派发的旧事件当成当前坐标事实。
struct SourceSubscription {
    source: Entity<LanguageBuffer>,
    text: TextSubscription,
}

/// MultiBuffer 对消费方公开的文本与语法更新边界。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MultiBufferEvent {
    TextChanged,
    Reparsed,
    MetadataChanged,
    /// diff 展开/折叠状态变化（宿主按展开状态重建组合片段，如 ProjectDiffView）。
    DiffExpansionChanged,
}

impl MultiBufferSnapshot {
    /// 主源语言的词边界策略；无源时返回默认。
    ///
    /// 全文搜索等不携带具体位置的消费方使用它；按位置消费方用 `word_boundary_at`。
    pub fn word_boundary(&self) -> WordBoundaryPolicy {
        self.excerpt_sources
            .first()
            .map_or_else(WordBoundaryPolicy::default, |source| source.word_boundary)
    }

    /// 指定组合偏移所属源语言的词边界策略。
    pub fn word_boundary_at(&self, offset: MultiBufferOffset) -> WordBoundaryPolicy {
        self.source_point(offset.into())
            .map_or_else(WordBoundaryPolicy::default, |(_, source, _)| {
                source.word_boundary
            })
    }

    /// 主源语言解析后的编辑器设置（对齐 Zed `LanguageSettings::for_buffer`）。
    pub fn language_settings(&self) -> Arc<LanguageSettings> {
        self.excerpt_sources.first().map_or_else(
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
    /// 这个值由最后一个 excerpt 的派生输出范围决定，不读取或复制组合文本。
    pub fn len_bytes(&self) -> MultiBufferOffset {
        ByteOffset::new(self.diff_transforms.summary().output.text.len).into()
    }

    /// 虚拟组合文本的逻辑行数。
    ///
    /// excerpt 映射在建立时已经累计了输出行边界，因此这里不扫描、更不拼接所有源文本。
    pub fn line_count(&self) -> usize {
        // 末尾片段的输出行终点等于 Summary 累积行数；总行数比终点多一行。
        self.diff_transforms.summary().output.text.lines + 1
    }

    /// 把组合偏移转换为逻辑行。
    ///
    /// 该查询只遍历覆盖请求范围的源 chunk；
    /// 它是 DisplayMap 迁出物化 `Snapshot` 后的基础坐标入口。
    pub fn byte_to_line(&self, offset: MultiBufferOffset) -> TextResult<Line> {
        self.ensure_output_boundary(offset)?;
        if self.excerpts.is_empty() {
            return Ok(Line::ZERO);
        }
        let (entry, at) =
            mapping_covering_output_end(&self.excerpts, &self.diff_transforms, offset.get())
                .ok_or(CoordinateError::OutOfBounds(offset.into()))?;
        let content_end = ByteOffset::new(at.bytes + entry.source_range.len());
        let source = self
            .excerpt_sources
            .get(entry.source_index)
            .ok_or(CoordinateError::OutOfBounds(offset.into()))?;
        // 片段末尾（含为分隔补出的合成换行）按源范围末端定位：
        // 区间终点必须落在最后一条内容行上，不能提前跳到下一片段。
        let source_offset = if offset >= content_end.into() {
            entry.source_range.end()
        } else {
            ByteOffset::new(entry.source_range.start().get() + offset.get() - at.bytes)
        };
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
        if target == Line::ZERO {
            return Ok(ByteOffset::ZERO.into());
        }

        let (entry, at) =
            mapping_at_output_line(&self.excerpts, &self.diff_transforms, target.get())
                .ok_or(CoordinateError::LineOutOfBounds(target))?;
        let source = self
            .excerpt_sources
            .get(entry.source_index)
            .ok_or(CoordinateError::LineOutOfBounds(target))?;
        let source_line = entry.source_start_line + target.get() - at.lines;
        let source_start = source.text.line_start_byte(Line::new(source_line))?;
        let relative = source_start
            .get()
            .saturating_sub(entry.source_range.start().get());
        Ok(ByteOffset::new(at.bytes + relative).into())
    }

    /// 把组合字节偏移转换为按 Unicode scalar value 计数的逻辑位置。
    pub fn byte_to_position(&self, offset: MultiBufferOffset) -> TextResult<Position> {
        let line = self.byte_to_line(offset)?;
        let line_start = self.line_start_byte(line)?;
        let column = self
            .bytes_in_range(line_start..offset)
            .map(|chunk| chunk.text.chars().count())
            .sum();
        Ok(Position::new(line, LogicalColumn::new(column)))
    }

    /// 返回组合文本中的 Tree-sitter 风格字节坐标。
    pub fn byte_to_point(&self, offset: MultiBufferOffset) -> TextResult<(Line, usize)> {
        let line = self.byte_to_line(offset)?;
        let line_start = self.line_start_byte(line)?;
        Ok((line, offset.get() - line_start.get()))
    }

    /// 组合文本末端坐标（最后一个逻辑行与行内字节列）。
    pub fn max_point(&self) -> MultiBufferPoint {
        let (line, column) = self
            .byte_to_point(self.len_bytes())
            .expect("组合文本末端必须是有效坐标");
        MultiBufferPoint {
            row: MultiBufferRow::new(line.get()),
            column,
        }
    }

    /// 组合范围的文本多维摘要。
    ///
    /// 由当前快照的坐标查询差分得到，不构造临时字符串。
    pub fn text_summary_for_range(&self, range: MultiBufferRange) -> TextResult<MBTextSummary> {
        let chars = self
            .byte_to_char(range.end())?
            .get()
            .saturating_sub(self.byte_to_char(range.start())?.get());
        let len_utf16 = self
            .byte_to_utf16_cu(range.end())?
            .get()
            .saturating_sub(self.byte_to_utf16_cu(range.start())?.get());
        let lines = self
            .byte_to_line(range.end())?
            .get()
            .saturating_sub(self.byte_to_line(range.start())?.get());
        Ok(MBTextSummary {
            len: range.len(),
            chars,
            len_utf16,
            lines,
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
        self.ensure_output_boundary(offset)?;
        if let Some((entry, at)) =
            mapping_covering_output_end(&self.excerpts, &self.diff_transforms, offset.get())
        {
            let content_start = ByteOffset::new(at.bytes);
            let content_end = ByteOffset::new(content_start.get() + entry.source_range.len());
            if content_start <= offset.into() && offset < content_end.into() {
                let source = self
                    .excerpt_sources
                    .get(entry.source_index)
                    .ok_or(CoordinateError::OutOfBounds(offset.into()))?;
                let source_offset = ByteOffset::new(
                    entry.source_range.start().get() + offset.get() - content_start.get(),
                );
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
        }
        // 片段之间的静态换行块或文档末尾：退回按偏移切分的块。
        self.bytes_in_range(offset..self.len_bytes())
            .next()
            .map(|chunk| (chunk.text, chunk.output_range.start))
            .ok_or(CoordinateError::OutOfBounds(offset.into()).into())
    }

    /// 把组合逻辑位置转换为字节偏移。
    pub fn position_to_byte(&self, position: Position) -> TextResult<MultiBufferOffset> {
        let line_start = self.line_start_byte(position.line())?;
        let line_end = if position.line().get() + 1 < self.line_count() {
            self.line_start_byte(Line::new(position.line().get() + 1))?
        } else {
            self.len_bytes()
        };
        let mut column = 0usize;
        for chunk in self.bytes_in_range(line_start..line_end) {
            for (offset, character) in chunk.text.char_indices() {
                if column == position.column().get() {
                    return Ok(ByteOffset::new(chunk.output_range.start.get() + offset).into());
                }
                if character == '\n' {
                    return Err(CoordinateError::OutOfBounds(ByteOffset::new(
                        chunk.output_range.start.get() + offset,
                    ))
                    .into());
                }
                column += 1;
            }
        }
        if column == position.column().get() {
            Ok(line_end)
        } else {
            Err(CoordinateError::OutOfBounds(line_end.into()).into())
        }
    }

    pub fn byte_to_char(&self, offset: MultiBufferOffset) -> TextResult<CharOffset> {
        self.ensure_output_boundary(offset)?;
        if offset == self.len_bytes() {
            return Ok(CharOffset::new(
                self.diff_transforms.summary().output.text.chars,
            ));
        }
        let mut cursor = MultiBufferCursor::new(&self.excerpts, &self.diff_transforms);
        cursor.seek_output(offset.into(), Bias::Right);
        let at = cursor.start().clone();
        let chars = self
            .bytes_in_range(ByteOffset::new(at.bytes).into()..offset)
            .map(|chunk| chunk.text.chars().count())
            .sum::<usize>();
        Ok(CharOffset::new(at.chars + chars))
    }

    pub fn char_to_byte(&self, target: CharOffset) -> TextResult<MultiBufferOffset> {
        let total = self.diff_transforms.summary().output.text.chars;
        if target.get() > total {
            return Err(CoordinateError::CharOutOfBounds(target).into());
        }
        if target.get() == total {
            return Ok(self.len_bytes());
        }
        let mut cursor = MultiBufferCursor::new(&self.excerpts, &self.diff_transforms);
        cursor.seek_output_char(target, Bias::Right);
        let at = cursor.start().clone();
        let mut chars = at.chars;
        let region_end = cursor
            .item()
            .map(|(excerpt, _)| {
                ByteOffset::new(at.bytes + excerpt.text_summary.len + excerpt.adds_newline as usize)
            })
            .ok_or(CoordinateError::CharOutOfBounds(target))?;
        for chunk in self.bytes_in_range(ByteOffset::new(at.bytes).into()..region_end.into()) {
            for (offset, _) in chunk.text.char_indices() {
                if chars == target.get() {
                    return Ok(ByteOffset::new(chunk.output_range.start.get() + offset).into());
                }
                chars += 1;
            }
        }
        Err(CoordinateError::CharOutOfBounds(target).into())
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
        self.ensure_output_boundary(offset)?;
        if offset == self.len_bytes() {
            return Ok(Utf16Offset::new(
                self.diff_transforms.summary().output.text.len_utf16,
            ));
        }
        let mut cursor = MultiBufferCursor::new(&self.excerpts, &self.diff_transforms);
        cursor.seek_output(offset.into(), Bias::Right);
        let at = cursor.start().clone();
        let units = self
            .bytes_in_range(ByteOffset::new(at.bytes).into()..offset)
            .flat_map(|chunk| chunk.text.chars())
            .map(char::len_utf16)
            .sum::<usize>();
        Ok(Utf16Offset::new(at.utf16 + units))
    }

    pub fn utf16_cu_to_byte(&self, target: Utf16Offset) -> TextResult<MultiBufferOffset> {
        let total = self.diff_transforms.summary().output.text.len_utf16;
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
        let at = cursor.start().clone();
        let mut units = at.utf16;
        let region_end = cursor
            .item()
            .map(|(excerpt, _)| {
                ByteOffset::new(at.bytes + excerpt.text_summary.len + excerpt.adds_newline as usize)
            })
            .ok_or(CoordinateError::Utf16PositionOutOfBounds(
                Utf16Position::new(Line::ZERO, target),
            ))?;
        for chunk in self.bytes_in_range(ByteOffset::new(at.bytes).into()..region_end.into()) {
            for (offset, character) in chunk.text.char_indices() {
                if units == target.get() {
                    return Ok(ByteOffset::new(chunk.output_range.start.get() + offset).into());
                }
                units += character.len_utf16();
                if units > target.get() {
                    break;
                }
            }
        }
        Err(
            CoordinateError::Utf16PositionOutOfBounds(Utf16Position::new(Line::ZERO, target))
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

    fn ensure_output_boundary(&self, offset: MultiBufferOffset) -> TextResult<()> {
        if offset > self.len_bytes() {
            return Err(CoordinateError::OutOfBounds(offset.into()).into());
        }
        if offset == ByteOffset::ZERO.into() || offset == self.len_bytes() {
            return Ok(());
        }

        let (entry, at) =
            mapping_covering_output_end(&self.excerpts, &self.diff_transforms, offset.get())
                .ok_or(CoordinateError::OutOfBounds(offset.into()))?;
        let output_start = ByteOffset::new(at.bytes);
        let separator = entry.adds_newline as usize;
        let output_end = ByteOffset::new(at.bytes + entry.text_summary.len + separator);
        let source_output_end = ByteOffset::new(output_start.get() + entry.source_range.len());
        if offset > source_output_end.into() {
            return Ok(());
        }
        if offset == source_output_end.into() && source_output_end < output_end {
            return Ok(());
        }

        let source = self
            .excerpt_sources
            .get(entry.source_index)
            .ok_or(CoordinateError::OutOfBounds(offset.into()))?;
        let source_offset =
            ByteOffset::new(entry.source_range.start().get() + offset.get() - output_start.get());
        source
            .text
            .chunk_at_byte(source_offset)
            .map(|_| ())
            .map_err(|_| CoordinateError::InvalidByteBoundary(offset.into()).into())
    }

    pub fn version(&self) -> BufferVersion {
        self.projection_version
    }

    /// 在权威映射树上物化片段快照；输出坐标由累积 Summary 派生。
    ///
    /// 组合坐标查询直接用树游标；本方法只在需要随机访问或移交所有权时调用。
    pub fn excerpts(&self) -> impl Iterator<Item = ExcerptSnapshot> + '_ {
        let mut cursor = MultiBufferCursor::new(&self.excerpts, &self.diff_transforms);
        cursor.seek_output(ByteOffset::ZERO, Bias::Right);
        std::iter::from_fn(move || {
            let (excerpt, _) = cursor.item()?;
            let snapshot = excerpt.to_snapshot(cursor.start().clone());
            cursor.next();
            Some(snapshot)
        })
    }

    /// 片段快照的共享句柄；显示层持有派生视图，树仍是唯一权威。
    ///
    /// 派生结果按版本惰性缓存：文本未变时重复的显示刷新不再重新遍历树。
    pub fn excerpts_arc(&self) -> Arc<[ExcerptSnapshot]> {
        Arc::clone(
            self.excerpts_cache.get_or_init(|| {
                Arc::from(snapshot_excerpts(&self.excerpts, &self.diff_transforms))
            }),
        )
    }

    /// 指定源路径的 excerpts（按组合顺序）。
    ///
    /// 用按路径升序的映射树做路径游标定位，不扫描全部 excerpts。
    pub fn excerpts_for_path(&self, path: &Path) -> impl Iterator<Item = ExcerptSnapshot> {
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

    /// 组合输出偏移所在的 excerpt（用累积输出字节的偏移游标在路径有序树上定位）。
    pub fn excerpt_at_output_offset(&self, offset: MultiBufferOffset) -> Option<ExcerptSnapshot> {
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
            offset.into(),
            affinity,
        )
        .unwrap_or_else(|| MultiBufferAnchor::boundary(offset))
    }

    /// 把源锚点解析回快照内的组合偏移（Editor 源锚点选区：源→投影）。
    ///
    /// 源锚点选区按需解析：投影重建不改变源，选区无需重映射，用重建后快照直接解析即得当前投影偏移。
    pub fn resolve_anchor(&self, anchor: &MultiBufferAnchor) -> Option<MultiBufferOffset> {
        resolve_anchor_in_mappings(
            &self.excerpts,
            &self.diff_transforms,
            &self.path_keys,
            &self.excerpt_sources[..],
            anchor,
        )
        .map(Into::into)
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
                let source = &self.excerpt_sources[excerpt.source_index];
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

    /// 查询组合坐标中光标所在 source 的括号对，并映射回组合坐标。
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
            .iter()
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
            let mapping = excerpt.to_mapping(cursor.start().clone());
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
        let Some(source) = self.excerpt_sources.get(excerpt.source_index) else {
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
        let source = self.excerpt_sources.get(mapping.source_index)?;
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
        let source = self.excerpt_sources.get(mapping.source_index)?;
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
        let line = self.byte_to_line(offset.into())?;
        let line_start = self.line_start_byte(line)?;
        let character = self
            .bytes_in_range(line_start..offset.into())
            .flat_map(|chunk| chunk.text.chars())
            .map(char::len_utf16)
            .sum();
        Ok(Utf16Position::new(line, Utf16Offset::new(character)))
    }

    fn utf16_position_to_byte(&self, position: Utf16Position) -> TextResult<ByteOffset> {
        let line_start = self.line_start_byte(position.line())?;
        let line_end = if position.line().get() + 1 < self.line_count() {
            self.line_start_byte(Line::new(position.line().get() + 1))?
        } else {
            self.len_bytes()
        };
        let mut units = 0;
        for chunk in self.bytes_in_range(line_start..line_end) {
            for (index, character) in chunk.text.char_indices() {
                if units == position.character().get() {
                    return Ok(ByteOffset::new(chunk.output_range.start.get() + index));
                }
                units += character.len_utf16();
                if units > position.character().get() {
                    return Err(CoordinateError::InvalidUtf16Boundary(position).into());
                }
            }
        }
        (units == position.character().get())
            .then_some(line_end.into())
            .ok_or(CoordinateError::Utf16PositionOutOfBounds(position).into())
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

            let source = self.snapshot.excerpt_sources.get(entry.source_index)?;
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
            source_range: ExcerptContext::new(text.version(), range, false),
            source_start_line: 0,
            text_summary,
            adds_newline: false,
            match_ranges: Vec::new(),
            source_index: 0,
            source_id: None,
            editable: true,
            starts_new_excerpt: false,
            diff_kind: None,
            diff_hunks: Vec::new(),
        };
        let diff_transforms = SumTree::from_iter([DiffTransform::from_excerpt(&excerpt)], ());
        Self {
            projection_version: text.version(),
            excerpts: SumTree::from_iter([excerpt], ()),
            diff_transforms,
            excerpts_cache: Arc::new(OnceLock::new()),
            path_keys: Arc::from([PathKey::min()]),
            excerpt_sources: Arc::from([ExcerptSourceSnapshot {
                text,
                syntax,
                highlight_cache: Arc::new(HighlightCache::new()),
                word_boundary: WordBoundaryPolicy::default(),
                settings: Arc::new(LanguageSettings::default()),
                capture_map: Arc::from([]),
            }]),
            capture_names,
            metadata_version: 0,
        }
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
    source_event_subscriptions: Vec<Subscription>,
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
    /// 非文本状态（语法安装、捕获表等）版本；纯文本编辑不推进它。
    metadata_epoch: u64,
    projection_changes: ProjectionChangeTopic,
    next_transaction_id: TransactionId,
    active_transaction: Option<TransactionId>,
    active_source_transactions: Vec<Entity<Buffer>>,
    undo_stack: Vec<CompositeHistoryEntry>,
    redo_stack: Vec<CompositeHistoryEntry>,
}

/// 组合文档的写入能力。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
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
    /// git 行级 diff 显示拓扑（hunks、跟踪区间与显示坐标）；`None` = 无 diff 需求。
    diff: Option<Box<diff_projection::DiffDisplayCache>>,
    /// 新 hunk 的初始展开策略；只决定初始状态，不覆盖用户显式切换。
    diff_expanded_by_default: bool,
    /// 已物化进组合文档的前导文件数量（diff 以路径顺序登记，就绪前缀之外的文件尚未物化）。
    diff_materialized_files: usize,
    /// 快照相关状态的单调版本；任何 excerpt/源文本/语法变化都推进它。
    snapshot_epoch: u64,
    /// `snapshot()` 的 O(1) 缓存：epoch 未变时直接复用上一份快照。
    snapshot_cache: std::cell::RefCell<Option<(u64, MultiBufferSnapshot)>>,
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
    /// 普通编辑器的文档统一经此构造：项目共享 LanguageBuffer 只作为工作区源（source），展开 diff hunk 时的 set_excerpts 只影响本组合文档，不污染项目共享文档。
    /// 整文件片段不创建文件标题块（单文件文档无多文件边界；
    /// 标题块由多文件投影与 diff 投影按 `show_file_header` 自行声明）。
    pub fn singleton(source: Entity<LanguageBuffer>, cx: &mut Context<Self>) -> Self {
        let line_count = source.read(cx).text_snapshot(cx).line_count();
        let mut multi_buffer = Self::empty(cx);
        multi_buffer.singleton_source = Some(source.clone());
        multi_buffer.set_excerpts(
            vec![
                ExcerptRange::line_range(source.clone(), 0..line_count, cx)
                    .with_starts_new_excerpt(false),
            ],
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
            snapshot_epoch: 0,
            snapshot_cache: std::cell::RefCell::new(None),
        }
    }

    /// 空组合状态只保存 source 与其投影映射；组合文本不拥有第二份 Buffer。
    fn empty_excerpt_state(_cx: &mut Context<Self>) -> ExcerptState {
        ExcerptState {
            source_subscriptions: Vec::new(),
            source_event_subscriptions: Vec::new(),
            diff_transforms: SumTree::new(()),
            excerpts: SumTree::new(()),
            sources: Vec::new(),
            source_indices: HashMap::new(),
            path_keys: Vec::new(),
            path_key_indices: HashMap::new(),
            capture_names: Arc::from([]),
            projection_version: BufferVersion::INITIAL,
            metadata_epoch: 0,
            projection_changes: ProjectionChangeTopic::default(),
            next_transaction_id: TransactionId::INITIAL,
            active_transaction: None,
            active_source_transactions: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// 推进虚拟组合投影的版本，并唤醒各自独立的显示消费者。
    fn publish_projection_change(&mut self, incremental: Option<SourceIncremental>) {
        self.snapshot_epoch = self.snapshot_epoch.wrapping_add(1);
        let old_version = self.state.projection_version;
        let new_version = old_version
            .next()
            .expect("组合投影版本不应溢出；溢出时必须创建新文档生命周期");
        self.state.projection_version = new_version;
        let batch =
            incremental.map(|incremental| incremental.batch.rebased_to(old_version, new_version));
        self.state
            .projection_changes
            .publish(old_version, new_version, batch);
    }

    /// 用编辑前冻结的投影树与当前投影树按游标推导结构变化范围，并发布增量批次。
    ///
    /// 结构变化不物化组合文本：前后两棵树按 excerpt item 游标比对公共前后缀，
    /// 变化范围始终对齐 excerpt 边界。源文本内部的精确编辑仍由 TextChangeBatch 单独投影。
    fn publish_projection_edit(&mut self, before: &ProjectionTrees, old_version: BufferVersion) {
        let after = self.projection_trees();
        let (old_range, new_range) = projection_changed_ranges(before, &after);
        let batch =
            TextChangeBatch::from_edits(old_version, old_version, vec![(old_range, new_range)]);
        self.publish_projection_change(Some(SourceIncremental { batch }));
    }

    pub(crate) fn publish_source_projection_edit(
        &mut self,
        before: &ProjectionTrees,
        source_change: &TextChangeBatch,
    ) {
        let after = self.projection_trees();
        let (old_range, new_range) = projection_changed_ranges(before, &after);
        let batch = source_change.projected_from(vec![(old_range, new_range)]);
        self.publish_projection_change(Some(SourceIncremental { batch }));
    }

    /// 冻结当前输入/输出投影树；供结构变化前保存旧坐标、变化后推导增量范围。
    pub(crate) fn projection_trees(&self) -> ProjectionTrees {
        (
            self.state.excerpts.clone(),
            self.state.diff_transforms.clone(),
        )
    }

    /// 把一次源编辑换算到所有受影响 excerpt 的组合坐标。
    ///
    /// Zed 的 `sync_from_buffer_changes` 不要求一个源只能对应一个 excerpt；
    /// 同一源的多个可见区间会分别生成 output edit。这里保留旧、当前两帧的
    /// excerpt 顺序，按源坐标配对后再计算每个 output 区间。
    fn source_incremental_change(
        &self,
        source_id: gpui::EntityId,
        source_change: &TextChangeBatch,
        old_mappings: &[ExcerptMapping],
    ) -> Option<SourceIncremental> {
        if source_change.requires_reset() {
            return Some(SourceIncremental {
                batch: source_change.projected_from(Vec::new()),
            });
        }
        if source_change.patch().is_empty() {
            return None;
        }
        let old_mappings = old_mappings
            .iter()
            .filter(|mapping| mapping.source_id == Some(source_id))
            .collect::<Vec<_>>();
        let new_mappings =
            mappings_for_source(&self.state.excerpts, &self.state.diff_transforms, source_id);
        if old_mappings.len() != new_mappings.len() {
            return None;
        }
        let mut output_edits = Vec::new();
        for (old_mapping, new_mapping) in old_mappings.into_iter().zip(new_mappings) {
            for patch_edit in source_change.patch().edits() {
                let old_range = patch_edit.old_range();
                let excerpt_range = old_mapping.source_range.range();
                let overlap = if old_range.is_empty() {
                    (old_range.start() >= excerpt_range.start()
                        && old_range.start() < excerpt_range.end())
                    .then_some(old_range)
                } else {
                    TextRange::new(
                        old_range.start().max(excerpt_range.start()),
                        old_range.end().min(excerpt_range.end()),
                    )
                    .ok()
                    .filter(|range| !range.is_empty())
                };
                let Some(overlap) = overlap else {
                    continue;
                };

                let old_output_start = old_mapping.output_range.start().get()
                    + overlap.start().get()
                    - excerpt_range.start().get();
                let old_output_end = old_mapping.output_range.start().get() + overlap.end().get()
                    - excerpt_range.start().get();
                let new_start_source = source_change
                    .position_map()
                    .map_old_position_with_affinity(overlap.start(), Affinity::Before)
                    .value();
                let mut new_end_source = source_change
                    .position_map()
                    .map_old_position_with_affinity(overlap.end(), Affinity::After)
                    .value();

                // 如果替换范围跨越多个 excerpt，替换文本只应插入到包含旧起点的
                // 第一个 excerpt 中；后续 excerpt 只删除自己可见的旧内容。
                let owns_replacement = old_range.start() >= excerpt_range.start()
                    && old_range.start() < excerpt_range.end();
                if owns_replacement && overlap.end() < old_range.end() {
                    new_end_source =
                        ByteOffset::new(new_end_source.get() + patch_edit.new_range().len());
                }
                let new_source_range = new_mapping.source_range.range();
                let new_output_start = new_mapping.output_range.start().get()
                    + new_start_source
                        .get()
                        .saturating_sub(new_source_range.start().get())
                        .min(new_source_range.len());
                let new_output_end = new_mapping.output_range.start().get()
                    + new_end_source
                        .get()
                        .saturating_sub(new_source_range.start().get())
                        .min(new_source_range.len());
                output_edits.push((
                    TextRange::new(
                        ByteOffset::new(old_output_start),
                        ByteOffset::new(old_output_end),
                    )
                    .ok()?,
                    TextRange::new(
                        ByteOffset::new(new_output_start),
                        ByteOffset::new(new_output_end),
                    )
                    .ok()?,
                ));
            }
        }
        if output_edits.is_empty() {
            return None;
        }
        output_edits.sort_by_key(|(old, _)| old.start());
        Some(SourceIncremental {
            batch: source_change.projected_from(output_edits),
        })
    }

    /// 以给定顺序重建组合文档。每个片段都保留源文件路径和源坐标映射。
    ///
    /// 结构变化也通过 output edit 发布。订阅者因此仍能沿用同一套 DisplayMap
    /// 增量同步协议；只有订阅者已经合并了多个无法连续组合的事件时，才需要自行
    /// 将批次退化为整体重建。
    pub fn set_excerpts(&mut self, excerpts: Vec<ExcerptRange>, cx: &mut Context<Self>) {
        let before = self.projection_trees();
        let old_version = self.state.projection_version;
        self.set_excerpts_internal(excerpts, cx);
        self.publish_projection_edit(&before, old_version);
    }

    fn set_excerpts_internal(&mut self, excerpts: Vec<ExcerptRange>, cx: &mut Context<Self>) {
        let mut unique_sources = Vec::<Entity<LanguageBuffer>>::new();
        let mut unique_source_ids = HashSet::new();
        for excerpt in &excerpts {
            if unique_source_ids.insert(excerpt.source.entity_id()) {
                unique_sources.push(excerpt.source.clone());
            }
        }
        let next_source_subscriptions = unique_sources
            .iter()
            .map(|source| SourceSubscription {
                source: source.clone(),
                text: source.update(cx, |source, cx| {
                    source.buffer().update(cx, |buffer, _| buffer.subscribe())
                }),
            })
            .collect::<Vec<_>>();
        let next_source_event_subscriptions = unique_sources
            .into_iter()
            .map(|source| {
                let observed = source.clone();
                cx.subscribe(&source, move |this, _, event, cx| match event {
                    LanguageBufferEvent::TextChanged => {
                        this.source_changed(observed.entity_id(), cx)
                    }
                    LanguageBufferEvent::Reparsed => this.source_reparsed(observed.entity_id(), cx),
                    LanguageBufferEvent::MetadataChanged => {
                        // 设置/语言变化：用新快照刷新该源的设置与词边界，再通知组合层消费者。
                        this.refresh_source_snapshot(observed.entity_id(), cx);
                        cx.emit(MultiBufferEvent::MetadataChanged);
                        cx.notify();
                    }
                })
            })
            .collect::<Vec<_>>();
        let ExcerptState {
            source_subscriptions,
            source_event_subscriptions,
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
            // 无路径的临时 Buffer（单行输入框等）以空路径参与组合；
            // 路径身份用于文件级折叠、标题与锚点解析。
            let path = PathKey::new(source.file_path().unwrap_or_default());
            let source_id = excerpt.source.entity_id();
            let source_index = match next_source_indices.get(&source_id).copied() {
                Some(index) => index,
                None => {
                    let snapshot = source.snapshot(cx);
                    next_sources.push(ExcerptSource {
                        entity: excerpt.source.clone(),
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

        // 只计算组合坐标：
        // 每个非末尾片段都以完整行边界结束（内容原样投影，末尾缺换行时补一个，空片段同样适用）；
        // 末尾片段保留内容原样。空片段（空文件、折叠 hunk 占位）经此不变式自然占据边界行，不做特例补行。
        let prepared_count = prepared.len();
        let mut next_excerpts = Vec::with_capacity(prepared_count);
        for (position, item) in prepared.into_iter().enumerate() {
            let display_path = item
                .excerpt
                .display_path
                .clone()
                .unwrap_or_else(|| item.path.clone());
            let Some((text_summary, ends_with_newline)) = snapshot_range_summary(
                &next_sources[item.source_index].text,
                item.excerpt.source_range,
            ) else {
                continue;
            };
            let adds_newline = position + 1 < prepared_count && !ends_with_newline;
            let path_index = intern_path(path_keys, path_key_indices, &item.path);
            next_excerpts.push(Excerpt {
                path: item.path,
                path_index,
                display_path,
                source_range: ExcerptContext::new(
                    next_sources[item.source_index].text.version(),
                    item.excerpt.source_range,
                    false,
                ),
                source_start_line: item.start_line,
                text_summary,
                adds_newline,
                match_ranges: item.excerpt.match_ranges.clone(),
                source_index: item.source_index,
                source_id: Some(item.source_id),
                editable: item.excerpt.editable,
                starts_new_excerpt: item.excerpt.starts_new_excerpt,
                diff_kind: item.excerpt.diff_kind,
                diff_hunks: item.excerpt.diff_hunks,
            });
        }

        *source_subscriptions = next_source_subscriptions;
        *source_event_subscriptions = next_source_event_subscriptions;
        *authoritative_excerpts = SumTree::from_iter(next_excerpts, ());
        *diff_transforms = SumTree::from_iter(
            authoritative_excerpts
                .iter()
                .map(DiffTransform::from_excerpt),
            (),
        );
        *sources = next_sources;
        *source_indices = sources
            .iter()
            .enumerate()
            .map(|(index, source)| (source.entity.entity_id(), index))
            .collect();
        *composite_capture_names = rebuild_capture_table(sources);
        cx.emit(MultiBufferEvent::TextChanged);
        cx.notify();
    }

    /// 在现有组合文档末尾追加有序片段。
    ///
    /// 追加是组合文档的增量写入边界：只物化新增片段，并通过显示文本物化 Buffer 的尾部编辑提交，不重建已有映射、源订阅或整份显示文本。
    /// 需要替换顺序或删除片段时仍应使用 [`Self::set_excerpts`]。
    pub fn append_excerpts(
        &mut self,
        excerpts: Vec<ExcerptRange>,
        cx: &mut Context<Self>,
    ) -> Vec<TextRange> {
        if excerpts.is_empty() {
            return Vec::new();
        }
        let before = self.projection_trees();
        let old_version = self.state.projection_version;

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
        self.register_sources(new_sources, cx);

        struct PreparedExcerpt {
            excerpt: ExcerptRange,
            path: PathKey,
            source_index: usize,
            source_id: gpui::EntityId,
            start_line: usize,
        }

        let mut prepared = Vec::with_capacity(excerpts.len());
        for excerpt in excerpts {
            let source_id = excerpt.source.entity_id();
            let Some(&source_index) = self.state.source_indices.get(&source_id) else {
                unreachable!("追加片段的源必须已注册");
            };
            let source = &self.state.sources[source_index];
            let path = PathKey::new(source.entity.read(cx).file_path().unwrap_or_default());
            if !snapshot_range_is_valid(&source.text, excerpt.source_range) {
                continue;
            }
            let start_line = source
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
        if prepared.is_empty() {
            return Vec::new();
        }

        // 追加后的输出起点由当前树摘要推导，不读 item 上存储的绝对坐标。
        let mut existing_output_len = self.state.diff_transforms.summary().output.text.len;
        let output_ends_with_newline = {
            let mut cursor =
                MultiBufferCursor::new(&self.state.excerpts, &self.state.diff_transforms);
            cursor.seek_output(ByteOffset::new(existing_output_len), Bias::Left);
            cursor.item().is_some_and(|(mapping, _)| {
                mapping.adds_newline
                    || self.state.sources[mapping.source_index]
                        .text
                        .slice_byte_range(
                            ByteOffset::new(mapping.source_range.end().get().saturating_sub(1)),
                            mapping.source_range.end(),
                        )
                        .is_ok_and(|text| text.as_str() == "\n")
            })
        };
        if existing_output_len > 0 && !output_ends_with_newline {
            // 前一个片段此前是末尾（无合成换行），追加后它不再是末尾，补上分隔换行。
            self.state.excerpts.update_last(
                |mapping| {
                    mapping.adds_newline = true;
                },
                (),
            );
            existing_output_len += 1;
        }
        let mut next_path_keys = std::mem::take(&mut self.state.path_keys);
        let mut next_path_key_indices = std::mem::take(&mut self.state.path_key_indices);
        let prepared_count = prepared.len();
        let mut next_excerpts = Vec::with_capacity(prepared_count);
        let mut next_match_ranges = Vec::new();
        for (position, item) in prepared.into_iter().enumerate() {
            let display_path = item
                .excerpt
                .display_path
                .clone()
                .unwrap_or_else(|| item.path.clone());
            let output_start = ByteOffset::new(existing_output_len);
            let Some((text_summary, ends_with_newline)) = snapshot_range_summary(
                &self.state.sources[item.source_index].text,
                item.excerpt.source_range,
            ) else {
                continue;
            };
            let adds_newline = position + 1 < prepared_count && !ends_with_newline;
            existing_output_len += item.excerpt.source_range.len() + adds_newline as usize;
            next_match_ranges.extend(item.excerpt.match_ranges.iter().filter_map(|matched| {
                if matched.start() < item.excerpt.source_range.start()
                    || matched.end() > item.excerpt.source_range.end()
                {
                    return None;
                }
                let start = output_start.get()
                    + matched
                        .start()
                        .get()
                        .saturating_sub(item.excerpt.source_range.start().get());
                let end = output_start.get()
                    + matched
                        .end()
                        .get()
                        .saturating_sub(item.excerpt.source_range.start().get());
                TextRange::new(ByteOffset::new(start), ByteOffset::new(end)).ok()
            }));
            let path_index =
                intern_path(&mut next_path_keys, &mut next_path_key_indices, &item.path);
            next_excerpts.push(Excerpt {
                path: item.path,
                path_index,
                display_path,
                source_range: ExcerptContext::new(
                    self.state.sources[item.source_index].text.version(),
                    item.excerpt.source_range,
                    false,
                ),
                source_start_line: item.start_line,
                text_summary,
                adds_newline,
                match_ranges: item.excerpt.match_ranges.clone(),
                source_index: item.source_index,
                source_id: Some(item.source_id),
                editable: item.excerpt.editable,
                starts_new_excerpt: item.excerpt.starts_new_excerpt,
                diff_kind: item.excerpt.diff_kind,
                diff_hunks: item.excerpt.diff_hunks,
            });
        }

        self.state.path_keys = next_path_keys;
        self.state.path_key_indices = next_path_key_indices;
        self.state.excerpts.extend(next_excerpts, ());
        self.rebuild_diff_transforms_from_excerpts();
        self.publish_projection_edit(&before, old_version);
        cx.emit(MultiBufferEvent::TextChanged);
        cx.notify();
        next_match_ranges
    }

    /// 为一组同路径片段构建位置无关映射项；`start_index` 是它们在文档中的起始序号。
    fn build_entries_for_excerpts(
        &mut self,
        excerpts: Vec<ExcerptRange>,
        start_index: usize,
        total: usize,
        cx: &App,
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
            let path = PathKey::new(source.entity.read(cx).file_path().unwrap_or_default());
            let display_path = excerpt.display_path.clone().unwrap_or_else(|| path.clone());
            let Some((text_summary, ends_with_newline)) =
                snapshot_range_summary(&source.text, excerpt.source_range)
            else {
                continue;
            };
            let adds_newline = start_index + position + 1 < total && !ends_with_newline;
            let start_line = source
                .text
                .byte_to_line(excerpt.source_range.start())
                .map_or(0, |line| line.get());
            let path_index = intern_path(&mut path_keys, &mut path_key_indices, &path);
            entries.push(Excerpt {
                path,
                path_index,
                display_path,
                source_range: ExcerptContext::new(
                    source.text.version(),
                    excerpt.source_range,
                    false,
                ),
                source_start_line: start_line,
                text_summary,
                adds_newline,
                match_ranges: excerpt.match_ranges,
                source_index,
                source_id: Some(source_id),
                editable: excerpt.editable,
                starts_new_excerpt: excerpt.starts_new_excerpt,
                diff_kind: excerpt.diff_kind,
                diff_hunks: excerpt.diff_hunks,
            });
        }
        self.state.path_keys = path_keys;
        self.state.path_key_indices = path_key_indices;
        entries
    }

    /// 某个源文本变化后，只更新其所在路径的映射项并 splice 回树。
    ///
    /// 同一路径可能同时包含该源的片段与其它源（如 diff 旧侧）的片段；
    /// 这里保留其余源的条目，只按 PositionMap 推进本源的 source_range 与匹配范围。
    fn splice_source_path(
        &mut self,
        source_id: gpui::EntityId,
        source_position_map: &PositionMap,
        expanded_excerpts: Option<&HashSet<usize>>,
        cx: &App,
    ) {
        let Some(path) = self
            .state
            .sources
            .iter()
            .find(|source| source.entity.entity_id() == source_id)
            .map(|source| PathKey::new(source.entity.read(cx).file_path().unwrap_or_default()))
        else {
            return;
        };
        let start_index = {
            let mut cursor = self.state.excerpts.cursor::<ExcerptSummary>(());
            cursor.seek(&path, Bias::Left);
            cursor.start().count
        };
        let total = self.state.excerpts.summary().count;
        let mut entries = Vec::new();
        {
            let mut cursor = self.state.excerpts.cursor::<ExcerptSummary>(());
            cursor.seek(&path, Bias::Left);
            let mut local = 0usize;
            while let Some(entry) = cursor.item() {
                if entry.path != path {
                    break;
                }
                let mut entry = entry.clone();
                if entry.source_id == Some(source_id) {
                    // outside = 本次编辑落在此 excerpt（直接编辑）；其它 excerpt 不吸收边界插入。
                    let outside = expanded_excerpts
                        .is_none_or(|expanded| expanded.contains(&(start_index + local)));
                    let source = &self.state.sources[entry.source_index];
                    // 源范围的权威表示是 Anchor：用源快照的版本化编辑日志推进，
                    // 不再把订阅批次的 PositionMap 当作版本事实。日志裁剪掉锚点版本时用本次变更的映射恢复。
                    entry.source_range = entry
                        .source_range
                        .mapped(&source.text, outside)
                        .unwrap_or_else(|| {
                            ExcerptContext::new(
                                source.text.version(),
                                source_position_map
                                    .map_old_range_with_stickiness(
                                        entry.source_range.range(),
                                        if outside {
                                            Stickiness::Expand
                                        } else {
                                            Stickiness::Never
                                        },
                                    )
                                    .value(),
                                outside,
                            )
                        });
                    entry.match_ranges = entry
                        .match_ranges
                        .iter()
                        .map(|matched| {
                            source_position_map
                                .map_old_range_with_stickiness(*matched, Stickiness::Never)
                                .value()
                        })
                        .collect();
                    if let Some((text_summary, ends_with_newline)) =
                        snapshot_range_summary(&source.text, entry.source_range.range())
                    {
                        entry.text_summary = text_summary;
                        entry.source_start_line = source
                            .text
                            .byte_to_line(entry.source_range.start())
                            .map_or(0, |line| line.get());
                        entry.adds_newline = start_index + local + 1 < total && !ends_with_newline;
                    }
                }
                entries.push(entry);
                local += 1;
                cursor.next();
            }
        }
        self.state.excerpts = splice_excerpt_entries(&self.state.excerpts, &path, entries);
        self.rebuild_diff_transforms_from_excerpts();
        self.fix_document_tail_newline();
    }

    /// 组合文档末尾的片段不应再有分隔用的合成换行；splice/移除后修正末尾 item。
    ///
    /// 移除末尾路径时，前一个路径的最后一个 item 会变成文档尾，必须清掉它此前的分隔换行标记。
    fn fix_document_tail_newline(&mut self) {
        if self.state.excerpts.is_empty() {
            self.state.diff_transforms = SumTree::new(());
            return;
        }
        self.state
            .excerpts
            .update_last(|entry| entry.adds_newline = false, ());
        self.rebuild_diff_transforms_from_excerpts();
    }

    /// 从输入 excerpts 树重建输出变换树。
    ///
    /// 输入树是唯一权威数据源；输出树只保存按同一顺序排列的显示变换。
    fn rebuild_diff_transforms_from_excerpts(&mut self) {
        let excerpts = self
            .state
            .excerpts
            .iter()
            .map(DiffTransform::from_excerpt)
            .collect::<Vec<_>>();
        self.state.diff_transforms = SumTree::from_iter(excerpts, ());
    }

    /// 从当前映射树派生组合坐标下的搜索匹配范围。
    ///
    /// excerpt 的 match_ranges 是权威数据；文档级列表按需派生，不另存可写副本。
    fn match_ranges_from_tree(&self) -> Vec<MultiBufferRange> {
        let mut ranges = Vec::new();
        let mut cursor = MultiBufferCursor::new(&self.state.excerpts, &self.state.diff_transforms);
        cursor.seek_output(ByteOffset::ZERO, Bias::Right);
        while let Some((excerpt, _)) = cursor.item() {
            let mapping = excerpt.to_mapping(cursor.start().clone());
            let output_start = mapping.output_range.start().get();
            let source_start = mapping.source_range.start().get();
            for matched in &mapping.match_ranges {
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
        self.state.excerpts = splice_excerpt_entries(&self.state.excerpts, &path_key, Vec::new());
        self.rebuild_diff_transforms_from_excerpts();
        self.fix_document_tail_newline();
        self.publish_projection_edit(&before, old_version);
        cx.emit(MultiBufferEvent::TextChanged);
        cx.notify();
        true
    }

    /// 注册尚未跟踪的源：文本/语法快照、变更订阅与捕获表；返回新增源的起始索引。
    fn register_sources(
        &mut self,
        new_sources: Vec<Entity<LanguageBuffer>>,
        cx: &mut Context<Self>,
    ) -> usize {
        if !new_sources.is_empty() {
            self.snapshot_epoch = self.snapshot_epoch.wrapping_add(1);
        }
        let first_new_source = self.state.sources.len();
        if new_sources.is_empty() {
            return first_new_source;
        }
        let new_source_subscriptions = new_sources
            .iter()
            .map(|source| SourceSubscription {
                source: source.clone(),
                text: source.update(cx, |source, cx| {
                    source.buffer().update(cx, |buffer, _| buffer.subscribe())
                }),
            })
            .collect::<Vec<_>>();
        let new_source_event_subscriptions = new_sources
            .iter()
            .map(|source| {
                let observed = source.clone();
                cx.subscribe(source, move |this, _, event, cx| match event {
                    LanguageBufferEvent::TextChanged => {
                        this.source_changed(observed.entity_id(), cx)
                    }
                    LanguageBufferEvent::Reparsed => this.source_reparsed(observed.entity_id(), cx),
                    LanguageBufferEvent::MetadataChanged => {
                        // 设置/语言变化：用新快照刷新该源的设置与词边界，再通知组合层消费者。
                        this.refresh_source_snapshot(observed.entity_id(), cx);
                        cx.emit(MultiBufferEvent::MetadataChanged);
                        cx.notify();
                    }
                })
            })
            .collect::<Vec<_>>();
        self.state.sources.extend(new_sources.iter().map(|source| {
            let snapshot = source.read(cx).snapshot(cx);
            ExcerptSource {
                entity: source.clone(),
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
        self.state
            .source_event_subscriptions
            .extend(new_source_event_subscriptions);
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
    fn excerpt_path(&self, excerpt: &ExcerptRange, cx: &App) -> PathKey {
        let source_index = self.state.source_indices[&excerpt.source.entity_id()];
        PathKey::new(
            self.state.sources[source_index]
                .entity
                .read(cx)
                .file_path()
                .unwrap_or_default(),
        )
    }

    /// 用给定 excerpts 替换其源路径现有的全部 excerpts（按路径有序插入）。
    ///
    /// 路径由片段源的 file_path 确定；同一调用内的片段必须属于同一路径。
    /// 新增源自动注册，其余路径的片段与其组合坐标保持不变。
    pub fn set_excerpts_for_path(
        &mut self,
        excerpts: Vec<ExcerptRange>,
        cx: &mut Context<Self>,
    ) -> bool {
        if excerpts.is_empty() {
            return false;
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
        self.register_sources(new_sources, cx);
        let path = self.excerpt_path(&excerpts[0], cx);
        debug_assert!(
            excerpts
                .iter()
                .all(|excerpt| self.excerpt_path(excerpt, cx) == path),
            "set_excerpts_for_path 的片段必须属于同一路径"
        );
        self.apply_excerpts_for_path(path, excerpts, cx);
        true
    }

    /// 用给定片段替换某路径的 excerpts，保持其余路径不变并维持路径升序。
    fn apply_excerpts_for_path(
        &mut self,
        path: PathKey,
        excerpts: Vec<ExcerptRange>,
        cx: &mut Context<Self>,
    ) {
        let before = self.projection_trees();
        let old_version = self.state.projection_version;
        // 该路径在树中的起始序号与原有条目数，决定新条目的全局位置与分隔换行标记。
        let (start_index, old_count) = {
            let mut start = self.state.excerpts.cursor::<ExcerptSummary>(());
            start.seek(&path, Bias::Left);
            let start_index = start.start().count;
            let mut end = self.state.excerpts.cursor::<ExcerptSummary>(());
            end.seek(&path, Bias::Right);
            (start_index, end.start().count.saturating_sub(start_index))
        };
        let total = self.state.excerpts.summary().count - old_count + excerpts.len();
        let entries = self.build_entries_for_excerpts(excerpts, start_index, total, cx);
        self.state.excerpts = splice_excerpt_entries(&self.state.excerpts, &path, entries);
        self.rebuild_diff_transforms_from_excerpts();
        self.fix_document_tail_newline();
        self.publish_projection_edit(&before, old_version);
        cx.emit(MultiBufferEvent::TextChanged);
        cx.notify();
    }

    fn source_changed(&mut self, source_id: gpui::EntityId, cx: &mut Context<Self>) {
        self.synchronize_source_change(source_id, DiffRefresh::RebuildProjection, None, cx);
    }

    /// 从 MultiBuffer 自己拥有的源订阅拉取下一段连续变化并推进投影。
    ///
    /// `LanguageBufferEvent` 只是唤醒信号；直接编辑、外部编辑和历史回放都经过此入口，
    /// 因而同一源只有一个增量游标，不会重放已消费的旧事件。
    fn synchronize_source_change(
        &mut self,
        source_id: gpui::EntityId,
        diff_refresh: DiffRefresh,
        expanded_excerpts: Option<&HashSet<usize>>,
        cx: &mut Context<Self>,
    ) -> Option<TextChangeBatch> {
        let source_change = self
            .state
            .source_subscriptions
            .iter()
            .find(|state| state.source.entity_id() == source_id)
            .map(|state| state.text.consume())?;
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
        // 外部整体刷新会重建 excerpt 拓扑；文本始终由当前 source 快照按需读取。
        if source_change.requires_reset() && self.is_diff_source(source_id, cx) {
            let before = self.projection_trees();
            self.refresh_source_snapshot(source_id, cx);
            self.rebuild_diff_projection_from(before, Some(&source_change), cx);
            return Some(source_change);
        }
        self.recompute_diff_for_source(source_id, diff_refresh, cx);
        // 普通编辑只更新受影响 source 的派生坐标；BufferDiff 仍独立维护 hunk 拓扑。
        self.apply_source_change(
            source_id,
            &position_map,
            &source_change,
            expanded_excerpts,
            cx,
        );
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
        let snapshot = source.read(cx).snapshot(cx);
        self.snapshot_epoch = self.snapshot_epoch.wrapping_add(1);
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
        self.state.capture_names = rebuild_capture_table(&mut self.state.sources);
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
        let snapshot = source.read(cx).snapshot(cx);

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
        self.state.capture_names = rebuild_capture_table(&mut self.state.sources);
        let old_mappings =
            mappings_for_source(&self.state.excerpts, &self.state.diff_transforms, source_id);
        // 绝对输出坐标由树摘要推导：源范围变化只 splice 受影响路径的 item，其余路径不变。
        self.splice_source_path(source_id, source_position_map, expanded_excerpts, cx);
        self.refresh_diff_display(cx);
        let incremental = self.source_incremental_change(source_id, source_change, &old_mappings);
        self.publish_projection_change(incremental);
        cx.emit(MultiBufferEvent::TextChanged);
        cx.notify();
    }

    fn source_reparsed(&mut self, source_id: gpui::EntityId, cx: &mut Context<Self>) {
        let Some(source) = self
            .state
            .sources
            .iter()
            .find(|state| state.entity.entity_id() == source_id)
            .map(|state| state.entity.clone())
        else {
            return;
        };
        let snapshot = source.read(cx).snapshot(cx);
        self.snapshot_epoch = self.snapshot_epoch.wrapping_add(1);
        self.state.metadata_epoch = self.state.metadata_epoch.wrapping_add(1);
        // 按源去重：只更新该源共享的一份 (text, syntax)，所有映射自动跟随。
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
        self.state.capture_names = rebuild_capture_table(&mut self.state.sources);
        cx.emit(MultiBufferEvent::Reparsed);
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.set_excerpts(Vec::new(), cx);
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

        let mut grouped: Vec<(Entity<LanguageBuffer>, Vec<Edit>)> = Vec::new();
        let mut edited_excerpts = HashSet::new();
        let push_source_edit =
            |mapping: &ExcerptMapping,
             source_range: TextRange,
             replacement: String,
             grouped: &mut Vec<(Entity<LanguageBuffer>, Vec<Edit>)>,
             edited_excerpts: &mut HashSet<usize>| {
                edited_excerpts.insert(mapping.excerpt_index);
                let source = source_entities[mapping.source_index].clone();
                let source_edit = Edit::replace(source_range, replacement);
                if let Some((_, source_edits)) = grouped
                    .iter_mut()
                    .find(|(candidate, _)| candidate.entity_id() == source.entity_id())
                {
                    source_edits.push(source_edit);
                } else {
                    grouped.push((source, vec![source_edit]));
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
            let start_index = start_mapping.excerpt_index;
            let end_index = if range.is_empty() {
                start_index
            } else {
                mapping_at_edit_end(
                    &self.state.excerpts,
                    &self.state.diff_transforms,
                    range.end(),
                )
                .map(|mapping| mapping.excerpt_index)
                .ok_or_else(|| TextError::InvariantViolation {
                    location: "MultiBuffer::edit",
                    detail: "编辑终点不在可见 excerpt 中".to_string(),
                })?
            };
            if end_index < start_index {
                return Err(TextError::InvariantViolation {
                    location: "MultiBuffer::edit",
                    detail: "编辑范围必须正序".to_string(),
                });
            }
            let mappings = mappings_between_indices(
                &self.state.excerpts,
                &self.state.diff_transforms,
                start_index,
                end_index,
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
            if start_index == end_index {
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

        let mut edited_source_ids = Vec::with_capacity(grouped.len());
        for (source, source_edits) in grouped {
            Self::update_source_text(
                &source,
                |buffer| buffer.edit(source_edits, metadata.clone()),
                cx,
            )?;
            edited_source_ids.push(source.entity_id());
        }

        // 组合编辑写回工作区源后，立即从 MultiBuffer 拥有的订阅拉取同一批变化。
        // 稍后到达的 LanguageBuffer 事件只负责唤醒，不再携带或重放增量。
        // hunk 变化由 BufferDiffEvent::DiffChanged 异步驱动物化；
        // 本轮回传的映射即编辑后、重物化前的坐标系，选区落位不依赖 diff 重建时机。
        for source_id in edited_source_ids {
            self.synchronize_source_change(
                source_id,
                DiffRefresh::PreserveProjection,
                Some(&edited_excerpts),
                cx,
            )
            .ok_or_else(|| TextError::InvariantViolation {
                location: "MultiBuffer::edit",
                detail: "源 Buffer 已提交编辑但 MultiBuffer 订阅未收到变化".to_string(),
            })?;
        }
        Ok(())
    }

    /// MultiBuffer 写入源 Buffer 的唯一入口。
    ///
    /// 文本内核只发布版本化变更；这里同步推进 LanguageBuffer，确保返回前文本与语法属于同一版本。
    /// 普通编辑与历史回放共同经过这一入口，不能各自维护跨层同步协议。
    fn update_source_text<T>(
        source: &Entity<LanguageBuffer>,
        update: impl FnOnce(&mut Buffer) -> TextResult<T>,
        cx: &mut Context<Self>,
    ) -> TextResult<T> {
        let source_buffer = source.read(cx).buffer();
        let result = source_buffer.update(cx, |buffer, cx| -> TextResult<T> {
            let version = buffer.version();
            let result = update(buffer)?;
            if buffer.version() != version {
                cx.notify();
            }
            Ok(result)
        })?;
        Ok(result)
    }

    /// 源文件路径变化后，按各源当前路径重建映射项（低频操作，允许整体重排）。
    fn rebuild_display(&mut self, cx: &mut Context<Self>) {
        let before = self.projection_trees();
        let old_version = self.state.projection_version;
        let mut path_keys = std::mem::take(&mut self.state.path_keys);
        let mut path_key_indices = std::mem::take(&mut self.state.path_key_indices);
        let mut entries = self.state.excerpts.iter().cloned().collect::<Vec<_>>();
        for entry in &mut entries {
            let path = PathKey::new(
                self.state.sources[entry.source_index]
                    .entity
                    .read(cx)
                    .file_path()
                    .unwrap_or_default(),
            );
            entry.path = path.clone();
            entry.display_path = path;
            entry.path_index = intern_path(&mut path_keys, &mut path_key_indices, &entry.path);
        }
        // 路径顺序可能变化；整体重排并按新位置重算分隔标记。
        entries.sort_by(|a, b| Ord::cmp(&a.path, &b.path));
        let total = entries.len();
        for (index, entry) in entries.iter_mut().enumerate() {
            let source = &self.state.sources[entry.source_index];
            if let Some((text_summary, ends_with_newline)) =
                snapshot_range_summary(&source.text, entry.source_range.range())
            {
                entry.text_summary = text_summary;
                entry.adds_newline = index + 1 < total && !ends_with_newline;
            }
        }
        self.state.path_keys = path_keys;
        self.state.path_key_indices = path_key_indices;
        self.state.excerpts = SumTree::from_iter(entries, ());
        self.rebuild_diff_transforms_from_excerpts();
        self.publish_projection_edit(&before, old_version);
        cx.emit(MultiBufferEvent::TextChanged);
        cx.notify();
    }

    pub fn start_transaction(&mut self, cx: &mut Context<Self>) -> TextResult<TransactionId> {
        if self.history_owner() == HistoryOwner::SourceBuffer {
            let source = self
                .singleton_source
                .as_ref()
                .expect("共享源历史必须有工作区源");
            return source
                .read(cx)
                .buffer()
                .update(cx, |buffer, _| buffer.start_transaction())?
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
            let buffer = source.read(cx).buffer();
            if active_source_transactions
                .iter()
                .any(|candidate| candidate.entity_id() == buffer.entity_id())
            {
                continue;
            }
            buffer
                .update(cx, |buffer, _| buffer.start_transaction())?
                .ok_or_else(|| TextError::InvariantViolation {
                    location: "MultiBuffer::start_transaction",
                    detail: "excerpt 底层 Buffer 已有活动事务".to_string(),
                })?;
            active_source_transactions.push(buffer);
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
                .read(cx)
                .buffer()
                .update(cx, |buffer, _| buffer.end_transaction())
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
        for buffer in active_source_transactions.drain(..) {
            if let Some(transaction_id) = buffer
                .update(cx, |buffer, _| buffer.end_transaction())
                .ok()
                .flatten()
            {
                transactions.push((buffer, transaction_id));
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
            let buffer = source.read(cx).buffer();
            return buffer.read(cx).current_history_transaction_id();
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
            let outcome = Self::update_source_text(
                &source,
                |buffer| if redo { buffer.redo() } else { buffer.undo() },
                cx,
            )?;
            let Some(outcome) = outcome else {
                return Ok(None);
            };
            let source_change = self
                .synchronize_source_change(
                    source.entity_id(),
                    DiffRefresh::PreserveProjection,
                    None,
                    cx,
                )
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
                if self.state.sources[excerpt.source_index]
                    .entity
                    .read(cx)
                    .buffer()
                    .entity_id()
                    == buffer.entity_id()
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
            let outcome = Self::update_source_text(
                &source,
                |buffer| if redo { buffer.redo() } else { buffer.undo() },
                cx,
            )?;
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
            self.synchronize_source_change(source_id, DiffRefresh::PreserveProjection, None, cx)
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

    /// 快照入口：epoch 未变时直接复用缓存，避免每次读取都展开整棵树。
    pub fn snapshot(&self, _cx: &App) -> MultiBufferSnapshot {
        let epoch = self.snapshot_epoch;
        if let Some((cached_epoch, snapshot)) = self.snapshot_cache.borrow().as_ref()
            && *cached_epoch == epoch
        {
            return snapshot.clone();
        }
        let snapshot = self.build_snapshot(_cx);
        *self.snapshot_cache.borrow_mut() = Some((epoch, snapshot.clone()));
        snapshot
    }

    fn build_snapshot(&self, _cx: &App) -> MultiBufferSnapshot {
        MultiBufferSnapshot {
            projection_version: self.state.projection_version,
            diff_transforms: self.state.diff_transforms.clone(),
            excerpts: self.state.excerpts.clone(),
            excerpts_cache: Arc::new(OnceLock::new()),
            path_keys: Arc::from(self.state.path_keys.clone()),
            excerpt_sources: Arc::from(
                self.state
                    .sources
                    .iter()
                    .map(|source| ExcerptSourceSnapshot {
                        text: source.text.clone(),
                        syntax: source.syntax.clone(),
                        highlight_cache: Arc::clone(&source.highlight_cache),
                        word_boundary: source.word_boundary,
                        settings: Arc::clone(&source.settings),
                        capture_map: Arc::clone(&source.capture_map),
                    })
                    .collect::<Vec<_>>(),
            ),
            capture_names: Arc::clone(&self.state.capture_names),
            metadata_version: self.state.metadata_epoch,
        }
    }

    /// 普通整文件文档的底层文本。
    ///
    /// 文档角色由构造时的 working source 决定，不随 diff 展开后的 excerpt 形状变化。
    pub fn as_singleton(&self, cx: &App) -> Option<Entity<Buffer>> {
        self.singleton_source
            .as_ref()
            .map(|source| source.read(cx).buffer())
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
        let mut cursor = MultiBufferCursor::new(&self.state.excerpts, &self.state.diff_transforms);
        cursor.seek_output(ByteOffset::ZERO, Bias::Right);
        while let Some((excerpt, _)) = cursor.item() {
            if excerpt.editable
                && self.state.sources[excerpt.source_index]
                    .entity
                    .read(cx)
                    .buffer()
                    .read(cx)
                    .is_dirty()
            {
                return true;
            }
            cursor.next();
        }
        false
    }

    /// 文档实际引用的、可落盘的底层文件 Buffer。
    ///
    /// 收集可编辑 excerpts 的源 Buffer 并按实体去重；无路径源（内存草稿）不参与。
    /// 显示文本物化 Buffer 永远不会出现在结果中。
    pub fn file_buffers(&self, cx: &App) -> Vec<(Entity<Buffer>, PathBuf)> {
        let mut buffers = Vec::<(Entity<Buffer>, PathBuf)>::new();
        let mut cursor = MultiBufferCursor::new(&self.state.excerpts, &self.state.diff_transforms);
        cursor.seek_output(ByteOffset::ZERO, Bias::Right);
        while let Some((entry, _)) = cursor.item() {
            if !entry.editable {
                cursor.next();
                continue;
            }
            let source = self.state.sources[entry.source_index].entity.read(cx);
            let Some(path) = source.file_path() else {
                continue;
            };
            let buffer = source.buffer();
            if !buffers
                .iter()
                .any(|(existing, _)| existing.entity_id() == buffer.entity_id())
            {
                buffers.push((buffer, path.to_path_buf()));
            }
            cursor.next();
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
        // 无工作区源时退回第一个可编辑片段（ProjectDiffView 等组合视图）。
        self.singleton_source
            .as_ref()
            .and_then(|source| source.read(cx).file_path())
            .or_else(|| {
                let mut cursor =
                    MultiBufferCursor::new(&self.state.excerpts, &self.state.diff_transforms);
                cursor.seek_output(ByteOffset::ZERO, Bias::Right);
                let mut path = None;
                while let Some((entry, _)) = cursor.item() {
                    if entry.editable {
                        path = self.state.sources[entry.source_index]
                            .entity
                            .read(cx)
                            .file_path();
                        if path.is_some() {
                            break;
                        }
                    }
                    cursor.next();
                }
                path
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
            offset.into(),
            affinity,
        )
        .unwrap_or_else(|| MultiBufferAnchor::boundary(offset))
    }

    /// 在当前 excerpts 中解析稳定位置；同一文件仍存在时优先落到最接近的源片段。
    pub fn resolve_anchor(&self, anchor: &MultiBufferAnchor) -> Option<MultiBufferOffset> {
        resolve_anchor_in_mappings(
            &self.state.excerpts,
            &self.state.diff_transforms,
            &self.state.path_keys,
            self.state.sources.as_slice(),
            anchor,
        )
        .map(Into::into)
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
                == MultiBufferOffset::new(self.state.diff_transforms.summary().output.text.len);
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

    /// 当前源折叠投影到组合坐标后的锚点范围。
    ///
    /// 源折叠端点先按当前源快照推进为源偏移，再投影到组合坐标并锚定为 MultiBufferAnchor；
    /// 消费方无需跨版本补偿。
    /// 一个源可能被展开的 diff hunk 切成多个 excerpt：
    /// 只要这些 excerpt 在源内连续覆盖，折叠范围就跨它们投影到组合坐标（中间夹入的旧侧 excerpt 也落在折叠范围内）；
    /// 跨过未展示内容或文件边界的折叠仍被丢弃。
    pub fn fold_ranges(&self, cx: &App) -> Arc<[Range<MultiBufferAnchor>]> {
        let snapshot = self.snapshot(cx);
        let mut projected: Vec<Range<MultiBufferAnchor>> = Vec::new();
        for (source_index, source) in self.state.sources.iter().enumerate() {
            let (source_text, source_folds) = {
                let snapshot = source.entity.read(cx).snapshot(cx);
                let folds = snapshot
                    .syntax
                    .fold_ranges(0..snapshot.text.len_bytes().get(), &snapshot.text);
                (snapshot.text, folds)
            };
            if source_folds.is_empty() {
                continue;
            }
            for fold in source_folds.iter() {
                let (Some(start), Some(end)) = (
                    fold.range.start.resolve_in(&source_text),
                    fold.range.end.resolve_in(&source_text),
                ) else {
                    continue;
                };
                if start >= end {
                    continue;
                }
                let path = PathKey::new(source.entity.read(cx).file_path().unwrap_or_default());
                let Some((start_mapping, end_mapping)) = source_mapping_range(
                    &self.state.excerpts,
                    &self.state.diff_transforms,
                    &path,
                    source_index,
                    start.get(),
                    end.get(),
                ) else {
                    continue;
                };
                let output_start = start_mapping.output_range.start().get() + start.get()
                    - start_mapping.source_range.start().get();
                let output_end = end_mapping.output_range.start().get() + end.get()
                    - end_mapping.source_range.start().get();
                if output_start < output_end {
                    projected.push(
                        snapshot.anchor_at(ByteOffset::new(output_start), Affinity::Before)
                            ..snapshot.anchor_at(ByteOffset::new(output_end), Affinity::After),
                    );
                }
            }
        }
        projected.sort_unstable_by_key(|range| {
            (
                snapshot
                    .resolve_anchor(&range.start)
                    .map_or(0, |offset| offset.get()),
                snapshot
                    .resolve_anchor(&range.end)
                    .map_or(0, |offset| offset.get()),
            )
        });
        projected.dedup();
        Arc::from(projected)
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

/// 定位输出范围终点严格覆盖目标偏移的片段（O(log n)）。
///
/// 语义等价于在片段数组上做 `partition_point(end <= offset)`：
/// 目标偏移落在合成换行或文档末尾时命中最后一个片段。
fn mapping_covering_output_end(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    offset: usize,
) -> Option<(ExcerptMapping, MappingPosition)> {
    let mut cursor = MultiBufferCursor::new(excerpts, tree);
    cursor.seek_output(ByteOffset::new(offset), Bias::Right);
    if cursor.item().is_some() {
        let at = cursor.start().clone();
        return cursor.mapping().map(|mapping| (mapping, at));
    }
    cursor.seek_output(ByteOffset::new(tree.summary().output.text.len), Bias::Left);
    let at = cursor.start().clone();
    cursor
        .item()
        .and_then(|_| cursor.mapping().map(|mapping| (mapping, at)))
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
    let mut cursor = MultiBufferCursor::new(excerpts, tree);
    cursor.seek_output(offset, Bias::Right);
    if cursor.item().is_none() {
        // 偏移在文档末尾（或之后）：命中最后一个映射。
        let mut last = MultiBufferCursor::new(excerpts, tree);
        // Right bias 保留末尾零长度 excerpt；它们仍然拥有自己的文件身份和边界行。
        last.seek_output(ByteOffset::new(tree.summary().output.text.len), Bias::Right);
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
    last.seek_output(ByteOffset::new(tree.summary().output.text.len), Bias::Right);
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
    cursor.seek_output_line(tree.summary().output.text.lines, Bias::Left);
    let at = cursor.start().clone();
    cursor
        .item()
        .and_then(|_| cursor.mapping().map(|mapping| (mapping, at)))
}

/// 在权威映射树中寻找同一源的连续 excerpt 覆盖范围。
///
/// 这是折叠投影的查询入口：只保留起止两个映射，不把整棵树展平成数组。
fn source_mapping_range(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    path: &PathKey,
    source_index: usize,
    source_start: usize,
    source_end: usize,
) -> Option<(ExcerptMapping, ExcerptMapping)> {
    let mut input_cursor = excerpts.cursor::<ExcerptSummary>(());
    input_cursor.seek(path, Bias::Left);
    let mut input_previous_end = None;
    let mut input_visible = false;
    while let Some(entry) = input_cursor.item() {
        if entry.path != *path {
            break;
        }
        if entry.source_index == source_index {
            if input_previous_end.is_none()
                && entry.source_range.start().get() <= source_start
                && source_start < entry.source_range.end().get()
            {
                input_previous_end = Some(entry.source_range.end().get());
                input_visible = source_end <= entry.source_range.end().get();
            } else if let Some(previous_end) = input_previous_end {
                if previous_end != entry.source_range.start().get() {
                    break;
                }
                input_previous_end = Some(entry.source_range.end().get());
                input_visible = source_end <= entry.source_range.end().get();
            }
            if input_visible {
                break;
            }
        }
        input_cursor.next();
    }
    if !input_visible {
        return None;
    }

    let mut cursor = MultiBufferCursor::new(excerpts, tree);
    cursor.seek_output(ByteOffset::ZERO, Bias::Right);
    let mut start_mapping = None;
    let mut previous_end = None;
    while let Some((excerpt, _)) = cursor.item() {
        if excerpt.source_index == source_index {
            let mapping = cursor.mapping().expect("双坐标游标必须有对应映射");
            if start_mapping.is_none() {
                if mapping.source_range.start().get() <= source_start
                    && source_start < mapping.source_range.end().get()
                {
                    if source_end <= mapping.source_range.end().get() {
                        return Some((mapping.clone(), mapping));
                    }
                    previous_end = Some(mapping.source_range.end());
                    start_mapping = Some(mapping);
                }
            } else {
                if previous_end != Some(mapping.source_range.start()) {
                    return None;
                }
                if mapping.source_range.start().get() < source_end
                    && source_end <= mapping.source_range.end().get()
                {
                    return Some((
                        start_mapping.take().expect("起始 excerpt 必须存在"),
                        mapping,
                    ));
                }
                previous_end = Some(mapping.source_range.end());
            }
        }
        cursor.next();
    }
    None
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

/// 在给定投影→源映射中把投影偏移锚定到源坐标。
///
/// 供 [`MultiBuffer::anchor_at`] 与 [`MultiBufferSnapshot::anchor_at`] 共用。
fn anchor_in_mappings(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    offset: ByteOffset,
    affinity: Affinity,
) -> Option<MultiBufferAnchor> {
    let (mapping, at) = mapping_at_tree(excerpts, tree, offset)?;
    let source_offset = ByteOffset::new(
        (mapping.source_range.start().get() + offset.get().saturating_sub(at.bytes))
            .min(mapping.source_range.end().get()),
    );
    let text_anchor =
        Anchor::new(mapping.source_range.version(), source_offset).with_affinity(affinity);
    Some(MultiBufferAnchor::excerpt(
        mapping.path_index,
        mapping.source_id,
        text_anchor,
    ))
}

/// 把锚点绑定的源 Anchor 按当前源文本快照推进到源坐标。
///
/// 找不到绑定源、或锚点版本已被编辑日志裁剪时返回 None，由调用方按最近路径回退。
fn excerpt_anchor_source_offset<S: SourceTexts + ?Sized>(
    excerpts: &SumTree<Excerpt>,
    path_keys: &[PathKey],
    sources: &S,
    anchor: &ExcerptAnchor,
) -> Option<ByteOffset> {
    let path_key = path_keys.get(anchor.path.get() as usize)?;
    let mut cursor = excerpts.cursor::<ExcerptSummary>(());
    cursor.seek(path_key, Bias::Left);
    while let Some(excerpt) = cursor.item() {
        if &excerpt.path != path_key {
            break;
        }
        if excerpt.source_id == anchor.source_id {
            let text = sources.source_text(excerpt.source_index)?;
            return anchor.text_anchor.resolve_in(text);
        }
        cursor.next();
    }
    None
}

/// 锚点绑定路径退出投影后，按当前路径顺序解析到最近的后继/前驱片段。
fn nearest_path_output_offset(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    path_keys: &[PathKey],
    path: PathKeyIndex,
) -> Option<ByteOffset> {
    let anchor_key = path_keys.get(path.get() as usize)?;
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
        return Some(ByteOffset::new(output_start));
    }
    if let Some(path_key) = preceding
        && let Some((output_end, _)) = last_mapping_for_path(excerpts, tree, path_key)
    {
        return Some(ByteOffset::new(output_end));
    }
    None
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
/// 同一文件仍存在时优先落到最接近的源片段；文件退出投影时按当前路径顺序落到最近的后继/前驱。
/// [`MultiBuffer::resolve_anchor`]（当前映射）与 [`MultiBufferSnapshot::resolve_anchor`]（快照映射）共用此解析逻辑。
fn resolve_anchor_in_mappings<S: SourceTexts + ?Sized>(
    excerpts: &SumTree<Excerpt>,
    tree: &SumTree<DiffTransform>,
    path_keys: &[PathKey],
    sources: &S,
    anchor: &MultiBufferAnchor,
) -> Option<ByteOffset> {
    let excerpt_anchor = match anchor {
        MultiBufferAnchor::Min => return Some(ByteOffset::ZERO),
        MultiBufferAnchor::Max => {
            return Some(ByteOffset::new(tree.summary().output.text.len));
        }
        MultiBufferAnchor::Excerpt(excerpt_anchor) => excerpt_anchor,
    };
    if let Some(source_offset) =
        excerpt_anchor_source_offset(excerpts, path_keys, sources, excerpt_anchor)
    {
        if let Some(offset) = nearest_output_offset_for_source(
            excerpts,
            tree,
            path_keys,
            excerpt_anchor.path,
            excerpt_anchor.source_id,
            source_offset,
        ) {
            return Some(offset);
        }
        if let Some(offset) = nearest_output_offset_for_source(
            excerpts,
            tree,
            path_keys,
            excerpt_anchor.path,
            None,
            source_offset,
        ) {
            return Some(offset);
        }
    }
    nearest_path_output_offset(excerpts, tree, path_keys, excerpt_anchor.path)
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
