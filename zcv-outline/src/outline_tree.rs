//! 大纲面板的「目录 → 文件 → 符号」树模型。
//!
//! 面板只持有带文件身份的大纲条目（`OutlineEntry`）；
//! 本模块把它按 display_path 组织成树，并计算折叠后的可见行。
//! 单文件文档保持扁平符号列表，多文件文档才出现目录与文件节点。

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use zcv_editor::OutlineEntry;
use zcv_ui::{AutoFoldDir, auto_fold_dirs};

use crate::outline_item::OutlineItemKey;

/// 大纲树中的一行。
#[derive(Clone, Debug)]
pub(crate) struct OutlineRow {
    pub(crate) depth: usize,
    /// 是否存在可折叠的子行；由树结构决定，不从相邻行推断。
    pub(crate) has_children: bool,
    pub(crate) kind: OutlineRowKind,
}

/// 行类型：目录、文件或符号。
#[derive(Clone, Debug)]
pub(crate) enum OutlineRowKind {
    Directory { path: PathBuf, name: String },
    File { path: PathBuf, name: String },
    Symbol(Arc<OutlineEntry>),
}

/// 行的折叠身份：目录与文件按路径，符号按输出范围。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum OutlineRowKey {
    Directory(PathBuf),
    File(PathBuf),
    Symbol(OutlineItemKey),
}

impl OutlineRow {
    pub(crate) fn key(&self) -> OutlineRowKey {
        match &self.kind {
            OutlineRowKind::Directory { path, .. } => OutlineRowKey::Directory(path.clone()),
            OutlineRowKind::File { path, .. } => OutlineRowKey::File(path.clone()),
            OutlineRowKind::Symbol(entry) => {
                OutlineRowKey::Symbol(OutlineItemKey::from_item(&entry.item))
            }
        }
    }
}

/// 文档是否包含两个及以上不同文件；决定是否构建文件树。
pub(crate) fn has_multiple_files(entries: &[Arc<OutlineEntry>]) -> bool {
    let mut first: Option<&Path> = None;
    for entry in entries {
        match first {
            None => first = Some(entry.display_path.as_path()),
            Some(path) if path != entry.display_path.as_path() => return true,
            Some(_) => {}
        }
    }
    false
}

/// 按查询过滤后构建大纲行。
///
/// 目录链的自动折叠需要当前展开状态：折叠集里已显式折叠的目录是折叠边界。
pub(crate) fn outline_rows(
    entries: &[Arc<OutlineEntry>],
    tree: bool,
    query: &str,
    collapsed: &HashSet<OutlineRowKey>,
) -> Vec<OutlineRow> {
    let filtered = if query.is_empty() {
        entries.to_vec()
    } else {
        entries
            .iter()
            .filter(|entry| entry.item.text.to_lowercase().contains(query))
            .cloned()
            .collect()
    };
    if !tree {
        return flat_symbol_rows(&filtered);
    }
    let mut root = OutlineDir::default();
    for entry in filtered {
        root.insert(entry);
    }
    let mut rows = Vec::new();
    root.flatten(0, collapsed, &mut rows);
    rows
}

/// 单文件文档：符号直接按源内 depth 排列，子项仍按相邻 depth 推断。
fn flat_symbol_rows(entries: &[Arc<OutlineEntry>]) -> Vec<OutlineRow> {
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| OutlineRow {
            depth: entry.item.depth,
            has_children: entries
                .get(index + 1)
                .is_some_and(|next| next.item.depth > entry.item.depth),
            kind: OutlineRowKind::Symbol(entry.clone()),
        })
        .collect()
}

/// 折叠后的可见行：折叠行隐藏其后所有更深层级的后代，但不影响同层兄弟。
pub(crate) fn visible_rows(
    rows: &[OutlineRow],
    collapsed: &HashSet<OutlineRowKey>,
) -> Vec<(OutlineRow, bool, bool)> {
    let mut visible = Vec::new();
    let mut collapsed_depth = None;
    for row in rows {
        if collapsed_depth.is_some_and(|depth| row.depth > depth) {
            continue;
        }
        collapsed_depth = None;
        let is_collapsed = row.has_children && collapsed.contains(&row.key());
        visible.push((row.clone(), row.has_children, is_collapsed));
        if is_collapsed {
            collapsed_depth = Some(row.depth);
        }
    }
    visible
}

/// 目录子树：每个节点携带自己的显示路径与名字，子目录按名字索引，保证输出顺序稳定。
#[derive(Default)]
struct OutlineDir {
    path: PathBuf,
    name: String,
    dirs: BTreeMap<String, OutlineDir>,
    files: BTreeMap<PathBuf, Vec<Arc<OutlineEntry>>>,
}

impl OutlineDir {
    fn insert(&mut self, entry: Arc<OutlineEntry>) {
        let path = entry.display_path.clone();
        let mut node = self;
        let mut accumulated = PathBuf::new();
        if let Some(parent) = path.parent() {
            for component in parent.components() {
                let name = component.as_os_str().to_string_lossy().into_owned();
                accumulated.push(component.as_os_str());
                node = node.dirs.entry(name.clone()).or_insert_with(|| OutlineDir {
                    path: accumulated.clone(),
                    name: name.clone(),
                    ..Default::default()
                });
            }
        }
        node.files.entry(path).or_default().push(entry);
    }

    fn flatten(
        &self,
        depth: usize,
        collapsed: &HashSet<OutlineRowKey>,
        rows: &mut Vec<OutlineRow>,
    ) {
        for child in self.dirs.values() {
            let (deepest, name) = auto_fold_dirs(child, |node| {
                !collapsed.contains(&OutlineRowKey::Directory(node.path.clone()))
            });
            rows.push(OutlineRow {
                depth,
                has_children: deepest.has_children(),
                kind: OutlineRowKind::Directory {
                    path: deepest.path.clone(),
                    name,
                },
            });
            if !collapsed.contains(&OutlineRowKey::Directory(deepest.path.clone())) {
                deepest.flatten(depth + 1, collapsed, rows);
            }
        }
        for (path, entries) in &self.files {
            rows.push(OutlineRow {
                depth,
                has_children: !entries.is_empty(),
                kind: OutlineRowKind::File {
                    path: path.clone(),
                    name: display_name(path),
                },
            });
            for (index, entry) in entries.iter().enumerate() {
                rows.push(OutlineRow {
                    depth: depth + 1 + entry.item.depth,
                    has_children: entries
                        .get(index + 1)
                        .is_some_and(|next| next.item.depth > entry.item.depth),
                    kind: OutlineRowKind::Symbol(entry.clone()),
                });
            }
        }
    }

    fn has_children(&self) -> bool {
        !self.dirs.is_empty() || !self.files.is_empty()
    }
}

impl AutoFoldDir for OutlineDir {
    fn dir_name(&self) -> &str {
        &self.name
    }

    fn single_dir_child(&self) -> Option<&Self> {
        if self.files.is_empty() && self.dirs.len() == 1 {
            self.dirs.values().next()
        } else {
            None
        }
    }
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}
