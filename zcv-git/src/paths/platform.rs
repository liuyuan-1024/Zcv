use std::path::PathBuf;

#[cfg(unix)]
pub(super) fn path_from_git_bytes(bytes: &[u8]) -> PathBuf {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;

    PathBuf::from(OsStr::from_bytes(bytes))
}

#[cfg(not(unix))]
pub(super) fn path_from_git_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}
