//! 单个文件的版本化 diff 状态实体。
//!
//! `BufferDiff` 是单个文件 diff 结果的权威状态：base/index/working 来源、版本绑定的 `BufferDiffSnapshot`、pending 操作与 `DiffOperations` 都由它持有。
//! 它不订阅 working buffer，也不决定何时重算：宿主在源文本变化时调用 [`BufferDiff::recompute`]，本层只负责后台计算、版本门控与结果发布。
//! hunk 的暂存语义统一相对 index 参照判定，所有视图共用同一套；展开/折叠、显示路径与上下文裁剪由 `MultiBuffer` 的 diff 投影持有。

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter};
use imara_diff::{Algorithm, Diff, InternedInput};
use zcv_git::DiffHunkKind;
use zcv_language::LanguageBuffer;
use zcv_text::{Anchor, BufferConfig, BufferVersion, ByteOffset, Line, Snapshot, TextRange};

use crate::word_diff::{MAX_WORD_DIFF_BYTES, MAX_WORD_DIFF_LINES, word_diff_ranges};

/// BufferDiff 变更事件。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffRefresh {
    /// 只更新 hunk 状态，保留当前组合文档投影。
    PreserveProjection,
    /// hunk 结果变化后重建组合文档投影。
    RebuildProjection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferDiffEvent {
    /// diff 结果或 pending 状态变化；`refresh` 决定订阅方是否重建组合投影。
    DiffChanged { refresh: DiffRefresh },
}

/// 单个文件的 diff 创建输入。
///
/// 只包含 diff 状态（working、base 与操作）；显示配置由注入项 `DiffFile` 提供。
#[derive(Clone)]
pub struct BufferDiffInput {
    /// 新侧源（工作区文件或修订文本的语言 Buffer 实体）。
    pub working: Entity<LanguageBuffer>,
    /// 新侧源文件路径（绝对；hunk 操作与导航定位用）。
    pub path: PathBuf,
    /// 旧侧（base 修订）全文；None 表示没有旧侧（如整体新增文件）。
    pub base_text: Option<Arc<str>>,
    /// index 参照全文；hunk 的暂存语义统一相对它判定。
    ///
    /// - 未提交视图（如普通编辑器 gutter）：HEAD 为 base、工作区为 working，真实 index 用于逐 hunk 判定；
    /// - 已暂存视图：working 本身就是 index，分类自然得到全部 Staged；
    /// - 未暂存视图：base 本身就是 index，分类自然得到全部 Unstaged；
    /// - None：index 尚未加载或无暂存语境，暂按 NoStaging（实心）渲染。
    pub index_text: Option<Arc<str>>,
    /// 由宿主注入的 diff 操作实现；无操作能力（普通编辑器 gutter）时为 None。
    pub operations: Option<Arc<dyn DiffOperations>>,
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
    /// 相对 index 参照的暂存语义；
    /// 由 snapshot 统一算好，显示层不再二次判定。
    pub staging: DiffHunkStaging,
    /// 新侧词级变化片段（working 锚点）；无词级结果时为空。
    pub buffer_word_diffs: Vec<Range<Anchor>>,
    /// 旧侧词级变化片段（相对 `diff_base_byte_range.start` 的字节偏移）。
    pub base_word_diffs: Vec<Range<usize>>,
}

/// pending 操作希望在 diff 结果中表达的效果。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

/// hunk 相对 index 参照的暂存语义；所有视图共用同一套（包括普通编辑器 gutter）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiffHunkStaging {
    /// 主 hunk 与 index→working 完全一致：内容尚未进入 index。
    Unstaged,
    /// 与 index→working 部分重叠：只有一部分进入了 index。
    PartiallyStaged,
    /// index→working 无重叠：内容已在 index 中。
    Staged,
    /// 没有 index 参照（index 尚未加载或无暂存语境），不参与暂存渲染。
    NoStaging,
}

/// 给主 hunks 标上相对 index 参照的暂存语义。
///
/// index 参照为 None 时全部 NoStaging；否则逐个与 index→working hunks 比对。
fn classify_staging(hunks: &mut [DiffHunk], index_hunks: Option<&[DiffHunk]>) {
    match index_hunks {
        None => {
            for hunk in hunks {
                hunk.staging = DiffHunkStaging::NoStaging;
            }
        }
        Some(index_hunks) => {
            for hunk in hunks {
                hunk.staging = staging_against(hunk, index_hunks);
            }
        }
    }
}

/// 单个主 hunk 相对 index→working hunks 的暂存语义。
///
/// 范围完全一致即完全未暂存；有交集但不等即部分暂存；完全不相交即已暂存。
/// 先找完全一致的匹配，避免被更早出现的部分重叠 hunk 短路。
fn staging_against(hunk: &DiffHunk, index_hunks: &[DiffHunk]) -> DiffHunkStaging {
    let range = hunk.buffer_range.start.offset().get()..hunk.buffer_range.end.offset().get();
    let mut partial = false;
    for index_hunk in index_hunks {
        // 空 working 且空 base 的 index hunk 不表达任何改动，忽略。
        let index_buffer_empty =
            index_hunk.buffer_range.start.offset() == index_hunk.buffer_range.end.offset();
        if index_buffer_empty && index_hunk.diff_base_byte_range.is_empty() {
            continue;
        }
        let index_range = index_hunk.buffer_range.start.offset().get()
            ..index_hunk.buffer_range.end.offset().get();
        if index_range == range {
            return DiffHunkStaging::Unstaged;
        }
        let overlaps = if range.is_empty() || index_range.is_empty() {
            range.start == index_range.start
        } else {
            range.start < index_range.end && index_range.start < range.end
        };
        if overlaps {
            partial = true;
        }
    }
    if partial {
        DiffHunkStaging::PartiallyStaged
    } else {
        DiffHunkStaging::Staged
    }
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

    /// pending 抑制后应当显示的 hunks（已带暂存语义）。
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
    index_text: Option<Arc<str>>,
    path: PathBuf,
    snapshot: BufferDiffSnapshot,
    operations: Option<Arc<dyn DiffOperations>>,
    /// diff 结果或 pending 的单调版本；显示层据此判断是否需要重新物化。
    revision: u64,
    /// 最近一次已完成计算对应的 working 版本；None 表示初始计算尚未返回。
    calculated_working_version: Option<BufferVersion>,
}

impl EventEmitter<BufferDiffEvent> for BufferDiff {}

impl BufferDiff {
    /// 依据 base/working 快照建立 diff 状态。
    pub fn new(input: BufferDiffInput, cx: &mut Context<Self>) -> Self {
        let working_text = input.working.read(cx).text_snapshot(cx);
        let snapshot = BufferDiffSnapshot {
            working_version: working_text.version(),
            hunks: Vec::new(),
            pending_hunks: Vec::new(),
        };
        let base_source = input.base_text.as_ref().map(|text| {
            let buffer = zcv_text::Buffer::from_text(text.to_string(), BufferConfig::default())
                .expect("base 修订文本必须能创建 Buffer");
            let buffer = cx.new(|_| buffer);
            // 旧侧源的文件路径必须与工作区源一致（绝对），excerpt 定位与导航按源路径匹配。
            cx.new(|cx| LanguageBuffer::new(buffer, Some(input.path.clone()), cx))
        });
        let mut this = Self {
            working: input.working,
            base_source,
            base_text: input.base_text,
            index_text: input.index_text,
            path: input.path,
            snapshot,
            operations: input.operations,
            revision: 0,
            calculated_working_version: None,
        };
        this.recompute(cx);
        this
    }

    /// 捕获当前 working 快照，在后台计算 diff。
    ///
    /// 由宿主在创建后与 working 文本变化时调用；本实体不订阅 working buffer。
    /// 结果回到前台后必须再次比对版本，避免较早任务覆盖后续编辑的 hunk。
    pub fn recompute(&mut self, cx: &mut Context<Self>) {
        self.recompute_with_refresh(DiffRefresh::RebuildProjection, cx);
    }

    /// 按指定投影策略异步重算当前 working 快照。
    pub fn recompute_with_refresh(&mut self, refresh: DiffRefresh, cx: &mut Context<Self>) {
        let working = self.working.read(cx).text_snapshot(cx);
        let working_version = working.version();
        let base_text = self.base_text.clone();
        let index_text = self.index_text.clone();
        let background = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let hunks = background
                .spawn(async move {
                    let mut hunks = compute_hunks(base_text.as_deref(), &working);
                    let index_hunks = index_reference_hunks(
                        base_text.as_deref(),
                        index_text.as_deref(),
                        &working,
                        &hunks,
                    );
                    classify_staging(&mut hunks, index_hunks.as_deref());
                    hunks
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.apply_recomputed_hunks(working_version, hunks, refresh, cx);
            });
        })
        .detach();
    }

    /// 接受仍对应当前 working 版本的后台结果。
    ///
    /// 只有 hunk 几何真正变化时才替换快照、清除 pending 并发出 BufferDiffEvent::DiffChanged；
    /// 行内文本修改等不改变 hunk 定位的编辑不做整体重建。
    fn apply_recomputed_hunks(
        &mut self,
        working_version: BufferVersion,
        hunks: Vec<DiffHunk>,
        refresh: DiffRefresh,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.working.read(cx).text_snapshot(cx).version() != working_version {
            // 结果对应的 working 版本已过期：立即按当前版本补算。
            // 否则若期间没有新的源事件（例如订阅尚未建立），diff 会永久停留在未计算状态，而显示层要求所有 diff 已计算，整份文档的 git 高亮就会消失。
            self.recompute_with_refresh(refresh, cx);
            return false;
        }
        let calculation_was_pending = self.calculated_working_version != Some(working_version);
        self.calculated_working_version = Some(working_version);
        if !calculation_was_pending && hunks_equivalent(&self.snapshot.hunks, &hunks) {
            return false;
        }
        self.snapshot = BufferDiffSnapshot {
            working_version,
            hunks,
            pending_hunks: Vec::new(),
        };
        self.revision = self.revision.wrapping_add(1).max(1);
        cx.emit(BufferDiffEvent::DiffChanged { refresh });
        true
    }

    /// diff 结果与 pending 的当前版本；变化即表示显示层需要重新物化。
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// 当前 working 版本的后台计算是否已经完成。
    pub fn is_current_version_calculated(&self, cx: &App) -> bool {
        self.calculated_working_version == Some(self.working.read(cx).text_snapshot(cx).version())
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

    /// 文件整体是否为新增：旧侧（base）缺失。
    pub fn is_created(&self) -> bool {
        self.base_text.is_none()
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
        cx.emit(BufferDiffEvent::DiffChanged {
            refresh: DiffRefresh::RebuildProjection,
        });
    }

    /// 清除全部 pending hunks（后台操作完成或失败后由宿主调用）。
    pub fn clear_pending_hunks(&mut self, cx: &mut Context<Self>) {
        if self.snapshot.pending_hunks.is_empty() {
            return;
        }
        self.snapshot.pending_hunks.clear();
        self.revision = self.revision.wrapping_add(1).max(1);
        cx.emit(BufferDiffEvent::DiffChanged {
            refresh: DiffRefresh::RebuildProjection,
        });
    }
}

/// 计算 index 参照（index→working）的 hunks。
///
/// - 无 index 参照 → None；
/// - index 与 working 相同（已暂存视图）→ 空，主 hunk 全部 Staged；
/// - index 与 base 相同（未暂存视图）→ 复用主 hunks，避免重复计算；
/// - 其余（未提交视图）→ 按 (index, working) 计算。
fn index_reference_hunks(
    base_text: Option<&str>,
    index_text: Option<&str>,
    working: &Snapshot,
    main_hunks: &[DiffHunk],
) -> Option<Vec<DiffHunk>> {
    let index = index_text?;
    if index == full_text(working).as_str() {
        return Some(Vec::new());
    }
    if base_text == Some(index) {
        return Some(main_hunks.to_vec());
    }
    Some(compute_hunks(Some(index), working))
}

/// working 快照全文。
fn full_text(working: &Snapshot) -> String {
    working
        .slice_text(
            TextRange::new(ByteOffset::ZERO, working.len_bytes()).expect("全文范围必须有序"),
        )
        .expect("全文范围必须有效")
        .as_str()
        .to_owned()
}

/// 从明确的一对文本快照导出 anchor hunk；暂存语义由调用方随后标注。
///
///
/// 返回的 `buffer_range` 归属于 working，`diff_base_byte_range` 归属于 base_text；
/// 二者共同构成后台执行所需的确定编辑依据。暂存语义初值为 NoStaging，由调用方随后标注。
pub(crate) fn compute_hunks(base_text: Option<&str>, working: &Snapshot) -> Vec<DiffHunk> {
    let version = working.version();
    // base 不存在即整份工作区文本为新增（新建文件）。
    let Some(base_text) = base_text else {
        return vec![DiffHunk {
            buffer_range: full_buffer_range(working, version),
            diff_base_byte_range: 0..0,
            kind: DiffHunkKind::Added,
            staging: DiffHunkStaging::NoStaging,
            buffer_word_diffs: Vec::new(),
            base_word_diffs: Vec::new(),
        }];
    };
    let working_text = working
        .slice_text(
            TextRange::new(ByteOffset::ZERO, working.len_bytes()).expect("工作区全文范围必须有序"),
        )
        .expect("工作区全文范围必须有效");
    let working_str = working_text.as_str();
    let input = InternedInput::new(base_text, working_str);
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
            let base_line_count = hunk.before.end - hunk.before.start;
            let new_line_count = hunk.after.end - hunk.after.start;
            let working_byte_range = line_start_or_end(working, hunk.after.start as usize).get()
                ..line_start_or_end(working, hunk.after.end as usize).get();
            let (base_word_diffs, buffer_word_diffs) = if word_diff_comparable(
                kind,
                base_line_count,
                new_line_count,
                diff_base_byte_range.len(),
            ) && working_byte_range.len()
                <= MAX_WORD_DIFF_BYTES
            {
                word_diff_anchors(
                    &base_text[diff_base_byte_range.clone()],
                    &working_str[working_byte_range.clone()],
                    working_byte_range.start,
                    version,
                )
            } else {
                (Vec::new(), Vec::new())
            };
            DiffHunk {
                buffer_range,
                diff_base_byte_range,
                kind,
                staging: DiffHunkStaging::NoStaging,
                buffer_word_diffs,
                base_word_diffs,
            }
        })
        .collect()
}

/// 词级 diff 只对规模受限、行数相同的修改块有意义；其余退回整行级别。
fn word_diff_comparable(
    kind: DiffHunkKind,
    base_line_count: u32,
    new_line_count: u32,
    base_bytes: usize,
) -> bool {
    kind == DiffHunkKind::Modified
        && base_line_count == new_line_count
        && base_line_count > 0
        && base_line_count as usize <= MAX_WORD_DIFF_LINES
        && base_bytes <= MAX_WORD_DIFF_BYTES
}

/// 对一对可比文本片段做词级 diff，并把新侧范围换算为 working 锚点。
///
/// `working_offset` 是 working 片段在工作区全文中的起始字节，锚点由此定位。
fn word_diff_anchors(
    base_snippet: &str,
    working_snippet: &str,
    working_offset: usize,
    version: BufferVersion,
) -> (Vec<Range<usize>>, Vec<Range<Anchor>>) {
    let (base_word_diffs, buffer_word_diffs) = word_diff_ranges(base_snippet, working_snippet);
    let buffer_word_diffs = buffer_word_diffs
        .into_iter()
        .map(|range| {
            Anchor::range_inside(
                version,
                TextRange::new(
                    ByteOffset::new(working_offset + range.start),
                    ByteOffset::new(working_offset + range.end),
                )
                .expect("词级范围必须正序"),
            )
        })
        .collect();
    (base_word_diffs, buffer_word_diffs)
}

/// 比较两组 hunk 的定位与类型是否等价（忽略锚点携带的版本）。
fn hunks_equivalent(a: &[DiffHunk], b: &[DiffHunk]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.buffer_range.start.offset() == b.buffer_range.start.offset()
                && a.buffer_range.end.offset() == b.buffer_range.end.offset()
                && a.diff_base_byte_range == b.diff_base_byte_range
                && a.kind == b.kind
                && a.staging == b.staging
                && a.base_word_diffs == b.base_word_diffs
                && a.buffer_word_diffs.len() == b.buffer_word_diffs.len()
                && a.buffer_word_diffs
                    .iter()
                    .zip(&b.buffer_word_diffs)
                    .all(|(a, b)| {
                        a.start.offset() == b.start.offset() && a.end.offset() == b.end.offset()
                    })
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
