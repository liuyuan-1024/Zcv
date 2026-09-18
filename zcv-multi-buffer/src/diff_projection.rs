//! MultiBuffer 的 git diff 投影：把版本化的 `BufferDiff` 结果物化为 excerpts 与显示坐标。
//!
//! 普通编辑器与多文件投影（Git 差异视图）共用同一套物化：
//! 宿主注入同一工作区源快照对应的 `BufferDiff`，本层只消费其 `BufferDiffSnapshot`，
//! 按展开状态把旧侧行物化为只读 excerpt、按显示策略裁剪可见行，并派生组合坐标显示 hunks。
//!
//! diff 状态（base/working、版本、hunk、pending、操作）全部归 `BufferDiff` 所有；
//! 展开/折叠、显示路径与上下文裁剪归本层所有，不进入 diff 快照。

use std::ops::Range;
use std::path::{Path, PathBuf};

use gpui::{App, Context, Entity};
use sum_tree::SumTree;
use zcv_git::DiffHunkKind;
use zcv_language::LanguageBuffer;
use zcv_text::{Affinity, Anchor, ByteOffset, Line, PositionMap, Snapshot, Stickiness};

use crate::buffer_diff::{BufferDiff, BufferDiffEvent, DiffHunk, DiffHunkStaging, DiffRefresh};
use crate::{
    DiffTransform, ExcerptDiffKind, ExcerptRange, MultiBuffer, MultiBufferCursor, MultiBufferEvent,
    PathKey, ProjectionRemap, mapping_count,
};

/// 编辑器投影使用的显示 hunk（组合文档行坐标）。
///
/// `range` 与 `old_range` 都是组合文档中的逻辑行范围；源文本定位由 `DiffHunkSource` 提供。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DisplayHunk {
    pub range: Range<usize>,
    pub old_range: Range<usize>,
    pub kind: DiffHunkKind,
    /// 相对 index 参照的暂存语义；无 index 参照时为 NoStaging（实心）。
    pub staging: DiffHunkStaging,
}

/// 一个文件的 diff 注入项：预创建的 diff 实体 + 显示配置。
///
/// diff 实体由 GitStore 按 (working, base) 共享；显示配置由注入方（视图）持有。
#[derive(Clone)]
pub struct DiffFile {
    /// 权威 diff 实体（GitStore 预创建并共享）。
    pub diff: Entity<BufferDiff>,
    /// 组合文档中的显示路径（文件标题与导航定位）。
    pub display_path: PathBuf,
    /// 显示策略：None 显示整个新侧文件（普通编辑器）；
    /// Some(n) 只显示 hunk 周围 n 行上下文（多文件投影）。
    pub context_lines: Option<usize>,
    /// 该文件的第一个可见片段是否创建文件标题块。
    pub show_file_header: bool,
}

/// 显示 hunk 对应的源定位（hunk 操作与导航用）。
#[derive(Clone)]
pub struct DiffHunkSource {
    /// 权威 diff 实体（操作实现与快照来源）。
    pub diff: Entity<BufferDiff>,
    /// 新侧源文件路径（绝对）。
    pub path: PathBuf,
    /// 当前工作区快照中的稳定操作范围；整文件新增块没有源 hunk。
    pub range: Option<Range<Anchor>>,
}

/// 一个文件的显示配置；diff 状态由 `BufferDiff` 持有。
pub(crate) struct DiffState {
    diff: Entity<BufferDiff>,
    /// 组合文档中的显示路径（文件标题与导航定位）。
    display_path: PathKey,
    /// 显示策略：None 显示整个新侧文件；Some(n) 只显示 hunk 周围 n 行上下文。
    context_lines: Option<usize>,
    /// 该文件的第一个可见片段是否创建文件标题块。
    show_file_header: bool,
    /// 显示层拥有的展开/折叠状态，与版本化 diff 结果分离。
    expansion: DiffExpansionState,
    /// 该文件物化出的变换记录（excerpt / boundary 索引、词级 anchor 等）。
    /// 编辑只增量更新组合映射，因此可据此只重算显示坐标而不重新物化 excerpt。
    materialized: Vec<MaterializedHunk>,
}

impl DiffState {
    /// 还原为宿主注入项；按路径替换/移除投影时使用。
    fn to_input(&self) -> DiffFile {
        DiffFile {
            diff: self.diff.clone(),
            display_path: self.display_path.as_path().to_path_buf(),
            context_lines: self.context_lines,
            show_file_header: self.show_file_header,
        }
    }
}

/// 一个 MultiBuffer 的 git diff 投影状态。
#[derive(Default)]
pub(crate) struct DiffDisplayCache {
    /// 显示坐标 hunks（组合坐标，跨文件展平）。
    display_hunks: Vec<DisplayHunk>,
    /// 每个 hunk 在组合文档中的旧侧显示行范围；折叠态或 Added hunk 为 None。
    display_old_ranges: Vec<Option<Range<usize>>>,
    /// 显示 hunk 对应的源文件与源 hunk；整文件新增块没有源 hunk。
    display_sources: Vec<DisplayHunkSource>,
    /// 与显示 hunk 同序的展开状态。
    display_expanded: Vec<bool>,
    /// 与显示 hunk 同序的词级变化片段（组合文档字节范围 + 新增/删除色）。
    display_word_diffs: Vec<Vec<(DiffHunkKind, Range<usize>)>>,
}

/// 一个文件内用户显式切换过展开状态的 hunk。
///
/// 只保存与展开策略默认值不同的显式覆盖，按 (变化类型, 旧侧行范围) 标识；
/// 未覆盖的 hunk 一律采用默认值。新增/修改/删除共用同一份状态，不为类型建立平行集合。
#[derive(Default, Clone)]
struct DiffExpansionState {
    overrides: Vec<HunkExpansionOverride>,
}

#[derive(Clone)]
struct HunkExpansionOverride {
    kind: DiffHunkKind,
    old_range: Range<usize>,
    expanded: bool,
}

pub(crate) struct PendingExpansionMigration {
    old_hunks: Vec<ResolvedHunk>,
    old_state: DiffExpansionState,
}

#[derive(Clone)]
struct DisplayHunkSource {
    file_index: usize,
    hunk_index: Option<usize>,
}

/// 一个 hunk 在显示层需要的行坐标（从 anchor 与旧侧字节范围派生）。
#[derive(Clone)]
struct ResolvedHunk {
    buffer_range: Range<Anchor>,
    /// working 源行范围。
    buffer_lines: Range<usize>,
    /// base 文本行范围（展开状态身份与旧侧物化）。
    base_lines: Range<usize>,
    kind: DiffHunkKind,
    staging: DiffHunkStaging,
    /// 旧侧字节范围起点（base_word_diffs 的相对基准）。
    base_byte_start: usize,
    /// 新侧词级变化片段（working 锚点）。
    buffer_word_diffs: Vec<Range<Anchor>>,
    /// 旧侧词级变化片段（相对 `diff_base_byte_range.start`）。
    base_word_diffs: Vec<Range<usize>>,
}

/// 一个输入 excerpt 的稳定身份。
///
/// 身份绑定源实体与源范围，不绑定组合文档中的序号，因此路径增删和 hunk 重物化不会使它失效。
#[derive(Clone, Copy, PartialEq, Eq)]
struct ExcerptAnchor {
    source_id: gpui::EntityId,
    source_range: zcv_text::TextRange,
}

/// 源文档中的显示边界。
#[derive(Clone, Copy, PartialEq, Eq)]
struct SourceBoundary {
    source_id: gpui::EntityId,
    offset: ByteOffset,
}

struct MaterializedHunk {
    old_range: Range<usize>,
    kind: DiffHunkKind,
    staging: DiffHunkStaging,
    old_excerpt: Option<ExcerptAnchor>,
    new_location: MaterializedHunkLocation,
    source: DisplayHunkSource,
    expanded: bool,
    base_byte_start: usize,
    buffer_word_diffs: Vec<Range<Anchor>>,
    base_word_diffs: Vec<Range<usize>>,
}

#[derive(Clone, Copy)]
enum MaterializedHunkLocation {
    Excerpt(ExcerptAnchor),
    Boundary(SourceBoundary),
}

/// 一次物化派生出的显示坐标；只依赖 hunk 身份与当前组合映射，可在编辑后重算。
struct DiffDisplay {
    hunks: Vec<DisplayHunk>,
    old_ranges: Vec<Option<Range<usize>>>,
    sources: Vec<DisplayHunkSource>,
    expanded: Vec<bool>,
    word_diffs: Vec<Vec<(DiffHunkKind, Range<usize>)>>,
}

struct ExcerptMaterializer<'a> {
    excerpts: &'a mut Vec<ExcerptRange>,
    display_path: &'a Path,
}

impl ExcerptMaterializer<'_> {
    fn push(
        &mut self,
        lines: Range<usize>,
        text: &Snapshot,
        source: &Entity<LanguageBuffer>,
        diff_kind: Option<ExcerptDiffKind>,
        starts_new_excerpt: bool,
        allow_empty: bool,
    ) -> Option<ExcerptAnchor> {
        let excerpt = projected_excerpt(
            source,
            text,
            lines,
            self.display_path,
            diff_kind,
            starts_new_excerpt,
            allow_empty,
        )?;
        let anchor = ExcerptAnchor {
            source_id: excerpt.source.entity_id(),
            source_range: excerpt.source_range,
        };
        self.excerpts.push(excerpt);
        Some(anchor)
    }
}

impl ExcerptAnchor {
    fn map_through_source_change(&mut self, source_id: gpui::EntityId, position_map: &PositionMap) {
        if self.source_id == source_id {
            self.source_range = position_map
                .map_old_range_with_stickiness(self.source_range, Stickiness::Expand)
                .value();
        }
    }
}

impl SourceBoundary {
    fn map_through_source_change(&mut self, source_id: gpui::EntityId, position_map: &PositionMap) {
        if self.source_id == source_id {
            self.offset = position_map
                .map_old_position_with_affinity(self.offset, Affinity::Before)
                .value();
        }
    }
}

impl MultiBuffer {
    /// 统一注入 git diff 投影（普通编辑器与多文件投影共用）。
    ///
    /// None 是加载态（新 diff 尚未算完），保留现有 hunks 与用户展开状态；
    /// Some 注入后按文本跟踪区间迁移展开状态并重建投影。
    /// 返回 true 表示组合文档被重建；调用方应同步显示快照，但不能重置源锚点选区。
    /// 按路径增量挂接一个文件的 diff。
    ///
    /// 新路径按路径顺序追加；同路径的 diff 变化重建整份投影以迁移展开状态。
    /// 返回 true 表示组合文档已更新；diff 仍在后台计算时返回 false，结果到达后自动物化。
    pub fn add_diff(&mut self, file: DiffFile, cx: &mut Context<Self>) -> bool {
        let existing = self.diffs.iter().position(|current| {
            current.display_path.as_path() == file.display_path
                || current.diff.read(cx).working().entity_id()
                    == file.diff.read(cx).working().entity_id()
        });
        if let Some(index) = existing {
            let current = &self.diffs[index];
            if current.diff.entity_id() == file.diff.entity_id()
                && current.display_path.as_path() == file.display_path
                && current.context_lines == file.context_lines
                && current.show_file_header == file.show_file_header
            {
                return false;
            }
            let mut files = self
                .diffs
                .iter()
                .map(DiffState::to_input)
                .collect::<Vec<_>>();
            files[index] = file;
            return self.set_diff_files(files, cx);
        }
        // 新路径按显示路径顺序插入：位于末尾时走增量追加，插到中间时整体重建。
        let insert_at = self.diffs.partition_point(|current| {
            current.display_path.as_path() < file.display_path.as_path()
        });
        let len = self.diffs.len();
        if insert_at == len {
            return self.append_diff_projection(vec![file], cx);
        }
        self.insert_diff_file(insert_at, file, cx)
    }

    /// 路径顺序在某文件之后的物化记录只需平移文件身份；excerpt 身份由源 anchor 保持稳定。
    fn shift_downstream_files(&mut self, file_index: usize, file_shift: isize) {
        for file in self.diffs.iter_mut().skip(file_index + 1) {
            for hunk in &mut file.materialized {
                hunk.source.file_index = (hunk.source.file_index as isize + file_shift) as usize;
            }
        }
    }

    /// 在 insert_at 处插入一个 diff 文件，只物化该文件的 excerpts 并按路径 splice。
    fn insert_diff_file(
        &mut self,
        insert_at: usize,
        file: DiffFile,
        cx: &mut Context<Self>,
    ) -> bool {
        let state = DiffState {
            diff: file.diff,
            display_path: PathKey::new(file.display_path),
            context_lines: file.context_lines,
            show_file_header: file.show_file_header,
            expansion: DiffExpansionState::default(),
            materialized: Vec::new(),
        };
        let subscription = cx.subscribe(&state.diff, |this, _, event, cx| {
            let BufferDiffEvent::DiffChanged { refresh } = event;
            this.diff_changed(*refresh, cx);
        });
        self.diffs.insert(insert_at, state);
        self.diff_subscriptions.insert(insert_at, subscription);
        self.diff_pending_expansion_migrations
            .insert(insert_at, None);

        let expanded_by_default = self.diff_expanded_by_default;
        let mut excerpts = Vec::new();
        let mut materialized = Vec::new();
        {
            let file = &self.diffs[insert_at];
            materialize_file(
                insert_at,
                file,
                &file.expansion,
                cx,
                expanded_by_default,
                &mut excerpts,
                &mut materialized,
            );
        }
        // 路径顺序在插入点之后的文件整体顺延。
        self.shift_downstream_files(insert_at, 1);
        self.set_excerpts_for_path(excerpts, cx);
        for hunk in materialized {
            self.diffs[hunk.source.file_index].materialized.push(hunk);
        }
        let revision = (
            self.diffs[insert_at].diff.entity_id(),
            self.diffs[insert_at].diff.read(cx).revision(),
        );
        self.diff_display_revisions.insert(insert_at, revision);
        if insert_at < self.diff_materialized_files {
            self.diff_materialized_files += 1;
        }
        let display =
            self.derive_diff_display(self.diffs.iter().flat_map(|file| file.materialized.iter()));
        let diff = self.diff.as_mut().expect("已确认 diff 投影存在");
        diff.display_hunks = display.hunks;
        diff.display_old_ranges = display.old_ranges;
        diff.display_sources = display.sources;
        diff.display_expanded = display.expanded;
        diff.display_word_diffs = display.word_diffs;
        cx.notify();
        true
    }

    /// 移除指定显示路径的 diff；用于 Git 状态中不再存在的文件。
    ///
    /// 按路径增量移除该文件的 excerpts（不动其余文件的源订阅），
    /// 再把路径顺序在其之后的物化记录下标整体前移，最后只重算显示坐标。
    /// 移除最后一个文件时回退到整份清理路径（可能恢复整文件 excerpt）。
    pub fn remove_diff(&mut self, path: &Path, cx: &mut Context<Self>) -> bool {
        if self.diff.is_none() {
            return false;
        }
        let Some(file_index) = self
            .diffs
            .iter()
            .position(|file| file.display_path.as_path() == path)
        else {
            return false;
        };
        if self.diffs.len() == 1 {
            self.set_diff_files(Vec::new(), cx);
            return true;
        }

        // 被移除路径在 excerpt 流中的区间（映射按源路径升序；显示路径可能被裁剪为相对路径）。
        let source_path = self.diffs[file_index]
            .diff
            .read(cx)
            .working()
            .read(cx)
            .file_path()
            .map_or_else(PathBuf::new, Path::to_path_buf);
        self.remove_excerpts_for_path(&source_path, cx);

        drop(self.diff_subscriptions.remove(file_index));
        self.diff_display_revisions.remove(file_index);
        self.diff_pending_expansion_migrations.remove(file_index);
        // 路径顺序在被移除文件之后的文件只需平移文件身份。
        self.shift_downstream_files(file_index, -1);
        self.diffs.remove(file_index);

        let display =
            self.derive_diff_display(self.diffs.iter().flat_map(|file| file.materialized.iter()));
        let diff = self.diff.as_mut().expect("已确认 diff 投影存在");
        diff.display_hunks = display.hunks;
        diff.display_old_ranges = display.old_ranges;
        diff.display_sources = display.sources;
        diff.display_expanded = display.expanded;
        diff.display_word_diffs = display.word_diffs;
        self.diff_materialized_files = self.diffs.len();
        cx.notify();
        true
    }

    /// 清除全部 diff，使组合文档回到无 diff 状态。
    pub fn clear_diffs(&mut self, cx: &mut Context<Self>) -> bool {
        if self.diff.is_none() && self.singleton_source.is_none() {
            return false;
        }
        self.set_diff_files(Vec::new(), cx)
    }

    /// 用给定文件列表整体替换投影；结构性变化（刷新、展开策略切换）的重建入口。
    ///
    /// 按路径增量更新请使用 Self::add_diff / Self::remove_diff。
    pub fn set_diff_files(&mut self, inputs: Vec<DiffFile>, cx: &mut Context<Self>) -> bool {
        if inputs.is_empty()
            && let Some(source) = self.singleton_source.clone()
        {
            self.diff = None;
            self.diffs.clear();
            let line_count = source.read(cx).text_snapshot(cx).line_count();
            self.set_excerpts(
                vec![
                    ExcerptRange::line_range(source, 0..line_count, cx)
                        .with_starts_new_excerpt(false),
                ],
                cx,
            );
            return true;
        }
        // 路径顺序的尾部追加：
        // 已有文件身份与顺序不变时，只登记新增文件，由 diff 计算完成事件增量物化，避免整份组合文档重建。
        let append_from = self.diff.as_ref().and_then(|_| {
            let old_len = self.diffs.len();
            (old_len > 0
                && inputs.len() > old_len
                && self.diffs.iter().zip(inputs.iter()).all(|(old, new)| {
                    old.display_path.as_path() == new.display_path
                        && old.diff.entity_id() == new.diff.entity_id()
                        && old.diff.read(cx).working().entity_id()
                            == new.diff.read(cx).working().entity_id()
                }))
            .then_some(old_len)
        });
        if let Some(old_len) = append_from {
            let appended = inputs[old_len..].to_vec();
            return self.append_diff_projection(appended, cx);
        }
        let old_files = self.diff.as_mut().map(|_| std::mem::take(&mut self.diffs));
        self.diff
            .get_or_insert_with(|| Box::new(DiffDisplayCache::default()));

        let mut next_files: Vec<DiffState> = inputs
            .into_iter()
            .map(|file| DiffState {
                diff: file.diff,
                display_path: PathKey::new(file.display_path),
                context_lines: file.context_lines,
                show_file_header: file.show_file_header,
                expansion: DiffExpansionState::default(),
                materialized: Vec::new(),
            })
            .collect();

        let mut pending_expansion_migrations = Vec::with_capacity(next_files.len());
        for file in &mut next_files {
            let mut pending_migration = None;
            if let Some(old_files) = old_files.as_deref()
                && let Some(old_file) = old_files.iter().find(|old| {
                    old.diff.read(cx).working().entity_id()
                        == file.diff.read(cx).working().entity_id()
                })
            {
                let old_resolved = resolve_file_hunks(old_file, cx);
                if file.diff.read(cx).is_current_version_calculated(cx) {
                    let new_resolved = resolve_file_hunks(file, cx);
                    migrate_expansion_state(
                        &old_resolved,
                        &old_file.expansion,
                        &new_resolved,
                        &mut file.expansion,
                    );
                } else {
                    pending_migration = Some(PendingExpansionMigration {
                        old_hunks: old_resolved,
                        old_state: old_file.expansion.clone(),
                    });
                }
            }
            pending_expansion_migrations.push(pending_migration);
        }
        self.diff_subscriptions = next_files
            .iter()
            .map(|file| {
                cx.subscribe(&file.diff, |this, _, event, cx| {
                    let BufferDiffEvent::DiffChanged { refresh } = event;
                    this.diff_changed(*refresh, cx);
                })
            })
            .collect();
        self.diffs = next_files;
        self.diff_pending_expansion_migrations = pending_expansion_migrations;
        // 新文件的 hunk 尚未算完时，保留已物化投影，避免先清空再展示结果导致一次
        // Git 刷新产生两次可见重建；各文件的就绪状态互不影响。
        if self
            .diffs
            .iter()
            .any(|file| !file.diff.read(cx).is_current_version_calculated(cx))
        {
            return false;
        }
        !self.rebuild_diff_projection(cx).is_identity()
    }

    /// 设置新 hunk 的初始展开策略；用户之后的显式展开/折叠不受投影刷新覆盖。
    ///
    /// 折叠/展开不改变源，编辑器源锚点选区自然存活，无需返回投影重映射。
    pub fn set_diff_hunks_expanded_by_default(&mut self, expanded: bool, cx: &mut Context<Self>) {
        self.diff
            .get_or_insert_with(|| Box::new(DiffDisplayCache::default()));
        if self.diff_expanded_by_default == expanded {
            return;
        }
        self.diff_expanded_by_default = expanded;
        // 策略切换不迁移旧状态：按新默认值重新应用（清空全部显式集合）。
        for file in &mut self.diffs {
            file.expansion = DiffExpansionState::default();
        }
        self.rebuild_diff_projection(cx);
        cx.emit(MultiBufferEvent::DiffExpansionChanged);
    }

    /// 按显示 hunk 索引切换展开/折叠（渲染层点击入口）。
    ///
    /// 折叠/展开不改变源，编辑器源锚点选区自然存活，无需返回投影重映射。
    pub fn toggle_diff_hunk_at(&mut self, display_index: usize, cx: &mut Context<Self>) {
        let expanded_by_default = self.diff_expanded_by_default;
        let mut file_index = None;
        if let Some(diff) = &mut self.diff
            && let Some(source) = diff.display_sources.get(display_index)
            && let Some(hunk) = diff.display_hunks.get(display_index)
            && let Some(expansion) = self
                .diffs
                .get_mut(source.file_index)
                .map(|file| &mut file.expansion)
        {
            expansion.toggle(hunk.kind, &hunk.old_range, expanded_by_default);
            file_index = Some(source.file_index);
        }
        let Some(file_index) = file_index else {
            return;
        };
        // 只重物化该文件所在路径；其余文件及其组合坐标保持不变。
        self.replace_materialized_file(file_index, cx);
        cx.emit(MultiBufferEvent::DiffExpansionChanged);
    }

    /// base 版本变化（HEAD 变化等）后由宿主调用：旧侧坐标空间已失效，按默认策略重置展开状态。
    ///
    /// 折叠/展开不改变源，编辑器源锚点选区自然存活，无需返回投影重映射。
    pub fn reset_diff_hunk_expansion_state(&mut self, cx: &mut Context<Self>) {
        if self.diff.is_none() {
            return;
        }
        for file in &mut self.diffs {
            file.expansion = DiffExpansionState::default();
        }
        self.rebuild_diff_projection(cx);
        cx.emit(MultiBufferEvent::DiffExpansionChanged);
    }

    /// 显示坐标 hunks（组合坐标，跨文件展平）。
    ///
    /// 坐标是当前组合映射下的派生缓存，在所有会改动组合文档的路径上同步刷新；
    /// 单个文件未就绪不会让其他文件的高亮消失。
    pub fn diff_hunks(&self) -> &[DisplayHunk] {
        self.diff.as_ref().map_or(&[], |diff| &diff.display_hunks)
    }

    /// 已挂接 diff 的显示路径集合（按组合文档顺序）。
    pub fn diff_paths(&self) -> Vec<PathBuf> {
        self.diffs
            .iter()
            .map(|file| file.display_path.as_path().to_path_buf())
            .collect()
    }

    /// 每个 hunk 在组合文档中的旧侧显示行范围。
    pub fn diff_hunk_old_ranges(&self) -> &[Option<Range<usize>>] {
        self.diff
            .as_ref()
            .map_or(&[], |diff| &diff.display_old_ranges)
    }

    /// 与 MultiBuffer::diff_hunks 平行的词级变化片段（组合文档字节范围 + 新增/删除色）。
    pub fn diff_hunk_word_diffs(&self) -> &[Vec<(DiffHunkKind, Range<usize>)>] {
        self.diff
            .as_ref()
            .map_or(&[], |diff| &diff.display_word_diffs)
    }

    /// 与 MultiBuffer::diff_hunks 平行的展开标志（渲染层按显示 hunk 索引查询）。
    pub fn diff_hunk_expanded(&self) -> Vec<bool> {
        self.diff
            .as_ref()
            .map_or(Vec::new(), |diff| diff.display_expanded.clone())
    }

    /// 显示 hunk 到源定位（hunk 操作与导航用）。
    pub fn buffer_diff_hunk_at(&self, display_index: usize, cx: &App) -> Option<DiffHunkSource> {
        let diff = self.diff.as_ref()?;
        // 缓存必须对应当前文件集合，否则 display_sources 的 file_index 可能指向别的文件。
        if self.diffs.len() != self.diff_display_revisions.len()
            || !self
                .diffs
                .iter()
                .zip(&self.diff_display_revisions)
                .all(|(file, previous)| file.diff.entity_id() == previous.0)
        {
            return None;
        }
        let source = diff.display_sources.get(display_index)?.clone();
        let file = self.diffs.get(source.file_index)?;
        let entity = file.diff.clone();
        let is_created = entity.read(cx).is_created();
        let path = entity.read(cx).path().clone();
        if is_created {
            // 整文件新增块没有可供 Git 操作重解析的 hunk。
            return Some(DiffHunkSource {
                diff: entity,
                path,
                range: None,
            });
        }
        let range = source.hunk_index.and_then(|index| {
            entity
                .read(cx)
                .snapshot()
                .visible_hunks()
                .get(index)
                .map(|hunk| hunk.buffer_range.clone())
        });
        Some(DiffHunkSource {
            diff: entity,
            path,
            range,
        })
    }

    /// 查询指定 diff 文件的 working source 是否有未保存修改。
    ///
    /// dirty 状态由源 Buffer 唯一拥有；组合文档只读取该状态，用于文件级提示。
    pub fn is_diff_file_dirty(&self, path: &Path, cx: &App) -> bool {
        self.diff.as_ref().is_some_and(|_| {
            self.diffs.iter().any(|file| {
                file.diff.read(cx).path() == path
                    && file
                        .diff
                        .read(cx)
                        .working()
                        .read(cx)
                        .buffer()
                        .read(cx)
                        .is_dirty()
            })
        })
    }

    /// 把打开请求中的 Deleted 片段换算为工作区文件中的合法定位行列（0-based）。
    ///
    /// Deleted 片段的内容来自 Git 修订文本，其字节坐标在打开的工作区文件中不存在；
    /// 经 hunk 把修订侧行号映射到工作区（新侧）行号，列沿用修订行内逻辑列，行与列都按工作区文件文本钳制到有效范围，返回值可直接用于行列导航。
    /// 非 Deleted 片段返回 None（坐标直接可用）。
    pub fn deleted_navigation_target(
        &self,
        location: &crate::ExcerptLocation,
        working_text: &Snapshot,
        cx: &App,
    ) -> Option<(usize, usize)> {
        let snapshot = self.snapshot(cx);
        // 仅处理 Deleted 片段：修订文本坐标需换算，其余片段直接可用。
        let in_deleted_excerpt =
            snapshot
                .excerpts_for_path(location.path.as_path())
                .any(|excerpt| {
                    excerpt.diff_kind() == Some(ExcerptDiffKind::Deleted)
                        && excerpt.source_range().start() <= location.source_range.start()
                        && location.source_range.end() <= excerpt.source_range().end()
                });
        if !in_deleted_excerpt {
            return None;
        }
        // 修订文本行号与列（列按 Unicode scalar 计数，与导航协议一致）。
        self.diff.as_ref()?;
        let file = self
            .diffs
            .iter()
            .find(|file| file.diff.read(cx).path() == &location.path)?;
        let base = file.diff.read(cx).base_source()?.clone();
        let base_text = base.read(cx).text_snapshot(cx);
        let position = base_text
            .byte_to_position(location.source_range.start())
            .ok()?;
        let old_line = position.line().get();
        let column = position.column().get();
        // 包含该修订行的 hunk（旧侧行范围）。
        let resolved = resolve_file_hunks(file, cx);
        let hunk = resolved
            .iter()
            .find(|hunk| hunk.base_lines.contains(&old_line))?;
        // 修改行在 hunk 内按偏移映射；纯删除锚定变更块起点。
        let offset = old_line - hunk.base_lines.start;
        let working_line = if hunk.buffer_lines.is_empty() {
            hunk.buffer_lines.start
        } else {
            (hunk.buffer_lines.start + offset).min(hunk.buffer_lines.end - 1)
        };
        // 行与列钳制到工作区文件有效范围（修改可能让行变短）。
        let line = working_line.min(working_text.line_count().saturating_sub(1));
        let column = clamp_column_to_line(working_text, line, column);
        Some((line, column))
    }

    /// 指定源是否属于当前 diff 投影。
    pub(crate) fn is_diff_source(&self, source_id: gpui::EntityId, cx: &App) -> bool {
        self.diff.as_ref().is_some_and(|_| {
            self.diffs
                .iter()
                .any(|file| file.diff.read(cx).working().entity_id() == source_id)
        })
    }

    pub(crate) fn recompute_diff_for_source(
        &mut self,
        source_id: gpui::EntityId,
        refresh: DiffRefresh,
        cx: &mut Context<Self>,
    ) {
        let diff = self.diff.as_ref().and_then(|_| {
            self.diffs
                .iter()
                .find(|file| file.diff.read(cx).working().entity_id() == source_id)
                .map(|file| file.diff.clone())
        });
        if let Some(diff) = diff {
            diff.update(cx, |diff, cx| diff.recompute_with_refresh(refresh, cx));
        }
    }

    /// BufferDiff 事件入口：只有当前物化结果落后于 diff 版本时才重建。
    fn diff_changed(&mut self, refresh: DiffRefresh, cx: &mut Context<Self>) {
        let Some(diff) = &mut self.diff else {
            return;
        };
        if refresh == DiffRefresh::PreserveProjection {
            return;
        }
        let working_is_dirty = self.diffs.iter().any(|file| {
            file.diff
                .read(cx)
                .working()
                .read(cx)
                .buffer()
                .read(cx)
                .is_dirty()
        });
        if working_is_dirty && !diff.display_expanded.iter().any(|&expanded| expanded) {
            // 折叠态组合文档的 excerpt 是用户当前正在编辑的稳定窗口。
            // 没有展开 hunk 时只更新 BufferDiff 快照，等保存/重新注入后再提交新的窗口；
            // 展开态则必须跟随新的 working 快照重物化，保证可见 hunk 与正文一致。
            return;
        }
        for index in 0..self.diffs.len() {
            if self.diff_pending_expansion_migrations[index].is_none()
                || !self.diffs[index]
                    .diff
                    .read(cx)
                    .is_current_version_calculated(cx)
            {
                continue;
            }
            let pending = self.diff_pending_expansion_migrations[index]
                .take()
                .expect("已检查 pending expansion migration 存在");
            let new_hunks = resolve_file_hunks(&self.diffs[index], cx);
            migrate_expansion_state(
                &pending.old_hunks,
                &pending.old_state,
                &new_hunks,
                &mut self.diffs[index].expansion,
            );
        }
        let (calculated_prefix, materialized, prefix_changed) = {
            let calculated_prefix = self
                .diffs
                .iter()
                .take_while(|file| file.diff.read(cx).is_current_version_calculated(cx))
                .count();
            let materialized = self.diff_materialized_files.min(self.diffs.len());
            let prefix_changed = self.diff_materialized_files > self.diffs.len()
                || self.diff_display_revisions.is_empty()
                || !self.diffs[..materialized]
                    .iter()
                    .zip(self.diff_display_revisions.iter())
                    .all(|(file, previous)| {
                        file.diff.entity_id() == previous.0
                            && file.diff.read(cx).revision() == previous.1
                    });
            (calculated_prefix, materialized, prefix_changed)
        };
        if prefix_changed {
            // 尚无任何已物化文件（首个 diff 结果到达）或文件集合与已物化前缀不一致时整体重建。
            if self.diff_display_revisions.is_empty()
                || self.diff_materialized_files > self.diffs.len()
            {
                self.rebuild_diff_projection(cx);
                return;
            }
            // 身份或版本变化的文件逐个原地重物化，只替换对应路径的 excerpts；
            // 未变化的路径其组合坐标与展开状态保持不变。
            let changed = (0..materialized)
                .filter(|&index| {
                    self.diff_display_revisions
                        .get(index)
                        .is_none_or(|(entity_id, revision)| {
                            self.diffs[index].diff.entity_id() != *entity_id
                                || self.diffs[index].diff.read(cx).revision() != *revision
                        })
                })
                .collect::<Vec<_>>();
            for index in changed {
                self.replace_materialized_file(index, cx);
            }
        }
        // 尾部新就绪的文件只增量追加，不重建已物化的前缀。
        if calculated_prefix > materialized {
            self.append_materialized_files(materialized, calculated_prefix, cx);
        }
    }

    /// 按路径顺序登记追加的 diff 文件，并物化其中已计算完成的前缀。
    ///
    /// 尚未计算完成的文件只登记订阅；结果到达后由 diff_changed 增量物化。
    pub fn append_diff_projection(&mut self, files: Vec<DiffFile>, cx: &mut Context<Self>) -> bool {
        if files.is_empty() {
            return false;
        }
        if self.diff.is_none() {
            return self.set_diff_files(files, cx);
        }
        let file_count = files.len();
        let next_files: Vec<DiffState> = files
            .into_iter()
            .map(|file| DiffState {
                diff: file.diff,
                display_path: PathKey::new(file.display_path),
                context_lines: file.context_lines,
                show_file_header: file.show_file_header,
                expansion: DiffExpansionState::default(),
                materialized: Vec::new(),
            })
            .collect();
        let subscriptions = next_files
            .iter()
            .map(|file| {
                cx.subscribe(&file.diff, |this, _, event, cx| {
                    let BufferDiffEvent::DiffChanged { refresh } = event;
                    this.diff_changed(*refresh, cx);
                })
            })
            .collect::<Vec<_>>();
        self.diff_subscriptions.extend(subscriptions);
        self.diffs.extend(next_files);
        self.diff_pending_expansion_migrations
            .extend((0..file_count).map(|_| None));
        let (from, to) = {
            let from = self.diff_materialized_files;
            let to = self
                .diffs
                .iter()
                .take_while(|file| file.diff.read(cx).is_current_version_calculated(cx))
                .count();
            (from, to)
        };
        if to > from {
            self.append_materialized_files(from, to, cx);
        }
        to == self.diffs.len()
    }

    /// 追加指定范围文件的物化结果，只扩展组合映射与显示坐标，不重建已有片段。
    fn append_materialized_files(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let base_excerpt_count = mapping_count(&self.state.diff_transforms);
        let expanded_by_default = self.diff_expanded_by_default;
        let mut excerpts = Vec::new();
        let mut materialized = Vec::new();
        {
            for index in from..to {
                materialize_file(
                    index,
                    &self.diffs[index],
                    &self.diffs[index].expansion,
                    cx,
                    expanded_by_default,
                    &mut excerpts,
                    &mut materialized,
                );
            }
        }
        let expected_excerpt_count = excerpts.len();
        let _ = self.append_excerpts(excerpts, cx);
        assert_eq!(
            mapping_count(&self.state.diff_transforms),
            base_excerpt_count + expected_excerpt_count,
            "追加 diff 物化必须全部建立组合映射"
        );
        let display = self.derive_diff_display(materialized.iter());
        for hunk in materialized {
            self.diffs[hunk.source.file_index].materialized.push(hunk);
        }
        let diff = self.diff.as_mut().expect("追加物化前 diff 状态必须存在");
        diff.display_hunks.extend(display.hunks);
        diff.display_old_ranges.extend(display.old_ranges);
        diff.display_sources.extend(display.sources);
        diff.display_expanded.extend(display.expanded);
        diff.display_word_diffs.extend(display.word_diffs);
        let revisions = self.diffs[from..to]
            .iter()
            .map(|file| (file.diff.entity_id(), file.diff.read(cx).revision()))
            .collect::<Vec<_>>();
        self.diff_display_revisions.truncate(from);
        self.diff_display_revisions.extend(revisions);
        self.diff_materialized_files = to;
        cx.notify();
    }

    /// 原地重物化单个文件，只替换其路径的 excerpts，其余路径保持不变。
    ///
    /// 用于某个文件的 diff 结果发生版本或身份变化时避免整份组合文档重建：
    /// 先按当前 hunk 收敛该文件的展开覆盖，再物化该文件，最后只重算显示坐标。
    fn replace_materialized_file(&mut self, file_index: usize, cx: &mut Context<Self>) {
        let resolved = resolve_file_hunks(&self.diffs[file_index], cx);
        self.diffs[file_index]
            .expansion
            .retain_for_current_hunks(&resolved);

        let expanded_by_default = self.diff_expanded_by_default;
        let mut excerpts = Vec::new();
        let mut materialized = Vec::new();
        {
            let file = &self.diffs[file_index];
            materialize_file(
                file_index,
                file,
                &file.expansion,
                cx,
                expanded_by_default,
                &mut excerpts,
                &mut materialized,
            );
        }
        // 映射树按源路径排序，显示路径可能被裁剪为相对路径。
        let path = PathKey::new(
            self.diffs[file_index]
                .diff
                .read(cx)
                .working()
                .read(cx)
                .file_path()
                .map_or_else(PathBuf::new, Path::to_path_buf),
        );
        // 该文件已无可见 hunk（差异被消除等）时必须移除其路径的 excerpts；
        // set_excerpts_for_path 对空片段集合是空操作，无法表达“清空该路径”。
        if excerpts.is_empty() {
            self.remove_excerpts_for_path(path.as_path(), cx);
        } else {
            self.set_excerpts_for_path(excerpts, cx);
        }
        self.diffs[file_index].materialized = materialized;
        self.diff_display_revisions[file_index] = (
            self.diffs[file_index].diff.entity_id(),
            self.diffs[file_index].diff.read(cx).revision(),
        );
        let display =
            self.derive_diff_display(self.diffs.iter().flat_map(|file| file.materialized.iter()));
        let diff = self.diff.as_mut().expect("已确认 diff 投影存在");
        diff.display_hunks = display.hunks;
        diff.display_old_ranges = display.old_ranges;
        diff.display_sources = display.sources;
        diff.display_expanded = display.expanded;
        diff.display_word_diffs = display.word_diffs;
        cx.notify();
    }

    /// 按展开状态与显示策略重建可见 excerpts，并派生显示坐标 hunks。
    ///
    /// 返回本次重建的投影坐标重映射：
    /// 投影版本未变时恒等，变化时携带重建前的投影→源映射，供调用方把重建前的光标经源忠实落到重建后投影（reload 会重裁剪并重置版本，裸偏移不再有效）。
    pub(crate) fn rebuild_diff_projection(&mut self, cx: &mut Context<Self>) -> ProjectionRemap {
        let before = (
            self.state.excerpts.clone(),
            self.state.diff_transforms.clone(),
        );
        self.rebuild_diff_projection_from(before, cx)
    }

    /// 按调用方在 hunk 生命周期变更前冻结的投影映射重建。
    ///
    /// 编辑坐标恢复属于投影事务，而非 hunk 刷新：调用方在源文本已更新、hunk
    /// 尚未重物化时保存映射，随后无论 hunk 怎样裁剪或失效，都用该映射解析编辑后的选区。
    pub(crate) fn rebuild_diff_projection_from(
        &mut self,
        before: (SumTree<crate::Excerpt>, SumTree<DiffTransform>),
        cx: &mut Context<Self>,
    ) -> ProjectionRemap {
        if self.diff.is_none() {
            return ProjectionRemap::identity();
        }
        let old_snapshot = self.snapshot(cx);
        self.rebuild_diff_projection_from_text(before, old_snapshot.text_bytes(), cx)
    }

    /// 使用调用方在源快照更新前保存的旧输出重建 diff 投影。
    ///
    /// 外部整体重载会先替换源快照，再重建 excerpts。旧映射此时仍可能只覆盖新文本的前缀；
    /// 因而旧输出必须在源快照替换前冻结，不能从更新后的源映射重新拼出旧帧。
    pub(crate) fn rebuild_diff_projection_from_text(
        &mut self,
        before: (SumTree<crate::Excerpt>, SumTree<DiffTransform>),
        old_text: Vec<u8>,
        cx: &mut Context<Self>,
    ) -> ProjectionRemap {
        if self.diff.is_none() {
            return ProjectionRemap::identity();
        }
        let old_version = self.state.projection_version;
        let expanded_by_default = self.diff_expanded_by_default;
        let mut excerpts = Vec::new();
        let mut materialized_hunks = Vec::new();
        for (file_index, file) in self.diffs.iter().enumerate() {
            materialize_file(
                file_index,
                file,
                &file.expansion,
                cx,
                expanded_by_default,
                &mut excerpts,
                &mut materialized_hunks,
            );
        }
        let expected_excerpt_count = excerpts.len();
        self.set_excerpts_internal(excerpts, cx);
        assert_eq!(
            mapping_count(&self.state.diff_transforms),
            expected_excerpt_count,
            "diff 物化生成的 excerpt 必须全部建立组合映射"
        );
        let new_snapshot = self.build_snapshot(cx);
        let new_text = new_snapshot.text_bytes();
        self.publish_projection_edit(&old_text, &new_text, old_version);
        let display = self.derive_diff_display(materialized_hunks.iter());
        for file in &mut self.diffs {
            file.materialized.clear();
        }
        for hunk in materialized_hunks {
            self.diffs[hunk.source.file_index].materialized.push(hunk);
        }
        let new_version = self.snapshot(cx).version();
        let diff = self.diff.as_mut().expect("投影重建前 diff 状态必须存在");
        diff.display_hunks = display.hunks;
        diff.display_old_ranges = display.old_ranges;
        diff.display_sources = display.sources;
        diff.display_expanded = display.expanded;
        diff.display_word_diffs = display.word_diffs;
        self.diff_display_revisions = self
            .diffs
            .iter()
            .map(|file| (file.diff.entity_id(), file.diff.read(cx).revision()))
            .collect();
        // 整体重建会物化全部文件（未就绪文件按空 hunk 投影），因此前缀直接取文件总数。
        self.diff_materialized_files = self.diffs.len();
        cx.notify();
        if new_version != old_version {
            ProjectionRemap::rebuilt(before.0, before.1)
        } else {
            ProjectionRemap::identity()
        }
    }

    /// 根据稳定源身份定位 excerpt 在当前组合文档中的真实逻辑行范围。
    /// 空片段仍对应编辑器中的一个空逻辑行。
    fn mapping_for_excerpt_anchor(&self, anchor: ExcerptAnchor) -> Option<crate::ExcerptMapping> {
        let mut cursor = MultiBufferCursor::new(&self.state.excerpts, &self.state.diff_transforms);
        cursor.seek_output(ByteOffset::ZERO, sum_tree::Bias::Right);
        while let Some((excerpt, _)) = cursor.item() {
            if excerpt.source_id == anchor.source_id && excerpt.source_range == anchor.source_range
            {
                return cursor.mapping();
            }
            cursor.next();
        }
        None
    }

    fn diff_excerpt_output_lines(&self, anchor: ExcerptAnchor) -> Range<usize> {
        let mapping = self
            .mapping_for_excerpt_anchor(anchor)
            .expect("diff excerpt 必须存在对应组合映射");
        mapping.output_start_line..mapping.output_end_line.max(mapping.output_start_line + 1)
    }

    pub(crate) fn map_materialized_source_anchors(
        &mut self,
        source_id: gpui::EntityId,
        position_map: &PositionMap,
    ) {
        for file in &mut self.diffs {
            for hunk in &mut file.materialized {
                if let Some(anchor) = &mut hunk.old_excerpt {
                    anchor.map_through_source_change(source_id, position_map);
                }
                match &mut hunk.new_location {
                    MaterializedHunkLocation::Excerpt(anchor) => {
                        anchor.map_through_source_change(source_id, position_map);
                    }
                    MaterializedHunkLocation::Boundary(boundary) => {
                        boundary.map_through_source_change(source_id, position_map);
                    }
                }
            }
        }
    }

    /// 从已物化的 hunk 身份与当前组合映射派生显示坐标。
    ///
    /// 与 `set_excerpts` 解耦：编辑只增量更新组合映射，因此可只重算坐标而不重新物化 excerpt。
    fn derive_diff_display<'a>(
        &self,
        materialized: impl IntoIterator<Item = &'a MaterializedHunk>,
    ) -> DiffDisplay {
        let materialized = materialized.into_iter().collect::<Vec<_>>();
        let mut hunks = Vec::with_capacity(materialized.len());
        let mut old_ranges = Vec::with_capacity(materialized.len());
        let mut sources = Vec::with_capacity(materialized.len());
        let mut expanded = Vec::with_capacity(materialized.len());
        let mut word_diffs = Vec::with_capacity(materialized.len());
        for hunk in materialized {
            let old_display = hunk
                .old_excerpt
                .map(|excerpt| self.diff_excerpt_output_lines(excerpt));
            word_diffs.push(self.combined_word_diffs(hunk));
            let new_range = match hunk.new_location {
                MaterializedHunkLocation::Excerpt(excerpt) => {
                    self.diff_excerpt_output_lines(excerpt)
                }
                MaterializedHunkLocation::Boundary(boundary) => {
                    let line = self.diff_excerpt_boundary_line(boundary);
                    line..line
                }
            };
            hunks.push(DisplayHunk {
                range: new_range,
                old_range: hunk.old_range.clone(),
                kind: hunk.kind,
                staging: hunk.staging,
            });
            old_ranges.push(old_display);
            sources.push(hunk.source.clone());
            expanded.push(hunk.expanded);
        }
        DiffDisplay {
            hunks,
            old_ranges,
            sources,
            expanded,
            word_diffs,
        }
    }

    /// 编辑后按当前组合映射重算显示坐标，不重新物化 excerpt。
    ///
    /// `apply_source_change` 增量更新了 excerpt 映射；
    /// 显示坐标必须同步刷新，否则版本门控会让全部 diff 高亮消失，直到下一次整体重建。
    pub(crate) fn refresh_diff_display(&mut self, cx: &mut Context<Self>) {
        if self.diff.is_none() {
            return;
        }
        if self.diffs.iter().all(|file| file.materialized.is_empty()) {
            return;
        }
        let display =
            self.derive_diff_display(self.diffs.iter().flat_map(|file| file.materialized.iter()));
        let diff = self.diff.as_mut().expect("已确认 diff 投影存在");
        diff.display_hunks = display.hunks;
        diff.display_old_ranges = display.old_ranges;
        diff.display_sources = display.sources;
        diff.display_expanded = display.expanded;
        diff.display_word_diffs = display.word_diffs;
        cx.notify();
    }

    /// 一个物化 hunk 的词级片段在组合文档中的字节范围。
    ///
    /// 旧侧 word diff 相对 base 字节范围起点，新侧 anchor 归于 working 源；
    /// 两者都经各自 excerpt 的源码→组合偏移映射换算到组合文档坐标。
    fn combined_word_diffs(&self, hunk: &MaterializedHunk) -> Vec<(DiffHunkKind, Range<usize>)> {
        let mut word_diffs = Vec::new();
        // 只有展开的旧侧才真正物化 base 行；折叠态是 working 坐标的占位片段，不能套用 base 偏移。
        if hunk.expanded
            && let Some(excerpt) = hunk.old_excerpt
        {
            let mapping = self
                .mapping_for_excerpt_anchor(excerpt)
                .expect("旧侧 diff excerpt 必须存在对应组合映射");
            let output_start = mapping.output_range.start().get();
            let source_start = mapping.source_range.start().get();
            word_diffs.extend(hunk.base_word_diffs.iter().map(|diff| {
                let start = output_start + hunk.base_byte_start + diff.start - source_start;
                let end = output_start + hunk.base_byte_start + diff.end - source_start;
                (DiffHunkKind::Deleted, start..end)
            }));
        }
        if let MaterializedHunkLocation::Excerpt(excerpt) = &hunk.new_location {
            let mapping = self
                .mapping_for_excerpt_anchor(*excerpt)
                .expect("新侧 diff excerpt 必须存在对应组合映射");
            let output_start = mapping.output_range.start().get();
            let source_start = mapping.source_range.start().get();
            word_diffs.extend(hunk.buffer_word_diffs.iter().map(|diff| {
                let start = output_start + diff.start.offset().get() - source_start;
                let end = output_start + diff.end.offset().get() - source_start;
                (DiffHunkKind::Added, start..end)
            }));
        }
        word_diffs
    }

    /// 根据源文档边界定位当前组合文档中的逻辑行。
    fn diff_excerpt_boundary_line(&self, boundary: SourceBoundary) -> usize {
        let mut cursor = MultiBufferCursor::new(&self.state.excerpts, &self.state.diff_transforms);
        cursor.seek_output(ByteOffset::ZERO, sum_tree::Bias::Right);
        let mut previous_end = None;
        while let Some((excerpt, _)) = cursor.item() {
            if excerpt.source_id == boundary.source_id {
                let start = excerpt.source_range.start();
                let end = excerpt.source_range.end();
                if boundary.offset < start {
                    return cursor.start().lines;
                }
                if boundary.offset == start || (start == end && boundary.offset == end) {
                    return cursor.start().lines;
                }
                if boundary.offset < end {
                    let source_line = self.state.sources[excerpt.source_index]
                        .text
                        .byte_to_line(boundary.offset)
                        .map_or(excerpt.source_start_line, |line| line.get());
                    return cursor.start().lines
                        + source_line.saturating_sub(excerpt.source_start_line);
                }
                if boundary.offset == end {
                    previous_end = Some(
                        (cursor.start().lines + excerpt.line_span).max(cursor.start().lines + 1),
                    );
                }
            }
            cursor.next();
        }
        previous_end.unwrap_or(0)
    }
}

/// 解析一个文件当前的可见 hunk（pending 抑制后）为显示行坐标。
fn resolve_file_hunks(file: &DiffState, cx: &App) -> Vec<ResolvedHunk> {
    let entity = file.diff.clone();
    let (working_text, base_text, hunks) = {
        let diff = entity.read(cx);
        let working_text = diff.working().read(cx).text_snapshot(cx);
        let base_text = diff
            .base_source()
            .map(|base| base.read(cx).text_snapshot(cx));
        (working_text, base_text, diff.snapshot().visible_hunks())
    };
    hunks
        .iter()
        .map(|hunk| resolve_hunk(hunk, &working_text, base_text.as_ref()))
        .collect()
}

/// 把 anchor hunk 展开为显示层需要的行坐标与旧侧字节范围。
fn resolve_hunk(hunk: &DiffHunk, working: &Snapshot, base: Option<&Snapshot>) -> ResolvedHunk {
    let buffer_lines = line_at_or_end(working, hunk.buffer_range.start.offset())
        ..line_at_or_end(working, hunk.buffer_range.end.offset());
    let base_lines = base.map_or(0..0, |base| {
        line_at_or_end(base, ByteOffset::new(hunk.diff_base_byte_range.start))
            ..line_at_or_end(base, ByteOffset::new(hunk.diff_base_byte_range.end))
    });
    ResolvedHunk {
        buffer_range: hunk.buffer_range.clone(),
        buffer_lines,
        base_lines,
        kind: hunk.kind,
        staging: hunk.staging,
        base_byte_start: hunk.diff_base_byte_range.start,
        buffer_word_diffs: hunk.buffer_word_diffs.clone(),
        base_word_diffs: hunk.base_word_diffs.clone(),
    }
}

/// 字节偏移所在行；偏移等于文本末尾（或多字节边界之外）时取 line_count。
fn line_at_or_end(text: &Snapshot, offset: ByteOffset) -> usize {
    text.byte_to_line(offset)
        .map_or_else(|_| text.line_count(), |line| line.get())
}

impl DiffExpansionState {
    fn is_expanded(
        &self,
        kind: DiffHunkKind,
        old_range: &Range<usize>,
        expanded_by_default: bool,
    ) -> bool {
        self.override_for(kind, old_range)
            .map_or(expanded_by_default, |over| over.expanded)
    }

    /// 切换展开/折叠；结果作为显式覆盖记录，后续刷新按锚点迁移。
    fn toggle(&mut self, kind: DiffHunkKind, old_range: &Range<usize>, expanded_by_default: bool) {
        let expanded = !self.is_expanded(kind, old_range, expanded_by_default);
        match self
            .overrides
            .iter_mut()
            .find(|over| over.kind == kind && over.old_range == *old_range)
        {
            Some(over) => over.expanded = expanded,
            None => self.overrides.push(HunkExpansionOverride {
                kind,
                old_range: old_range.clone(),
                expanded,
            }),
        }
    }

    fn override_for(
        &self,
        kind: DiffHunkKind,
        old_range: &Range<usize>,
    ) -> Option<&HunkExpansionOverride> {
        self.overrides
            .iter()
            .find(|over| over.kind == kind && over.old_range == *old_range)
    }

    /// 只保留仍能对应到当前 hunk 的显式覆盖。
    fn retain_for_current_hunks(&mut self, hunks: &[ResolvedHunk]) {
        self.overrides.retain(|over| {
            hunks.iter().any(|hunk| {
                hunk.kind == over.kind && base_ranges_correspond(&hunk.base_lines, &over.old_range)
            })
        });
    }
}

/// 两个旧侧行范围是否指向同一个 hunk。
///
/// 新增块的位置是空范围（插入点），按点包含匹配；删除/修改按区间相交匹配。
fn base_ranges_correspond(a: &Range<usize>, b: &Range<usize>) -> bool {
    if a.is_empty() || b.is_empty() {
        let point = if a.is_empty() { a.start } else { b.start };
        let range = if a.is_empty() { b } else { a };
        range.start <= point && point <= range.end
    } else {
        a.start < b.end && b.start < a.end
    }
}

/// 按工作区锚点把显示层展开/折叠状态迁移到新的 diff 结果。
///
/// 显式覆盖只在对应到同一 hunk 时随位置迁移；hunk 身份变化时回落到展开策略默认值。
fn migrate_expansion_state(
    old: &[ResolvedHunk],
    old_expansion: &DiffExpansionState,
    new: &[ResolvedHunk],
    expansion: &mut DiffExpansionState,
) {
    let use_anchors = !old.is_empty() && !new.is_empty();
    let corresponds = |old_hunk: &ResolvedHunk, new_hunk: &ResolvedHunk| {
        if use_anchors {
            old_hunk.buffer_range.start.version() == new_hunk.buffer_range.start.version()
                && old_hunk.buffer_range.start.offset() == new_hunk.buffer_range.start.offset()
        } else {
            old_hunk.kind == new_hunk.kind
                && old_hunk.base_lines.start < new_hunk.base_lines.end
                && new_hunk.base_lines.start < old_hunk.base_lines.end
        }
    };
    for new_hunk in new {
        let Some(old_hunk) = old.iter().find(|old_hunk| corresponds(old_hunk, new_hunk)) else {
            continue;
        };
        let Some(over) = old_expansion.override_for(old_hunk.kind, &old_hunk.base_lines) else {
            continue;
        };
        expansion.overrides.push(HunkExpansionOverride {
            kind: new_hunk.kind,
            old_range: new_hunk.base_lines.clone(),
            expanded: over.expanded,
        });
    }
}

/// 把单个文件的可见行物化为 excerpts，并派生显示坐标 hunks。
fn materialize_file(
    file_index: usize,
    file: &DiffState,
    expansion: &DiffExpansionState,
    cx: &App,
    expanded_by_default: bool,
    excerpts: &mut Vec<ExcerptRange>,
    materialized_hunks: &mut Vec<MaterializedHunk>,
) {
    let resolved = resolve_file_hunks(file, cx);
    let working = file.diff.read(cx).working().clone();
    let base_source = file.diff.read(cx).base_source().cloned();
    let is_created = file.diff.read(cx).is_created();
    let working_text = working.read(cx).text_snapshot(cx);
    let line_count = working_text.line_count();
    let display_path = file.display_path.clone();
    let context_lines = file.context_lines;
    let show_file_header = file.show_file_header;
    let mut materializer = ExcerptMaterializer {
        excerpts,
        display_path: display_path.as_path(),
    };

    // 整文件新增：整个新侧文件作为 Added 显示（无旧侧）。
    if is_created && resolved.is_empty() {
        let new_excerpt = materializer
            .push(
                0..line_count,
                &working_text,
                &working,
                Some(ExcerptDiffKind::Added),
                show_file_header,
                false,
            )
            .expect("整文件新增投影必须生成 excerpt");
        materialized_hunks.push(MaterializedHunk {
            old_range: 0..0,
            kind: DiffHunkKind::Added,
            staging: DiffHunkStaging::NoStaging,
            old_excerpt: None,
            new_location: MaterializedHunkLocation::Excerpt(new_excerpt),
            source: DisplayHunkSource {
                file_index,
                hunk_index: None,
            },
            expanded: true,
            base_byte_start: 0,
            buffer_word_diffs: Vec::new(),
            base_word_diffs: Vec::new(),
        });
        return;
    }
    // 无行级差异：整文件模式显示整个新侧文件（空文件保留占位行），裁剪模式不显示。
    if resolved.is_empty() {
        if context_lines.is_none() {
            let _ = materializer.push(
                0..line_count,
                &working_text,
                &working,
                None,
                show_file_header,
                true,
            );
        }
        return;
    }

    let visible = match context_lines {
        None => std::iter::once(0..line_count).collect::<Vec<_>>(),
        Some(context) => excerpt_line_ranges(&resolved, line_count, context),
    };
    for context_range in visible {
        let mut current = context_range.start;
        // 文件标题块只在宿主声明时创建（ProjectDiffView 多文件投影；普通编辑器整文件不创建）。
        let mut starts_new_excerpt = show_file_header;
        for (hunk_index, hunk) in resolved
            .iter()
            .enumerate()
            .filter(|(_, hunk)| hunk_is_inside_excerpt(hunk, &context_range))
        {
            if current < hunk.buffer_lines.start {
                let _ = materializer.push(
                    current..hunk.buffer_lines.start,
                    &working_text,
                    &working,
                    None,
                    starts_new_excerpt,
                    false,
                );
                starts_new_excerpt = false;
            }
            // 旧侧：展开时物化完整旧行；裁剪模式折叠时用空占位行标记删除点。
            let old_display = if !hunk.base_lines.is_empty() {
                if expansion.is_expanded(hunk.kind, &hunk.base_lines, expanded_by_default)
                    && let Some(base) = base_source.as_ref()
                {
                    let base_text = base.read(cx).text_snapshot(cx);
                    let old_excerpt = materializer
                        .push(
                            hunk.base_lines.clone(),
                            &base_text,
                            base,
                            Some(ExcerptDiffKind::Deleted),
                            starts_new_excerpt,
                            false,
                        )
                        .expect("展开的旧侧投影必须生成 excerpt");
                    starts_new_excerpt = false;
                    Some(old_excerpt)
                } else if context_lines.is_some() {
                    // 折叠占位行：空 Deleted 片段（组合文档为它保留一个显示行）。
                    let base = base_source.as_ref().expect("删除点占位需要 base 来源");
                    let base_text = base.read(cx).text_snapshot(cx);
                    let old_excerpt = materializer
                        .push(
                            hunk.base_lines.start..hunk.base_lines.start,
                            &base_text,
                            base,
                            Some(ExcerptDiffKind::Deleted),
                            starts_new_excerpt,
                            true,
                        )
                        .expect("折叠的旧侧占位必须生成 excerpt");
                    starts_new_excerpt = false;
                    Some(old_excerpt)
                } else {
                    None
                }
            } else {
                None
            };
            // 新侧：可编辑 excerpt；纯删除 hunk 用空范围锚定到删除点。
            let new_location = if !hunk.buffer_lines.is_empty() {
                let new_excerpt = materializer
                    .push(
                        hunk.buffer_lines.clone(),
                        &working_text,
                        &working,
                        Some(ExcerptDiffKind::Added),
                        starts_new_excerpt,
                        false,
                    )
                    .expect("非空新侧投影必须生成 excerpt");
                starts_new_excerpt = false;
                MaterializedHunkLocation::Excerpt(new_excerpt)
            } else {
                MaterializedHunkLocation::Boundary(SourceBoundary {
                    source_id: working.entity_id(),
                    offset: hunk.buffer_range.start.offset(),
                })
            };
            materialized_hunks.push(MaterializedHunk {
                old_range: hunk.base_lines.clone(),
                kind: hunk.kind,
                staging: hunk.staging,
                old_excerpt: old_display,
                new_location,
                source: DisplayHunkSource {
                    file_index,
                    hunk_index: Some(hunk_index),
                },
                expanded: expansion.is_expanded(hunk.kind, &hunk.base_lines, expanded_by_default),
                base_byte_start: hunk.base_byte_start,
                buffer_word_diffs: hunk.buffer_word_diffs.clone(),
                base_word_diffs: hunk.base_word_diffs.clone(),
            });
            current = hunk.buffer_lines.end;
        }
        if current < context_range.end {
            let _ = materializer.push(
                current..context_range.end,
                &working_text,
                &working,
                None,
                starts_new_excerpt,
                false,
            );
        }
    }
}

/// 构造一个投影片段（空行策略由 allow_empty 控制：占位行允许空源范围）。
fn projected_excerpt(
    source: &Entity<LanguageBuffer>,
    text: &Snapshot,
    lines: Range<usize>,
    display_path: &Path,
    diff_kind: Option<ExcerptDiffKind>,
    starts_new_excerpt: bool,
    allow_empty: bool,
) -> Option<ExcerptRange> {
    if lines.is_empty() && !allow_empty {
        return None;
    }
    let mut excerpt = ExcerptRange::line_range_from_text(source.clone(), text, lines);
    // 空源范围的普通片段没有可显示内容：跳过（deleted 文件的占位上下文等）。
    // 整文件显示（allow_empty）保留占位行，diff 片段（旧侧/新增）始终物化。
    if excerpt.source_range().is_empty() && !allow_empty && diff_kind.is_none() {
        return None;
    }
    excerpt = excerpt
        .with_display_path(display_path.to_path_buf())
        .with_starts_new_excerpt(starts_new_excerpt)
        .with_editable(diff_kind != Some(ExcerptDiffKind::Deleted));
    if let Some(diff_kind) = diff_kind {
        excerpt = excerpt.with_diff_kind(diff_kind);
    }
    Some(excerpt)
}

fn excerpt_line_ranges(
    hunks: &[ResolvedHunk],
    line_count: usize,
    context_lines: usize,
) -> Vec<Range<usize>> {
    let max_line = line_count.saturating_sub(1);
    let mut ranges = hunks
        .iter()
        .map(|hunk| {
            let start = hunk
                .buffer_lines
                .start
                .min(max_line)
                .saturating_sub(context_lines);
            // Zcv 的行范围右开；
            // 非空 hunk 先换算为最后一条变更行，才能得到真正的后两行上下文。
            let changed_end_line = if hunk.buffer_lines.is_empty() {
                hunk.buffer_lines.start
            } else {
                hunk.buffer_lines.end.saturating_sub(1)
            };
            let end_line = changed_end_line.saturating_add(context_lines).min(max_line);
            start..end_line + 1
        })
        .collect::<Vec<_>>();
    ranges.sort_by_key(|range| range.start);

    let mut merged = Vec::<Range<usize>>::new();
    for range in ranges {
        if let Some(previous) = merged.last_mut()
            && range.start <= previous.end
        {
            previous.end = previous.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

fn hunk_is_inside_excerpt(hunk: &ResolvedHunk, excerpt: &Range<usize>) -> bool {
    if hunk.buffer_lines.is_empty() {
        excerpt.contains(&hunk.buffer_lines.start)
    } else {
        hunk.buffer_lines.start >= excerpt.start && hunk.buffer_lines.end <= excerpt.end
    }
}

/// 把列（Unicode scalar 计数）钳制到文本中指定行的有效长度（行 0-based）。
fn clamp_column_to_line(text: &Snapshot, line: usize, column: usize) -> usize {
    let line = line.min(text.line_count().saturating_sub(1));
    let line_chars = text
        .line_content(Line::new(line), None)
        .map_or(0, |content| content.len_chars());
    column.min(line_chars)
}
