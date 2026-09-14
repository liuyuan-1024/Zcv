use std::path::Path;

#[cfg(unix)]
use anyhow::Context as _;
use anyhow::Result;
use notify::RecursiveMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FileSystemSemantics {
    pub(super) case_sensitive: bool,
    pub(super) requires_poll_watcher: bool,
}

pub(super) fn path_semantics(path: &Path) -> Result<FileSystemSemantics> {
    Ok(FileSystemSemantics {
        case_sensitive: path_case_sensitive(path)?,
        requires_poll_watcher: requires_poll_watcher(path),
    })
}

pub(super) fn native_recursive_mode() -> RecursiveMode {
    if native_watcher_is_recursive() {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    }
}

pub(super) fn recursive_registration_covers(mode_is_poll: bool) -> bool {
    mode_is_poll || native_watcher_is_recursive()
}

fn native_watcher_is_recursive() -> bool {
    cfg!(any(target_os = "macos", target_os = "windows"))
}

pub(super) fn requires_poll_watcher(path: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        return requires_poll_linux(path);
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        false
    }
}

#[cfg(target_os = "macos")]
fn path_case_sensitive(path: &Path) -> Result<bool> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let c_path = CString::new(path.as_os_str().as_bytes())
        .with_context(|| format!("文件系统路径包含无效字节：{}", path.display()))?;
    let result = unsafe { libc::pathconf(c_path.as_ptr(), libc::_PC_CASE_SENSITIVE) };
    match result {
        0 => Ok(false),
        1 => Ok(true),
        value => Err(anyhow::anyhow!(
            "无法查询路径大小写语义：{}（pathconf 返回 {value}，系统错误：{}）",
            path.display(),
            std::io::Error::last_os_error()
        )),
    }
}

#[cfg(target_os = "windows")]
fn path_case_sensitive(path: &Path) -> Result<bool> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_CASE_SENSITIVE_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileCaseSensitiveInfo,
        GetFileInformationByHandleEx, OPEN_EXISTING,
    };

    const FILE_CS_FLAG_CASE_SENSITIVE_DIR: u32 = 0x0000_0001;

    let wide_path = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let handle = unsafe {
        CreateFileW(
            wide_path.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        anyhow::bail!(
            "无法打开路径以查询大小写语义：{}：{}",
            path.display(),
            std::io::Error::last_os_error()
        );
    }

    let mut info = FILE_CASE_SENSITIVE_INFO::default();
    let result = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileCaseSensitiveInfo,
            (&mut info as *mut FILE_CASE_SENSITIVE_INFO).cast(),
            std::mem::size_of::<FILE_CASE_SENSITIVE_INFO>() as u32,
        )
    };
    let error = if result == 0 {
        Some(std::io::Error::last_os_error())
    } else {
        None
    };
    unsafe { CloseHandle(handle) };

    if let Some(error) = error {
        return Err(anyhow::anyhow!(
            "无法查询路径大小写语义：{}：{}",
            path.display(),
            error
        ));
    }

    Ok(info.Flags & FILE_CS_FLAG_CASE_SENSITIVE_DIR != 0)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn path_case_sensitive(path: &Path) -> Result<bool> {
    use std::ffi::OsString;
    use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
    use std::os::unix::fs::MetadataExt as _;
    use std::path::PathBuf;

    let existing_path = std::fs::canonicalize(path)
        .with_context(|| format!("无法规范化文件系统路径：{}", path.display()))?;
    let mut current = existing_path.clone();

    for component in existing_path.components().rev() {
        let Some(alternate_name) = alternate_case(component.as_os_str()) else {
            let Some(parent) = current.parent() else {
                break;
            };
            current = parent.to_path_buf();
            continue;
        };
        let parent = current.parent().context("无法确定文件系统路径的父目录")?;
        let alternate_path = parent.join(alternate_name);
        let current_metadata = std::fs::symlink_metadata(&current)
            .with_context(|| format!("无法读取路径元数据：{}", current.display()))?;

        match std::fs::symlink_metadata(&alternate_path) {
            Ok(alternate_metadata) => {
                return Ok(current_metadata.dev() != alternate_metadata.dev()
                    || current_metadata.ino() != alternate_metadata.ino());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("无法读取大小写探测路径：{}", alternate_path.display())
                });
            }
        }
    }

    anyhow::bail!(
        "无法探测路径大小写语义：{}（路径没有可用于无副作用探测的字母组件）",
        path.display()
    );

    fn alternate_case(component: &std::ffi::OsStr) -> Option<OsString> {
        let mut bytes = component.as_bytes().to_vec();
        for byte in &mut bytes {
            if byte.is_ascii_lowercase() {
                *byte = byte.to_ascii_uppercase();
                return Some(OsString::from_vec(bytes));
            }
            if byte.is_ascii_uppercase() {
                *byte = byte.to_ascii_lowercase();
                return Some(OsString::from_vec(bytes));
            }
        }
        None
    }
}

#[cfg(not(any(unix, target_os = "windows")))]
fn path_case_sensitive(path: &Path) -> Result<bool> {
    anyhow::bail!("当前平台无法查询路径大小写语义：{}", path.display())
}

#[cfg(target_os = "linux")]
fn requires_poll_linux(path: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c_path.as_ptr(), &mut stat) } != 0 {
        return false;
    }

    const V9FS_MAGIC: u64 = 0x0102_1997;
    const NFS_SUPER_MAGIC: u64 = 0x0000_6969;
    const CIFS_MAGIC: u64 = 0xFF53_4D42;
    const SMB_SUPER_MAGIC: u64 = 0x0000_517B;
    const SMB2_MAGIC: u64 = 0xFE53_4D42;
    const FUSE_SUPER_MAGIC: u64 = 0x6573_5546;

    matches!(
        stat.f_type as u64,
        V9FS_MAGIC | NFS_SUPER_MAGIC | CIFS_MAGIC | SMB_SUPER_MAGIC | SMB2_MAGIC | FUSE_SUPER_MAGIC
    )
}
