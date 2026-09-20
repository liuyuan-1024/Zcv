use super::*;

#[test]
fn tooltip_lines_are_structured_without_embedded_separators() {
    let spec = TooltipSpec::from_lines(["完整值", "右键复制该列信息", "第三行提示"]);

    assert_eq!(
        spec.lines,
        vec![
            "完整值".to_string(),
            "右键复制该列信息".to_string(),
            "第三行提示".to_string(),
        ]
    );
}
