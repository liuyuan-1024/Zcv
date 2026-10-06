//! JSX/TSX 标签自动闭合的语法结构查询。
//!
//! 输入 `>` 后是否补全闭合标签由语法树决定，与通用括号配对完全分离：
//! 这里只回答“光标处的开放标签是否尚未闭合、闭合标签文本是什么”，编辑事务仍由唯一的 Editor 输入管线提交。

use tree_sitter::Node;
use zcv_text::{ByteOffset, Snapshot};

use crate::JsxTagAutoCloseConfig;
use crate::syntax_map::SyntaxSnapshot;
use crate::tree_sitter_utils::node_text;

/// 检查开放标签是否已闭合时，最多沿父元素回看两层。
///
/// 继续向上遍历整棵树代价高且容易把远处的同名闭合标签误判为当前标签的闭合。
const ALREADY_CLOSED_PARENT_ELEMENT_WALK_BACK_LIMIT: usize = 2;

impl SyntaxSnapshot {
    /// 在刚输入 `>` 之后的光标位置判断是否应补全 JSX/TSX 闭合标签。
    ///
    /// 返回闭合标签文本（如 `</div>`）；
    /// 当前语法层没有标签结构、开放标签已闭合、或该 `>` 不属于开放标签时返回 `None`。
    pub fn jsx_tag_close_text_at(&self, offset: ByteOffset, text: &Snapshot) -> Option<String> {
        assert_eq!(
            self.version(),
            text.version(),
            "标签结构查询要求文本与语法同版本"
        );
        let caret = offset.get();
        if caret == 0 || caret > text.len_bytes().get() {
            return None;
        }
        // 编辑区间只覆盖刚插入的 `>`；它不是开放标签的结束符时不补全。
        let edited = caret - 1..caret;
        if node_text(text, edited.clone()).as_deref() != Some(">") {
            return None;
        }
        // 文末光标点在节点右开区间之外，按 `>` 自身的位置选层。
        let layer = self.selected_layer_at(edited.start, text)?;
        let config = layer.language.jsx_tag_auto_close()?;
        let root = layer.tree.root_node();
        let node = root.named_descendant_for_byte_range(edited.start, edited.end)?;
        let open_tag = if node.kind() == config.open_tag_node_name {
            node
        } else if let Some(parent) = node.parent()
            && parent.kind() == config.open_tag_node_name
        {
            parent
        } else {
            return None;
        };
        // 片段以外必须以 `<` 开头；文档类型与闭合标签不自动补全。
        let open_tag_text = node_text(text, open_tag.byte_range())?;
        let mut prefix = open_tag_text.chars();
        if prefix.next() != Some('<') || matches!(prefix.next(), Some('!') | Some('/')) {
            return None;
        }
        let tag_name = jsx_tag_name(&open_tag, &config, text);
        has_unclosed_open_tag(&open_tag, &config, &tag_name, text).then(|| format!("</{tag_name}>"))
    }
}

/// 开放标签的标签名：第一个命名子节点在配置的种类内时取其文本，否则视为空（如片段 `<>`）。
fn jsx_tag_name(open_tag: &Node, config: &JsxTagAutoCloseConfig, text: &Snapshot) -> String {
    open_tag
        .named_child(0)
        .filter(|node| {
            node.kind() == config.tag_name_node_name
                || config
                    .tag_name_node_alternates
                    .iter()
                    .any(|alternate| *alternate == node.kind())
        })
        .and_then(|node| node_text(text, node.byte_range()))
        .unwrap_or_default()
}

/// 节点标签名是否等于给定名字；没有命名子节点时只有空名字视为相等。
fn tag_node_name_equals(node: &Node, name: &str, text: &Snapshot) -> bool {
    match node.named_child(0) {
        Some(node_name) => node_text(text, node_name.byte_range()).is_some_and(|text| text == name),
        None => name.is_empty(),
    }
}

/// 从开放标签所在子树统计同名开放与闭合标签，判断是否存在未闭合的开放标签。
///
/// 只沿 `jsx_element` 向下遍历，并在错误节点或不同名元素处停止向上回看；
/// 与 Zed 的朴素计数一致，不尝试构造完整错误恢复树。
fn has_unclosed_open_tag(
    open_tag: &Node,
    config: &JsxTagAutoCloseConfig,
    tag_name: &str,
    text: &Snapshot,
) -> bool {
    let mut ancestors = Vec::new();
    let mut current = open_tag.parent();
    while let Some(node) = current {
        ancestors.push(node);
        current = node.parent();
    }

    let mut tree_root_node = *open_tag;
    let mut parent_element_node_count = 0usize;
    let mut doing_deep_search = false;
    for ancestor in &ancestors {
        tree_root_node = *ancestor;
        let is_element = ancestor.kind() == config.jsx_element_node_name;
        if ancestor.is_error() || !is_element {
            break;
        }
        let is_first = parent_element_node_count == 0;
        if !is_first {
            let has_open_tag_with_same_tag_name = ancestor
                .named_child(0)
                .filter(|node| node.kind() == config.open_tag_node_name)
                .is_some_and(|node| tag_node_name_equals(&node, tag_name, text));
            if has_open_tag_with_same_tag_name {
                doing_deep_search = true;
            } else if doing_deep_search {
                break;
            }
        }
        parent_element_node_count += 1;
        if !doing_deep_search
            && parent_element_node_count >= ALREADY_CLOSED_PARENT_ELEMENT_WALK_BACK_LIMIT
        {
            break;
        }
    }

    let mut unclosed_open_tag_count: i32 = 0;
    let mut stack: Vec<Node> = children(&tree_root_node);
    while let Some(node) = stack.pop() {
        let kind = node.kind();
        if kind == config.open_tag_node_name {
            if tag_node_name_equals(&node, tag_name, text) {
                unclosed_open_tag_count += 1;
            }
        } else if kind == config.close_tag_node_name {
            if tag_node_name_equals(&node, tag_name, text) {
                unclosed_open_tag_count -= 1;
            }
        } else if kind == config.jsx_element_node_name {
            stack.extend(children(&node));
        }
    }
    unclosed_open_tag_count > 0
}

fn children<'a>(node: &Node<'a>) -> Vec<Node<'a>> {
    (0..node.child_count() as u32)
        .filter_map(|index| node.child(index))
        .collect()
}
