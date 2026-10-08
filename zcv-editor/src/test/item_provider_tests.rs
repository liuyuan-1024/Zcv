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
