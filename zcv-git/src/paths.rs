use std::path::PathBuf;

mod platform;

/// 将 Git 输出的原始路径字节转换为本地路径。
pub fn path_from_git_bytes(bytes: &[u8]) -> PathBuf {
    platform::path_from_git_bytes(bytes)
}
