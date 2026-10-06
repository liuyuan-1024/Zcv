//! 稳定 excerpt 输入坐标到 diff 输出坐标的增量同步。
//!
//! 源编辑先生成输入编辑。
//! 同步器保留未变的变换前后缀，只重算输入编辑覆盖的区间，并通过旧、新变换的输出摘要生成显示层消费的同一批输出编辑。

use super::*;

#[derive(Clone, Debug)]
pub(super) struct InputEdit {
    pub old: Range<ExcerptOffset>,
    pub new: Range<ExcerptOffset>,
}

impl InputEdit {
    pub fn new(old: Range<usize>, new: Range<usize>) -> Self {
        Self {
            old: ExcerptOffset::new(old.start)..ExcerptOffset::new(old.end),
            new: ExcerptOffset::new(new.start)..ExcerptOffset::new(new.end),
        }
    }
}

pub(super) fn merge_input_edits(mut edits: Vec<InputEdit>) -> Vec<InputEdit> {
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

struct SourceFrame<'a> {
    sources: &'a [ExcerptSource],
    previous: Option<(gpui::EntityId, &'a Snapshot)>,
}

impl SourceTexts for SourceFrame<'_> {
    fn source_text(&self, index: usize) -> Option<&Snapshot> {
        let source = self.sources.get(index)?;
        Some(match self.previous {
            Some((id, text)) if source.entity.entity_id() == id => text,
            _ => &source.text,
        })
    }
}

/// 在输入坐标处拆分变换树；
/// 内容节点可拆分，删除节点按 Bias 归入边界的一侧。
/// 未触及的子树保持共享，不物化组合文本。
fn split_at_input<S: SourceTexts>(
    excerpts: &SumTree<Excerpt>,
    transforms: &SumTree<DiffTransform>,
    offset: ExcerptOffset,
    bias: Bias,
    sources: &S,
) -> (SumTree<DiffTransform>, SumTree<DiffTransform>, usize) {
    let mut cursor = transforms.cursor::<MappingPosition>(());
    let mut prefix = cursor.slice(&offset, bias);
    let output_start = cursor.start().bytes;
    let input_start = cursor.start().input_offset.get();
    let Some(transform) = cursor.item() else {
        return (prefix, SumTree::new(()), output_start);
    };
    let relative = offset.get().saturating_sub(input_start);
    if let DiffTransform::BufferContent { summary, hunks } = transform
        && relative > 0
    {
        // 左右区间各自从源快照聚合；最长行等 max 维度无法由整段摘要减去前缀得到。
        let transform_start = cursor.start().input_offset;
        let transform_end = ExcerptOffset::new(transform_start.get() + summary.input.len);
        let left = input_range_summary(excerpts, sources, transform_start, offset);
        let complete = relative == summary.input.len;
        let mut left_summary = summary.clone();
        left_summary.input = left;
        left_summary.output = left;
        prefix.push(
            DiffTransform::BufferContent {
                summary: left_summary,
                hunks: hunks.clone(),
            },
            (),
        );
        let mut suffix = SumTree::new(());
        if !complete {
            let right = input_range_summary(excerpts, sources, offset, transform_end);
            let mut right_summary = summary.clone();
            right_summary.input = right;
            right_summary.output = right;
            suffix.push(
                DiffTransform::BufferContent {
                    summary: right_summary,
                    hunks: hunks.clone(),
                },
                (),
            );
        }
        cursor.next();
        suffix.append(cursor.suffix(), ());
        return (prefix, suffix, output_start + relative);
    }
    (prefix, cursor.suffix(), output_start)
}

/// 按 hunk 元数据合并相邻内容变换；读取游标独立跨越逻辑 excerpt 边界。
fn append_transforms(next: &mut SumTree<DiffTransform>, incoming: SumTree<DiffTransform>) {
    let Some(last) = next.last() else {
        next.append(incoming, ());
        return;
    };
    let mut cursor = incoming.cursor::<MappingPosition>(());
    cursor.next();
    let Some(first) = cursor.item() else {
        return;
    };
    let merge = matches!((last, first),
        (DiffTransform::BufferContent { hunks: left, .. }, DiffTransform::BufferContent { hunks: right, .. }) if left == right);
    if merge {
        let first = first.clone();
        next.update_last(
            |last| {
                if let (
                    DiffTransform::BufferContent { summary: left, .. },
                    DiffTransform::BufferContent { summary: right, .. },
                ) = (last, first)
                {
                    left.input += right.input;
                    left.output += right.output;
                }
            },
            (),
        );
        cursor.next();
        next.append(cursor.suffix(), ());
    } else {
        drop(cursor);
        next.append(incoming, ());
    }
}

impl MultiBuffer {
    /// 显式构造、整体替换与重命名时重建输出变换；输入 excerpts 已由其所有者建立。
    pub(super) fn rebuild_all_diff_transforms(&mut self, cx: &App) {
        let mut transforms = SumTree::new(());
        for excerpt in self.state.excerpts.iter() {
            append_transforms(
                &mut transforms,
                SumTree::from_iter(
                    self.diff_transforms_for_excerpt(excerpt, 0..diff_output_text(excerpt).len, cx),
                    (),
                ),
            );
        }
        self.state.diff_transforms = transforms;
    }

    pub(super) fn sync_diff_transforms(
        &mut self,
        before: &ProjectionTrees,
        edits: Vec<InputEdit>,
        old_source: Option<(gpui::EntityId, &Snapshot)>,
        cx: &App,
    ) -> Vec<(TextRange, TextRange)> {
        let sources = SourceFrame {
            sources: &self.state.sources,
            previous: old_source,
        };
        let mut next = SumTree::new(());
        let mut previous_end = None;
        let mut output_edits = Vec::new();
        for edit in merge_input_edits(edits) {
            let (prefix, _, old_start) =
                split_at_input(&before.0, &before.1, edit.old.start, Bias::Left, &sources);
            let retained = if let Some(end) = previous_end {
                split_at_input(&before.0, &prefix, end, Bias::Right, &sources).1
            } else {
                prefix
            };
            append_transforms(&mut next, retained);
            let new_start = next.summary().output.len;
            let (_, _, old_end) =
                split_at_input(&before.0, &before.1, edit.old.end, Bias::Right, &sources);
            let mut cursor = self.state.excerpts.cursor::<ExcerptSummary>(());
            cursor.seek(&edit.new.start, Bias::Right);
            // 文尾空 excerpt 仍有一个输入身份，须保留其零长度内容变换。
            if cursor.item().is_none()
                && edit.new.end.get() == self.state.excerpts.summary().text.len
            {
                cursor.prev();
            }
            while let Some(excerpt) = cursor.item() {
                let start = cursor.start().text.len;
                let end = start + diff_output_text(excerpt).len;
                if start > edit.new.end.get() || (start == edit.new.end.get() && start != end) {
                    break;
                }
                let range = edit.new.start.get().saturating_sub(start)
                    ..(edit.new.end.get().min(end) - start);
                append_transforms(
                    &mut next,
                    SumTree::from_iter(self.diff_transforms_for_excerpt(excerpt, range, cx), ()),
                );
                if end > edit.new.end.get() {
                    break;
                }
                cursor.next();
            }
            let new_end = next.summary().output.len;
            output_edits.push((
                TextRange::new(ByteOffset::new(old_start), ByteOffset::new(old_end))
                    .expect("旧输出增量必须正序"),
                TextRange::new(ByteOffset::new(new_start), ByteOffset::new(new_end))
                    .expect("新输出增量必须正序"),
            ));
            previous_end = Some(edit.old.end);
        }
        if output_edits.is_empty() {
            return output_edits;
        }
        let (_, suffix, _) = split_at_input(
            &before.0,
            &before.1,
            previous_end.expect("非空编辑必须有同步终点"),
            Bias::Right,
            &sources,
        );
        append_transforms(&mut next, suffix);
        assert_eq!(
            next.summary().input,
            self.state.excerpts.summary().text,
            "diff 变换输入必须精确覆盖逻辑 excerpts"
        );
        self.state.diff_transforms = next;
        output_edits
    }
}
