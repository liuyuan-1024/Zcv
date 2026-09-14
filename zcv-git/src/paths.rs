use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use zcv_path::{AbsolutePathBuf, RelativePathBuf};

mod platform;

/// 将 Git 输出的原始路径字节转换为本地路径。
pub fn path_from_git_bytes(bytes: &[u8]) -> PathBuf {
    platform::path_from_git_bytes(bytes)
}

/// 把绝对路径或仓库相对路径转换成 Git revision 所需的 `/` 路径。
pub(crate) fn revision_path(working_directory: &Path, path: &Path) -> Result<RelativePathBuf> {
    let relative = if !path.is_absolute() {
        path.to_path_buf()
    } else if let Ok(relative) = path.strip_prefix(working_directory) {
        relative.to_path_buf()
    } else {
        let canonical =
            AbsolutePathBuf::canonicalize(path).context("无法规范化 Git revision 路径")?;
        canonical
            .as_path()
            .strip_prefix(working_directory)
            .context("路径不属于 Git 工作目录")?
            .to_path_buf()
    };
    RelativePathBuf::from_path(&relative).context("Git revision 路径不是有效的相对路径")
}
