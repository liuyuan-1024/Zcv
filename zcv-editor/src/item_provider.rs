//! 文本文件 Item Provider：项目文件 → Editor。
//!
//! 打开文件时框架经 ItemProvider 注册表分发到本 provider，使 Workspace 不直接依赖 Editor 类型。

use std::io::Read;
use std::path::{Path, PathBuf};

use gpui::{App, AppContext, Entity, Task};
use zcv_multi_buffer::MultiBuffer;
use zcv_project::Project;
use zcv_workspace::{ItemHandle, ItemProvider};

use crate::Editor;

/// 二进制检测窗口：读取文件头 8KB 检查 null 字节，命中视为二进制拒绝文本打开。
const BINARY_SNIFF_SIZE: usize = 8192;

/// 任何普通文件都交给编辑器打开（文本兜底，含 .gitignore 等无扩展名文件）。
pub(crate) struct TextFileProvider;

impl ItemProvider for TextFileProvider {
    fn supports(&self, path: &Path, _cx: &App) -> bool {
        !path.is_dir()
    }

    fn open_item(
        &self,
        path: PathBuf,
        project: Entity<Project>,
        cx: &mut App,
    ) -> Task<anyhow::Result<Box<dyn ItemHandle>>> {
        // 二进制防护：文件头含 null 字节（如 .DS_Store）以文本打开只会得到乱码，直接拒绝。
        if is_binary(&path) {
            return Task::ready(Err(anyhow::anyhow!("二进制文件，无法以文本打开")));
        }
        let singleton = match project.update(cx, |project, cx| project.open_buffer(&path, cx)) {
            Ok(multi_buffer) => multi_buffer,
            Err(error) => return Task::ready(Err(anyhow::anyhow!("{error}"))),
        };
        // 普通编辑器统一经 `from_working_source` 构建独立组合文档（整文件可编辑 excerpt）：
        // 项目共享 singleton 只作为工作区源，展开 diff hunk 时的 set_excerpts 只影响本组合文档，不污染项目共享文档（ProjectDiffView 等仍引用同一 singleton）。
        let multi_buffer = cx.new(|cx| MultiBuffer::from_working_source(singleton, cx));
        let editor = cx.new(|cx| Editor::for_multi_buffer(multi_buffer, cx));
        Task::ready(Ok(Box::new(editor) as Box<dyn ItemHandle>))
    }
}

/// 注册文本文件 Provider；可重复调用（按具体 Provider 类型去重）。
pub(crate) fn init(cx: &mut App) {
    zcv_workspace::register_item_provider(TextFileProvider, cx);
}

/// 文件头含 null 字节视为二进制（只读前 8KB，读取失败按文本放行）。
fn is_binary(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0u8; BINARY_SNIFF_SIZE];
    let read = file.read(&mut head).unwrap_or(0);
    head[..read].contains(&0)
}

#[cfg(test)]
mod tests {
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
}
