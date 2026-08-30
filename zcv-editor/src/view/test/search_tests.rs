//! 文件内搜索：SearchableItem 实现（搜索/跳转/替换/编辑后自动重搜）。

use gpui::{TestAppContext, VisualTestContext};
use zcv_text::ByteOffset;
use zcv_text::SearchQuery;
use zcv_workspace::{Direction, SearchableItem};

use super::common::test_buffer;
use super::*;
use crate::display_map::{
    DisplayMap, DisplayRow, LineStyles, StreamLineSource, ViewportChunkSource, WrapViewportRowKind,
    render_viewport_chunks,
};

fn editor_with_text<'a>(
    cx: &'a mut TestAppContext,
    text: &str,
) -> (Entity<Editor>, &'a mut VisualTestContext) {
    let buffer = test_buffer(cx, text);
    cx.add_window_view(move |_, cx| Editor::for_language_buffer(buffer, cx))
}

fn editor_text(editor: &Entity<Editor>, cx: &VisualTestContext) -> String {
    cx.read_entity(editor, |this, cx| {
        let buffer = this.multi_buffer.read(cx).snapshot(cx);
        let buffer = buffer.text();
        buffer
            .slice_byte_range(ByteOffset::ZERO, buffer.len_bytes())
            .expect("完整测试 Buffer 应可读取")
            .as_str()
            .to_owned()
    })
}

fn query(text: &str) -> SearchQuery {
    SearchQuery {
        query: text.to_string(),
        case_sensitive: false,
        whole_word: false,
        regex: false,
    }
}

#[gpui::test]
fn search_finds_all_matches_and_reports_count(cx: &mut TestAppContext) {
    let (editor, cx) = editor_with_text(cx, "abc abc abc");
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.search(&query("abc"), window, cx);
            assert_eq!(editor.search_count(cx), (3, Some(0)));
            let matches = editor.search_highlights().unwrap().0;
            assert_eq!(
                matches[0].range(),
                zcv_text::TextRange::new(ByteOffset::new(0), ByteOffset::new(3),).unwrap()
            );
            assert!(editor.search_highlights().is_some());
        });
    });
}

#[gpui::test]
fn search_respects_case_and_whole_word_options(cx: &mut TestAppContext) {
    let (editor, cx) = editor_with_text(cx, "Cat catalog cat");
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            // 大小写不敏感（默认）：3 处。
            editor.search(&query("cat"), window, cx);
            assert_eq!(editor.search_count(cx), (3, Some(0)));
            // 大小写敏感：只有小写 "cat" 2 处。
            let mut sensitive = query("cat");
            sensitive.case_sensitive = true;
            editor.search(&sensitive, window, cx);
            assert_eq!(editor.search_count(cx), (2, Some(0)));
            // 整词：排除 "catalog"，"Cat" 与 "cat" 都是独立词 → 2 处。
            let mut whole_word = query("cat");
            whole_word.whole_word = true;
            editor.search(&whole_word, window, cx);
            assert_eq!(editor.search_count(cx), (2, Some(0)));
        });
    });
}

#[gpui::test]
fn search_regex_matches_pattern(cx: &mut TestAppContext) {
    let (editor, cx) = editor_with_text(cx, "a1 b22 c333");
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            let mut regex = query(r"\d+");
            regex.regex = true;
            editor.search(&regex, window, cx);
            assert_eq!(editor.search_count(cx), (3, Some(0)));
            let matches = editor.search_highlights().unwrap().0;
            assert_eq!(
                matches[2].range(),
                zcv_text::TextRange::new(ByteOffset::new(8), ByteOffset::new(11),).unwrap()
            );
        });
    });
}

#[gpui::test]
fn empty_query_clears_search(cx: &mut TestAppContext) {
    let (editor, cx) = editor_with_text(cx, "abc");
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.search(&query("abc"), window, cx);
            assert_eq!(editor.search_count(cx), (1, Some(0)));
            // 空 query 视为无搜索。
            editor.search(&query(""), window, cx);
            assert_eq!(editor.search_count(cx), (0, None));
            assert!(editor.search_highlights().is_none());
        });
    });
}

#[gpui::test]
fn activate_match_moves_in_direction_and_wraps(cx: &mut TestAppContext) {
    let (editor, cx) = editor_with_text(cx, "abc abc abc");
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.search(&query("abc"), window, cx);
            assert_eq!(editor.search_count(cx), (3, Some(0)));
            // Next 循环：0 → 1 → 2 → 0。
            editor.activate_match_in_direction(Direction::Next, 1, window, cx);
            assert_eq!(editor.search_count(cx), (3, Some(1)));
            editor.activate_match_in_direction(Direction::Next, 1, window, cx);
            assert_eq!(editor.search_count(cx), (3, Some(2)));
            editor.activate_match_in_direction(Direction::Next, 1, window, cx);
            assert_eq!(editor.search_count(cx), (3, Some(0)));
            // Prev 循环：0 → 2。
            editor.activate_match_in_direction(Direction::Prev, 1, window, cx);
            assert_eq!(editor.search_count(cx), (3, Some(2)));
            // 跳转会移动选区到匹配位置（选区 head 指向匹配终点）。
            assert_eq!(editor.selections().primary().head(), ByteOffset::new(11));
            assert_eq!(editor.selections().primary().start(), ByteOffset::new(8));
        });
    });
}

#[gpui::test]
fn replace_current_replaces_active_match_only(cx: &mut TestAppContext) {
    let (editor, cx) = editor_with_text(cx, "abc abc abc");
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.search(&query("abc"), window, cx);
            assert!(editor.replace_current("X", window, cx));
        });
    });
    // 只替换活动匹配（第一个）。
    assert_eq!(editor_text(&editor, cx), "X abc abc");
}

#[gpui::test]
fn replace_all_replaces_every_match(cx: &mut TestAppContext) {
    let (editor, cx) = editor_with_text(cx, "abc abc abc");
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.search(&query("abc"), window, cx);
            let replaced = editor.replace_all("X", window, cx);
            assert_eq!(replaced, 3);
        });
    });
    assert_eq!(editor_text(&editor, cx), "X X X");
}

#[gpui::test]
fn editing_researches_and_keeps_active_match(cx: &mut TestAppContext) {
    let (editor, cx) = editor_with_text(cx, "foo bar foo");
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.search(&query("foo"), window, cx);
            assert_eq!(editor.search_count(cx), (2, Some(0)));
            // 经 Editor 编辑入口修改文本（触发事务 → research_after_edit）。
            editor.set_text("foo baz foo foo", cx);
        });
    });
    cx.read_entity(&editor, |editor, cx| {
        // 重搜后匹配更新（3 处），活动匹配保持原序号 0。
        assert_eq!(editor.search_count(cx), (3, Some(0)));
    });
}

#[gpui::test]
fn clear_search_removes_state_and_highlights(cx: &mut TestAppContext) {
    let (editor, cx) = editor_with_text(cx, "abc");
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.search(&query("abc"), window, cx);
            assert!(editor.search_highlights().is_some());
            editor.clear_search(window, cx);
            assert!(editor.search_highlights().is_none());
            assert_eq!(editor.search_count(cx), (0, None));
        });
    });
}

#[gpui::test]
fn replace_all_researches_and_clears_stale_result(cx: &mut TestAppContext) {
    let (editor, cx) = editor_with_text(cx, "abc abc abc");
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.search(&query("abc"), window, cx);
            assert_eq!(editor.search_count(cx), (3, Some(0)));
            let replaced = editor.replace_all("X", window, cx);
            assert_eq!(replaced, 3);
        });
    });
    // 替换后自动重搜：query 已无匹配，高亮与计数清空。
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.search_count(cx), (0, None));
        assert!(editor.search_highlights().is_none());
    });
    assert_eq!(editor_text(&editor, cx), "X X X");
}

#[gpui::test]
fn replace_current_researches_and_keeps_remaining_matches(cx: &mut TestAppContext) {
    let (editor, cx) = editor_with_text(cx, "abc abc abc");
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.search(&query("abc"), window, cx);
            assert!(editor.replace_current("X", window, cx));
        });
    });
    // 替换当前后自动重搜：剩 2 个匹配，活动匹配保持序号 0。
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.search_count(cx), (2, Some(0)));
    });
    assert_eq!(editor_text(&editor, cx), "X abc abc");
}

#[gpui::test]
fn replace_keeps_syntax_snapshot_in_sync(cx: &mut TestAppContext) {
    use std::path::PathBuf;

    use zcv_language::LanguageBuffer;
    use zcv_text::{Buffer, BufferConfig};

    // 带语言的 Buffer：替换必须通知 LanguageBuffer 同步语法树并重新解析，
    // 否则语法快照停留在旧版本，渲染层按版本闸门清空全部高亮。
    let text = "fn main() {\n    let x = 1;\n}\n";
    let buffer =
        Buffer::scratch(text.to_owned(), BufferConfig::default()).expect("测试 Buffer 应能创建");
    let buffer = cx.new(|_| buffer);
    let language_buffer =
        cx.new(|cx| LanguageBuffer::new(buffer.clone(), Some(PathBuf::from("main.rs")), cx));
    cx.run_until_parked();
    let (editor, cx) =
        cx.add_window_view(|_, cx| Editor::for_language_buffer(language_buffer.clone(), cx));
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.search(&query("fn main"), window, cx);
            assert!(editor.replace_current("MAIN", window, cx));
        });
    });
    cx.run_until_parked();
    let buffer_version = cx.read_entity(&buffer, |buffer, _| buffer.version());
    cx.read_entity(&language_buffer, |language_buffer, _| {
        assert_eq!(
            language_buffer.syntax_snapshot().version(),
            buffer_version,
            "替换后语法快照必须与文本版本同步"
        );
    });
}

#[gpui::test]
fn element_style_pipeline_backgrounds_all_matches(cx: &mut TestAppContext) {
    use std::ops::Range;

    let (editor, cx) = editor_with_text(cx, "abc abc abc");
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.search(&query("abc"), window, cx);
        });
        let engine_snapshot = editor.read(cx).render_snapshot();
        let display = DisplayMap::new(engine_snapshot.clone()).snapshot();
        let viewport = display.slice_viewport(DisplayRow::new(0), 1).unwrap();
        // 与 element.rs 相同的背景层构建。
        let search_highlights = editor.read(cx).search_highlights().unwrap();
        let colors = zcv_theme::color::current(cx);
        let search_backgrounds: Vec<(Range<usize>, gpui::Rgba)> = search_highlights
            .0
            .iter()
            .enumerate()
            .map(|(index, m)| {
                let r = m.range();
                (
                    r.start().get()..r.end().get(),
                    if index == search_highlights.1 {
                        colors.search_active_match_background
                    } else {
                        colors.search_match_background
                    },
                )
            })
            .collect();
        assert_eq!(search_backgrounds.len(), 3, "三个匹配都应进入背景层");
        for row in viewport.rows() {
            let WrapViewportRowKind::Text {
                source,
                text,
                byte_range,
                global_byte_start,
                ..
            } = row.kind();
            {
                let inlay_snapshot = display
                    .wrap_snapshot()
                    .tab_snapshot()
                    .fold_snapshot()
                    .inlay_snapshot();
                let stream_line = match source {
                    StreamLineSource::Buffer(line) => inlay_snapshot
                        .stream()
                        .buffer_to_stream(zcv_text::Line::new(*line)),
                    _ => continue,
                };
                let tab_width = display.buffer_snapshot().config().tab.tab_width();
                let rendered = render_viewport_chunks(
                    ViewportChunkSource {
                        text: text.as_ref(),
                        global_byte_start: *global_byte_start,
                        stream_line,
                        segments: None,
                        inlay: inlay_snapshot,
                    },
                    tab_width,
                    LineStyles {
                        spans: &[],
                        styles: &[],
                        backgrounds: &search_backgrounds,
                        marked: &[],
                    },
                    byte_range.clone(),
                );
                let with_bg = rendered
                    .chunks
                    .iter()
                    .filter(|c| c.background.is_some())
                    .count();
                assert!(with_bg >= 2, "同一行多个匹配都应带背景，实际 {with_bg}");
                return;
            }
        }
        panic!("未找到文本行");
    });
}

#[gpui::test]
fn backgrounds_render_across_multiple_lines(cx: &mut TestAppContext) {
    let (editor, cx) = editor_with_text(cx, "abc\nxxx\nabc\nabc");
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.search(&query("abc"), window, cx);
        });
        let engine_snapshot = editor.read(cx).render_snapshot();
        let display = DisplayMap::new(engine_snapshot.clone()).snapshot();
        // 渲染全部 4 行，统计带背景的 chunk。
        let search_highlights = editor.read(cx).search_highlights().unwrap();
        let colors = zcv_theme::color::current(cx);
        let search_backgrounds: Vec<(Range<usize>, gpui::Rgba)> = search_highlights
            .0
            .iter()
            .enumerate()
            .map(|(index, m)| {
                let r = m.range();
                (
                    r.start().get()..r.end().get(),
                    if index == search_highlights.1 {
                        colors.search_active_match_background
                    } else {
                        colors.search_match_background
                    },
                )
            })
            .collect();
        let viewport = display.slice_viewport(DisplayRow::new(0), 4).unwrap();
        let mut with_bg = 0usize;
        for row in viewport.rows() {
            let WrapViewportRowKind::Text {
                source,
                text,
                byte_range,
                global_byte_start,
                ..
            } = row.kind();
            {
                let inlay_snapshot = display
                    .wrap_snapshot()
                    .tab_snapshot()
                    .fold_snapshot()
                    .inlay_snapshot();
                let stream_line = match source {
                    StreamLineSource::Buffer(line) => inlay_snapshot
                        .stream()
                        .buffer_to_stream(zcv_text::Line::new(*line)),
                    _ => continue,
                };
                let tab_width = display.buffer_snapshot().config().tab.tab_width();
                let rendered = render_viewport_chunks(
                    ViewportChunkSource {
                        text: text.as_ref(),
                        global_byte_start: *global_byte_start,
                        stream_line,
                        segments: None,
                        inlay: inlay_snapshot,
                    },
                    tab_width,
                    LineStyles {
                        spans: &[],
                        styles: &[],
                        backgrounds: &search_backgrounds,
                        marked: &[],
                    },
                    byte_range.clone(),
                );
                with_bg += rendered
                    .chunks
                    .iter()
                    .filter(|c| c.background.is_some())
                    .count();
            }
        }
        assert_eq!(with_bg, 3, "三行的匹配都应带背景，实际 {with_bg}");
    });
}

/// 真实场景：带语法高亮的 Markdown 搜索 "zcv"，匹配行必须渲染背景（spans 与背景层共存）。
#[gpui::test]
fn markdown_with_syntax_highlights_backgrounds_matches(cx: &mut TestAppContext) {
    let text = r#"# zcv editor

- **zcv** search
- `zcv` highlighting
- [zcv](https://example.com/zcv)

## zcv rendering

> zcv backgrounds coexist with *zcv syntax spans*.

```text
zcv active
zcv inactive
zcv final
```
"#;
    let expected = text.matches("zcv").count();
    let buffer = zcv_text::Buffer::scratch(text.to_owned(), Default::default()).unwrap();
    let buffer = cx.new(|_| buffer);
    let language_buffer = cx.new(|cx| {
        zcv_language::LanguageBuffer::new(
            buffer,
            Some(std::path::PathBuf::from("search_fixture.md")),
            cx,
        )
    });
    let (editor, cx) =
        cx.add_window_view(move |_, cx| Editor::for_language_buffer(language_buffer, cx));

    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.search(&query("zcv"), window, cx);
        });
        // 用 zcv-text 快照构造 DisplayMap（与 element 渲染相同路径）。
        let snapshot = editor.read(cx).render_snapshot();
        let display = DisplayMap::new(snapshot.clone()).snapshot();
        let line_count = display.line_count();
        let search_highlights = editor.read(cx).search_highlights().unwrap();
        assert_eq!(
            search_highlights.0.len(),
            expected,
            "Markdown 夹具中的 zcv 应全部匹配"
        );
        let colors = zcv_theme::color::current(cx);
        let search_backgrounds: Vec<(Range<usize>, gpui::Rgba)> = search_highlights
            .0
            .iter()
            .enumerate()
            .map(|(index, m)| {
                let r = m.range();
                (
                    r.start().get()..r.end().get(),
                    if index == search_highlights.1 {
                        colors.search_active_match_background
                    } else {
                        colors.search_match_background
                    },
                )
            })
            .collect();
        // 渲染全部行，统计带背景的 chunk（应与匹配数一致）。
        let viewport = display
            .slice_viewport(DisplayRow::new(0), line_count)
            .unwrap();
        let mut with_bg = 0usize;
        for row in viewport.rows() {
            let WrapViewportRowKind::Text {
                source,
                text,
                byte_range,
                global_byte_start,
                ..
            } = row.kind();
            {
                let inlay_snapshot = display
                    .wrap_snapshot()
                    .tab_snapshot()
                    .fold_snapshot()
                    .inlay_snapshot();
                let stream_line = match source {
                    StreamLineSource::Buffer(line) => inlay_snapshot
                        .stream()
                        .buffer_to_stream(zcv_text::Line::new(*line)),
                    _ => continue,
                };
                let tab_width = display.buffer_snapshot().config().tab.tab_width();
                let rendered = render_viewport_chunks(
                    ViewportChunkSource {
                        text: text.as_ref(),
                        global_byte_start: *global_byte_start,
                        stream_line,
                        segments: None,
                        inlay: inlay_snapshot,
                    },
                    tab_width,
                    LineStyles {
                        // 与 element 相同：语法高亮 spans + 搜索背景层共存。
                        spans: &display.highlighted_spans_for_viewport(&viewport),
                        styles: display.highlight_styles(),
                        backgrounds: &search_backgrounds,
                        marked: &[],
                    },
                    byte_range.clone(),
                );
                with_bg += rendered
                    .chunks
                    .iter()
                    .filter(|c| c.background.is_some())
                    .count();
            }
        }
        assert_eq!(
            with_bg, expected,
            "{expected} 处 zcv 匹配都应带背景，实际 {with_bg}"
        );
    });
}
