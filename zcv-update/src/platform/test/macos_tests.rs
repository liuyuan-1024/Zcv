use super::*;

#[test]
fn download_metadata_is_stripped_recursively() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let root = tempfile::tempdir().unwrap();
    let nested = root.path().join("nested");
    std::fs::create_dir(&nested).unwrap();
    let file = nested.join("file.txt");
    std::fs::write(&file, "content").unwrap();

    for path in [root.path(), &nested, &file] {
        let c_path = CString::new(path.as_os_str().as_bytes()).unwrap();
        let c_attr = CString::new("com.apple.quarantine").unwrap();
        let value = b"0083;5f0b2f00;Safari;";
        let result = unsafe {
            libc::setxattr(
                c_path.as_ptr(),
                c_attr.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
                0,
            )
        };
        assert_eq!(result, 0, "无法为 {} 设置测试属性", path.display());
    }

    strip_download_metadata(root.path()).unwrap();

    for path in [root.path(), &nested, &file] {
        let c_path = CString::new(path.as_os_str().as_bytes()).unwrap();
        let c_attr = CString::new("com.apple.quarantine").unwrap();
        let result = unsafe {
            libc::getxattr(
                c_path.as_ptr(),
                c_attr.as_ptr(),
                std::ptr::null_mut(),
                0,
                0,
                0,
            )
        };
        assert_eq!(result, -1, "属性应已被移除: {}", path.display());
    }
}

#[test]
fn atomic_swap_exchanges_complete_directories() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first");
    let second = root.path().join("second");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    std::fs::write(first.join("version"), "old").unwrap();
    std::fs::write(second.join("version"), "new").unwrap();

    atomic_swap(&first, &second).unwrap();

    assert_eq!(
        std::fs::read_to_string(first.join("version")).unwrap(),
        "new"
    );
    assert_eq!(
        std::fs::read_to_string(second.join("version")).unwrap(),
        "old"
    );
}
