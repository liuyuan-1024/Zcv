use super::tests::render_line_chunks;
use super::*;
use gpui::rgba;

/// 背景覆盖层：命中区间内所有 chunk 都带背景色（搜索高亮普通/活动匹配共用此层）。
#[test]
fn backgrounds_layer_colors_all_matching_chunks() {
    let text = "abc abc";
    let line = render_line_chunks(
        text,
        4,
        0,
        HighlightStyles {
            spans: &[],
            styles: &[],
            backgrounds: &[(0..3, rgba(0x74ade83d)), (4..7, rgba(0x74ade8b3))],
            marked: &[],
            dimmed: &[],
        },
        0..text.len(),
    );
    let chunks = line.chunks;
    assert_eq!(chunks.len(), 3, "两个匹配 + 中间空格各自成段");
    // 普通匹配（0-3）与活动匹配（4-7）背景色不同且都命中。
    assert_eq!(chunks[0].background, Some(rgba(0x74ade83d)));
    assert_eq!(chunks[1].background, None, "无背景区间不应被着色");
    assert_eq!(chunks[2].background, Some(rgba(0x74ade8b3)));
}

/// 匹配紧邻引号（同一语法段）时，背景不得吞掉区间外的引号字符。
#[test]
fn backgrounds_do_not_spill_into_adjacent_quotes() {
    let text = "\"abc\" abc";
    // 匹配区间 1..4（abc），引号在 0 与 4。
    let line = render_line_chunks(
        text,
        4,
        0,
        HighlightStyles {
            spans: &[],
            styles: &[],
            backgrounds: &[(1..4, rgba(0x74ade83d))],
            marked: &[],
            dimmed: &[],
        },
        0..text.len(),
    );
    let chunks = line.chunks;
    assert_eq!(
        chunks.len(),
        3,
        "引号 / 匹配 / 空格后文本应切分为三段的精确子段"
    );
    assert_eq!(chunks[0].text, "\"", "左引号单独成段");
    assert_eq!(chunks[0].background, None, "左引号不应着色");
    assert_eq!(chunks[1].text, "abc");
    assert_eq!(
        chunks[1].background,
        Some(rgba(0x74ade83d)),
        "匹配词本身着色"
    );
}
