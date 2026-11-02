//! 选择领域测试：稳定坐标投影、手势状态机、命中几何与公式占位映射。

use gpui::{
    Bounds, IntoElement, ParentElement, Render, Styled, StyledText, TestAppContext, Window, div,
    point, px, size,
};
use zcv_settings::{SettingsStore, UserSettings};

use super::{PlaceholderMap, RenderSelection, SelectableText, SelectionLayout, SelectionModel};
use crate::document::{Block, BlockKind, Inline, InlineStyle, parse};

fn plain(text: &str) -> Inline {
    Inline {
        text: text.into(),
        style: InlineStyle::default(),
    }
}

fn math_inline(source: &str) -> Inline {
    Inline {
        text: source.into(),
        style: InlineStyle {
            math: true,
            ..Default::default()
        },
    }
}

fn paragraph(content: Vec<Inline>) -> Block {
    Block {
        source: 0..0,
        kind: BlockKind::Paragraph(content),
    }
}

#[test]
fn projection_separates_blocks_code_lines_and_cells() {
    let blocks = parse(
        "para one\n\n```rust\nfn a() {}\n\nlet x = 1;\n```\n\n| A | B |\n|---|---|\n| 1 | 2 |\n",
    );
    let selectable = SelectableText::from_blocks(&blocks);
    // 块间与代码行间以 \n 分隔，单元格间以 \t 分隔，空行保留。
    assert_eq!(
        selectable.text(),
        "para one\nfn a() {}\n\nlet x = 1;\nA\tB\n1\t2"
    );
    // 片段与渲染的文本单元一一对应：段落、非空代码行、非空单元格。
    assert_eq!(
        selectable.segments(),
        &[0..8, 9..18, 20..30, 31..32, 33..34, 35..36, 37..38]
    );
    // 块区间含尾部换行：三击复制整块时保持块边界。
    assert_eq!(selectable.block_range(5), Some(0..9));
    assert_eq!(selectable.block_range(25), Some(9..31));
    assert_eq!(selectable.block_range(32), Some(31..38));
}

#[test]
fn projection_keeps_math_source_as_selectable_text() {
    let blocks = vec![paragraph(vec![
        plain("a "),
        math_inline("x^2"),
        plain(" b"),
    ])];
    let selectable = SelectableText::from_blocks(&blocks);
    // 选择坐标空间直接使用 LaTeX 源码，复制无需事后替换。
    assert_eq!(selectable.text(), "a x^2 b");
}

#[test]
fn triple_click_selects_the_innermost_block() {
    let blocks = parse("- 项目一\n\n  项目二段落\n");
    let selectable = SelectableText::from_blocks(&blocks);
    let item_two = selectable.text().find("项目二").expect("应包含列表项");
    // 命中列表项内的段落：最内层块是段落，不是整个列表。
    let paragraph_range = selectable.block_range(item_two).expect("应命中段落块");
    assert_eq!(&selectable.text()[paragraph_range.clone()], "项目二段落");
}

#[test]
fn single_click_places_caret_and_end_clears_it() {
    let selectable = SelectableText::from_blocks(&[paragraph(vec![plain("hello world")])]);
    let mut model = SelectionModel::default();
    model.begin_selection(&selectable, 5, 1, false);
    assert_eq!(model.range(), Some(5..5));
    model.end_selection();
    assert_eq!(model.range(), None);
}

#[test]
fn double_click_selects_words_with_unicode_semantics() {
    let selectable = SelectableText::from_blocks(&[paragraph(vec![plain("hello 世界 foo")])]);
    let mut model = SelectionModel::default();
    model.begin_selection(&selectable, 2, 2, false);
    assert_eq!(model.range(), Some(0..5));
    // "世界" 占 6..12 字节；命中词内字符边界即选中整个词。
    model.begin_selection(&selectable, 6, 2, false);
    assert_eq!(model.range(), Some(6..12));
    model.end_selection();
}

#[test]
fn drag_updates_character_selection_across_blocks() {
    let selectable = SelectableText::from_blocks(&[
        paragraph(vec![plain("aaa")]),
        paragraph(vec![plain("bbb")]),
    ]);
    let mut model = SelectionModel::default();
    model.begin_selection(&selectable, 1, 1, false);
    model.update_selection(&selectable, 5);
    assert_eq!(model.range(), Some(1..5));
    // 反向拖拽同样成立。
    model.update_selection(&selectable, 0);
    assert_eq!(model.range(), Some(0..1));
    model.end_selection();
    assert_eq!(model.range(), Some(0..1));
}

#[test]
fn drag_snaps_word_selection_to_word_boundaries() {
    let selectable = SelectableText::from_blocks(&[paragraph(vec![plain("one two three")])]);
    let mut model = SelectionModel::default();
    // 双击 "two"（4..7）后向右拖到 "three" 内：整词吸附到 4..13。
    model.begin_selection(&selectable, 4, 2, false);
    model.update_selection(&selectable, 10);
    assert_eq!(model.range(), Some(4..13));
    // 向左拖回 "one" 内：吸附到 0..7。
    model.update_selection(&selectable, 1);
    assert_eq!(model.range(), Some(0..7));
}

#[test]
fn drag_snaps_block_selection_to_block_boundaries() {
    let selectable = SelectableText::from_blocks(&[
        paragraph(vec![plain("first")]),
        paragraph(vec![plain("second")]),
        paragraph(vec![plain("third")]),
    ]);
    let mut model = SelectionModel::default();
    // 三击第二段（6..13）后拖到第三段内：整块吸附。
    model.begin_selection(&selectable, 8, 3, false);
    assert_eq!(model.range(), Some(6..13));
    model.update_selection(&selectable, 16);
    assert_eq!(model.range(), Some(6..18));
    // 拖到第一段内：反向整块吸附。
    model.update_selection(&selectable, 2);
    assert_eq!(model.range(), Some(0..13));
}

#[test]
fn shift_click_extends_from_the_last_anchor() {
    let selectable = SelectableText::from_blocks(&[paragraph(vec![plain("hello world")])]);
    let mut model = SelectionModel::default();
    // 单击落定后 Shift+点击：从上次锚点扩展。
    model.begin_selection(&selectable, 2, 1, false);
    model.end_selection();
    model.begin_selection(&selectable, 8, 1, true);
    assert_eq!(model.range(), Some(2..8));
    // 反向 Shift+点击：从锚点收缩到 1..2。
    model.begin_selection(&selectable, 1, 1, true);
    assert_eq!(model.range(), Some(1..2));
    // Shift+双击按上次粒度提升：从锚点扩展到整词。
    model.end_selection();
    model.begin_selection(&selectable, 2, 1, false);
    model.end_selection();
    model.begin_selection(&selectable, 4, 2, true);
    assert_eq!(model.range(), Some(0..5));
}

#[test]
fn select_all_and_copy_project_math_source() {
    let selectable = SelectableText::from_blocks(&[paragraph(vec![
        plain("a "),
        math_inline("x^2"),
        plain(" b"),
    ])]);
    let mut model = SelectionModel::default();
    model.select_all(&selectable);
    assert_eq!(model.range(), Some(0..7));
    assert_eq!(model.selected_text(&selectable).as_deref(), Some("a x^2 b"));
}

#[test]
fn reset_clears_selection_and_anchor() {
    let selectable = SelectableText::from_blocks(&[paragraph(vec![plain("hello")])]);
    let mut model = SelectionModel::default();
    model.begin_selection(&selectable, 1, 1, false);
    model.update_selection(&selectable, 4);
    model.reset();
    assert_eq!(model.range(), None);
    // reset 后 Shift+点击没有可扩展的锚点，退化为普通定位。
    model.begin_selection(&selectable, 3, 1, true);
    assert_eq!(model.range(), Some(3..3));
}

#[test]
fn placeholder_mapping_maps_selection_to_render_ranges() {
    // 渲染文本 "a ⎵⎵⎵ b"（NBSP 占位），选择文本 "a x^2 b"。
    let mut layout = SelectionLayout::default();
    layout.begin_frame(&[0..7]);
    let run = layout.register_next_run(
        10,
        vec![PlaceholderMap {
            render: 2..8,
            selectable: 2..5,
        }],
        gpui::TextLayout::default(),
    );
    // 全选高亮整个渲染区间；选区终止于本 run，末行不做行尾延伸。
    assert_eq!(
        layout.render_range_for(run, &(0..7)),
        Some(RenderSelection {
            range: 0..10,
            ends_here: true,
        })
    );
    // 从公式终点开始的选择高亮占位之后的文本。
    assert_eq!(
        layout.render_range_for(run, &(5..7)),
        Some(RenderSelection {
            range: 8..10,
            ends_here: true,
        })
    );
    // 公式之前的选择不触及占位。
    assert_eq!(
        layout.render_range_for(run, &(0..2)),
        Some(RenderSelection {
            range: 0..2,
            ends_here: true,
        })
    );
    // 覆盖公式的选择高亮整个占位区域。
    assert_eq!(
        layout.render_range_for(run, &(2..5)),
        Some(RenderSelection {
            range: 2..8,
            ends_here: true,
        })
    );
    // 无交集返回 None。
    assert_eq!(layout.render_range_for(run, &(7..9)), None);
}

fn shape_two_runs_with_gap(
    cx: &mut gpui::TestAppContext,
) -> (SelectionLayout, Bounds<gpui::Pixels>, Bounds<gpui::Pixels>) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    struct TestWindow;

    impl Render for TestWindow {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div()
        }
    }

    let (_, cx) = cx.add_window_view(|_, _| TestWindow);
    let first = StyledText::new("first");
    let second = StyledText::new("second");
    let first_layout = first.layout().clone();
    let second_layout = second.layout().clone();
    cx.draw(Default::default(), size(px(200.), px(100.)), |_, _| {
        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(first.into_any_element())
            .child(second.into_any_element())
    });
    let first_bounds = first_layout.bounds();
    let second_bounds = second_layout.bounds();
    let mut layout = SelectionLayout::default();
    layout.begin_frame(&[0..5, 6..12]);
    let run_a = layout.register_next_run(5, Vec::new(), first_layout);
    let run_b = layout.register_next_run(6, Vec::new(), second_layout);
    layout.set_run_bounds(run_a, first_bounds);
    layout.set_run_bounds(run_b, second_bounds);
    (layout, first_bounds, second_bounds)
}

#[gpui::test]
fn hit_testing_clamps_and_splits_block_gaps(cx: &mut TestAppContext) {
    let (layout, first_bounds, second_bounds) = shape_two_runs_with_gap(cx);
    // 内容上方钳制到首段起点。
    assert_eq!(
        layout.index_for_position(point(px(100.), first_bounds.top() - px(50.))),
        Some(0)
    );
    // 块间隙上半归属前段末尾，下半归属后段开头。
    let gap_top = first_bounds.bottom();
    let gap_bottom = second_bounds.origin.y;
    let middle = (gap_top + gap_bottom) / 2.;
    assert_eq!(
        layout.index_for_position(point(px(100.), gap_top + px(1.))),
        Some(5)
    );
    assert_eq!(
        layout.index_for_position(point(px(100.), middle + px(1.))),
        Some(6)
    );
    // 内容下方钳制到末段终点。
    assert_eq!(
        layout.index_for_position(point(px(100.), second_bounds.bottom() + px(50.))),
        Some(12)
    );
    // 行内水平越界钳制到行尾。
    assert_eq!(
        layout.index_for_position(point(
            first_bounds.right() + px(50.),
            first_bounds.origin.y + px(5.)
        )),
        Some(5)
    );
}

#[gpui::test]
fn hit_testing_maps_math_placeholder_to_source_bounds(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    struct TestWindow;

    impl Render for TestWindow {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div()
        }
    }

    let (_, cx) = cx.add_window_view(|_, _| TestWindow);
    let placeholder = '\u{00A0}';
    let rendered = format!("a {placeholder}{placeholder}{placeholder} b");
    let text = StyledText::new(rendered.clone());
    let text_layout = text.layout().clone();
    cx.draw(Default::default(), size(px(200.), px(100.)), |_, _| {
        div().child(text.into_any_element())
    });
    let bounds = text_layout.bounds();
    let mut layout = SelectionLayout::default();
    layout.begin_frame(&[0..7]);
    let run = layout.register_next_run(
        rendered.len(),
        vec![PlaceholderMap {
            render: 2..8,
            selectable: 2..5,
        }],
        text_layout.clone(),
    );
    layout.set_run_bounds(run, bounds);

    let line_y = bounds.origin.y + px(5.);
    // 占位区域前半命中到公式起点。
    let front = text_layout.position_for_index(2).expect("占位起点应可定位");
    assert_eq!(layout.index_for_position(point(front.x, line_y)), Some(2));
    // 占位之后的 b 命中到公式终点之后（渲染偏移 9 经占位差补偿到 6）。
    let after = text_layout
        .position_for_index(9)
        .expect("占位后的字符应可定位");
    assert_eq!(layout.index_for_position(point(after.x, line_y)), Some(6));
}
#[test]
fn keyboard_extend_moves_head_and_keeps_tail() {
    let selectable = SelectableText::from_blocks(&[paragraph(vec![plain("hello world")])]);
    let mut model = SelectionModel::default();
    // 单击落定：锚点与活动端都在 5。
    model.begin_selection(&selectable, 5, 1, false);
    model.end_selection();
    // Shift+右 两次：固定端保持锚点，活动端右移两个字素。
    model.extend_to(6);
    model.extend_to(7);
    assert_eq!(model.range(), Some(5..7));
    assert_eq!(model.head(), 7);
    // Shift+左 一次：活动端回退，固定端不变。
    model.extend_to(6);
    assert_eq!(model.range(), Some(5..6));
    // 活动端回到锚点：空选区清除。
    model.extend_to(5);
    assert_eq!(model.range(), None);
    // 越过锚点继续左移：选区反向，固定端仍是原锚点。
    model.extend_to(3);
    assert_eq!(model.range(), Some(3..5));
}

#[test]
fn keyboard_extend_after_drag_uses_the_gesture_head() {
    let selectable = SelectableText::from_blocks(&[paragraph(vec![plain("hello world")])]);
    let mut model = SelectionModel::default();
    // 拖拽选择 2..8，活动端在 8。
    model.begin_selection(&selectable, 2, 1, false);
    model.update_selection(&selectable, 8);
    model.end_selection();
    assert_eq!(model.range(), Some(2..8));
    // Shift+右：从活动端 8 继续扩展，固定端是 2。
    model.extend_to(9);
    assert_eq!(model.range(), Some(2..9));
    // Shift+左 越过固定端：选区反向。
    model.extend_to(1);
    assert_eq!(model.range(), Some(1..2));
}

#[test]
fn keyboard_extend_after_select_all_shrinks_from_the_end() {
    let selectable = SelectableText::from_blocks(&[paragraph(vec![plain("hello")])]);
    let mut model = SelectionModel::default();
    model.select_all(&selectable);
    assert_eq!(model.range(), Some(0..5));
    // 全选后 Shift+左：从末端收缩一个字素。
    model.extend_to(4);
    assert_eq!(model.range(), Some(0..4));
}

#[test]
fn grapheme_moves_respect_multibyte_characters() {
    let selectable = SelectableText::from_blocks(&[paragraph(vec![plain("世界 foo")])]);
    // "世界" 每个汉字是 3 字节的独立字素。
    assert_eq!(selectable.next_grapheme_offset(0), 3);
    assert_eq!(selectable.next_grapheme_offset(3), 6);
    assert_eq!(selectable.prev_grapheme_offset(6), 3);
    assert_eq!(selectable.prev_grapheme_offset(3), 0);
    // 越界保持原位。
    assert_eq!(
        selectable.next_grapheme_offset(selectable.len()),
        selectable.len()
    );
    assert_eq!(selectable.prev_grapheme_offset(0), 0);
}

#[test]
fn render_offset_for_maps_caret_and_segment_gaps() {
    // 两个相邻片段（中间 \n 不属于任何片段），光标在段间时归属前一个 run 的末尾。
    let mut layout = SelectionLayout::default();
    layout.begin_frame(&[0..5, 6..12]);
    let run_a = layout.register_next_run(5, Vec::new(), gpui::TextLayout::default());
    let run_b = layout.register_next_run(6, Vec::new(), gpui::TextLayout::default());
    // run 内偏移恒等映射。
    assert_eq!(layout.render_offset_for(run_a, 2), Some(2));
    assert_eq!(layout.render_offset_for(run_b, 9), Some(3));
    // 段间换行偏移归属前一个 run 的末尾。
    assert_eq!(layout.render_offset_for(run_a, 5), Some(5));
    assert_eq!(layout.render_offset_for(run_b, 5), None);
    // 超出两 run 范围的偏移不属于任何 run。
    assert_eq!(layout.render_offset_for(run_a, 6), None);
    assert_eq!(layout.render_offset_for(run_b, 12), Some(6));
}

#[gpui::test]
fn vertical_move_crosses_blocks_and_clamps_at_edges(cx: &mut TestAppContext) {
    let (layout, _first_bounds, _second_bounds) = shape_two_runs_with_gap(cx);

    // 第二段行首向上：目标行中点落在块间隙上半，归属第一段末尾。
    assert_eq!(layout.vertical_move(6, true), Some(5));
    // 第二段行尾附近向上：同样经间隙分界到第一段末尾。
    assert_eq!(layout.vertical_move(11, true), Some(5));
    // 首段行首向上：已在首行，保持不动。
    assert_eq!(layout.vertical_move(0, true), None);
    // 末段终点向下：钳制到末段终点，活动端不变。
    assert_eq!(layout.vertical_move(12, false), Some(12));
    // 第一段末尾向下：目标行中点落在块间隙下半，归属第二段开头。
    assert_eq!(layout.vertical_move(5, false), Some(6));
}

#[test]
fn render_range_marks_whether_selection_ends_in_run() {
    // 两个相邻片段：选择坐标 0..5 与 6..12，段间分隔符偏移 5。
    let mut layout = SelectionLayout::default();
    layout.begin_frame(&[0..5, 6..12]);
    let run_a = layout.register_next_run(5, Vec::new(), gpui::TextLayout::default());
    let run_b = layout.register_next_run(6, Vec::new(), gpui::TextLayout::default());

    // 选区终止于第一个 run：末行不做行尾延伸。
    assert_eq!(
        layout.render_range_for(run_a, &(1..5)),
        Some(RenderSelection {
            range: 1..5,
            ends_here: true,
        })
    );
    // 选区恰好覆盖到 run 边界：选区终点等于 run 终点，仍视为终止于本 run。
    assert_eq!(
        layout.render_range_for(run_a, &(0..5)),
        Some(RenderSelection {
            range: 0..5,
            ends_here: true,
        })
    );
    // 选区跨过第一个 run 继续到第二个 run：第一个 run 的末行做行尾延伸。
    assert_eq!(
        layout.render_range_for(run_a, &(2..9)),
        Some(RenderSelection {
            range: 2..5,
            ends_here: false,
        })
    );
    assert_eq!(
        layout.render_range_for(run_b, &(2..9)),
        Some(RenderSelection {
            range: 0..3,
            ends_here: true,
        })
    );
}
