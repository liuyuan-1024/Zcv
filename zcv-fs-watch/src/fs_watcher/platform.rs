use std::path::Path;

use notify::RecursiveMode;

pub(super) fn native_recursive_mode() -> RecursiveMode {
    if cfg!(target_os = "macos") {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    }
}

pub(super) fn case_insensitive_paths() -> bool {
    cfg!(target_os = "macos")
}

pub(super) fn recursive_registration_covers(mode_is_poll: bool) -> bool {
    mode_is_poll || cfg!(target_os = "macos")
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
