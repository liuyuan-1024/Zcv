//! 单个文件的版本化 diff 状态实体。
//!
//! `BufferDiff` 是单个文件 diff 结果的权威状态：base/index/working 来源、版本绑定的 `BufferDiffSnapshot`、pending 操作与 `DiffOperations` 都由它持有。
//! 源文本变化与修订输入提交共用唯一计算任务；修订快照和 hunk 结果在版本校验后整体安装，投影层只消费结果。
//! hunk 的暂存语义统一相对 index 参照判定，所有视图共用同一套；展开/折叠与显示路径由 `MultiBuffer` 的 diff 投影持有。

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use futures::join;
use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Subscription, Task};
use imara_diff::{Algorithm, Diff, InternedInput};
use sum_tree::{Item, SumTree};
use zcv_language::{
    EditedLanguageBufferSnapshot, LanguageBuffer, LanguageBufferEvent, LanguageRegistry,
};
use zcv_text::{
    Anchor, Buffer as TextBuffer, BufferConfig, BufferVersion, ByteOffset, Line, Snapshot,
    TextRange, TextResult,
};

use zcv_text::word_diff::{MAX_WORD_DIFF_BYTES, MAX_WORD_DIFF_LINES, word_diff_ranges};

/// hunk 变化类型（判定规则：旧侧空→Added、新侧空→Deleted）。
///
/// 由本层的行级 diff 计算产生，是编辑器、版本控制视图与 gutter 共用的稳定变化类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiffHunkKind {
    /// 旧侧计数为 0（纯新增）。
    Added,
    /// 新旧两侧计数均非 0。
    Modified,
    /// 新侧计数为 0（纯删除）。
    Deleted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BufferDiffEvent {
    /// diff 结果或 pending 状态变化；范围位于当前 working 快照，None 表示本次不做范围同步。
    DiffChanged {
        changed_range: Option<Range<Anchor>>,
    },
}

/// 单个文件的 diff 创建输入。
///
/// 只包含 diff 状态（working、base 与操作）；显示配置由注入项 `DiffFile` 提供。
#[derive(Clone)]
pub struct BufferDiffInput {
    /// 新侧源（工作区文件的语言 Buffer 实体）。
    pub working: Entity<LanguageBuffer>,
    /// 新侧源文件路径（绝对；hunk 操作与导航定位用）。
    pub path: PathBuf,
    /// 旧侧（base 修订）文本；None 表示没有旧侧（如整体新增文件）。
    pub base_text: Option<Arc<str>>,
    /// index 参照文本；hunk 的暂存语义统一相对它判定。
    ///
    /// - 未提交视图（如普通编辑器 gutter）：HEAD 为 base、工作区为 working，真实 index 用于逐 hunk 判定；
    /// - 已暂存视图：working 本身就是 index，分类自然得到全部 Staged；
    /// - 未暂存视图：base 本身就是 index，分类自然得到全部 Unstaged；
    /// - None：index 尚未加载或无暂存语境，暂按 NoStaging（实心）渲染。
    pub index_text: Option<Arc<str>>,
    /// 创建 base/index 语言缓冲所用注册表；由宿主装配层注入。
    pub language_registry: Arc<LanguageRegistry>,
    /// 调用方给出的稳定共享键（例如由 base/index 修订身份派生）；同一键复用同一 diff 实体。
    pub key: u64,
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
    /// working buffer 中的半开源范围；两端均吸附到边界插入之前，Deleted 时为空范围。
    pub buffer_range: Range<Anchor>,
    /// base 文本中的字节范围。
    pub diff_base_byte_range: Range<usize>,
    pub kind: DiffHunkKind,
    /// 相对 index 参照的暂存语义；
    /// 由 snapshot 统一算好，显示层不再二次判定。
    pub staging: DiffHunkStaging,
    /// 新侧词级变化片段（working 锚点），按源顺序排列且互不重叠；无词级结果时为空。
    pub buffer_word_diffs: Vec<Range<Anchor>>,
    /// 旧侧词级变化片段（相对 `diff_base_byte_range.start` 的字节偏移），按源顺序排列且互不重叠。
    pub base_word_diffs: Vec<Range<usize>>,
}

/// 将行级 diff 的字节边界转换为文本行边界。
///
/// 终止换行后的 EOF 对应新增的空编辑器行，但不属于行级 diff 的内容范围；
/// 没有终止换行时，EOF 则是最后一行之后的半开范围端点。
pub fn diff_line_boundary(text: &Snapshot, offset: ByteOffset) -> usize {
    let line_count = text.line_count();
    if offset == text.len_bytes() {
        let last_line = Line::new(line_count.saturating_sub(1));
        if text.line_start_byte(last_line) == Ok(offset) {
            return last_line.get();
        }
        return line_count;
    }
    text.byte_to_line(offset)
        .map_or(line_count, |line| line.get())
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
/// 只表达 diff 本身：anchor hunk 与 pending 的 optimistic 结果；
/// 展开/折叠与显示坐标由显示层派生。
#[derive(Clone)]
pub struct BufferDiffSnapshot {
    hunks: SumTree<DiffHunk>,
    pending_hunks: Vec<PendingHunk>,
}

impl BufferDiffSnapshot {
    /// 原始 hunks（忽略 pending 抑制）；生成编辑与跨 diff 关联时使用。
    pub fn hunks(&self) -> impl Iterator<Item = &DiffHunk> {
        self.hunks.iter()
    }

    pub fn hunk_count(&self) -> usize {
        self.hunks.summary().count
    }

    /// 查询与当前 working 字节范围相交的 hunks。
    ///
    /// 锚点范围保存在树摘要中，查询时解析到同一份 working 快照；树只下探到相交分支。
    pub fn hunks_intersecting_working_range<'a>(
        &'a self,
        range: Range<ByteOffset>,
        working: &'a Snapshot,
    ) -> impl 'a + Iterator<Item = &'a DiffHunk> {
        self.hunks
            .filter::<_, DiffHunkSummary>(working, move |summary| {
                let Some(summary_range) = &summary.range else {
                    return false;
                };
                let Some(summary_start) = summary_range.start.resolve_in(working).ok() else {
                    return true;
                };
                let Some(summary_end) = summary_range.end.resolve_in(working).ok() else {
                    return true;
                };
                summary_start <= range.end && summary_end >= range.start
            })
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

#[derive(Clone, Debug)]
/// SumTree 中 hunk 工作区锚点范围与数量的聚合摘要。
pub struct DiffHunkSummary {
    range: Option<Range<Anchor>>,
    count: usize,
}

impl sum_tree::Summary for DiffHunkSummary {
    type Context<'a> = &'a Snapshot;

    fn zero<'a>(_working: Self::Context<'a>) -> Self {
        Self {
            range: None,
            count: 0,
        }
    }

    fn add_summary<'a>(&mut self, other: &Self, _working: Self::Context<'a>) {
        self.count += other.count;
        match (&mut self.range, &other.range) {
            // Diff hunks 按 working 文档顺序生成，合并后保留首尾锚点即可表示子树覆盖范围。
            (Some(current), Some(other)) => current.end = other.end,
            (None, Some(other)) => self.range = Some(other.clone()),
            (_, None) => {}
        }
    }
}

impl Item for DiffHunk {
    type Summary = DiffHunkSummary;

    fn summary(&self, _working: &Snapshot) -> Self::Summary {
        DiffHunkSummary {
            range: Some(self.buffer_range.clone()),
            count: 1,
        }
    }
}

/// 一次 diff 计算的全部输入版本。
///
/// working、base、index 三者任一前进都会使在途结果过期；
/// 结果安装前必须整体相等，不能只比较 working 文本版本。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DiffInputVersions {
    working: BufferVersion,
    base: Option<BufferVersion>,
    index: Option<BufferVersion>,
}

/// 单个文件的 diff 状态实体。
pub struct BufferDiff {
    working: Entity<LanguageBuffer>,
    base_source: Option<Entity<LanguageBuffer>>,
    index_source: Option<Entity<LanguageBuffer>>,
    path: PathBuf,
    /// 新建 base/index 缓冲用的注册表，与宿主装配层共用同一份。
    language_registry: Arc<LanguageRegistry>,
    snapshot: BufferDiffSnapshot,
    operations: Option<Arc<dyn DiffOperations>>,
    /// diff 结果或 pending 的单调版本；显示层据此判断是否需要重新物化。
    revision: u64,
    /// 最近一次已发布结果的输入；同时作为未变修订语言快照的复用依据。
    calculated_inputs: Option<CalculatedDiffInputs>,
    /// 修订准备和 diff 计算共用此任务；替换即取消，实体销毁时随之取消。
    calculation_task: Option<Task<()>>,
    /// 宿主提交的不可变修订输入；语言缓冲与 hunks 都是它和 working 快照的派生结果。
    revision_texts: RevisionTexts,
    _working_subscription: Subscription,
}

#[derive(Clone, PartialEq, Eq)]
struct RevisionTexts {
    base: Option<Arc<str>>,
    index: Option<Arc<str>>,
}

struct CalculatedDiffInputs {
    versions: DiffInputVersions,
    revisions: RevisionTexts,
}

struct RevisionPreparation {
    source: Entity<LanguageBuffer>,
    edited: Option<Task<TextResult<EditedLanguageBufferSnapshot>>>,
}

impl RevisionPreparation {
    async fn finish(self) -> PreparedRevision {
        let edited = match self.edited {
            Some(task) => Some(task.await.expect("diff 修订文本必须能派生语言快照")),
            None => None,
        };
        PreparedRevision {
            source: self.source,
            edited,
        }
    }
}

struct PreparedRevision {
    source: Entity<LanguageBuffer>,
    edited: Option<EditedLanguageBufferSnapshot>,
}

impl PreparedRevision {
    fn install(self, cx: &mut Context<BufferDiff>) -> Entity<LanguageBuffer> {
        if let Some(edited) = self.edited {
            let source_version = self.source.read(cx).text_snapshot().version();
            // 派生期间主文档已前进：丢弃过期修订，不安装、不改变当前状态。
            if edited.base_version() == source_version {
                self.source.update(cx, |source, cx| {
                    source
                        .fast_forward(edited, cx)
                        .expect("版本比对通过后派生状态必须能安装");
                });
            }
        }
        self.source
    }
}

/// 准备修订派生快照；源编辑时直接复用未变化的修订，不重新物化全文或解析语法。
fn prepare_revision(
    source: Option<Entity<LanguageBuffer>>,
    text: Option<Arc<str>>,
    unchanged: bool,
    path: &std::path::Path,
    registry: &Arc<LanguageRegistry>,
    cx: &mut Context<BufferDiff>,
) -> Option<RevisionPreparation> {
    let text = text?;
    if unchanged {
        return Some(RevisionPreparation {
            source: source.expect("已发布的存在修订必须有语言缓冲"),
            edited: None,
        });
    }
    let source = source.unwrap_or_else(|| empty_revision_buffer(path, registry, cx));
    let background = cx.background_executor().clone();
    let target = source.clone();
    let edited = cx.spawn(async move |_, cx| {
        let text = background.spawn(async move { text.to_string() }).await;
        target
            .update(cx, |source, cx| source.snapshot_with_text(text, cx))
            .await
    });
    Some(RevisionPreparation {
        source,
        edited: Some(edited),
    })
}

/// 由修订文本创建语言缓冲；None 表示该侧不存在。
fn empty_revision_buffer(
    path: &std::path::Path,
    registry: &Arc<LanguageRegistry>,
    cx: &mut Context<BufferDiff>,
) -> Entity<LanguageBuffer> {
    let buffer = TextBuffer::from_text(String::new(), BufferConfig::default())
        .expect("空修订文本必须能创建 Buffer");
    cx.new(|cx| LanguageBuffer::new(buffer, Some(path.to_path_buf()), Arc::clone(registry), cx))
}

impl EventEmitter<BufferDiffEvent> for BufferDiff {}

impl BufferDiff {
    /// 依据 base/index 文本与 working 实体建立 diff 状态。
    ///
    /// base/index 语言缓冲由本实体创建并持有，修订更新与源文本变化共用同一个后台任务。
    pub fn new(input: BufferDiffInput, cx: &mut Context<Self>) -> Self {
        let BufferDiffInput {
            working,
            path,
            base_text,
            index_text,
            language_registry,
            key: _,
            operations,
        } = input;
        let working_snapshot = working.read(cx).text_snapshot();
        let snapshot = BufferDiffSnapshot {
            hunks: SumTree::new(&working_snapshot),
            pending_hunks: Vec::new(),
        };
        let subscription = cx.subscribe(&working, |this, _, event, cx| {
            if *event == LanguageBufferEvent::TextChanged {
                this.recompute(cx);
            }
        });
        let mut this = Self {
            working,
            base_source: None,
            index_source: None,
            path,
            language_registry,
            snapshot,
            operations,
            revision: 0,
            calculated_inputs: None,
            calculation_task: None,
            revision_texts: RevisionTexts {
                base: base_text,
                index: index_text,
            },
            _working_subscription: subscription,
        };
        this.recompute(cx);
        this
    }

    /// 一次提交完整的修订输入；同一个任务准备语言快照、计算 hunks 并整体安装。
    pub fn set_revisions(
        &mut self,
        base_text: Option<Arc<str>>,
        index_text: Option<Arc<str>>,
        cx: &mut Context<Self>,
    ) {
        let revisions = RevisionTexts {
            base: base_text,
            index: index_text,
        };
        if self.revision_texts == revisions {
            return;
        }
        self.revision_texts = revisions;
        self.recompute(cx);
    }

    /// 源变化与修订变化共用唯一计算任务；投影层只消费结果，不参与任务调度。
    fn recompute(&mut self, cx: &mut Context<Self>) {
        let working = self.working.read(cx).text_snapshot();
        let before = self.input_versions(cx);
        let revisions = self.revision_texts.clone();
        let base_unchanged = self
            .calculated_inputs
            .as_ref()
            .is_some_and(|inputs| inputs.revisions.base == revisions.base);
        let index_unchanged = self
            .calculated_inputs
            .as_ref()
            .is_some_and(|inputs| inputs.revisions.index == revisions.index);
        let base = prepare_revision(
            self.base_source.clone(),
            revisions.base.clone(),
            base_unchanged,
            &self.path,
            &self.language_registry,
            cx,
        );
        let index = prepare_revision(
            self.index_source.clone(),
            revisions.index.clone(),
            index_unchanged,
            &self.path,
            &self.language_registry,
            cx,
        );
        let background = cx.background_executor().clone();
        let task = cx.spawn(async move |this, cx| {
            let prepare = async |revision: Option<RevisionPreparation>| match revision {
                Some(revision) => Some(revision.finish().await),
                None => None,
            };
            let (base, index) = join!(prepare(base), prepare(index));
            let hunks = background
                .spawn(async move {
                    let working_text = full_text(&working);
                    let mut hunks =
                        compute_hunks(revisions.base.as_deref(), &working_text, &working);
                    let index_hunks = index_reference_hunks(
                        revisions.base.as_deref(),
                        revisions.index.as_deref(),
                        &working_text,
                        &working,
                        &hunks,
                    );
                    classify_staging(&mut hunks, index_hunks.as_deref());
                    hunks
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.input_versions(cx) != before {
                    this.recompute(cx);
                    return;
                }
                // base/index 与 hunks 属于同一批输入；消费者不会看到半安装的修订结果。
                this.base_source = base.map(|revision| revision.install(cx));
                this.index_source = index.map(|revision| revision.install(cx));
                let versions = this.input_versions(cx);
                this.apply_recomputed_hunks(versions, hunks, cx);
            });
        });
        self.calculation_task = Some(task);
    }

    /// 接受仍对应当前 working/base/index 版本的后台结果。
    ///
    /// 计算任务已在安装入口校验输入版本，修订快照与本结果在同一轮更新中发布。
    /// 只有 hunk 几何真正变化时才替换快照、清除 pending 并发出 BufferDiffEvent::DiffChanged；
    /// 行内文本修改等不改变 hunk 定位的编辑不做整体重建。
    fn apply_recomputed_hunks(
        &mut self,
        versions: DiffInputVersions,
        hunks: Vec<DiffHunk>,
        cx: &mut Context<Self>,
    ) -> bool {
        let calculation_was_pending = self
            .calculated_inputs
            .as_ref()
            .map(|inputs| inputs.versions)
            != Some(versions);
        let working = self.working.read(cx).text_snapshot();
        let previous_hunks = self.snapshot.hunks.iter().cloned().collect::<Vec<_>>();
        let mut changed_range = changed_hunk_range(&previous_hunks, &hunks, &working);
        if calculation_was_pending {
            let pending_range = anchor_ranges_union(
                self.snapshot
                    .pending_hunks
                    .iter()
                    .map(|pending| pending.buffer_range.clone()),
                &working,
            );
            changed_range = union_anchor_ranges(changed_range, pending_range, &working);
        }
        self.calculated_inputs = Some(CalculatedDiffInputs {
            versions,
            revisions: self.revision_texts.clone(),
        });
        if !calculation_was_pending && hunks_equivalent(&previous_hunks, &hunks) {
            return false;
        }
        self.snapshot = BufferDiffSnapshot {
            hunks: SumTree::from_iter(hunks, &working),
            pending_hunks: Vec::new(),
        };
        self.revision = self.revision.wrapping_add(1).max(1);
        cx.emit(BufferDiffEvent::DiffChanged { changed_range });
        true
    }

    /// 当前 working/base/index 版本。
    fn input_versions(&self, cx: &App) -> DiffInputVersions {
        DiffInputVersions {
            working: self.working.read(cx).text_snapshot().version(),
            base: self
                .base_source
                .as_ref()
                .map(|base| base.read(cx).text_snapshot().version()),
            index: self
                .index_source
                .as_ref()
                .map(|index| index.read(cx).text_snapshot().version()),
        }
    }

    /// diff 结果与 pending 的当前版本；变化即表示显示层需要重新物化。
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// 当前 working/base/index 输入的后台计算是否已经完成。
    pub fn is_current_version_calculated(&self, cx: &App) -> bool {
        self.calculated_inputs
            .as_ref()
            .map(|inputs| inputs.versions)
            == Some(self.input_versions(cx))
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

    /// index 参照文档；hunk 暂存语义相对它判定。
    pub fn index_source(&self) -> Option<&Entity<LanguageBuffer>> {
        self.index_source.as_ref()
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// 文件整体是否为新增：旧侧（base）缺失。
    pub fn is_created(&self) -> bool {
        self.base_source.is_none()
    }

    pub fn operations(&self) -> Option<Arc<dyn DiffOperations>> {
        self.operations.clone()
    }

    /// 把新的 optimistic pending hunks 合并进当前集合，并通知显示层重新物化。
    ///
    /// 与既有 pending 按 working 偏移合并：新 hunk 重叠的旧 pending 被替换，其余保留。
    /// 这样同一文件连续多次操作不会让先前被抑制的 hunk 重新出现（对齐 Zed 的 set_pending_hunks）。
    pub fn set_pending_hunks(&mut self, hunks: Vec<PendingHunk>, cx: &mut Context<Self>) {
        if hunks.is_empty() {
            return;
        }
        let mut pending = std::mem::take(&mut self.snapshot.pending_hunks);
        let working = self.working.read(cx).text_snapshot();
        let changed_range = anchor_ranges_union(
            pending
                .iter()
                .map(|pending| pending.buffer_range.clone())
                .chain(hunks.iter().map(|hunk| hunk.buffer_range.clone())),
            &working,
        );
        for hunk in hunks {
            pending.retain(|existing| {
                existing.buffer_range.end.offset().get() <= hunk.buffer_range.start.offset().get()
                    || hunk.buffer_range.end.offset().get()
                        <= existing.buffer_range.start.offset().get()
            });
            let position = pending.partition_point(|existing| {
                existing.buffer_range.start.offset().get() < hunk.buffer_range.start.offset().get()
            });
            pending.insert(position, hunk);
        }
        self.snapshot.pending_hunks = pending;
        self.revision = self.revision.wrapping_add(1).max(1);
        cx.emit(BufferDiffEvent::DiffChanged { changed_range });
    }

    /// 清除全部 pending hunks（后台操作完成或失败后由宿主调用）。
    pub fn clear_pending_hunks(&mut self, cx: &mut Context<Self>) {
        if self.snapshot.pending_hunks.is_empty() {
            return;
        }
        let working = self.working.read(cx).text_snapshot();
        let changed_range = anchor_ranges_union(
            self.snapshot
                .pending_hunks
                .iter()
                .map(|pending| pending.buffer_range.clone()),
            &working,
        );
        self.snapshot.pending_hunks.clear();
        self.revision = self.revision.wrapping_add(1).max(1);
        cx.emit(BufferDiffEvent::DiffChanged { changed_range });
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
    working_text: &str,
    working: &Snapshot,
    main_hunks: &[DiffHunk],
) -> Option<Vec<DiffHunk>> {
    let index = index_text?;
    if index == working_text {
        return Some(Vec::new());
    }
    if base_text == Some(index) {
        return Some(main_hunks.to_vec());
    }
    Some(compute_hunks(Some(index), working_text, working))
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
fn compute_hunks(base_text: Option<&str>, working_str: &str, working: &Snapshot) -> Vec<DiffHunk> {
    let version = working.version();
    // base 不存在即整份工作区文本为新增（新建文件）。
    let Some(base_text) = base_text else {
        return vec![DiffHunk {
            buffer_range: working.anchor_before(ByteOffset::ZERO)
                ..working.anchor_before(working.len_bytes()),
            diff_base_byte_range: 0..0,
            kind: DiffHunkKind::Added,
            staging: DiffHunkStaging::NoStaging,
            buffer_word_diffs: Vec::new(),
            base_word_diffs: Vec::new(),
        }];
    };
    let input = InternedInput::new(base_text, working_str);
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    diff.postprocess_lines(&input);
    let old_offsets = line_offsets(base_text);
    diff.hunks()
        .map(|hunk| {
            let diff_base_byte_range =
                old_offsets[hunk.before.start as usize]..old_offsets[hunk.before.end as usize];
            let buffer_range =
                anchor_line_range(working, hunk.after.start as usize..hunk.after.end as usize);
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

/// 找出新旧 hunk 序列中最小的连续变化范围，并将其锚定到当前 working 快照。
fn changed_hunk_range(
    old: &[DiffHunk],
    new: &[DiffHunk],
    working: &Snapshot,
) -> Option<Range<Anchor>> {
    let prefix = old
        .iter()
        .zip(new)
        .take_while(|(old, new)| diff_hunks_equal_in(old, new, working))
        .count();
    let mut suffix = 0;
    while prefix + suffix < old.len().min(new.len())
        && diff_hunks_equal_in(
            &old[old.len() - suffix - 1],
            &new[new.len() - suffix - 1],
            working,
        )
    {
        suffix += 1;
    }

    let old_end = old.len() - suffix;
    let new_end = new.len() - suffix;
    anchor_ranges_union(
        old[prefix..old_end]
            .iter()
            .chain(&new[prefix..new_end])
            .map(|hunk| hunk.buffer_range.clone()),
        working,
    )
}

fn diff_hunks_equal_in(old: &DiffHunk, new: &DiffHunk, working: &Snapshot) -> bool {
    let resolve = |anchor: &Anchor| anchor.resolve_in(working).ok();
    resolve(&old.buffer_range.start) == resolve(&new.buffer_range.start)
        && resolve(&old.buffer_range.end) == resolve(&new.buffer_range.end)
        && old.diff_base_byte_range == new.diff_base_byte_range
        && old.kind == new.kind
        && old.staging == new.staging
        && old.base_word_diffs == new.base_word_diffs
        && old.buffer_word_diffs.len() == new.buffer_word_diffs.len()
        && old
            .buffer_word_diffs
            .iter()
            .zip(&new.buffer_word_diffs)
            .all(|(old, new)| {
                resolve(&old.start) == resolve(&new.start) && resolve(&old.end) == resolve(&new.end)
            })
}

fn anchor_ranges_union(
    ranges: impl IntoIterator<Item = Range<Anchor>>,
    working: &Snapshot,
) -> Option<Range<Anchor>> {
    let mut bounds: Option<(ByteOffset, ByteOffset)> = None;
    for range in ranges {
        let start = range.start.resolve_in(working).ok()?;
        let end = range.end.resolve_in(working).ok()?;
        bounds = Some(match bounds {
            Some((current_start, current_end)) => (current_start.min(start), current_end.max(end)),
            None => (start, end),
        });
    }
    let (start, end) = bounds?;
    let range = TextRange::new(start, end).ok()?;
    Some(Anchor::range_outside(working.version(), range))
}

fn union_anchor_ranges(
    first: Option<Range<Anchor>>,
    second: Option<Range<Anchor>>,
    working: &Snapshot,
) -> Option<Range<Anchor>> {
    anchor_ranges_union(first.into_iter().chain(second), working)
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

/// 行范围 → 半开锚点范围（行号右端等于 line_count 时取文本末尾）。
///
/// 起点是区块身份，边界插入后仍位于新文本之前；两端采用相同吸附方向，纯删除保持空范围。
fn anchor_line_range(text: &Snapshot, lines: Range<usize>) -> Range<Anchor> {
    let start = line_start_or_end(text, lines.start);
    let end = line_start_or_end(text, lines.end);
    text.anchor_before(start)..text.anchor_before(end)
}

fn line_start_or_end(text: &Snapshot, line: usize) -> ByteOffset {
    text.line_start_byte(Line::new(line))
        .unwrap_or(text.len_bytes())
}

#[cfg(test)]
#[path = "test/buffer_diff_tests.rs"]
mod tests;
