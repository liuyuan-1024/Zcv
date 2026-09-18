//! 路径语义的平台后端。
//!
//! 只承载本机路径字符串语法与 Windows 扩展路径前缀转换；
//! 值类型、相对路径规范化与稳定身份的表征逻辑留在 `path.rs`。

use std::path::Path;

use super::PathStyle;

#[cfg(target_os = "windows")]
pub(super) const fn local_style() -> PathStyle {
    PathStyle::Windows
}

#[cfg(not(target_os = "windows"))]
pub(super) const fn local_style() -> PathStyle {
    PathStyle::Unix
}

/// 用于缓存键和工作区持久化的稳定路径身份。
///
/// Windows 上移除可安全转换的 `\\?\` 扩展前缀并把分隔符统一为 `/`；
/// 其它平台直接返回路径文本。
#[cfg(target_os = "windows")]
pub(super) fn stable_identity(path: &Path) -> String {
    let text = path.to_string_lossy();
    let text = text
        .strip_prefix(r"\\?\UNC\")
        .map(|unc| format!(r"\\{unc}"))
        .or_else(|| text.strip_prefix(r"\\?\").map(str::to_owned))
        .unwrap_or_else(|| text.into_owned());
    text.replace('\\', "/")
}

#[cfg(not(target_os = "windows"))]
pub(super) fn stable_identity(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
