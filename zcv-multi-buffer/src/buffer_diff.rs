//! 单个文件的版本化 diff 状态实体。
//!
//! `BufferDiff` 是 diff 状态与操作能力的唯一拥有者：
//! base/working buffer、当前版本、版本绑定的 `BufferDiffSnapshot`、pending 操作与 `DiffOperations` 都由它持有。
//! 它自行观察 working buffer，源文本变化时重算并发出事件；显示层只订阅结果并物化。
//! 展开/折叠、显示路径与上下文裁剪等显示状态不属于本层，由 `MultiBuffer` 的 diff 投影持有。

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Subscription};
use imara_diff::{Algorithm, Diff, InternedInput};
use zcv_git::DiffHunkKind;
use zcv_language::{LanguageBuffer, LanguageBufferEvent};
use zcv_text::{Anchor, BufferConfig, BufferVersion, ByteOffset, Line, Snapshot, TextRange};

/// BufferDiff 变更事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BufferDiffEvent {
    /// diff 结果或 pending 状态变化；订阅方应重新物化显示。
    DiffChanged,
}

/// 一个文件的 diff 注入输入。
///
/// diff 状态（working、base、版本与操作）与显示配置（display_path、context_lines、show_file_header）一起由宿主提供，但只有前者进入 `BufferDiff`。
pub struct BufferDiffInput {
    /// 新侧源（工作区文件或修订文本的语言 Buffer 实体）。
    pub working: Entity<LanguageBuffer>,
    /// 新侧源文件路径（绝对；hunk 操作与导航定位用）。
    pub path: PathBuf,
    /// 旧侧（base 修订）全文；None 表示没有旧侧（如整体新增文件）。
    pub base_text: Option<Arc<str>>,
    /// 该文件整体为新增（无派生 hunk 时整个文件作为 Added 显示）。
    pub is_created: bool,
    /// 由宿主注入的 diff 操作实现；无操作能力（普通编辑器 gutter）时为 None。
    pub operations: Option<Arc<dyn DiffOperations>>,
    /// 组合文档中的显示路径（文件标题与导航定位）。
    pub display_path: PathBuf,
    /// 显示策略：None 显示整个新侧文件（普通编辑器）；
    /// Some(n) 只显示 hunk 周围 n 行上下文（多文件投影）。
    pub context_lines: Option<usize>,
    /// 该文件的第一个可见片段是否创建文件标题块。
    pub show_file_header: bool,
}

/// 把当前 diff hunk 转换为具体 Git 编辑操作的端口。
///
/// 实现由宿主提供（GitStore），操作结果必须是基于当前 snapshot 已经确定的编辑；
/// 后台执行阶段不再重新执行磁盘 diff 定位变更块。
pub trait DiffOperations: Send + Sync {
    /// 是否支持暂存工作区变更块（unstaged diff）。
    fn supports_staging(&self) -> bool;
    /// 是否支持取消暂存已暂存变更块（staged diff）。
    fn supports_unstaging(&self) -> bool;
    /// 是否支持用 index 内容还原工作区变更块（unstaged diff）。
    fn supports_restore(&self) -> bool;
    /// 暂存工作区变更块（unstaged diff：base=index，working=工作区）。
    fn stage(&self, diff: Entity<BufferDiff>, ranges: Vec<Range<Anchor>>, cx: &mut App);
    /// 取消暂存已暂存变更块（staged diff：base=HEAD，working=index）。
    fn unstage(&self, diff: Entity<BufferDiff>, ranges: Vec<Range<Anchor>>, cx: &mut App);
    /// 用 index 内容还原工作区变更块（unstaged diff）。
    fn restore(&self, diff: Entity<BufferDiff>, ranges: Vec<Range<Anchor>>, cx: &mut App);
}

/// 一个 hunk 在 working buffer 中的锚点定位。
///
/// hunk 的身份与定位都由 `buffer_range` 与 `diff_base_byte_range` 表达：
/// 前者随 buffer 编辑推进，后者是旧侧文本中的字节范围，直接用于生成确定的编辑。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffHunk {
    /// working buffer 中的源文本锚点范围（Deleted 时为空范围，锚定删除点）。
    pub buffer_range: Range<Anchor>,
    /// base 文本中的字节范围。
    pub diff_base_byte_range: Range<usize>,
    pub kind: DiffHunkKind,
}

/// pending 操作希望在 diff 结果中表达的效果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingSense {
    /// 抑制该 hunk（apply/reject 后立即从当前 diff 中消失）。
    Suppress,
}

/// 尚未完成后台操作的 optimistic hunk。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingHunk {
    pub buffer_range: Range<Anchor>,
    pub diff_base_byte_range: Range<usize>,
    pub kind: DiffHunkKind,
    /// 发起操作时的 working buffer 版本；
    /// 旧版本 pending 不得抑制新版本 hunk。
    pub buffer_version: BufferVersion,
    pub sense: PendingSense,
}

impl PendingHunk {
    pub fn suppress(hunk: &DiffHunk, buffer_version: BufferVersion) -> Self {
        Self {
            buffer_range: hunk.buffer_range.clone(),
            diff_base_byte_range: hunk.diff_base_byte_range.clone(),
            kind: hunk.kind,
            buffer_version,
            sense: PendingSense::Suppress,
        }
    }
}

/// 某个明确版本下的不可变 diff 结果。
///
/// 只表达 diff 本身：buffer 版本、anchor hunk 与 pending 的 optimistic 结果；
/// 展开/折叠、显示坐标与上下文裁剪由显示层派生。
#[derive(Clone)]
pub struct BufferDiffSnapshot {
    working_version: BufferVersion,
    hunks: Vec<DiffHunk>,
    pending_hunks: Vec<PendingHunk>,
}

impl BufferDiffSnapshot {
    /// 当前 diff 对应的 working buffer 版本。
    pub fn working_version(&self) -> BufferVersion {
        self.working_version
    }

    /// 原始 hunks（忽略 pending 抑制）；生成编辑与跨 diff 关联时使用。
    pub fn hunks(&self) -> &[DiffHunk] {
        &self.hunks
    }

    /// pending 抑制后应当显示的 hunks。
    pub fn visible_hunks(&self) -> Vec<DiffHunk> {
        self.hunks
            .iter()
            .filter(|hunk| !self.is_suppressed(hunk))
            .cloned()
            .collect()
    }

    pub fn pending_hunks(&self) -> &[PendingHunk] {
        &self.pending_hunks
    }

    fn is_suppressed(&self, hunk: &DiffHunk) -> bool {
        self.pending_hunks.iter().any(|pending| {
            pending.sense == PendingSense::Suppress
                && pending.buffer_version == hunk.buffer_range.start.version()
                && pending.buffer_range.start.offset() == hunk.buffer_range.start.offset()
                && pending.diff_base_byte_range == hunk.diff_base_byte_range
        })
    }
}

/// 单个文件的 diff 状态实体。
pub struct BufferDiff {
    working: Entity<LanguageBuffer>,
    base_source: Option<Entity<LanguageBuffer>>,
    base_text: Option<Arc<str>>,
    path: PathBuf,
    is_created: bool,
    snapshot: BufferDiffSnapshot,
    operations: Option<Arc<dyn DiffOperations>>,
    /// diff 结果或 pending 的单调版本；显示层据此判断是否需要重新物化。
    revision: u64,
    /// 观察 working buffer：源文本变化时由本实体自行重算并发出事件。
    _working_subscription: Subscription,
}

impl EventEmitter<BufferDiffEvent> for BufferDiff {}

impl BufferDiff {
    /// 依据 base/working 快照建立 diff 状态。
    pub fn new(input: BufferDiffInput, cx: &mut Context<Self>) -> Self {
        let working_text = input.working.read(cx).text_snapshot(cx);
        let hunks = compute_hunks(input.base_text.as_deref(), &working_text, input.is_created);
        let snapshot = BufferDiffSnapshot {
            working_version: working_text.version(),
            hunks,
            pending_hunks: Vec::new(),
        };
        let base_source = input.base_text.as_ref().map(|text| {
            let buffer = zcv_text::Buffer::from_text(text.to_string(), BufferConfig::default())
                .expect("base 修订文本必须能创建 Buffer");
            let buffer = cx.new(|_| buffer);
            // 旧侧源的文件路径必须与工作区源一致（绝对），excerpt 定位与导航按源路径匹配。
            cx.new(|cx| LanguageBuffer::new(buffer, Some(input.path.clone()), cx))
        });
        // diff 状态的所有者自行响应 working buffer 版本变化，显示层只订阅结果。
        let working_subscription = cx.subscribe(&input.working, |this, _, event, cx| {
            if matches!(event, LanguageBufferEvent::TextChanged) {
                this.recompute(cx);
            }
        });
        Self {
            working: input.working,
            base_source,
            base_text: input.base_text,
            path: input.path,
            is_created: input.is_created,
            snapshot,
            operations: input.operations,
            revision: 0,
            _working_subscription: working_subscription,
        }
    }

    /// 用当前 working 快照重算 diff 结果；
    /// 只由本实体对 working buffer 的订阅调用。
    ///
    /// 只有 hunk 几何真正变化时才替换快照、清除 pending 并发出 BufferDiffEvent::DiffChanged；
    /// 行内文本修改等不改变 hunk 定位的编辑不做整体重建。
    fn recompute(&mut self, cx: &mut Context<Self>) -> bool {
        let working_text = self.working.read(cx).text_snapshot(cx);
        let hunks = compute_hunks(self.base_text.as_deref(), &working_text, self.is_created);
        if hunks_equivalent(&self.snapshot.hunks, &hunks) {
            return false;
        }
        self.snapshot = BufferDiffSnapshot {
            working_version: working_text.version(),
            hunks,
            pending_hunks: Vec::new(),
        };
        self.revision = self.revision.wrapping_add(1).max(1);
        cx.emit(BufferDiffEvent::DiffChanged);
        true
    }

    /// diff 结果与 pending 的当前版本；变化即表示显示层需要重新物化。
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn snapshot(&self) -> &BufferDiffSnapshot {
        &self.snapshot
    }

    pub fn working(&self) -> &Entity<LanguageBuffer> {
        &self.working
    }

    pub fn base_source(&self) -> Option<&Entity<LanguageBuffer>> {
        self.base_source.as_ref()
    }

    pub fn base_text(&self) -> Option<Arc<str>> {
        self.base_text.clone()
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn is_created(&self) -> bool {
        self.is_created
    }

    pub fn operations(&self) -> Option<Arc<dyn DiffOperations>> {
        self.operations.clone()
    }

    /// 替换 optimistic pending hunks，并通知显示层重新物化。
    pub fn set_pending_hunks(&mut self, hunks: Vec<PendingHunk>, cx: &mut Context<Self>) {
        if hunks.is_empty() {
            return;
        }
        self.snapshot.pending_hunks = hunks;
        self.revision = self.revision.wrapping_add(1).max(1);
        cx.emit(BufferDiffEvent::DiffChanged);
    }

    /// 清除全部 pending hunks（后台操作完成或失败后由宿主调用）。
    pub fn clear_pending_hunks(&mut self, cx: &mut Context<Self>) {
        if self.snapshot.pending_hunks.is_empty() {
            return;
        }
        self.snapshot.pending_hunks.clear();
        self.revision = self.revision.wrapping_add(1).max(1);
        cx.emit(BufferDiffEvent::DiffChanged);
    }

    /// 更新操作实现；宿主重新注入同一实体时同步。
    pub fn set_operations(&mut self, operations: Option<Arc<dyn DiffOperations>>) {
        self.operations = operations;
    }
}

/// 从明确的一对文本快照导出 anchor hunk。
///
/// 返回的 `buffer_range` 归属于 working，`diff_base_byte_range` 归属于 base_text；
/// 二者共同构成后台执行所需的确定编辑依据。
pub(crate) fn compute_hunks(
    base_text: Option<&str>,
    working: &Snapshot,
    is_created: bool,
) -> Vec<DiffHunk> {
    let version = working.version();
    if is_created {
        return vec![DiffHunk {
            buffer_range: full_buffer_range(working, version),
            diff_base_byte_range: 0..0,
            kind: DiffHunkKind::Added,
        }];
    }
    let Some(base_text) = base_text else {
        return Vec::new();
    };
    let working_text = working
        .slice_text(
            TextRange::new(ByteOffset::ZERO, working.len_bytes()).expect("工作区全文范围必须有序"),
        )
        .expect("工作区全文范围必须有效");
    let input = InternedInput::new(base_text, working_text.as_str());
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    diff.postprocess_lines(&input);
    let old_offsets = line_offsets(base_text);
    diff.hunks()
        .map(|hunk| {
            let diff_base_byte_range =
                old_offsets[hunk.before.start as usize]..old_offsets[hunk.before.end as usize];
            let buffer_range = anchor_line_range(
                working,
                version,
                hunk.after.start as usize..hunk.after.end as usize,
            );
            let kind = if diff_base_byte_range.is_empty() {
                DiffHunkKind::Added
            } else if hunk.after.is_empty() {
                DiffHunkKind::Deleted
            } else {
                DiffHunkKind::Modified
            };
            DiffHunk {
                buffer_range,
                diff_base_byte_range,
                kind,
            }
        })
        .collect()
}

/// 比较两组 hunk 的定位与类型是否等价（忽略锚点携带的版本）。
fn hunks_equivalent(a: &[DiffHunk], b: &[DiffHunk]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.buffer_range.start.offset() == b.buffer_range.start.offset()
                && a.buffer_range.end.offset() == b.buffer_range.end.offset()
                && a.diff_base_byte_range == b.diff_base_byte_range
                && a.kind == b.kind
        })
}

/// 文本每一行起始字节偏移；末尾追加文本总长，便于把行范围右端映射为字节偏移。
fn line_offsets(text: &str) -> Vec<usize> {
    let mut offsets = vec![0];
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        offset += line.len();
        offsets.push(offset);
    }
    offsets
}

/// 行范围 → 锚点范围（行号右端等于 line_count 时取文本末尾）。
fn anchor_line_range(
    text: &Snapshot,
    version: BufferVersion,
    lines: Range<usize>,
) -> Range<Anchor> {
    let start = line_start_or_end(text, lines.start);
    let end = line_start_or_end(text, lines.end);
    Anchor::range_inside(
        version,
        TextRange::new(start, end).expect("hunk 行范围必须正序"),
    )
}

fn line_start_or_end(text: &Snapshot, line: usize) -> ByteOffset {
    text.line_start_byte(Line::new(line))
        .unwrap_or(text.len_bytes())
}

fn full_buffer_range(text: &Snapshot, version: BufferVersion) -> Range<Anchor> {
    Anchor::range_inside(
        version,
        TextRange::new(ByteOffset::ZERO, text.len_bytes()).expect("全文范围必须有序"),
    )
}
