use std::path::{Path, PathBuf};

use gpui::{AppContext as _, TestAppContext};
use zcv_language::LanguageBuffer;
use zcv_text::{Buffer, BufferConfig, ByteOffset, Edit, TextRange, TransactionMetadata};

use super::*;

fn singleton(path: &str, text: &str, cx: &mut TestAppContext) -> gpui::Entity<MultiBuffer> {
    let buffer = cx.new(|_| {
        Buffer::scratch(text.to_owned(), BufferConfig::default()).expect("应创建测试 Buffer")
    });
    let language = cx.new(|cx| LanguageBuffer::new(buffer, Some(PathBuf::from(path)), cx));
    cx.new(|cx| MultiBuffer::singleton(language, cx))
}

#[gpui::test]
fn singleton_snapshot_tracks_text_syntax_and_path(cx: &mut TestAppContext) {
    let multi_buffer = singleton("src/main.rs", "fn main() {}\n", cx);
    let text_buffer = cx.read_entity(&multi_buffer, |buffer, cx| {
        assert_eq!(buffer.file_path(cx), Some(PathBuf::from("src/main.rs")));
        buffer.as_singleton(cx).expect("应为 singleton")
    });

    cx.update_entity(&text_buffer, |buffer, cx| {
        buffer
            .edit(
                [Edit::insert(ByteOffset::new(3), "async ").unwrap()],
                TransactionMetadata::default(),
            )
            .expect("测试编辑应成功");
        cx.notify();
    });
    cx.run_until_parked();

    let updated = cx.read_entity(&multi_buffer, |buffer, cx| buffer.snapshot(cx));
    assert_eq!(updated.text().version(), updated.syntax().version());
}

#[gpui::test]
fn excerpts_preserve_order_and_map_output_to_source(cx: &mut TestAppContext) {
    let first = singleton("src/a.rs", "zero\none\ntwo\n", cx);
    let second = singleton("src/b.rs", "alpha\nbeta\n", cx);
    let combined = cx.new(MultiBuffer::empty);

    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                MultiBufferExcerpt::new(
                    first,
                    TextRange::new(ByteOffset::new(5), ByteOffset::new(9)).unwrap(),
                    vec![TextRange::new(ByteOffset::new(5), ByteOffset::new(8)).unwrap()],
                ),
                MultiBufferExcerpt::new(
                    second,
                    TextRange::new(ByteOffset::new(6), ByteOffset::new(11)).unwrap(),
                    vec![TextRange::new(ByteOffset::new(6), ByteOffset::new(10)).unwrap()],
                ),
            ],
            cx,
        );
    });

    let (text, excerpts, first_location, second_location, match_ranges) =
        cx.read_entity(&combined, |buffer, cx| {
            let snapshot = buffer.snapshot(cx);
            let text = String::from_utf8(snapshot.text_bytes()).unwrap();
            let first_offset = ByteOffset::new(text.find("one").unwrap());
            let second_offset = ByteOffset::new(text.find("beta").unwrap());
            (
                text,
                snapshot.excerpts().to_vec(),
                buffer
                    .location_for_range(
                        TextRange::new(first_offset, ByteOffset::new(first_offset.get() + 3))
                            .unwrap(),
                    )
                    .unwrap(),
                buffer.location_for_offset(second_offset).unwrap(),
                buffer.match_ranges().to_vec(),
            )
        });

    assert_eq!(text, "one\nbeta\n");
    assert_eq!(excerpts.len(), 2);
    assert_eq!(excerpts[0].display_path(), Path::new("src/a.rs"));
    assert_eq!(excerpts[1].display_path(), Path::new("src/b.rs"));
    assert_eq!(excerpts[0].source_start_line(), 2);
    assert_eq!(excerpts[1].source_start_line(), 2);
    assert_eq!(first_location.path, PathBuf::from("src/a.rs"));
    assert_eq!(
        first_location.source_range,
        TextRange::new(ByteOffset::new(5), ByteOffset::new(8)).unwrap()
    );
    assert_eq!(second_location.path, PathBuf::from("src/b.rs"));
    assert_eq!(second_location.source_range.start(), ByteOffset::new(6));
    assert_eq!(match_ranges.len(), 2);
    assert_eq!(match_ranges[0].start().get(), text.find("one").unwrap());
    assert_eq!(match_ranges[1].start().get(), text.find("beta").unwrap());
}

#[gpui::test]
fn composite_anchor_resolves_in_the_same_file_after_excerpt_refresh(cx: &mut TestAppContext) {
    let first = singleton("src/a.rs", "zero\none\ntwo\nthree\nfour\n", cx);
    let second = singleton("src/b.rs", "alpha\nbeta\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                MultiBufferExcerpt::line_range(first.clone(), 0..2, cx),
                MultiBufferExcerpt::line_range(first.clone(), 3..5, cx),
                MultiBufferExcerpt::line_range(second, 0..2, cx),
            ],
            cx,
        );
    });
    let anchor = cx.read_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        let excerpt = &snapshot.excerpts()[1];
        buffer
            .anchor_for_offset(ByteOffset::new(excerpt.output_range().start().get() + 2))
            .expect("应捕获第二个 hunk 内的位置")
    });

    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(vec![MultiBufferExcerpt::line_range(first, 0..2, cx)], cx);
        let offset = buffer
            .resolve_anchor(&anchor)
            .expect("同一文件仍有 excerpt 时应解析到最近位置");
        assert_eq!(
            offset,
            buffer.snapshot(cx).excerpts()[0].output_range().end()
        );
    });
}

#[gpui::test]
fn composite_anchor_falls_forward_when_its_file_leaves_the_diff(cx: &mut TestAppContext) {
    let first = singleton("src/a.rs", "one\n", cx);
    let second = singleton("src/b.rs", "two\n", cx);
    let third = singleton("src/c.rs", "three\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                MultiBufferExcerpt::line_range(first.clone(), 0..1, cx),
                MultiBufferExcerpt::line_range(second, 0..1, cx),
                MultiBufferExcerpt::line_range(third.clone(), 0..1, cx),
            ],
            cx,
        );
    });
    let anchor = cx.read_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        let excerpt = &snapshot.excerpts()[0];
        buffer
            .anchor_for_offset(excerpt.output_range().start())
            .expect("应捕获首文件位置")
    });

    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(vec![MultiBufferExcerpt::line_range(third, 0..1, cx)], cx);
        let offset = buffer
            .resolve_anchor(&anchor)
            .expect("原文件消失后应解析到仍存在的后继文件");
        assert_eq!(
            offset,
            buffer.snapshot(cx).excerpts()[0].output_range().start()
        );
    });
}

#[gpui::test]
fn empty_files_keep_distinct_composite_lines_and_locations(cx: &mut TestAppContext) {
    let first = singleton("deleted/a.rs", "", cx);
    let second = singleton("deleted/b.rs", "", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                MultiBufferExcerpt::line_range(first, 0..1, cx),
                MultiBufferExcerpt::line_range(second, 0..1, cx),
            ],
            cx,
        );
    });

    cx.read_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        assert_eq!(String::from_utf8(snapshot.text_bytes()).unwrap(), "\n\n");
        assert_eq!(snapshot.excerpts()[0].output_start_line(), 0);
        assert_eq!(snapshot.excerpts()[1].output_start_line(), 1);
        assert_eq!(
            buffer.location_for_offset(ByteOffset::new(1)).unwrap().path,
            PathBuf::from("deleted/b.rs")
        );
    });
}

#[gpui::test]
fn source_reparse_does_not_reload_composite_text(cx: &mut TestAppContext) {
    let source = singleton("src/main.rs", "fn main() {\n    println!(\"ok\");\n}\n", cx);
    let source_len = cx.read_entity(&source, |source, cx| source.snapshot(cx).text().len_bytes());
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |combined, cx| {
        combined.set_excerpts(
            vec![MultiBufferExcerpt::new(
                source,
                TextRange::new(ByteOffset::ZERO, source_len).unwrap(),
                Vec::new(),
            )],
            cx,
        );
    });
    let before = cx.read_entity(&combined, |combined, cx| combined.snapshot(cx).version());

    cx.run_until_parked();

    let after = cx.read_entity(&combined, |combined, cx| combined.snapshot(cx).version());
    assert_eq!(after, before, "语法解析完成不应重载组合投影文本");
}

#[gpui::test]
fn composite_edits_are_applied_to_the_underlying_buffer(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "zero\none\ntwo\n", cx);
    let source_text = cx.read_entity(&source, |buffer, cx| {
        buffer.as_singleton(cx).expect("应为 singleton")
    });
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![MultiBufferExcerpt::new(
                source,
                TextRange::new(ByteOffset::new(5), ByteOffset::new(9)).unwrap(),
                Vec::new(),
            )],
            cx,
        );
        buffer.start_transaction(cx).unwrap();
        buffer
            .edit(
                vec![Edit::replace(
                    TextRange::new(ByteOffset::ZERO, ByteOffset::new(3)).unwrap(),
                    "ONE",
                )],
                TransactionMetadata::default(),
                cx,
            )
            .unwrap();
        assert!(buffer.end_transaction(cx).is_some());
    });

    let source_contents = cx.read_entity(&source_text, |buffer, _| {
        buffer
            .slice_byte_range(ByteOffset::ZERO, buffer.len_bytes())
            .unwrap()
            .as_str()
            .to_owned()
    });
    assert_eq!(source_contents, "zero\nONE\ntwo\n");
    let projection = cx.read_entity(&combined, |buffer, cx| {
        String::from_utf8(buffer.snapshot(cx).text_bytes()).unwrap()
    });
    assert_eq!(projection, "ONE\n");
    cx.read_entity(&combined, |buffer, cx| {
        let files = buffer.file_buffers(cx);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].1, PathBuf::from("src/a.rs"));
        assert!(
            buffer.is_dirty(cx),
            "源 Buffer 变脏时组合文档也必须是 dirty"
        );
    });
}

#[gpui::test]
fn composite_file_buffers_are_deduplicated_across_excerpts(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "zero\none\ntwo\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                MultiBufferExcerpt::new(
                    source.clone(),
                    TextRange::new(ByteOffset::ZERO, ByteOffset::new(5)).unwrap(),
                    Vec::new(),
                ),
                MultiBufferExcerpt::new(
                    source,
                    TextRange::new(ByteOffset::new(5), ByteOffset::new(9)).unwrap(),
                    Vec::new(),
                ),
            ],
            cx,
        );
    });

    cx.read_entity(&combined, |buffer, cx| {
        assert_eq!(buffer.file_buffers(cx).len(), 1);
    });
}

#[gpui::test]
fn composite_tracks_edits_made_through_another_editor(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "zero\none\ntwo\n", cx);
    let source_text = cx.read_entity(&source, |buffer, cx| buffer.as_singleton(cx).unwrap());
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![MultiBufferExcerpt::new(
                source,
                TextRange::new(ByteOffset::new(5), ByteOffset::new(9)).unwrap(),
                Vec::new(),
            )],
            cx,
        )
    });

    cx.update_entity(&source_text, |buffer, cx| {
        buffer
            .edit(
                [Edit::replace(
                    TextRange::new(ByteOffset::new(5), ByteOffset::new(8)).unwrap(),
                    "ONE",
                )],
                TransactionMetadata::default(),
            )
            .unwrap();
        cx.notify();
    });
    cx.run_until_parked();

    let projection = cx.read_entity(&combined, |buffer, cx| {
        String::from_utf8(buffer.snapshot(cx).text_bytes()).unwrap()
    });
    assert_eq!(projection, "ONE\n");
}

#[gpui::test]
fn composite_splits_cross_excerpt_edits_across_source_buffers(cx: &mut TestAppContext) {
    let first = singleton("src/a.rs", "one\n", cx);
    let second = singleton("src/b.rs", "two\n", cx);
    let first_text = cx.read_entity(&first, |buffer, cx| buffer.as_singleton(cx).unwrap());
    let second_text = cx.read_entity(&second, |buffer, cx| buffer.as_singleton(cx).unwrap());
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                MultiBufferExcerpt::new(
                    first,
                    TextRange::new(ByteOffset::ZERO, ByteOffset::new(4)).unwrap(),
                    Vec::new(),
                ),
                MultiBufferExcerpt::new(
                    second,
                    TextRange::new(ByteOffset::ZERO, ByteOffset::new(4)).unwrap(),
                    Vec::new(),
                ),
            ],
            cx,
        );
        buffer.start_transaction(cx).unwrap();
        buffer
            .edit(
                vec![Edit::replace(
                    TextRange::new(ByteOffset::new(1), ByteOffset::new(6)).unwrap(),
                    "X",
                )],
                TransactionMetadata::default(),
                cx,
            )
            .unwrap();
        assert!(buffer.end_transaction(cx).is_some());
    });

    let read = |buffer: &gpui::Entity<Buffer>, cx: &TestAppContext| {
        cx.read_entity(buffer, |buffer, _| {
            buffer
                .slice_byte_range(ByteOffset::ZERO, buffer.len_bytes())
                .unwrap()
                .as_str()
                .to_owned()
        })
    };
    assert_eq!(read(&first_text, cx), "oX");
    assert_eq!(read(&second_text, cx), "o\n");
}

#[gpui::test]
fn read_only_composite_rejects_edits(cx: &mut TestAppContext) {
    let source = singleton("index.txt", "index 内容\n", cx);
    let combined = cx.new(MultiBuffer::empty_read_only);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(vec![MultiBufferExcerpt::line_range(source, 0..1, cx)], cx);
        assert!(buffer.is_read_only());
        let error = buffer
            .edit(
                vec![Edit::insert(ByteOffset::ZERO, "不能写入").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect_err("只读组合文档必须拒绝编辑");
        assert_eq!(
            error,
            zcv_text::TextError::Storage(zcv_text::StorageError::ReadOnly)
        );
    });
}

#[gpui::test]
fn materialized_diff_old_side_is_selectable_but_only_new_side_is_editable(cx: &mut TestAppContext) {
    let old = singleton("src/a.rs", "旧内容\n", cx);
    let current = singleton("src/a.rs", "上下文\n新内容\n之后\n", cx);
    let current_buffer = cx.read_entity(&current, |source, cx| source.as_singleton(cx).unwrap());
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                MultiBufferExcerpt::line_range(current.clone(), 0..1, cx),
                MultiBufferExcerpt::line_range(old.clone(), 0..1, cx)
                    .with_editable(false)
                    .with_starts_new_excerpt(false)
                    .with_diff_kind(ExcerptDiffKind::Deleted),
                MultiBufferExcerpt::line_range(current.clone(), 1..2, cx)
                    .with_starts_new_excerpt(false)
                    .with_diff_kind(ExcerptDiffKind::Added),
                MultiBufferExcerpt::line_range(current, 2..3, cx).with_starts_new_excerpt(false),
            ],
            cx,
        );
    });

    cx.read_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        assert_eq!(
            String::from_utf8(snapshot.text_bytes()).unwrap(),
            "上下文\n旧内容\n新内容\n之后\n"
        );
        assert_eq!(snapshot.excerpts().len(), 4);
        assert!(snapshot.excerpts()[0].starts_new_excerpt());
        assert!(!snapshot.excerpts()[1].starts_new_excerpt());
        assert_eq!(snapshot.excerpts()[1].source_line_for_output_line(1), None);
        assert_eq!(buffer.file_buffers(cx).len(), 1, "旧修订来源不能参与保存");

        let old_offset = "上下文\n".len() + 1;
        let old_anchor = buffer
            .anchor_for_offset(ByteOffset::new(old_offset))
            .expect("旧侧必须能建立普通 MultiBuffer 锚点");
        assert_eq!(
            buffer.resolve_anchor(&old_anchor),
            Some(ByteOffset::new(old_offset))
        );
    });

    cx.update_entity(&combined, |buffer, cx| {
        let old_error = buffer
            .edit(
                vec![Edit::insert(ByteOffset::new("上下文\n".len() + 1), "不能写").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect_err("旧侧只允许选择和导航");
        assert_eq!(
            old_error,
            zcv_text::TextError::Storage(zcv_text::StorageError::ReadOnly)
        );

        buffer
            .edit(
                vec![Edit::insert(ByteOffset::new("上下文\n旧内容\n".len()), "可写").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("新侧行首必须归属于可编辑片段");
    });

    let current_text = cx.read_entity(&current_buffer, |buffer, _| {
        buffer
            .slice_byte_range(ByteOffset::ZERO, buffer.len_bytes())
            .unwrap()
            .as_str()
            .to_owned()
    });
    assert_eq!(current_text, "上下文\n可写新内容\n之后\n");
    cx.read_entity(&combined, |buffer, cx| {
        assert_eq!(
            String::from_utf8(buffer.snapshot(cx).text_bytes()).unwrap(),
            "上下文\n旧内容\n可写新内容\n之后\n"
        );
    });
}
