//! Editor 与具体文本 Buffer 之间的组合文档边界。
//!
//! 组合文档按调用方给出的顺序物化多个来源的 excerpts，并保留组合坐标到源文件坐标的映射。
//! 普通编辑器是「整文件单 excerpt」的组合文档，与多文件文档走同一条链路。
//! Editor 始终只消费本层，不感知来源数量。
//! diff 投影（git hunks、展开状态、跟踪区间与显示坐标）也归本层，见 [`diff_projection`]。

mod diff_projection;

pub use diff_projection::{DiffFileInput, DiffHunkSourceInfo};

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Subscription};
use zcv_language::{
    AutoClosePair, BracketPair, FoldRange, HighlightSpan, LanguageBuffer, LanguageBufferEvent,
    NewlineIndent, SyntaxSnapshot,
};
use zcv_text::{
    Buffer, BufferConfig, BufferVersion, ByteOffset, Edit, Line, PositionMap, Snapshot, Stickiness,
    StorageError, TextChangeBatch, TextError, TextRange, TextResult, TextSubscription,
    TransactionId, TransactionMetadata,
};

/// 组合文档中的一个源片段。
#[derive(Clone)]
pub struct MultiBufferExcerpt {
    source: Entity<LanguageBuffer>,
    source_range: TextRange,
    match_ranges: Vec<TextRange>,
    display_path: Option<PathBuf>,
    editable: bool,
    starts_new_excerpt: bool,
    diff_kind: Option<ExcerptDiffKind>,
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
        }
    }

    pub fn with_display_path(mut self, path: PathBuf) -> Self {
        self.display_path = Some(path);
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
    path: PathBuf,
    source_id: gpui::EntityId,
    source_offset: ByteOffset,
    following_paths: Vec<PathBuf>,
    preceding_paths: Vec<PathBuf>,
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
    path: PathBuf,
    display_path: PathBuf,
    output_range: TextRange,
    source_range: TextRange,
    output_start_line: usize,
    output_end_line: usize,
    source_start_line: usize,
    /// 指向源表（`ExcerptState::sources` / 快照的 `excerpt_sources`）的索引。
    source_index: usize,
    source_id: gpui::EntityId,
    editable: bool,
    starts_new_excerpt: bool,
    diff_kind: Option<ExcerptDiffKind>,
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

/// 多文件文档中一个可见片段的一帧元数据。
///
/// 文本仍通过组合投影供编辑器的折叠、换行和命中测试使用；
/// 路径、源行号、语法与边界保持为一等数据，不能编码进投影文本。
/// Editor 的通用文件标题和片段分隔块只消费本结构。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExcerptSnapshot {
    path: PathBuf,
    display_path: PathBuf,
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
        &self.path
    }

    pub fn display_path(&self) -> &Path {
        &self.display_path
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

    pub fn source_start_line(&self) -> usize {
        self.source_start_line
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
            .then(|| self.source_start_line + output_line - self.output_start_line)
    }
}

/// 一帧组合文档的不可变快照。
#[derive(Clone, Debug)]
pub struct MultiBufferSnapshot {
    text: Snapshot,
    syntax: SyntaxSnapshot,
    excerpts: Arc<[ExcerptSnapshot]>,
    excerpt_mappings: Arc<[ExcerptMapping]>,
    /// 按源去重的 (text, syntax, capture_map) 表（映射经 `source_index` 引用）。
    excerpt_sources: Arc<[ExcerptSourceSnapshot]>,
    capture_names: Arc<[Arc<str>]>,
}

/// Editor 对组合文档变化的独立订阅。
pub struct MultiBufferSubscription {
    text: TextSubscription,
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

impl MultiBufferSubscription {
    pub fn consume(&self) -> TextChangeBatch {
        self.text.consume()
    }
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
    pub fn text(&self) -> &Snapshot {
        &self.text
    }

    pub fn syntax(&self) -> &SyntaxSnapshot {
        &self.syntax
    }

    /// 返回当前组合文档的完整 UTF-8 内容，供预览等只读消费者使用。
    pub fn text_bytes(&self) -> Vec<u8> {
        self.text
            .slice_byte_range(ByteOffset::ZERO, self.text.len_bytes())
            .expect("完整快照范围必须有效")
            .as_str()
            .as_bytes()
            .to_vec()
    }

    pub fn version(&self) -> BufferVersion {
        self.text.version()
    }

    pub fn excerpts(&self) -> &[ExcerptSnapshot] {
        &self.excerpts
    }

    pub fn capture_names(&self) -> Arc<[Arc<str>]> {
        Arc::clone(&self.capture_names)
    }

    /// 查询组合坐标中的语法高亮，并把每个源 Buffer 的 capture index 映射到本快照的统一表。
    ///
    /// 无 excerpt 的纯文本帧（placeholder 等）退回快照自身语法表。
    pub fn highlights(&self, range: std::ops::Range<usize>) -> Vec<HighlightSpan> {
        if self.excerpt_mappings.is_empty() {
            return self.syntax.highlights(range, &self.text);
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
            let end = offset
                .get()
                .saturating_add(1)
                .min(self.text.len_bytes().get());
            return self.syntax.bracket_pairs(start..end, &self.text);
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
            return self.syntax.suggested_newline_indent(offset, &self.text);
        };
        source
            .syntax
            .suggested_newline_indent(source_offset, &source.text)
    }

    /// 查询严格包围组合范围的最小 source 语法节点，并映射回组合坐标。
    pub fn ancestor_range(&self, range: std::ops::Range<usize>) -> Option<std::ops::Range<usize>> {
        let Some((mapping, source, source_range)) = self.source_range(range.clone()) else {
            return self.syntax.ancestor_range(range, &self.text);
        };
        let ancestor = source.syntax.ancestor_range(source_range, &source.text)?;
        let excerpt_start = mapping.source_range.start().get();
        let excerpt_end = mapping.source_range.end().get();
        if ancestor.start < excerpt_start || ancestor.end > excerpt_end {
            return None;
        }
        let output_start = mapping.output_range.start().get();
        Some(
            (output_start + ancestor.start - excerpt_start)
                ..(output_start + ancestor.end - excerpt_start),
        )
    }

    fn source_point(
        &self,
        offset: ByteOffset,
    ) -> Option<(&ExcerptMapping, &ExcerptSourceSnapshot, ByteOffset)> {
        let index = mapping_index_at(&self.excerpt_mappings, offset)?;
        let mapping = &self.excerpt_mappings[index];
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
        let index = mapping_index_at(&self.excerpt_mappings, ByteOffset::new(range.start))?;
        let mapping = &self.excerpt_mappings[index];
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
        self.excerpts
            .iter()
            .find(|excerpt| line >= excerpt.output_start_line && line < excerpt.output_end_line)
    }
}

/// 纯文本帧：无 excerpt 的独立文本（placeholder 等），语法为空表。
impl From<Snapshot> for MultiBufferSnapshot {
    fn from(text: Snapshot) -> Self {
        let capture_names = SyntaxSnapshot::empty(text.version()).capture_names();
        Self {
            syntax: SyntaxSnapshot::empty(text.version()),
            excerpts: Arc::from([]),
            excerpt_mappings: Arc::from([]),
            excerpt_sources: Arc::from([]),
            capture_names,
            text,
        }
    }
}

struct ExcerptState {
    projection: Entity<LanguageBuffer>,
    excerpts: Vec<MultiBufferExcerpt>,
    source_subscriptions: Vec<SourceSubscription>,
    source_event_subscriptions: Vec<Subscription>,
    mappings: Vec<ExcerptMapping>,
    /// 按源去重的 (text, syntax, capture_map) 表。
    sources: Vec<ExcerptSource>,
    /// 源实体到 `sources` 索引的派生索引，供增量追加按身份查找源状态。
    source_indices: HashMap<gpui::EntityId, usize>,
    match_ranges: Vec<TextRange>,
    capture_names: Arc<[Arc<str>]>,
    next_transaction_id: TransactionId,
    active_transaction: Option<TransactionId>,
    active_source_transactions: Vec<Entity<Buffer>>,
    undo_stack: Vec<CompositeHistoryEntry>,
    redo_stack: Vec<CompositeHistoryEntry>,
}

/// Editor 持有的组合文档模型。
///
/// 恒为 excerpts 形态；普通编辑器是整文件单 excerpt，与多文件文档共用同一套投影、编辑与历史链路。
pub struct MultiBuffer {
    state: ExcerptState,
    read_only: bool,
    /// 普通整文件文档的稳定角色与权威源。
    ///
    /// excerpts 会因 diff 展开而改变形状，不能据此推断文档角色；
    /// 历史、配置、重命名与保存等文件级事实按本字段委托给底层 LanguageBuffer。
    /// `None` 表示真正的多来源组合文档。
    working_source: Option<Entity<LanguageBuffer>>,
    /// git 行级 diff 投影（hunks、展开状态、跟踪区间与显示坐标）；`None` = 无 diff 需求。
    diff: Option<Box<diff_projection::DiffProjection>>,
}

impl EventEmitter<MultiBufferEvent> for MultiBuffer {}

impl MultiBuffer {
    /// 创建空的可编辑组合文档；调用方可重复设置 ordered excerpts。
    pub fn empty(cx: &mut Context<Self>) -> Self {
        Self::empty_with_read_only(false, cx)
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
        multi_buffer.sync_working_source_config(source.entity_id(), cx);
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
        Self::empty_with_read_only(true, cx)
    }

    fn empty_with_read_only(read_only: bool, cx: &mut Context<Self>) -> Self {
        Self {
            state: Self::empty_excerpt_state(cx),
            read_only,
            working_source: None,
            diff: None,
        }
    }

    /// 空组合状态骨架：独立投影 buffer + 空 excerpts/sources。
    fn empty_excerpt_state(cx: &mut Context<Self>) -> ExcerptState {
        let text = Buffer::scratch(String::new(), BufferConfig::default())
            .expect("空组合文档 Buffer 应能创建");
        let text = cx.new(|_| text);
        let projection = cx.new(|cx| LanguageBuffer::new(text.clone(), None, cx));
        cx.subscribe(&projection, |_, _, event, cx| {
            cx.emit(match event {
                LanguageBufferEvent::TextChanged => MultiBufferEvent::TextChanged,
                LanguageBufferEvent::Reparsed => MultiBufferEvent::Reparsed,
                LanguageBufferEvent::MetadataChanged => MultiBufferEvent::MetadataChanged,
            });
            cx.notify();
        })
        .detach();
        ExcerptState {
            projection,
            excerpts: Vec::new(),
            source_subscriptions: Vec::new(),
            source_event_subscriptions: Vec::new(),
            mappings: Vec::new(),
            sources: Vec::new(),
            source_indices: HashMap::new(),
            match_ranges: Vec::new(),
            capture_names: Arc::from([]),
            next_transaction_id: TransactionId::INITIAL,
            active_transaction: None,
            active_source_transactions: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
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
                        this.sync_working_source_config(observed.entity_id(), cx);
                        cx.emit(MultiBufferEvent::MetadataChanged);
                        cx.notify();
                    }
                })
            })
            .collect::<Vec<_>>();
        let ExcerptState {
            projection,
            excerpts: stored_excerpts,
            source_subscriptions,
            source_event_subscriptions,
            mappings,
            sources,
            source_indices,
            match_ranges,
            capture_names: composite_capture_names,
            ..
        } = &mut self.state;

        // 按源去重构建 (text, syntax) 表：同一文件的大量片段共享一份源状态。
        let mut next_sources: Vec<ExcerptSource> = Vec::new();
        let mut next_source_indices = HashMap::new();
        struct PreparedExcerpt {
            excerpt: MultiBufferExcerpt,
            path: PathBuf,
            source_index: usize,
            source_id: gpui::EntityId,
            text: String,
            start_line: usize,
        }
        let mut prepared = Vec::with_capacity(excerpts.len());
        for excerpt in excerpts {
            let source = excerpt.source.read(cx);
            // 无路径的临时 Buffer（单行输入框等）以空路径参与组合；
            // 路径身份用于文件级折叠、标题与锚点解析。
            let path = source
                .file_path()
                .map_or_else(PathBuf::new, Path::to_path_buf);
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
            let Ok(text) = source_snapshot.text.slice_text(excerpt.source_range) else {
                continue;
            };
            let start_line = source_snapshot
                .text
                .byte_to_line(excerpt.source_range.start())
                .map_or(1, |line| line.get() + 1);
            prepared.push(PreparedExcerpt {
                excerpt,
                path,
                source_index,
                source_id,
                text: text.as_str().to_owned(),
                start_line,
            });
        }

        // 物化组合投影（对齐 Zed 的 excerpt 尾换行不变式）：
        // 每个非末尾片段都以完整行边界结束（内容原样投影，末尾缺换行时补一个，空片段同样适用）；
        // 末尾片段保留内容原样。空片段（空文件、折叠 hunk 占位）经此不变式自然占据边界行，不做特例补行。
        let prepared_count = prepared.len();
        let mut output = String::new();
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
            let output_start = ByteOffset::new(output.len());
            let output_start_line = output_line;
            output.push_str(&item.text);
            output_line += item.text.bytes().filter(|byte| *byte == b'\n').count();
            if position + 1 < prepared_count && !item.text.ends_with('\n') {
                output.push('\n');
                output_line += 1;
            }
            let output_end = ByteOffset::new(output.len());
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
            next_mappings.push(ExcerptMapping {
                excerpt_index,
                path: item.path,
                display_path,
                output_range,
                source_range: item.excerpt.source_range,
                output_start_line,
                output_end_line,
                source_start_line: item.start_line,
                source_index: item.source_index,
                source_id: item.source_id,
                editable: item.excerpt.editable,
                starts_new_excerpt: item.excerpt.starts_new_excerpt,
                diff_kind: item.excerpt.diff_kind,
            });
            valid_excerpts.push(item.excerpt);
        }

        let text_buffer = projection.read(cx).buffer();
        text_buffer.update(cx, |buffer, cx| {
            buffer
                .reload_from_text(output)
                .expect("组合文档投影必须是合法 UTF-8 文本");
            cx.notify();
        });
        *source_subscriptions = next_source_subscriptions;
        *source_event_subscriptions = next_source_event_subscriptions;
        *stored_excerpts = valid_excerpts;
        *mappings = next_mappings;
        *sources = next_sources;
        *source_indices = sources
            .iter()
            .enumerate()
            .map(|(index, source)| (source.entity.entity_id(), index))
            .collect();
        *match_ranges = next_match_ranges;
        *composite_capture_names = rebuild_capture_table(sources);
        cx.notify();
    }

    /// 在现有组合文档末尾追加有序片段。
    ///
    /// 追加是组合文档的增量写入边界：只物化新增片段，并通过投影 Buffer 的尾部编辑提交文本，不重建已有映射、源订阅或整份投影。
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
                        this.sync_working_source_config(observed.entity_id(), cx);
                        cx.emit(MultiBufferEvent::MetadataChanged);
                        cx.notify();
                    }
                })
            })
            .collect::<Vec<_>>();

        let first_new_source = self.state.sources.len();
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

        struct PreparedExcerpt {
            excerpt: MultiBufferExcerpt,
            path: PathBuf,
            source_index: usize,
            source_id: gpui::EntityId,
            text: String,
            start_line: usize,
        }

        let mut prepared = Vec::with_capacity(excerpts.len());
        for excerpt in excerpts {
            let source_id = excerpt.source.entity_id();
            let Some(&source_index) = self.state.source_indices.get(&source_id) else {
                unreachable!("追加片段的源必须已注册");
            };
            let source = &self.state.sources[source_index];
            let path = source
                .entity
                .read(cx)
                .file_path()
                .map_or_else(PathBuf::new, Path::to_path_buf);
            let Ok(text) = source.text.slice_text(excerpt.source_range) else {
                continue;
            };
            let start_line = source
                .text
                .byte_to_line(excerpt.source_range.start())
                .map_or(1, |line| line.get() + 1);
            prepared.push(PreparedExcerpt {
                excerpt,
                path,
                source_index,
                source_id,
                text: text.as_str().to_owned(),
                start_line,
            });
        }
        if prepared.is_empty() {
            return Vec::new();
        }

        let projection_snapshot = self.state.projection.read(cx).text_snapshot(cx);
        let mut output = String::new();
        let existing_output_len = projection_snapshot.len_bytes().get();
        let output_ends_with_newline = existing_output_len > 0
            && projection_snapshot
                .slice_byte_range(
                    ByteOffset::new(existing_output_len - 1),
                    ByteOffset::new(existing_output_len),
                )
                .is_ok_and(|text| text.as_str().ends_with('\n'));
        let mut output_line = projection_snapshot.line_count().saturating_sub(1);
        if existing_output_len > 0 && !output_ends_with_newline {
            output.push('\n');
            output_line += 1;
        }

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
            let output_start = ByteOffset::new(existing_output_len + output.len());
            let output_start_line = output_line;
            output.push_str(&item.text);
            output_line += item.text.bytes().filter(|byte| *byte == b'\n').count();
            if position + 1 < prepared_count && !item.text.ends_with('\n') {
                output.push('\n');
                output_line += 1;
            }
            let output_end = ByteOffset::new(existing_output_len + output.len());
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
            next_mappings.push(ExcerptMapping {
                excerpt_index,
                path: item.path,
                display_path,
                output_range,
                source_range: item.excerpt.source_range,
                output_start_line,
                output_end_line,
                source_start_line: item.start_line,
                source_index: item.source_index,
                source_id: item.source_id,
                editable: item.excerpt.editable,
                starts_new_excerpt: item.excerpt.starts_new_excerpt,
                diff_kind: item.excerpt.diff_kind,
            });
            valid_excerpts.push(item.excerpt);
        }

        let text_buffer = self.state.projection.read(cx).buffer();
        text_buffer.update(cx, |buffer, cx| {
            let edit = Edit::insert(ByteOffset::new(existing_output_len), output)
                .expect("组合文档增量追加编辑必须有效");
            buffer
                .edit([edit], TransactionMetadata::default())
                .expect("组合文档增量追加必须是合法 UTF-8 编辑");
            cx.notify();
        });
        self.state.excerpts.extend(valid_excerpts);
        self.state.mappings.extend(next_mappings);
        self.state
            .match_ranges
            .extend(next_match_ranges.iter().copied());
        cx.notify();
        next_match_ranges
    }

    /// 普通整文件文档的投影沿用源 Buffer 配置。
    ///
    /// 组合文档不伪造一份全局配置；编辑行为按光标所在 source 查询。
    fn sync_working_source_config(&mut self, source_id: gpui::EntityId, cx: &mut Context<Self>) {
        let Some(source) = self
            .working_source
            .as_ref()
            .filter(|source| source.entity_id() == source_id)
        else {
            return;
        };
        let source_buffer = source.read(cx).buffer();
        let config = source_buffer.read(cx).config().clone();
        let projection_buffer = self.state.projection.read(cx).buffer();
        if projection_buffer.read(cx).config() == &config {
            return;
        }
        projection_buffer.update(cx, |buffer, cx| {
            buffer.set_config(config);
            cx.notify();
        });
    }

    fn source_changed(&mut self, source_id: gpui::EntityId, cx: &mut Context<Self>) {
        let patch = self
            .state
            .source_subscriptions
            .iter()
            .find(|state| state.source.entity_id() == source_id)
            .map(|state| state.text.consume());
        if let Some(patch) = patch
            && !patch.is_empty()
        {
            let position_map = patch.position_map();
            self.apply_source_change(source_id, &position_map, Some(&patch), None, cx);
            // 工作区源被外部编辑：hunk 显示坐标随文本位置推进（组合编辑已在 edit 内同步映射）。
            let mut diff_changed = false;
            if let Some(new_version) = patch.new_version() {
                diff_changed =
                    self.map_diff_hunks_through_edit(source_id, &position_map, new_version, cx);
            }
            if diff_changed {
                self.rebuild_diff_projection(cx);
            }
        } else {
            // 组合编辑已同步消费文本补丁后，LanguageBuffer 仍可能在本轮安装更新的语法快照。
            // 仅刷新源派生状态，不能退化为一次整体重载。
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
        if let Some(excerpt_source) = self
            .state
            .sources
            .iter_mut()
            .find(|source| source.entity.entity_id() == source_id)
        {
            excerpt_source.text = source.read(cx).text_snapshot(cx);
            excerpt_source.syntax = source.read(cx).syntax_snapshot();
        }
        self.state.capture_names = rebuild_capture_table(&mut self.state.sources);
    }

    /// 将一个源 Buffer 的版本化编辑投影为受影响 excerpts 的局部编辑。
    ///
    /// `set_excerpts` 只负责 excerpts 结构变更；
    /// 普通文本编辑不能整体重载组合投影，否则所有下游位置状态都会退化为 reset。
    fn apply_source_change(
        &mut self,
        source_id: gpui::EntityId,
        source_position_map: &PositionMap,
        source_patch: Option<&TextChangeBatch>,
        expanded_excerpts: Option<&HashSet<usize>>,
        cx: &mut Context<Self>,
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
        let mappings = self.state.mappings.clone();
        let replacements = if let Some(source_patch) = source_patch
            && self
                .working_source
                .as_ref()
                .is_some_and(|working_source| working_source.entity_id() == source_id)
            && mappings.len() == 1
        {
            source_patch
                .patch()
                .edits()
                .iter()
                .map(|edit| {
                    let replacement = text
                        .slice_text(edit.new_range())
                        .expect("工作区源补丁的新范围必须有效")
                        .as_str()
                        .to_owned();
                    Edit::replace(edit.old_range(), replacement)
                })
                .collect::<Vec<_>>()
        } else {
            mappings
                .iter()
                .filter(|mapping| mapping.source_id == source_id)
                .map(|mapping| {
                    let excerpt = &self.state.excerpts[mapping.excerpt_index];
                    let mut replacement = text
                        .slice_text(excerpt.source_range)
                        .expect("已映射的 excerpt 源范围必须有效")
                        .as_str()
                        .to_owned();
                    if mapping.excerpt_index + 1 < mappings.len() && !replacement.ends_with('\n') {
                        replacement.push('\n');
                    }
                    Edit::replace(mapping.output_range, replacement)
                })
                .collect::<Vec<_>>()
        };
        if replacements.is_empty() {
            return;
        }

        let projection = self.state.projection.read(cx).buffer();
        let outcome = projection.update(cx, |buffer, cx| {
            let outcome = buffer
                .edit(replacements, TransactionMetadata::default())
                .expect("源 Buffer 的合法编辑必须能投影到组合文档");
            cx.notify();
            outcome
        });
        let output_position_map = outcome.event().position_map();
        let output = projection.read(cx).snapshot();
        if let Some(excerpt_source) = self
            .state
            .sources
            .iter_mut()
            .find(|source| source.entity.entity_id() == source_id)
        {
            excerpt_source.text = text;
            excerpt_source.syntax = syntax;
        }
        for mapping in &mut self.state.mappings {
            mapping.output_range = output_position_map
                .map_old_range_with_stickiness(mapping.output_range, Stickiness::Expand)
                .value();
            mapping.source_range = self.state.excerpts[mapping.excerpt_index].source_range;
            mapping.output_start_line = output
                .byte_to_line(mapping.output_range.start())
                .expect("组合 excerpt 起点必须在投影文本内")
                .get();
            mapping.output_end_line = output
                .byte_to_line(mapping.output_range.end())
                .expect("组合 excerpt 终点必须在投影文本内")
                .get();
            let excerpt_source = &self.state.sources[mapping.source_index];
            mapping.source_start_line = excerpt_source
                .text
                .byte_to_line(mapping.source_range.start())
                .expect("excerpt 源起点必须在源文本内")
                .get()
                + 1;
        }
        self.state.capture_names = rebuild_capture_table(&mut self.state.sources);
        self.rebuild_match_ranges();
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
    /// 投影文本只是快照，不是可变的第二份文档。
    /// 同一 excerpt 直接映射；跨 excerpt 替换只在起始 excerpt 插入新文本，
    /// 并删除起始尾段、中间 excerpt 和结束首段。
    pub fn edit(
        &mut self,
        edits: Vec<Edit>,
        metadata: TransactionMetadata,
        cx: &mut Context<Self>,
    ) -> TextResult<PositionMap> {
        if self.read_only {
            return Err(StorageError::ReadOnly.into());
        }

        let global_map = PositionMap::from_edits(&edits);
        let mappings = self.state.mappings.clone();
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
            let source_buffer = source.read(cx).buffer();
            let outcome = Self::update_source_text(
                &source_buffer,
                |buffer| buffer.edit(source_edits, metadata.clone()),
                cx,
            )?;
            source_maps.push((
                source.entity_id(),
                outcome.event().position_map().clone(),
                outcome.event().new_version(),
            ));
        }

        // 组合编辑写回工作区源后，直接将同一份源位置映射投影到受影响 excerpts。
        // 消费对应订阅可避免随后到达的源事件重复投影。
        let mut diff_changed = false;
        for (source_id, position_map, new_version) in &source_maps {
            let source_patch = self
                .state
                .source_subscriptions
                .iter()
                .find(|state| state.source.entity_id() == *source_id)
                .map(|subscription| subscription.text.consume());
            self.apply_source_change(
                *source_id,
                position_map,
                source_patch.as_ref(),
                Some(&edited_excerpts),
                cx,
            );
            diff_changed |=
                self.map_diff_hunks_through_edit(*source_id, position_map, *new_version, cx);
        }
        if diff_changed {
            self.rebuild_diff_projection(cx);
        }
        Ok(global_map)
    }

    /// MultiBuffer 写入源 Buffer 的唯一入口。
    ///
    /// 文本内核只发布版本化变更；LanguageBuffer 负责消费变更并维护语法派生状态。
    /// 因此 MultiBuffer 成功改变源文本后必须在这里唤醒其观察者，普通编辑与历史回放不能各自承担这项跨层协议。
    fn update_source_text<T>(
        source: &Entity<Buffer>,
        update: impl FnOnce(&mut Buffer) -> TextResult<T>,
        cx: &mut Context<Self>,
    ) -> TextResult<T> {
        source.update(cx, |buffer, cx| {
            let version = buffer.version();
            let result = update(buffer)?;
            if buffer.version() != version {
                cx.notify();
            }
            Ok(result)
        })
    }

    fn rebuild_projection(&mut self, cx: &mut Context<Self>) {
        let excerpts = std::mem::take(&mut self.state.excerpts);
        self.set_excerpts(excerpts, cx);
    }

    pub fn start_transaction(&mut self, cx: &mut Context<Self>) -> TextResult<TransactionId> {
        if let Some(source) = &self.working_source {
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
        if let Some(source) = &self.working_source {
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
        if let Some(source) = &self.working_source {
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
        if let Some(source) = self.working_source.clone() {
            let source_buffer = source.read(cx).buffer();
            let projection_buffer = self.state.projection.read(cx).buffer();
            let (projection_subscription, old_version) =
                projection_buffer.update(cx, |buffer, _| (buffer.subscribe(), buffer.version()));
            let outcome = Self::update_source_text(
                &source_buffer,
                |buffer| if redo { buffer.redo() } else { buffer.undo() },
                cx,
            )?;
            let Some(outcome) = outcome else {
                return Ok(None);
            };
            let source_map = PositionMap::from_edits(outcome.delta().edits());
            let source_id = source.entity_id();
            for excerpt in self
                .state
                .excerpts
                .iter_mut()
                .filter(|excerpt| excerpt.source.entity_id() == source_id)
            {
                excerpt.source_range = source_map
                    .map_old_range_with_stickiness(excerpt.source_range, Stickiness::Expand)
                    .value();
                for matched in &mut excerpt.match_ranges {
                    *matched = source_map
                        .map_old_range_with_stickiness(*matched, Stickiness::Never)
                        .value();
                }
            }
            let source_version = source_buffer.read(cx).version();
            let diff_changed =
                self.map_diff_hunks_through_edit(source_id, &source_map, source_version, cx);
            self.rebuild_projection(cx);
            if diff_changed {
                self.rebuild_diff_projection(cx);
            }
            let change = projection_subscription.consume();
            let position_map = change.position_map();
            return Ok(Some(MultiBufferHistoryOutcome {
                transaction_id: outcome.transaction_id(),
                position_map,
                old_version,
                new_version: projection_buffer.read(cx).version(),
            }));
        }
        let (entry, old_version, projection_subscription) = {
            let ExcerptState {
                projection,
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
            let projection_buffer = projection.read(cx).buffer();
            let (subscription, old_version) =
                projection_buffer.update(cx, |buffer, _| (buffer.subscribe(), buffer.version()));
            (entry, old_version, subscription)
        };

        let mut source_maps = Vec::new();
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
            let outcome = Self::update_source_text(
                buffer,
                |buffer| if redo { buffer.redo() } else { buffer.undo() },
                cx,
            )?;
            let Some(outcome) = outcome else {
                return Err(TextError::InvariantViolation {
                    location: "MultiBuffer::replay_history",
                    detail: "底层 Buffer 缺少对应的历史节点".to_string(),
                });
            };
            source_maps.push((
                buffer.entity_id(),
                PositionMap::from_edits(outcome.delta().edits()),
            ));
        }
        for excerpt in self.state.excerpts.iter_mut() {
            let buffer = excerpt.source.read(cx).buffer();
            let Some((_, position_map)) =
                source_maps.iter().find(|(id, _)| *id == buffer.entity_id())
            else {
                continue;
            };
            excerpt.source_range = position_map
                .map_old_range_with_stickiness(excerpt.source_range, Stickiness::Expand)
                .value();
            for matched in &mut excerpt.match_ranges {
                *matched = position_map
                    .map_old_range_with_stickiness(*matched, Stickiness::Never)
                    .value();
            }
        }
        self.rebuild_projection(cx);
        let change = projection_subscription.consume();
        let position_map = change.position_map();
        let new_version = self.text_buffer(cx).read(cx).version();
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

    pub fn snapshot(&self, cx: &App) -> MultiBufferSnapshot {
        let projection = self.state.projection.read(cx);
        let text = projection.text_snapshot(cx);
        let syntax = projection.syntax_snapshot();
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
            text,
            syntax,
            excerpts,
            excerpt_mappings: Arc::from(self.state.mappings.clone()),
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

    /// Editor 布局、命中测试和文本算法使用的当前投影。
    /// 组合文档的修改必须走 `MultiBuffer::edit`，不能直接把本 Buffer 当作第二份可变文档。
    pub fn text_buffer(&self, cx: &App) -> Entity<Buffer> {
        self.state.projection.read(cx).buffer()
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
        self.read_only
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
    /// 组合投影 Buffer 永远不会出现在结果中。
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
        let text_buffer = self.state.projection.read(cx).buffer();
        let subscription = text_buffer.update(cx, |buffer, _| buffer.subscribe());
        let snapshot = self.snapshot(cx);
        (MultiBufferSubscription { text: subscription }, snapshot)
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
        let state = &self.state;
        let (index, mapping) = state.mappings.iter().enumerate().find(|(index, mapping)| {
            offset >= mapping.output_range.start()
                && (offset < mapping.output_range.end()
                    || (*index + 1 == state.mappings.len() && offset == mapping.output_range.end()))
        })?;
        let source_offset = ByteOffset::new(
            (mapping.source_range.start().get()
                + offset
                    .get()
                    .saturating_sub(mapping.output_range.start().get()))
            .min(mapping.source_range.end().get()),
        );
        let following_paths =
            distinct_neighbor_paths(state.mappings[index + 1..].iter(), &mapping.path);
        let preceding_paths =
            distinct_neighbor_paths(state.mappings[..index].iter().rev(), &mapping.path);
        Some(MultiBufferAnchor {
            path: mapping.path.clone(),
            source_id: mapping.source_id,
            source_offset,
            following_paths,
            preceding_paths,
        })
    }

    /// 在当前 excerpts 中解析稳定位置；同一文件仍存在时优先落到最接近的源片段。
    pub fn resolve_anchor(&self, anchor: &MultiBufferAnchor) -> Option<ByteOffset> {
        let state = &self.state;
        if let Some(offset) = nearest_output_offset_for_source(
            &state.mappings,
            &anchor.path,
            Some(anchor.source_id),
            anchor.source_offset,
        ) {
            return Some(offset);
        }
        if let Some(offset) = nearest_output_offset_for_source(
            &state.mappings,
            &anchor.path,
            None,
            anchor.source_offset,
        ) {
            return Some(offset);
        }
        for path in &anchor.following_paths {
            if let Some(mapping) = state.mappings.iter().find(|mapping| &mapping.path == path) {
                return Some(mapping.output_range.start());
            }
        }
        for path in &anchor.preceding_paths {
            if let Some(mapping) = state
                .mappings
                .iter()
                .rev()
                .find(|mapping| &mapping.path == path)
            {
                return Some(mapping.output_range.end());
            }
        }
        None
    }

    /// 把组合文档中的选区映射回同一个源片段；跨片段选区没有单一源位置。
    pub fn location_for_range(&self, range: TextRange) -> Option<ExcerptLocation> {
        let state = &self.state;
        let mapping = state
            .mappings
            .iter()
            .enumerate()
            .find_map(|(index, mapping)| {
                let starts_inside = range.start() >= mapping.output_range.start();
                let ends_inside = range.end() <= mapping.output_range.end();
                let empty_point_inside = !range.is_empty()
                    || range.start() < mapping.output_range.end()
                    || (index + 1 == state.mappings.len()
                        && range.start() == mapping.output_range.end());
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
            path: mapping.path.clone(),
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
        self.rebuild_projection(cx);
    }

    /// `offset` 处所在 excerpt 的源语言名（组合文档按光标所在源文件显示语言）。
    pub fn language_at(&self, offset: ByteOffset, cx: &App) -> Option<&'static str> {
        let mapping = self.mapping_at(offset)?;
        let excerpt = self.state.excerpts.get(mapping.excerpt_index)?;
        excerpt.source.read(cx).language_name()
    }

    /// `offset` 处 source 的 Buffer 配置；无 excerpt 时使用投影自身配置。
    pub fn buffer_config_at(&self, offset: ByteOffset, cx: &App) -> BufferConfig {
        self.mapping_at(offset)
            .and_then(|mapping| self.state.excerpts.get(mapping.excerpt_index))
            .map(|excerpt| excerpt.source.read(cx).buffer())
            .map(|buffer| buffer.read(cx).config().clone())
            .unwrap_or_else(|| {
                self.state
                    .projection
                    .read(cx)
                    .buffer()
                    .read(cx)
                    .config()
                    .clone()
            })
    }

    /// 当前已安装解析对应的折叠范围。
    ///
    /// 只投影完整落在单个 excerpt 内的源折叠范围，避免跨越未展示内容或文件边界生成无效折叠。
    pub fn fold_ranges(&self, cx: &App) -> Arc<[FoldRange]> {
        let mut projected = Vec::new();
        for mapping in &self.state.mappings {
            let source_start = mapping.source_range.start().get();
            let source_end = mapping.source_range.end().get();
            let output_start = mapping.output_range.start().get();
            let output_end = mapping.output_range.end().get();
            let source_folds = self.state.sources[mapping.source_index]
                .entity
                .read(cx)
                .fold_ranges();

            projected.extend(source_folds.iter().filter_map(|fold| {
                if fold.range.start < source_start || fold.range.end > source_end {
                    return None;
                }
                let start = output_start + fold.range.start - source_start;
                let end = output_start + fold.range.end - source_start;
                (start < end && end <= output_end).then_some(FoldRange { range: start..end })
            }));
        }
        projected.sort_unstable_by_key(|fold| (fold.range.start, fold.range.end));
        projected.dedup();
        Arc::from(projected)
    }

    /// 定位组合偏移所属的映射；最后一个映射的结束偏移视为命中（光标位于文档末尾）。
    fn mapping_at(&self, offset: ByteOffset) -> Option<&ExcerptMapping> {
        mapping_index_at(&self.state.mappings, offset).map(|index| &self.state.mappings[index])
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
fn mapping_index_at(mappings: &[ExcerptMapping], offset: ByteOffset) -> Option<usize> {
    mappings
        .iter()
        .enumerate()
        .find_map(|(index, mapping)| {
            let content_end =
                ByteOffset::new(mapping.output_range.start().get() + mapping.source_range.len());
            ((offset >= mapping.output_range.start() && offset < content_end)
                || (mapping.source_range.is_empty() && offset == mapping.output_range.start())
                || (index + 1 == mappings.len() && offset == content_end))
                .then_some(index)
        })
        .or_else(|| {
            mappings
                .iter()
                .position(|mapping| mapping.output_range.start() > offset)
        })
}

fn nearest_output_offset_for_source(
    mappings: &[ExcerptMapping],
    path: &Path,
    source_id: Option<gpui::EntityId>,
    source_offset: ByteOffset,
) -> Option<ByteOffset> {
    mappings
        .iter()
        .filter(|mapping| {
            mapping.path == path && source_id.is_none_or(|source_id| mapping.source_id == source_id)
        })
        .map(|mapping| {
            let clamped = ByteOffset::new(
                source_offset
                    .get()
                    .max(mapping.source_range.start().get())
                    .min(mapping.source_range.end().get()),
            );
            let distance = source_offset.get().abs_diff(clamped.get());
            let output = ByteOffset::new(
                mapping.output_range.start().get()
                    + clamped
                        .get()
                        .saturating_sub(mapping.source_range.start().get()),
            );
            (distance, output)
        })
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, output)| output)
}

fn distinct_neighbor_paths<'a>(
    mappings: impl Iterator<Item = &'a ExcerptMapping>,
    anchor_path: &Path,
) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    mappings
        .filter(|mapping| mapping.path != anchor_path && seen.insert(mapping.path.clone()))
        .map(|mapping| mapping.path.clone())
        .collect()
}

#[cfg(test)]
#[path = "test/multi_buffer_tests.rs"]
mod tests;
