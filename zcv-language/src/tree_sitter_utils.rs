//! tree-sitter 适配工具：池化游标与解析器、文本提供者、树编辑与偏移映射。

use std::any::Any;
use std::ops::ControlFlow;
use std::ops::{Deref, DerefMut, Range};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use tree_sitter::{InputEdit, Parser, Point, QueryCursor};
use zcv_text::{ByteOffset, Snapshot, TextChangeBatch, TextError, TextResult};

use crate::Language;

/// 单次解析允许占用的后台时间片（约 3ms）。
pub(crate) const PARSE_TIME_SLICE: Duration = Duration::from_millis(3);

/// 解析开始前的语言或范围设置失败。
///
/// 与预算用尽不同：这些失败继续重试也不会成功，调用方必须显式终止。
#[derive(Debug)]
pub(crate) enum ParseSetupError {
    /// 该语言没有可解析的语法（纯文本兜底）。
    NoGrammar,
    /// `Parser::set_language` 失败（通常是 grammar ABI 与 tree-sitter 不兼容）。
    Language(tree_sitter::LanguageError),
    /// `Parser::set_included_ranges` 失败（范围重叠或非法）。
    IncludedRanges(tree_sitter::IncludedRangesError),
    /// 注入范围端点无法换算为 tree-sitter 行列坐标。
    IncludedRangePoint(TextError),
}

/// 一次 tree-sitter 解析无法完成的原因。
#[derive(Debug)]
pub(crate) enum ParseError {
    /// 时间片预算用尽：parser 保留断点，调用方应在下一片继续恢复。
    BudgetExhausted,
    /// 协作取消：调用方必须放弃本次解析。
    Cancelled,
    /// 语言或范围设置失败：继续重试不会成功。
    Setup(ParseSetupError),
    /// tree-sitter 在未取消、未超预算的情况下拒绝解析。
    Failed,
}

impl std::fmt::Display for ParseSetupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoGrammar => write!(formatter, "语言没有可解析的语法"),
            Self::Language(error) => write!(formatter, "无法设置解析语言：{error}"),
            Self::IncludedRanges(error) => write!(formatter, "无法设置解析范围：{error}"),
            Self::IncludedRangePoint(error) => write!(formatter, "无法换算解析范围端点：{error}"),
        }
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BudgetExhausted => write!(formatter, "解析时间片预算用尽"),
            Self::Cancelled => write!(formatter, "解析已取消"),
            Self::Setup(error) => write!(formatter, "解析设置失败：{error}"),
            Self::Failed => write!(formatter, "tree-sitter 拒绝解析"),
        }
    }
}

/// 一次语法解析的协作取消标记。
///
/// Tree-sitter 在解析进度回调中读取它；
/// 任务所有者取消旧解析时无需等待整棵树完成。
#[derive(Clone, Debug, Default)]
pub(crate) struct ParseCancellation(Arc<AtomicBool>);

impl ParseCancellation {
    pub(crate) fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// 把值移交给专用后台线程释放，避免主线程释放大语法树卡顿。
pub(crate) fn drop_offloaded<T: Send + 'static>(value: T) {
    static DROP_TX: OnceLock<mpsc::Sender<Box<dyn Any + Send>>> = OnceLock::new();
    let tx = DROP_TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("syntax-drop".into())
            .spawn(move || {
                // 收到的每个值在迭代末尾释放，即从池线程完成 dealloc。
                while rx.recv().is_ok() {}
            })
            .expect("后台释放线程应能创建");
        tx
    });
    // 无界通道：send 永不阻塞，drop 路径只付出一次入队开销。
    let _ = tx.send(Box::new(value));
}

/// 支持分片恢复的树解析器：每个时间片内经 progress callback 检查预算，
/// 预算用尽即中断但保留 parser 状态，下一片从断点恢复。
///
/// ParserHandle 在分片间存活，不归还池（池取用会 reset，清掉 outstanding 状态）。
pub(crate) struct IncrementalParser {
    handle: ParserHandle,
    /// 是否已开始解析（首片传入旧树做增量；恢复片由 outstanding 状态接管，不再传旧树）。
    started: bool,
}

impl IncrementalParser {
    pub(crate) fn new() -> Self {
        Self {
            handle: ParserHandle::new(),
            started: false,
        }
    }

    /// 解析一个时间片。
    ///
    /// 预算用尽返回 `Err(ParseError::BudgetExhausted)`，parser 保留断点，调用方应在下一片继续恢复；
    /// 语言或范围设置失败返回 `Err(ParseError::Setup)`，调用方必须终止而不是重试。
    pub(crate) fn parse_slice(
        &mut self,
        language: &Language,
        snapshot: &Snapshot,
        old_tree: Option<&tree_sitter::Tree>,
        included_range: Option<Range<usize>>,
        cancellation: &ParseCancellation,
        budget: Duration,
    ) -> Result<tree_sitter::Tree, ParseError> {
        if cancellation.is_cancelled() {
            return Err(ParseError::Cancelled);
        }
        let parser = self
            .handle
            .0
            .as_mut()
            .expect("IncrementalParser 始终持有池化 parser");
        let Some(grammar) = language.grammar() else {
            return Err(ParseError::Setup(ParseSetupError::NoGrammar));
        };
        if !self.started {
            // 语言与范围限制只在首片设置：`set_language` 会 reset parser 并清掉 outstanding 状态，恢复片再调用会摧毁断点。
            parser
                .set_language(grammar)
                .map_err(|error| ParseError::Setup(ParseSetupError::Language(error)))?;
            if let Some(range) = included_range {
                let start = point_at(snapshot, ByteOffset::new(range.start)).map_err(|error| {
                    ParseError::Setup(ParseSetupError::IncludedRangePoint(error))
                })?;
                let end = point_at(snapshot, ByteOffset::new(range.end)).map_err(|error| {
                    ParseError::Setup(ParseSetupError::IncludedRangePoint(error))
                })?;
                parser
                    .set_included_ranges(&[tree_sitter::Range {
                        start_byte: range.start,
                        end_byte: range.end,
                        start_point: start,
                        end_point: end,
                    }])
                    .map_err(|error| ParseError::Setup(ParseSetupError::IncludedRanges(error)))?;
            } else {
                // 首片必须显式清空池中解析器可能残留的范围限制。
                parser
                    .set_included_ranges(&[])
                    .map_err(|error| ParseError::Setup(ParseSetupError::IncludedRanges(error)))?;
            }
        }
        let deadline = Instant::now() + budget;
        let mut progress = |_: &tree_sitter::ParseState| {
            if cancellation.is_cancelled() || Instant::now() >= deadline {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        let options = tree_sitter::ParseOptions::new().progress_callback(&mut progress);
        // 首片带旧树（增量）；恢复片无旧树——parser 的 outstanding 状态从断点继续。
        let old = if self.started { None } else { old_tree };
        let tree = parser.parse_with_options(
            &mut |offset, _| chunk_from(snapshot, offset),
            old,
            Some(options),
        );
        self.started = true;
        match tree {
            Some(tree) => Ok(tree),
            None if cancellation.is_cancelled() => Err(ParseError::Cancelled),
            // 进度回调因预算中断：parser 保留断点，调用方继续下一片。
            None => Err(ParseError::BudgetExhausted),
        }
    }
}

/// 池化 tree-sitter 解析器句柄，Drop 时归还。
struct ParserHandle(Option<Parser>);

impl ParserHandle {
    fn new() -> Self {
        let mut parser = parser_pool()
            .lock()
            .expect("Parser 池不应在持锁期间 panic")
            .pop()
            .unwrap_or_default();
        // 清掉上一次可能残留的取消/中断状态，保证池中解析器状态干净。
        parser.reset();
        Self(Some(parser))
    }
}

impl Drop for ParserHandle {
    fn drop(&mut self) {
        if let Some(parser) = self.0.take() {
            parser_pool()
                .lock()
                .expect("Parser 池不应在持锁期间 panic")
                .push(parser);
        }
    }
}

fn parser_pool() -> &'static Mutex<Vec<Parser>> {
    static PARSERS: OnceLock<Mutex<Vec<Parser>>> = OnceLock::new();
    PARSERS.get_or_init(|| Mutex::new(Vec::new()))
}

/// 用池化解析器解析文本（增量复用 `old_tree`；`included_range` 用于注入层解析）。
///
/// 与 `parse_slice` 共用 `ParseError`，但一次性解析没有时间片预算，因此不会返回 `BudgetExhausted`。
pub(crate) fn parse_tree(
    language: &Language,
    snapshot: &Snapshot,
    old_tree: Option<&tree_sitter::Tree>,
    included_range: Option<Range<usize>>,
    cancellation: &ParseCancellation,
) -> Result<tree_sitter::Tree, ParseError> {
    if cancellation.is_cancelled() {
        return Err(ParseError::Cancelled);
    }
    let mut handle = ParserHandle::new();
    let parser = handle.0.as_mut().expect("ParserHandle 始终持有池化 parser");
    let Some(grammar) = language.grammar() else {
        return Err(ParseError::Setup(ParseSetupError::NoGrammar));
    };
    parser
        .set_language(grammar)
        .map_err(|error| ParseError::Setup(ParseSetupError::Language(error)))?;
    if let Some(range) = included_range {
        let start = point_at(snapshot, ByteOffset::new(range.start))
            .map_err(|error| ParseError::Setup(ParseSetupError::IncludedRangePoint(error)))?;
        let end = point_at(snapshot, ByteOffset::new(range.end))
            .map_err(|error| ParseError::Setup(ParseSetupError::IncludedRangePoint(error)))?;
        parser
            .set_included_ranges(&[tree_sitter::Range {
                start_byte: range.start,
                end_byte: range.end,
                start_point: start,
                end_point: end,
            }])
            .map_err(|error| ParseError::Setup(ParseSetupError::IncludedRanges(error)))?;
    } else {
        // 池化后必须显式清空上一次注入解析留下的范围限制，否则主树解析会被裁剪。
        parser
            .set_included_ranges(&[])
            .map_err(|error| ParseError::Setup(ParseSetupError::IncludedRanges(error)))?;
    }
    let mut progress = |_: &tree_sitter::ParseState| {
        if cancellation.is_cancelled() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let options = tree_sitter::ParseOptions::new().progress_callback(&mut progress);
    match parser.parse_with_options(
        &mut |offset, _| chunk_from(snapshot, offset),
        old_tree,
        Some(options),
    ) {
        Some(tree) => Ok(tree),
        None if cancellation.is_cancelled() => Err(ParseError::Cancelled),
        None => Err(ParseError::Failed),
    }
}

/// 池化 tree-sitter 查询游标。
pub(crate) struct QueryCursorHandle(Option<QueryCursor>);

impl QueryCursorHandle {
    pub(crate) fn new() -> Self {
        let mut cursor = query_cursor_pool()
            .lock()
            .expect("QueryCursor 池不应在持锁期间 panic")
            .pop()
            .unwrap_or_default();
        cursor.set_match_limit(64);
        Self(Some(cursor))
    }
}

impl Deref for QueryCursorHandle {
    type Target = QueryCursor;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("QueryCursorHandle 持有有效 cursor")
    }
}

impl DerefMut for QueryCursorHandle {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().expect("QueryCursorHandle 持有有效 cursor")
    }
}

impl Drop for QueryCursorHandle {
    fn drop(&mut self) {
        if let Some(mut cursor) = self.0.take() {
            // 归还前把范围限制重置为全域，否则下一个借出者在不设置范围时（如注入收集）会被上一次的范围限制误导。
            cursor.set_byte_range(0..usize::MAX);
            cursor.set_point_range(Point::new(0, 0)..Point::new(usize::MAX, usize::MAX));
            cursor.set_containing_byte_range(0..usize::MAX);
            cursor.set_containing_point_range(Point::new(0, 0)..Point::new(usize::MAX, usize::MAX));
            query_cursor_pool()
                .lock()
                .expect("QueryCursor 池不应在持锁期间 panic")
                .push(cursor);
        }
    }
}

fn query_cursor_pool() -> &'static Mutex<Vec<QueryCursor>> {
    static QUERY_CURSORS: OnceLock<Mutex<Vec<QueryCursor>>> = OnceLock::new();
    QUERY_CURSORS.get_or_init(|| Mutex::new(Vec::new()))
}

/// 把 tree-sitter 的文本回调适配到 zcv-text 快照的分块读取。
pub(crate) struct SnapshotTextProvider<'a>(pub(crate) &'a Snapshot);

impl<'a> tree_sitter::TextProvider<&'a [u8]> for SnapshotTextProvider<'a> {
    type I = SnapshotByteChunks<'a>;

    fn text(&mut self, node: tree_sitter::Node) -> Self::I {
        SnapshotByteChunks {
            snapshot: self.0,
            next: node.start_byte(),
            end: node.end_byte(),
        }
    }
}

pub(crate) struct SnapshotByteChunks<'a> {
    snapshot: &'a Snapshot,
    next: usize,
    end: usize,
}

impl<'a> Iterator for SnapshotByteChunks<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.end {
            return None;
        }
        let (chunk, chunk_start) = self
            .snapshot
            .chunk_at_byte(ByteOffset::new(self.next))
            .ok()?;
        let local_start = self.next - chunk_start.get();
        let local_end = (self.end - chunk_start.get()).min(chunk.len());
        let bytes = &chunk.as_bytes()[local_start..local_end];
        self.next += bytes.len();
        Some(bytes)
    }
}

/// 解析回调用的文本分块：从给定偏移返回一个包含该偏移的连续字节切片。
pub(crate) fn chunk_from(snapshot: &Snapshot, offset: usize) -> &[u8] {
    if offset >= snapshot.len_bytes().get() {
        return &[];
    }
    let Ok((chunk, chunk_start)) = snapshot.chunk_at_byte(ByteOffset::new(offset)) else {
        return &[];
    };
    &chunk.as_bytes()[offset - chunk_start.get()..]
}

/// 把字节偏移转换为 tree-sitter 的行列坐标。
pub(crate) fn point_at(snapshot: &Snapshot, offset: ByteOffset) -> TextResult<Point> {
    let (line, column) = snapshot.byte_to_point(offset)?;
    Ok(Point::new(line.get(), column))
}

/// 计算插入文本后的新行列坐标（插入起点 + 文本）。
pub(crate) fn advance_point(start: Point, text: &str) -> Point {
    let mut rows = 0;
    let mut last_line_bytes = 0;
    for part in text.split_inclusive('\n') {
        if part.ends_with('\n') {
            rows += 1;
            last_line_bytes = 0;
        } else {
            last_line_bytes = part.len();
        }
    }
    if rows == 0 {
        Point::new(start.row, start.column + text.len())
    } else {
        Point::new(start.row + rows, last_line_bytes)
    }
}

/// 把一次文本编辑应用到树上，失败（坐标换算出错）时返回 false，调用方应丢弃树。
pub(crate) fn edit_tree(
    tree: &mut tree_sitter::Tree,
    old_snapshot: &Snapshot,
    new_snapshot: &Snapshot,
    changes: &TextChangeBatch,
) -> bool {
    for edit in changes.patch().edits().iter().rev() {
        let old = edit.old_range();
        let new = edit.new_range();
        let (Ok(start_position), Ok(old_end_position), Ok(inserted)) = (
            point_at(old_snapshot, old.start()),
            point_at(old_snapshot, old.end()),
            new_snapshot.slice_text(new),
        ) else {
            return false;
        };
        tree.edit(&InputEdit {
            start_byte: old.start().get(),
            old_end_byte: old.end().get(),
            new_end_byte: old.start().get() + new.len(),
            start_position,
            old_end_position,
            new_end_position: advance_point(start_position, inserted.as_str()),
        });
    }
    true
}

pub(crate) fn ranges_overlap(
    left: &std::ops::Range<usize>,
    right: &std::ops::Range<usize>,
) -> bool {
    left.start < right.end && right.start < left.end
}

pub(crate) fn encloses(outer: &std::ops::Range<usize>, inner: &std::ops::Range<usize>) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

/// 取节点范围内的文本（去掉首尾空白）。
pub(crate) fn node_text(snapshot: &Snapshot, range: std::ops::Range<usize>) -> Option<String> {
    snapshot
        .slice_byte_range(ByteOffset::new(range.start), ByteOffset::new(range.end))
        .ok()
        .map(|text| text.as_str().trim().to_owned())
}
