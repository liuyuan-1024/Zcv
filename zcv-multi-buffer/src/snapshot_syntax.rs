//! `MultiBufferSnapshot` 的语法节点、括号、输入范围与高亮查询。
//!
//! 本文件的类型与字段仍定义在 crate 根模块 `multi_buffer.rs`。

use super::*;

impl MultiBufferSnapshot {
    pub fn capture_names(&self) -> Arc<[Arc<str>]> {
        Arc::clone(&self.capture_names)
    }

    /// 查询组合坐标中的语法高亮，并把每个源 Buffer 的 capture index 映射到本快照的统一表。
    ///
    pub fn highlights(&self, range: Range<usize>) -> Vec<HighlightSpan> {
        let mut spans = Vec::new();
        // 片段按输出顺序排列，内容结束位置单调递增；
        // 先用输出字节游标定位第一个可能重叠的片段，避免每个视口范围都扫描整份多文件结果。
        let mut cursor = MultiBufferCursor::new(
            &self.excerpts,
            &self.diff_transforms,
            Sources::Snapshot(&self.excerpt_sources),
        );
        cursor.seek(ByteOffset::new(range.start), Bias::Right);
        while let Some((excerpt, _)) = cursor.item() {
            let output_start = cursor.start().bytes;
            if output_start >= range.end {
                break;
            }
            // 非末尾 excerpt 可能为显示边界补一个换行；
            // 该字节不属于 source，不能越过 source_range 去查询下一段源文本的语法。
            let output_end = output_start + excerpt.source_range.len();
            let start = range.start.max(output_start);
            let end = range.end.min(output_end);
            if start < end {
                let source = self
                    .source_snapshot(excerpt.source_index)
                    .expect("excerpt 必须引用已注册的源快照");
                let source_start = excerpt.source_range.start().get() + start - output_start;
                let source_end = excerpt.source_range.start().get() + end - output_start;
                let source_offset = excerpt.source_range.start().get();
                spans.extend(
                    source
                        .syntax
                        .highlights(
                            source_start..source_end,
                            &source.text,
                            &source.highlight_cache,
                        )
                        .into_iter()
                        .filter_map(|span| {
                            let capture = *source.capture_map.get(span.capture as usize)?;
                            Some(HighlightSpan {
                                range: (output_start + span.range.start - source_offset)
                                    ..(output_start + span.range.end - source_offset),
                                capture,
                            })
                        }),
                );
            }
            cursor.next();
        }
        spans
    }

    /// 返回组合输出位置对应的源快照与坐标映射。
    ///
    /// 删除 hunk 对应只读基线源；excerpt 间补充换行按边界规则关联相邻源。
    pub fn source_at(&self, offset: impl Into<MultiBufferOffset>) -> Option<MultiBufferSource<'_>> {
        let (mapping, _, source_offset) =
            self.source_point(ByteOffset::new(offset.into().get()))?;
        Some(MultiBufferSource {
            snapshot: self,
            mapping,
            source_offset,
        })
    }

    /// 组合输出位置映射到可编辑源后，查询该源同版本的输入政策。
    pub fn input_scope_at(&self, offset: MultiBufferOffset) -> Option<InputScope<'_>> {
        let (mapping, source, source_offset) = self.source_point(offset.into())?;
        if !mapping.editable {
            return None;
        }
        source.syntax.input_scope_at(source_offset, &source.text)
    }

    /// 在同一可编辑 excerpt 内，按范围起点查询源语言的输入政策。
    pub fn input_scope_at_range_start(&self, range: MultiBufferRange) -> Option<InputScope<'_>> {
        let (mapping, source, source_range) =
            self.source_range(range.start().get()..range.end().get())?;
        if !mapping.editable {
            return None;
        }
        source
            .syntax
            .input_scope_at(ByteOffset::new(source_range.start), &source.text)
    }

    /// 组合输出位置映射到可编辑源后，查询该处应补全的 JSX/TSX 闭合标签文本。
    pub fn jsx_tag_close_text_at(&self, offset: MultiBufferOffset) -> Option<String> {
        let (mapping, source, source_offset) = self.source_point(offset.into())?;
        if !mapping.editable {
            return None;
        }
        source
            .syntax
            .jsx_tag_close_text_at(source_offset, &source.text)
    }

    pub fn bracket_pairs_at(&self, offset: impl Into<MultiBufferOffset>) -> Vec<BracketPair> {
        let offset: MultiBufferOffset = offset.into();
        let offset = ByteOffset::new(offset.get());
        let Some((mapping, source, source_offset)) = self.source_point(offset) else {
            return Vec::new();
        };
        let excerpt_start = mapping.source_range.start().get();
        let excerpt_end = mapping.source_range.end().get();
        let query_start = source_offset.get().saturating_sub(1).max(excerpt_start);
        let query_end = source_offset.get().saturating_add(1).min(excerpt_end);
        source
            .syntax
            .bracket_pairs(query_start..query_end, &source.text)
            .into_iter()
            .filter(|pair| {
                pair.open.start >= excerpt_start
                    && pair.close.end <= excerpt_end
                    && pair.open.start < pair.open.end
                    && pair.close.start < pair.close.end
            })
            .map(|pair| {
                let output_start = mapping.output_range.start().get();
                BracketPair {
                    open: (output_start + pair.open.start - excerpt_start)
                        ..(output_start + pair.open.end - excerpt_start),
                    close: (output_start + pair.close.start - excerpt_start)
                        ..(output_start + pair.close.end - excerpt_start),
                    newline_only: pair.newline_only,
                }
            })
            .collect()
    }

    /// 查询选区所处的 `brackets.scm` 换行括号。
    ///
    /// 查询使用完整 excerpt，避免开闭捕获落在光标两侧时被局部 query 范围截断；
    /// 结果仍限制在同一可编辑源片段内，并要求括号与选区之间只有非换行空白。
    pub fn newline_only_bracket_at(&self, range: MultiBufferRange) -> bool {
        let start = range.start().get();
        let end = range.end().get();
        let Some((mapping, source, source_range)) = self.source_range(start..end) else {
            return false;
        };
        if !mapping.editable {
            return false;
        }
        let pairs = source.syntax.bracket_pairs(
            mapping.source_range.start().get()..mapping.source_range.end().get(),
            &source.text,
        );
        let selection_start = source_range.start;
        let selection_end = source_range.end;
        let pair = pairs
            .into_iter()
            .filter(|pair| {
                pair.open.start >= mapping.source_range.start().get()
                    && pair.close.end <= mapping.source_range.end().get()
                    && pair.open.end <= selection_start
                    && selection_end <= pair.close.start
            })
            .min_by_key(|pair| pair.close.end - pair.open.start);
        let Some(pair) = pair else {
            return false;
        };
        if !pair.newline_only {
            return false;
        }
        let between = |range: Range<usize>| {
            source
                .text
                .slice_byte_range(ByteOffset::new(range.start), ByteOffset::new(range.end))
                .map(|text| {
                    text.as_str().chars().all(|character| {
                        character.is_whitespace() && character != '\n' && character != '\r'
                    })
                })
                .unwrap_or(false)
        };
        between(pair.open.end..selection_start) && between(selection_end..pair.close.start)
    }

    /// 在同一可编辑 excerpt 的源行内检查输入配对两侧的非换行空白。
    pub fn newline_input_pair_at(&self, range: MultiBufferRange, open: &str, close: &str) -> bool {
        let Some((mapping, source, source_range)) =
            self.source_range(range.start().get()..range.end().get())
        else {
            return false;
        };
        if !mapping.editable {
            return false;
        }
        let Ok(line) = source
            .text
            .byte_to_line(ByteOffset::new(source_range.start))
        else {
            return false;
        };
        let Ok(line_start) = source.text.line_start_byte(line) else {
            return false;
        };
        let line_end = if line.get() + 1 < source.text.line_count() {
            let Ok(next) = source.text.line_start_byte(Line::new(line.get() + 1)) else {
                return false;
            };
            next.get()
        } else {
            source.text.len_bytes().get()
        };
        let start = line_start.get().max(mapping.source_range.start().get());
        let end = line_end.min(mapping.source_range.end().get());
        let Ok(before) = source
            .text
            .slice_byte_range(ByteOffset::new(start), ByteOffset::new(source_range.start))
        else {
            return false;
        };
        let Ok(after) = source
            .text
            .slice_byte_range(ByteOffset::new(source_range.end), ByteOffset::new(end))
        else {
            return false;
        };
        before
            .as_str()
            .trim_end_matches([' ', '\t'])
            .ends_with(open.trim_end())
            && after
                .as_str()
                .trim_start_matches([' ', '\t'])
                .starts_with(close.trim_start())
    }

    /// 查询组合坐标中光标所在 source 的换行缩进建议。
    pub fn suggested_newline_indent(
        &self,
        offset: impl Into<MultiBufferOffset>,
    ) -> TextResult<NewlineIndent> {
        let offset: MultiBufferOffset = offset.into();
        let offset = ByteOffset::new(offset.get());
        let Some((_, source, source_offset)) = self.source_point(offset) else {
            return Ok(NewlineIndent {
                base_indent: String::new(),
                additional_levels: 0,
            });
        };
        source
            .syntax
            .suggested_newline_indent(source_offset, &source.text)
    }

    /// 将选区扩展到当前语法层中严格包围它的下一个节点。
    pub fn expand_selection_range(&self, range: Range<usize>) -> Option<Range<usize>> {
        let (mapping, source, source_range) = self.source_range(range.clone())?;
        let ancestor = source
            .syntax
            .expand_selection_range(source_range, &source.text)?;
        project_range(ancestor, mapping.source_range.range(), mapping.output_range)
    }
}
