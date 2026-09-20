use gpui::TestAppContext;

use super::*;

#[gpui::test]
fn provider_supports_files_without_extension(cx: &mut TestAppContext) {
    cx.read(|cx| {
        // .gitignore / Makefile 等无扩展名文件必须可打开（文本兜底）。
        assert!(TextFileProvider.supports(Path::new(".gitignore"), cx));
        assert!(TextFileProvider.supports(Path::new("Makefile"), cx));
        assert!(TextFileProvider.supports(Path::new("src/main.rs"), cx));
    });
}

#[test]
fn binary_sniff_detects_null_bytes_in_head() {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let binary = directory.path().join("app.bin");
    std::fs::write(&binary, b"\x00\x01\x02").expect("应写入二进制内容");
    assert!(is_binary(&binary), "含 null 字节的文件应判定为二进制");

    let text = directory.path().join("note.txt");
    std::fs::write(&text, "普通文本内容").expect("应写入文本");
    assert!(!is_binary(&text), "纯文本不应判定为二进制");
}
