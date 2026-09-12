//! 局部绑定与引用查询。
//!
//! 每个语法层独立解析作用域，引用不会跨注入语言绑定。
//! 查询可以接收子范围，但解析仍在完整语法树上进行，以保留范围外参数和外层定义。

use std::collections::HashSet;
use std::ops::Range;

use tree_sitter::StreamingIterator;
use zcv_text::{BufferVersion, Snapshot};

use crate::syntax_map::SyntaxSnapshot;
use crate::tree_sitter_utils::{QueryCursorHandle, SnapshotTextProvider, encloses, node_text};

/// 当前语法快照内一个可确定归属的局部名称。
///
/// 只有能按作用域包含关系确定归属的引用才会放入 `references`；
/// 未解析或有歧义的名称不会被猜测归并。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalBinding {
    /// 产生结果的文本版本。
    pub version: BufferVersion,
    /// 绑定定义的名称。
    pub name: String,
    /// 定义名称范围。
    pub definition_range: Range<usize>,
    /// 解析到此定义的引用范围。
    pub references: Vec<Range<usize>>,
    /// 定义所属的最小作用域范围。
    pub scope_range: Range<usize>,
    /// 定义节点的语法种类。
    pub kind: String,
    /// 产生结果的语法层名称。
    pub language: &'static str,
    /// 产生结果的注入层深度，主语言层为 0。
    pub language_depth: u32,
}

#[derive(Clone, Debug)]
struct LocalDefinition {
    range: Range<usize>,
    name: String,
    kind: String,
}

#[derive(Clone, Debug)]
struct LocalReference {
    range: Range<usize>,
    name: String,
}

impl SyntaxSnapshot {
    /// 查询指定范围内可以确定归属的局部绑定及其引用。
    ///
    /// 查询在完整语法层上执行，以便范围内的引用可以解析到范围外的参数或外层定义；
    /// 最终结果才按调用方范围裁剪。不同注入层分别解析，引用不会跨语言层绑定。
    pub fn local_bindings(&self, range: Range<usize>, text: &Snapshot) -> Vec<LocalBinding> {
        if !self.can_query(&range, text) {
            return Vec::new();
        }

        let mut bindings = Vec::new();
        for layer in self.layers_for_range(&range) {
            let Some(query) = layer.language.locals() else {
                continue;
            };
            let names = query.capture_names();
            let tree_range = layer.tree.root_node().byte_range();
            let mut cursor = QueryCursorHandle::new();
            cursor.set_byte_range(tree_range.clone());
            let mut matches =
                cursor.matches(query, layer.tree.root_node(), SnapshotTextProvider(text));
            let mut scopes = vec![tree_range];
            let mut definitions = Vec::new();
            let mut references = Vec::new();
            let mut seen_scopes = HashSet::new();
            let mut seen_definitions = HashSet::new();
            let mut seen_references = HashSet::new();

            while let Some(query_match) = matches.next() {
                for capture in query_match.captures {
                    let node = capture.node;
                    let node_range = node.byte_range();
                    match names.get(capture.index as usize).copied() {
                        Some("local.scope") => {
                            if seen_scopes.insert(node_range.clone()) {
                                scopes.push(node_range);
                            }
                        }
                        Some("local.definition") => {
                            let Some(name) = node_text(text, node_range.clone()) else {
                                continue;
                            };
                            if !name.is_empty()
                                && seen_definitions.insert((node_range.clone(), name.clone()))
                            {
                                definitions.push(LocalDefinition {
                                    range: node_range,
                                    name,
                                    kind: node.kind().to_owned(),
                                });
                            }
                        }
                        Some("local.reference") => {
                            let Some(name) = node_text(text, node_range.clone()) else {
                                continue;
                            };
                            if !name.is_empty()
                                && seen_references.insert((node_range, name.clone()))
                            {
                                references.push(LocalReference {
                                    range: node.byte_range(),
                                    name,
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }

            scopes.sort_unstable_by_key(|scope| (scope.start, scope.end));
            definitions
                .sort_unstable_by_key(|definition| (definition.range.start, definition.range.end));
            references
                .sort_unstable_by_key(|reference| (reference.range.start, reference.range.end));

            for definition in &definitions {
                let scope_range = smallest_scope(&scopes, &definition.range);
                let definition_references = references
                    .iter()
                    .filter(|reference| {
                        reference.range != definition.range
                            && reference.name == definition.name
                            && range_intersects(&reference.range, &range)
                    })
                    .filter_map(|reference| {
                        resolve_definition(&reference.name, &reference.range, &definitions, &scopes)
                            .filter(|resolved| resolved.range == definition.range)
                            .map(|_| reference.range.clone())
                    })
                    .collect::<Vec<_>>();

                if range_intersects(&definition.range, &range) || !definition_references.is_empty()
                {
                    bindings.push(LocalBinding {
                        version: self.version,
                        name: definition.name.clone(),
                        definition_range: definition.range.clone(),
                        references: definition_references,
                        scope_range,
                        kind: definition.kind.clone(),
                        language: layer.language.name(),
                        language_depth: layer.depth,
                    });
                }
            }
        }

        bindings.sort_unstable_by_key(|binding| {
            (
                binding.definition_range.start,
                binding.definition_range.end,
                binding.language_depth,
            )
        });
        bindings
    }
}

fn smallest_scope(scopes: &[Range<usize>], range: &Range<usize>) -> Range<usize> {
    scopes
        .iter()
        .filter(|scope| encloses(scope, range))
        .min_by_key(|scope| scope.len())
        .cloned()
        .unwrap_or_else(|| range.clone())
}

fn resolve_definition<'a>(
    name: &str,
    reference: &Range<usize>,
    definitions: &'a [LocalDefinition],
    scopes: &[Range<usize>],
) -> Option<&'a LocalDefinition> {
    let reference_scope = smallest_scope(scopes, reference);
    definitions
        .iter()
        .filter(|definition| {
            definition.name == name
                && definition.range.start <= reference.start
                && encloses(&smallest_scope(scopes, &definition.range), &reference_scope)
        })
        .min_by_key(|definition| {
            let scope = smallest_scope(scopes, &definition.range);
            (scope.len(), std::cmp::Reverse(definition.range.start))
        })
}

fn range_intersects(left: &Range<usize>, right: &Range<usize>) -> bool {
    if left.start == left.end {
        return left.start >= right.start && left.start <= right.end;
    }
    if right.start == right.end {
        return right.start >= left.start && right.start <= left.end;
    }
    left.start < right.end && right.start < left.end
}
