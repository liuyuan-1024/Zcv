//! MultiBuffer 的 git diff 投影：文件集 hunks、展开状态、跟踪区间迁移、统一物化与显示坐标派生。
//!
//! 普通编辑器与多文件投影（Git 差异视图）共用同一套物化：
//! 宿主只注入「新侧源 + 源坐标 hunks + 旧侧全文」，本层按展开状态把旧侧行物化为只读 excerpt、按显示策略裁剪可见行，并派生组合坐标显示 hunks。
//! 两种场景只差输入数据（单文件 vs 多文件）与显示策略（整文件 vs hunk 上下文），不再存在第二套物化路径。
//!
//! 展开状态跨 diff 刷新迁移：按工作区文本跟踪区间匹配新旧 hunk，文本位置未变即视为同一hunk——编辑移位、HEAD/index 变化都不丢失状态；
/// base 版本变化导致旧侧坐标失效时由宿主调用 MultiBuffer::reset_diff_hunk_expansion_state 重置。
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity};
use zcv_git::{DiffHunk, DiffHunkKind};
use zcv_language::LanguageBuffer;
use zcv_text::{BufferVersion, Line, PositionMap, Stickiness, TextRange, TrackedRange};

use crate::{ExcerptDiffKind, MultiBuffer, MultiBufferEvent, MultiBufferExcerpt};

/// 一个文件的 diff 投影输入。
///
/// 宿主（普通编辑器 / 项目差异视图）只提供数据；物化与显示坐标由本层统一完成。
#[derive(Clone)]
pub struct DiffFileInput {
    /// 新侧源（工作区文件或修订文本的语言 Buffer 实体）。
    pub working: Entity<LanguageBuffer>,
    /// 新侧源文件路径（绝对；hunk 操作与导航定位用）。
    pub path: PathBuf,
    /// 新侧坐标 hunks（working 文本行范围）；空表示无行级差异。
    pub hunks: Vec<DiffHunk>,
    /// 旧侧（base 修订）全文；None 表示没有旧侧（如整体新增文件）。
    pub base_text: Option<Arc<str>>,
    /// 组合文档中的显示路径（文件标题与导航定位）。
    pub display_path: PathBuf,
    /// 显示策略：None 显示整个新侧文件（普通编辑器）；Some(n) 只显示 hunk 周围 n 行上下文（多文件投影）。
    pub context_lines: Option<usize>,
    /// 该文件整体为新增（无 hunks 时整个文件作为 Added 显示）。
    pub is_created: bool,
    /// 该文件的第一个可见片段是否创建文件标题块。
    pub show_file_header: bool,
}

/// 显示 hunk 对应的源定位（操作与导航用）。
#[derive(Clone, Debug)]
pub struct DiffHunkSourceInfo {
    pub path: PathBuf,
    pub source: Option<DiffHunk>,
}

/// 一个 MultiBuffer 的 git diff 投影状态。
#[derive(Default)]
pub(crate) struct DiffProjection {
    /// 每个文件的投影状态（顺序 = 组合文档中的显示顺序）。
    files: Vec<DiffFileProjection>,
    /// 新 hunk 的初始展开策略；只决定初始状态，不覆盖用户显式切换。
    expanded_by_default: bool,
    /// 显示坐标 hunks（组合坐标，跨文件展平）。
    display_hunks: Vec<DiffHunk>,
    /// 每个 hunk 在组合文档中的旧侧显示行范围；折叠态或 Added hunk 为 None。
    display_old_ranges: Vec<Option<Range<usize>>>,
    /// 显示坐标对应的组合文档版本（注入/重建后发生编辑会使坐标失效）。
    display_version: Option<BufferVersion>,
}

/// 单个文件的 diff 投影状态（含展开状态与 base 修订源）。
struct DiffFileProjection {
    working: Entity<LanguageBuffer>,
    /// 源坐标 hunks（新侧行范围；随编辑推进）。
    hunks: Vec<DiffHunk>,
    /// 与 hunks 并行的工作区源文本跟踪区间（随编辑推进）。
    ranges: Vec<TrackedRange>,
    /// 旧侧全文（物化旧侧行的数据源）。
    base_text: Option<Arc<str>>,
    /// 旧侧修订源（base_text 对应的 LanguageBuffer）。
    base_source: Option<Entity<LanguageBuffer>>,
    /// 新侧源文件路径（绝对；操作与导航定位用）。
    path: PathBuf,
    display_path: PathBuf,
    context_lines: Option<usize>,
    is_created: bool,
    show_file_header: bool,
    /// 默认折叠模式下被用户显式展开的删除/修改 hunk（按旧侧行范围标识）。
    expanded_deleted: Vec<Range<usize>>,
    expanded_modified: Vec<Range<usize>>,
    /// 默认展开模式下被用户显式折叠的 hunk。
    collapsed_deleted: Vec<Range<usize>>,
    collapsed_modified: Vec<Range<usize>>,
    /// 本文件在 display_hunks 中的起始索引。
    display_start: usize,
    /// 本文件在 display_hunks 中的数量。
    display_len: usize,
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
    pub fn set_diff_projection(
        &mut self,
        files: Option<Vec<DiffFileInput>>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(files) = files else {
            return false;
        };
        let old_files = self
            .diff
            .as_mut()
            .map(|diff| std::mem::take(&mut diff.files));
        let diff = self
            .diff
            .get_or_insert_with(|| Box::new(DiffProjection::default()));
        let expanded_by_default = diff.expanded_by_default;
        diff.files = files
            .into_iter()
            .map(|input| {
                let mut state = DiffFileProjection::new(input, cx);
                if let Some(old_files) = old_files.as_deref()
                    && let Some(old) = old_files
                        .iter()
                        .find(|old| old.working.entity_id() == state.working.entity_id())
                {
                    state.migrate_expansion_state(old, expanded_by_default);
                    if old.base_text == state.base_text {
                        state.base_source = old.base_source.clone();
                    }
                }
                state.ensure_base_source(cx);
                state
            })
            .collect();
        self.rebuild_diff_projection(cx)
    }

    /// 设置新 hunk 的初始展开策略；用户之后的显式展开/折叠不受投影刷新覆盖。
    ///
    /// 返回 true 表示组合文档被重建。
    pub fn set_diff_hunks_expanded_by_default(
        &mut self,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let diff = self
            .diff
            .get_or_insert_with(|| Box::new(DiffProjection::default()));
        if diff.expanded_by_default == expanded {
            return false;
        }
        diff.expanded_by_default = expanded;
        // 策略切换不迁移旧状态：按新默认值重新应用（清空全部显式集合）。
        for file in &mut diff.files {
            file.expanded_deleted.clear();
            file.expanded_modified.clear();
            file.collapsed_deleted.clear();
            file.collapsed_modified.clear();
        }
        let rebuilt = self.rebuild_diff_projection(cx);
        cx.emit(MultiBufferEvent::DiffExpansionChanged);
        rebuilt
    }

    /// 按显示 hunk 索引切换展开/折叠（渲染层点击入口）。
    ///
    /// 返回 true 表示组合文档被重建。
    pub fn toggle_diff_hunk_at(&mut self, display_index: usize, cx: &mut Context<Self>) -> bool {
        let expanded_by_default = self
            .diff
            .as_ref()
            .is_some_and(|diff| diff.expanded_by_default);
        let mut toggled = false;
        if let Some(diff) = &mut self.diff
            && let Some(file) = diff.files.iter_mut().find(|file| {
                display_index >= file.display_start
                    && display_index < file.display_start + file.display_len
            })
            && let Some(hunk) = file.hunks.get(display_index - file.display_start).cloned()
            && hunk.kind != DiffHunkKind::Added
        {
            file.toggle_expansion(hunk.kind, &hunk.old_range, expanded_by_default);
            toggled = true;
        }
        if !toggled {
            return false;
        }
        let rebuilt = self.rebuild_diff_projection(cx);
        cx.emit(MultiBufferEvent::DiffExpansionChanged);
        rebuilt
    }

    /// base 版本变化（HEAD 变化等）后由宿主调用：旧侧坐标空间已失效，按默认策略重置展开状态。
    ///
    /// 返回 true 表示组合文档被重建。
    pub fn reset_diff_hunk_expansion_state(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(diff) = &mut self.diff else {
            return false;
        };
        for file in &mut diff.files {
            file.expanded_deleted.clear();
            file.expanded_modified.clear();
            file.collapsed_deleted.clear();
            file.collapsed_modified.clear();
        }
        let rebuilt = self.rebuild_diff_projection(cx);
        cx.emit(MultiBufferEvent::DiffExpansionChanged);
        rebuilt
    }

    /// 与当前组合文档版本匹配的显示坐标 hunks；未注入、加载态或注入后发生编辑时返回空。
    pub fn diff_hunks<'a>(&'a self, cx: &'a App) -> &'a [DiffHunk] {
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
        diff.files
            .iter()
            .flat_map(|file| {
                file.hunks.iter().map(|hunk| {
                    file.is_expanded(hunk.kind, &hunk.old_range, diff.expanded_by_default)
                })
            })
            .collect()
    }

    /// 显示 hunk 到源定位（hunk 操作与导航用）。
    pub fn diff_hunk_source_at(
        &self,
        display_index: usize,
        cx: &App,
    ) -> Option<DiffHunkSourceInfo> {
        let diff = self.display_state(cx)?;
        let file = diff.file_for_display_index(display_index)?;
        if file.hunks.is_empty() && file.is_created {
            // 整文件新增块没有源 hunk。
            return Some(DiffHunkSourceInfo {
                path: file.path.clone(),
                source: None,
            });
        }
        let hunk = file.hunks.get(display_index - file.display_start)?.clone();
        Some(DiffHunkSourceInfo {
            path: file.path.clone(),
            source: Some(hunk),
        })
    }

    /// 某文件的旧侧（base 修订）全文；未注入或该文件无旧侧时返回 None。
    pub fn diff_base_text(&self, path: &Path) -> Option<Arc<str>> {
        let diff = self.diff.as_ref()?;
        diff.files
            .iter()
            .find(|file| file.path == path)
            .and_then(|file| file.base_text.clone())
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
        let file = diff.files.iter().find(|file| file.path == location.path)?;
        let base = file.base_source.as_ref()?;
        let base_text = base.read(cx).text_snapshot(cx);
        let position = base_text
            .byte_to_position(location.source_range.start())
            .ok()?;
        let old_line = position.line().get();
        let column = position.column().get();
        // 包含该修订行的 hunk（旧侧行范围）。
        let hunk = diff
            .display_hunks
            .iter()
            .enumerate()
            .find_map(|(index, _)| {
                let info = self.diff_hunk_source_at(index, cx)?;
                (info.path == location.path)
                    .then_some(info.source.as_ref())
                    .flatten()
                    .filter(|hunk| hunk.old_range.contains(&old_line))
                    .cloned()
            })?;
        // 修改行在 hunk 内按偏移映射；纯删除锚定变更块起点。
        let offset = old_line - hunk.old_range.start;
        let working_line = if hunk.range.is_empty() {
            hunk.range.start
        } else {
            (hunk.range.start + offset).min(hunk.range.end - 1)
        };
        // 行与列钳制到工作区文件有效范围（修改可能让行变短）。
        let line = working_line.min(working_text.line_count().saturating_sub(1));
        let column = clamp_column_to_line(working_text, line, column);
        Some((line, column))
    }

    /// 工作区源编辑后推进 hunk 文本区间。
    ///
    /// 组合编辑在 MultiBuffer::edit 内同步调用（用本次编辑的 PositionMap）；
    /// 外部编辑由 source_changed 调用（用消费到的文本变化 patch）。
    /// 返回 true 表示该文件的 hunks 被推进（调用方应重建投影）。
    pub(crate) fn map_diff_hunks_through_edit(
        &mut self,
        source_id: gpui::EntityId,
        position_map: &PositionMap,
        new_version: BufferVersion,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(diff) = &mut self.diff else {
            return false;
        };
        let Some(file) = diff
            .files
            .iter_mut()
            .find(|file| file.working.entity_id() == source_id)
        else {
            return false;
        };
        if file.hunks.is_empty() {
            return false;
        }
        for range in &mut file.ranges {
            *range = range
                .map_through_position_map(new_version, position_map)
                .value();
        }
        let working_text = file.working.read(cx).text_snapshot(cx);
        let mapped_ranges = file
            .ranges
            .iter()
            .map(|range| line_range_for_tracked_range(*range, &working_text))
            .collect::<Vec<_>>();
        for (hunk, range) in file.hunks.iter_mut().zip(mapped_ranges) {
            hunk.range = range;
        }
        true
    }

    /// 统一物化：按展开状态与显示策略把每个文件的可见行物化为 excerpts，并派生显示坐标 hunks。
    ///
    /// 返回 true 表示组合文档文本版本发生变化。
    pub(crate) fn rebuild_diff_projection(&mut self, cx: &mut Context<Self>) -> bool {
        if self.diff.is_none() {
            return false;
        }
        let old_version = self.text_buffer(cx).read(cx).snapshot().version();
        let diff = self.diff.as_mut().expect("已确认 diff 投影存在");
        let expanded_by_default = diff.expanded_by_default;
        let mut excerpts = Vec::new();
        let mut materialized_hunks = Vec::new();
        for file in &mut diff.files {
            file.display_start = materialized_hunks.len();
            materialize_file(
                file,
                cx,
                expanded_by_default,
                &mut excerpts,
                &mut materialized_hunks,
            );
            file.display_len = materialized_hunks.len() - file.display_start;
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
            display_hunks.push(DiffHunk {
                range: new_range,
                old_range: hunk.old_range,
                kind: hunk.kind,
            });
            display_old_ranges.push(old_display);
        }
        let new_version = self.text_buffer(cx).read(cx).snapshot().version();
        let diff = self.diff.as_mut().expect("投影重建前 diff 状态必须存在");
        diff.display_hunks = display_hunks;
        diff.display_old_ranges = display_old_ranges;
        diff.display_version = Some(new_version);
        cx.notify();
        new_version != old_version
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
    fn display_state<'a>(&'a self, cx: &'a App) -> Option<&'a DiffProjection> {
        let diff = self.diff.as_ref()?;
        (diff.display_version == Some(self.text_buffer(cx).read(cx).snapshot().version()))
            .then_some(diff)
    }
}

impl DiffProjection {
    fn file_for_display_index(&self, display_index: usize) -> Option<&DiffFileProjection> {
        self.files.iter().find(|file| {
            display_index >= file.display_start
                && display_index < file.display_start + file.display_len
        })
    }
}

impl DiffFileProjection {
    fn new(input: DiffFileInput, cx: &Context<MultiBuffer>) -> Self {
        let working_text = input.working.read(cx).text_snapshot(cx);
        let ranges = tracked_ranges_for_hunks(&input.hunks, &working_text);
        Self {
            working: input.working,
            hunks: input.hunks,
            ranges,
            base_text: input.base_text,
            base_source: None,
            path: input.path.clone(),
            display_path: input.display_path,
            context_lines: input.context_lines,
            is_created: input.is_created,
            show_file_header: input.show_file_header,
            expanded_deleted: Vec::new(),
            expanded_modified: Vec::new(),
            collapsed_deleted: Vec::new(),
            collapsed_modified: Vec::new(),
            display_start: 0,
            display_len: 0,
        }
    }

    /// 建立旧侧修订源实体（缺失或文本变化时）。
    fn ensure_base_source(&mut self, cx: &mut Context<MultiBuffer>) {
        if self.base_source.is_some() {
            return;
        }
        self.base_source = self.base_text.as_ref().map(|text| {
            let buffer =
                zcv_text::Buffer::from_text(text.to_string(), zcv_text::BufferConfig::default())
                    .expect("base 修订文本必须能创建 Buffer");
            let buffer = cx.new(|_| buffer);
            // 旧侧源的文件路径必须与工作区源一致（绝对），excerpt 定位与导航按源路径匹配。
            cx.new(|cx| LanguageBuffer::new(buffer, Some(self.path.clone()), cx))
        });
    }

    /// hunk 的当前展开状态；尚未进入投影的新 hunk 直接采用默认策略。
    fn is_expanded(
        &self,
        kind: DiffHunkKind,
        old_range: &Range<usize>,
        expanded_by_default: bool,
    ) -> bool {
        match kind {
            DiffHunkKind::Deleted => {
                if expanded_by_default {
                    !self.collapsed_deleted.contains(old_range)
                } else {
                    self.expanded_deleted.contains(old_range)
                }
            }
            DiffHunkKind::Modified => {
                if expanded_by_default {
                    !self.collapsed_modified.contains(old_range)
                } else {
                    self.expanded_modified.contains(old_range)
                }
            }
            DiffHunkKind::Added => true,
        }
    }

    /// 切换展开/折叠（按旧侧行范围标识）。
    fn toggle_expansion(
        &mut self,
        kind: DiffHunkKind,
        old_range: &Range<usize>,
        expanded_by_default: bool,
    ) {
        let is_expanded = self.is_expanded(kind, old_range, expanded_by_default);
        match kind {
            DiffHunkKind::Deleted => {
                if is_expanded {
                    self.expanded_deleted.retain(|range| range != old_range);
                    if expanded_by_default && !self.collapsed_deleted.contains(old_range) {
                        self.collapsed_deleted.push(old_range.clone());
                    }
                } else {
                    self.collapsed_deleted.retain(|range| range != old_range);
                    if !self.expanded_deleted.contains(old_range) {
                        self.expanded_deleted.push(old_range.clone());
                    }
                }
            }
            DiffHunkKind::Modified => {
                if is_expanded {
                    self.expanded_modified.retain(|range| range != old_range);
                    if expanded_by_default && !self.collapsed_modified.contains(old_range) {
                        self.collapsed_modified.push(old_range.clone());
                    }
                } else {
                    self.collapsed_modified.retain(|range| range != old_range);
                    if !self.expanded_modified.contains(old_range) {
                        self.expanded_modified.push(old_range.clone());
                    }
                }
            }
            DiffHunkKind::Added => {}
        }
    }

    /// 按新 hunk 列表迁移展开/折叠集合。
    ///
    /// 显式状态从旧 hunk 迁移：按工作区文本跟踪区间匹配（编辑移位、base 变化都保持识别）；
    /// 未匹配到旧 hunk 的新 hunk 采用默认策略，真正消失的 hunk 状态被清理。
    fn migrate_expansion_state(&mut self, old: &DiffFileProjection, expanded_by_default: bool) {
        let use_ranges = !old.ranges.is_empty() && !self.ranges.is_empty();
        let matches = |old_index: usize, new_index: usize| {
            if use_ranges {
                let old_range = old.ranges[old_index];
                let new_range = self.ranges[new_index];
                old_range.version() == new_range.version()
                    && old_range.range().start() == new_range.range().start()
            } else {
                let old_hunk = &old.hunks[old_index];
                let new_hunk = &self.hunks[new_index];
                old_hunk.kind == new_hunk.kind
                    && old_hunk.old_range.start < new_hunk.old_range.end
                    && new_hunk.old_range.start < old_hunk.old_range.end
            }
        };
        let mut expanded_deleted = Vec::new();
        let mut expanded_modified = Vec::new();
        let mut collapsed_deleted = Vec::new();
        let mut collapsed_modified = Vec::new();
        for (new_index, hunk) in self.hunks.iter().enumerate() {
            let explicitly_expanded = |kind_set: &[Range<usize>]| {
                old.hunks.iter().enumerate().any(|(old_index, old_hunk)| {
                    matches(old_index, new_index) && kind_set.contains(&old_hunk.old_range)
                })
            };
            match hunk.kind {
                DiffHunkKind::Deleted => {
                    let was_expanded = explicitly_expanded(&old.expanded_deleted);
                    let was_collapsed = explicitly_expanded(&old.collapsed_deleted);
                    if expanded_by_default {
                        if was_collapsed {
                            collapsed_deleted.push(hunk.old_range.clone());
                        } else {
                            expanded_deleted.push(hunk.old_range.clone());
                        }
                    } else if was_expanded {
                        expanded_deleted.push(hunk.old_range.clone());
                    }
                }
                DiffHunkKind::Modified => {
                    let was_expanded = explicitly_expanded(&old.expanded_modified);
                    let was_collapsed = explicitly_expanded(&old.collapsed_modified);
                    if expanded_by_default {
                        if was_collapsed {
                            collapsed_modified.push(hunk.old_range.clone());
                        } else {
                            expanded_modified.push(hunk.old_range.clone());
                        }
                    } else if was_expanded {
                        expanded_modified.push(hunk.old_range.clone());
                    }
                }
                DiffHunkKind::Added => {}
            }
        }
        self.expanded_deleted = expanded_deleted;
        self.expanded_modified = expanded_modified;
        self.collapsed_deleted = collapsed_deleted;
        self.collapsed_modified = collapsed_modified;
    }
}

/// 把单个文件的可见行物化为 excerpts，并派生显示坐标 hunks。
fn materialize_file(
    file: &mut DiffFileProjection,
    cx: &App,
    expanded_by_default: bool,
    excerpts: &mut Vec<MultiBufferExcerpt>,
    materialized_hunks: &mut Vec<MaterializedHunk>,
) {
    let working = file.working.clone();
    let base_source = file.base_source.clone();
    let working_text = working.read(cx).text_snapshot(cx);
    let line_count = working_text.line_count();
    let display_path = file.display_path.clone();
    let context_lines = file.context_lines;
    let is_created = file.is_created;
    let show_file_header = file.show_file_header;
    let mut materializer = ExcerptMaterializer {
        excerpts,
        display_path: &display_path,
    };

    // 整文件新增：整个新侧文件作为 Added 显示（无旧侧）。
    if is_created && file.hunks.is_empty() {
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
        });
        return;
    }
    // 无行级差异：整文件模式显示整个新侧文件（空文件保留占位行），裁剪模式不显示。
    if file.hunks.is_empty() {
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
        Some(context) => excerpt_line_ranges(&file.hunks, line_count, context),
    };
    for context_range in visible {
        let mut current = context_range.start;
        // 文件标题块只在宿主声明时创建（ProjectDiffView 多文件投影；普通编辑器整文件不创建）。
        let mut starts_new_excerpt = show_file_header;
        for hunk in file
            .hunks
            .iter()
            .filter(|hunk| hunk_is_inside_excerpt(hunk, &context_range))
        {
            if current < hunk.range.start {
                let _ = materializer.push(
                    current..hunk.range.start,
                    &working_text,
                    &working,
                    None,
                    starts_new_excerpt,
                    false,
                );
                starts_new_excerpt = false;
            }
            // 旧侧：展开时物化完整旧行；裁剪模式折叠时用空占位行标记删除点。
            let old_display = if !hunk.old_range.is_empty() {
                if file.is_expanded(hunk.kind, &hunk.old_range, expanded_by_default)
                    && let Some(base) = base_source.as_ref()
                {
                    let base_text = base.read(cx).text_snapshot(cx);
                    let old_excerpt = materializer
                        .push(
                            hunk.old_range.clone(),
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
                            hunk.old_range.start..hunk.old_range.start,
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
            let new_location = if !hunk.range.is_empty() {
                let new_excerpt = materializer
                    .push(
                        hunk.range.clone(),
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
                old_range: hunk.old_range.clone(),
                kind: hunk.kind,
                old_excerpt: old_display,
                new_location,
            });
            current = hunk.range.end;
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
    hunks: &[DiffHunk],
    line_count: usize,
    context_lines: usize,
) -> Vec<Range<usize>> {
    let max_line = line_count.saturating_sub(1);
    let mut ranges = hunks
        .iter()
        .map(|hunk| {
            let start = hunk.range.start.min(max_line).saturating_sub(context_lines);
            // Zcv 的行范围右开；Zed 的 Point 终点位于最后一条变更行内。
            // 非空 hunk 先换算为最后一条变更行，才能得到真正的后两行上下文。
            let changed_end_line = if hunk.range.is_empty() {
                hunk.range.start
            } else {
                hunk.range.end.saturating_sub(1)
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

fn hunk_is_inside_excerpt(hunk: &DiffHunk, excerpt: &Range<usize>) -> bool {
    if hunk.range.is_empty() {
        excerpt.contains(&hunk.range.start)
    } else {
        hunk.range.start >= excerpt.start && hunk.range.end <= excerpt.end
    }
}

/// 把 hunk 新侧行范围转为工作区源文本跟踪区间（注入时使用）。
fn tracked_ranges_for_hunks(hunks: &[DiffHunk], text: &zcv_text::Snapshot) -> Vec<TrackedRange> {
    let version = text.version();
    hunks
        .iter()
        .map(|hunk| {
            let start = text
                .line_start_byte(Line::new(hunk.range.start))
                .unwrap_or(text.len_bytes());
            let end = text
                .line_start_byte(Line::new(hunk.range.end))
                .unwrap_or(text.len_bytes());
            let range = TextRange::new(start, end).expect("hunk 新侧行范围必须正序");
            TrackedRange::from_range(version, range, Stickiness::Never)
        })
        .collect()
}

fn line_range_for_tracked_range(range: TrackedRange, text: &zcv_text::Snapshot) -> Range<usize> {
    let bytes = range.range();
    let start = text
        .byte_to_line(bytes.start())
        .expect("跟踪区间起点必须属于当前文本")
        .get();
    if bytes.is_empty() {
        return start..start;
    }
    let end_line = text
        .byte_to_line(bytes.end())
        .expect("跟踪区间终点必须属于当前文本");
    let end_line_start = text
        .line_start_byte(end_line)
        .expect("跟踪区间终点行必须有效");
    let end = end_line.get() + usize::from(bytes.end() > end_line_start);
    start..end.max(start + 1)
}

/// 把列（Unicode scalar 计数）钳制到文本中指定行的有效长度（行 0-based）。
fn clamp_column_to_line(text: &zcv_text::Snapshot, line: usize, column: usize) -> usize {
    let line = line.min(text.line_count().saturating_sub(1));
    let line_chars = text
        .line_content(Line::new(line), None)
        .map_or(0, |content| content.len_chars());
    column.min(line_chars)
}
