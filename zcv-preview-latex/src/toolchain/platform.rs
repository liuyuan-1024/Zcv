//! TeX 工具查找依赖的平台路径和可执行文件规则。

use std::{
    env::consts::EXE_SUFFIX,
    path::{Path, PathBuf},
};

pub(super) fn executable(directory: &Path, program: &str) -> Option<PathBuf> {
    let path = directory.join(format!("{program}{EXE_SUFFIX}"));
    let metadata = path.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o111 == 0 {
            return None;
        }
    }
    Some(path)
}

pub(super) fn search_directories() -> impl Iterator<Item = PathBuf> {
    // 从 Finder 启动时，PATH 通常不包含 MacTeX 的工具目录。
    #[cfg(target_os = "macos")]
    let directories = [PathBuf::from("/Library/TeX/texbin")];
    #[cfg(not(target_os = "macos"))]
    let directories = [];
    directories.into_iter()
}
