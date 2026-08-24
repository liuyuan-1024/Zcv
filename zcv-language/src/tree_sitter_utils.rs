//! tree-sitter 适配工具：池化游标与解析器、文本提供者、树编辑与偏移映射。

use std::any::Any;
use std::ops::{Deref, DerefMut, Range};
use std::sync::{Mutex, OnceLock, mpsc};
use std::thread;

use tree_sitter::{InputEdit, Parser, Point, QueryCursor};
use zcv_text::{ByteOffset, Snapshot};

use crate::Language;

/// 把值移交给专用后台线程释放，避免主线程释放大语法树卡顿（对齐 Zed `SyntaxSnapshot::drop`）。
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

/// 池化 tree-sitter 解析器句柄，Drop 时归还（对齐 Zed `with_parser`）。
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
pub(crate) fn parse_tree(
    language: &Language,
    snapshot: &Snapshot,
    old_tree: Option<&tree_sitter::Tree>,
    included_range: Option<Range<usize>>,
) -> Option<tree_sitter::Tree> {
    let mut handle = ParserHandle::new();
    let parser = handle.0.as_mut()?;
    parser.set_language(language.grammar()).ok()?;
    if let Some(range) = included_range {
        let start = point_at(snapshot, ByteOffset::new(range.start)).ok()?;
        let end = point_at(snapshot, ByteOffset::new(range.end)).ok()?;
        parser
            .set_included_ranges(&[tree_sitter::Range {
                start_byte: range.start,
                end_byte: range.end,
                start_point: start,
                end_point: end,
            }])
            .ok()?;
    } else {
        // 池化后必须显式清空上一次注入解析留下的范围限制，否则主树解析会被裁剪。
        parser.set_included_ranges(&[]).ok()?;
    }
    parser.parse_with_options(
        &mut |offset, _| chunk_from(snapshot, offset),
        old_tree,
        None,
    )
}

/// 池化 tree-sitter 查询游标（对齐 Zed `QUERY_CURSORS`）。
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
            // 归还前把范围限制重置为全域（对齐 Zed），否则下一个借出者在不设置范围时（如注入收集）会被上一次的范围限制误导。
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

/// 把 tree-sitter 的文本回调适配到引擎快照的分块读取。
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
pub(crate) fn point_at(snapshot: &Snapshot, offset: ByteOffset) -> zcv_text::TextResult<Point> {
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
    changes: &zcv_text::TextChangeBatch,
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

/// 把范围两端映射过编辑，得到编辑后的新范围。
pub(crate) fn map_range_through_changes(
    range: std::ops::Range<usize>,
    changes: &zcv_text::TextChangeBatch,
) -> std::ops::Range<usize> {
    map_offset(range.start, true, changes)..map_offset(range.end, false, changes)
}

fn map_offset(offset: usize, before: bool, changes: &zcv_text::TextChangeBatch) -> usize {
    let mut delta = 0isize;
    for edit in changes.patch().edits() {
        let old = edit.old_range();
        let new = edit.new_range();
        if offset < old.start().get() || (before && offset == old.start().get()) {
            break;
        }
        if offset <= old.end().get() {
            return if before {
                new.start().get()
            } else {
                new.end().get()
            };
        }
        delta += new.len() as isize - old.len() as isize;
    }
    offset.saturating_add_signed(delta)
}

pub(crate) fn ranges_overlap(
    left: &std::ops::Range<usize>,
    right: &std::ops::Range<usize>,
) -> bool {
    left.start < right.end && right.start < left.end
}

/// 查询范围与层范围相交（空查询视为一个点，落在层内即可）。
pub(crate) fn range_touches(
    layer: &std::ops::Range<usize>,
    query: &std::ops::Range<usize>,
) -> bool {
    if query.is_empty() {
        layer.start <= query.start && query.start < layer.end
    } else {
        ranges_overlap(layer, query)
    }
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
