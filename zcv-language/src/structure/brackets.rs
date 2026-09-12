//! 语法括号配对查询。
//!
//! 配对来自语言注册的 `brackets.scm`；
//! 模块只返回源文本范围，不负责自动闭合输入。

use std::ops::Range;

use tree_sitter::StreamingIterator;
use zcv_text::Snapshot;

use crate::syntax_map::SyntaxSnapshot;
use crate::tree_sitter_utils::{QueryCursorHandle, SnapshotTextProvider};

/// 语法查询识别出的开闭范围。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BracketPair {
    pub open: Range<usize>,
    pub close: Range<usize>,
}

impl SyntaxSnapshot {
    /// 查询范围内的括号配对，并按开括号位置排序。
    pub fn bracket_pairs(&self, range: Range<usize>, text: &Snapshot) -> Vec<BracketPair> {
        if !self.can_query(&range, text) {
            return Vec::new();
        }
        let mut pairs = Vec::new();
        for layer in self.layers_for_range(&range) {
            let Some(query) = layer.language.brackets() else {
                continue;
            };
            let names = query.capture_names();
            let mut cursor = QueryCursorHandle::new();
            cursor.set_byte_range(range.clone());
            let mut matches =
                cursor.matches(query, layer.tree.root_node(), SnapshotTextProvider(text));
            while let Some(query_match) = matches.next() {
                let mut open = None;
                let mut close = None;
                for capture in query_match.captures {
                    match names.get(capture.index as usize).copied() {
                        Some("open") => open = Some(capture.node.byte_range()),
                        Some("close") => close = Some(capture.node.byte_range()),
                        _ => {}
                    }
                }
                if let (Some(open), Some(close)) = (open, close) {
                    pairs.push(BracketPair { open, close });
                }
            }
        }
        pairs.sort_unstable_by_key(|pair| (pair.open.start, pair.close.end));
        pairs
    }
}
