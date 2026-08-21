//! 工作区持久化公共原语：项目身份哈希与原子写盘。
//!
//! 布局状态与窗口边界共用同一套键控与提交机制，保证两个文件域中的项目身份一致。

use std::fs;
use std::path::Path;

use anyhow::{Context as _, Result};

/// 项目根的工作区身份：固定 FNV-1a 64 位哈希的十六进制形式。
/// 空工作区使用固定标识；同一项目在所有持久化文件中共用同一身份。
pub(crate) fn workspace_identity(root: Option<&Path>) -> String {
    let identity = root
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| "__empty__".to_owned());
    // 固定 FNV-1a，避免依赖 DefaultHasher 的跨版本实现细节。
    let hash = identity.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    format!("{hash:016x}")
}

/// 原子写盘：先写临时文件再重命名提交，避免中断留下半截文件。
/// 目录按需创建；Windows 上先移除已存在的目标（rename 无法覆盖）。
pub(crate) fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path.parent().context("持久化路径没有父目录")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("无法创建持久化目录 {}", parent.display()))?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, content)
        .with_context(|| format!("无法写入临时文件 {}", temporary.display()))?;
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("无法替换旧文件 {}", path.display()))?;
    }
    fs::rename(&temporary, path).with_context(|| format!("无法提交文件 {}", path.display()))?;
    Ok(())
}
