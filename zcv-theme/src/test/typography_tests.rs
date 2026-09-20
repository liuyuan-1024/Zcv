use super::*;
use gpui::TestAppContext;

/// UI 行高（=墨迹）必须容纳 UI 字体（含 CJK 回退）的真实墨迹：行盒 ⊇ 墨迹是裁剪容器不切字的前提。
#[gpui::test]
fn ui_line_covers_shaped_ink(cx: &mut TestAppContext) {
    cx.update(|cx| {
        set_base_typography(cx, None, Some(13.), None);
        let line = ui_line(cx);
        assert!(
            line > ui_size(cx),
            "行高应大于字号：ui_line={line}，ui_size={}",
            ui_size(cx)
        );
        for probe in ["Ag中", "gpqyj_j", "汉字徽章"] {
            let probe_ink = shaped_ink(cx, ui_font(), ui_size(cx), probe);
            assert!(
                line >= probe_ink,
                "行盒应容纳墨迹：probe={probe}，ui_line={line}，墨迹={probe_ink}"
            );
        }
    });
}

/// 内容行高同理容纳内容字体墨迹；且不低于墨迹下限（倍数调小也不裁剪的不变式）。
#[gpui::test]
fn content_line_covers_shaped_ink(cx: &mut TestAppContext) {
    cx.update(|cx| {
        // 故意把行高倍数调到小于墨迹比：content_line 的墨迹下限应兜底。
        set_base_typography(cx, Some(16.), None, Some(1.1));
        let line = content_line(cx);
        assert!(
            line > content_size(cx),
            "行高应大于字号：content_line={line}，content_size={}",
            content_size(cx)
        );
        for probe in ["Ag中", "gpqyj_j", "汉字徽章"] {
            let probe_ink = shaped_ink(cx, content_font(), content_size(cx), probe);
            assert!(
                line >= probe_ink,
                "行盒应容纳墨迹：probe={probe}，content_line={line}，墨迹={probe_ink}"
            );
        }
    });
}

/// probe 文本塑形后的墨迹高度（ascent + |descent|）；descent 符号跨平台不一致，取绝对值。
fn shaped_ink(cx: &App, font: Font, font_size: Pixels, probe: &str) -> Pixels {
    let text: SharedString = probe.into();
    let run = TextRun {
        len: text.len(),
        font,
        color: black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let shaped =
        WindowTextSystem::new(cx.text_system().clone()).shape_line(text, font_size, &[run], None);
    shaped.ascent + shaped.descent.abs()
}
