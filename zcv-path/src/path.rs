//! 跨平台路径语义。
//!
//! 原生绝对路径只用于操作系统边界；
//! 项目内部的相对路径使用 `/` 保存，需要传给 Git 或持久化时不再依赖当前平台的 `PathBuf` 字符串格式。

use std::{
    fmt, io,
    ops::Deref,
    path::{Path, PathBuf},
};

mod platform;

/// 当前路径字符串使用的语法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathStyle {
    Unix,
    Windows,
}

impl PathStyle {
    pub(crate) const fn local() -> Self {
        platform::local_style()
    }
}

/// 已确认是绝对路径的本地路径。
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AbsolutePathBuf(PathBuf);

impl AbsolutePathBuf {
    pub fn new(path: PathBuf) -> io::Result<Self> {
        if path.is_absolute() {
            Ok(Self(path))
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("路径不是绝对路径：{}", path.display()),
            ))
        }
    }

    /// 解析磁盘上的路径，并移除普通 Windows 路径不需要的 `\\?\` 前缀。
    ///
    /// 原生路径仍由文件系统后端保留；
    /// 这里的结果用于工作区、Git 和持久化等需要稳定路径身份的边界。
    pub fn canonicalize(path: &Path) -> io::Result<Self> {
        Self::new(dunce::canonicalize(path)?)
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }

    pub fn join_relative(&self, relative: &RelativePathBuf) -> Self {
        Self(self.0.join(relative.as_path()))
    }

    pub fn relative_path(&self, path: &Path) -> Option<RelativePathBuf> {
        let relative = path.strip_prefix(&self.0).ok()?;
        RelativePathBuf::from_path(relative).ok()
    }
}

/// 将可以安全转换的 Windows 扩展路径转换为普通路径。
///
/// 长路径、保留设备名和无法安全表达的 UNC 路径保持原样；
/// 这不是无条件删除 `\\?\` 前缀，因此不会破坏只能通过扩展路径访问的文件。
pub fn simplify_native(path: &Path) -> PathBuf {
    dunce::simplified(path).to_path_buf()
}

/// 规范化用于比较和索引的路径，即使最终文件尚不存在也保留规范化的父路径。
///
/// 这用于创建目标、删除事件和重命名目标等场景：
/// 最终路径可能尚不存在，但已有祖先仍然可以提供稳定的绝对路径身份。
pub fn normalize_for_comparison(path: &Path) -> io::Result<AbsolutePathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut missing_components = Vec::new();
    let mut existing = path.as_path();
    while !existing.exists() {
        let name = existing.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("无法定位路径：{}", path.display()),
            )
        })?;
        missing_components.push(name.to_os_string());
        existing = existing.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("无法定位路径：{}", path.display()),
            )
        })?;
    }

    let mut normalized = AbsolutePathBuf::canonicalize(existing)?.into_path_buf();
    for component in missing_components.into_iter().rev() {
        normalized.push(component);
    }
    AbsolutePathBuf::new(simplify_native(&normalized))
}

impl AsRef<Path> for AbsolutePathBuf {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl Deref for AbsolutePathBuf {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        self.as_path()
    }
}

impl From<AbsolutePathBuf> for PathBuf {
    fn from(path: AbsolutePathBuf) -> Self {
        path.into_path_buf()
    }
}

impl fmt::Display for AbsolutePathBuf {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.display().fmt(formatter)
    }
}

/// 已规范化的项目相对路径，内部始终使用 `/` 分隔符。
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RelativePathBuf(String);

impl RelativePathBuf {
    pub fn empty() -> Self {
        Self(String::new())
    }

    pub fn from_path(path: &Path) -> io::Result<Self> {
        Self::from_path_with_style(path, PathStyle::local())
    }

    pub(crate) fn from_path_with_style(path: &Path, style: PathStyle) -> io::Result<Self> {
        let text = path
            .to_str()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "路径不是有效的 UTF-8"))?;
        Self::from_str_with_style(text, style)
    }

    fn from_str_with_style(text: &str, style: PathStyle) -> io::Result<Self> {
        let separators: &[char] = match style {
            PathStyle::Unix => &['/'],
            PathStyle::Windows => &['/', '\\'],
        };

        let absolute = match style {
            PathStyle::Unix => text.starts_with('/'),
            PathStyle::Windows => {
                text.starts_with('/')
                    || text.starts_with('\\')
                    || (text.as_bytes().get(1) == Some(&b':')
                        && text
                            .as_bytes()
                            .get(2)
                            .is_some_and(|byte| *byte == b'/' || *byte == b'\\'))
            }
        };
        if absolute {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("相对路径不能是绝对路径：{text}"),
            ));
        }

        let mut components = Vec::new();
        for component in text.split(separators) {
            match component {
                "" | "." => {}
                ".." => {
                    if components.pop().is_none() {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!("相对路径越过根目录：{text}"),
                        ));
                    }
                }
                component => components.push(component),
            }
        }
        Ok(Self(components.join("/")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn join(&self, other: &Self) -> Self {
        if self.is_empty() {
            other.clone()
        } else if other.is_empty() {
            self.clone()
        } else {
            Self(format!("{}/{}", self.0, other.0))
        }
    }

    pub fn into_path_buf(self) -> PathBuf {
        PathBuf::from(self.0)
    }
}

impl AsRef<Path> for RelativePathBuf {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl fmt::Display for RelativePathBuf {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// 用于缓存键和工作区持久化的稳定路径身份。
///
/// 调用方应在路径进入工作区时先完成磁盘 canonicalize；
/// 此类型只负责把已确定的绝对路径转换为跨平台稳定的字符串，不改变文件系统语义。
pub fn stable_identity(path: &Path) -> String {
    platform::stable_identity(path)
}

#[cfg(test)]
#[path = "test/path_tests.rs"]
mod tests;
