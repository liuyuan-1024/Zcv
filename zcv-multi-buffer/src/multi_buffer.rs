//! Editor 与具体文本 Buffer 之间的组合文档边界。
//!
//! 组合文档按调用方给出的顺序组织多个来源的 excerpts，并保留组合坐标到源文件坐标的映射。
//! 普通编辑器是「整文件单 excerpt」的组合文档；多文件差异视图在此重排显示 excerpts。
//! Editor 始终只消费本层，不感知来源数量。
//! diff 显示拓扑（git hunks、展开状态、跟踪区间与显示坐标）只服务需要重排 excerpts 的组合文档，见 [`diff_projection`]。

mod buffer_diff;
mod diff_projection;
mod path_key;
mod word_diff;

pub use buffer_diff::{
    BufferDiff, BufferDiffEvent, BufferDiffInput, BufferDiffSnapshot, DiffHunk, DiffHunkStaging,
    DiffOperations, DiffRefresh, PendingHunk, PendingSense,
};
pub use diff_projection::{DiffFile, DiffHunkSource, DisplayHunk};
pub use path_key::{PathKey, PathKeyIndex};

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use gpui::{App, Context, Entity, EventEmitter, Subscription};
use sum_tree::{Bias, ContextLessSummary, Dimension, Item, SeekTarget, SumTree};
use unicode_segmentation::UnicodeSegmentation;
use zcv_language::{
    AutoClosePair, BracketPair, FoldRange, HighlightSpan, LanguageBuffer, LanguageBufferEvent,
    LocalBinding, NewlineIndent, OutlineItem, OutlineTextRange, SyntaxNode, SyntaxSnapshot,
};
use zcv_text::{
    Affinity, Buffer, BufferConfig, BufferVersion, ByteOffset, CharOffset, CoordinateError, Edit,
    Line, LineEndingStyle, LogicalColumn, MovementDirection, MovementUnit, Position, PositionMap,
    Snapshot, Stickiness, StorageError, TextChangeBatch, TextError, TextRange, TextRead,
    TextResult, TextSubscription, TransactionId, TransactionMetadata, Utf16Offset, Utf16Position,
};

/// 组合文档中的一个源片段。
#[derive(Clone)]
pub struct MultiBufferExcerpt {
    source: Entity<LanguageBuffer>,
    source_range: TextRange,
    match_ranges: Vec<TextRange>,
    display_path: Option<PathKey>,
    editable: bool,
    starts_new_excerpt: bool,
    diff_kind: Option<ExcerptDiffKind>,
    /// 工作区侧顺序键（对应源文件中的起始行）；删除块来自 base 文本，无法用 source_range 排序。
    order_line: Option<usize>,
}

/// 组合投影片段在统一 diff 中承担的文本侧别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExcerptDiffKind {
    Added,
    Deleted,
}

impl MultiBufferExcerpt {
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
            order_line: None,
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

    /// 显式指定工作区侧顺序键（删除块用其工作区锚点行，而不是 base 文本行）。
    pub fn with_order_line(mut self, order_line: usize) -> Self {
        self.order_line = Some(order_line);
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
/// 主位置绑定到底层文件与源字节；文件退出投影时按原有文件顺序解析到最近的后继，
/// 没有后继时再回到前驱。该语义用于在 excerpts 结构刷新后保持阅读位置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultiBufferAnchor {
    path: PathKeyIndex,
    source_id: gpui::EntityId,
    source_offset: ByteOffset,
    following_paths: Vec<PathKeyIndex>,
    preceding_paths: Vec<PathKeyIndex>,
}

impl MultiBufferAnchor {
    /// 源自身变更（外部编辑、共享 Buffer 的其他 Editor 编辑）后推进源偏移。
    ///
    /// 仅当锚点绑定该源时生效；按 `affinity` 决定同点插入的吸附方向。
    /// 投影重建不经过这里——重建不改变源，源锚点直接按重建后快照解析即可。
    pub fn map_through_source_change(
        &mut self,
        source_id: gpui::EntityId,
        position_map: &PositionMap,
        affinity: Affinity,
    ) {
        if self.source_id == source_id {
            self.source_offset = position_map
                .map_old_position_with_affinity(self.source_offset, affinity)
                .value();
        }
    }
}

/// 一次组合投影重建造成的坐标重映射。
///
/// 把「重建前」的投影坐标经源忠实映射到重建后的当前投影坐标：
/// 未触发重建时投影坐标连续，映射为恒等；
/// 重建（reload 重裁剪）后同一源位置的投影偏移可能改变，必须经源解析——重建前的光标不能把裸偏移直接当作重建后投影坐标。
/// 编辑落位与 diff 展开/折叠共用同一映射：结构刷新不改变源，光标同样经源保持逻辑位置。
#[derive(Clone, Debug)]
pub struct ProjectionRemap {
    /// 重建前的投影→源映射快照；`None` 表示恒等（本次未重建投影）。
    before: Option<SumTree<ExcerptMapping>>,
}

impl ProjectionRemap {
    /// 恒等重映射：投影坐标连续，无需经源解析。
    pub fn identity() -> Self {
        Self { before: None }
    }

    /// 一次真实重建：`before` 是重建前的投影→源映射快照。
    pub(crate) fn rebuilt(before: SumTree<ExcerptMapping>) -> Self {
        Self {
            before: Some(before),
        }
    }

    /// 是否为恒等映射（本次未触发投影重建）。
    pub fn is_identity(&self) -> bool {
        self.before.is_none()
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
    capture_map: Arc<[u32]>,
}

/// 不可变快照帧中的源状态（不携带实体引用）。
#[derive(Clone, Debug)]
struct ExcerptSourceSnapshot {
    text: Snapshot,
    syntax: SyntaxSnapshot,
    capture_map: Arc<[u32]>,
}

#[derive(Clone, Debug)]
struct ExcerptMapping {
    excerpt_index: usize,
    path: PathKey,
    /// 路径身份索引；锚点解析用它做整数比较。
    path_index: PathKeyIndex,
    display_path: PathKey,
    output_range: TextRange,
    source_range: TextRange,
    output_start_line: usize,
    output_end_line: usize,
    source_start_line: usize,
    /// 工作区侧顺序键；同一路径内必须升序。
    order_line: usize,
    /// 指向源表（`ExcerptState::sources` / 快照的 `excerpt_sources`）的索引。
    source_index: usize,
    source_id: gpui::EntityId,
    editable: bool,
    starts_new_excerpt: bool,
    diff_kind: Option<ExcerptDiffKind>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ExcerptMappingSummary {
    bytes: usize,
    lines: usize,
    count: usize,
    /// 子树内按路径升序的最后一个路径；按路径游标 seek 依赖它。
    path_key: PathKey,
    /// 子树内同一路径下最后一个工作区侧顺序键。
    max_order_line: usize,
}

impl ContextLessSummary for ExcerptMappingSummary {
    fn zero() -> Self {
        Self::default()
    }

    fn add_summary(&mut self, summary: &Self) {
        debug_assert!(
            summary.path_key >= self.path_key,
            "excerpt 必须按路径升序排列：{:?} 之后出现了 {:?}",
            self.path_key,
            summary.path_key,
        );
        debug_assert!(
            summary.path_key > self.path_key
                || (summary.path_key == self.path_key
                    && summary.max_order_line >= self.max_order_line),
            "同一路径内 excerpt 必须按工作区源偏移升序",
        );
        self.bytes += summary.bytes;
        self.lines += summary.lines;
        self.count += summary.count;
        self.path_key = summary.path_key.clone();
        self.max_order_line = summary.max_order_line;
    }
}

impl Item for ExcerptMapping {
    type Summary = ExcerptMappingSummary;

    fn summary(&self, _cx: ()) -> Self::Summary {
        Self::Summary {
            bytes: self.output_range.len(),
            lines: self.output_end_line.saturating_sub(self.output_start_line),
            count: 1,
            path_key: self.path.clone(),
            max_order_line: self.order_line,
        }
    }
}

impl Dimension<'_, ExcerptMappingSummary> for PathKey {
    fn zero(_: ()) -> Self {
        Self::min()
    }

    fn add_summary(&mut self, summary: &ExcerptMappingSummary, _: ()) {
        *self = summary.path_key.clone();
    }
}

impl SeekTarget<'_, ExcerptMappingSummary, ExcerptMappingSummary> for PathKey {
    fn cmp(&self, cursor_location: &ExcerptMappingSummary, _: ()) -> Ordering {
        Ord::cmp(self, &cursor_location.path_key)
    }
}

/// 组合输出字节偏移维度：在路径有序树上按累积输出字节 seek。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct OutputOffset(usize);

impl Dimension<'_, ExcerptMappingSummary> for OutputOffset {
    fn zero(_: ()) -> Self {
        Self(0)
    }

    fn add_summary(&mut self, summary: &ExcerptMappingSummary, _: ()) {
        self.0 += summary.bytes;
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

fn mapping_count(mappings: &SumTree<ExcerptMapping>) -> usize {
    mappings.summary().count
}

fn mapping_vec(mappings: &SumTree<ExcerptMapping>) -> Vec<ExcerptMapping> {
    mappings.iter().cloned().collect()
}

/// 读取一个源范围的长度和换行摘要，不构造组合字符串。
fn snapshot_range_is_valid(text: &Snapshot, range: TextRange) -> bool {
    let len = text.len_bytes();
    range.start() <= range.end()
        && range.end() <= len
        && (range.start() == len || text.chunk_at_byte(range.start()).is_ok())
        && (range.end() == len || text.chunk_at_byte(range.end()).is_ok())
}

fn snapshot_range_summary(text: &Snapshot, range: TextRange) -> Option<(usize, bool)> {
    if !snapshot_range_is_valid(text, range) {
        return None;
    }
    // 空片段没有源内容可显示，但必须在组合文档中占一个空行；
    // 否则删除点占位行会被相邻行吸收，折叠后的删除块不可见。
    if range.start() == range.end() {
        return Some((0, false));
    }
    let start_line = text.byte_to_line(range.start()).ok()?.get();
    let end_line = text.byte_to_line(range.end()).ok()?.get();
    let ends_with_newline = range
        .end()
        .get()
        .checked_sub(1)
        .and_then(|offset| text.chunk_at_byte(ByteOffset::new(offset)).ok())
        .is_some_and(|(chunk, chunk_start)| {
            chunk.as_bytes()[offset_index(ByteOffset::new(range.end().get() - 1), chunk_start)]
                == b'\n'
        });
    Some((end_line.saturating_sub(start_line), ends_with_newline))
}

fn offset_index(offset: ByteOffset, chunk_start: ByteOffset) -> usize {
    offset.get().saturating_sub(chunk_start.get())
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
    shift: usize,
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
    output_range: TextRange,
    source_range: TextRange,
    output_start_line: usize,
    output_end_line: usize,
    source_start_line: usize,
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

    pub fn output_range(&self) -> TextRange {
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
    /// 仅无 excerpt 的独立占位文本使用；真实组合文档始终为 `None`。
    plain_text: Option<Snapshot>,
    plain_syntax: Option<SyntaxSnapshot>,
    config: BufferConfig,
    projection_version: BufferVersion,
    excerpts: Arc<[ExcerptSnapshot]>,
    excerpt_mappings: Arc<[ExcerptMapping]>,
    /// 按路径升序的映射树；excerpts_for_path 用路径游标查询。
    excerpt_tree: SumTree<ExcerptMapping>,
    /// 路径索引表：PathKeyIndex 对应的路径，供锚点解析按路径 seek。
    path_keys: Arc<[PathKey]>,
    /// 按源去重的 (text, syntax, capture_map) 表（映射经 `source_index` 引用）。
    excerpt_sources: Arc<[ExcerptSourceSnapshot]>,
    capture_names: Arc<[Arc<str>]>,
}

/// 虚拟组合文本的一段连续借用。
///
/// `text` 永远直接借用某个源 Buffer，或借用 excerpt 间的静态换行边界；
/// 它不来自任何组合文本物化。
/// `output_range` 保留该段在组合坐标中的位置，使后续 DisplayMap 游标无需回退到整份物化快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiBufferChunk<'a> {
    pub text: &'a str,
    pub output_range: Range<ByteOffset>,
}

/// 在 excerpt 映射与源快照之间向前推进的组合文本游标。
///
/// 输入范围必须位于组合快照内；游标只向前移动，跨 excerpt 时不会拷贝或拼接文本。
pub struct MultiBufferChunks<'a> {
    snapshot: &'a MultiBufferSnapshot,
    range: Range<ByteOffset>,
    mapping_index: usize,
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
    pub fn config(&self) -> &BufferConfig {
        &self.config
    }

    pub fn syntax_version(&self) -> BufferVersion {
        self.projection_version
    }

    /// 返回当前组合文档的完整 UTF-8 内容，供预览等只读消费者使用。
    pub fn text_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.len_bytes().get());
        for chunk in self.text_chunks(ByteOffset::ZERO..self.len_bytes()) {
            bytes.extend_from_slice(chunk.text.as_bytes());
        }
        bytes
    }

    /// 组合文本总字节数。
    /// 这个值由最后一个 excerpt 的派生输出范围决定，不读取或复制组合文本。
    pub fn len_bytes(&self) -> ByteOffset {
        self.excerpt_mappings.last().map_or_else(
            || {
                self.plain_text
                    .as_ref()
                    .map_or(ByteOffset::ZERO, Snapshot::len_bytes)
            },
            |mapping| mapping.output_range.end(),
        )
    }

    /// 虚拟组合文本的逻辑行数。
    ///
    /// excerpt 映射在建立时已经累计了输出行边界，因此这里不扫描、更不拼接所有源文本。
    pub fn line_count(&self) -> usize {
        self.excerpts.last().map_or_else(
            || self.plain_text.as_ref().map_or(1, Snapshot::line_count),
            |excerpt| excerpt.output_end_line + 1,
        )
    }

    /// 把组合偏移转换为逻辑行。
    ///
    /// 该查询只遍历覆盖请求范围的源 chunk；
    /// 它是 DisplayMap 迁出物化 `Snapshot` 后的基础坐标入口。
    pub fn byte_to_line(&self, offset: ByteOffset) -> TextResult<Line> {
        self.ensure_output_boundary(offset)?;
        if self.excerpt_mappings.is_empty() {
            return self
                .plain_text
                .as_ref()
                .map_or(Ok(Line::ZERO), |text| text.byte_to_line(offset));
        }
        let index = self
            .excerpt_mappings
            .partition_point(|mapping| mapping.output_range.end() <= offset);
        let mapping = self
            .excerpt_mappings
            .get(index)
            .or_else(|| self.excerpt_mappings.last())
            .ok_or(CoordinateError::OutOfBounds(offset))?;
        let content_end =
            ByteOffset::new(mapping.output_range.start().get() + mapping.source_range.len());
        let source = self
            .excerpt_sources
            .get(mapping.source_index)
            .ok_or(CoordinateError::OutOfBounds(offset))?;
        // 片段末尾（含为分隔补出的合成换行）按源范围末端定位：
        // 区间终点必须落在最后一条内容行上，不能提前跳到下一片段。
        let source_offset = if offset >= content_end {
            mapping.source_range.end()
        } else {
            ByteOffset::new(
                mapping.source_range.start().get()
                    + offset
                        .get()
                        .saturating_sub(mapping.output_range.start().get()),
            )
        };
        let source_line = source.text.byte_to_line(source_offset)?.get();
        Ok(Line::new(
            mapping.output_start_line + source_line.saturating_sub(mapping.source_start_line),
        ))
    }

    /// 返回组合逻辑行的起始字节偏移。
    pub fn line_start_byte(&self, target: Line) -> TextResult<ByteOffset> {
        if target.get() >= self.line_count() {
            return Err(CoordinateError::LineOutOfBounds(target).into());
        }
        if target == Line::ZERO {
            return Ok(ByteOffset::ZERO);
        }

        if self.excerpt_mappings.is_empty() {
            return self.plain_text.as_ref().map_or(
                Err(CoordinateError::LineOutOfBounds(target).into()),
                |text| text.line_start_byte(target),
            );
        }
        let index = self
            .excerpt_mappings
            .partition_point(|mapping| mapping.output_end_line <= target.get());
        let mapping = self
            .excerpt_mappings
            .get(index)
            // 最后一个 excerpt 的 `output_end_line` 同时是末行的定位边界。
            // 它没有后继 excerpt 可供二分游标落入，仍应解析回该源的末行。
            .or_else(|| {
                self.excerpt_mappings
                    .last()
                    .filter(|mapping| mapping.output_end_line == target.get())
            })
            .ok_or(CoordinateError::LineOutOfBounds(target))?;
        let source = self
            .excerpt_sources
            .get(mapping.source_index)
            .ok_or(CoordinateError::LineOutOfBounds(target))?;
        let source_line = mapping.source_start_line + target.get() - mapping.output_start_line;
        let source_start = source.text.line_start_byte(Line::new(source_line))?;
        let relative = source_start
            .get()
            .saturating_sub(mapping.source_range.start().get());
        Ok(ByteOffset::new(
            mapping.output_range.start().get() + relative,
        ))
    }

    /// 把组合字节偏移转换为按 Unicode scalar value 计数的逻辑位置。
    pub fn byte_to_position(&self, offset: ByteOffset) -> TextResult<Position> {
        let line = self.byte_to_line(offset)?;
        let line_start = self.line_start_byte(line)?;
        let column = self
            .text_chunks(line_start..offset)
            .map(|chunk| chunk.text.chars().count())
            .sum();
        Ok(Position::new(line, LogicalColumn::new(column)))
    }

    /// 返回组合文本中的 Tree-sitter 风格字节坐标。
    pub fn byte_to_point(&self, offset: ByteOffset) -> TextResult<(Line, usize)> {
        let line = self.byte_to_line(offset)?;
        let line_start = self.line_start_byte(line)?;
        Ok((line, offset.get() - line_start.get()))
    }

    /// 读取指定组合范围；结果只在调用方需要跨源拼接时短暂存在。
    pub fn text_for_range(&self, range: TextRange) -> TextResult<String> {
        if self.excerpt_mappings.is_empty() {
            return self
                .plain_text
                .as_ref()
                .ok_or_else(|| TextError::from(CoordinateError::OutOfBounds(range.end())))?
                .slice_text(range)
                .map(|text| text.as_str().to_owned());
        }
        Ok(self
            .text_chunks(range.start()..range.end())
            .map(|chunk| chunk.text)
            .collect())
    }

    /// 返回**包含**组合偏移的文本块及其组合坐标起点。
    ///
    /// 注意与 `text_chunks(offset..)` 的区别：后者从偏移处切开块，本方法返回偏移所在的完整块。
    pub fn chunk_at_byte(&self, offset: ByteOffset) -> TextResult<(&str, ByteOffset)> {
        self.ensure_output_boundary(offset)?;
        if self.excerpt_mappings.is_empty() {
            let text = self
                .plain_text
                .as_ref()
                .ok_or_else(|| TextError::from(CoordinateError::OutOfBounds(offset)))?;
            return text.chunk_at_byte(offset);
        }
        let mapping_index = self
            .excerpt_mappings
            .partition_point(|mapping| mapping.output_range.end() <= offset);
        if let Some(mapping) = self.excerpt_mappings.get(mapping_index) {
            let content_start = mapping.output_range.start();
            let content_end = ByteOffset::new(content_start.get() + mapping.source_range.len());
            if content_start <= offset && offset < content_end {
                let source = self
                    .excerpt_sources
                    .get(mapping.source_index)
                    .ok_or(CoordinateError::OutOfBounds(offset))?;
                let source_offset = ByteOffset::new(
                    mapping.source_range.start().get() + offset.get() - content_start.get(),
                );
                let (chunk, source_chunk_start) = source.text.chunk_at_byte(source_offset)?;
                // 源 chunk 可能起始于片段之前（或越过片段末尾）：裁剪到片段范围内，
                // 组合文本只投影片段内容。
                let chunk_end = ByteOffset::new(source_chunk_start.get() + chunk.len());
                let slice_start = source_chunk_start.max(mapping.source_range.start());
                let slice_end = chunk_end.min(mapping.source_range.end());
                let text = &chunk[slice_start.get() - source_chunk_start.get()
                    ..slice_end.get() - source_chunk_start.get()];
                let output_chunk_start = ByteOffset::new(
                    content_start.get() + (slice_start.get() - mapping.source_range.start().get()),
                );
                return Ok((text, output_chunk_start));
            }
        }
        // 片段之间的静态换行块或文档末尾：退回按偏移切分的块。
        self.text_chunks(offset..self.len_bytes())
            .next()
            .map(|chunk| (chunk.text, chunk.output_range.start))
            .ok_or(CoordinateError::OutOfBounds(offset).into())
    }

    /// 把组合逻辑位置转换为字节偏移。
    pub fn position_to_byte(&self, position: Position) -> TextResult<ByteOffset> {
        let line_start = self.line_start_byte(position.line())?;
        let line_end = if position.line().get() + 1 < self.line_count() {
            self.line_start_byte(Line::new(position.line().get() + 1))?
        } else {
            self.len_bytes()
        };
        let mut column = 0usize;
        for chunk in self.text_chunks(line_start..line_end) {
            for (offset, character) in chunk.text.char_indices() {
                if column == position.column().get() {
                    return Ok(ByteOffset::new(chunk.output_range.start.get() + offset));
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
            Err(CoordinateError::OutOfBounds(line_end).into())
        }
    }

    pub fn byte_to_char(&self, offset: ByteOffset) -> TextResult<CharOffset> {
        self.ensure_output_boundary(offset)?;
        Ok(CharOffset::new(
            self.text_chunks(ByteOffset::ZERO..offset)
                .map(|chunk| chunk.text.chars().count())
                .sum(),
        ))
    }

    pub fn char_to_byte(&self, target: CharOffset) -> TextResult<ByteOffset> {
        let mut chars = 0usize;
        for chunk in self.text_chunks(ByteOffset::ZERO..self.len_bytes()) {
            for (offset, _) in chunk.text.char_indices() {
                if chars == target.get() {
                    return Ok(ByteOffset::new(chunk.output_range.start.get() + offset));
                }
                chars += 1;
            }
        }
        (chars == target.get())
            .then_some(self.len_bytes())
            .ok_or(CoordinateError::CharOutOfBounds(target).into())
    }

    pub fn movement_boundary(
        &self,
        offset: CharOffset,
        direction: MovementDirection,
        unit: MovementUnit,
    ) -> TextResult<CharOffset> {
        // 与单 Buffer 共用同一份文本移动语义，组合文档不得另实现一套边界规则。
        zcv_text::movement_boundary_in_text(
            self,
            self.config.word_boundary,
            offset,
            direction,
            unit,
        )
    }

    /// 返回包含当前位置的词边界。组合文本沿连续 chunk 读取，不构造临时字符串。
    pub fn surrounding_word(&self, offset: CharOffset) -> TextResult<(CharOffset, CharOffset)> {
        let offset = self.char_to_byte(offset)?;
        let is_word = |byte: ByteOffset| {
            self.char_at_byte(byte).is_some_and(|character| {
                self.config.word_boundary.is_identifier_continue(character)
            })
        };
        let mut start = offset;
        while start > ByteOffset::ZERO {
            let previous = self.previous_grapheme_boundary(start)?;
            if !is_word(previous) {
                break;
            }
            start = previous;
        }
        let mut end = offset;
        while end < self.len_bytes() && is_word(end) {
            end = self.next_grapheme_boundary(end)?;
        }
        Ok((self.byte_to_char(start)?, self.byte_to_char(end)?))
    }

    pub fn is_inside_word(&self, offset: CharOffset) -> TextResult<bool> {
        let offset = self.char_to_byte(offset)?;
        Ok(self
            .char_at_byte(offset)
            .is_some_and(|character| self.config.word_boundary.is_identifier_continue(character)))
    }

    pub fn byte_to_utf16_cu(&self, offset: ByteOffset) -> TextResult<Utf16Offset> {
        self.ensure_output_boundary(offset)?;
        Ok(Utf16Offset::new(
            self.text_chunks(ByteOffset::ZERO..offset)
                .flat_map(|chunk| chunk.text.chars())
                .map(char::len_utf16)
                .sum(),
        ))
    }

    pub fn utf16_cu_to_byte(&self, target: Utf16Offset) -> TextResult<ByteOffset> {
        let mut units = 0usize;
        for chunk in self.text_chunks(ByteOffset::ZERO..self.len_bytes()) {
            for (offset, character) in chunk.text.char_indices() {
                if units == target.get() {
                    return Ok(ByteOffset::new(chunk.output_range.start.get() + offset));
                }
                units += character.len_utf16();
                if units > target.get() {
                    break;
                }
            }
        }
        (units == target.get()).then_some(self.len_bytes()).ok_or(
            CoordinateError::Utf16PositionOutOfBounds(zcv_text::Utf16Position::new(
                Line::ZERO,
                target,
            ))
            .into(),
        )
    }

    /// 从组合偏移范围连续借用文本块。
    ///
    /// 虚拟 MultiBuffer 文本入口：
    /// 消费者按输出坐标请求范围，游标通过 excerpt 映射定位源快照，再直接返回 Rope chunk 的子切片。
    /// 片段间为保持行边界注入的换行也以静态借用块返回。
    pub fn text_chunks(&self, range: Range<ByteOffset>) -> MultiBufferChunks<'_> {
        let end = range.end.min(self.len_bytes());
        let start = range.start.min(end);
        let mapping_index = self
            .excerpt_mappings
            .partition_point(|mapping| mapping.output_range.end() <= start);
        MultiBufferChunks {
            snapshot: self,
            range: start..end,
            mapping_index,
            offset: start,
        }
    }

    fn ensure_output_boundary(&self, offset: ByteOffset) -> TextResult<()> {
        if offset > self.len_bytes() {
            return Err(CoordinateError::OutOfBounds(offset).into());
        }
        if offset == ByteOffset::ZERO || offset == self.len_bytes() {
            return Ok(());
        }

        if self.excerpt_mappings.is_empty() {
            return self
                .plain_text
                .as_ref()
                .ok_or(CoordinateError::OutOfBounds(offset))?
                .chunk_at_byte(offset)
                .map(|_| ())
                .map_err(|_| CoordinateError::InvalidByteBoundary(offset).into());
        }

        let mapping_index = self
            .excerpt_mappings
            .partition_point(|mapping| mapping.output_range.end() <= offset);
        let mapping = self
            .excerpt_mappings
            .get(mapping_index)
            .ok_or(CoordinateError::OutOfBounds(offset))?;
        let source_output_end =
            ByteOffset::new(mapping.output_range.start().get() + mapping.source_range.len());
        if offset > source_output_end {
            return Ok(());
        }
        if offset == source_output_end && source_output_end < mapping.output_range.end() {
            return Ok(());
        }

        let source = self
            .excerpt_sources
            .get(mapping.source_index)
            .ok_or(CoordinateError::OutOfBounds(offset))?;
        let source_offset = ByteOffset::new(
            mapping.source_range.start().get() + offset.get() - mapping.output_range.start().get(),
        );
        source
            .text
            .chunk_at_byte(source_offset)
            .map(|_| ())
            .map_err(|_| CoordinateError::InvalidByteBoundary(offset).into())
    }

    pub fn version(&self) -> BufferVersion {
        self.projection_version
    }

    pub fn excerpts(&self) -> &[ExcerptSnapshot] {
        &self.excerpts
    }

    /// 指定源路径的 excerpts（按组合顺序）。
    ///
    /// 用按路径升序的映射树做路径游标定位，不扫描全部 excerpts。
    pub fn excerpts_for_path<'a>(
        &'a self,
        path: &Path,
    ) -> impl Iterator<Item = &'a ExcerptSnapshot> + 'a {
        let path_key = PathKey::new(path);
        let mut cursor = self.excerpt_tree.cursor::<ExcerptMappingSummary>(());
        cursor.seek(&path_key, Bias::Left);
        cursor
            .take_while(move |mapping| mapping.path == path_key)
            .filter_map(|mapping| self.excerpts.get(mapping.excerpt_index))
    }

    /// 组合输出偏移所在的 excerpt（用累积输出字节的偏移游标在路径有序树上定位）。
    pub fn excerpt_at_output_offset(&self, offset: ByteOffset) -> Option<&ExcerptSnapshot> {
        let mut cursor = self.excerpt_tree.cursor::<OutputOffset>(());
        cursor.seek(&OutputOffset(offset.get()), Bias::Right);
        let mapping = cursor.item()?;
        self.excerpts.get(mapping.excerpt_index)
    }

    /// 把快照内的组合偏移锚定到底层源坐标（Editor 源锚点选区：投影→源）。
    pub fn anchor_for_offset(&self, offset: ByteOffset) -> Option<MultiBufferAnchor> {
        anchor_in_mappings(&self.excerpt_tree, offset)
    }

    /// 把源锚点解析回快照内的组合偏移（Editor 源锚点选区：源→投影）。
    ///
    /// 源锚点选区按需解析：投影重建不改变源，选区无需重映射，用重建后快照直接解析即得当前投影偏移。
    pub fn resolve_anchor(&self, anchor: &MultiBufferAnchor) -> Option<ByteOffset> {
        resolve_anchor_in_mappings(&self.excerpt_tree, &self.path_keys, anchor)
    }

    pub fn capture_names(&self) -> Arc<[Arc<str>]> {
        Arc::clone(&self.capture_names)
    }

    /// 查询组合坐标中的语法高亮，并把每个源 Buffer 的 capture index 映射到本快照的统一表。
    ///
    /// 无 excerpt 的纯文本帧（placeholder 等）退回快照自身语法表。
    pub fn highlights(&self, range: std::ops::Range<usize>) -> Vec<HighlightSpan> {
        if self.excerpt_mappings.is_empty() {
            return match (&self.plain_syntax, &self.plain_text) {
                (Some(syntax), Some(text)) => syntax.highlights(range, text),
                _ => Vec::new(),
            };
        }
        let mut spans = Vec::new();
        // mappings 按组合输出顺序建立，且每个 mapping 的内容结束位置单调递增；
        // 先定位第一个可能重叠的片段，避免每个视口范围都扫描整份多文件结果。
        let first = self.excerpt_mappings.partition_point(|excerpt| {
            excerpt.output_range.start().get() + excerpt.source_range.len() <= range.start
        });
        for excerpt in self.excerpt_mappings[first..].iter() {
            let output_start = excerpt.output_range.start().get();
            if output_start >= range.end {
                break;
            }
            // 非末尾 excerpt 可能为显示边界补一个换行；该字节不属于 source，
            // 不能越过 source_range 去查询下一段源文本的语法。
            let output_end = output_start + excerpt.source_range.len();
            let start = range.start.max(output_start);
            let end = range.end.min(output_end);
            if start >= end {
                continue;
            }
            let source = &self.excerpt_sources[excerpt.source_index];
            let source_start = excerpt.source_range.start().get() + start - output_start;
            let source_end = excerpt.source_range.start().get() + end - output_start;
            spans.extend(
                source
                    .syntax
                    .highlights(source_start..source_end, &source.text)
                    .into_iter()
                    .filter_map(|span| {
                        let capture = *source.capture_map.get(span.capture as usize)?;
                        Some(HighlightSpan {
                            range: (output_start + span.range.start
                                - excerpt.source_range.start().get())
                                ..(output_start + span.range.end
                                    - excerpt.source_range.start().get()),
                            capture,
                        })
                    }),
            );
        }
        spans
    }

    /// 查询组合坐标中光标所在 source 的括号对，并映射回组合坐标。
    pub fn bracket_pairs_at(&self, offset: ByteOffset) -> Vec<BracketPair> {
        let Some((mapping, source, source_offset)) = self.source_point(offset) else {
            let start = offset.get().saturating_sub(1);
            let end = offset.get().saturating_add(1).min(self.len_bytes().get());
            return match (&self.plain_syntax, &self.plain_text) {
                (Some(syntax), Some(text)) => syntax.bracket_pairs(start..end, text),
                _ => Vec::new(),
            };
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
    pub fn suggested_newline_indent(&self, offset: ByteOffset) -> TextResult<NewlineIndent> {
        let Some((_, source, source_offset)) = self.source_point(offset) else {
            return match (&self.plain_syntax, &self.plain_text) {
                (Some(syntax), Some(text)) => syntax.suggested_newline_indent(offset, text),
                _ => Ok(NewlineIndent {
                    base_indent: String::new(),
                    additional_levels: 0,
                }),
            };
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
    pub fn node_at(&self, offset: ByteOffset) -> Option<SyntaxNode> {
        let Some((mapping, source, source_offset)) = self.source_point(offset) else {
            return match (&self.plain_syntax, &self.plain_text) {
                (Some(syntax), Some(text)) => syntax.node_at(offset.get(), text),
                _ => None,
            };
        };
        let node = source.syntax.node_at(source_offset.get(), &source.text)?;
        project_syntax_node(&node, mapping.source_range, mapping.output_range)
    }

    /// 返回组合坐标中选区所在语法层的节点链，顺序为最小节点到语法根节点。
    pub fn node_ancestors(&self, range: std::ops::Range<usize>) -> Vec<SyntaxNode> {
        let Some((mapping, source, source_range)) = self.source_range(range.clone()) else {
            return match (&self.plain_syntax, &self.plain_text) {
                (Some(syntax), Some(text)) => syntax.node_ancestors(range, text),
                _ => Vec::new(),
            };
        };
        source
            .syntax
            .node_ancestors(source_range, &source.text)
            .into_iter()
            .filter_map(|node| {
                project_syntax_node(&node, mapping.source_range, mapping.output_range)
            })
            .collect()
    }

    /// 将选区扩展到当前语法层中严格包围它的下一个节点。
    pub fn expand_selection_range(
        &self,
        range: std::ops::Range<usize>,
    ) -> Option<std::ops::Range<usize>> {
        let Some((mapping, source, source_range)) = self.source_range(range.clone()) else {
            return match (&self.plain_syntax, &self.plain_text) {
                (Some(syntax), Some(text)) => syntax.expand_selection_range(range, text),
                _ => None,
            };
        };
        let ancestor = source
            .syntax
            .expand_selection_range(source_range, &source.text)?;
        project_range(ancestor, mapping.source_range, mapping.output_range)
    }

    /// 返回当前组合文档中可见源范围内的文件大纲项。
    ///
    /// 大纲先从每个源的 `SyntaxSnapshot` 计算，再只投影完整落在 excerpt 内的定义；
    /// 这样不会把跨未展示内容的语法节点误投影到差异或搜索组合文档中。
    pub fn outline_items(&self) -> Vec<OutlineItem> {
        if self.excerpt_mappings.is_empty() {
            return match (&self.plain_syntax, &self.plain_text) {
                (Some(syntax), Some(text)) => syntax.outline(0..self.len_bytes().get(), text),
                _ => Vec::new(),
            };
        }

        let mut projected = Vec::new();
        for (source_index, source) in self.excerpt_sources.iter().enumerate() {
            let outlines = source
                .syntax
                .outline(0..source.text.len_bytes().get(), &source.text);
            for mapping in self
                .excerpt_mappings
                .iter()
                .filter(|mapping| mapping.source_index == source_index)
            {
                for item in outlines.iter().filter_map(|item| {
                    project_outline_item(item, mapping.source_range, mapping.output_range)
                }) {
                    projected.push(item);
                }
            }
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
        let Some(mapping) = (self.excerpt_mappings.len() == 1).then(|| &self.excerpt_mappings[0])
        else {
            return Vec::new();
        };
        let Some(source) = self.excerpt_sources.get(mapping.source_index) else {
            return Vec::new();
        };
        let source_len = source.text.len_bytes().get();
        let is_full_file = mapping.source_range.start().get() == 0
            && mapping.source_range.end().get() == source_len
            && mapping.output_range.start().get() == 0
            && mapping.output_range.end().get() == source_len;
        if !is_full_file {
            return Vec::new();
        }
        source.syntax.local_bindings(0..source_len, &source.text)
    }

    fn source_point(
        &self,
        offset: ByteOffset,
    ) -> Option<(&ExcerptMapping, &ExcerptSourceSnapshot, ByteOffset)> {
        let mapping = mapping_at_tree(&self.excerpt_tree, offset)?;
        let source = self.excerpt_sources.get(mapping.source_index)?;
        let delta = offset
            .get()
            .saturating_sub(mapping.output_range.start().get())
            .min(mapping.source_range.len());
        Some((
            mapping,
            source,
            ByteOffset::new(mapping.source_range.start().get() + delta),
        ))
    }

    fn source_range(
        &self,
        range: std::ops::Range<usize>,
    ) -> Option<(
        &ExcerptMapping,
        &ExcerptSourceSnapshot,
        std::ops::Range<usize>,
    )> {
        let mapping = mapping_at_tree(&self.excerpt_tree, ByteOffset::new(range.start))?;
        let output_start = mapping.output_range.start().get();
        let content_end = output_start + mapping.source_range.len();
        if range.start < output_start || range.end > content_end {
            return None;
        }
        let source = self.excerpt_sources.get(mapping.source_index)?;
        let source_start = mapping.source_range.start().get() + range.start - output_start;
        let source_end = mapping.source_range.start().get() + range.end - output_start;
        Some((mapping, source, source_start..source_end))
    }

    pub fn excerpt_for_output_line(&self, line: usize) -> Option<&ExcerptSnapshot> {
        let index = self
            .excerpts
            .partition_point(|excerpt| excerpt.output_end_line <= line);
        self.excerpts.get(index).filter(|excerpt| {
            (excerpt.output_start_line <= line && line < excerpt.output_end_line)
                || (excerpt.output_start_line == excerpt.output_end_line
                    && line == excerpt.output_start_line)
        })
    }
}

/// `MultiBufferSnapshot` 是连续文本视图，而不是由临时 `Buffer` 复制出的影子文本。
///
/// 对外实现 `zcv_text::TextRead` 后，搜索等跨文本算法直接消费 excerpt 游标；
/// 文本所有权仍然只存在于各源 Buffer 中。
impl TextRead for MultiBufferSnapshot {
    fn slice_text(&self, range: TextRange) -> TextResult<Cow<'_, str>> {
        Ok(Cow::Owned(self.text_for_range(range)?))
    }

    fn chunks(&self, range: TextRange) -> TextResult<impl Iterator<Item = &str> + '_> {
        self.ensure_output_boundary(range.start())?;
        self.ensure_output_boundary(range.end())?;
        Ok(self
            .text_chunks(range.start()..range.end())
            .map(|chunk| chunk.text))
    }

    fn len_bytes(&self) -> ByteOffset {
        self.len_bytes()
    }

    fn len_chars(&self) -> CharOffset {
        self.byte_to_char(self.len_bytes())
            .expect("组合文本末端必须是有效字符边界")
    }

    fn line_count(&self) -> usize {
        self.line_count()
    }

    fn line_start(&self, line: Line) -> TextResult<ByteOffset> {
        self.line_start_byte(line)
    }

    fn byte_to_position(&self, offset: ByteOffset) -> TextResult<Position> {
        self.byte_to_position(offset)
    }

    fn byte_to_line(&self, offset: ByteOffset) -> TextResult<Line> {
        self.byte_to_line(offset)
    }

    fn position_to_byte(&self, position: Position) -> TextResult<ByteOffset> {
        self.position_to_byte(position)
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
            .and_then(|offset| self.char_at_byte(offset))
    }

    fn char_at_byte(&self, offset: ByteOffset) -> Option<char> {
        (offset < self.len_bytes())
            .then(|| self.chunk_at_byte(offset).ok())
            .flatten()
            .and_then(|(chunk, start)| chunk[offset.get() - start.get()..].chars().next())
    }

    fn byte_to_char(&self, offset: ByteOffset) -> TextResult<CharOffset> {
        self.byte_to_char(offset)
    }

    fn char_to_byte(&self, offset: CharOffset) -> TextResult<ByteOffset> {
        self.char_to_byte(offset)
    }

    fn byte_to_utf16_position(&self, offset: ByteOffset) -> TextResult<Utf16Position> {
        let line = self.byte_to_line(offset)?;
        let line_start = self.line_start_byte(line)?;
        let character = self
            .text_chunks(line_start..offset)
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
        for chunk in self.text_chunks(line_start..line_end) {
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
            .then_some(line_end)
            .ok_or(CoordinateError::Utf16PositionOutOfBounds(position).into())
    }

    fn byte_to_utf16_cu(&self, offset: ByteOffset) -> TextResult<Utf16Offset> {
        self.byte_to_utf16_cu(offset)
    }

    fn utf16_cu_to_byte(&self, offset: Utf16Offset) -> TextResult<ByteOffset> {
        self.utf16_cu_to_byte(offset)
    }

    fn is_grapheme_boundary(&self, offset: ByteOffset) -> TextResult<bool> {
        self.ensure_output_boundary(offset)?;
        if offset == ByteOffset::ZERO || offset == self.len_bytes() {
            return Ok(true);
        }
        Ok(self
            .text_chunks(ByteOffset::ZERO..self.len_bytes())
            .flat_map(|chunk| {
                chunk
                    .text
                    .grapheme_indices(true)
                    .map(move |(index, _)| ByteOffset::new(chunk.output_range.start.get() + index))
            })
            .any(|boundary| boundary == offset))
    }

    fn previous_grapheme_boundary(&self, offset: ByteOffset) -> TextResult<ByteOffset> {
        self.ensure_output_boundary(offset)?;
        Ok(self
            .text_chunks(ByteOffset::ZERO..offset)
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
        self.ensure_output_boundary(offset)?;
        let mut saw_current = false;
        for chunk in self.text_chunks(offset..self.len_bytes()) {
            for (index, _) in chunk.text.grapheme_indices(true) {
                let boundary = ByteOffset::new(chunk.output_range.start.get() + index);
                if boundary > offset || saw_current {
                    return Ok(boundary);
                }
                saw_current = true;
            }
        }
        Ok(self.len_bytes())
    }

    fn line_ending_style(&self) -> LineEndingStyle {
        let mut saw_lf = false;
        let mut saw_crlf = false;
        let mut saw_lone_cr = false;
        let mut previous_was_cr = false;
        for chunk in self.text_chunks(ByteOffset::ZERO..self.len_bytes()) {
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

impl<'a> Iterator for MultiBufferChunks<'a> {
    type Item = MultiBufferChunk<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.snapshot.excerpt_mappings.is_empty() {
            if self.offset >= self.range.end {
                return None;
            }
            let text = self.snapshot.plain_text.as_ref()?;
            let (chunk, chunk_start) = text.chunk_at_byte(self.offset).ok()?;
            let start = self.offset.get().checked_sub(chunk_start.get())?;
            let end = (self.range.end.get() - chunk_start.get()).min(chunk.len());
            if start >= end || !chunk.is_char_boundary(start) || !chunk.is_char_boundary(end) {
                return None;
            }
            let output_start = self.offset;
            self.offset = ByteOffset::new(chunk_start.get() + end);
            return Some(MultiBufferChunk {
                text: &chunk[start..end],
                output_range: output_start..self.offset,
            });
        }

        while self.offset < self.range.end {
            let mapping = self.snapshot.excerpt_mappings.get(self.mapping_index)?;
            if self.offset >= mapping.output_range.end() {
                self.mapping_index += 1;
                continue;
            }

            let output_start = mapping.output_range.start();
            let source_output_end =
                ByteOffset::new(output_start.get() + mapping.source_range.len());

            if self.offset >= source_output_end {
                // 非末尾 excerpt 为保持组合行边界而补出的换行不属于任何源文本。
                let end = self.range.end.min(mapping.output_range.end());
                if end <= self.offset {
                    self.mapping_index += 1;
                    continue;
                }
                let start = self.offset;
                self.offset = end;
                return Some(MultiBufferChunk {
                    text: "\n",
                    output_range: start..end,
                });
            }

            let source = self.snapshot.excerpt_sources.get(mapping.source_index)?;
            let source_offset = ByteOffset::new(
                mapping.source_range.start().get() + self.offset.get() - output_start.get(),
            );
            let (chunk, chunk_start) = source.text.chunk_at_byte(source_offset).ok()?;
            let start = source_offset.get() - chunk_start.get();
            let source_end = mapping.source_range.end().get();
            let end = (source_end - chunk_start.get())
                .min(chunk.len())
                .min(start + (self.range.end.get() - self.offset.get()));
            if start >= end || !chunk.is_char_boundary(start) || !chunk.is_char_boundary(end) {
                return None;
            }
            let output_start = self.offset;
            self.offset = ByteOffset::new(self.offset.get() + end - start);
            return Some(MultiBufferChunk {
                text: &chunk[start..end],
                output_range: output_start..self.offset,
            });
        }
        None
    }
}

fn project_outline_item(
    item: &OutlineItem,
    source_range: TextRange,
    output_range: TextRange,
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
    output_range: TextRange,
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
    output_range: TextRange,
) -> Option<std::ops::Range<usize>> {
    let source_start = source_range.start().get();
    (source_start <= range.start && range.end <= source_range.end().get()).then(|| {
        let output_start = output_range.start().get();
        (output_start + range.start - source_start)..(output_start + range.end - source_start)
    })
}

/// 纯文本帧：无 excerpt 的独立文本（placeholder 等），语法为空表。
impl From<Snapshot> for MultiBufferSnapshot {
    fn from(text: Snapshot) -> Self {
        let capture_names = SyntaxSnapshot::empty(text.version()).capture_names();
        Self {
            plain_syntax: Some(SyntaxSnapshot::empty(text.version())),
            config: text.config().clone(),
            projection_version: text.version(),
            excerpts: Arc::from([]),
            excerpt_mappings: Arc::from([]),
            excerpt_tree: SumTree::new(()),
            path_keys: Arc::from([]),
            excerpt_sources: Arc::from([]),
            capture_names,
            plain_text: Some(text),
        }
    }
}

struct ExcerptState {
    excerpts: Vec<MultiBufferExcerpt>,
    source_subscriptions: Vec<SourceSubscription>,
    source_event_subscriptions: Vec<Subscription>,
    mappings: SumTree<ExcerptMapping>,
    /// 按源去重的 (text, syntax, capture_map) 表。
    sources: Vec<ExcerptSource>,
    /// 源实体到 `sources` 索引的派生索引，供增量追加按身份查找源状态。
    source_indices: HashMap<gpui::EntityId, usize>,
    match_ranges: Vec<TextRange>,
    /// 路径身份表：索引一经分配不再变化，锚点用索引做紧凑表示。
    path_keys: Vec<PathKey>,
    path_key_indices: HashMap<PathKey, PathKeyIndex>,
    capture_names: Arc<[Arc<str>]>,
    projection_version: BufferVersion,
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
    working_source: Option<Entity<LanguageBuffer>>,
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
    /// 对每个 BufferDiff 的订阅：diff 结果或 pending 变化时重新物化显示。
    diff_subscriptions: Vec<Subscription>,
    /// 上次物化时各文件的 BufferDiff 身份与版本；实体替换或版本推进都视为缓存过期。
    diff_display_revisions: Vec<(gpui::EntityId, u64)>,
    /// 替换 base 后，新 BufferDiff 的首次后台结果返回前暂存的展开状态迁移来源。
    diff_pending_expansion_migrations: Vec<Option<diff_projection::PendingExpansionMigration>>,
    /// 外部源变更（共享 Buffer 的其他 Editor、直接编辑源）留下的源 PositionMap。
    ///
    /// 源锚点选区是单一数据源：只有源自身变更才需要推进源锚点，投影重建不经过这里。
    /// 本编辑器自己发起的编辑在 [`MultiBuffer::edit`] 内已经物化到显示流；
    /// 后续源事件只消费到相同版本，不会再次进入此队列。
    pending_source_remaps: Vec<(gpui::EntityId, PositionMap)>,
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
        if self.working_source.is_some() {
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
    pub fn from_working_source(source: Entity<LanguageBuffer>, cx: &mut Context<Self>) -> Self {
        let line_count = source.read(cx).text_snapshot(cx).line_count();
        let mut multi_buffer = Self::empty(cx);
        multi_buffer.working_source = Some(source.clone());
        multi_buffer.set_excerpts(
            vec![
                MultiBufferExcerpt::line_range(source.clone(), 0..line_count, cx)
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
            working_source: None,
            title: None,
            diffs: Vec::new(),
            diff: None,
            diff_expanded_by_default: false,
            diff_materialized_files: 0,
            diff_subscriptions: Vec::new(),
            diff_display_revisions: Vec::new(),
            diff_pending_expansion_migrations: Vec::new(),
            pending_source_remaps: Vec::new(),
        }
    }

    /// 空组合状态只保存 source 与其投影映射；组合文本不拥有第二份 Buffer。
    fn empty_excerpt_state(_cx: &mut Context<Self>) -> ExcerptState {
        ExcerptState {
            excerpts: Vec::new(),
            source_subscriptions: Vec::new(),
            source_event_subscriptions: Vec::new(),
            mappings: SumTree::new(()),
            sources: Vec::new(),
            source_indices: HashMap::new(),
            match_ranges: Vec::new(),
            path_keys: Vec::new(),
            path_key_indices: HashMap::new(),
            capture_names: Arc::from([]),
            projection_version: BufferVersion::INITIAL,
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
        let old_version = self.state.projection_version;
        let new_version = old_version
            .next()
            .expect("组合投影版本不应溢出；溢出时必须创建新文档生命周期");
        self.state.projection_version = new_version;
        let batch = incremental.map(|incremental| {
            incremental
                .batch
                .shifted_by(incremental.shift)
                .rebased_to(old_version, new_version)
        });
        self.state
            .projection_changes
            .publish(old_version, new_version, batch);
    }

    /// 把单次源编辑换算到组合坐标；跨多个 excerpt 或整体重载时返回 None（走整体重载）。
    fn source_incremental_change(
        &self,
        source_id: gpui::EntityId,
        source_change: Option<&TextChangeBatch>,
    ) -> Option<SourceIncremental> {
        let source_change = source_change?;
        if source_change.requires_reset() || source_change.patch().is_empty() {
            return None;
        }
        let mut excerpts = self
            .state
            .excerpts
            .iter()
            .filter(|excerpt| excerpt.source.entity_id() == source_id);
        let excerpt = excerpts.next()?;
        if excerpts.next().is_some() {
            return None;
        }
        let mapping = self.state.mappings.iter().find(|mapping| {
            self.state
                .excerpts
                .get(mapping.excerpt_index)
                .is_some_and(|candidate| candidate.source.entity_id() == source_id)
        })?;
        let shift = mapping
            .output_range
            .start()
            .get()
            .checked_sub(excerpt.source_range.start().get())?;
        Some(SourceIncremental {
            batch: source_change.clone(),
            shift,
        })
    }

    /// 以给定顺序重建组合文档。每个片段都保留源文件路径和源坐标映射。
    pub fn set_excerpts(&mut self, excerpts: Vec<MultiBufferExcerpt>, cx: &mut Context<Self>) {
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
                        cx.emit(MultiBufferEvent::MetadataChanged);
                        cx.notify();
                    }
                })
            })
            .collect::<Vec<_>>();
        let ExcerptState {
            excerpts: stored_excerpts,
            source_subscriptions,
            source_event_subscriptions,
            mappings,
            sources,
            source_indices,
            match_ranges,
            path_keys,
            path_key_indices,
            capture_names: composite_capture_names,
            ..
        } = &mut self.state;

        // 按源去重构建 (text, syntax) 表：同一文件的大量片段共享一份源状态。
        let mut next_sources: Vec<ExcerptSource> = Vec::new();
        let mut next_source_indices = HashMap::new();
        struct PreparedExcerpt {
            excerpt: MultiBufferExcerpt,
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
            let path = PathKey::new(
                source
                    .file_path()
                    .map_or_else(PathBuf::new, Path::to_path_buf),
            );
            let source_id = excerpt.source.entity_id();
            let source_index = match next_source_indices.get(&source_id).copied() {
                Some(index) => index,
                None => {
                    next_sources.push(ExcerptSource {
                        entity: excerpt.source.clone(),
                        text: source.text_snapshot(cx),
                        syntax: source.syntax_snapshot(),
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
        let mut output_offset = 0usize;
        let mut output_line = 0usize;
        let mut next_mappings = Vec::with_capacity(prepared_count);
        let mut next_match_ranges = Vec::new();
        let mut valid_excerpts = Vec::with_capacity(prepared_count);
        for (position, item) in prepared.into_iter().enumerate() {
            let display_path = item
                .excerpt
                .display_path
                .clone()
                .unwrap_or_else(|| item.path.clone());
            let output_start = ByteOffset::new(output_offset);
            let output_start_line = output_line;
            let Some((line_count, ends_with_newline)) = snapshot_range_summary(
                &next_sources[item.source_index].text,
                item.excerpt.source_range,
            ) else {
                continue;
            };
            output_offset += item.excerpt.source_range.len();
            output_line += line_count;
            if position + 1 < prepared_count && !ends_with_newline {
                output_offset += 1;
                output_line += 1;
            }
            let output_end = ByteOffset::new(output_offset);
            let output_end_line = output_line;
            let output_range =
                TextRange::new(output_start, output_end).expect("组合片段输出范围必须正序");
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
            let excerpt_index = valid_excerpts.len();
            let path_index = intern_path(path_keys, path_key_indices, &item.path);
            next_mappings.push(ExcerptMapping {
                excerpt_index,
                path: item.path,
                path_index,
                display_path,
                output_range,
                source_range: item.excerpt.source_range,
                output_start_line,
                output_end_line,
                source_start_line: item.start_line,
                order_line: item.excerpt.order_line.unwrap_or(item.start_line),
                source_index: item.source_index,
                source_id: item.source_id,
                editable: item.excerpt.editable,
                starts_new_excerpt: item.excerpt.starts_new_excerpt,
                diff_kind: item.excerpt.diff_kind,
            });
            valid_excerpts.push(item.excerpt);
        }

        *source_subscriptions = next_source_subscriptions;
        *source_event_subscriptions = next_source_event_subscriptions;
        *stored_excerpts = valid_excerpts;
        *mappings = SumTree::from_iter(next_mappings, ());
        *sources = next_sources;
        *source_indices = sources
            .iter()
            .enumerate()
            .map(|(index, source)| (source.entity.entity_id(), index))
            .collect();
        *match_ranges = next_match_ranges;
        *composite_capture_names = rebuild_capture_table(sources);
        self.publish_projection_change(None);
        cx.emit(MultiBufferEvent::TextChanged);
        cx.notify();
    }

    /// 在现有组合文档末尾追加有序片段。
    ///
    /// 追加是组合文档的增量写入边界：只物化新增片段，并通过显示文本物化 Buffer 的尾部编辑提交，不重建已有映射、源订阅或整份显示文本。
    /// 需要替换顺序或删除片段时仍应使用 [`Self::set_excerpts`]。
    pub fn append_excerpts(
        &mut self,
        excerpts: Vec<MultiBufferExcerpt>,
        cx: &mut Context<Self>,
    ) -> Vec<TextRange> {
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
        self.register_sources(new_sources, cx);

        struct PreparedExcerpt {
            excerpt: MultiBufferExcerpt,
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
            let path = PathKey::new(
                source
                    .entity
                    .read(cx)
                    .file_path()
                    .map_or_else(PathBuf::new, Path::to_path_buf),
            );
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

        let mut existing_output_len = self
            .state
            .mappings
            .last()
            .map_or(0, |mapping| mapping.output_range.end().get());
        let mut output_line = self
            .state
            .mappings
            .last()
            .map_or(0, |mapping| mapping.output_end_line);
        let output_ends_with_newline = self.state.mappings.last().is_some_and(|mapping| {
            mapping.output_range.len() > mapping.source_range.len()
                || self.state.sources[mapping.source_index]
                    .text
                    .slice_byte_range(
                        ByteOffset::new(mapping.source_range.end().get().saturating_sub(1)),
                        mapping.source_range.end(),
                    )
                    .is_ok_and(|text| text.as_str() == "\n")
        });
        if existing_output_len > 0 && !output_ends_with_newline {
            self.state.mappings.update_last(
                |mapping| {
                    mapping.output_range = TextRange::new(
                        mapping.output_range.start(),
                        ByteOffset::new(mapping.output_range.end().get() + 1),
                    )
                    .expect("追加 excerpt 的行边界必须有效");
                    mapping.output_end_line += 1;
                },
                (),
            );
            existing_output_len += 1;
            output_line += 1;
        }
        let mut next_path_keys = std::mem::take(&mut self.state.path_keys);
        let mut next_path_key_indices = std::mem::take(&mut self.state.path_key_indices);
        let prepared_count = prepared.len();
        let existing_excerpt_count = self.state.excerpts.len();
        let mut next_mappings = Vec::with_capacity(prepared_count);
        let mut next_match_ranges = Vec::new();
        let mut valid_excerpts = Vec::with_capacity(prepared_count);
        for (position, item) in prepared.into_iter().enumerate() {
            let display_path = item
                .excerpt
                .display_path
                .clone()
                .unwrap_or_else(|| item.path.clone());
            let output_start = ByteOffset::new(existing_output_len);
            let output_start_line = output_line;
            let Some((line_count, ends_with_newline)) = snapshot_range_summary(
                &self.state.sources[item.source_index].text,
                item.excerpt.source_range,
            ) else {
                continue;
            };
            existing_output_len += item.excerpt.source_range.len();
            output_line += line_count;
            if position + 1 < prepared_count && !ends_with_newline {
                existing_output_len += 1;
                output_line += 1;
            }
            let output_end = ByteOffset::new(existing_output_len);
            let output_end_line = output_line;
            let output_range =
                TextRange::new(output_start, output_end).expect("组合片段输出范围必须正序");
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
            let excerpt_index = existing_excerpt_count + valid_excerpts.len();
            let path_index =
                intern_path(&mut next_path_keys, &mut next_path_key_indices, &item.path);
            next_mappings.push(ExcerptMapping {
                excerpt_index,
                path: item.path,
                path_index,
                display_path,
                output_range,
                source_range: item.excerpt.source_range,
                output_start_line,
                output_end_line,
                source_start_line: item.start_line,
                order_line: item.excerpt.order_line.unwrap_or(item.start_line),
                source_index: item.source_index,
                source_id: item.source_id,
                editable: item.excerpt.editable,
                starts_new_excerpt: item.excerpt.starts_new_excerpt,
                diff_kind: item.excerpt.diff_kind,
            });
            valid_excerpts.push(item.excerpt);
        }

        self.state.path_keys = next_path_keys;
        self.state.path_key_indices = next_path_key_indices;
        self.state.excerpts.extend(valid_excerpts);
        self.state.mappings.extend(next_mappings, ());
        self.state
            .match_ranges
            .extend(next_match_ranges.iter().copied());
        self.publish_projection_change(None);
        cx.emit(MultiBufferEvent::TextChanged);
        cx.notify();
        next_match_ranges
    }

    /// 用当前 excerpts 与已有 sources 重算映射与匹配范围，不重建源订阅、不重新校验源范围。
    ///
    /// 供按路径的增量更新使用：excerpt 列表已被调用方裁剪/替换，这里只重算组合坐标。
    fn rebuild_mappings_from_excerpts(
        &mut self,
        cx: &App,
    ) -> (SumTree<ExcerptMapping>, Vec<TextRange>) {
        let mut path_keys = std::mem::take(&mut self.state.path_keys);
        let mut path_key_indices = std::mem::take(&mut self.state.path_key_indices);
        let count = self.state.excerpts.len();
        let mut mappings = Vec::with_capacity(count);
        let mut match_ranges = Vec::new();
        let mut output_offset = 0usize;
        let mut output_line = 0usize;
        for (position, excerpt) in self.state.excerpts.iter().enumerate() {
            let source_id = excerpt.source.entity_id();
            let Some(&source_index) = self.state.source_indices.get(&source_id) else {
                continue;
            };
            let source = &self.state.sources[source_index];
            let path = PathKey::new(
                source
                    .entity
                    .read(cx)
                    .file_path()
                    .map_or_else(PathBuf::new, Path::to_path_buf),
            );
            let display_path = excerpt.display_path.clone().unwrap_or_else(|| path.clone());
            let Some((line_count, ends_with_newline)) =
                snapshot_range_summary(&source.text, excerpt.source_range)
            else {
                continue;
            };
            let output_start = ByteOffset::new(output_offset);
            let output_start_line = output_line;
            output_offset += excerpt.source_range.len();
            output_line += line_count;
            // 非末尾片段若缺换行则补一个合成换行（与 set_excerpts 同一不变式）。
            if position + 1 < count && !ends_with_newline {
                output_offset += 1;
                output_line += 1;
            }
            let output_end = ByteOffset::new(output_offset);
            let output_end_line = output_line;
            let output_range =
                TextRange::new(output_start, output_end).expect("组合片段输出范围必须正序");
            for matched in &excerpt.match_ranges {
                if matched.start() < excerpt.source_range.start()
                    || matched.end() > excerpt.source_range.end()
                {
                    continue;
                }
                let start = output_start.get()
                    + matched
                        .start()
                        .get()
                        .saturating_sub(excerpt.source_range.start().get());
                let end = output_start.get()
                    + matched
                        .end()
                        .get()
                        .saturating_sub(excerpt.source_range.start().get());
                if let Ok(range) = TextRange::new(ByteOffset::new(start), ByteOffset::new(end)) {
                    match_ranges.push(range);
                }
            }
            let start_line = source
                .text
                .byte_to_line(excerpt.source_range.start())
                .map_or(0, |line| line.get());
            let path_index = intern_path(&mut path_keys, &mut path_key_indices, &path);
            mappings.push(ExcerptMapping {
                excerpt_index: position,
                path,
                path_index,
                display_path,
                output_range,
                source_range: excerpt.source_range,
                output_start_line,
                output_end_line,
                source_start_line: start_line,
                order_line: excerpt.order_line.unwrap_or(start_line),
                source_index,
                source_id,
                editable: excerpt.editable,
                starts_new_excerpt: excerpt.starts_new_excerpt,
                diff_kind: excerpt.diff_kind,
            });
        }
        self.state.path_keys = path_keys;
        self.state.path_key_indices = path_key_indices;
        (SumTree::from_iter(mappings, ()), match_ranges)
    }

    /// 移除指定源路径的全部 excerpts；其余片段的组合坐标自动顺延。
    ///
    /// 只重算映射与匹配范围，不重建源订阅；路径不存在时返回 false。
    pub fn remove_excerpts_for_path(&mut self, path: &Path, cx: &mut Context<Self>) -> bool {
        let mut removed = false;
        self.state.excerpts.retain(|excerpt| {
            let is_path = excerpt.source.read(cx).file_path() == Some(path);
            removed |= is_path;
            !is_path
        });
        if !removed {
            return false;
        }
        let (mappings, match_ranges) = self.rebuild_mappings_from_excerpts(cx);
        self.state.mappings = mappings;
        self.state.match_ranges = match_ranges;
        self.publish_projection_change(None);
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
                        cx.emit(MultiBufferEvent::MetadataChanged);
                        cx.notify();
                    }
                })
            })
            .collect::<Vec<_>>();
        self.state
            .sources
            .extend(new_sources.iter().map(|source| ExcerptSource {
                entity: source.clone(),
                text: source.read(cx).text_snapshot(cx),
                syntax: source.read(cx).syntax_snapshot(),
                capture_map: Arc::from([]),
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
    fn excerpt_path(&self, excerpt: &MultiBufferExcerpt, cx: &App) -> PathKey {
        let source_index = self.state.source_indices[&excerpt.source.entity_id()];
        PathKey::new(
            self.state.sources[source_index]
                .entity
                .read(cx)
                .file_path()
                .map_or_else(PathBuf::new, Path::to_path_buf),
        )
    }

    /// 用给定 excerpts 替换其源路径现有的全部 excerpts（按路径有序插入）。
    ///
    /// 路径由片段源的 file_path 确定；同一调用内的片段必须属于同一路径。
    /// 新增源自动注册，其余路径的片段与其组合坐标保持不变。
    pub fn set_excerpts_for_path(
        &mut self,
        excerpts: Vec<MultiBufferExcerpt>,
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
        excerpts: Vec<MultiBufferExcerpt>,
        cx: &mut Context<Self>,
    ) {
        let mut next = Vec::with_capacity(self.state.excerpts.len() + excerpts.len());
        let mut trailing = Vec::new();
        for excerpt in std::mem::take(&mut self.state.excerpts) {
            let excerpt_path = self.excerpt_path(&excerpt, cx);
            if excerpt_path < path {
                next.push(excerpt);
            } else if excerpt_path > path {
                trailing.push(excerpt);
            }
        }
        next.extend(excerpts);
        next.extend(trailing);
        self.state.excerpts = next;
        let (mappings, match_ranges) = self.rebuild_mappings_from_excerpts(cx);
        self.state.mappings = mappings;
        self.state.match_ranges = match_ranges;
        self.publish_projection_change(None);
        cx.emit(MultiBufferEvent::TextChanged);
        cx.notify();
    }

    fn source_changed(&mut self, source_id: gpui::EntityId, cx: &mut Context<Self>) {
        // 源变更在这里统一驱动 diff 重算，并推进虚拟组合投影。
        let source_change = self
            .state
            .source_subscriptions
            .iter()
            .find(|state| state.source.entity_id() == source_id)
            .map(|state| state.text.consume());
        if let Some(source_change) = source_change
            && !source_change.is_empty()
        {
            // 直接编辑已同步当前源快照时，事件只消费对应版本，不能重复推进映射。
            let stored_version = self
                .state
                .sources
                .iter()
                .find(|source| source.entity.entity_id() == source_id)
                .map(|source| source.text.version());
            if source_change.new_version() == stored_version {
                return;
            }
            let position_map = source_change.position_map();
            // 外部源变更：选区先按源坐标推进，再解析到重建后的显示拓扑。
            self.pending_source_remaps
                .push((source_id, position_map.clone()));
            self.recompute_diff_for_source(source_id, DiffRefresh::RebuildProjection, cx);
            // 外部整体刷新会重建 excerpt 拓扑；文本始终由当前 source 快照按需读取。
            if source_change.requires_reset() && self.is_diff_source(source_id, cx) {
                self.refresh_source_snapshot(source_id, cx);
                self.rebuild_diff_projection(cx);
                return;
            }
            // 普通编辑只更新受影响 source 的派生坐标；BufferDiff 仍独立维护 hunk 拓扑。
            self.apply_source_change(source_id, &position_map, Some(&source_change), None, cx);
        } else {
            // 组合编辑已消费该源事务；这里只刷新可能稍后到达的语法快照。
            self.refresh_source_snapshot(source_id, cx);
        }
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
        let text = source.read(cx).text_snapshot(cx);
        let syntax = source.read(cx).syntax_snapshot();
        if let Some(excerpt_source) = self
            .state
            .sources
            .iter_mut()
            .find(|source| source.entity.entity_id() == source_id)
        {
            excerpt_source.text = text.clone();
            excerpt_source.syntax = syntax;
        }
        self.state.capture_names = rebuild_capture_table(&mut self.state.sources);
    }

    /// 把源 Buffer 的版本化编辑同步到组合投影。
    fn apply_source_change(
        &mut self,
        source_id: gpui::EntityId,
        source_position_map: &PositionMap,
        _source_change: Option<&TextChangeBatch>,
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
        // 源事件可能已经由 LanguageBuffer 的同步通知路径物化到显示流；
        // 版本相等表示这次 PositionMap 已被消费，不能再次推进 excerpt 范围。
        let source_version = source.read(cx).text_snapshot(cx).version();
        let stored_version = self
            .state
            .sources
            .iter()
            .find(|candidate| candidate.entity.entity_id() == source_id)
            .map(|candidate| candidate.text.version());
        if _source_change.is_none() && stored_version == Some(source_version) {
            self.refresh_source_snapshot(source_id, cx);
            return;
        }
        self.update_source_ranges(source_id, source_position_map, expanded_excerpts);

        let text = source.read(cx).text_snapshot(cx);
        let syntax = source.read(cx).syntax_snapshot();

        if let Some(excerpt_source) = self
            .state
            .sources
            .iter_mut()
            .find(|source| source.entity.entity_id() == source_id)
        {
            excerpt_source.text = text;
            excerpt_source.syntax = syntax;
        }
        self.state.capture_names = rebuild_capture_table(&mut self.state.sources);
        self.rebuild_projection_layout();
        self.rebuild_match_ranges();
        self.refresh_diff_display(cx);
        let incremental = self.source_incremental_change(source_id, _source_change);
        self.publish_projection_change(incremental);
        cx.emit(MultiBufferEvent::TextChanged);
        cx.notify();
    }

    fn update_source_ranges(
        &mut self,
        source_id: gpui::EntityId,
        source_position_map: &PositionMap,
        expanded_excerpts: Option<&HashSet<usize>>,
    ) {
        for (excerpt_index, excerpt) in self
            .state
            .excerpts
            .iter_mut()
            .enumerate()
            .filter(|(_, excerpt)| excerpt.source.entity_id() == source_id)
        {
            let stickiness = expanded_excerpts.map_or(Stickiness::Expand, |expanded| {
                if expanded.contains(&excerpt_index) {
                    Stickiness::Expand
                } else {
                    Stickiness::Never
                }
            });
            excerpt.source_range = source_position_map
                .map_old_range_with_stickiness(excerpt.source_range, stickiness)
                .value();
            for matched in &mut excerpt.match_ranges {
                *matched = source_position_map
                    .map_old_range_with_stickiness(*matched, Stickiness::Never)
                    .value();
            }
        }
    }

    /// 以 source 快照与 excerpt 范围重建派生坐标。
    ///
    /// 这里不写入任何组合文本；`output_range` 与行摘要是唯一的连续投影索引。
    fn rebuild_projection_layout(&mut self) {
        let mut mappings = mapping_vec(&self.state.mappings);
        let mapping_count = mappings.len();
        let layouts = mappings
            .iter()
            .enumerate()
            .scan(
                (0usize, 0usize),
                |(output_offset, output_line), (index, mapping)| {
                    let excerpt = &self.state.excerpts[mapping.excerpt_index];
                    let source = &self.state.sources[mapping.source_index];
                    let (line_count, ends_with_newline) =
                        snapshot_range_summary(&source.text, excerpt.source_range)?;
                    let output_start = ByteOffset::new(*output_offset);
                    let output_start_line = *output_line;
                    *output_offset += excerpt.source_range.len();
                    *output_line += line_count;
                    if index + 1 < mapping_count && !ends_with_newline {
                        *output_offset += 1;
                        *output_line += 1;
                    }
                    Some((
                        TextRange::new(output_start, ByteOffset::new(*output_offset))
                            .expect("组合 excerpt 输出范围必须正序"),
                        output_start_line,
                        *output_line,
                        source
                            .text
                            .byte_to_line(excerpt.source_range.start())
                            .expect("excerpt 源起点必须有效")
                            .get(),
                    ))
                },
            )
            .collect::<Vec<_>>();

        for (mapping, (output_range, output_start_line, output_end_line, source_start_line)) in
            mappings.iter_mut().zip(layouts)
        {
            mapping.source_range = self.state.excerpts[mapping.excerpt_index].source_range;
            mapping.output_range = output_range;
            mapping.output_start_line = output_start_line;
            mapping.output_end_line = output_end_line;
            mapping.source_start_line = source_start_line;
        }
        self.state.mappings = SumTree::from_iter(mappings, ());
    }

    fn rebuild_match_ranges(&mut self) {
        self.state.match_ranges = self
            .state
            .mappings
            .iter()
            .flat_map(|mapping| {
                self.state.excerpts[mapping.excerpt_index]
                    .match_ranges
                    .iter()
                    .filter(move |matched| {
                        matched.start() >= mapping.source_range.start()
                            && matched.end() <= mapping.source_range.end()
                    })
                    .map(move |matched| {
                        TextRange::new(
                            ByteOffset::new(
                                mapping.output_range.start().get() + matched.start().get()
                                    - mapping.source_range.start().get(),
                            ),
                            ByteOffset::new(
                                mapping.output_range.start().get() + matched.end().get()
                                    - mapping.source_range.start().get(),
                            ),
                        )
                        .expect("excerpt 内匹配投影范围必须有效")
                    })
            })
            .collect();
    }

    fn source_reparsed(&mut self, source_id: gpui::EntityId, cx: &mut Context<Self>) {
        let Some(source) = self
            .state
            .source_subscriptions
            .iter()
            .find(|state| state.source.entity_id() == source_id)
            .map(|state| state.source.clone())
        else {
            return;
        };
        let text = source.read(cx).text_snapshot(cx);
        let syntax = source.read(cx).syntax_snapshot();
        // 按源去重：只更新该源共享的一份 (text, syntax)，所有映射自动跟随。
        if let Some(excerpt_source) = self
            .state
            .sources
            .iter_mut()
            .find(|source| source.entity.entity_id() == source_id)
        {
            excerpt_source.text = text;
            excerpt_source.syntax = syntax;
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
    /// 返回本次编辑的投影重映射：调用方据此把「编辑后、重建前」的投影坐标（如光标）经源忠实落到重建后的当前投影，而不是把裸偏移直接当作重建后坐标。
    pub fn edit(
        &mut self,
        edits: Vec<Edit>,
        metadata: TransactionMetadata,
        cx: &mut Context<Self>,
    ) -> TextResult<ProjectionRemap> {
        if self.capability.is_read_only() {
            return Err(StorageError::ReadOnly.into());
        }

        let mappings = mapping_vec(&self.state.mappings);
        let stored_excerpts = self.state.excerpts.clone();

        let mut grouped: Vec<(Entity<LanguageBuffer>, Vec<Edit>)> = Vec::new();
        let mut edited_excerpts = HashSet::new();
        let push_source_edit =
            |mapping: &ExcerptMapping,
             source_range: TextRange,
             replacement: String,
             grouped: &mut Vec<(Entity<LanguageBuffer>, Vec<Edit>)>,
             edited_excerpts: &mut HashSet<usize>| {
                edited_excerpts.insert(mapping.excerpt_index);
                let source = stored_excerpts[mapping.excerpt_index].source.clone();
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
            let start_index = mappings
                .iter()
                .enumerate()
                .find_map(|(index, mapping)| {
                    let content_end = ByteOffset::new(
                        mapping.output_range.start().get() + mapping.source_range.len(),
                    );
                    ((range.start() >= mapping.output_range.start() && range.start() < content_end)
                        || (mapping.source_range.is_empty()
                            && range.start() == mapping.output_range.start())
                        || (index + 1 == mappings.len() && range.start() == content_end))
                        .then_some(index)
                })
                .or_else(|| {
                    mappings
                        .iter()
                        .position(|mapping| mapping.output_range.start() > range.start())
                })
                .ok_or_else(|| TextError::InvariantViolation {
                    location: "MultiBuffer::edit",
                    detail: "编辑起点不在可见 excerpt 中".to_string(),
                })?;
            let end_index = if range.is_empty() {
                start_index
            } else {
                mappings
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(index, mapping)| {
                        let content_end = ByteOffset::new(
                            mapping.output_range.start().get() + mapping.source_range.len(),
                        );
                        (range.end() > mapping.output_range.start() && range.end() <= content_end)
                            .then_some(index)
                    })
                    .or_else(|| {
                        mappings
                            .iter()
                            .enumerate()
                            .rev()
                            .find_map(|(index, mapping)| {
                                let content_end = ByteOffset::new(
                                    mapping.output_range.start().get() + mapping.source_range.len(),
                                );
                                (content_end < range.end()).then_some(index)
                            })
                    })
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
            if mappings[start_index..=end_index]
                .iter()
                .any(|mapping| !mapping.editable)
            {
                return Err(StorageError::ReadOnly.into());
            }
            let start_mapping = &mappings[start_index];
            let end_mapping = &mappings[end_index];
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
                for mapping in &mappings[start_index + 1..end_index] {
                    push_source_edit(
                        mapping,
                        mapping.source_range,
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

        let mut source_maps = Vec::with_capacity(grouped.len());
        for (source, source_edits) in grouped {
            let outcome = Self::update_source_text(
                &source,
                |buffer| buffer.edit(source_edits, metadata.clone()),
                cx,
            )?;
            source_maps.push((
                source.entity_id(),
                outcome.event().position_map().clone(),
                TextChangeBatch::from_event(outcome.event()),
            ));
        }

        // 组合编辑写回工作区源后，直接将同一份源事件映射到受影响 excerpts。
        // 源事件稍后到达时会看到相同的显示版本，只消费事件而不再次物化。
        // hunk 变化由 BufferDiffEvent::DiffChanged 异步驱动物化；
        // 本轮回传的映射即编辑后、重物化前的坐标系，选区落位不依赖 diff 重建时机。
        for (source_id, position_map, source_change) in &source_maps {
            self.apply_source_change(
                *source_id,
                position_map,
                Some(source_change),
                Some(&edited_excerpts),
                cx,
            );
        }
        Ok(ProjectionRemap::identity())
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
        source.update(cx, |source, cx| {
            source.synchronize_pending_changes(cx);
        });
        Ok(result)
    }

    fn rebuild_display(&mut self, cx: &mut Context<Self>) {
        let excerpts = std::mem::take(&mut self.state.excerpts);
        self.set_excerpts(excerpts, cx);
    }

    pub fn start_transaction(&mut self, cx: &mut Context<Self>) -> TextResult<TransactionId> {
        if self.history_owner() == HistoryOwner::SourceBuffer {
            let source = self
                .working_source
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
        let ExcerptState {
            excerpts,
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
        for excerpt in excerpts.iter().filter(|excerpt| excerpt.editable) {
            let buffer = excerpt.source.read(cx).buffer();
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
                .working_source
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
                .working_source
                .as_ref()
                .expect("共享源历史必须有工作区源");
            let buffer = source.read(cx).buffer();
            let buffer = buffer.read(cx);
            return buffer
                .current_history_node()
                .and_then(|node| buffer.history_node(node))
                .map(|node| node.transaction_id);
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
                .working_source
                .clone()
                .expect("共享源历史必须有工作区源");
            let source_buffer = source.read(cx).buffer();
            let old_version = self.snapshot(cx).version();
            let source_subscription = source_buffer.update(cx, |buffer, _| buffer.subscribe());
            let outcome = Self::update_source_text(
                &source,
                |buffer| if redo { buffer.redo() } else { buffer.undo() },
                cx,
            )?;
            let Some(outcome) = outcome else {
                return Ok(None);
            };
            let source_change = source_subscription.consume();
            let source_map = source_change.position_map();
            self.recompute_diff_for_source(source.entity_id(), DiffRefresh::PreserveProjection, cx);
            self.apply_source_change(
                source.entity_id(),
                &source_map,
                Some(&source_change),
                None,
                cx,
            );
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

        let mut source_changes = Vec::new();
        for (buffer, expected_transaction) in &entry.buffers {
            if !redo
                && buffer
                    .read(cx)
                    .current_history_node()
                    .and_then(|id| buffer.read(cx).history_node(id))
                    .is_none_or(|node| node.transaction_id != *expected_transaction)
            {
                return Err(TextError::InvariantViolation {
                    location: "MultiBuffer::undo",
                    detail: "底层 Buffer 历史已在 MultiBuffer 外部分叉".to_string(),
                });
            }
            // 订阅源 Buffer：合并事务的 undo/redo 会回放多个批次，订阅批次经 compose 给出跨批次复合 old→new 映射。
            let Some(source) = self
                .state
                .excerpts
                .iter()
                .find(|excerpt| excerpt.source.read(cx).buffer().entity_id() == buffer.entity_id())
                .map(|excerpt| excerpt.source.clone())
            else {
                return Err(TextError::InvariantViolation {
                    location: "MultiBuffer::replay_history",
                    detail: "历史 Buffer 不再属于当前组合文档源".to_string(),
                });
            };
            let source_subscription = buffer.update(cx, |buffer, _| buffer.subscribe());
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
            let source_change = source_subscription.consume();
            source_changes.push((source_id, source_change.position_map(), source_change));
        }
        for (source_id, position_map, source_change) in &source_changes {
            self.recompute_diff_for_source(*source_id, DiffRefresh::PreserveProjection, cx);
            self.apply_source_change(*source_id, position_map, Some(source_change), None, cx);
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

    pub fn snapshot(&self, _cx: &App) -> MultiBufferSnapshot {
        let (plain_text, plain_syntax) = if self.state.mappings.is_empty() {
            self.working_source.as_ref().map_or((None, None), |source| {
                let source = source.read(_cx);
                (
                    Some(source.text_snapshot(_cx)),
                    Some(source.syntax_snapshot()),
                )
            })
        } else {
            (None, None)
        };
        let excerpts = self
            .state
            .mappings
            .iter()
            .map(|mapping| ExcerptSnapshot {
                path: mapping.path.clone(),
                display_path: mapping.display_path.clone(),
                output_range: mapping.output_range,
                source_range: mapping.source_range,
                output_start_line: mapping.output_start_line,
                output_end_line: mapping.output_end_line,
                source_start_line: mapping.source_start_line,
                editable: mapping.editable,
                starts_new_excerpt: mapping.starts_new_excerpt,
                diff_kind: mapping.diff_kind,
            })
            .collect::<Arc<[_]>>();
        MultiBufferSnapshot {
            plain_text,
            plain_syntax,
            config: self
                .state
                .sources
                .first()
                .map(|source| source.entity.read(_cx).buffer().read(_cx).config().clone())
                .unwrap_or_default(),
            projection_version: self.state.projection_version,
            excerpts,
            excerpt_mappings: Arc::from(mapping_vec(&self.state.mappings)),
            excerpt_tree: self.state.mappings.clone(),
            path_keys: Arc::from(self.state.path_keys.clone()),
            excerpt_sources: Arc::from(
                self.state
                    .sources
                    .iter()
                    .map(|source| ExcerptSourceSnapshot {
                        text: source.text.clone(),
                        syntax: source.syntax.clone(),
                        capture_map: Arc::clone(&source.capture_map),
                    })
                    .collect::<Vec<_>>(),
            ),
            capture_names: Arc::clone(&self.state.capture_names),
        }
    }

    /// 普通整文件文档的底层文本。
    ///
    /// 文档角色由构造时的 working source 决定，不随 diff 展开后的 excerpt 形状变化。
    pub fn as_singleton(&self, cx: &App) -> Option<Entity<Buffer>> {
        self.working_source
            .as_ref()
            .map(|source| source.read(cx).buffer())
    }

    /// 普通编辑器的工作区源（展开 diff 时作为新侧输入）。
    pub fn working_source(&self) -> Option<Entity<LanguageBuffer>> {
        self.working_source.clone()
    }

    pub fn is_read_only(&self) -> bool {
        self.capability.is_read_only()
    }

    pub fn is_dirty(&self, cx: &App) -> bool {
        self.state
            .excerpts
            .iter()
            .filter(|excerpt| excerpt.editable)
            .any(|excerpt| excerpt.source.read(cx).buffer().read(cx).is_dirty())
    }

    /// 文档实际引用的、可落盘的底层文件 Buffer。
    ///
    /// 收集可编辑 excerpts 的源 Buffer 并按实体去重；无路径源（内存草稿）不参与。
    /// 显示文本物化 Buffer 永远不会出现在结果中。
    pub fn file_buffers(&self, cx: &App) -> Vec<(Entity<Buffer>, PathBuf)> {
        let mut buffers = Vec::<(Entity<Buffer>, PathBuf)>::new();
        for excerpt in self
            .state
            .excerpts
            .iter()
            .filter(|excerpt| excerpt.editable)
        {
            let source = excerpt.source.read(cx);
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
        self.working_source
            .as_ref()
            .and_then(|source| source.read(cx).file_path().map(Path::to_path_buf))
            .or_else(|| {
                self.state
                    .excerpts
                    .iter()
                    .find(|excerpt| excerpt.editable)
                    .and_then(|excerpt| excerpt.source.read(cx).file_path().map(Path::to_path_buf))
            })
    }

    pub fn location_for_offset(&self, offset: ByteOffset) -> Option<ExcerptLocation> {
        let range = TextRange::new(offset, offset).expect("同点组合范围必须有效");
        self.location_for_range(range)
    }

    /// 把当前组合偏移锚定到底层文件坐标，并记录文件消失时的邻接解析顺序。
    pub fn anchor_for_offset(&self, offset: ByteOffset) -> Option<MultiBufferAnchor> {
        anchor_in_mappings(&self.state.mappings, offset)
    }

    /// 把「编辑后、重建前」投影坐标锚定为源锚点（编辑器源锚点选区的编辑落位）。
    ///
    /// 闭包给出的编辑后选区落在重建前投影坐标；
    /// 重建（reclip）时用重建前映射锚定，未重建时当前映射即编辑后映射。锚定到源后，重建不改变源，选区按重建后快照解析即忠实落位。
    pub fn anchor_after_edit(
        &self,
        remap: &ProjectionRemap,
        offset: ByteOffset,
    ) -> Option<MultiBufferAnchor> {
        match &remap.before {
            Some(before) => anchor_in_mappings(before, offset),
            None => self.anchor_for_offset(offset),
        }
    }

    /// 取走并清空外部源变更留下的源 PositionMap（编辑器据此推进源锚点选区）。
    pub fn take_pending_source_remaps(&mut self) -> Vec<(gpui::EntityId, PositionMap)> {
        std::mem::take(&mut self.pending_source_remaps)
            .into_iter()
            .collect()
    }

    /// 在当前 excerpts 中解析稳定位置；同一文件仍存在时优先落到最接近的源片段。
    pub fn resolve_anchor(&self, anchor: &MultiBufferAnchor) -> Option<ByteOffset> {
        resolve_anchor_in_mappings(&self.state.mappings, &self.state.path_keys, anchor)
    }

    /// 把组合文档中的选区映射回同一个源片段；跨片段选区没有单一源位置。
    pub fn location_for_range(&self, range: TextRange) -> Option<ExcerptLocation> {
        let state = &self.state;
        let mappings = mapping_vec(&state.mappings);
        let mapping = state
            .mappings
            .iter()
            .enumerate()
            .find_map(|(index, mapping)| {
                let starts_inside = range.start() >= mapping.output_range.start();
                let ends_inside = range.end() <= mapping.output_range.end();
                let empty_point_inside = !range.is_empty()
                    || range.start() < mapping.output_range.end()
                    || (index + 1 == mappings.len() && range.start() == mapping.output_range.end());
                (starts_inside && ends_inside && empty_point_inside).then_some(mapping)
            })?;
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
    pub fn match_ranges(&self) -> &[TextRange] {
        &self.state.match_ranges
    }

    /// 更新工作区源的文件路径并重建投影（路径参与 excerpt 元数据与锚点解析）。
    pub fn set_file_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(source) = self.working_source.clone() else {
            return;
        };
        source.update(cx, |source, cx| source.set_file_path(path, cx));
        self.rebuild_display(cx);
    }

    /// `offset` 处所在 excerpt 的源语言名（组合文档按光标所在源文件显示语言）。
    pub fn language_at(&self, offset: ByteOffset, cx: &App) -> Option<&'static str> {
        let mapping = self.mapping_at(offset)?;
        let excerpt = self.state.excerpts.get(mapping.excerpt_index)?;
        excerpt.source.read(cx).language_name()
    }

    /// `offset` 处 source 的 Buffer 配置；无 excerpt 时使用显式默认配置。
    pub fn buffer_config_at(&self, offset: ByteOffset, cx: &App) -> BufferConfig {
        self.mapping_at(offset)
            .and_then(|mapping| self.state.excerpts.get(mapping.excerpt_index))
            .map(|excerpt| excerpt.source.read(cx).buffer())
            .map(|buffer| buffer.read(cx).config().clone())
            .unwrap_or_default()
    }

    /// 当前已安装解析对应的折叠范围。
    ///
    /// 一个源可能被展开的 diff hunk 切成多个 excerpt：
    /// 只要这些 excerpt 在源内连续覆盖，折叠范围就跨它们投影到组合坐标（中间夹入的旧侧 excerpt 也落在折叠范围内）；
    /// 跨过未展示内容或文件边界的折叠仍被丢弃。
    pub fn fold_ranges(&self, cx: &App) -> Arc<[FoldRange]> {
        let mut projected = Vec::new();
        for (source_index, source) in self.state.sources.iter().enumerate() {
            let source_folds = source.entity.read(cx).fold_ranges();
            if source_folds.is_empty() {
                continue;
            }
            let mappings: Vec<&ExcerptMapping> = self
                .state
                .mappings
                .iter()
                .filter(|mapping| mapping.source_index == source_index)
                .collect();
            for fold in source_folds.iter() {
                let (start, end) = (fold.range.start, fold.range.end);
                if start >= end {
                    continue;
                }
                let Some(start_index) = mappings.iter().position(|mapping| {
                    mapping.source_range.start().get() <= start
                        && start < mapping.source_range.end().get()
                }) else {
                    continue;
                };
                let Some(end_index) = mappings.iter().position(|mapping| {
                    mapping.source_range.start().get() < end
                        && end <= mapping.source_range.end().get()
                }) else {
                    continue;
                };
                if start_index > end_index {
                    continue;
                }
                // 起止 excerpt 之间必须在源内连续覆盖，否则折叠跨过未展示内容。
                if !mappings[start_index..end_index]
                    .windows(2)
                    .all(|pair| pair[0].source_range.end() == pair[1].source_range.start())
                {
                    continue;
                }
                let start_mapping = mappings[start_index];
                let end_mapping = mappings[end_index];
                let output_start = start_mapping.output_range.start().get() + start
                    - start_mapping.source_range.start().get();
                let output_end = end_mapping.output_range.start().get() + end
                    - end_mapping.source_range.start().get();
                if output_start < output_end {
                    projected.push(FoldRange {
                        range: output_start..output_end,
                    });
                }
            }
        }
        projected.sort_unstable_by_key(|fold| (fold.range.start, fold.range.end));
        projected.dedup();
        Arc::from(projected)
    }

    /// 定位组合偏移所属的映射；最后一个映射的结束偏移视为命中（光标位于文档末尾）。
    fn mapping_at(&self, offset: ByteOffset) -> Option<ExcerptMapping> {
        mapping_at_tree(&self.state.mappings, offset).cloned()
    }

    /// `offset` 处所在 excerpt 源语言的自动闭合对。
    pub fn auto_close_pairs(
        &self,
        offset: ByteOffset,
        cx: &App,
    ) -> Option<&'static [AutoClosePair]> {
        let mapping = self.mapping_at(offset)?;
        let excerpt = self.state.excerpts.get(mapping.excerpt_index)?;
        Some(excerpt.source.read(cx).language()?.auto_close_pairs())
    }
}

/// 按真实 source 内容定位组合偏移。
///
/// 非末尾 excerpt 为分隔而补出的换行不属于任何 source；位于该换行上的光标按编辑语义落到后继 excerpt。
/// 用累积输出字节游标定位组合偏移所属的映射（O(log n)）。
///
/// 语义与旧的线性扫描一致：命中映射内容的偏移；空片段命中其起点；
/// 合成换行区域的偏移落到下一个映射；文档末尾命中最后一个映射。
fn mapping_at_tree(tree: &SumTree<ExcerptMapping>, offset: ByteOffset) -> Option<&ExcerptMapping> {
    let mut cursor = tree.cursor::<OutputOffset>(());
    cursor.seek(&OutputOffset(offset.get()), Bias::Right);
    if cursor.item().is_none() {
        // 偏移在文档末尾（或之后）：命中最后一个映射。
        let mut last = tree.cursor::<OutputOffset>(());
        last.seek(&OutputOffset(tree.summary().bytes), Bias::Left);
        return last.item();
    }
    let (content_end, is_empty, start) = {
        let mapping = cursor.item()?;
        (
            ByteOffset::new(mapping.output_range.start().get() + mapping.source_range.len()),
            mapping.source_range.is_empty(),
            mapping.output_range.start(),
        )
    };
    if offset < content_end || (is_empty && offset == start) {
        return cursor.item();
    }
    // 落在合成换行区域：优先落到下一个映射，没有下一个则命中最后一个。
    cursor.next();
    if let Some(next) = cursor.item() {
        return Some(next);
    }
    let mut last = tree.cursor::<OutputOffset>(());
    last.seek(&OutputOffset(tree.summary().bytes), Bias::Left);
    last.item()
}

/// 在给定投影→源映射中把投影偏移锚定到源坐标，并记录文件消失时的邻接解析顺序。
///
/// [`MultiBuffer::anchor_for_offset`]（当前映射）与 [`MultiBuffer::remap_offset`]（重建前映射）共用此锚定逻辑。
fn anchor_in_mappings(
    tree: &SumTree<ExcerptMapping>,
    offset: ByteOffset,
) -> Option<MultiBufferAnchor> {
    let mapping = mapping_at_tree(tree, offset)?;
    let source_offset = ByteOffset::new(
        (mapping.source_range.start().get()
            + offset
                .get()
                .saturating_sub(mapping.output_range.start().get()))
        .min(mapping.source_range.end().get()),
    );
    let path_index = mapping.path_index;
    let source_id = mapping.source_id;
    let found_start = mapping.output_range.start();

    let mut following = HashSet::new();
    let mut following_paths = Vec::new();
    let mut cursor = tree.cursor::<OutputOffset>(());
    cursor.seek(&OutputOffset(found_start.get()), Bias::Right);
    if cursor
        .item()
        .is_some_and(|item| item.output_range.start() == found_start)
    {
        cursor.next();
    }
    while let Some(item) = cursor.item() {
        if item.path_index != path_index && following.insert(item.path_index) {
            following_paths.push(item.path_index);
        }
        cursor.next();
    }

    let mut preceding = HashSet::new();
    let mut preceding_paths = Vec::new();
    let mut cursor = tree.cursor::<OutputOffset>(());
    cursor.seek(&OutputOffset(found_start.get()), Bias::Left);
    if cursor
        .item()
        .is_some_and(|item| item.output_range.start() == found_start)
    {
        cursor.prev();
    }
    while let Some(item) = cursor.item() {
        if item.path_index != path_index && preceding.insert(item.path_index) {
            preceding_paths.push(item.path_index);
        }
        cursor.prev();
    }

    Some(MultiBufferAnchor {
        path: path_index,
        source_id,
        source_offset,
        following_paths,
        preceding_paths,
    })
}

fn nearest_output_offset_for_source(
    tree: &SumTree<ExcerptMapping>,
    path_keys: &[PathKey],
    path: PathKeyIndex,
    source_id: Option<gpui::EntityId>,
    source_offset: ByteOffset,
) -> Option<ByteOffset> {
    let path_key = path_keys.get(path.get() as usize)?;
    let mut cursor = tree.cursor::<ExcerptMappingSummary>(());
    cursor.seek(path_key, Bias::Left);
    let matching = cursor
        .take_while(|mapping| &mapping.path == path_key)
        .filter(|mapping| source_id.is_none_or(|source_id| mapping.source_id == source_id))
        .collect::<Vec<_>>();

    // 源位置仍在可见 excerpt 内时，不应进入“最近”逻辑。
    // 半开区间的边界属于后一段；
    // 只有最后一段的结束位置仍归最后一段，保证删除前一行后光标落在下一行开头，而不是回跳到前一段。
    if let Some(mapping) = matching
        .iter()
        .copied()
        .find(|mapping| {
            mapping.source_range.start() <= source_offset
                && source_offset < mapping.source_range.end()
        })
        .or_else(|| {
            matching
                .last()
                .copied()
                .filter(|mapping| mapping.source_range.end() == source_offset)
        })
    {
        return Some(ByteOffset::new(
            mapping.output_range.start().get()
                + source_offset
                    .get()
                    .saturating_sub(mapping.source_range.start().get()),
        ));
    }

    matching
        .into_iter()
        .map(|mapping| {
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
                mapping.output_range.start().get()
                    + clamped
                        .get()
                        .saturating_sub(mapping.source_range.start().get()),
            );
            (distance, at_end_only, output)
        })
        .min_by_key(|(distance, at_end_only, _)| (*distance, *at_end_only))
        .map(|(_, _, output)| output)
}

/// 在给定投影→源映射中把源锚点解析回投影偏移；同一文件仍存在时优先落到最接近的源片段。
///
/// [`MultiBuffer::resolve_anchor`]（当前映射）与 [`MultiBufferSnapshot::resolve_anchor`]（快照映射）共用此解析逻辑。
fn resolve_anchor_in_mappings(
    tree: &SumTree<ExcerptMapping>,
    path_keys: &[PathKey],
    anchor: &MultiBufferAnchor,
) -> Option<ByteOffset> {
    if let Some(offset) = nearest_output_offset_for_source(
        tree,
        path_keys,
        anchor.path,
        Some(anchor.source_id),
        anchor.source_offset,
    ) {
        return Some(offset);
    }
    if let Some(offset) =
        nearest_output_offset_for_source(tree, path_keys, anchor.path, None, anchor.source_offset)
    {
        return Some(offset);
    }
    for path in &anchor.following_paths {
        if let Some(path_key) = path_keys.get(path.get() as usize)
            && let Some(mapping) = first_mapping_for_path(tree, path_key)
        {
            return Some(mapping.output_range.start());
        }
    }
    for path in &anchor.preceding_paths {
        if let Some(path_key) = path_keys.get(path.get() as usize)
            && let Some(mapping) = last_mapping_for_path(tree, path_key)
        {
            return Some(mapping.output_range.end());
        }
    }
    None
}

/// 路径区间内的第一个映射（路径不存在时 None）。
fn first_mapping_for_path<'a>(
    tree: &'a SumTree<ExcerptMapping>,
    path_key: &PathKey,
) -> Option<&'a ExcerptMapping> {
    let mut cursor = tree.cursor::<ExcerptMappingSummary>(());
    cursor.seek(path_key, Bias::Left);
    cursor.item().filter(|mapping| &mapping.path == path_key)
}

/// 路径区间内的最后一个映射（路径不存在时 None）。
fn last_mapping_for_path<'a>(
    tree: &'a SumTree<ExcerptMapping>,
    path_key: &PathKey,
) -> Option<&'a ExcerptMapping> {
    let mut cursor = tree.cursor::<ExcerptMappingSummary>(());
    // Right 定位到路径区间之后（或末尾），回退一个即该路径的最后一个映射。
    cursor.seek(path_key, Bias::Right);
    cursor.prev();
    cursor.item().filter(|mapping| &mapping.path == path_key)
}

#[cfg(test)]
#[path = "test/multi_buffer_tests.rs"]
mod tests;
