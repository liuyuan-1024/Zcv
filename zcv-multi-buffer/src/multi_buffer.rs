//! Editor 与具体文本 Buffer 之间的组合文档边界。
//!
//! singleton 文档直接投影一个语言 Buffer；
//! 组合文档按调用方给出的顺序物化多个来源的 excerpts，并保留组合坐标到源文件坐标的映射。
//! Editor 始终只消费本层，不感知来源数量。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Subscription};
use zcv_language::{
    AutoClosePair, FoldRange, HighlightSpan, LanguageBuffer, LanguageBufferEvent, SyntaxSnapshot,
};
use zcv_text::{
    Buffer, BufferConfig, BufferVersion, ByteOffset, Edit, PositionMap, Snapshot, Stickiness,
    TextChangeBatch, TextError, TextRange, TextResult, TextSubscription, TransactionId,
    TransactionMetadata,
};

/// 组合文档中的一个源片段。
#[derive(Clone)]
pub struct MultiBufferExcerpt {
    source: Entity<MultiBuffer>,
    source_range: TextRange,
    match_ranges: Vec<TextRange>,
    display_path: Option<PathBuf>,
}

impl MultiBufferExcerpt {
    pub fn new(
        source: Entity<MultiBuffer>,
        source_range: TextRange,
        match_ranges: Vec<TextRange>,
    ) -> Self {
        Self {
            source,
            source_range,
            match_ranges,
            display_path: None,
        }
    }

    pub fn with_display_path(mut self, path: PathBuf) -> Self {
        self.display_path = Some(path);
        self
    }

    pub fn match_count(&self) -> usize {
        self.match_ranges.len()
    }
}

/// 组合坐标对应的源文件位置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExcerptLocation {
    pub path: PathBuf,
    pub source_range: TextRange,
}

/// 一个源组合文档的去重共享状态：文本、语法与 capture 映射各保存一份，
/// 该源的所有 excerpt 映射只引用 `source_index`，避免同一文件大量搜索片段重复克隆。
#[derive(Clone, Debug)]
struct ExcerptSource {
    /// 源组合文档实体（更新时按 id 定位）。
    entity: Entity<MultiBuffer>,
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

    pub fn source_line_for_output_line(&self, output_line: usize) -> Option<usize> {
        (output_line >= self.output_start_line)
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
    source: Entity<MultiBuffer>,
    text: MultiBufferSubscription,
}

/// MultiBuffer 对消费方公开的文本与语法更新边界。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MultiBufferEvent {
    TextChanged,
    Reparsed,
    MetadataChanged,
}

impl MultiBufferSnapshot {
    pub fn singleton(text: Snapshot, syntax: SyntaxSnapshot) -> Self {
        let capture_names = syntax.capture_names();
        Self {
            text,
            syntax,
            excerpts: Arc::from([]),
            excerpt_mappings: Arc::from([]),
            excerpt_sources: Arc::from([]),
            capture_names,
        }
    }

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

    pub fn is_composite(&self) -> bool {
        !self.excerpts.is_empty()
    }

    pub fn capture_names(&self) -> Arc<[Arc<str>]> {
        Arc::clone(&self.capture_names)
    }

    /// 查询组合坐标中的语法高亮，并把每个源 Buffer 的 capture index 映射到本快照的统一表。
    pub fn highlights(&self, range: std::ops::Range<usize>) -> Vec<HighlightSpan> {
        if self.excerpt_mappings.is_empty() {
            return self.syntax.highlights(range, &self.text);
        }

        let mut spans = Vec::new();
        for excerpt in self.excerpt_mappings.iter() {
            let output_start = excerpt.output_range.start().get();
            let output_end = excerpt.output_range.end().get();
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

    pub fn excerpt_for_output_line(&self, line: usize) -> Option<&ExcerptSnapshot> {
        self.excerpts
            .iter()
            .find(|excerpt| line >= excerpt.output_start_line && line < excerpt.output_end_line)
    }
}

impl From<Snapshot> for MultiBufferSnapshot {
    fn from(text: Snapshot) -> Self {
        let syntax = SyntaxSnapshot::empty(text.version());
        Self::singleton(text, syntax)
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
    match_ranges: Vec<TextRange>,
    capture_names: Arc<[Arc<str>]>,
    next_transaction_id: TransactionId,
    active_transaction: Option<TransactionId>,
    active_source_transactions: Vec<Entity<Buffer>>,
    undo_stack: Vec<CompositeHistoryEntry>,
    redo_stack: Vec<CompositeHistoryEntry>,
}

enum MultiBufferKind {
    Singleton(Entity<LanguageBuffer>),
    Excerpts(Box<ExcerptState>),
}

/// Editor 持有的组合文档模型。
pub struct MultiBuffer {
    kind: MultiBufferKind,
}

impl EventEmitter<MultiBufferEvent> for MultiBuffer {}

impl MultiBuffer {
    pub fn singleton(singleton: Entity<LanguageBuffer>, cx: &mut Context<Self>) -> Self {
        cx.subscribe(&singleton, |_, _, event, cx| {
            cx.emit(match event {
                LanguageBufferEvent::TextChanged => MultiBufferEvent::TextChanged,
                LanguageBufferEvent::Reparsed => MultiBufferEvent::Reparsed,
                LanguageBufferEvent::MetadataChanged => MultiBufferEvent::MetadataChanged,
            });
            cx.notify();
        })
        .detach();
        Self {
            kind: MultiBufferKind::Singleton(singleton),
        }
    }

    /// 创建空的可编辑组合文档；调用方可重复设置 ordered excerpts。
    pub fn empty(cx: &mut Context<Self>) -> Self {
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
        Self {
            kind: MultiBufferKind::Excerpts(Box::new(ExcerptState {
                projection,
                excerpts: Vec::new(),
                source_subscriptions: Vec::new(),
                source_event_subscriptions: Vec::new(),
                mappings: Vec::new(),
                sources: Vec::new(),
                match_ranges: Vec::new(),
                capture_names: Arc::from([]),
                next_transaction_id: TransactionId::INITIAL,
                active_transaction: None,
                active_source_transactions: Vec::new(),
                undo_stack: Vec::new(),
                redo_stack: Vec::new(),
            })),
        }
    }

    /// 以给定顺序重建组合文档。每个片段都保留源文件路径和源坐标映射。
    pub fn set_excerpts(&mut self, excerpts: Vec<MultiBufferExcerpt>, cx: &mut Context<Self>) {
        let mut unique_sources = Vec::<Entity<MultiBuffer>>::new();
        for excerpt in &excerpts {
            if !unique_sources
                .iter()
                .any(|source| source.entity_id() == excerpt.source.entity_id())
            {
                unique_sources.push(excerpt.source.clone());
            }
        }
        let next_source_subscriptions = unique_sources
            .iter()
            .map(|source| SourceSubscription {
                source: source.clone(),
                text: source.update(cx, |source, cx| source.subscribe_and_snapshot(cx).0),
            })
            .collect::<Vec<_>>();
        let next_source_event_subscriptions = unique_sources
            .into_iter()
            .map(|source| {
                let observed = source.clone();
                cx.subscribe(&source, move |this, _, event, cx| match event {
                    MultiBufferEvent::TextChanged => this.source_changed(observed.entity_id(), cx),
                    MultiBufferEvent::Reparsed => this.source_reparsed(observed.entity_id(), cx),
                    MultiBufferEvent::MetadataChanged => {
                        cx.emit(MultiBufferEvent::MetadataChanged);
                        cx.notify();
                    }
                })
            })
            .collect::<Vec<_>>();
        let MultiBufferKind::Excerpts(state) = &mut self.kind else {
            panic!("singleton MultiBuffer 不能改为组合文档");
        };
        let ExcerptState {
            projection,
            excerpts: stored_excerpts,
            source_subscriptions,
            source_event_subscriptions,
            mappings,
            sources,
            match_ranges,
            capture_names: composite_capture_names,
            ..
        } = state.as_mut();

        // 按源去重构建 (text, syntax) 表：同一文件的大量片段共享一份源状态。
        let mut next_sources: Vec<ExcerptSource> = Vec::new();
        let mut output = String::new();
        let mut next_mappings = Vec::with_capacity(excerpts.len());
        let mut next_match_ranges = Vec::new();
        let mut output_line = 0usize;
        let mut valid_excerpts = Vec::with_capacity(excerpts.len());
        for excerpt in excerpts {
            let Some(path) = excerpt.source.read(cx).file_path(cx) else {
                continue;
            };
            let source_index = match next_sources
                .iter()
                .position(|source| source.entity.entity_id() == excerpt.source.entity_id())
            {
                Some(index) => index,
                None => {
                    let source_snapshot = excerpt.source.read(cx).snapshot(cx);
                    next_sources.push(ExcerptSource {
                        entity: excerpt.source.clone(),
                        text: source_snapshot.text().clone(),
                        syntax: source_snapshot.syntax().clone(),
                        capture_map: Arc::from([]),
                    });
                    next_sources.len() - 1
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
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
                output_line += 1;
            }
            let display_path = excerpt.display_path.clone().unwrap_or_else(|| path.clone());
            let output_start = ByteOffset::new(output.len());
            let output_start_line = output_line;
            output.push_str(text.as_str());
            output_line += text.as_str().bytes().filter(|byte| *byte == b'\n').count();
            if !output.ends_with('\n') {
                output.push('\n');
                output_line += 1;
            }
            let output_end = ByteOffset::new(output.len());
            let output_end_line = output_line;
            let output_range =
                TextRange::new(output_start, output_end).expect("组合片段输出范围必须正序");
            next_match_ranges.extend(excerpt.match_ranges.iter().filter_map(|matched| {
                if matched.start() < excerpt.source_range.start()
                    || matched.end() > excerpt.source_range.end()
                {
                    return None;
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
                TextRange::new(ByteOffset::new(start), ByteOffset::new(end)).ok()
            }));
            let excerpt_index = valid_excerpts.len();
            next_mappings.push(ExcerptMapping {
                excerpt_index,
                path,
                display_path,
                output_range,
                source_range: excerpt.source_range,
                output_start_line,
                output_end_line,
                source_start_line: start_line,
                source_index,
            });
            valid_excerpts.push(excerpt);
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
        *match_ranges = next_match_ranges;
        *composite_capture_names = rebuild_capture_table(sources);
        cx.notify();
    }

    fn source_changed(&mut self, source_id: gpui::EntityId, cx: &mut Context<Self>) {
        let patch = match &self.kind {
            MultiBufferKind::Singleton(_) => return,
            MultiBufferKind::Excerpts(state) => state
                .source_subscriptions
                .iter()
                .find(|state| state.source.entity_id() == source_id)
                .map(|state| state.text.consume()),
        };
        if let Some(patch) = patch
            && !patch.is_empty()
        {
            let position_map = PositionMap::from_text_patch(patch.patch());
            if let MultiBufferKind::Excerpts(state) = &mut self.kind {
                for excerpt in state
                    .excerpts
                    .iter_mut()
                    .filter(|excerpt| excerpt.source.entity_id() == source_id)
                {
                    excerpt.source_range = position_map
                        .map_old_range_with_stickiness(excerpt.source_range, Stickiness::Expand)
                        .value();
                    for matched in &mut excerpt.match_ranges {
                        *matched = position_map
                            .map_old_range_with_stickiness(*matched, Stickiness::Never)
                            .value();
                    }
                }
            }
        }
        self.rebuild_projection(cx);
    }

    fn source_reparsed(&mut self, source_id: gpui::EntityId, cx: &mut Context<Self>) {
        let source = match &self.kind {
            MultiBufferKind::Singleton(_) => return,
            MultiBufferKind::Excerpts(state) => state
                .source_subscriptions
                .iter()
                .find(|state| state.source.entity_id() == source_id)
                .map(|state| state.source.clone()),
        };
        let Some(source) = source else {
            return;
        };
        let source_snapshot = source.read(cx).snapshot(cx);
        let MultiBufferKind::Excerpts(state) = &mut self.kind else {
            return;
        };
        // 按源去重：只更新该源共享的一份 (text, syntax)，所有映射自动跟随。
        if let Some(excerpt_source) = state
            .sources
            .iter_mut()
            .find(|source| source.entity.entity_id() == source_id)
        {
            excerpt_source.text = source_snapshot.text().clone();
            excerpt_source.syntax = source_snapshot.syntax().clone();
        }
        state.capture_names = rebuild_capture_table(&mut state.sources);
        cx.emit(MultiBufferEvent::Reparsed);
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.set_excerpts(Vec::new(), cx);
    }

    /// 将 MultiBuffer 坐标中的编辑拆分到各个底层 Buffer。
    ///
    /// 对齐 Zed `MultiBuffer::edit` 的边界：投影文本只是快照，不是可变的第二份文档。
    /// 同一 excerpt 直接映射；跨 excerpt 替换只在起始 excerpt 插入新文本，
    /// 并删除起始尾段、中间 excerpt 和结束首段，与 Zed `convert_edits_to_buffer_edits` 一致。
    pub fn edit(
        &mut self,
        edits: Vec<Edit>,
        metadata: TransactionMetadata,
        cx: &mut Context<Self>,
    ) -> TextResult<PositionMap> {
        if let MultiBufferKind::Singleton(singleton) = &self.kind {
            let outcome = singleton
                .read(cx)
                .buffer()
                .update(cx, |buffer, _| buffer.edit(edits.clone(), metadata))?;
            return Ok(outcome.event().position_map().clone());
        }

        let global_map = PositionMap::from_edits(&edits);
        let (mappings, stored_excerpts) = match &self.kind {
            MultiBufferKind::Excerpts(state) => (state.mappings.clone(), state.excerpts.clone()),
            MultiBufferKind::Singleton(_) => unreachable!(),
        };

        let mut grouped: Vec<(Entity<MultiBuffer>, Vec<Edit>)> = Vec::new();
        let push_source_edit =
            |mapping: &ExcerptMapping,
             source_range: TextRange,
             replacement: String,
             grouped: &mut Vec<(Entity<MultiBuffer>, Vec<Edit>)>| {
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
                .position(|mapping| {
                    let content_end = ByteOffset::new(
                        mapping.output_range.start().get() + mapping.source_range.len(),
                    );
                    range.start() >= mapping.output_range.start() && range.start() <= content_end
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
            let end_index = mappings
                .iter()
                .enumerate()
                .rev()
                .find_map(|(index, mapping)| {
                    let content_end = ByteOffset::new(
                        mapping.output_range.start().get() + mapping.source_range.len(),
                    );
                    (range.end() >= mapping.output_range.start() && range.end() <= content_end)
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
                })?;
            if end_index < start_index {
                return Err(TextError::InvariantViolation {
                    location: "MultiBuffer::edit",
                    detail: "编辑范围必须正序".to_string(),
                });
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
                );
            } else {
                push_source_edit(
                    start_mapping,
                    TextRange::new(source_start, start_mapping.source_range.end())
                        .expect("起始 excerpt 尾段必须有效"),
                    edit.replacement().to_owned(),
                    &mut grouped,
                );
                for mapping in &mappings[start_index + 1..end_index] {
                    push_source_edit(mapping, mapping.source_range, String::new(), &mut grouped);
                }
                push_source_edit(
                    end_mapping,
                    TextRange::new(end_mapping.source_range.start(), source_end)
                        .expect("结束 excerpt 首段必须有效"),
                    String::new(),
                    &mut grouped,
                );
            }
        }

        let mut source_maps = Vec::with_capacity(grouped.len());
        for (source, source_edits) in grouped {
            let source_buffer =
                source
                    .read(cx)
                    .as_singleton(cx)
                    .ok_or_else(|| TextError::InvariantViolation {
                        location: "MultiBuffer::edit",
                        detail: "excerpt 来源必须是 singleton MultiBuffer".to_string(),
                    })?;
            let outcome = source_buffer.update(cx, |buffer, cx| -> TextResult<_> {
                let outcome = buffer.edit(source_edits, metadata.clone())?;
                cx.notify();
                Ok(outcome)
            })?;
            source_maps.push((source.entity_id(), outcome.event().position_map().clone()));
        }

        if let MultiBufferKind::Excerpts(state) = &mut self.kind {
            for excerpt in state.excerpts.iter_mut() {
                let Some((_, position_map)) = source_maps
                    .iter()
                    .find(|(id, _)| *id == excerpt.source.entity_id())
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
        }
        self.rebuild_projection(cx);
        Ok(global_map)
    }

    fn rebuild_projection(&mut self, cx: &mut Context<Self>) {
        let excerpts = match &mut self.kind {
            MultiBufferKind::Singleton(_) => return,
            MultiBufferKind::Excerpts(state) => std::mem::take(&mut state.excerpts),
        };
        self.set_excerpts(excerpts, cx);
    }

    pub fn start_transaction(&mut self, cx: &mut Context<Self>) -> TextResult<TransactionId> {
        match &mut self.kind {
            MultiBufferKind::Singleton(singleton) => singleton
                .read(cx)
                .buffer()
                .update(cx, |buffer, _| buffer.start_transaction())?
                .ok_or_else(|| TextError::InvariantViolation {
                    location: "MultiBuffer::start_transaction",
                    detail: "底层 Buffer 已有活动事务".to_string(),
                }),
            MultiBufferKind::Excerpts(state) => {
                let ExcerptState {
                    excerpts,
                    next_transaction_id,
                    active_transaction,
                    active_source_transactions,
                    ..
                } = state.as_mut();
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
                for excerpt in excerpts.iter() {
                    let Some(buffer) = excerpt.source.read(cx).as_singleton(cx) else {
                        continue;
                    };
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
        }
    }

    pub fn end_transaction(&mut self, cx: &mut Context<Self>) -> Option<TransactionId> {
        match &mut self.kind {
            MultiBufferKind::Singleton(singleton) => singleton
                .read(cx)
                .buffer()
                .update(cx, |buffer, _| buffer.end_transaction())
                .ok()
                .flatten(),
            MultiBufferKind::Excerpts(state) => {
                let ExcerptState {
                    active_transaction,
                    active_source_transactions,
                    undo_stack,
                    redo_stack,
                    ..
                } = state.as_mut();
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
                    None
                } else {
                    undo_stack.push(CompositeHistoryEntry {
                        id,
                        buffers: transactions,
                    });
                    redo_stack.clear();
                    Some(id)
                }
            }
        }
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
        if let MultiBufferKind::Singleton(singleton) = &self.kind {
            let buffer = singleton.read(cx).buffer();
            let old_version = buffer.read(cx).version();
            let outcome = buffer.update(
                cx,
                |buffer, _| {
                    if redo { buffer.redo() } else { buffer.undo() }
                },
            )?;
            let Some(outcome) = outcome else {
                return Ok(None);
            };
            let position_map = PositionMap::from_edits(outcome.delta().edits());
            return Ok(Some(MultiBufferHistoryOutcome {
                transaction_id: outcome.transaction_id(),
                position_map,
                old_version,
                new_version: buffer.read(cx).version(),
            }));
        }

        let (entry, old_version, projection_subscription) = {
            let MultiBufferKind::Excerpts(state) = &mut self.kind else {
                unreachable!()
            };
            let ExcerptState {
                projection,
                undo_stack,
                redo_stack,
                ..
            } = state.as_mut();
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
            let outcome = buffer.update(
                cx,
                |buffer, _| {
                    if redo { buffer.redo() } else { buffer.undo() }
                },
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
        if let MultiBufferKind::Excerpts(state) = &mut self.kind {
            for excerpt in state.excerpts.iter_mut() {
                let Some(buffer) = excerpt.source.read(cx).as_singleton(cx) else {
                    continue;
                };
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
        }
        self.rebuild_projection(cx);
        let change = projection_subscription.consume();
        let position_map = PositionMap::from_text_patch(change.patch());
        let new_version = self.text_buffer(cx).read(cx).version();
        let transaction_id = entry.id;
        if let MultiBufferKind::Excerpts(state) = &mut self.kind {
            if redo {
                state.undo_stack.push(entry);
            } else {
                state.redo_stack.push(entry);
            }
        }
        Ok(Some(MultiBufferHistoryOutcome {
            transaction_id,
            position_map,
            old_version,
            new_version,
        }))
    }

    pub fn snapshot(&self, cx: &App) -> MultiBufferSnapshot {
        let language_buffer = self.language_buffer();
        let language_buffer = language_buffer.read(cx);
        let text = language_buffer.buffer().read(cx).snapshot();
        let syntax = language_buffer.syntax_snapshot();
        match &self.kind {
            MultiBufferKind::Singleton(_) => MultiBufferSnapshot::singleton(text, syntax),
            MultiBufferKind::Excerpts(state) => {
                let excerpts = state
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
                    })
                    .collect::<Arc<[_]>>();
                MultiBufferSnapshot {
                    text,
                    syntax,
                    excerpts,
                    excerpt_mappings: Arc::from(state.mappings.clone()),
                    excerpt_sources: Arc::from(
                        state
                            .sources
                            .iter()
                            .map(|source| ExcerptSourceSnapshot {
                                text: source.text.clone(),
                                syntax: source.syntax.clone(),
                                capture_map: Arc::clone(&source.capture_map),
                            })
                            .collect::<Vec<_>>(),
                    ),
                    capture_names: Arc::clone(&state.capture_names),
                }
            }
        }
    }

    /// Editor 布局、命中测试和文本算法使用的当前投影。
    /// 组合文档的修改必须走 `MultiBuffer::edit`，不能直接把本 Buffer 当作第二份可变文档。
    pub fn text_buffer(&self, cx: &App) -> Entity<Buffer> {
        self.language_buffer().read(cx).buffer()
    }

    /// singleton 对应的底层文本；组合文档返回 None。
    pub fn as_singleton(&self, cx: &App) -> Option<Entity<Buffer>> {
        match &self.kind {
            MultiBufferKind::Singleton(singleton) => Some(singleton.read(cx).buffer()),
            MultiBufferKind::Excerpts(_) => None,
        }
    }

    pub fn is_read_only(&self) -> bool {
        false
    }

    pub fn is_composite(&self) -> bool {
        matches!(self.kind, MultiBufferKind::Excerpts(_))
    }

    pub fn is_dirty(&self, cx: &App) -> bool {
        match &self.kind {
            MultiBufferKind::Singleton(language_buffer) => {
                language_buffer.read(cx).buffer().read(cx).is_dirty()
            }
            MultiBufferKind::Excerpts(state) => state
                .excerpts
                .iter()
                .any(|excerpt| excerpt.source.read(cx).is_dirty(cx)),
        }
    }

    /// 文档实际引用的、可落盘的底层文件 Buffer。
    ///
    /// singleton 返回自身；组合文档递归收集 excerpts 的源 Buffer 并按实体去重。
    /// 组合投影 Buffer 永远不会出现在结果中。
    pub fn file_buffers(&self, cx: &App) -> Vec<(Entity<Buffer>, PathBuf)> {
        match &self.kind {
            MultiBufferKind::Singleton(language_buffer) => {
                let language_buffer = language_buffer.read(cx);
                language_buffer
                    .file_path()
                    .map(|path| vec![(language_buffer.buffer(), path.to_path_buf())])
                    .unwrap_or_default()
            }
            MultiBufferKind::Excerpts(state) => {
                let mut buffers = Vec::<(Entity<Buffer>, PathBuf)>::new();
                for excerpt in &state.excerpts {
                    for (buffer, path) in excerpt.source.read(cx).file_buffers(cx) {
                        if !buffers
                            .iter()
                            .any(|(existing, _)| existing.entity_id() == buffer.entity_id())
                        {
                            buffers.push((buffer, path));
                        }
                    }
                }
                buffers
            }
        }
    }

    pub fn subscribe_and_snapshot(
        &mut self,
        cx: &mut Context<Self>,
    ) -> (MultiBufferSubscription, MultiBufferSnapshot) {
        let language_buffer = self.language_buffer();
        let text_buffer = language_buffer.read(cx).buffer();
        let subscription = text_buffer.update(cx, |buffer, _| buffer.subscribe());
        let snapshot = self.snapshot(cx);
        (MultiBufferSubscription { text: subscription }, snapshot)
    }

    pub fn file_path(&self, cx: &App) -> Option<PathBuf> {
        match &self.kind {
            MultiBufferKind::Singleton(singleton) => {
                singleton.read(cx).file_path().map(Path::to_path_buf)
            }
            MultiBufferKind::Excerpts(_) => None,
        }
    }

    pub fn location_for_offset(&self, offset: ByteOffset) -> Option<ExcerptLocation> {
        let range = TextRange::new(offset, offset).expect("同点组合范围必须有效");
        self.location_for_range(range)
    }

    /// 把组合文档中的选区映射回同一个源片段；跨片段选区没有单一源位置。
    pub fn location_for_range(&self, range: TextRange) -> Option<ExcerptLocation> {
        let MultiBufferKind::Excerpts(state) = &self.kind else {
            return None;
        };
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
        match &self.kind {
            MultiBufferKind::Singleton(_) => &[],
            MultiBufferKind::Excerpts(state) => &state.match_ranges,
        }
    }

    pub fn set_file_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if let MultiBufferKind::Singleton(singleton) = &self.kind {
            singleton.update(cx, |buffer, cx| buffer.set_file_path(path, cx));
        }
    }

    pub fn language_name(&self, cx: &App) -> Option<&'static str> {
        match &self.kind {
            MultiBufferKind::Singleton(singleton) => singleton.read(cx).language_name(),
            MultiBufferKind::Excerpts(_) => None,
        }
    }

    /// 当前已安装解析对应的折叠范围（Arc 共享，O(1) 克隆）。
    ///
    /// singleton 直接返回 LanguageBuffer 的共享缓存（多个 Editor 不重复计算）；
    /// 组合文档的投影没有语言层，不提供折叠。
    pub fn fold_ranges(&self, cx: &App) -> Arc<[FoldRange]> {
        match &self.kind {
            MultiBufferKind::Singleton(singleton) => singleton.read(cx).fold_ranges(),
            MultiBufferKind::Excerpts(_) => Arc::from([]),
        }
    }

    pub fn auto_close_pairs(&self, cx: &App) -> Option<&'static [AutoClosePair]> {
        match &self.kind {
            MultiBufferKind::Singleton(singleton) => {
                Some(singleton.read(cx).language()?.auto_close_pairs())
            }
            MultiBufferKind::Excerpts(_) => None,
        }
    }

    fn language_buffer(&self) -> Entity<LanguageBuffer> {
        match &self.kind {
            MultiBufferKind::Singleton(singleton) => singleton.clone(),
            MultiBufferKind::Excerpts(state) => state.projection.clone(),
        }
    }
}

#[cfg(test)]
#[path = "test/multi_buffer_tests.rs"]
mod tests;
