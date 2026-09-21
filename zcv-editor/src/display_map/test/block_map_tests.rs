//! 块投影层的白盒测试：验证结构变化只重建受影响区间，未受影响的变换子树按 Arc 复用。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{AppContext as _, TestAppContext};
use zcv_language::LanguageBuffer;
use zcv_multi_buffer::{ExcerptRange, MultiBuffer};
use zcv_text::{Buffer, BufferConfig};

use super::*;
use crate::display_map::DisplayMap;

fn language_buffer(
    path: &str,
    text: &str,
    cx: &mut TestAppContext,
) -> gpui::Entity<LanguageBuffer> {
    cx.new(|cx| {
        LanguageBuffer::new(
            Buffer::from_text(text.to_owned(), BufferConfig::default())
                .expect("测试 Buffer 应能创建"),
            Some(PathBuf::from(path)),
            Arc::new(zcv_language::LanguageRegistry::new()),
            cx,
        )
    })
}

/// 按显示顺序收集每个块变换携带的身份。
///
/// 身份是变换子树共享的 `Arc`，因此可用于判断两个快照是否复用了同一变换。
fn block_placements(snapshot: &BlockSnapshot) -> Vec<Arc<BlockPlacement>> {
    let mut cursor = snapshot.transforms.cursor::<InputToOutput>(());
    cursor.seek(&InputRows(0), Bias::Left);
    let mut placements = Vec::new();
    while let Some(transform) = cursor.item() {
        if let TransformKind::Block(placement) = &transform.kind {
            placements.push(placement.clone());
        }
        cursor.next();
    }
    placements
}

#[gpui::test]
fn folding_a_middle_buffer_reuses_surrounding_block_transforms(cx: &mut TestAppContext) {
    let first = language_buffer("src/a.rs", "a0\na1\n", cx);
    let middle = language_buffer("src/b.rs", "b0\nb1\n", cx);
    let last = language_buffer("src/c.rs", "c0\nc1\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(first, 0..1, cx)], cx);
        buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(middle.clone(), 0..1, cx)], cx);
        buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(last, 0..1, cx)], cx);
    });
    let (_, snapshot) = cx.update_entity(&combined, |buffer, cx| buffer.subscribe_and_snapshot(cx));
    let display = cx.new(|cx| DisplayMap::new(snapshot, cx));
    let display_snapshot = cx.update_entity(&display, |map, cx| map.snapshot(cx));
    let wrap_snapshot = display_snapshot.wrap_snapshot().clone();
    let excerpts = display_snapshot.buffer_snapshot().excerpts_arc();
    let middle_id = cx.update_entity(&middle, |buffer, _cx| buffer.buffer_id());

    let unfolded = BlockSnapshot::new(wrap_snapshot.clone(), excerpts.clone(), &HashSet::new());
    let mut folded_buffers = HashSet::new();
    folded_buffers.insert(middle_id);
    let folded = unfolded.sync(wrap_snapshot.clone(), excerpts, &folded_buffers, &[]);

    assert_eq!(
        folded.transforms.summary().input_rows,
        wrap_snapshot.line_count(),
        "块投影变换的输入行必须精确覆盖换行投影"
    );

    let before = block_placements(&unfolded);
    let after = block_placements(&folded);
    assert_eq!(before.len(), 3, "三个文件各有一个 header");
    assert_eq!(after.len(), 3, "折叠中间文件后仍保留三个块");
    assert!(
        Arc::ptr_eq(&before[0], &after[0]),
        "折叠区间之前的块变换必须复用旧子树"
    );
    assert!(
        !Arc::ptr_eq(&before[1], &after[1]),
        "被折叠文件的块必须重建"
    );
    assert!(
        Arc::ptr_eq(&before[2], &after[2]),
        "折叠区间之后的块变换必须复用旧子树"
    );
    assert!(
        folded.line_count() < unfolded.line_count(),
        "整文件折叠必须隐藏被折叠文件的文本行"
    );
}

/// 回归：被折叠文件含多个 excerpt 时新旧块数量不同，后缀必须按旧规格下标定位，
/// 否则会与重建区间重叠，使变换输入行多算。
#[gpui::test]
fn folding_a_buffer_with_multiple_excerpts_keeps_input_coverage(cx: &mut TestAppContext) {
    let first = language_buffer("src/a.rs", "a0\na1\n", cx);
    let middle = language_buffer("src/b.rs", "b0\nb1\nb2\n", cx);
    let last = language_buffer("src/c.rs", "c0\nc1\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(first, 0..1, cx)], cx);
        buffer.set_excerpts_for_path(
            vec![
                ExcerptRange::line_range(middle.clone(), 0..1, cx),
                ExcerptRange::line_range(middle.clone(), 1..2, cx),
            ],
            cx,
        );
        buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(last, 0..1, cx)], cx);
    });
    let (_, snapshot) = cx.update_entity(&combined, |buffer, cx| buffer.subscribe_and_snapshot(cx));
    let display = cx.new(|cx| DisplayMap::new(snapshot, cx));
    let display_snapshot = cx.update_entity(&display, |map, cx| map.snapshot(cx));
    let wrap_snapshot = display_snapshot.wrap_snapshot().clone();
    let excerpts = display_snapshot.buffer_snapshot().excerpts_arc();
    let middle_id = cx.update_entity(&middle, |buffer, _cx| buffer.buffer_id());

    let unfolded = BlockSnapshot::new(wrap_snapshot.clone(), excerpts.clone(), &HashSet::new());
    let before = block_placements(&unfolded);
    assert_eq!(
        before.len(),
        4,
        "A header + B header + B divider + C header"
    );

    let mut folded_buffers = HashSet::new();
    folded_buffers.insert(middle_id);
    let folded = unfolded.sync(wrap_snapshot.clone(), excerpts, &folded_buffers, &[]);

    assert_eq!(
        folded.transforms.summary().input_rows,
        wrap_snapshot.line_count(),
        "折叠含多个 excerpt 的文件后输入行仍必须精确覆盖换行投影"
    );
    let after = block_placements(&folded);
    assert_eq!(after.len(), 3, "折叠后 B 合并为一个整文件折叠块");
    assert!(
        Arc::ptr_eq(
            before.last().expect("旧布局应有 C header"),
            after.last().expect("新布局应保留 C header")
        ),
        "折叠区间之后的 C header 必须复用旧变换"
    );
}

#[gpui::test]
fn out_of_range_wrap_row_fails_explicitly(cx: &mut TestAppContext) {
    let buffer = language_buffer("src/a.rs", "a0\na1\n", cx);
    let combined = cx.new(|cx| MultiBuffer::singleton(buffer, cx));
    let (_, snapshot) = cx.update_entity(&combined, |buffer, cx| buffer.subscribe_and_snapshot(cx));
    let display = cx.new(|cx| DisplayMap::new(snapshot, cx));
    let display_snapshot = cx.update_entity(&display, |map, cx| map.snapshot(cx));
    let wrap_snapshot = display_snapshot.wrap_snapshot().clone();
    let excerpts = display_snapshot.buffer_snapshot().excerpts_arc();
    let block = BlockSnapshot::new(wrap_snapshot.clone(), excerpts, &HashSet::new());

    let out_of_range = wrap_snapshot.line_count();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        block.projected_wrap_row_to_display_row(out_of_range)
    }));
    assert!(
        result.is_err(),
        "越界换行行必须显式失败，而不是静默夹取到末行"
    );
}
