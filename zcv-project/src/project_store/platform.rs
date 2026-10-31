use std::path::Path;

use anyhow::{Context as _, Result};

/// 将文件或目录交给系统回收站。调用方只表达「移到回收站」，
/// 是否真正回收、回收站已满时是否改为永久删除，均由系统（Shell）自行决定并提示用户。
#[cfg(windows)]
pub(super) fn recycle(path: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt as _;

    use windows_sys::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_WANTNUKEWARNING, SHFILEOPSTRUCTW, SHFileOperationW,
    };

    // `pFrom` 是单个 `NUL` 结尾的路径列表，整体再以一个额外 `NUL` 结束。
    let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
    from.extend_from_slice(&[0, 0]);

    // 不设置 FOF_NO_UI / FOF_NOCONFIRMATION：保留系统自身的进度与确认交互，
    // 由 Shell 决定回收还是（条目过大时）在用户确认后永久删除。
    let mut op = SHFILEOPSTRUCTW {
        hwnd: std::ptr::null_mut(),
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        pTo: std::ptr::null(),
        fFlags: (FOF_ALLOWUNDO | FOF_WANTNUKEWARNING) as u16,
        fAnyOperationsAborted: 0,
        hNameMappings: std::ptr::null_mut(),
        lpszProgressTitle: std::ptr::null(),
    };

    let code = unsafe { SHFileOperationW(&mut op) };
    if code != 0 {
        anyhow::bail!("回收站操作失败（错误码 {code}）：{}", path.display());
    }
    if op.fAnyOperationsAborted != 0 {
        anyhow::bail!("回收站操作已取消：{}", path.display());
    }
    Ok(())
}

#[cfg(not(windows))]
pub(super) fn recycle(path: &Path) -> Result<()> {
    trash::delete(path).map_err(|error| anyhow::anyhow!("{error}"))
}

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
