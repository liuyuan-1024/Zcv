//! 面板持有的树导航状态与可见行快照。

use std::{collections::HashSet, rc::Rc};

/// 树行契约：折叠/展开与祖先导航所需的行模型最小信息。
pub trait TreeRow {
    fn is_dir(&self) -> bool;
    fn depth(&self) -> usize;
    fn expanded(&self) -> bool;
}

/// 树导航状态原语：可见行缓存 + 展开集合 + 选中键。
///
/// `key_of` 决定行的身份键；返回 None 的行不可选中（如分组头）。
/// 选中分两层：`selected` 是游标（始终至多一条），`selected_set` 是多选标记集（空 = 纯单选模式）；
/// `anchor` 是 shift 区间扩展的起点，普通导航/点击会重置。
/// 只依赖标准库，不触碰数据源与渲染。
pub struct TreeState<K, Row> {
    expanded: HashSet<K>,
    selected: Option<K>,
    /// 多选标记集（空 = 纯单选模式）。
    selected_set: HashSet<K>,
    /// 可见行替换或多选标记变化时递增，供行序相关的派生快照判断失效。
    selection_generation: u64,
    /// shift 区间扩展锚点；行消失时置空。
    anchor: Option<K>,
    rows: Rc<[Row]>,
    key_of: fn(&Row) -> Option<K>,
}

impl<K: Eq + std::hash::Hash + Clone, Row: TreeRow> TreeState<K, Row> {
    pub fn new(key_of: fn(&Row) -> Option<K>) -> Self {
        Self {
            expanded: HashSet::new(),
            selected: None,
            selected_set: HashSet::new(),
            selection_generation: 0,
            anchor: None,
            rows: Rc::from([]),
            key_of,
        }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn row_snapshot(&self) -> Rc<[Row]> {
        self.rows.clone()
    }

    pub fn selected(&self) -> Option<&K> {
        self.selected.as_ref()
    }

    /// 只移动当前行，保留多选标记和范围锚点。
    pub fn move_cursor(&mut self, key: K) {
        self.selected = Some(key);
    }

    pub fn clear_selection(&mut self) {
        self.selected = None;
        self.anchor = None;
        self.clear_selected_set();
    }

    pub fn expanded(&self) -> &HashSet<K> {
        &self.expanded
    }

    pub fn expand(&mut self, key: K) -> bool {
        self.expanded.insert(key)
    }

    pub fn collapse(&mut self, key: &K) {
        self.expanded.remove(key);
    }

    /// 保留仍存在的目录，并默认展开新目录。
    pub fn reconcile_expansion(&mut self, previous: &HashSet<K>, current: &HashSet<K>) {
        self.expanded.retain(|key| current.contains(key));
        self.expanded.extend(current.difference(previous).cloned());
    }

    /// 行重建前，把所有导航键一次性迁移到新身份。
    pub fn remap_keys(&mut self, map: impl Fn(&K) -> K) {
        self.expanded = self.expanded.iter().map(&map).collect();
        self.selected = self.selected.as_ref().map(&map);
        self.anchor = self.anchor.as_ref().map(&map);
        self.set_selected_set(self.selected_set.iter().map(map).collect());
    }

    /// 更新行的显示属性，保留顺序、身份和导航元数据。
    pub fn update_row_details(&mut self, mut update: impl FnMut(&Row) -> Option<Row>) -> bool
    where
        Row: Clone,
    {
        let mut changed = false;
        for index in 0..self.rows.len() {
            let old = &self.rows[index];
            if let Some(new) = update(old) {
                assert!(
                    (self.key_of)(old) == (self.key_of)(&new)
                        && old.is_dir() == new.is_dir()
                        && old.depth() == new.depth()
                        && old.expanded() == new.expanded(),
                    "行显示属性更新必须保留导航元数据"
                );
                Rc::make_mut(&mut self.rows)[index] = new;
                changed = true;
            }
        }
        changed
    }

    pub fn selected_set(&self) -> &HashSet<K> {
        &self.selected_set
    }

    pub fn selection_generation(&self) -> u64 {
        self.selection_generation
    }

    fn set_selected_set(&mut self, selected_set: HashSet<K>) {
        self.selected_set = selected_set;
        self.selection_generation = self.selection_generation.wrapping_add(1);
    }

    fn clear_selected_set(&mut self) {
        if !self.selected_set.is_empty() {
            self.selected_set.clear();
            self.selection_generation = self.selection_generation.wrapping_add(1);
        }
    }

    /// 替换可见行；选中行消失时迁移到相邻可选行，多选集与锚点同步剪枝。
    pub fn replace_rows(&mut self, rows: Vec<Row>) {
        let previous_selected_idx = self.selected.as_ref().and_then(|selected| {
            self.rows
                .iter()
                .position(|row| (self.key_of)(row).as_ref() == Some(selected))
        });
        let next_selected = previous_selected_idx.map(|index| {
            self.rows
                .iter()
                .skip(index + 1)
                .filter_map(|row| (self.key_of)(row))
                .collect::<Vec<_>>()
        });
        let previous_selected = previous_selected_idx.map(|index| {
            self.rows
                .iter()
                .take(index)
                .rev()
                .filter_map(|row| (self.key_of)(row))
                .collect::<Vec<_>>()
        });
        self.rows = rows.into();
        let key_of = self.key_of;
        // 先把当前行键收集为集合：三处剪枝从 O(行数 × 选中集) 降为集合查找。
        let alive_keys: HashSet<K> = self.rows.iter().filter_map(key_of).collect();
        if self
            .selected
            .as_ref()
            .is_some_and(|selected| !alive_keys.contains(selected))
        {
            self.selected = next_selected
                .into_iter()
                .flatten()
                .find(|key| alive_keys.contains(key))
                .or_else(|| {
                    previous_selected
                        .into_iter()
                        .flatten()
                        .find(|key| alive_keys.contains(key))
                });
        }
        self.selected_set.retain(|key| alive_keys.contains(key));
        self.selection_generation = self.selection_generation.wrapping_add(1);
        if self
            .anchor
            .as_ref()
            .is_some_and(|anchor| !alive_keys.contains(anchor))
        {
            self.anchor = None;
        }
    }

    /// 无选中时选中第一个可选行。
    pub fn ensure_selected(&mut self) {
        if self.selected.is_some() {
            return;
        }
        if let Some(key) = self.rows.iter().find_map(|r| (self.key_of)(r)) {
            self.selected = Some(key);
        }
    }

    /// 直接设置选中键（鼠标点击等交互入口）；普通选中重置为单选态。
    pub fn select(&mut self, key: K) {
        self.selected = Some(key.clone());
        self.anchor = Some(key);
        self.clear_selected_set();
    }

    /// 当前选中行在可见行中的位置。
    pub fn selected_idx(&self) -> Option<usize> {
        let selected = self.selected.clone()?;
        self.rows
            .iter()
            .position(|r| (self.key_of)(r).as_ref() == Some(&selected))
    }

    /// 上移选中；无选中时选中最后一个可选行；普通导航重置为单选态。
    pub fn select_up(&mut self) {
        if let Some(key) = self.prev_selectable_key() {
            self.selected = Some(key);
        }
        self.anchor = self.selected.clone();
        self.clear_selected_set();
    }

    /// 下移选中；无选中时选中第一个可选行；普通导航重置为单选态。
    pub fn select_down(&mut self) {
        if let Some(key) = self.next_selectable_key() {
            self.selected = Some(key);
        }
        self.anchor = self.selected.clone();
        self.clear_selected_set();
    }

    /// 折叠选中行；返回 true 表示行模型需要重建（展开的目录被折叠）。
    pub fn collapse_selection(&mut self) -> bool {
        let Some(idx) = self.selected_idx() else {
            return false;
        };
        let Some(row) = self.rows.get(idx) else {
            return false;
        };
        if row.is_dir() && row.expanded() {
            if let Some(key) = (self.key_of)(row) {
                self.collapse(&key);
                return true;
            }
            return false;
        }
        if row.depth() > 0 {
            // 已折叠/叶子：选中上移到上层祖先行。
            let parent_depth = row.depth() - 1;
            if let Some(parent_idx) = self.rows[..idx].iter().rposition(|r| {
                r.is_dir() && r.depth() == parent_depth && (self.key_of)(r).is_some()
            }) && let Some(key) = (self.key_of)(&self.rows[parent_idx])
            {
                self.selected = Some(key);
            }
            return false;
        }
        false
    }

    /// 展开选中行；返回 true 表示行模型需要重建（折叠的目录被展开）。
    pub fn expand_selection(&mut self) -> bool {
        let Some(idx) = self.selected_idx() else {
            return false;
        };
        let Some(row) = self.rows.get(idx) else {
            return false;
        };
        if row.is_dir() && !row.expanded() {
            if let Some(key) = (self.key_of)(row) {
                self.expand(key);
                return true;
            }
            return false;
        }
        self.select_down();
        false
    }

    /// 翻转展开标记（鼠标激活目录时用）。
    pub fn toggle_expand(&mut self, key: &K) {
        if self.expanded.contains(key) {
            self.collapse(key);
        } else {
            self.expand(key.clone());
        }
    }

    /// shift 扩展到目标行：以锚点为起点整体重算区间集合，游标移到目标行，锚点不动。
    ///
    /// 锚点缺失时取当前游标为锚（仍无则取目标行自身）；
    /// 区间为可见行序中两键之间（闭区间，两个方向均支持）。
    pub fn extend_to(&mut self, target: &K) {
        let anchor = self
            .anchor
            .clone()
            .or_else(|| self.selected.clone())
            .unwrap_or_else(|| target.clone());
        self.set_selected_set(self.range_keys(&anchor, target));
        self.anchor = Some(anchor);
        self.selected = Some(target.clone());
    }

    /// shift+上方向：游标按 select_up 语义上移一步并按锚点重算区间集合；返回游标是否移动。
    pub fn extend_up(&mut self) -> bool {
        let Some(target) = self.prev_selectable_key() else {
            return false;
        };
        self.extend_to(&target);
        true
    }

    /// shift+下方向：游标按 select_down 语义下移一步并按锚点重算区间集合；返回游标是否移动。
    pub fn extend_down(&mut self) -> bool {
        let Some(target) = self.next_selectable_key() else {
            return false;
        };
        self.extend_to(&target);
        true
    }

    /// cmd/ctrl+点击：切换目标行的多选标记（对称差），游标移到目标行，锚点不动。
    ///
    /// 纯单选态首次打标记时，会把普通点击选中的当前游标行一并加入集合。
    ///
    /// 否则当前游标会落在集合之外，后续从该行拖拽会收拢为单选，只移动一项。
    pub fn toggle_selection(&mut self, key: &K) {
        if self.selected_set.is_empty()
            && let Some(cursor) = self.selected.as_ref()
            && cursor != key
        {
            self.selected_set.insert(cursor.clone());
        }
        if !self.selected_set.remove(key) {
            self.selected_set.insert(key.clone());
        }
        self.selection_generation = self.selection_generation.wrapping_add(1);
        self.selected = Some(key.clone());
    }

    /// 有效选中集：集合非空时按可见行序返回集合元素，否则回退为游标单条（可为空）。
    pub fn effective_selection(&self) -> Vec<K> {
        if self.selected_set.is_empty() {
            return self.selected.clone().into_iter().collect();
        }
        self.rows
            .iter()
            .filter_map(|r| (self.key_of)(r))
            .filter(|key| self.selected_set.contains(key))
            .collect()
    }

    /// 行是否命中多选标记集。
    pub fn is_in_selection_set(&self, key: &K) -> bool {
        self.selected_set.contains(key)
    }

    /// 游标的上一个可选行键（无选中时为最后一个可选行，对应 select_up）。
    fn prev_selectable_key(&self) -> Option<K> {
        match self.selected_idx() {
            None => self
                .rows
                .iter()
                .rposition(|r| (self.key_of)(r).is_some())
                .and_then(|idx| (self.key_of)(&self.rows[idx])),
            Some(idx) => self.rows[..idx]
                .iter()
                .rposition(|r| (self.key_of)(r).is_some())
                .and_then(|prev| (self.key_of)(&self.rows[prev])),
        }
    }

    /// 游标的下一个可选行键（无选中时为第一个可选行，对应 select_down）。
    fn next_selectable_key(&self) -> Option<K> {
        match self.selected_idx() {
            None => self
                .rows
                .iter()
                .position(|r| (self.key_of)(r).is_some())
                .and_then(|idx| (self.key_of)(&self.rows[idx])),
            Some(idx) => self.rows[idx + 1..]
                .iter()
                .position(|r| (self.key_of)(r).is_some())
                .map(|offset| idx + 1 + offset)
                .and_then(|next| (self.key_of)(&self.rows[next])),
        }
    }

    /// 可见行序中 `from` 与 `to` 之间（闭区间）的可选行键集合；
    /// 任一端不在可见行中时为空集。
    fn range_keys(&self, from: &K, to: &K) -> HashSet<K> {
        let index_of = |key: &K| {
            self.rows
                .iter()
                .position(|r| (self.key_of)(r).as_ref() == Some(key))
        };
        let (Some(start), Some(end)) = (index_of(from), index_of(to)) else {
            return HashSet::new();
        };
        let (lo, hi) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        self.rows[lo..=hi]
            .iter()
            .filter_map(|r| (self.key_of)(r))
            .collect()
    }
}

#[cfg(test)]
#[path = "test/tree_state_tests.rs"]
mod tests;
