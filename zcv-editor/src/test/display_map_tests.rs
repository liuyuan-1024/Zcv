use std::{
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::Arc,
};

use gpui::{AppContext, TestAppContext, font, px};
use zcv_buffer_diff::{BufferDiff, BufferDiffInput};
use zcv_language::{LanguageBuffer, LanguageRegistry};
use zcv_multi_buffer::{DiffFile, ExcerptRange, MultiBuffer, MultiBufferOffset};
use zcv_text::{Affinity, Buffer, BufferConfig, Edit, Line, TransactionMetadata};
use zcv_theme::ThemeChoice;

use super::test_support::{WrapRowKind, projected_line_text};
use super::*;

fn display_snapshot(cx: &mut TestAppContext, map: &Entity<DisplayMap>) -> DisplaySnapshot {
    cx.update_entity(map, |map, cx| map.snapshot(cx))
}

fn sync(
    cx: &mut TestAppContext,
    map: &Entity<DisplayMap>,
    snapshot: impl Into<MultiBufferSnapshot>,
    batch: TextChangeBatch,
) {
    cx.update_entity(map, |map, cx| map.sync(snapshot, batch, cx));
}

fn fold_range(
    cx: &mut TestAppContext,
    map: &Entity<DisplayMap>,
    start: usize,
    end: usize,
) -> DisplayMapResult<()> {
    cx.update_entity(map, |map, cx| {
        let range = {
            let display = map.snapshot(cx);
            let snapshot = display.buffer_snapshot();
            snapshot.anchor_at(MultiBufferOffset::new(start), Affinity::Before)
                ..snapshot.anchor_at(MultiBufferOffset::new(end), Affinity::After)
        };
        map.fold_range(range, FoldPlaceholder::default(), cx)
    })
}

fn set_tab_width(cx: &mut TestAppContext, map: &Entity<DisplayMap>, tab_width: NonZeroUsize) {
    cx.update_entity(map, |map, cx| map.set_tab_width(tab_width, cx));
}

fn set_wrap_width(
    cx: &mut TestAppContext,
    map: &Entity<DisplayMap>,
    wrap_width: Option<gpui::Pixels>,
    font: gpui::Font,
    font_size: gpui::Pixels,
) {
    let text_system = cx.text_system().clone();
    cx.update_entity(map, |map, cx| {
        map.set_wrap_width(wrap_width, font, font_size, &text_system, cx)
    });
}

fn apply_test_theme(cx: &mut TestAppContext, id: &'static str) {
    cx.update(|cx| ThemeChoice::Named(id).apply(cx, None));
}

#[gpui::test]
fn display_snapshot_resolves_syntax_styles_from_current_theme(cx: &mut TestAppContext) {
    apply_test_theme(cx, "light");
    let source_buffer = Buffer::from_text("fn main() {}".to_owned(), BufferConfig::default())
        .expect("测试 Buffer 应能创建");
    let source = cx.new(|cx| {
        LanguageBuffer::new(
            source_buffer,
            Some(PathBuf::from("main.rs")),
            std::sync::Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    cx.run_until_parked();
    let multi_buffer = cx.new(|cx| MultiBuffer::singleton(source, cx));
    let (_, snapshot) =
        cx.update_entity(&multi_buffer, |multi, cx| multi.subscribe_and_snapshot(cx));
    let map = cx.new(|cx| DisplayMap::new(snapshot, cx));

    let snapshot = display_snapshot(cx, &map);
    let styles = cx.update(|app| snapshot.highlight_styles(app));
    assert!(!styles.is_empty(), "语法解析应提供 capture 表");
    let light = styles[0].color;
    apply_test_theme(cx, "dark");
    let dark = cx.update(|app| snapshot.highlight_styles(app))[0].color;

    assert_ne!(light, dark, "同一 DisplayMap 应按当前主题重新派生语法颜色");
}

#[gpui::test]
fn no_op_sync_keeps_the_display_projection_stable(cx: &mut TestAppContext) {
    let buffer = Buffer::from_text("paragraph".to_string(), BufferConfig::default())
        .expect("测试 Buffer 应能创建");
    let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
    let before = display_snapshot(cx, &map);
    let current = MultiBufferSnapshot::from(buffer.snapshot());

    sync(cx, &map, current, TextChangeBatch::default());

    // 统一读取入口总是逐层同步，但无输入变化的同步不得改变可观察的显示投影。
    let after = display_snapshot(cx, &map);
    assert_eq!(after.line_count(), before.line_count());
    assert_eq!(
        after.buffer_snapshot().version(),
        before.buffer_snapshot().version()
    );
}

#[gpui::test]
fn display_pipeline_receives_the_source_transaction_batch(cx: &mut TestAppContext) {
    let source_buffer = Buffer::from_text("fn main() {}\n".to_owned(), BufferConfig::default())
        .expect("测试 Buffer 应能创建");
    let source = cx.new(|cx| {
        LanguageBuffer::new(
            source_buffer,
            Some(PathBuf::from("main.rs")),
            std::sync::Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    cx.run_until_parked();

    let multi_buffer = cx.new(|cx| MultiBuffer::singleton(source.clone(), cx));
    let (projection_subscription, snapshot) =
        cx.update_entity(&multi_buffer, |multi, cx| multi.subscribe_and_snapshot(cx));
    let display = cx.new(|cx| DisplayMap::new(snapshot, cx));
    display.update(cx, |display, cx| {
        display.set_multi_buffer(multi_buffer.clone(), projection_subscription, cx);
    });

    cx.update_entity(&source, |source, cx| {
        source
            .edit(
                [Edit::insert(MultiBufferOffset::new(3).into(), "async ").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("测试编辑应成功");
    });
    cx.run_until_parked();

    let display_text = |cx: &mut TestAppContext, display: &Entity<DisplayMap>| {
        String::from_utf8(display_snapshot(cx, display).buffer_snapshot().text_bytes())
            .expect("显示快照必须是 UTF-8")
    };
    assert_eq!(display_text(cx, &display), "fn async main() {}\n");

    cx.update_entity(&source, |source, cx| {
        source
            .replace_text("fn replacement() {}\n".to_owned(), cx)
            .expect("外部重载应成功");
    });
    cx.run_until_parked();

    assert_eq!(display_text(cx, &display), "fn replacement() {}\n");
}

/// 回归：块分类不能只缓存逻辑边界的下标。
///
/// diff 文件集合更新时，某个位置可从文件 B 的首片段变成文件 A 的后续片段，也可反向变化；两种情况下边界下标都不变。
/// Zed 按当前相邻 excerpt 的 BufferId分类，Zcv 必须据当前路径重建 header/divider，不能把旧实体块留在新拓扑中。
#[gpui::test]
fn block_boundaries_reclassify_when_their_file_changes(cx: &mut TestAppContext) {
    let first = cx.new(|cx| {
        LanguageBuffer::new(
            Buffer::from_text("a0\na1\n".to_owned(), BufferConfig::default())
                .expect("测试 Buffer 应能创建"),
            Some(PathBuf::from("src/a.rs")),
            std::sync::Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    let second = cx.new(|cx| {
        LanguageBuffer::new(
            Buffer::from_text("b0\n".to_owned(), BufferConfig::default())
                .expect("测试 Buffer 应能创建"),
            Some(PathBuf::from("src/b.rs")),
            std::sync::Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(first.clone(), 0..1, cx)], cx);
        buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(second.clone(), 0..1, cx)], cx);
    });
    let (subscription, snapshot) =
        cx.update_entity(&combined, |buffer, cx| buffer.subscribe_and_snapshot(cx));
    let display = cx.new(|cx| DisplayMap::new(snapshot, cx));
    cx.update_entity(&display, |map, cx| {
        map.set_multi_buffer(combined.clone(), subscription, cx);
    });
    display_snapshot(cx, &display);

    let block_kinds = |cx: &mut TestAppContext| {
        let snapshot = display_snapshot(cx, &display);
        let mut rows = snapshot.rows(DisplayRow::ZERO, snapshot.line_count());
        std::iter::from_fn(|| rows.next())
            .filter_map(|row| {
                row.block().map(|block| {
                    (
                        block.kind,
                        block.excerpt.path().to_path_buf(),
                        block.excerpt.source_start_line(),
                    )
                })
            })
            .collect::<Vec<_>>()
    };

    assert_eq!(
        block_kinds(cx),
        vec![
            (DisplayBlockKind::BufferHeader, PathBuf::from("src/a.rs"), 1),
            (DisplayBlockKind::BufferHeader, PathBuf::from("src/b.rs"), 1),
        ]
    );

    // 两个逻辑边界仍是下标 0、1，但第二个从 B 的首片段变成 A 的后续片段。
    cx.update_entity(&combined, |buffer, cx| {
        assert!(buffer.remove_excerpts_for_path(Path::new("src/b.rs"), cx));
        buffer.set_excerpts_for_path(
            vec![
                ExcerptRange::line_range(first.clone(), 0..1, cx),
                ExcerptRange::line_range(first.clone(), 1..2, cx),
            ],
            cx,
        );
    });
    assert_eq!(
        block_kinds(cx),
        vec![
            (DisplayBlockKind::BufferHeader, PathBuf::from("src/a.rs"), 1),
            (
                DisplayBlockKind::ExcerptBoundary,
                PathBuf::from("src/a.rs"),
                2
            ),
        ],
        "同文件后续逻辑 excerpt 必须是 divider，不能保留已移除文件的 header"
    );

    // 反向切回 B：同一边界现在必须重新成为 B 的实体 header。
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(first, 0..1, cx)], cx);
        buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(second, 0..1, cx)], cx);
    });
    assert_eq!(
        block_kinds(cx),
        vec![
            (DisplayBlockKind::BufferHeader, PathBuf::from("src/a.rs"), 1),
            (DisplayBlockKind::BufferHeader, PathBuf::from("src/b.rs"), 1),
        ],
        "新文件首片段必须恢复为实体 header，不能遗留同文件 divider"
    );
}

/// 匿名 Buffer 没有文件路径时仍是不同的组合实体。
///
/// Zed 用 BufferId（其本地 Buffer 的 `remote_id`）区分这类来源；
/// Zcv 用本地 `buffer_id` 承担同一职责，不能把它们归并到空路径并吞掉后一个 header。
/// 折叠占位符在 chunk 流中携带行内替换描述；普通文本 chunk 不携带。
#[gpui::test]
fn fold_placeholder_chunk_carries_inline_renderer(cx: &mut TestAppContext) {
    let buffer = Buffer::from_text(
        "anchor\nhidden one\nafter".to_owned(),
        BufferConfig::default(),
    )
    .expect("测试 Buffer 应能创建");
    let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
    fold_range(cx, &map, 6, 17).expect("折叠应成功");

    let display = display_snapshot(cx, &map);
    let mut placeholder_seen = false;
    let mut plain_chunk_seen = false;
    display
        .chunks(
            DisplayRow::ZERO..DisplayRow::new(display.line_count()),
            HighlightStyles::default(),
            None,
        )
        .for_each_row(|event| {
            let DisplayRowEvent::Text { chunks, .. } = event else {
                return;
            };
            for chunk in chunks {
                if chunk.is_placeholder {
                    placeholder_seen = true;
                    assert!(
                        chunk.renderer.is_some(),
                        "占位符 chunk 必须携带行内替换描述"
                    );
                } else if chunk.renderer.is_none() {
                    plain_chunk_seen = true;
                }
            }
        });
    assert!(placeholder_seen, "折叠行必须产生占位符 chunk");
    assert!(plain_chunk_seen, "折叠行仍须有普通文本 chunk");
}

#[gpui::test]
fn anonymous_buffers_keep_distinct_header_identities(cx: &mut TestAppContext) {
    let anonymous = |text: &str, cx: &mut TestAppContext| {
        cx.new(|cx| {
            LanguageBuffer::new(
                Buffer::from_text(text.to_owned(), BufferConfig::default())
                    .expect("测试 Buffer 应能创建"),
                None,
                std::sync::Arc::new(LanguageRegistry::new()),
                cx,
            )
        })
    };
    let first = anonymous("first\n", cx);
    let second = anonymous("second\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(first, 0..1, cx)], cx);
        buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(second, 0..1, cx)], cx);
    });

    let snapshot = cx.update_entity(&combined, |buffer, cx| buffer.snapshot(cx));
    let display = cx.new(|cx| DisplayMap::new(snapshot, cx));
    let display = display_snapshot(cx, &display);
    let mut rows = display.rows(DisplayRow::ZERO, display.line_count());
    let headers = std::iter::from_fn(|| rows.next())
        .filter_map(|row| row.block().cloned())
        .filter(|block| block.kind == DisplayBlockKind::BufferHeader)
        .map(|block| block.excerpt.buffer_id())
        .collect::<Vec<_>>();
    assert_eq!(headers.len(), 2, "两个匿名 Buffer 都必须有独立 header");
    assert_ne!(headers[0], headers[1], "匿名 Buffer 不能共享空路径身份");
}

#[gpui::test]
fn projection_map_roundtrips_unicode_buffer_points_and_byte_offsets(cx: &mut TestAppContext) {
    let buffer = Buffer::from_text("a你😀\nβ".to_string(), BufferConfig::default())
        .expect("测试 Buffer 应能创建");
    let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
    let cases = [
        MultiBufferOffset::new(0),
        MultiBufferOffset::new(1),
        MultiBufferOffset::new(4),
        MultiBufferOffset::new(8),
        MultiBufferOffset::new(9),
        MultiBufferOffset::new(11),
    ];

    for offset in cases {
        let display_point = display_snapshot(cx, &map)
            .offset_to_display_point(offset)
            .expect("合法字节偏移应能映射");
        assert_eq!(
            display_snapshot(cx, &map)
                .buffer_snapshot()
                .byte_to_position(
                    display_snapshot(cx, &map)
                        .display_point_to_offset(display_point)
                        .expect("合法显示点应能还原"),
                )
                .expect("合法显示点应能还原"),
            display_snapshot(cx, &map)
                .buffer_snapshot()
                .byte_to_position(offset)
                .expect("合法字节偏移应能转换为位置")
        );
        assert_eq!(
            display_snapshot(cx, &map)
                .offset_to_display_point(offset)
                .expect("合法字节偏移应能映射"),
            display_point
        );
        assert_eq!(
            display_snapshot(cx, &map)
                .display_point_to_offset(display_point)
                .expect("合法 DisplayPoint 应能转回 MultiBufferOffset"),
            offset
        );
    }
}

#[gpui::test]
fn projection_map_uses_display_columns_for_tabs(cx: &mut TestAppContext) {
    let buffer = Buffer::from_text("\tx".to_string(), BufferConfig::default())
        .expect("测试 Buffer 应能创建");
    let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));

    let after_tab = display_snapshot(cx, &map)
        .offset_to_display_point(MultiBufferOffset::new(1))
        .expect("tab 后的偏移应能映射");
    assert_eq!(after_tab.column(), DisplayColumn::new(4));
    assert_eq!(
        display_snapshot(cx, &map)
            .display_point_to_offset(after_tab)
            .expect("显示列应能还原为 tab 后的偏移"),
        MultiBufferOffset::new(1)
    );
}

#[gpui::test]
fn projection_map_rejects_out_of_bounds_points_and_invalid_byte_boundaries(
    cx: &mut TestAppContext,
) {
    let buffer =
        Buffer::from_text("你".to_string(), BufferConfig::default()).expect("测试 Buffer 应能创建");
    let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));

    assert!(
        display_snapshot(cx, &map)
            .buffer_snapshot()
            .position_to_byte(Position::new(Line::ZERO, LogicalColumn::new(2)))
            .is_err()
    );
    assert!(
        display_snapshot(cx, &map)
            .display_point_to_offset(DisplayPoint::new(DisplayRow::new(1), DisplayColumn::ZERO,))
            .is_err()
    );
    assert!(
        display_snapshot(cx, &map)
            .offset_to_display_point(MultiBufferOffset::new(1))
            .is_err()
    );
}

#[gpui::test]
fn projection_map_keeps_its_snapshot_version_after_buffer_changes(cx: &mut TestAppContext) {
    let mut buffer =
        Buffer::from_text("a".to_string(), BufferConfig::default()).expect("测试 Buffer 应能创建");
    let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
    let mapped_version = display_snapshot(cx, &map).buffer_snapshot().version();

    buffer
        .edit(
            [Edit::insert(MultiBufferOffset::new(1).into(), "b").unwrap()],
            TransactionMetadata::default(),
        )
        .expect("测试编辑应成功");

    assert_ne!(mapped_version, buffer.version());
    assert_eq!(
        display_snapshot(cx, &map).buffer_snapshot().version(),
        mapped_version
    );
    assert_eq!(
        display_snapshot(cx, &map).buffer_snapshot().len_bytes(),
        MultiBufferOffset::new(1)
    );
    assert!(
        display_snapshot(cx, &map)
            .offset_to_display_point(MultiBufferOffset::new(2))
            .is_err()
    );
}

#[gpui::test]
fn folding_changes_display_rows_and_viewport_contents(cx: &mut TestAppContext) {
    let buffer = Buffer::from_text(
        "anchor\nhidden one\nhidden two\nafter".to_string(),
        BufferConfig::default(),
    )
    .expect("测试 Buffer 应能创建");
    let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
    let before = display_snapshot(cx, &map);
    fold_range(cx, &map, 6, 28).expect("折叠应成功");

    assert_eq!(display_snapshot(cx, &map).line_count(), 2);
    assert_eq!(
        display_snapshot(cx, &map)
            .offset_to_display_point(MultiBufferOffset::new("anchor\nhidden ".len()))
            .expect("隐藏位置应能投影")
            .row(),
        DisplayRow::ZERO
    );

    let snapshot = display_snapshot(cx, &map);
    assert_ne!(before.version(), snapshot.version());
    assert_eq!(
        before.buffer_snapshot().version(),
        snapshot.buffer_snapshot().version()
    );
    let mut cursor = snapshot.rows(DisplayRow::ZERO, 8);
    let rows: Vec<_> = std::iter::from_fn(|| cursor.next()).collect();
    assert_eq!(rows.len(), 2);
    assert!(matches!(rows[1].kind(), WrapRowKind::Text { .. }));
}

#[gpui::test]
fn folded_rows_have_an_immediately_derived_tab_width(cx: &mut TestAppContext) {
    let text = "before\nfn folded() {\n  let value = 1;\n}\nafter\n";
    let buffer =
        Buffer::from_text(text.to_owned(), BufferConfig::default()).expect("测试 Buffer 应能创建");
    let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
    let fold_start = text.find('\n').expect("折叠入口行应有换行符");
    let fold_end = text.find("}\n").expect("折叠范围应有闭合行");
    fold_range(cx, &map, fold_start, fold_end).expect("折叠应成功");

    let line_count = display_snapshot(cx, &map).line_count();
    let longest = cx.read_entity(&map, |map, _| map.longest_unwrapped_row());
    assert!(longest.get() < line_count);
}

#[gpui::test]
fn folded_bracket_projects_close_to_merged_row(cx: &mut TestAppContext) {
    // 回归：折叠后闭合括号保留可见，光标在 `{` 上的括号高亮投影到合并行的真实 `}` 列。
    let buffer = Buffer::from_text(
        "fn main() {\n    let x = 1;\n}\nfn other() {\n    let y = 2;\n}".to_string(),
        BufferConfig::default(),
    )
    .expect("测试 Buffer 应能创建");
    let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
    // 折叠 fn main：范围 = [行 0 换行符(11), `}`(27))。
    fold_range(cx, &map, 11, 27).expect("折叠应成功");
    let snapshot = display_snapshot(cx, &map);

    // 真实 `}` 的字节范围投影到合并行占位符之后的列（anchor 11 字符 + 占位符 1 列 = 12）。
    let projected = snapshot
        .project_text_range(
            MultiBufferRange::new(MultiBufferOffset::new(27), MultiBufferOffset::new(28))
                .expect("`}` 范围应合法"),
        )
        .expect("投影应成功");
    assert_eq!(projected.len(), 1);
    assert_eq!(
        projected[0].start(),
        DisplayPoint::new(DisplayRow::ZERO, DisplayColumn::new(12))
    );
    assert_eq!(
        projected[0].end(),
        DisplayPoint::new(DisplayRow::ZERO, DisplayColumn::new(13))
    );

    // 占位符列（11）吸附折叠起点字节；尾段列（12）映射到 close 行字节（`}`）。
    assert_eq!(
        snapshot
            .display_point_to_offset(DisplayPoint::new(DisplayRow::ZERO, DisplayColumn::new(11)))
            .expect("占位符列应可映射"),
        MultiBufferOffset::new(11)
    );
    assert_eq!(
        snapshot
            .display_point_to_offset(DisplayPoint::new(DisplayRow::ZERO, DisplayColumn::new(12)))
            .expect("尾段列应可映射"),
        MultiBufferOffset::new(27)
    );
    assert_eq!(
        snapshot
            .display_point_to_offset_with_bias(
                DisplayPoint::new(DisplayRow::ZERO, DisplayColumn::new(11)),
                FoldBias::Left,
            )
            .expect("占位符左偏置应可映射"),
        MultiBufferOffset::new(11)
    );
    assert_eq!(
        snapshot
            .display_point_to_offset_with_bias(
                DisplayPoint::new(DisplayRow::ZERO, DisplayColumn::new(11)),
                FoldBias::Right,
            )
            .expect("占位符右偏置应可映射到折叠终点"),
        MultiBufferOffset::new(27)
    );
    // 合并行行尾 = close 行内容末尾。
    assert_eq!(
        display_snapshot(cx, &map)
            .end_of_row(MultiBufferOffset::new(11))
            .expect("行尾应可定位"),
        MultiBufferOffset::new(28)
    );
    // 可见字节全偏移 roundtrip（26 是折叠内隐藏字节，投影不可逆）。
    for offset in [0usize, 11, 27, 28, 29, 57] {
        let point = snapshot
            .offset_to_display_point(MultiBufferOffset::new(offset))
            .expect("可见偏移应能映射");
        assert_eq!(
            snapshot
                .display_point_to_offset(point)
                .expect("显示点应能还原"),
            MultiBufferOffset::new(offset)
        );
    }
}

#[gpui::test]
fn tab_width_change_updates_tab_point_without_a_line_width_cache(cx: &mut TestAppContext) {
    let buffer = Buffer::from_text("\tx".to_string(), BufferConfig::default())
        .expect("测试 Buffer 应能创建");
    let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
    let before = display_snapshot(cx, &map);
    let before_point = before
        .offset_to_display_point(MultiBufferOffset::new(1))
        .expect("Tab 后的文本必须可映射");
    assert_eq!(before_point.column().get(), 4);
    set_tab_width(
        cx,
        &map,
        NonZeroUsize::new(2).expect("测试 Tab 宽度必须非零"),
    );

    let after = display_snapshot(cx, &map);
    assert_ne!(before.version(), after.version());
    assert_eq!(
        after
            .offset_to_display_point(MultiBufferOffset::new(1))
            .expect("配置变化后的 Tab 后文本必须可映射")
            .column()
            .get(),
        2
    );
}

#[gpui::test]
fn rows_consumes_the_requested_rows(cx: &mut TestAppContext) {
    let buffer = Buffer::from_text("a\nb\nc".to_string(), BufferConfig::default())
        .expect("测试 Buffer 应能创建");
    let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
    let snapshot = display_snapshot(cx, &map);
    let mut cursor = snapshot.rows(DisplayRow::ZERO, snapshot.line_count());
    let mut rows = Vec::new();
    while let Some(row) = cursor.next() {
        rows.push(row);
    }
    assert_eq!(rows.len(), snapshot.line_count());
}

fn wrap_map(text: &str, width: f32, cx: &mut TestAppContext) -> Entity<DisplayMap> {
    let buffer =
        Buffer::from_text(text.to_owned(), BufferConfig::default()).expect("测试 Buffer 应能创建");
    let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
    set_wrap_width(cx, &map, Some(px(width)), font("Helvetica"), px(16.));
    map
}

#[gpui::test]
fn async_rewrap_settles_after_background_task(cx: &mut TestAppContext) {
    // 大文本 + 软换行：确保重排超出同步时限，走后台任务，再由 run_until_parked 落地。
    let text: String = (0..1_500)
        .map(|row| format!("line {row} 这是一段足够长的中文文本，用来触发软换行与后台重排\n"))
        .collect();
    let mut buffer =
        Buffer::from_text(text, BufferConfig::default()).expect("测试 Buffer 应能创建");
    let snapshot: MultiBufferSnapshot = buffer.snapshot().into();
    let display = cx.new(|cx| {
        let mut display_map = DisplayMap::new(snapshot.clone(), cx);
        display_map.set_wrap_width(
            Some(px(120.)),
            font("Helvetica"),
            px(16.),
            &cx.text_system().clone(),
            cx,
        );
        display_map
    });
    let subscription = buffer.subscribe();
    buffer
        .edit(
            [Edit::insert(MultiBufferOffset::new(0).into(), "新插入的一行\n").unwrap()],
            TransactionMetadata::default(),
        )
        .expect("测试编辑应成功");
    let updated: MultiBufferSnapshot = buffer.snapshot().into();
    let batch = subscription.consume();
    cx.update_entity(&display, |display_map, cx| {
        display_map.sync(updated, batch, cx);
    });
    cx.run_until_parked();

    let line_count = display_snapshot(cx, &display).line_count();
    assert!(
        line_count > 1_500,
        "软换行后显示行数应显著增加，实际 {line_count}"
    );
    // 后台完成后 offset → display point → offset 仍一致。
    cx.read_entity(&display, |display_map, _| {
        assert_offset_roundtrip(display_map)
    });
}

/// 回归：非换行短行会合并成一个同构变换；同一批次两个编辑都落在该变换内时，
/// 增量 splice 的游标不得越过第二个编辑的起点（cannot seek backward）。
#[gpui::test]
fn two_edits_inside_one_isomorphic_run_keep_wrap_forward(cx: &mut TestAppContext) {
    let text: String = (0..20).map(|row| format!("line{row:02}\n")).collect();
    let mut buffer =
        Buffer::from_text(text, BufferConfig::default()).expect("测试 Buffer 应能创建");
    let snapshot: MultiBufferSnapshot = buffer.snapshot().into();
    let display = cx.new(|cx| {
        let mut display_map = DisplayMap::new(snapshot.clone(), cx);
        display_map.set_wrap_width(
            Some(px(200.)),
            font("Helvetica"),
            px(16.),
            &cx.text_system().clone(),
            cx,
        );
        display_map
    });
    let subscription = buffer.subscribe();
    // 固定 7 字节行："lineNN\n"；在第 5、15 行行首各插入一行。
    buffer
        .edit(
            [
                Edit::insert(MultiBufferOffset::new(5 * 7).into(), "new05\n").unwrap(),
                Edit::insert(MultiBufferOffset::new(15 * 7).into(), "new15\n").unwrap(),
            ],
            TransactionMetadata::default(),
        )
        .expect("测试编辑应成功");
    let updated: MultiBufferSnapshot = buffer.snapshot().into();
    let batch = subscription.consume();
    cx.update_entity(&display, |display_map, cx| {
        display_map.sync(updated, batch, cx);
    });
    cx.run_until_parked();
    cx.read_entity(&display, |display_map, _| {
        assert_offset_roundtrip(display_map);
    });
}

/// 回归：只有下层版本推进、没有 Tab 结构编辑时（元数据/语法变化），Wrap 变换树必须保留，
/// 不能被重建为空树（否则 check_invariants 会看到输入点 0 ≠ Tab 行数）。
#[gpui::test]
fn metadata_only_tab_change_keeps_wrap_transform_tree(cx: &mut TestAppContext) {
    let old_text: String = (0..200).map(|row| format!("aaa{row:03}\n")).collect();
    let old_snapshot: MultiBufferSnapshot = Buffer::from_text(old_text, BufferConfig::default())
        .expect("测试 Buffer 应能创建")
        .snapshot()
        .into();
    let display = cx.new(|cx| {
        let mut display_map = DisplayMap::new(old_snapshot.clone(), cx);
        display_map.set_wrap_width(
            Some(px(200.)),
            font("Helvetica"),
            px(16.),
            &cx.text_system().clone(),
            cx,
        );
        display_map
    });
    // 另一个同行的快照：模拟"下层版本/元数据前进、批次里没有文本编辑"。
    let new_text: String = (0..200).map(|row| format!("bbb{row:03}\n")).collect();
    let mut new_buffer =
        Buffer::from_text(new_text, BufferConfig::default()).expect("测试 Buffer 应能创建");
    new_buffer
        .edit(
            [Edit::replace(
                zcv_text::TextRange::new(
                    MultiBufferOffset::new(0).into(),
                    MultiBufferOffset::new(3).into(),
                )
                .unwrap(),
                "zzz",
            )],
            TransactionMetadata::default(),
        )
        .expect("测试编辑应成功");
    let new_snapshot: MultiBufferSnapshot = new_buffer.snapshot().into();
    cx.update_entity(&display, |display_map, cx| {
        display_map.sync(new_snapshot, TextChangeBatch::default(), cx);
    });
    cx.run_until_parked();
    cx.read_entity(&display, |display_map, _| {
        assert_offset_roundtrip(display_map);
    });
}

/// 回归：同一未换行同构段内的多处编辑混合插入与删除时，重排后的变换输入
/// 必须仍精确覆盖 Tab 行数（不能多也不能少）。
#[gpui::test]
fn mixed_insert_delete_in_one_isomorphic_run_keeps_wrap_input_aligned(cx: &mut TestAppContext) {
    let text: String = (0..20).map(|row| format!("line{row:02}\n")).collect();
    let mut buffer =
        Buffer::from_text(text, BufferConfig::default()).expect("测试 Buffer 应能创建");
    let snapshot: MultiBufferSnapshot = buffer.snapshot().into();
    let display = cx.new(|cx| {
        let mut display_map = DisplayMap::new(snapshot.clone(), cx);
        display_map.set_wrap_width(
            Some(px(200.)),
            font("Helvetica"),
            px(16.),
            &cx.text_system().clone(),
            cx,
        );
        display_map
    });
    let subscription = buffer.subscribe();
    // 固定 7 字节行；同一批在第 5 行插入、删除第 10 行、在第 15 行插入。
    buffer
        .edit(
            [
                Edit::insert(MultiBufferOffset::new(5 * 7).into(), "new05\n").unwrap(),
                Edit::delete(
                    zcv_text::TextRange::new(
                        MultiBufferOffset::new(10 * 7).into(),
                        MultiBufferOffset::new(11 * 7).into(),
                    )
                    .unwrap(),
                ),
                Edit::insert(MultiBufferOffset::new(15 * 7).into(), "new15\n").unwrap(),
            ],
            TransactionMetadata::default(),
        )
        .expect("测试编辑应成功");
    let updated: MultiBufferSnapshot = buffer.snapshot().into();
    let batch = subscription.consume();
    cx.update_entity(&display, |display_map, cx| {
        display_map.sync(updated, batch, cx);
    });
    cx.run_until_parked();
    cx.read_entity(&display, |display_map, _| {
        assert_offset_roundtrip(display_map);
    });
}

/// 压力回归：随机多编辑批次在软换行下重排后，Wrap 变换输入必须精确等于 Tab 行数。
#[gpui::test]
fn random_multi_edit_wrap_sync_keeps_input_coverage(cx: &mut TestAppContext) {
    fn next(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *state >> 33
    }

    let mut text = String::new();
    for row in 0..2400 {
        if row % 3 == 0 {
            text.push_str(&format!("long {row} {}\n", "x".repeat(150)));
        } else {
            text.push_str(&format!("s{row}\n"));
        }
    }
    let mut buffer =
        Buffer::from_text(text.clone(), BufferConfig::default()).expect("测试 Buffer 应能创建");
    let snapshot: MultiBufferSnapshot = buffer.snapshot().into();
    let display = cx.new(|cx| {
        let mut display_map = DisplayMap::new(snapshot.clone(), cx);
        display_map.set_wrap_width(
            Some(px(200.)),
            font("Helvetica"),
            px(16.),
            &cx.text_system().clone(),
            cx,
        );
        display_map
    });
    let subscription = buffer.subscribe();
    let mut mirror = text;
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    for round in 0..400 {
        let len = mirror.len();
        if len < 60 {
            break;
        }
        let count = 1 + (next(&mut state) as usize % 4);
        let mut edits = Vec::new();
        let mut replacements = Vec::new();
        let mut position = 2 + (next(&mut state) as usize % 8);
        for index in 0..count {
            if position + 6 >= len {
                break;
            }
            let available = len - position - 2;
            let span = 1 + (next(&mut state) as usize % available.min(12));
            let end = position + span;
            let replacement = if next(&mut state).is_multiple_of(3) {
                format!("R{index}\n")
            } else {
                format!("r{index}")
            };
            edits.push(Edit::replace(
                zcv_text::TextRange::new(
                    MultiBufferOffset::new(position).into(),
                    MultiBufferOffset::new(end).into(),
                )
                .unwrap(),
                replacement.clone(),
            ));
            replacements.push((position..end, replacement));
            position = end + 2 + (next(&mut state) as usize % 7);
        }
        if edits.is_empty() {
            continue;
        }
        for (range, replacement) in replacements.iter().rev() {
            mirror.replace_range(range.clone(), replacement);
        }
        buffer
            .edit(edits, TransactionMetadata::default())
            .expect("测试编辑应成功");
        let updated: MultiBufferSnapshot = buffer.snapshot().into();
        let batch = subscription.consume();
        cx.update_entity(&display, |display_map, cx| {
            display_map.sync(updated, batch, cx);
        });
        // 连续编辑不等待：覆盖后台重排 + 急切插值同时有 pending 批次的路径。
        if round % 4 == 3 {
            cx.run_until_parked();
        }
    }
    cx.run_until_parked();
    cx.read_entity(&display, |display_map, _| {
        assert_offset_roundtrip(display_map);
    });
}

/// 对每个字符边界做 offset ↔ display point 双向 roundtrip。
fn assert_offset_roundtrip(map: &DisplayMap) {
    let snapshot = map.cached_snapshot();
    let len = snapshot.buffer_snapshot().len_bytes().get();
    let mut offset = 0;
    while offset < len {
        let point = snapshot
            .offset_to_display_point(MultiBufferOffset::new(offset))
            .expect("合法偏移应能映射");
        assert_eq!(
            snapshot
                .display_point_to_offset(point)
                .expect("显示点应能还原"),
            MultiBufferOffset::new(offset),
            "offset {offset} roundtrip 失败"
        );
        offset += snapshot
            .buffer_snapshot()
            .text_for_range(
                MultiBufferRange::new(MultiBufferOffset::new(offset), MultiBufferOffset::new(len))
                    .expect("测试范围应合法"),
            )
            .expect("文本应可读取")
            .chars()
            .next()
            .map_or(1, char::len_utf8);
    }
    let _ = len;
}

#[gpui::test]
fn soft_wrap_splits_wide_lines_into_display_rows(cx: &mut TestAppContext) {
    // 前导空白产生续行缩进。
    let map = wrap_map("    aa bbb cccc ddddd eeee\nshort", 72., cx);
    assert!(display_snapshot(cx, &map).is_wrapped());
    assert!(
        display_snapshot(cx, &map).line_count() > 2,
        "宽行应拆成多个显示行"
    );

    let snapshot = display_snapshot(cx, &map);
    let mut cursor = snapshot.rows(DisplayRow::ZERO, display_snapshot(cx, &map).line_count());
    let rows: Vec<_> = std::iter::from_fn(|| cursor.next()).collect();
    assert_eq!(rows.len(), display_snapshot(cx, &map).line_count());
    assert_eq!(rows[0].index(), DisplayRow::ZERO);

    // 首段行号从 0 开始，续行片段起点大于 0 且带假空格缩进。
    let WrapRowKind::Text {
        fragment_index,
        byte_range,
        indent,
        ..
    } = rows[1].kind();
    assert_eq!(*fragment_index, 1);
    assert!(*indent > 0, "前导空白应产生续行缩进");
    assert!(byte_range.start > 0, "续行应从行中某字节开始");
}

#[gpui::test]
fn soft_wrap_mixed_commit_message_rows_fit_the_shaped_width(cx: &mut TestAppContext) {
    let message = "修复 SVG 与 Markdown 公式预览的缩放、居中、清晰度、颜色及边界裁剪问题";
    let width = px(420.);
    let map = wrap_map(message, 420., cx);
    let snapshot = display_snapshot(cx, &map);
    let mut cursor = snapshot.rows(DisplayRow::ZERO, snapshot.line_count());
    let rows: Vec<_> = std::iter::from_fn(|| cursor.next()).collect();
    let text_system = gpui::WindowTextSystem::new(cx.text_system().clone());
    let font = font("Helvetica");
    let run = gpui::TextRun {
        len: message.len(),
        font,
        ..Default::default()
    };

    for row in &rows {
        let WrapRowKind::Text {
            byte_range,
            indent,
            projected_line,
            ..
        } = row.kind();
        let text = projected_line_text(&snapshot, *projected_line).expect("显示行文本应可解析");
        let mut rendered = " ".repeat(*indent);
        rendered.push_str(&text.as_ref()[byte_range.clone()]);
        let run = gpui::TextRun {
            len: rendered.len(),
            font: run.font.clone(),
            ..run.clone()
        };
        let shaped = text_system.shape_line(rendered.into(), px(16.), &[run], None);
        assert!(
            shaped.width() <= width,
            "提交信息软换行行宽不能超过统一布局宽度：width={width:?}, shaped={:?}",
            shaped.width()
        );
    }
}

#[gpui::test]
fn soft_wrap_without_leading_whitespace_has_zero_indent(cx: &mut TestAppContext) {
    let map = wrap_map("aa bbb cccc ddddd eeee\nshort", 72., cx);
    let snapshot = display_snapshot(cx, &map);
    let mut cursor = snapshot.rows(DisplayRow::ZERO, display_snapshot(cx, &map).line_count());
    let rows: Vec<_> = std::iter::from_fn(|| cursor.next()).collect();
    let WrapRowKind::Text { indent, .. } = rows[1].kind();
    assert_eq!(*indent, 0, "无前导空白的行不应产生缩进");
}

#[gpui::test]
fn soft_wrap_passthrough_when_disabled(cx: &mut TestAppContext) {
    let buffer = Buffer::from_text(
        "aa bbb cccc ddddd eeee\nshort".to_string(),
        BufferConfig::default(),
    )
    .expect("测试 Buffer 应能创建");
    let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
    set_wrap_width(cx, &map, None, font("Helvetica"), px(16.));
    assert!(!display_snapshot(cx, &map).is_wrapped());
    assert_eq!(display_snapshot(cx, &map).line_count(), 2);
    cx.read_entity(&map, |m, _| assert_offset_roundtrip(m));
}

#[gpui::test]
fn soft_wrap_coordinates_roundtrip_through_fragments(cx: &mut TestAppContext) {
    // 含 CJK 与 tab 的行，验证片段内列换算与字节映射一致。
    let map = wrap_map("aa bbb\tccc 你好世界 ddddd eeee\nshort", 72., cx);
    cx.read_entity(&map, |m, _| assert_offset_roundtrip(m));
}

#[gpui::test]
fn soft_wrap_inline_edit_rewraps_affected_line(cx: &mut TestAppContext) {
    let mut buffer = Buffer::from_text(
        "aa bbb cccc ddddd eeee\nshort".to_string(),
        BufferConfig::default(),
    )
    .expect("测试 Buffer 应能创建");
    let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
    set_wrap_width(cx, &map, Some(px(72.)), font("Helvetica"), px(16.));
    let before = display_snapshot(cx, &map);
    let wrapped_rows = before.line_count();

    let subscription = buffer.subscribe();
    buffer
        .edit(
            [Edit::insert(MultiBufferOffset::new("aa bbb ".len()).into(), "xxxx").unwrap()],
            TransactionMetadata::default(),
        )
        .expect("测试编辑应成功");
    sync(cx, &map, buffer.snapshot(), subscription.consume());

    let after = display_snapshot(cx, &map);
    assert_ne!(before.version(), after.version());
    assert!(after.line_count() >= wrapped_rows, "编辑后行数应重新计算");
    cx.read_entity(&map, |m, _| assert_offset_roundtrip(m));
}

#[gpui::test]
fn soft_wrap_inline_edit_inside_merged_isomorphic_segment_keeps_line_count(
    cx: &mut TestAppContext,
) {
    let text = (0..106)
        .map(|row| format!("let value_{row} = {row};\n"))
        .collect::<String>();
    let mut buffer =
        Buffer::from_text(text, BufferConfig::default()).expect("测试 Buffer 应能创建");
    let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
    set_wrap_width(cx, &map, Some(px(800.)), font("Helvetica"), px(16.));
    let expected_lines = buffer.line_count();

    let subscription = buffer.subscribe();
    let edit_offset = buffer.line_start_byte(Line::new(28)).expect("测试行应存在");
    buffer
        .edit(
            [Edit::insert(edit_offset, "#").unwrap()],
            TransactionMetadata::default(),
        )
        .expect("行内插入 # 应成功");
    sync(cx, &map, buffer.snapshot(), subscription.consume());

    assert_eq!(
        display_snapshot(cx, &map).buffer_snapshot().line_count(),
        expected_lines
    );
    assert_eq!(display_snapshot(cx, &map).line_count(), expected_lines);
    cx.read_entity(&map, |m, _| assert_offset_roundtrip(m));
}

#[gpui::test]
fn soft_wrap_structural_edit_rewraps_all_rows(cx: &mut TestAppContext) {
    let mut buffer = Buffer::from_text(
        "aa bbb cccc ddddd eeee\nshort".to_string(),
        BufferConfig::default(),
    )
    .expect("测试 Buffer 应能创建");
    let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
    set_wrap_width(cx, &map, Some(px(72.)), font("Helvetica"), px(16.));

    let subscription = buffer.subscribe();
    buffer
        .edit(
            [Edit::insert(MultiBufferOffset::new(3).into(), "\n").unwrap()],
            TransactionMetadata::default(),
        )
        .expect("测试编辑应成功");
    sync(cx, &map, buffer.snapshot(), subscription.consume());
    cx.read_entity(&map, |m, _| assert_offset_roundtrip(m));
}

#[gpui::test]
fn soft_wrap_equal_row_multi_line_edit_stays_incremental(cx: &mut TestAppContext) {
    // 等行数多行编辑（行数不变、折叠拓扑不变，如行移动/撤销回放）：
    // 不再按"含换行"升级为全量重排，软换行逐行增量重排即可。
    let text = (0..40)
        .map(|row| format!("line number {row} content here\n"))
        .collect::<String>();
    let mut buffer =
        Buffer::from_text(text, BufferConfig::default()).expect("测试 Buffer 应能创建");
    let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
    set_wrap_width(cx, &map, Some(px(150.)), font("Helvetica"), px(16.));
    let expected_rows = display_snapshot(cx, &map).line_count();

    let subscription = buffer.subscribe();
    // 替换 3 行为 3 行更长的内容：行数不变、内容变化，增量路径应覆盖全部受影响行。
    buffer
        .edit(
            [Edit::replace(
            MultiBufferRange::new(
                buffer.line_start_byte(Line::new(5)).expect("测试行应存在"),
                buffer.line_start_byte(Line::new(8)).expect("测试行应存在"),
            )
            .expect("测试行区间应合法").into(),
            "replaced line aaaaaaaaaaaaaaaaaaaaa\nreplaced line bbbbbbbbbbbbbbbbbbbbbbb\nreplaced line ccccccccccccccccccccc\n",
            )],
            TransactionMetadata::default(),
        )
        .expect("测试事务应成功");

    sync(cx, &map, buffer.snapshot(), subscription.consume());
    cx.read_entity(&map, |m, _| assert_offset_roundtrip(m));
    // 变更行变长后软换行显示行数应增加（增量重排确实生效）。
    assert!(
        display_snapshot(cx, &map).line_count() > expected_rows,
        "变长内容应产生更多显示行"
    );
}

#[gpui::test]
fn soft_wrap_stays_active_after_editing_an_expanded_diff_excerpt(cx: &mut TestAppContext) {
    let path = PathBuf::from("src/a.rs");
    let prefix = "let before = ".repeat(12);
    let suffix = "; let after = value;\n".repeat(8);
    let working_text = format!("{prefix}fresh_marker{suffix}");
    let base_text = format!("{prefix}base_marker{suffix}");
    let registry = Arc::new(LanguageRegistry::new());
    let source = cx.new(|cx| {
        LanguageBuffer::new(
            Buffer::from_text(working_text, BufferConfig::default()).expect("测试 Buffer 应能创建"),
            Some(path.clone()),
            Arc::clone(&registry),
            cx,
        )
    });
    cx.run_until_parked();

    let diff = cx.new(|cx| {
        BufferDiff::new(
            BufferDiffInput {
                working: source.clone(),
                path: path.clone(),
                base_text: Some(base_text),
                index_text: None,
                language_registry: registry,
                key: 0,
                operations: None,
            },
            cx,
        )
    });
    cx.run_until_parked();

    let combined = cx.new(MultiBuffer::empty);
    let line_count = cx.read_entity(&source, |source, _cx| source.text_snapshot().line_count());
    cx.update_entity(&combined, |buffer, cx| {
        buffer.add_diff(
            DiffFile {
                diff,
                display_path: path.clone(),
                excerpt_ranges: vec![0..line_count],
            },
            cx,
        );
        buffer.set_diff_hunks_expanded_by_default(true, cx);
    });
    cx.run_until_parked();

    let (subscription, snapshot) =
        cx.update_entity(&combined, |buffer, cx| buffer.subscribe_and_snapshot(cx));
    let display = cx.new(|cx| DisplayMap::new(snapshot, cx));
    display.update(cx, |map, cx| {
        map.set_multi_buffer(combined.clone(), subscription, cx);
    });
    set_wrap_width(cx, &display, Some(px(120.)), font("Helvetica"), px(16.));
    let before = display_snapshot(cx, &display);
    assert!(before.is_wrapped());
    assert!(before.line_count() > before.buffer_snapshot().line_count());

    let composite = String::from_utf8(before.buffer_snapshot().text_bytes())
        .expect("diff 组合文本必须是 UTF-8");
    let current_marker = composite
        .rfind("fresh_marker")
        .expect("展开 diff 必须包含可编辑的新侧文本");
    cx.update_entity(&combined, |buffer, cx| {
        buffer
            .edit(
                vec![Edit::insert(MultiBufferOffset::new(current_marker + 2).into(), "x").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("新侧 diff excerpt 应接受编辑");
    });
    cx.run_until_parked();

    let after = display_snapshot(cx, &display);
    assert!(after.is_wrapped(), "diff 重算不能关闭软换行");
    assert!(
        after.line_count() > after.buffer_snapshot().line_count(),
        "diff 元数据刷新后软换行投影仍应包含续行"
    );
}

#[gpui::test]
fn soft_wrap_with_fold_collapses_hidden_rows(cx: &mut TestAppContext) {
    let buffer = Buffer::from_text(
        "anchor\nhidden one\nhidden two\nafter".to_string(),
        BufferConfig::default(),
    )
    .expect("测试 Buffer 应能创建");
    let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
    fold_range(cx, &map, 6, 28).expect("折叠应成功");
    set_wrap_width(cx, &map, Some(px(72.)), font("Helvetica"), px(16.));

    let snapshot = display_snapshot(cx, &map);
    let mut cursor = snapshot.rows(DisplayRow::ZERO, 10);
    let rows: Vec<_> = std::iter::from_fn(|| cursor.next()).collect();
    assert_eq!(rows.len(), 2, "折叠后仅剩 anchor 与 after 两行");
    // 折叠隐藏区域的位置映射到 anchor 是现状语义（roundtrip 不可逆），
    // 只对可见文本字节做双向验证。
    for offset in [
        0usize,
        "anchor".len(),
        "anchor\nhidden one\nhidden two\nafter".len() - 1,
    ] {
        let point = snapshot
            .offset_to_display_point(MultiBufferOffset::new(offset))
            .expect("可见偏移应能映射");
        assert_eq!(
            snapshot
                .display_point_to_offset(point)
                .expect("显示点应能还原"),
            MultiBufferOffset::new(offset)
        );
    }
}

#[gpui::test]
fn soft_wrap_row_boundaries_follow_fragments(cx: &mut TestAppContext) {
    let buffer = Buffer::from_text(
        "aa bbb cccc ddddd eeee".to_string(),
        BufferConfig::default(),
    )
    .expect("测试 Buffer 应能创建");
    let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
    set_wrap_width(cx, &map, Some(px(72.)), font("Helvetica"), px(16.));
    assert!(display_snapshot(cx, &map).line_count() > 1);

    let snapshot = display_snapshot(cx, &map);
    // 第二行（首个续行）行首 = 片段起点字节，行尾 = 片段终点字节。
    let continuation_offset = snapshot
        .display_point_to_offset(DisplayPoint::new(DisplayRow::new(1), DisplayColumn::ZERO))
        .expect("续行行首应可映射");
    assert_eq!(
        display_snapshot(cx, &map)
            .beginning_of_row(continuation_offset)
            .expect("行首应可定位"),
        continuation_offset
    );
    let end = display_snapshot(cx, &map)
        .end_of_row(continuation_offset)
        .expect("行尾应可定位");
    assert!(end.get() > continuation_offset.get(), "行尾应在片段终点");
    assert_eq!(
        snapshot
            .display_point_to_offset(DisplayPoint::new(
                DisplayRow::new(1),
                DisplayColumn::new(200),
            ))
            .expect("越界列应钳制到行尾"),
        end
    );
    // 片段终点即下一片段起点（前闭后开）：从终点再行首停在下一片段起点。
    assert_eq!(
        display_snapshot(cx, &map)
            .beginning_of_row(end)
            .expect("行尾再行首应回到片段起点"),
        end
    );
    // 片段中间的任意位置行首都回到片段起点。
    let middle = MultiBufferOffset::new((continuation_offset.get() + end.get()) / 2);
    assert_eq!(
        display_snapshot(cx, &map)
            .beginning_of_row(middle)
            .expect("片段中间行首应回到片段起点"),
        continuation_offset
    );
}

#[gpui::test]
fn projected_range_columns_use_display_width_for_cjk(cx: &mut TestAppContext) {
    // 全角字符占两个显示列；投影范围列必须与渲染端 window_start_column 同处显示列空间，
    // 否则含 CJK 时选区/词级 diff/括号几何会把显示列当字符列换算，落到错误字节。
    let buffer = Buffer::from_text("你a\n".to_string(), BufferConfig::default())
        .expect("测试 Buffer 应能创建");
    let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));
    let snapshot = display_snapshot(cx, &map);

    let projected = snapshot
        .project_text_range(
            MultiBufferRange::new(MultiBufferOffset::new(3), MultiBufferOffset::new(4))
                .expect("a 范围应合法"),
        )
        .expect("投影应成功");
    assert_eq!(projected.len(), 1);
    assert_eq!(
        projected[0].start(),
        DisplayPoint::new(DisplayRow::ZERO, DisplayColumn::new(2)),
        "全角字符之后的起始列应为显示列 2，而不是字符计数 1"
    );
    assert_eq!(
        projected[0].end(),
        DisplayPoint::new(DisplayRow::ZERO, DisplayColumn::new(3))
    );
}

/// 性能测量入口：未换行模式下 `longest_unwrapped_row` 的单次成本随文档行数的变化。
///
/// 仅测试使用，不改变生产可见性。用
/// `cargo test -p zcv-editor --lib measure_longest_unwrapped_row_scaling -- --nocapture` 查看输出。
#[gpui::test]
fn measure_longest_unwrapped_row_scaling(cx: &mut TestAppContext) {
    use std::hint::black_box;
    use std::time::Instant;

    for lines in [1_000usize, 8_000, 32_000] {
        let text: String = (0..lines)
            .map(|index| format!("fn line_{index}() {{ let value = {index}; }}\n"))
            .collect();
        let buffer =
            Buffer::from_text(text, BufferConfig::default()).expect("测量 Buffer 应能创建");
        let map = cx.new(|cx| DisplayMap::new(buffer.snapshot(), cx));

        // 预热，排除首次分配与惰性初始化。
        black_box(cx.read_entity(&map, |map, _| map.longest_unwrapped_row()));

        const ITERATIONS: u32 = 50;
        let start = Instant::now();
        for _ in 0..ITERATIONS {
            black_box(cx.read_entity(&map, |map, _| map.longest_unwrapped_row()));
        }
        println!(
            "longest_unwrapped_row lines={lines} per_call={:?}",
            start.elapsed() / ITERATIONS
        );
    }
}
