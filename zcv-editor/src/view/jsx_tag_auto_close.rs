//! JSX/TSX 标签自动闭合输入。
//!
//! 输入 `>` 后先提交该字符，再用提交后的语法树判断开放标签是否尚未闭合；
//! 闭合标签作为同一事务的第二段编辑插入，并让光标停在开闭标签之间。
//! 语法树尚未推进到当前文本时不做补全，不用旧节点猜测结构。

use std::sync::Arc;

use gpui::Context;
use zcv_multi_buffer::MultiBufferOffset;
use zcv_text::Edit;

use super::Editor;
use super::input::input_metadata;
use crate::selection::{Selection, SelectionSet, apply_edits};

impl Editor {
    /// 输入 `>` 时按当前 JSX/TSX 语法树在同一事务中补全闭合标签。
    ///
    /// 命中时提交 `>` 与闭合标签并返回 true；输入不是单个 `>`、语法层没有标签结构、
    /// 开放标签已闭合或解析尚未落地时都按普通输入处理并返回 false。
    pub(super) fn try_jsx_tag_autoclose(
        &mut self,
        text: &str,
        before: &SelectionSet,
        cx: &mut Context<Self>,
    ) -> bool {
        if text != ">" {
            return false;
        }
        // 只在当前语法层的语言声明了 JSX 标签结构时进入两段提交；
        // 其他语言的 `>` 继续走普通输入路径，不额外查询语法树。
        let has_jsx_structure = {
            let snapshot = self.display_snapshot(cx).buffer_snapshot().clone();
            before.as_slice().iter().any(|selection| {
                snapshot
                    .input_scope_at(selection.head())
                    .is_some_and(|scope| scope.jsx_tag_auto_close().is_some())
            })
        };
        if !has_jsx_structure {
            return false;
        }
        let targets: Vec<(Selection, Arc<str>)> = before
            .as_slice()
            .iter()
            .map(|selection| (*selection, Arc::<str>::from(text)))
            .collect();
        self.change_with_post_snapshot_edits(
            before.clone(),
            input_metadata("输入文本", false),
            cx,
            |plan| {
                let outcome = apply_edits(plan, &targets)?;
                // 第一段结束后每个选区都落在刚插入的 `>` 之后。
                let position_map = outcome.position_map().cloned().unwrap_or_default();
                let carets = before
                    .as_slice()
                    .iter()
                    .map(|selection| {
                        let start = position_map
                            .map_old_position(selection.start().into())
                            .value();
                        Selection::caret(if selection.is_caret() {
                            start.into()
                        } else {
                            MultiBufferOffset::new(start.get() + text.len())
                        })
                    })
                    .collect();
                Ok((
                    outcome,
                    SelectionSet::new_with_primary(carets, before.primary_index()),
                ))
            },
            |snapshot, first_selections| {
                let mut edits = Vec::new();
                for selection in first_selections.as_slice() {
                    let caret = selection.head();
                    let Some(close_text) = snapshot.jsx_tag_close_text_at(caret) else {
                        continue;
                    };
                    edits.push(
                        Edit::insert(caret.into(), close_text)
                            .expect("零宽插入位置必然满足范围约束"),
                    );
                }
                if edits.is_empty() {
                    return Ok(None);
                }
                edits.sort_unstable_by_key(|edit| edit.range().start());
                Ok(Some((
                    edits,
                    SelectionSet::new_with_primary(
                        first_selections.as_slice().to_vec(),
                        first_selections.primary_index(),
                    ),
                )))
            },
        )
        .is_ok()
    }
}
