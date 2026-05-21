//! 项目目录树（文件树面板的数据源）。
//!
//! 持有项目根、按需懒加载的目录子项缓存，以及目录展开状态。子项排序规则：
//! 目录优先、字母序（不区分大小写）。
//!
//! 这是一个面向只读浏览的快照层：`visible_rows` 给 UI 直接消费；`expand`
//! / `collapse` / `toggle` 由命令侧调用。本层不负责文件内容、git 状态、
//! watch 失效——这些后续再叠加。

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// 节点类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryKind {
    Directory,
    File,
}

/// 一个目录下的单个子项。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeEntry {
    pub path: PathBuf,
    pub name: String,
    pub kind: EntryKind,
}

/// 用于渲染的一行。
///
/// 借用 [`ProjectTree`] 内部缓存，避免快照时复制大量 PathBuf。生命周期与
/// 产出它的 `visible_rows` 调用保持一致。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TreeRow<'a> {
    pub path: &'a Path,
    pub name: &'a str,
    pub depth: usize,
    pub kind: EntryKind,
    /// 仅对目录有意义；文件恒为 false。
    pub expanded: bool,
}

/// 项目目录树。
pub struct ProjectTree {
    root: PathBuf,
    /// 项目根的展示名（取 `root.file_name()`；空时回落到完整路径）。
    /// 缓存在结构体里，避免 `visible_rows` 借出 `&str` 时无处寄存。
    root_name: String,
    /// 已读取过的目录及其排序好的子项列表。
    children: HashMap<PathBuf, Vec<TreeEntry>>,
    /// 当前展开的目录路径集合。根目录默认在集合里。
    expanded: HashSet<PathBuf>,
}

impl ProjectTree {
    /// 新建并立刻加载根目录子项；根目录读取失败时返回 IO 错误。
    pub fn new(root: PathBuf) -> io::Result<Self> {
        let root_name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.to_string_lossy().into_owned());
        let mut expanded = HashSet::new();
        expanded.insert(root.clone());
        let mut tree = Self {
            root: root.clone(),
            root_name,
            children: HashMap::new(),
            expanded,
        };
        tree.load_dir(&root)?;
        Ok(tree)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn is_expanded(&self, path: &Path) -> bool {
        self.expanded.contains(path)
    }

    /// 展开目录。
    ///
    /// 第一次展开时按需读盘；读盘失败的目录保持折叠并保留错误的语义（调用方
    /// 拿到 `Err` 自行决定如何提示）。
    pub fn expand(&mut self, path: &Path) -> io::Result<()> {
        self.ensure_loaded(path)?;
        self.expanded.insert(path.to_path_buf());
        Ok(())
    }

    pub fn collapse(&mut self, path: &Path) {
        self.expanded.remove(path);
    }

    pub fn toggle(&mut self, path: &Path) -> io::Result<()> {
        if self.is_expanded(path) {
            self.collapse(path);
            Ok(())
        } else {
            self.expand(path)
        }
    }

    /// 在 `parent` 目录下创建一个文件 / 子目录，并刷新该目录的子项缓存，使
    /// `visible_rows` 立即反映新条目。返回新条目的完整路径。
    ///
    /// 同名条目已存在时返回 [`io::ErrorKind::AlreadyExists`]，不会覆盖。
    pub fn create_entry(
        &mut self,
        parent: &Path,
        name: &str,
        kind: EntryKind,
    ) -> io::Result<PathBuf> {
        let path = parent.join(name);
        match kind {
            EntryKind::File => {
                fs::File::create_new(&path)?;
            }
            EntryKind::Directory => fs::create_dir(&path)?,
        }
        // 丢弃父目录缓存并重读，让新条目按既有排序规则进入 visible_rows。
        self.children.remove(parent);
        self.load_dir(parent)?;
        Ok(path)
    }

    /// 自根向下做 DFS，按目录优先 + 字母序产出可见行。
    ///
    /// 注意：返回值借用 `&self`，调用 `expand`/`collapse` 前必须先把它丢弃。
    /// 项目根本身占第一行（depth=0）；其子项在 depth=1。
    pub fn visible_rows(&self) -> Vec<TreeRow<'_>> {
        let mut rows = Vec::new();
        let root_expanded = self.expanded.contains(&self.root);
        rows.push(TreeRow {
            path: &self.root,
            name: &self.root_name,
            depth: 0,
            kind: EntryKind::Directory,
            expanded: root_expanded,
        });
        if root_expanded {
            self.collect_visible(&self.root, 1, &mut rows);
        }
        rows
    }

    fn collect_visible<'a>(&'a self, dir: &Path, depth: usize, rows: &mut Vec<TreeRow<'a>>) {
        let Some(entries) = self.children.get(dir) else {
            return;
        };
        for entry in entries {
            let expanded =
                matches!(entry.kind, EntryKind::Directory) && self.expanded.contains(&entry.path);
            rows.push(TreeRow {
                path: &entry.path,
                name: &entry.name,
                depth,
                kind: entry.kind,
                expanded,
            });
            if expanded {
                self.collect_visible(&entry.path, depth + 1, rows);
            }
        }
    }

    fn ensure_loaded(&mut self, dir: &Path) -> io::Result<()> {
        if self.children.contains_key(dir) {
            return Ok(());
        }
        self.load_dir(dir)
    }

    fn load_dir(&mut self, dir: &Path) -> io::Result<()> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            // symlink 暂按其目标类型对待：is_dir / is_file 会跟随。后续若要
            // 单独标识 symlink，再扩 EntryKind。
            let kind = if file_type.is_dir() {
                EntryKind::Directory
            } else {
                EntryKind::File
            };
            entries.push(TreeEntry {
                path: entry.path(),
                name: entry.file_name().to_string_lossy().into_owned(),
                kind,
            });
        }
        entries.sort_by(|a, b| match (a.kind, b.kind) {
            (EntryKind::Directory, EntryKind::File) => std::cmp::Ordering::Less,
            (EntryKind::File, EntryKind::Directory) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        });
        self.children.insert(dir.to_path_buf(), entries);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{File, create_dir_all};
    use std::path::PathBuf;

    fn tmp_root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("zom-project-tree-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn root_display_name(root: &Path) -> String {
        root.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.to_string_lossy().into_owned())
    }

    #[test]
    fn root_should_render_as_first_row_and_subdirs_should_load_on_expand() {
        let root = tmp_root("lazy");
        create_dir_all(root.join("src/inner")).unwrap();
        File::create(root.join("README.md")).unwrap();
        File::create(root.join("src/lib.rs")).unwrap();
        File::create(root.join("src/inner/mod.rs")).unwrap();

        let mut tree = ProjectTree::new(root.clone()).unwrap();
        let root_name = root_display_name(&root);
        let rows: Vec<_> = tree
            .visible_rows()
            .into_iter()
            .map(|row| (row.name.to_string(), row.depth, row.kind, row.expanded))
            .collect();
        // 根行 depth=0 默认展开；其下两项 depth=1（目录优先 + 字母序）。
        assert_eq!(
            rows,
            vec![
                (root_name.clone(), 0, EntryKind::Directory, true),
                ("src".to_string(), 1, EntryKind::Directory, false),
                ("README.md".to_string(), 1, EntryKind::File, false),
            ]
        );

        tree.expand(&root.join("src")).unwrap();
        let rows: Vec<_> = tree
            .visible_rows()
            .into_iter()
            .map(|row| (row.name.to_string(), row.depth))
            .collect();
        assert_eq!(
            rows,
            vec![
                (root_name.clone(), 0),
                ("src".to_string(), 1),
                ("inner".to_string(), 2),
                ("lib.rs".to_string(), 2),
                ("README.md".to_string(), 1),
            ]
        );

        tree.collapse(&root.join("src"));
        assert_eq!(tree.visible_rows().len(), 3);

        // 折叠根目录后只剩根这一行。
        tree.collapse(&root);
        assert_eq!(tree.visible_rows().len(), 1);
    }

    #[test]
    fn create_entry_should_write_to_disk_and_refresh_rows() {
        let root = tmp_root("create");
        let mut tree = ProjectTree::new(root.clone()).unwrap();

        let file = tree.create_entry(&root, "new.txt", EntryKind::File).unwrap();
        assert!(file.is_file());
        let dir = tree
            .create_entry(&root, "newdir", EntryKind::Directory)
            .unwrap();
        assert!(dir.is_dir());

        let names: Vec<_> = tree
            .visible_rows()
            .into_iter()
            .map(|row| row.name.to_string())
            .collect();
        assert!(names.contains(&"new.txt".to_string()));
        assert!(names.contains(&"newdir".to_string()));

        // 重复创建不覆盖，报 AlreadyExists。
        assert_eq!(
            tree.create_entry(&root, "new.txt", EntryKind::File)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
    }

    #[test]
    fn entries_should_be_sorted_dirs_first_then_alpha() {
        let root = tmp_root("sort");
        create_dir_all(root.join("b_dir")).unwrap();
        create_dir_all(root.join("A_dir")).unwrap();
        File::create(root.join("zfile")).unwrap();
        File::create(root.join("Afile")).unwrap();

        let tree = ProjectTree::new(root.clone()).unwrap();
        let names: Vec<_> = tree
            .visible_rows()
            .into_iter()
            .map(|row| row.name.to_string())
            .collect();
        assert_eq!(
            names,
            vec![
                root_display_name(&root),
                "A_dir".to_string(),
                "b_dir".to_string(),
                "Afile".to_string(),
                "zfile".to_string(),
            ]
        );
    }
}
