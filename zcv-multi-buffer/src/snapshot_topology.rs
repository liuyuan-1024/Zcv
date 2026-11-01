//! `MultiBufferSnapshot` 的 excerpt 拓扑、逻辑边界与 diff 显示查询。
//!
//! 本文件的类型与字段仍定义在 crate 根模块 `multi_buffer.rs`。

use super::*;

impl MultiBufferSnapshot {
    /// 当前逻辑窗口引用的文件身份，按源去重；读取不枚举 excerpts。
    pub fn file_buffer_ids(&self) -> impl Iterator<Item = BufferId> + '_ {
        self.file_sources.iter().map(|(id, _)| *id)
    }

    /// 一个逻辑文件在当前输出投影中的范围，包含其全部窗口与删除变换。
    pub fn buffer_range(&self, buffer_id: BufferId) -> Option<MultiBufferRange> {
        let file = self.file_sources.get(&buffer_id)?;
        let mut first = self.excerpts.cursor::<ExcerptSummary>(());
        first.seek(&file.path, Bias::Left);
        let excerpt = first.item()?;
        let start = self
            .logical_excerpt_snapshot(excerpt, first.start().text.len)
            .output_range
            .start();
        let mut last = self.excerpts.cursor::<ExcerptSummary>(());
        last.seek(&file.path, Bias::Right);
        last.prev();
        let excerpt = last.item()?;
        let end = self
            .logical_excerpt_snapshot(excerpt, last.start().text.len)
            .output_range
            .end();
        Some(MultiBufferRange::new(start, end).expect("文件窗口必须连续且正序"))
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty_source_count > 0
    }

    /// 当前组合快照绑定的 diff 显示输入。
    ///
    /// `None` 表示普通文档没有 diff 装饰；调用方不得为此建立空的并列缓存。
    pub fn diff_display(&self) -> Option<&Arc<DiffDisplaySnapshot>> {
        self.diff_display.as_ref()
    }

    /// 组合片段边界的拓扑版本。
    ///
    /// 源文本编辑只推进 `version`；只有增删片段或改变逻辑 excerpt 边界时才推进本版本。
    /// 显示块据此复用边界分类，不把普通行内编辑误当成组合结构变化。
    pub fn topology_version(&self) -> u64 {
        self.topology_version
    }

    /// 按输入顺序遍历稳定逻辑 excerpts；diff 展开不会增删这些窗口。
    pub fn excerpts(&self) -> impl Iterator<Item = ExcerptSnapshot> + '_ {
        let mut cursor = self.excerpts.cursor::<ExcerptSummary>(());
        cursor.next();
        iter::from_fn(move || {
            let excerpt = cursor.item()?;
            let start = cursor.start().text.len;
            let snapshot = self.logical_excerpt_snapshot(excerpt, start);
            cursor.next();
            Some(snapshot)
        })
    }

    fn logical_excerpt_snapshot(&self, excerpt: &Excerpt, input_start: usize) -> ExcerptSnapshot {
        let mut start_cursor = self.diff_transforms.cursor::<MappingPosition>(());
        start_cursor.seek(&ExcerptOffset::new(input_start), Bias::Left);
        let start = start_cursor.start().bytes
            + input_start.saturating_sub(start_cursor.start().input_offset.get());
        let end = if excerpt.adds_newline {
            let input_end = input_start + excerpt.output_text_summary().len;
            let mut end_cursor = self.diff_transforms.cursor::<MappingPosition>(());
            end_cursor.seek(&ExcerptOffset::new(input_end), Bias::Left);
            end_cursor.start().bytes
                + input_end.saturating_sub(end_cursor.start().input_offset.get())
        } else {
            // 最后一个窗口延伸到输出文尾，包含输入文尾的零长度删除变换。
            self.len_bytes().get()
        };
        let mut snapshot = excerpt.output_region().to_snapshot(MappingPosition {
            bytes: start,
            lines: self
                .byte_to_line(MultiBufferOffset::new(start))
                .expect("excerpt 输出起点必须有效")
                .get(),
            ..MappingPosition::default()
        });
        snapshot.output_range =
            MultiBufferRange::new(start, end).expect("逻辑 excerpt 输出范围必须正序");
        snapshot.output_end_line = self
            .byte_to_line(MultiBufferOffset::new(end))
            .expect("excerpt 输出终点必须有效")
            .get();
        snapshot
    }

    /// 只枚举起点位于指定输出范围中的逻辑边界；先定位输入窗口，再连续推进。
    /// Header、divider 与整文件折叠共用此边界流；单文件文档没有边界。
    pub fn excerpt_boundaries_in_range(
        &self,
        range: std::ops::RangeInclusive<MultiBufferOffset>,
    ) -> impl Iterator<Item = ExcerptBoundary> + '_ {
        let mut output = MultiBufferCursor::new(
            &self.excerpts,
            &self.diff_transforms,
            Sources::Snapshot(&self.excerpt_sources),
        );
        output.seek((*range.start()).into(), Bias::Left);
        let mut cursor = output.excerpts;
        let mut previous_cursor = cursor.clone();
        previous_cursor.prev();
        let mut previous = previous_cursor.item().map(|excerpt| {
            self.logical_excerpt_snapshot(excerpt, previous_cursor.start().text.len)
        });
        iter::from_fn(move || {
            if self.singleton {
                return None;
            }
            loop {
                let excerpt = cursor.item()?;
                let next = self.logical_excerpt_snapshot(excerpt, cursor.start().text.len);
                cursor.next();
                if next.output_range.start() > *range.end() {
                    return None;
                }
                let boundary = ExcerptBoundary {
                    previous: previous.clone(),
                    next: next.clone(),
                };
                previous = Some(next);
                if boundary.next.output_range.start() >= *range.start() {
                    return Some(boundary);
                }
            }
        })
    }

    /// 输出位置对应的逻辑窗口；删除侧与新侧共享窗口身份。
    pub fn logical_excerpt_at_output_offset(
        &self,
        offset: MultiBufferOffset,
    ) -> Option<ExcerptSnapshot> {
        let mut cursor = MultiBufferCursor::new(
            &self.excerpts,
            &self.diff_transforms,
            Sources::Snapshot(&self.excerpt_sources),
        );
        cursor.seek(offset.into(), Bias::Right);
        let excerpt = cursor.excerpts.item()?;
        Some(self.logical_excerpt_snapshot(excerpt, cursor.excerpts.start().text.len))
    }

    /// 逻辑窗口变化的输出范围；共同前后缀保留在原有树中，显示层只消费受影响范围。
    pub fn excerpt_boundary_change(
        &self,
        previous: &Self,
    ) -> Option<(MultiBufferRange, MultiBufferRange)> {
        if self.topology_version == previous.topology_version {
            return None;
        }
        let same = |left: &Excerpt, right: &Excerpt| {
            left.path == right.path
                && left.source_id == right.source_id
                && left.buffer_id == right.buffer_id
                && left.source_range == right.source_range
                && left.adds_newline == right.adds_newline
        };
        let mut old = previous.excerpts.cursor::<ExcerptSummary>(());
        let mut new = self.excerpts.cursor::<ExcerptSummary>(());
        old.next();
        new.next();
        while let (Some(left), Some(right)) = (old.item(), new.item()) {
            if !same(left, right) {
                break;
            }
            old.next();
            new.next();
        }
        if old.item().is_none() && new.item().is_none() {
            return None;
        }
        let old_start = old.item().map_or(previous.len_bytes(), |excerpt| {
            previous
                .logical_excerpt_snapshot(excerpt, old.start().text.len)
                .output_range
                .start()
        });
        let new_start = new.item().map_or(self.len_bytes(), |excerpt| {
            self.logical_excerpt_snapshot(excerpt, new.start().text.len)
                .output_range
                .start()
        });
        let old_first = old.start().count;
        let new_first = new.start().count;
        old.seek(
            &ExcerptIndex(previous.excerpts.summary().count),
            Bias::Right,
        );
        new.seek(&ExcerptIndex(self.excerpts.summary().count), Bias::Right);
        old.prev();
        new.prev();
        while old.start().count >= old_first && new.start().count >= new_first {
            let (Some(left), Some(right)) = (old.item(), new.item()) else {
                break;
            };
            if !same(left, right) {
                break;
            }
            old.prev();
            new.prev();
        }
        let old_end = old.item().map_or(old_start, |excerpt| {
            previous
                .logical_excerpt_snapshot(excerpt, old.start().text.len)
                .output_range
                .end()
        });
        let new_end = new.item().map_or(new_start, |excerpt| {
            self.logical_excerpt_snapshot(excerpt, new.start().text.len)
                .output_range
                .end()
        });
        Some((
            MultiBufferRange::new(old_start, old_end.max(old_start)).expect("旧窗口变化必须正序"),
            MultiBufferRange::new(new_start, new_end.max(new_start)).expect("新窗口变化必须正序"),
        ))
    }

    /// 按逻辑路径定位输出读取区域，不扫描其它文件。
    pub fn regions_for_path(&self, path: &Path) -> impl Iterator<Item = ExcerptSnapshot> {
        let path_key = PathKey::with_order(path, self.path_order);
        let mut cursor = MultiBufferCursor::new(
            &self.excerpts,
            &self.diff_transforms,
            Sources::Snapshot(&self.excerpt_sources),
        );
        cursor.seek_path(&path_key, Bias::Left);
        let mut snapshots = Vec::new();
        while let Some((excerpt, _)) = cursor.item() {
            if excerpt.path != path_key {
                break;
            }
            snapshots.push(excerpt.to_snapshot(cursor.start().clone()));
            cursor.next();
        }
        snapshots.into_iter()
    }

    pub fn excerpt_for_output_line(&self, line: usize) -> Option<ExcerptSnapshot> {
        let (entry, at) = mapping_at_output_line(
            &self.excerpts,
            &self.diff_transforms,
            Sources::Snapshot(&self.excerpt_sources),
            line,
        )?;
        let excerpt = entry.entry.to_snapshot(at);
        ((excerpt.output_start_line <= line && line < excerpt.output_end_line)
            || (excerpt.output_start_line == excerpt.output_end_line
                && line == excerpt.output_start_line))
            .then_some(excerpt)
    }
}
