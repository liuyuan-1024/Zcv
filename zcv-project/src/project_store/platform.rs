use std::path::Path;

use anyhow::{Context as _, Result};

pub(super) fn create_symlink(
    source: &Path,
    target: &Path,
    destination: &Path,
    file_type: std::fs::FileType,
) -> Result<()> {
    #[cfg(unix)]
    {
        let _ = file_type;
        std::os::unix::fs::symlink(target, destination)
            .with_context(|| format!("重建符号链接失败：{}", source.display()))?;
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt as _;

        if file_type.is_symlink_dir() {
            std::os::windows::fs::symlink_dir(target, destination)
        } else {
            std::os::windows::fs::symlink_file(target, destination)
        }
        .with_context(|| format!("重建符号链接失败：{}", source.display()))?;
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = (source, target, destination, file_type);
        anyhow::bail!("当前平台不支持符号链接复制")
    }

    Ok(())
}
