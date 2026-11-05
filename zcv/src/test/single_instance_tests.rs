use super::*;

use crate::single_instance::enforce_single_instance;

#[test]
fn single_instance_only_applies_to_release_without_stateless_override() {
    assert!(
        !enforce_single_instance(true, false),
        "开发构建应跳过单实例，允许旧实例与新二进制并存"
    );
    assert!(
        !enforce_single_instance(true, true),
        "开发构建 + ZCV_STATELESS 仍应跳过单实例"
    );
    assert!(
        !enforce_single_instance(false, true),
        "ZCV_STATELESS 应在 release 构建中显式绕过单实例"
    );
    assert!(
        enforce_single_instance(false, false),
        "正式构建默认必须启用单实例转交"
    );
}

#[test]
fn file_request_preserves_windows_path() {
    let request = InitialOpen::File(vec![
        PathBuf::from(r"C:\项目\页面.html"),
        PathBuf::from(r"D:\资料\说明.md"),
    ]);
    let bytes = encode_request(&request).unwrap();
    let units: Vec<u16> = bytes[5..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    assert_eq!(decode_request(bytes[0], &units).unwrap(), request);
}

#[test]
fn project_request_preserves_multiple_roots() {
    let request = InitialOpen::Project(vec![
        PathBuf::from(r"C:\项目一"),
        PathBuf::from(r"D:\项目二"),
    ]);
    let bytes = encode_request(&request).unwrap();
    let units: Vec<u16> = bytes[5..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    assert_eq!(decode_request(bytes[0], &units).unwrap(), request);
}

#[test]
fn invalid_request_rejects_empty_or_unterminated_paths() {
    assert!(decode_request(1, &[]).is_err());
    assert!(decode_request(1, &[0]).is_err());
    assert!(decode_request(2, &[b'C' as u16]).is_err());
}

#[test]
fn empty_request_activates_existing_instance() {
    let bytes = encode_request(&InitialOpen::Empty).unwrap();
    assert_eq!(bytes, [0, 0, 0, 0, 0]);
    assert_eq!(decode_request(0, &[]).unwrap(), InitialOpen::Empty);
}
