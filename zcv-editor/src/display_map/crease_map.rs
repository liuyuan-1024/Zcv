//! 编辑器消费的折叠候选（crease）索引。
//!
//! 对齐 Zed 的 `CreaseMap`：折叠候选以「稳定组合锚点 + 稳定身份」表示，由 `SumTree` 承载并按锚点游标查询。
//! 视口渲染只按可见行范围 seek 到相关候选，不再遍历整份候选集合；
//！锚点随文本编辑自动推进，无需每帧重新解析全部候选。
//!
//! 当前折叠候选只来自语言层的语法折叠范围；
//! 显式注入（宿主 crease）尚未接入，接入时按 zed 的 `insert` / `remove` 身份协议扩展即可。

use std::cmp::Ordering;
use std::collections::HashMap;
use std::ops::Range;

use sum_tree::{Bias, Item, SeekTarget, SumTree};
use zcv_multi_buffer::{MultiBufferAnchor, MultiBufferOffset, MultiBufferSnapshot};
use zcv_text::{Affinity, Line};

/// 折叠候选在 `CreaseMap` 中的稳定身份。
#[derive(Copy, Clone, Default, Debug, Eq, PartialEq, Hash)]
pub(crate) struct CreaseId(usize);

/// 一个折叠候选：组合文档中的稳定锚点范围。
#[derive(Clone, Debug)]
pub(crate) struct Crease {
    range: Range<MultiBufferAnchor>,
}

impl Crease {
    pub(crate) fn simple(range: Range<MultiBufferAnchor>) -> Self {
        Self { range }
    }

    pub(crate) fn range(&self) -> &Range<MultiBufferAnchor> {
        &self.range
    }
}

#[derive(Clone, Debug)]
struct CreaseItem {
    id: CreaseId,
    crease: Crease,
}

/// 可写的折叠候选集合；`snapshot` 是渲染只读的派生视图。
#[derive(Debug)]
pub(crate) struct CreaseMap {
    snapshot: CreaseSnapshot,
    next_id: CreaseId,
    id_to_range: HashMap<CreaseId, Range<MultiBufferAnchor>>,
}

impl CreaseMap {
    pub(crate) fn new(snapshot: &MultiBufferSnapshot) -> Self {
        Self {
            snapshot: CreaseSnapshot::new(snapshot),
            next_id: CreaseId::default(),
            id_to_range: HashMap::new(),
        }
    }

    pub(crate) fn snapshot(&self) -> CreaseSnapshot {
        self.snapshot.clone()
    }

    pub(crate) fn insert(
        &mut self,
        creases: impl IntoIterator<Item = Crease>,
        snapshot: &MultiBufferSnapshot,
    ) -> Vec<CreaseId> {
        let mut new_ids = Vec::new();
        self.snapshot.creases = {
            let mut new_creases = SumTree::new(snapshot);
            let mut cursor = self.snapshot.creases.cursor::<ItemSummary>(snapshot);
            for crease in creases {
                let crease_range = crease.range().clone();
                new_creases.append(cursor.slice(&crease_range, Bias::Left), snapshot);

                let id = self.next_id;
                self.next_id.0 += 1;
                self.id_to_range.insert(id, crease_range);
                new_creases.push(CreaseItem { crease, id }, snapshot);
                new_ids.push(id);
            }
            new_creases.append(cursor.suffix(), snapshot);
            new_creases
        };
        new_ids
    }

    pub(crate) fn remove(
        &mut self,
        ids: impl IntoIterator<Item = CreaseId>,
        snapshot: &MultiBufferSnapshot,
    ) {
        let mut ids_to_remove = ids.into_iter().collect::<Vec<_>>();
        ids_to_remove.retain(|id| self.id_to_range.remove(id).is_some());
        if ids_to_remove.is_empty() {
            return;
        }
        let ids_to_remove = ids_to_remove
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        self.snapshot.creases = {
            let mut new_creases = SumTree::new(snapshot);
            for item in self.snapshot.creases.iter() {
                if !ids_to_remove.contains(&item.id) {
                    new_creases.push(item.clone(), snapshot);
                }
            }
            new_creases
        };
    }
}

/// 渲染只读的折叠候选视图。
#[derive(Clone)]
pub(crate) struct CreaseSnapshot {
    creases: SumTree<CreaseItem>,
}

impl std::fmt::Debug for CreaseSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreaseSnapshot").finish_non_exhaustive()
    }
}

impl CreaseSnapshot {
    fn new(snapshot: &MultiBufferSnapshot) -> Self {
        Self {
            creases: SumTree::new(snapshot),
        }
    }

    /// 遍历全部候选；只用于「包含某行」这类按需查询，不在渲染热路径使用。
    pub(crate) fn creases(&self) -> impl Iterator<Item = &Crease> {
        self.creases.iter().map(|item| &item.crease)
    }

    /// 起点落在给定组合行范围内的候选；按锚点 seek，避免遍历整份候选集合。
    pub(crate) fn creases_in_range<'a>(
        &'a self,
        range: Range<Line>,
        snapshot: &'a MultiBufferSnapshot,
    ) -> impl 'a + Iterator<Item = &'a Crease> {
        let start = line_start_anchor(snapshot, range.start);
        let mut cursor = self.creases.cursor::<ItemSummary>(snapshot);
        cursor.seek(&start, Bias::Left);
        std::iter::from_fn(move || {
            while let Some(item) = cursor.item() {
                cursor.next();
                let Some(offset) = snapshot.resolve_anchor(&item.crease.range.start) else {
                    continue;
                };
                let Ok(line) = snapshot.byte_to_line(offset) else {
                    continue;
                };
                if line < range.start {
                    continue;
                }
                if line >= range.end {
                    return None;
                }
                return Some(&item.crease);
            }
            None
        })
    }

    /// 起点恰好在给定行上的候选。
    pub(crate) fn crease_at_line<'a>(
        &'a self,
        line: Line,
        snapshot: &'a MultiBufferSnapshot,
    ) -> Option<&'a Crease> {
        self.creases_in_range(line..Line::new(line.get() + 1), snapshot)
            .next()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ItemSummary {
    range: Range<MultiBufferAnchor>,
}

impl sum_tree::Summary for ItemSummary {
    type Context<'a> = &'a MultiBufferSnapshot;

    fn zero<'a>(_cx: Self::Context<'a>) -> Self {
        Self {
            range: MultiBufferAnchor::Min..MultiBufferAnchor::Min,
        }
    }

    fn add_summary<'a>(&mut self, other: &Self, _cx: Self::Context<'a>) {
        self.range = other.range.clone();
    }
}

impl Item for CreaseItem {
    type Summary = ItemSummary;

    fn summary(&self, _cx: &MultiBufferSnapshot) -> Self::Summary {
        ItemSummary {
            range: self.crease.range().clone(),
        }
    }
}

/// 按组合锚点定位候选：解析到当前快照的组合偏移后比较。
impl<'a> SeekTarget<'a, ItemSummary, ItemSummary> for MultiBufferAnchor {
    fn cmp(&self, cursor_location: &ItemSummary, snapshot: &MultiBufferSnapshot) -> Ordering {
        anchor_cmp(self, &cursor_location.range.start, snapshot)
    }
}

impl<'a> SeekTarget<'a, ItemSummary, ItemSummary> for Range<MultiBufferAnchor> {
    fn cmp(&self, cursor_location: &ItemSummary, snapshot: &MultiBufferSnapshot) -> Ordering {
        anchor_cmp(&self.start, &cursor_location.range.start, snapshot)
            .then_with(|| anchor_cmp(&self.end, &cursor_location.range.end, snapshot))
    }
}

fn anchor_cmp(
    left: &MultiBufferAnchor,
    right: &MultiBufferAnchor,
    snapshot: &MultiBufferSnapshot,
) -> Ordering {
    match (
        snapshot.resolve_anchor(left),
        snapshot.resolve_anchor(right),
    ) {
        (Some(left), Some(right)) => Ord::cmp(&left.get(), &right.get()),
        // 无法解析的锚点不参与定位：把它们排在两端之外，游标查询会再按行过滤。
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
    }
}

fn line_start_anchor(snapshot: &MultiBufferSnapshot, line: Line) -> MultiBufferAnchor {
    match snapshot.line_start_byte(line) {
        Ok(offset) => snapshot.anchor_at(MultiBufferOffset::from(offset), Affinity::Before),
        Err(_) => MultiBufferAnchor::Max,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zcv_text::{Buffer, BufferConfig};

    fn snapshot_of(text: &str) -> MultiBufferSnapshot {
        let buffer = Buffer::from_text(text.to_owned(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        MultiBufferSnapshot::from(buffer.snapshot())
    }

    fn line_anchor(snapshot: &MultiBufferSnapshot, line: usize, end: bool) -> MultiBufferAnchor {
        let start = snapshot
            .line_start_byte(Line::new(line))
            .expect("测试行应存在");
        let offset = if end {
            snapshot
                .line_start_byte(Line::new(line + 1))
                .unwrap_or_else(|_| snapshot.len_bytes())
        } else {
            start
        };
        snapshot.anchor_at(
            offset,
            if end {
                Affinity::After
            } else {
                Affinity::Before
            },
        )
    }

    #[test]
    fn creases_query_only_the_requested_line_range() {
        let snapshot = snapshot_of("aa\nbb\ncc\ndd\n");
        let mut map = CreaseMap::new(&snapshot);
        map.insert(
            [
                Crease::simple(line_anchor(&snapshot, 1, false)..line_anchor(&snapshot, 1, true)),
                Crease::simple(line_anchor(&snapshot, 3, false)..line_anchor(&snapshot, 3, true)),
            ],
            &snapshot,
        );
        let creases = map.snapshot();
        assert_eq!(creases.creases().count(), 2);
        assert_eq!(
            creases
                .creases_in_range(Line::new(0)..Line::new(2), &snapshot)
                .count(),
            1
        );
        assert_eq!(
            creases
                .creases_in_range(Line::new(2)..Line::new(4), &snapshot)
                .count(),
            1
        );
        assert!(creases.crease_at_line(Line::new(1), &snapshot).is_some());
        assert!(creases.crease_at_line(Line::new(0), &snapshot).is_none());
    }

    #[test]
    fn removing_creases_drops_them_from_queries() {
        let snapshot = snapshot_of("aa\nbb\ncc\n");
        let mut map = CreaseMap::new(&snapshot);
        let first = line_anchor(&snapshot, 0, false)..line_anchor(&snapshot, 1, true);
        let second = line_anchor(&snapshot, 1, false)..line_anchor(&snapshot, 2, true);
        let ids = map.insert([Crease::simple(first), Crease::simple(second)], &snapshot);
        assert_eq!(map.snapshot().creases().count(), 2);

        map.remove([ids[0]], &snapshot);
        assert_eq!(map.snapshot().creases().count(), 1);
        assert!(
            map.snapshot()
                .crease_at_line(Line::new(0), &snapshot)
                .is_none()
        );
        assert!(
            map.snapshot()
                .crease_at_line(Line::new(1), &snapshot)
                .is_some()
        );
    }
}
