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
use std::sync::Arc;

use gpui::{App, AppContext as _, Context, Entity, Subscription};
use zcv_git::DiffHunkKind;
use zcv_language::LanguageBuffer;
use zcv_text::{Anchor, BufferVersion, ByteOffset, Line, Snapshot};

use crate::buffer_diff::{BufferDiff, BufferDiffEvent, BufferDiffInput, DiffHunk};
use crate::{
    ExcerptDiffKind, ExcerptMapping, MultiBuffer, MultiBufferEvent, MultiBufferExcerpt,
    ProjectionRemap,
};

/// 编辑器投影使用的显示 hunk（组合文档行坐标）。
///
/// `range` 与 `old_range` 都是组合文档中的逻辑行范围；源文本定位由 `DiffHunkSource` 提供。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DisplayHunk {
    pub range: Range<usize>,
    pub old_range: Range<usize>,
    pub kind: DiffHunkKind,
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
pub(crate) struct DiffFileProjection {
    diff: Entity<BufferDiff>,
    /// 组合文档中的显示路径（文件标题与导航定位）。
    display_path: PathBuf,
    /// 显示策略：None 显示整个新侧文件；Some(n) 只显示 hunk 周围 n 行上下文。
    context_lines: Option<usize>,
    /// 该文件的第一个可见片段是否创建文件标题块。
    show_file_header: bool,
}

/// 一个 MultiBuffer 的 git diff 投影状态。
#[derive(Default)]
pub(crate) struct MultiBufferDiffProjection {
    /// 每个文件的投影状态（顺序 = 组合文档中的显示顺序）。
    files: Vec<DiffFileProjection>,
    /// 显示层拥有的展开/折叠状态，与版本化 diff 结果分离。
    expansion: Vec<DiffExpansionState>,
    /// 新 hunk 的初始展开策略；只决定初始状态，不覆盖用户显式切换。
    expanded_by_default: bool,
    /// 显示坐标 hunks（组合坐标，跨文件展平）。
    display_hunks: Vec<DisplayHunk>,
    /// 每个 hunk 在组合文档中的旧侧显示行范围；折叠态或 Added hunk 为 None。
    display_old_ranges: Vec<Option<Range<usize>>>,
    /// 显示 hunk 对应的源文件与源 hunk；整文件新增块没有源 hunk。
    display_sources: Vec<DisplayHunkSource>,
    /// 与显示 hunk 同序的展开状态。
    display_expanded: Vec<bool>,
    /// 显示坐标对应的组合文档版本（注入/重建后发生编辑会使坐标失效）。
    display_version: Option<BufferVersion>,
    /// 对每个 BufferDiff 的订阅：diff 结果或 pending 变化时重新物化显示。
    subscriptions: Vec<Subscription>,
    /// 上次物化时各 BufferDiff 的版本；用于抑制已同步重建后的重复事件。
    display_revisions: Vec<u64>,
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
}

/// 一个 hunk 在本次物化出的 excerpt 序列中的位置。
///
/// 最终组合行坐标只能在 `MultiBuffer::set_excerpts` 建立实际映射后派生；
/// 这里保存 excerpt 身份或 excerpt 边界，不平行累计另一份组合行号。
struct MaterializedHunk {
    old_range: Range<usize>,
    kind: DiffHunkKind,
    old_excerpt: Option<usize>,
    new_location: MaterializedHunkLocation,
    source: DisplayHunkSource,
    expanded: bool,
}

enum MaterializedHunkLocation {
    Excerpt(usize),
    Boundary(usize),
}

struct ExcerptMaterializer<'a> {
    excerpts: &'a mut Vec<MultiBufferExcerpt>,
    display_path: &'a Path,
}

impl ExcerptMaterializer<'_> {
    fn push(
        &mut self,
        lines: Range<usize>,
        text: &zcv_text::Snapshot,
        source: &Entity<LanguageBuffer>,
        diff_kind: Option<ExcerptDiffKind>,
        starts_new_excerpt: bool,
        allow_empty: bool,
    ) -> Option<usize> {
        let excerpt = projected_excerpt(
            source,
            text,
            lines,
            self.display_path,
            diff_kind,
            starts_new_excerpt,
            allow_empty,
        )?;
        let index = self.excerpts.len();
        self.excerpts.push(excerpt);
        Some(index)
    }

    fn boundary(&self) -> usize {
        self.excerpts.len()
    }
}

impl MultiBuffer {
    /// 统一注入 git diff 投影（普通编辑器与多文件投影共用）。
    ///
    /// None 是加载态（新 diff 尚未算完），保留现有 hunks 与用户展开状态；
    /// Some 注入后按文本跟踪区间迁移展开状态并重建投影。
    /// 返回 true 表示组合文档被重建（调用方应重置光标）。
    pub fn set_buffer_diffs(
        &mut self,
        files: Option<Vec<BufferDiffInput>>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(inputs) = files else {
            return false;
        };
        let old_files = self
            .diff
            .as_mut()
            .map(|diff| std::mem::take(&mut diff.files));
        let old_expansion = self
            .diff
            .as_mut()
            .map(|diff| std::mem::take(&mut diff.expansion));
        let diff = self
            .diff
            .get_or_insert_with(|| Box::new(MultiBufferDiffProjection::default()));

        // 同一 working + base 的既有实体直接复用：pending 与展开状态跨刷新存活。
        let mut next_files = Vec::with_capacity(inputs.len());
        for input in inputs {
            let reused = old_files.as_deref().and_then(|old_files| {
                old_files
                    .iter()
                    .find(|old| {
                        let old = old.diff.read(cx);
                        old.working().entity_id() == input.working.entity_id()
                            && old.base_text() == input.base_text
                    })
                    .map(|old| old.diff.clone())
            });
            let display_path = input.display_path.clone();
            let context_lines = input.context_lines;
            let show_file_header = input.show_file_header;
            let operations = input.operations.clone();
            let entity = match reused {
                Some(entity) => {
                    entity.update(cx, |diff, _| diff.set_operations(operations));
                    entity
                }
                None => cx.new(|cx| BufferDiff::new(input, cx)),
            };
            next_files.push(DiffFileProjection {
                diff: entity,
                display_path,
                context_lines,
                show_file_header,
            });
        }

        let mut next_expansion = Vec::with_capacity(next_files.len());
        for file in &next_files {
            let mut expansion = DiffExpansionState::default();
            if let (Some(old_files), Some(old_expansion)) =
                (old_files.as_deref(), old_expansion.as_deref())
                && let Some((old_file, old_state)) =
                    old_files.iter().zip(old_expansion.iter()).find(|(old, _)| {
                        old.diff.read(cx).working().entity_id()
                            == file.diff.read(cx).working().entity_id()
                    })
            {
                let old_resolved = resolve_file_hunks(old_file, cx);
                let new_resolved = resolve_file_hunks(file, cx);
                migrate_expansion_state(&old_resolved, old_state, &new_resolved, &mut expansion);
            }
            next_expansion.push(expansion);
        }
        diff.subscriptions = next_files
            .iter()
            .map(|file| {
                cx.subscribe(&file.diff, |this, _, event, cx| {
                    if matches!(event, BufferDiffEvent::DiffChanged) {
                        this.diff_changed(cx);
                    }
                })
            })
            .collect();
        diff.files = next_files;
        diff.expansion = next_expansion;
        !self.rebuild_diff_projection(cx).is_identity()
    }

    /// 设置新 hunk 的初始展开策略；用户之后的显式展开/折叠不受投影刷新覆盖。
    ///
    /// 折叠/展开不改变源，编辑器源锚点选区自然存活，无需返回投影重映射。
    pub fn set_diff_hunks_expanded_by_default(&mut self, expanded: bool, cx: &mut Context<Self>) {
        let diff = self
            .diff
            .get_or_insert_with(|| Box::new(MultiBufferDiffProjection::default()));
        if diff.expanded_by_default == expanded {
            return;
        }
        diff.expanded_by_default = expanded;
        // 策略切换不迁移旧状态：按新默认值重新应用（清空全部显式集合）。
        diff.expansion = vec![DiffExpansionState::default(); diff.files.len()];
        self.rebuild_diff_projection(cx);
        cx.emit(MultiBufferEvent::DiffExpansionChanged);
    }

    /// 按显示 hunk 索引切换展开/折叠（渲染层点击入口）。
    ///
    /// 折叠/展开不改变源，编辑器源锚点选区自然存活，无需返回投影重映射。
    pub fn toggle_diff_hunk_at(&mut self, display_index: usize, cx: &mut Context<Self>) {
        let expanded_by_default = self
            .diff
            .as_ref()
            .is_some_and(|diff| diff.expanded_by_default);
        let mut toggled = false;
        if let Some(diff) = &mut self.diff
            && let Some(source) = diff.display_sources.get(display_index)
            && let Some(hunk) = diff.display_hunks.get(display_index)
            && let Some(expansion) = diff.expansion.get_mut(source.file_index)
        {
            expansion.toggle(hunk.kind, &hunk.old_range, expanded_by_default);
            toggled = true;
        }
        if !toggled {
            return;
        }
        self.rebuild_diff_projection(cx);
        cx.emit(MultiBufferEvent::DiffExpansionChanged);
    }

    /// base 版本变化（HEAD 变化等）后由宿主调用：旧侧坐标空间已失效，按默认策略重置展开状态。
    ///
    /// 折叠/展开不改变源，编辑器源锚点选区自然存活，无需返回投影重映射。
    pub fn reset_diff_hunk_expansion_state(&mut self, cx: &mut Context<Self>) {
        let Some(diff) = &mut self.diff else {
            return;
        };
        diff.expansion = vec![DiffExpansionState::default(); diff.files.len()];
        self.rebuild_diff_projection(cx);
        cx.emit(MultiBufferEvent::DiffExpansionChanged);
    }

    /// 与当前组合文档版本匹配的显示坐标 hunks；未注入、加载态或注入后发生编辑时返回空。
    pub fn diff_hunks<'a>(&'a self, cx: &'a App) -> &'a [DisplayHunk] {
        match self.display_state(cx) {
            Some(diff) => &diff.display_hunks,
            None => &[],
        }
    }

    /// 每个 hunk 在组合文档中的旧侧显示行范围（与 MultiBuffer::diff_hunks 同门控）。
    pub fn diff_hunk_old_ranges<'a>(&'a self, cx: &'a App) -> &'a [Option<Range<usize>>] {
        match self.display_state(cx) {
            Some(diff) => &diff.display_old_ranges,
            None => &[],
        }
    }

    /// 与 MultiBuffer::diff_hunks 平行的展开标志（渲染层按显示 hunk 索引查询）。
    pub fn diff_hunk_expanded(&self, cx: &App) -> Vec<bool> {
        let Some(diff) = self.display_state(cx) else {
            return Vec::new();
        };
        diff.display_expanded.clone()
    }

    /// 显示 hunk 到源定位（hunk 操作与导航用）。
    pub fn buffer_diff_hunk_at(&self, display_index: usize, cx: &App) -> Option<DiffHunkSource> {
        let diff = self.display_state(cx)?;
        let source = diff.display_sources.get(display_index)?.clone();
        let file = diff.files.get(source.file_index)?;
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

    /// 某文件的旧侧（base 修订）全文；未注入或该文件无旧侧时返回 None。
    pub fn diff_base_text(&self, path: &Path, cx: &App) -> Option<Arc<str>> {
        let diff = self.diff.as_ref()?;
        diff.files
            .iter()
            .find(|file| file.diff.read(cx).path() == path)
            .and_then(|file| file.diff.read(cx).base_text())
    }

    /// 把打开请求中的 Deleted 片段换算为工作区文件中的合法定位行列（0-based）。
    ///
    /// Deleted 片段的内容来自 Git 修订文本，其字节坐标在打开的工作区文件中不存在；
    /// 经 hunk 把修订侧行号映射到工作区（新侧）行号，列沿用修订行内逻辑列，行与列都按工作区文件文本钳制到有效范围，返回值可直接用于行列导航。
    /// 非 Deleted 片段返回 None（坐标直接可用）。
    pub fn deleted_navigation_target(
        &self,
        location: &crate::ExcerptLocation,
        working_text: &zcv_text::Snapshot,
        cx: &App,
    ) -> Option<(usize, usize)> {
        let snapshot = self.snapshot(cx);
        // 仅处理 Deleted 片段：修订文本坐标需换算，其余片段直接可用。
        let in_deleted_excerpt = snapshot.excerpts().iter().any(|excerpt| {
            excerpt.path() == location.path
                && excerpt.diff_kind() == Some(ExcerptDiffKind::Deleted)
                && excerpt.source_range().start() <= location.source_range.start()
                && location.source_range.end() <= excerpt.source_range().end()
        });
        if !in_deleted_excerpt {
            return None;
        }
        // 修订文本行号与列（列按 Unicode scalar 计数，与导航协议一致）。
        let diff = self.diff.as_ref()?;
        let file = diff
            .files
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
        self.diff.as_ref().is_some_and(|diff| {
            diff.files
                .iter()
                .any(|file| file.diff.read(cx).working().entity_id() == source_id)
        })
    }

    /// BufferDiff 事件入口：只有当前物化结果落后于 diff 版本时才重建。
    fn diff_changed(&mut self, cx: &mut Context<Self>) {
        let Some(diff) = &self.diff else {
            return;
        };
        let stale = diff.files.len() != diff.display_revisions.len()
            || diff
                .files
                .iter()
                .zip(diff.display_revisions.iter())
                .any(|(file, revision)| file.diff.read(cx).revision() != *revision);
        if !stale {
            return;
        }
        // 先丢弃已消失 hunk 的展开覆盖，再重物化。
        let resolved = diff
            .files
            .iter()
            .map(|file| resolve_file_hunks(file, cx))
            .collect::<Vec<_>>();
        let diff = self.diff.as_mut().expect("已确认 diff 投影存在");
        for (expansion, hunks) in diff.expansion.iter_mut().zip(&resolved) {
            expansion.retain_for_current_hunks(hunks);
        }
        self.rebuild_diff_projection(cx);
    }

    /// 统一物化：按展开状态与显示策略把每个文件的可见行物化为 excerpts，并派生显示坐标 hunks。
    ///
    /// 返回本次重建的投影坐标重映射：
    /// 投影版本未变时恒等，变化时携带重建前的投影→源映射，供调用方把重建前的光标经源忠实落到重建后投影（reload 会重裁剪并重置版本，裸偏移不再有效）。
    pub(crate) fn rebuild_diff_projection(&mut self, cx: &mut Context<Self>) -> ProjectionRemap {
        let before = self.state.mappings.clone();
        self.rebuild_diff_projection_from(before, cx)
    }

    /// 按调用方在 hunk 生命周期变更前冻结的投影映射重建。
    ///
    /// 编辑坐标恢复属于投影事务，而非 hunk 刷新：调用方在源文本已更新、hunk
    /// 尚未重物化时保存映射，随后无论 hunk 怎样裁剪或失效，都用该映射解析编辑后的选区。
    pub(crate) fn rebuild_diff_projection_from(
        &mut self,
        before: Vec<ExcerptMapping>,
        cx: &mut Context<Self>,
    ) -> ProjectionRemap {
        if self.diff.is_none() {
            return ProjectionRemap::identity();
        }
        let old_version = self.text_buffer(cx).read(cx).snapshot().version();
        let diff = self.diff.as_mut().expect("已确认 diff 投影存在");
        let expanded_by_default = diff.expanded_by_default;
        let mut excerpts = Vec::new();
        let mut materialized_hunks = Vec::new();
        for (file_index, file) in diff.files.iter().enumerate() {
            let resolved = resolve_file_hunks(file, cx);
            materialize_file(
                file_index,
                file,
                &resolved,
                diff.expansion
                    .get(file_index)
                    .expect("展开状态必须与文件一一对应"),
                cx,
                expanded_by_default,
                &mut excerpts,
                &mut materialized_hunks,
            );
        }
        let expected_excerpt_count = excerpts.len();
        self.set_excerpts(excerpts, cx);
        assert_eq!(
            self.state.mappings.len(),
            expected_excerpt_count,
            "diff 物化生成的 excerpt 必须全部建立组合映射"
        );
        let mut display_hunks = Vec::with_capacity(materialized_hunks.len());
        let mut display_old_ranges = Vec::with_capacity(materialized_hunks.len());
        let mut display_sources = Vec::with_capacity(materialized_hunks.len());
        let mut display_expanded = Vec::with_capacity(materialized_hunks.len());
        for hunk in materialized_hunks {
            let old_display = hunk
                .old_excerpt
                .map(|excerpt| self.diff_excerpt_output_lines(excerpt));
            let new_range = match hunk.new_location {
                MaterializedHunkLocation::Excerpt(excerpt) => {
                    self.diff_excerpt_output_lines(excerpt)
                }
                MaterializedHunkLocation::Boundary(boundary) => {
                    let line = self.diff_excerpt_boundary_line(boundary);
                    line..line
                }
            };
            display_hunks.push(DisplayHunk {
                range: new_range,
                old_range: hunk.old_range,
                kind: hunk.kind,
            });
            display_old_ranges.push(old_display);
            display_sources.push(hunk.source);
            display_expanded.push(hunk.expanded);
        }
        let new_version = self.text_buffer(cx).read(cx).snapshot().version();
        let diff = self.diff.as_mut().expect("投影重建前 diff 状态必须存在");
        diff.display_hunks = display_hunks;
        diff.display_old_ranges = display_old_ranges;
        diff.display_sources = display_sources;
        diff.display_expanded = display_expanded;
        diff.display_version = Some(new_version);
        diff.display_revisions = diff
            .files
            .iter()
            .map(|file| file.diff.read(cx).revision())
            .collect();
        cx.notify();
        if new_version != old_version {
            ProjectionRemap::rebuilt(before)
        } else {
            ProjectionRemap::identity()
        }
    }

    /// diff 片段在最终组合文档中的真实逻辑行范围。
    /// 空片段仍对应编辑器中的一个空逻辑行。
    fn diff_excerpt_output_lines(&self, excerpt: usize) -> Range<usize> {
        let mapping = self
            .state
            .mappings
            .get(excerpt)
            .expect("diff excerpt 必须存在对应组合映射");
        mapping.output_start_line..mapping.output_end_line.max(mapping.output_start_line + 1)
    }

    /// excerpt 序列边界在最终组合文档中的真实逻辑行。
    fn diff_excerpt_boundary_line(&self, boundary: usize) -> usize {
        if let Some(next) = self.state.mappings.get(boundary) {
            next.output_start_line
        } else if let Some(previous) = boundary
            .checked_sub(1)
            .and_then(|index| self.state.mappings.get(index))
        {
            previous.output_end_line.max(previous.output_start_line + 1)
        } else {
            0
        }
    }

    /// 显示坐标只在组合文档未被后续编辑时有效（版本门控）。
    fn display_state<'a>(&'a self, cx: &'a App) -> Option<&'a MultiBufferDiffProjection> {
        let diff = self.diff.as_ref()?;
        (diff.display_version == Some(self.text_buffer(cx).read(cx).snapshot().version()))
            .then_some(diff)
    }
}

/// 解析一个文件当前的可见 hunk（pending 抑制后）为显示行坐标。
fn resolve_file_hunks(file: &DiffFileProjection, cx: &App) -> Vec<ResolvedHunk> {
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
    file: &DiffFileProjection,
    resolved: &[ResolvedHunk],
    expansion: &DiffExpansionState,
    cx: &App,
    expanded_by_default: bool,
    excerpts: &mut Vec<MultiBufferExcerpt>,
    materialized_hunks: &mut Vec<MaterializedHunk>,
) {
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
        display_path: &display_path,
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
            old_excerpt: None,
            new_location: MaterializedHunkLocation::Excerpt(new_excerpt),
            source: DisplayHunkSource {
                file_index,
                hunk_index: None,
            },
            expanded: true,
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
        Some(context) => excerpt_line_ranges(resolved, line_count, context),
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
                MaterializedHunkLocation::Boundary(materializer.boundary())
            };
            materialized_hunks.push(MaterializedHunk {
                old_range: hunk.base_lines.clone(),
                kind: hunk.kind,
                old_excerpt: old_display,
                new_location,
                source: DisplayHunkSource {
                    file_index,
                    hunk_index: Some(hunk_index),
                },
                expanded: expansion.is_expanded(hunk.kind, &hunk.base_lines, expanded_by_default),
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
    text: &zcv_text::Snapshot,
    lines: Range<usize>,
    display_path: &Path,
    diff_kind: Option<ExcerptDiffKind>,
    starts_new_excerpt: bool,
    allow_empty: bool,
) -> Option<MultiBufferExcerpt> {
    if lines.is_empty() && !allow_empty {
        return None;
    }
    let mut excerpt = MultiBufferExcerpt::line_range_from_text(source.clone(), text, lines);
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
            // Zcv 的行范围右开；Zed 的 Point 终点位于最后一条变更行内。
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
fn clamp_column_to_line(text: &zcv_text::Snapshot, line: usize, column: usize) -> usize {
    let line = line.min(text.line_count().saturating_sub(1));
    let line_chars = text
        .line_content(Line::new(line), None)
        .map_or(0, |content| content.len_chars());
    column.min(line_chars)
}
