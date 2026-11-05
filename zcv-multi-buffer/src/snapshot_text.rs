//! `MultiBufferSnapshot` 的文本读取与坐标转换：行、字节、字符、UTF-16 查询，文本拼接与源点定位。
//!
//! 本文件的类型与字段仍定义在 crate 根模块 `multi_buffer.rs`。

use super::*;

impl MultiBufferSnapshot {
    pub(crate) fn source_snapshot(&self, source_index: usize) -> Option<&ExcerptSourceSnapshot> {
        self.excerpt_sources.get(&source_index)
    }

    fn first_source_snapshot(&self) -> Option<&ExcerptSourceSnapshot> {
        self.excerpt_sources.first().map(|(_, source)| source)
    }

    /// 全文搜索使用首个源的宿主语言政策，不伪造光标位置。
    pub fn global_search_word_boundary(&self) -> WordBoundaryPolicy {
        self.first_source_snapshot()
            .map_or_else(WordBoundaryPolicy::default, |source| {
                source.syntax.global_word_boundary()
            })
    }

    /// 政策与可见源片段范围一起派生，词扫描不能把相邻 excerpt 拼成一个词。
    fn word_context_at(
        &self,
        offset: MultiBufferOffset,
    ) -> TextResult<(WordBoundaryPolicy, Range<usize>)> {
        let at = self.source_point(offset.into());
        let at = if at
            .as_ref()
            .is_some_and(|(mapping, _, _)| mapping.entry.source_range.is_empty())
            && offset > MultiBufferOffset::ZERO
        {
            let previous = self.previous_grapheme_boundary(offset.into())?;
            self.source_point(previous).or(at)
        } else {
            at
        };
        Ok(at.map_or_else(
            || (WordBoundaryPolicy::default(), 0..self.len_bytes().get()),
            |(mapping, source, source_offset)| {
                let policy = source
                    .syntax
                    .word_scope_at(source_offset, &source.text)
                    .map_or_else(WordBoundaryPolicy::default, |scope| scope.word_boundary());
                let start = mapping.output_range.start().get();
                (policy, start..start + mapping.entry.source_range.len())
            },
        ))
    }

    /// 主源语言解析后的编辑器设置。
    pub fn language_settings(&self) -> Arc<LanguageSettings> {
        self.first_source_snapshot().map_or_else(
            || Arc::new(LanguageSettings::default()),
            |source| Arc::clone(&source.settings),
        )
    }

    pub fn is_singleton(&self) -> bool {
        self.singleton_source_index.is_some()
    }

    /// 指定组合偏移所属源语言解析后的编辑器设置。
    pub fn language_settings_at(&self, offset: MultiBufferOffset) -> Arc<LanguageSettings> {
        self.source_point(offset.into()).map_or_else(
            || Arc::new(LanguageSettings::default()),
            |(_, source, _)| Arc::clone(&source.settings),
        )
    }

    /// 当前源快照元数据版本。
    ///
    /// 该版本独立于组合文本投影版本：语法重解析不改变文本坐标，但必须让显示层替换其持有的源快照。
    pub fn metadata_version(&self) -> u64 {
        self.metadata_version
    }

    /// 返回当前组合文档的完整 UTF-8 内容，供预览等只读消费者使用。
    pub fn text_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.len_bytes().get());
        for chunk in self.bytes_in_range(ByteOffset::ZERO.into()..self.len_bytes()) {
            bytes.extend_from_slice(chunk.text.as_bytes());
        }
        bytes
    }

    /// 组合文本总字节数。
    /// 由变换树的输出摘要决定，不读取或复制组合文本。
    pub fn len_bytes(&self) -> MultiBufferOffset {
        ByteOffset::new(self.diff_transforms.summary().output.len).into()
    }

    /// 虚拟组合文本的逻辑行数。
    ///
    /// excerpt 映射在建立时已经累计了输出行边界，因此这里不扫描、更不拼接所有源文本。
    pub fn line_count(&self) -> usize {
        // 末尾片段的输出行终点等于 Summary 累积行数；总行数比终点多一行。
        self.diff_transforms.summary().output.lines + 1
    }

    /// 把组合偏移转换为逻辑行。
    pub fn byte_to_line(&self, offset: MultiBufferOffset) -> TextResult<Line> {
        if offset == self.len_bytes() {
            return Ok(Line::new(self.diff_transforms.summary().output.lines));
        }
        let (entry, at, source, source_offset) = self.source_point_at_byte(offset)?;
        let source_line = source.text.byte_to_line(source_offset)?.get();
        Ok(Line::new(
            at.lines + source_line.saturating_sub(entry.source_start_line),
        ))
    }

    /// 返回组合逻辑行的起始字节偏移。
    pub fn line_start_byte(&self, target: Line) -> TextResult<MultiBufferOffset> {
        if target.get() >= self.line_count() {
            return Err(CoordinateError::LineOutOfBounds(target).into());
        }
        self.position_to_byte(Position::new(target, LogicalColumn::ZERO))
    }

    /// 组合逻辑行的完整字节范围（含行尾换行符）；行号越界返回 None。
    ///
    /// 最后一行以组合文本末尾为终点。
    pub fn line_byte_range(&self, line: Line) -> Option<Range<MultiBufferOffset>> {
        let start = self.line_start_byte(line).ok()?;
        let end = if line.get() + 1 < self.line_count() {
            self.line_start_byte(Line::new(line.get() + 1)).ok()?
        } else {
            self.len_bytes()
        };
        Some(start..end)
    }

    /// 组合逻辑行内容的字节范围（不含行尾 `\r`/`\n`）。
    pub fn line_content_byte_range(&self, line: Line) -> Option<Range<MultiBufferOffset>> {
        let range = self.line_byte_range(line)?;
        let mut end = range.end;
        while end > range.start {
            let last = MultiBufferOffset::new(end.get() - 1);
            let is_line_break = self
                .bytes_in_range(last..end)
                .next()
                .and_then(|chunk| chunk.text.as_bytes().first().copied())
                .is_some_and(|byte| byte == b'\n' || byte == b'\r');
            if !is_line_break {
                break;
            }
            end = last;
        }
        Some(range.start..end)
    }

    /// 组合逻辑行的文本（含行尾换行符）；单块时借用源文本切片，跨块时拼接。
    ///
    /// 行号越界返回 None。
    pub fn line_text(&self, line: Line) -> Option<Cow<'_, str>> {
        let range = self.line_byte_range(line)?;
        if range.is_empty() {
            return Some(Cow::Borrowed(""));
        }
        let mut chunks = self.bytes_in_range(range);
        let first = chunks.next()?;
        if let Some(second) = chunks.next() {
            let mut text = String::from(first.text);
            text.push_str(second.text);
            text.extend(chunks.map(|chunk| chunk.text));
            Some(Cow::Owned(text))
        } else {
            Some(Cow::Borrowed(first.text))
        }
    }

    /// 组合逻辑行内容的字节长度与字符数（不含行尾换行符）。
    pub fn line_content_metrics(&self, line: Line) -> Option<(usize, usize)> {
        let content = self.line_content_byte_range(line)?;
        let content_len = content.end.get() - content.start.get();
        let mut byte = content.start.get();
        let mut chars = 0;
        while byte < content.end.get() {
            let (chunk, chunk_start) = self.chunk_at_byte(MultiBufferOffset::new(byte)).ok()?;
            let start = byte - chunk_start.get();
            let end = (content.end.get() - chunk_start.get()).min(chunk.len());
            chars += chunk[start..end].chars().count();
            byte = chunk_start.get() + end;
        }
        Some((content_len, chars))
    }

    /// 定位到起始行的前向行游标。
    ///
    /// 显示层逐可见行读取组合行内容时必须走它，而不是逐行调用无状态的
    /// line_start_byte/line_content_byte_range：后者每次都对映射树整树 seek，
    /// 成本随目标名次增长，会让滚动帧随文档深度变慢。
    pub fn line_cursor(&self, start: Line) -> Option<MultiBufferLineCursor<'_>> {
        MultiBufferLineCursor::new(self, start)
    }

    /// 把组合字节偏移转换为按 Unicode scalar value 计数的逻辑位置。
    pub fn byte_to_position(&self, offset: MultiBufferOffset) -> TextResult<Position> {
        let mut cursor = MultiBufferCursor::new(
            &self.excerpts,
            &self.diff_transforms,
            Sources::Snapshot(&self.excerpt_sources),
        );
        cursor.seek(offset.into(), Bias::Right);
        self.byte_to_position_with_cursor(&mut cursor, offset)
    }

    pub(crate) fn byte_to_position_with_cursor(
        &self,
        cursor: &mut MultiBufferCursor<'_>,
        offset: MultiBufferOffset,
    ) -> TextResult<Position> {
        if offset == self.len_bytes() && self.excerpts.is_empty() {
            return Ok(Position::new(Line::ZERO, LogicalColumn::ZERO));
        }
        let (entry, at, source, source_offset) = self.source_point_at_cursor(cursor, offset)?;
        let source_start = source.text.byte_to_position(entry.source_range.start())?;
        let source_position = source.text.byte_to_position(source_offset)?;
        let row_delta = source_position.line().get() - entry.source_start_line;
        if offset == self.len_bytes() && entry.adds_newline {
            return Ok(Position::new(
                Line::new(self.diff_transforms.summary().output.lines),
                LogicalColumn::ZERO,
            ));
        }
        let column = if row_delta == 0 {
            at.column_chars + source_position.column().get() - source_start.column().get()
        } else {
            source_position.column().get()
        };
        Ok(Position::new(
            Line::new(at.lines + row_delta),
            LogicalColumn::new(column),
        ))
    }

    /// 返回组合文本中的 Tree-sitter 风格字节坐标。
    pub fn byte_to_point(&self, offset: MultiBufferOffset) -> TextResult<(Line, usize)> {
        if offset == self.len_bytes() && self.excerpts.is_empty() {
            return Ok((Line::ZERO, 0));
        }
        let (entry, at, source, source_offset) = self.source_point_at_byte(offset)?;
        let source_start = source.text.byte_to_point(entry.source_range.start())?;
        let source_point = source.text.byte_to_point(source_offset)?;
        let row_delta = source_point.0.get() - entry.source_start_line;
        if offset == self.len_bytes() && entry.adds_newline {
            return Ok((Line::new(self.diff_transforms.summary().output.lines), 0));
        }
        let column = if row_delta == 0 {
            at.column_bytes + source_point.1 - source_start.1
        } else {
            source_point.1
        };
        Ok((Line::new(at.lines + row_delta), column))
    }

    /// 组合范围的文本多维摘要。
    ///
    /// 全文范围直接取输出树摘要；其余范围沿映射游标逐段聚合，每段只测量与查询区间相交的部分。
    /// 行宽维度不做前缀相减，因此最长行、首行宽度与源范围摘要始终一致。
    pub fn text_summary_for_range(&self, range: MultiBufferRange) -> TextResult<MBTextSummary> {
        let start = range.start().get();
        let end = range.end().get();
        self.ensure_output_boundary(range.start())?;
        self.ensure_output_boundary(range.end())?;
        if start >= end {
            return Ok(MBTextSummary::default());
        }
        if start == 0 && end == self.len_bytes().get() {
            return Ok(self.diff_transforms.summary().output);
        }
        let mut summary = MBTextSummary::default();
        let mut cursor = MultiBufferCursor::new(
            &self.excerpts,
            &self.diff_transforms,
            Sources::Snapshot(&self.excerpt_sources),
        );
        cursor.seek(ByteOffset::new(start), Bias::Right);
        if cursor.item().is_none() {
            cursor.prev();
        }
        while let Some((region, _)) = cursor.item() {
            let region_start = cursor.start().bytes;
            let region_end =
                region_start + region.text_summary.len + usize::from(region.adds_newline);
            if region_start >= end {
                break;
            }
            let from = start.max(region_start) - region_start;
            let to = end.min(region_end) - region_start;
            if from < to {
                let content_len = region.text_summary.len;
                let content_to = to.min(content_len);
                if from < content_to {
                    let source = self
                        .source_snapshot(region.source_index)
                        .ok_or(CoordinateError::OutOfBounds(range.start().into()))?;
                    let byte_start = region.source_range.start().get() + from;
                    let byte_end = region.source_range.start().get() + content_to;
                    summary += snapshot_range_summary(
                        &source.text,
                        TextRange::new(ByteOffset::new(byte_start), ByteOffset::new(byte_end))
                            .expect("组合区间子范围必须正序"),
                    )
                    .ok_or(CoordinateError::OutOfBounds(range.start().into()))?
                    .0;
                }
                if to > content_len {
                    summary += MBTextSummary::newline();
                }
            }
            cursor.next();
        }
        Ok(summary)
    }

    /// 读取指定组合范围；结果只在调用方需要跨源拼接时短暂存在。
    pub fn text_for_range(&self, range: MultiBufferRange) -> TextResult<String> {
        Ok(self
            .bytes_in_range(range.start()..range.end())
            .map(|chunk| chunk.text)
            .collect())
    }

    /// 返回**包含**组合偏移的文本块及其组合坐标起点。
    ///
    /// 注意与 `bytes_in_range(offset..)` 的区别：后者从偏移处切开块，本方法返回偏移所在的完整块。
    pub fn chunk_at_byte(
        &self,
        offset: MultiBufferOffset,
    ) -> TextResult<(&str, MultiBufferOffset)> {
        let (entry, at, source, source_offset) = self.source_point_at_byte(offset)?;
        let content_start = ByteOffset::new(at.bytes);
        let content_end = ByteOffset::new(content_start.get() + entry.source_range.len());
        if content_start <= offset.into() && offset < content_end.into() {
            let (chunk, source_chunk_start) = source.text.chunk_at_byte(source_offset)?;
            // 源 chunk 可能起始于片段之前（或越过片段末尾）：裁剪到片段范围内，
            // 组合文本只投影片段内容。
            let chunk_end = ByteOffset::new(source_chunk_start.get() + chunk.len());
            let slice_start = source_chunk_start.max(entry.source_range.start());
            let slice_end = chunk_end.min(entry.source_range.end());
            let text = &chunk[slice_start.get() - source_chunk_start.get()
                ..slice_end.get() - source_chunk_start.get()];
            let output_chunk_start = MultiBufferOffset::new(
                content_start.get() + (slice_start.get() - entry.source_range.start().get()),
            );
            return Ok((text, output_chunk_start));
        }
        // 片段之间的静态换行块或文档末尾：退回按偏移切分的块。
        self.bytes_in_range(offset..self.len_bytes())
            .next()
            .map(|chunk| (chunk.text, chunk.output_range.start))
            .ok_or(CoordinateError::OutOfBounds(offset.into()).into())
    }

    /// 把组合逻辑位置转换为字节偏移。
    ///
    /// 按完整行列定位输出变换与逻辑窗口，再由同版本源快照解析区域内的列。
    pub fn position_to_byte(&self, position: Position) -> TextResult<MultiBufferOffset> {
        if position.line().get() >= self.line_count() {
            return Err(CoordinateError::LineOutOfBounds(position.line()).into());
        }
        if self.excerpts.is_empty() {
            if position == Position::new(Line::ZERO, LogicalColumn::ZERO) {
                return Ok(ByteOffset::ZERO.into());
            }
            return Err(CoordinateError::OutOfBounds(ByteOffset::ZERO).into());
        }

        let mut cursor = MultiBufferCursor::new(
            &self.excerpts,
            &self.diff_transforms,
            Sources::Snapshot(&self.excerpt_sources),
        );
        cursor.seek_output_position(position, Bias::Right);
        if cursor.item().is_none() {
            cursor.prev();
        }
        let (excerpt, _) = cursor
            .item()
            .ok_or(CoordinateError::LineOutOfBounds(position.line()))?;
        let at = cursor.start();
        let row_delta = position.line().get() - at.lines;
        if position.line().get() == self.diff_transforms.summary().output.lines
            && position.column() == LogicalColumn::ZERO
            && excerpt.adds_newline
            && row_delta == excerpt.text_summary.lines + 1
        {
            return Ok(self.len_bytes());
        }
        let source = self
            .source_snapshot(excerpt.source_index)
            .ok_or(CoordinateError::LineOutOfBounds(position.line()))?;
        let source_line = excerpt.source_start_line + row_delta;
        let source_column = if row_delta == 0 {
            let source_start = source.text.byte_to_position(excerpt.source_range.start())?;
            source_start.column().get() + position.column().get() - at.column_chars
        } else {
            position.column().get()
        };
        let source_offset = source.text.position_to_byte(Position::new(
            Line::new(source_line),
            LogicalColumn::new(source_column),
        ))?;
        if source_offset > excerpt.source_range.end() {
            return Err(CoordinateError::OutOfBounds(self.len_bytes().into()).into());
        }
        Ok(
            ByteOffset::new(at.bytes + source_offset.get() - excerpt.source_range.start().get())
                .into(),
        )
    }

    pub fn byte_to_char(&self, offset: MultiBufferOffset) -> TextResult<CharOffset> {
        if offset == self.len_bytes() {
            return Ok(CharOffset::new(self.diff_transforms.summary().output.chars));
        }
        let (entry, at, source, source_offset) = self.source_point_at_byte(offset)?;
        let source_start = source.text.byte_to_char(entry.source_range.start())?;
        let source_char = source.text.byte_to_char(source_offset)?;
        Ok(CharOffset::new(
            at.chars + source_char.get() - source_start.get(),
        ))
    }

    pub fn char_to_byte(&self, target: CharOffset) -> TextResult<MultiBufferOffset> {
        let total = self.diff_transforms.summary().output.chars;
        if target.get() > total {
            return Err(CoordinateError::CharOutOfBounds(target).into());
        }
        if target.get() == total {
            return Ok(self.len_bytes());
        }
        let mut cursor = MultiBufferCursor::new(
            &self.excerpts,
            &self.diff_transforms,
            Sources::Snapshot(&self.excerpt_sources),
        );
        cursor.seek_output_char(target, Bias::Right);
        let (excerpt, _) = cursor
            .item()
            .ok_or(CoordinateError::CharOutOfBounds(target))?;
        let at = cursor.start();
        let source = self
            .source_snapshot(excerpt.source_index)
            .ok_or(CoordinateError::CharOutOfBounds(target))?;
        let source_start = source.text.byte_to_char(excerpt.source_range.start())?;
        let source_char = source_start.get() + target.get() - at.chars;
        let source_offset = source.text.char_to_byte(CharOffset::new(source_char))?;
        if source_offset > excerpt.source_range.end() {
            return Err(CoordinateError::CharOutOfBounds(target).into());
        }
        Ok(
            ByteOffset::new(at.bytes + source_offset.get() - excerpt.source_range.start().get())
                .into(),
        )
    }

    pub fn movement_boundary(
        &self,
        offset: CharOffset,
        direction: MovementDirection,
        unit: MovementUnit,
    ) -> TextResult<CharOffset> {
        // 与单 Buffer 共用同一份文本移动语义，组合文档不得另实现一套边界规则。
        let byte = self.char_to_byte(offset)?;
        let (policy, source_range) = self.word_context_at(byte)?;
        let target = zcv_text::movement_boundary_in_text(self, policy, offset, direction, unit)?;
        if matches!(
            unit,
            MovementUnit::Word
                | MovementUnit::Identifier
                | MovementUnit::Subword
                | MovementUnit::Symbol
        ) {
            if (direction == MovementDirection::Next && byte.get() == source_range.end)
                || (direction == MovementDirection::Previous && byte.get() == source_range.start)
            {
                return Ok(target);
            }
            let target_byte = self
                .char_to_byte(target)?
                .get()
                .clamp(source_range.start, source_range.end);
            self.byte_to_char(ByteOffset::new(target_byte).into())
        } else {
            Ok(target)
        }
    }

    /// 返回包含当前位置的词边界。组合文本沿连续 chunk 读取，不构造临时字符串。
    pub fn surrounding_word(&self, offset: CharOffset) -> TextResult<(CharOffset, CharOffset)> {
        let byte = self.char_to_byte(offset)?;
        let (policy, source_range) = self.word_context_at(byte)?;
        zcv_text::surrounding_word_in_range(
            self,
            policy,
            offset,
            self.byte_to_char(ByteOffset::new(source_range.start).into())?
                ..self.byte_to_char(ByteOffset::new(source_range.end).into())?,
        )
    }

    pub fn is_inside_word(&self, offset: CharOffset) -> TextResult<bool> {
        let byte = self.char_to_byte(offset)?;
        let (policy, source_range) = self.word_context_at(byte)?;
        zcv_text::is_inside_word_in_range(
            self,
            policy,
            offset,
            self.byte_to_char(ByteOffset::new(source_range.start).into())?
                ..self.byte_to_char(ByteOffset::new(source_range.end).into())?,
        )
    }

    pub fn byte_to_utf16_cu(&self, offset: MultiBufferOffset) -> TextResult<Utf16Offset> {
        if offset == self.len_bytes() {
            return Ok(Utf16Offset::new(
                self.diff_transforms.summary().output.len_utf16,
            ));
        }
        let (entry, at, source, source_offset) = self.source_point_at_byte(offset)?;
        let source_start = source.text.byte_to_utf16_cu(entry.source_range.start())?;
        let source_units = source.text.byte_to_utf16_cu(source_offset)?;
        Ok(Utf16Offset::new(
            at.utf16 + source_units.get() - source_start.get(),
        ))
    }

    pub fn utf16_cu_to_byte(&self, target: Utf16Offset) -> TextResult<MultiBufferOffset> {
        let total = self.diff_transforms.summary().output.len_utf16;
        if target.get() > total {
            return Err(CoordinateError::Utf16OffsetOutOfBounds(target).into());
        }
        if target.get() == total {
            return Ok(self.len_bytes());
        }
        let mut cursor = MultiBufferCursor::new(
            &self.excerpts,
            &self.diff_transforms,
            Sources::Snapshot(&self.excerpt_sources),
        );
        cursor.seek_output_utf16(target, Bias::Right);
        let (excerpt, _) = cursor
            .item()
            .ok_or(CoordinateError::Utf16OffsetOutOfBounds(target))?;
        let at = cursor.start();
        let source = self
            .source_snapshot(excerpt.source_index)
            .ok_or(CoordinateError::Utf16OffsetOutOfBounds(target))?;
        let source_start = source.text.byte_to_utf16_cu(excerpt.source_range.start())?;
        let source_units = source_start.get() + target.get() - at.utf16;
        let source_offset = source
            .text
            .utf16_cu_to_byte(Utf16Offset::new(source_units))?;
        if source_offset > excerpt.source_range.end() {
            return Err(CoordinateError::Utf16OffsetOutOfBounds(target).into());
        }
        Ok(
            ByteOffset::new(at.bytes + source_offset.get() - excerpt.source_range.start().get())
                .into(),
        )
    }

    /// 从组合偏移范围连续借用文本块。
    ///
    /// 虚拟 MultiBuffer 文本入口：
    /// 消费者按输出坐标请求范围，游标通过 excerpt 映射定位源快照，再直接返回 Rope chunk 的子切片。
    /// 片段间为保持行边界注入的换行也以静态借用块返回。
    pub fn bytes_in_range(&self, range: Range<MultiBufferOffset>) -> MultiBufferBytes<'_> {
        let end = range.end.min(self.len_bytes());
        let start = range.start.min(end);
        let mut cursor = MultiBufferCursor::new(
            &self.excerpts,
            &self.diff_transforms,
            Sources::Snapshot(&self.excerpt_sources),
        );
        cursor.seek(start.into(), Bias::Right);
        MultiBufferBytes {
            snapshot: self,
            range: start.into()..end.into(),
            cursor,
            offset: start.into(),
        }
    }

    fn source_point_at_byte(
        &self,
        offset: MultiBufferOffset,
    ) -> TextResult<(
        ExcerptCoordinates,
        MappingPosition,
        &ExcerptSourceSnapshot,
        ByteOffset,
    )> {
        if offset > self.len_bytes() {
            return Err(CoordinateError::OutOfBounds(offset.into()).into());
        }
        let mut cursor = MultiBufferCursor::new(
            &self.excerpts,
            &self.diff_transforms,
            Sources::Snapshot(&self.excerpt_sources),
        );
        cursor.seek(offset.into(), Bias::Right);
        self.source_point_at_cursor(&mut cursor, offset)
    }

    fn source_point_at_cursor(
        &self,
        cursor: &mut MultiBufferCursor<'_>,
        offset: MultiBufferOffset,
    ) -> TextResult<(
        ExcerptCoordinates,
        MappingPosition,
        &ExcerptSourceSnapshot,
        ByteOffset,
    )> {
        if offset > self.len_bytes() {
            return Err(CoordinateError::OutOfBounds(offset.into()).into());
        }
        if cursor.item().is_none() {
            cursor.prev();
        }
        let (excerpt, _) = cursor
            .item()
            .ok_or(CoordinateError::OutOfBounds(offset.into()))?;
        let entry = ExcerptCoordinates {
            source_index: excerpt.source_index,
            source_range: excerpt.source_range.range(),
            source_start_line: excerpt.source_start_line,
            adds_newline: excerpt.adds_newline,
        };
        let at = cursor.start().clone();
        let source = self
            .source_snapshot(entry.source_index)
            .ok_or(CoordinateError::OutOfBounds(offset.into()))?;
        let relative = offset
            .get()
            .saturating_sub(at.bytes)
            .min(entry.source_range.len());
        let source_offset = ByteOffset::new(entry.source_range.start().get() + relative);
        source
            .text
            .chunk_at_byte(source_offset)
            .map_err(|_| CoordinateError::InvalidByteBoundary(offset.into()))?;
        Ok((entry, at, source, source_offset))
    }

    pub(crate) fn ensure_output_boundary(&self, offset: MultiBufferOffset) -> TextResult<()> {
        if offset > self.len_bytes() {
            return Err(CoordinateError::OutOfBounds(offset.into()).into());
        }
        if offset == self.len_bytes()
            || (offset == ByteOffset::ZERO.into() && self.excerpts.is_empty())
        {
            return Ok(());
        }
        self.source_point_at_byte(offset).map(|_| ())
    }

    pub fn version(&self) -> BufferVersion {
        self.projection_version
    }

    pub(crate) fn source_point(
        &self,
        offset: ByteOffset,
    ) -> Option<(ExcerptMapping, &ExcerptSourceSnapshot, ByteOffset)> {
        let (mapping, at) = mapping_at_tree(
            &self.excerpts,
            &self.diff_transforms,
            Sources::Snapshot(&self.excerpt_sources),
            offset,
        )?;
        let source = self.source_snapshot(mapping.source_index)?;
        let delta = offset
            .get()
            .saturating_sub(at.bytes)
            .min(mapping.source_range.len());
        let source_offset = ByteOffset::new(mapping.source_range.start().get() + delta);
        Some((mapping, source, source_offset))
    }

    pub(crate) fn source_range(
        &self,
        range: Range<usize>,
    ) -> Option<(ExcerptMapping, &ExcerptSourceSnapshot, Range<usize>)> {
        let (mapping, at) = mapping_at_tree(
            &self.excerpts,
            &self.diff_transforms,
            Sources::Snapshot(&self.excerpt_sources),
            ByteOffset::new(range.start),
        )?;
        let output_start = at.bytes;
        let content_end = output_start + mapping.source_range.len();
        if range.start < output_start || range.end > content_end {
            return None;
        }
        let source = self.source_snapshot(mapping.source_index)?;
        let source_start = mapping.source_range.start().get() + range.start - output_start;
        let source_end = mapping.source_range.start().get() + range.end - output_start;
        Some((mapping, source, source_start..source_end))
    }
}
