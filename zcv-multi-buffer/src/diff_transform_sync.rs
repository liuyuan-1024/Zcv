//! 逻辑 excerpt 编辑到组合输出编辑的前向同步。

use super::*;

#[derive(Clone, Debug)]
pub(super) struct InputEdit {
    pub old: Range<ExcerptOffset>,
    pub new: Range<ExcerptOffset>,
}

impl InputEdit {
    pub(crate) fn new(old: Range<usize>, new: Range<usize>) -> Self {
        Self {
            old: ExcerptOffset::new(old.start)..ExcerptOffset::new(old.end),
            new: ExcerptOffset::new(new.start)..ExcerptOffset::new(new.end),
        }
    }
}

pub(super) struct OutputEdit {
    old: MultiBufferRange,
    new: MultiBufferRange,
}

impl OutputEdit {
    pub(super) fn into_text_ranges(self) -> (TextRange, TextRange) {
        (self.old.into(), self.new.into())
    }
}

fn merge_input_edits(mut edits: Vec<InputEdit>) -> Vec<InputEdit> {
    edits.sort_by_key(|edit| (edit.old.start, edit.new.start));
    let mut merged: Vec<InputEdit> = Vec::new();
    for edit in edits {
        if let Some(previous) = merged.last_mut()
            && (previous.old.end >= edit.old.start || previous.new.end >= edit.new.start)
        {
            previous.old.end = previous.old.end.max(edit.old.end);
            previous.new.end = previous.new.end.max(edit.new.end);
        } else {
            merged.push(edit);
        }
    }
    merged
}

impl SeekTarget<'_, ExcerptSummary, ExcerptSummary> for ExcerptOffset {
    fn cmp(&self, location: &ExcerptSummary, _: ()) -> Ordering {
        Ord::cmp(&self.get(), &location.text.len)
    }
}

/// 变更原因由调用方给出，投影本身只读取当前已安装的窗口和 hunk 状态。
#[derive(Clone, Copy)]
pub(super) enum DiffChangeKind<'a> {
    BufferEdited,
    ExcerptsChanged(&'a SumTree<Excerpt>),
    ExpansionChanged,
    DiffUpdated,
    ReplacedAll,
}

impl Dimension<'_, ExcerptSummary> for ExcerptOffset {
    fn zero(_: ()) -> Self {
        Self::default()
    }

    fn add_summary(&mut self, summary: &ExcerptSummary, _: ()) {
        self.0 += summary.text.len;
    }
}

impl Dimension<'_, DiffTransformSummary> for ExcerptOffset {
    fn zero(_: ()) -> Self {
        Self::default()
    }

    fn add_summary(&mut self, summary: &DiffTransformSummary, _: ()) {
        self.0 += summary.input.len;
    }
}

type TransformPosition = sum_tree::Dimensions<ExcerptOffset, MultiBufferOffset>;

/// 同步期间尚未发布的投影。新增侧 hunk 可跨多笔相邻编辑，直到写完其输入范围。
pub(super) struct TransformBuilder<'a> {
    excerpts: &'a SumTree<Excerpt>,
    sources: &'a [ExcerptSource],
    transforms: SumTree<DiffTransform>,
    current_insertion: Option<(ExcerptOffset, DiffTransformHunkInfo)>,
}

impl TransformBuilder<'_> {
    pub(super) fn push_content(&mut self, end: ExcerptOffset) {
        if let Some((insertion_end, info)) = self.current_insertion.take() {
            if end < insertion_end {
                self.push_content_segment(end, vec![info.clone()]);
                self.current_insertion = Some((insertion_end, info));
                return;
            }
            self.push_content_segment(insertion_end, vec![info]);
        }
        self.push_content_segment(end, Vec::new());
    }

    fn push_content_segment(&mut self, end: ExcerptOffset, hunks: Vec<DiffTransformHunkInfo>) {
        let start = ExcerptOffset::new(self.transforms.summary().input.len);
        if start < end {
            let summary = input_range_summary(self.excerpts, self.sources, start, end);
            self.push(DiffTransform::content(summary, hunks));
        }
    }

    pub(super) fn set_current_insertion(
        &mut self,
        end: ExcerptOffset,
        info: DiffTransformHunkInfo,
    ) {
        if self.transforms.summary().input.len < end.get() {
            self.current_insertion = Some((end, info));
        }
    }

    pub(super) fn push(&mut self, transform: DiffTransform) {
        if !extend_last_content(&mut self.transforms, &transform) {
            self.transforms.push(transform, ());
        }
    }

    fn append(&mut self, subtree: SumTree<DiffTransform>) {
        let Some(first) = subtree.first() else {
            return;
        };
        if !extend_last_content(&mut self.transforms, first) {
            self.transforms.append(subtree, ());
            return;
        }
        let mut cursor = subtree.cursor::<()>(());
        cursor.next();
        cursor.next();
        self.transforms.append(cursor.suffix(), ());
    }
}

fn extend_last_content(tree: &mut SumTree<DiffTransform>, next: &DiffTransform) -> bool {
    let DiffTransform::BufferContent { summary, hunks } = next else {
        return false;
    };
    let mut extended = false;
    tree.update_last(
        |last| {
            if let DiffTransform::BufferContent {
                summary: previous,
                hunks: previous_hunks,
            } = last
                && previous_hunks == hunks
            {
                previous.input += summary.input;
                previous.output += summary.output;
                extended = true;
            }
        },
        (),
    );
    extended
}

impl MultiBuffer {
    /// 构造、整体替换与重排均显式覆盖旧、新的完整输入。
    pub(super) fn rebuild_all_diff_transforms(&mut self, cx: &App) -> Vec<OutputEdit> {
        let old_len = self.state.diff_transforms.summary().input.len;
        let new_len = self.state.excerpts.summary().text.len;
        self.sync_diff_transforms(
            vec![InputEdit::new(0..old_len, 0..new_len)],
            DiffChangeKind::ReplacedAll,
            cx,
        )
    }

    /// 复用未变的完整旧节点，从新 excerpts 补建触及的节点并同时生成输出失效范围。
    pub(super) fn sync_diff_transforms(
        &mut self,
        edits: Vec<InputEdit>,
        change_kind: DiffChangeKind<'_>,
        cx: &App,
    ) -> Vec<OutputEdit> {
        if edits.is_empty() {
            return Vec::new();
        }

        let mut edits = merge_input_edits(edits).into_iter().peekable();
        let mut excerpts = self.state.excerpts.cursor::<ExcerptOffset>(());
        let mut old = self.state.diff_transforms.cursor::<TransformPosition>(());
        let mut next = TransformBuilder {
            excerpts: &self.state.excerpts,
            sources: &self.state.sources,
            transforms: SumTree::new(()),
            current_insertion: None,
        };
        let mut output_edits = Vec::new();
        let mut output_delta = 0isize;
        let mut at_transform_boundary = true;

        while let Some(edit) = edits.next() {
            if at_transform_boundary {
                next.append(old.slice(&edit.old.start, Bias::Left));
                if let Some(transform) = old.item()
                    && old.end().0 == edit.old.start
                    && old.start().0 < edit.old.start
                {
                    next.push(transform.clone());
                    old.next();
                }
            }

            let old_start = old.start().1.get() + (edit.old.start.get() - old.start().0.get());
            let new_start = old_start
                .checked_add_signed(output_delta)
                .expect("新输出偏移必须非负");

            // 编辑边界上的零输入节点也被消费，由新窗口负责重新生成。
            while old.item().is_some() && old.end().0 <= edit.old.end {
                old.next();
            }

            excerpts.seek_forward(&edit.new.start, Bias::Right);
            if excerpts.item().is_none() && *excerpts.start() == edit.new.start {
                excerpts.prev();
            }
            while let Some(excerpt) = excerpts.item() {
                self.recompute_excerpt_diff(
                    excerpt,
                    excerpts.next_item(),
                    *excerpts.start(),
                    &edit.new,
                    &mut next,
                    cx,
                );
                if excerpts.end() > edit.new.end {
                    break;
                }
                excerpts.next();
            }

            let old_end = old.start().1.get() + (edit.old.end.get() - old.start().0.get());
            let built = next.transforms.summary();
            let new_end = built.output.len + (edit.new.end.get() - built.input.len);
            let output = OutputEdit {
                old: MultiBufferRange::new(old_start, old_end).expect("旧输出范围必须正序"),
                new: MultiBufferRange::new(new_start, new_end).expect("新输出范围必须正序"),
            };
            output_delta += output.new.len() as isize - output.old.len() as isize;
            output_edits.push(output);

            at_transform_boundary = edits
                .peek()
                .is_none_or(|next_edit| next_edit.old.start >= old.end().0);
            if at_transform_boundary {
                let mut end = edit.new.end;
                if old.start().0 < edit.old.end {
                    end.0 += old.end().0.get() - edit.old.end.get();
                    old.next();
                }
                next.push_content(end);
            }
        }

        next.append(old.suffix());
        drop(old);
        if next.transforms.is_empty() {
            next.push(DiffTransform::content(MBTextSummary::default(), Vec::new()));
        }
        let transforms = next.transforms;
        let old_excerpts = match change_kind {
            DiffChangeKind::BufferEdited | DiffChangeKind::ReplacedAll => None,
            DiffChangeKind::ExcerptsChanged(excerpts) => Some(excerpts),
            DiffChangeKind::ExpansionChanged | DiffChangeKind::DiffUpdated => {
                Some(&self.state.excerpts)
            }
        };
        if let Some(old_excerpts) = old_excerpts {
            let sources = Sources::Workspace(&self.state.sources);
            let mut before =
                MultiBufferCursor::new(old_excerpts, &self.state.diff_transforms, sources);
            let mut after = MultiBufferCursor::new(&self.state.excerpts, &transforms, sources);
            output_edits.retain(|edit| {
                !projection_text_equal_in_ranges(&mut before, &mut after, edit.old, edit.new)
            });
        }
        self.state.diff_transforms = transforms;
        output_edits
    }
}
