use super::{WrapEdit, WrapPatch};
use std::{ops::Range, sync::Arc};

use gpui::{AppContext as _, Entity, TestAppContext, font, px};
use zcv_language::{LanguageBuffer, LanguageRegistry};
use zcv_multi_buffer::{ExcerptRange, MultiBuffer};
use zcv_text::{Affinity, Buffer, BufferConfig, ByteOffset, Edit, TransactionMetadata};

use super::super::DisplayMap;
use super::{MultiBufferOffset, WrapMap};

fn reflow_fixture(cx: &mut TestAppContext) -> (Buffer, Entity<DisplayMap>, Entity<WrapMap>) {
    let text = (0..220)
        .map(|row| format!("第 {row} 行 abcdefghij 这是一段需要换行的文字\n"))
        .collect();
    let buffer = Buffer::from_text(text, BufferConfig::default()).unwrap();
    let display = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
    let wrap = cx.read_entity(&display, |map, _| map.wrap_map.clone());
    (buffer, display, wrap)
}

fn configure(cx: &mut TestAppContext, display: &Entity<DisplayMap>, width: Option<f32>) {
    cx.update_entity(display, |display, cx| {
        display.set_wrap_width(
            width.map(px),
            font("Helvetica"),
            px(16.),
            &cx.text_system().clone(),
            cx,
        );
    });
}

#[gpui::test]
fn async_rewrap_empty_composite_keeps_one_text_row(cx: &mut TestAppContext) {
    cx.background_executor.set_block_on_ticks(0..=0);
    let buffer = cx.new(MultiBuffer::empty);
    let (subscription, snapshot) =
        cx.update_entity(&buffer, |buffer, cx| buffer.subscribe_and_snapshot(cx));
    let display = cx.new(|cx| DisplayMap::new(snapshot, cx));
    cx.update_entity(&display, |map, cx| {
        map.set_multi_buffer(buffer, subscription, cx);
    });
    configure(cx, &display, Some(120.));
    cx.run_until_parked();
    let snapshot = cx.update_entity(&display, |map, cx| map.snapshot(cx));
    assert_eq!(snapshot.line_count(), 1);
    let point = snapshot
        .offset_to_display_point(MultiBufferOffset::ZERO)
        .unwrap();
    assert_eq!(
        snapshot.display_point_to_offset(point).unwrap(),
        MultiBufferOffset::ZERO
    );
}

#[gpui::test]
fn soft_wrap_equal_total_rows_still_invalidates_decoration_geometry(cx: &mut TestAppContext) {
    let mut buffer = Buffer::from_text(
        format!("{}\n{}\n", "a".repeat(40), "b".repeat(20)),
        BufferConfig::default(),
    )
    .unwrap();
    let changes = buffer.subscribe();
    let display = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
    configure(cx, &display, Some(120.));
    cx.run_until_parked();
    let before = display.update(cx, |map, cx| map.snapshot(cx));
    let before_second_row = before.line_to_display_row(zcv_text::Line::new(1)).unwrap();
    let cached = before.diff_decorations();
    buffer
        .edit(
            [
                Edit::delete(
                    zcv_text::TextRange::new(ByteOffset::ZERO, ByteOffset::new(20)).unwrap(),
                ),
                Edit::insert(ByteOffset::new(61), "b".repeat(20)).unwrap(),
            ],
            TransactionMetadata::default(),
        )
        .unwrap();
    display.update(cx, |map, cx| {
        map.sync(buffer.snapshot(), changes.consume(), cx)
    });
    cx.run_until_parked();
    let after = display.update(cx, |map, cx| map.snapshot(cx));
    assert_eq!(before.line_count(), after.line_count());
    assert_ne!(
        before_second_row,
        after.line_to_display_row(zcv_text::Line::new(1)).unwrap(),
        "行数不变但源行映射已变化"
    );
    assert!(
        !Arc::ptr_eq(&cached, &after.diff_decorations()),
        "源行映射改变必须失效装饰几何"
    );
}

#[gpui::test]
fn async_rewrap_startup_loads_multibyte_excerpts_before_empty_task_finishes(
    cx: &mut TestAppContext,
) {
    cx.background_executor.set_block_on_ticks(0..=0);
    let buffer = cx.new(MultiBuffer::empty);
    let (subscription, snapshot) =
        cx.update_entity(&buffer, |buffer, cx| buffer.subscribe_and_snapshot(cx));
    let display = cx.new(|cx| {
        let mut map = DisplayMap::new(snapshot, cx);
        map.set_multi_buffer(buffer.clone(), subscription, cx);
        map
    });
    configure(cx, &display, Some(120.));
    for file in 0..2 {
        let text = (0..130)
            .map(|row| format!("{file}:{row}\t{}\r\n", "中文🙂abc".repeat(20)))
            .collect();
        let source = cx.new(|cx| {
            LanguageBuffer::new(
                Buffer::from_text(text, BufferConfig::default()).unwrap(),
                None,
                Arc::new(LanguageRegistry::new()),
                cx,
            )
        });
        cx.update_entity(&buffer, |buffer, cx| {
            buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(source, 0..130, cx)], cx);
        });
        cx.update_entity(&display, |map, cx| map.snapshot(cx));
    }
    cx.run_until_parked();
    let snapshot = cx.update_entity(&display, |map, cx| map.snapshot(cx));
    assert!(snapshot.line_count() > 260);
    for row in [0, 1, 50, 130, 259] {
        let offset = snapshot
            .buffer_snapshot()
            .line_start_byte(zcv_text::Line::new(row))
            .unwrap();
        let point = snapshot.offset_to_display_point(offset).unwrap();
        assert_eq!(snapshot.display_point_to_offset(point).unwrap(), offset);
    }
}

#[gpui::test]
fn async_rewrap_preserves_edits_and_anchors_through_width_replacement(cx: &mut TestAppContext) {
    cx.background_executor.set_block_on_ticks(0..=0);
    let (mut buffer, display, wrap) = reflow_fixture(cx);
    let changes = buffer.subscribe();
    let anchor = buffer
        .snapshot()
        .anchor_with_affinity(ByteOffset::new(10), Affinity::After);
    configure(cx, &display, Some(120.));
    for text in ["前缀\n", "又一行\n"] {
        buffer
            .edit(
                [Edit::insert(ByteOffset::ZERO, text).unwrap()],
                TransactionMetadata::default(),
            )
            .unwrap();
        cx.update_entity(&display, |map, cx| {
            map.sync(buffer.snapshot(), changes.consume(), cx)
        });
    }
    let before = cx.read_entity(&wrap, |map, _| map.snapshot.version);
    let old_display = cx.update_entity(&display, |map, cx| map.snapshot(cx));
    let old_point = old_display
        .offset_to_display_point(MultiBufferOffset::new(
            anchor.resolve_in(&buffer.snapshot()).unwrap().get(),
        ))
        .unwrap();
    assert_eq!(
        old_display
            .display_point_to_offset(old_point)
            .unwrap()
            .get(),
        anchor.resolve_in(&buffer.snapshot()).unwrap().get()
    );
    configure(cx, &display, Some(210.));
    buffer
        .edit(
            [Edit::insert(ByteOffset::ZERO, "最后一行\n").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    cx.update_entity(&display, |map, cx| {
        map.sync(buffer.snapshot(), changes.consume(), cx)
    });
    cx.run_until_parked();
    let after = cx.update_entity(&display, |map, cx| map.snapshot(cx));
    cx.read_entity(&wrap, |map, _| {
        assert!(!map.snapshot.interpolated);
        assert!(map.background_task.is_none());
        assert!(map.pending_edits.is_empty());
        assert!(map.snapshot.version > before, "后台安装不能回退已发布版本");
    });
    let expected = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
    configure(cx, &expected, Some(210.));
    cx.run_until_parked();
    let expected = cx.update_entity(&expected, |map, cx| map.snapshot(cx));
    assert_eq!(after.line_count(), expected.line_count());
    for row in [0, 3, 100, 222] {
        let offset = after
            .buffer_snapshot()
            .line_start_byte(zcv_text::Line::new(row))
            .unwrap();
        assert_eq!(
            after.offset_to_display_point(offset).unwrap(),
            expected.offset_to_display_point(offset).unwrap()
        );
    }
    let offset = MultiBufferOffset::new(anchor.resolve_in(&buffer.snapshot()).unwrap().get());
    let point = after.offset_to_display_point(offset).unwrap();
    assert_eq!(after.display_point_to_offset(point).unwrap(), offset);
}

#[gpui::test]
fn async_rewrap_empty_frames_keep_one_pending_snapshot(cx: &mut TestAppContext) {
    cx.background_executor.set_block_on_ticks(0..=0);
    let (_, display, wrap) = reflow_fixture(cx);
    configure(cx, &display, Some(120.));
    for _ in 0..1_000 {
        cx.update_entity(&display, |map, cx| map.snapshot(cx));
    }
    cx.read_entity(&wrap, |map, _| {
        assert!(map.background_task.is_some());
        assert_eq!(map.pending_edits.len(), 1, "空帧不能重复保留同版本快照");
    });
    cx.run_until_parked();
    let settled = cx.update_entity(&display, |map, cx| map.snapshot(cx));
    assert!(settled.line_count() > 220);
    cx.read_entity(&wrap, |map, _| assert!(map.pending_edits.is_empty()));
}

#[gpui::test]
fn async_rewrap_cancellation_and_close_release_measurement_resources(cx: &mut TestAppContext) {
    cx.background_executor.set_block_on_ticks(0..=0);
    let text_system = cx.update(|app| app.text_system().clone());
    let (_, display, wrap) = reflow_fixture(cx);
    for width in [120., 180., 90., 210.] {
        configure(cx, &display, Some(width));
    }
    configure(cx, &display, None);
    cx.run_until_parked();
    let snapshot = cx.update_entity(&display, |map, cx| map.snapshot(cx));
    assert_eq!(
        snapshot.line_count(),
        snapshot.buffer_snapshot().line_count()
    );
    cx.read_entity(&wrap, |map, _| {
        assert!(!map.snapshot.interpolated);
        assert!(map.background_task.is_none());
        assert!(map.pending_edits.is_empty());
    });
    // GPUI 按字体复用 LineWrapper；取消任务会把测量器归还池，保留字符宽度缓存。
    let pooled_baseline = Arc::strong_count(&text_system);
    for width in [120., 180., 90., 210.] {
        configure(cx, &display, Some(width));
    }
    configure(cx, &display, None);
    cx.run_until_parked();
    assert_eq!(
        Arc::strong_count(&text_system),
        pooled_baseline,
        "重复取消后测量器池必须稳定"
    );
    configure(cx, &display, Some(120.));
    cx.run_until_parked();
    let mut worker = cx.read_entity(&wrap, |map, _| map.worker());
    let tab = worker.snapshot.tab_snapshot.clone();
    let range = super::TabPoint::zero()..tab.max_point();
    let mut update = Box::pin(async move {
        worker
            .apply_edits(
                tab,
                &[super::TabEdit {
                    old: range.clone(),
                    new: range,
                }],
            )
            .await
    });
    let mut task_cx = std::task::Context::from_waker(std::task::Waker::noop());
    assert!(std::future::Future::poll(update.as_mut(), &mut task_cx).is_pending());
    drop(update);
    assert_eq!(
        Arc::strong_count(&text_system),
        pooled_baseline,
        "取消后测量器必须归还 GPUI 池，不保留额外任务句柄"
    );
    configure(cx, &display, Some(90.));
    let weak = wrap.downgrade();
    drop(wrap);
    drop(display);
    // App 的事件周期回收已释放的实体，随后执行器销毁取消的任务。
    cx.update(|_| {});
    cx.run_until_parked();
    assert!(weak.upgrade().is_none());
    assert_eq!(
        Arc::strong_count(&text_system),
        pooled_baseline - 1,
        "关闭视图后必须释放换行配置，测量器由 GPUI 按字体复用"
    );
}

#[gpui::test]
fn async_rewrap_config_change_preserves_unconsumed_net_patch(cx: &mut TestAppContext) {
    cx.background_executor.set_block_on_ticks(0..=0);
    let (_, display, wrap) = reflow_fixture(cx);
    let original = cx.read_entity(&wrap, |map, _| map.snapshot.line_count());
    configure(cx, &display, Some(120.));
    cx.run_until_parked();
    // 后台净编辑尚未被 DisplayMap 消费，配置入口必须继续组合，不能清空它。
    cx.update_entity(&wrap, |map, cx| {
        map.set_wrap_width(
            Some(px(210.)),
            font("Helvetica"),
            px(16.),
            cx.text_system().clone(),
            cx,
        );
    });
    cx.run_until_parked();
    cx.update_entity(&wrap, |map, cx| {
        let (snapshot, edits) = map.sync(map.snapshot.tab_snapshot.clone(), &[], cx);
        assert_eq!(edits, vec![edit(0..original, 0..snapshot.line_count())]);
    });
}

fn edit(old: Range<usize>, new: Range<usize>) -> WrapEdit {
    WrapEdit { old, new }
}

fn compose(old: Vec<WrapEdit>, next: Vec<WrapEdit>) -> Vec<WrapEdit> {
    WrapPatch { edits: old }.compose(next).into_inner()
}

#[test]
fn compose_disjoint_before() {
    assert_eq!(
        compose(vec![edit(1..3, 1..4)], vec![edit(0..0, 0..4)]),
        vec![edit(0..0, 0..4), edit(1..3, 5..8)],
    );
}

#[test]
fn compose_disjoint_after() {
    assert_eq!(
        compose(vec![edit(1..3, 1..4)], vec![edit(5..9, 5..7)]),
        vec![edit(1..3, 1..4), edit(4..8, 5..7)],
    );
}

#[test]
fn compose_overlapping() {
    assert_eq!(
        compose(vec![edit(1..3, 1..4)], vec![edit(3..5, 3..6)]),
        vec![edit(1..4, 1..6)],
    );
}

#[test]
fn compose_two_disjoint_and_overlapping() {
    assert_eq!(
        compose(
            vec![edit(1..3, 1..4), edit(8..12, 9..11)],
            vec![edit(0..0, 0..4), edit(3..10, 7..9)],
        ),
        vec![edit(0..0, 0..4), edit(1..12, 5..10)],
    );
}
