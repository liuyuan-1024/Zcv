use zcv_multi_buffer::{MultiBufferOffset, MultiBufferSnapshot};
use zcv_text::{Buffer, BufferConfig, Line};

use super::*;

fn multi_snapshot(text: &str) -> MultiBufferSnapshot {
    Buffer::from_text(text.to_owned(), BufferConfig::default())
        .expect("测试 Buffer 应能创建")
        .snapshot()
        .into()
}

fn inlay(_snapshot: &MultiBufferSnapshot, position: usize, text: &str) -> Inlay {
    Inlay {
        position: MultiBufferOffset::new(position),
        text: text.to_owned(),
    }
}

fn snapshot_with(text: &str, specs: &[(usize, &str)]) -> InlaySnapshot {
    let buffer = multi_snapshot(text);
    let mut map = InlayMap::new(buffer.clone()).0;
    let inlays = specs
        .iter()
        .map(|(position, text)| inlay(&buffer, *position, text))
        .collect();
    map.sync(buffer, Vec::new(), inlays).0
}

#[test]
fn line_text_borrows_without_inlays() {
    let snapshot = snapshot_with("ab\ncd", &[]);
    let text = snapshot.line_text(Line::new(0)).expect("行 0 应可解析");
    assert_eq!(text, "ab\n");
    assert!(matches!(text, Cow::Borrowed(_)));
}

#[test]
fn line_text_projects_inlays_after_anchor_characters() {
    let snapshot = snapshot_with("ab\ncd", &[(1, ": hint")]);
    // 行 0 投影：锚定 'a' 之后注入。
    let text = snapshot.line_text(Line::new(0)).unwrap();
    assert_eq!(text, "a: hintb\n");
    assert!(matches!(text, Cow::Owned(_)));
    // buffer 行 1 无注入。
    assert_eq!(snapshot.line_text(Line::new(1)).unwrap(), "cd");
}

#[test]
fn multiple_inlays_accumulate_prefix() {
    let snapshot = snapshot_with("ab\ncd", &[(0, "A"), (1, "BB")]);
    let infos = snapshot.line_inlays(Line::new(0));
    assert_eq!(infos.len(), 2);
    assert_eq!(infos[0].anchor, 0);
    assert_eq!(infos[0].projected, 0);
    assert_eq!(infos[1].anchor, 1);
    assert_eq!(infos[1].projected, 1 + 1);
    assert_eq!(snapshot.line_text(Line::new(0)).unwrap(), "AaBBb\n");
}

#[test]
fn offset_roundtrip_and_inlay_snapping() {
    let snapshot = snapshot_with("abcdef\n", &[(2, "XY")]);
    let line = Line::new(0);
    // 原始 → 投影（字符起点语义）：锚定偏移处的字符在注入文本之后，右移注入长度。
    assert_eq!(snapshot.to_projected_offset(line, 1), 1);
    assert_eq!(snapshot.to_projected_offset(line, 2), 4);
    assert_eq!(snapshot.to_projected_offset(line, 3), 5);
    // 投影 → 原始：注入段内吸附到锚定后；段外减去前缀。
    assert_eq!(snapshot.to_original_offset(line, 2), 2);
    assert_eq!(snapshot.to_original_offset(line, 3), 2);
    assert_eq!(snapshot.to_original_offset(line, 4), 2);
    assert_eq!(snapshot.to_original_offset(line, 5), 3);
    assert_eq!(snapshot.to_original_offset(line, 7), 5);
    // roundtrip：原始 → 投影 → 原始 恒等。
    for byte in 0..6 {
        assert_eq!(
            snapshot.to_original_offset(line, snapshot.to_projected_offset(line, byte)),
            byte
        );
    }
}

#[test]
fn version_changes_only_on_inlay_config_change() {
    let buffer = multi_snapshot("ab\n");
    let mut map = InlayMap::new(buffer.clone()).0;
    let first = map
        .sync(buffer.clone(), Vec::new(), vec![inlay(&buffer, 1, "x")])
        .0;
    // 相同配置重复同步：不变化。
    let second = map
        .sync(buffer.clone(), Vec::new(), vec![inlay(&buffer, 1, "x")])
        .0;
    assert_eq!(second.version(), first.version());
    // 配置变化：版本递增。
    let third_inlays = vec![inlay(&buffer, 1, "xx")];
    let third = map.sync(buffer, Vec::new(), third_inlays).0;
    assert_eq!(third.version(), first.version() + 1);
}
