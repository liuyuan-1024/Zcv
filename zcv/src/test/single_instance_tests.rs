use super::*;

#[test]
fn file_request_preserves_windows_path() {
    let request = InitialOpen::File(PathBuf::from(r"C:\项目\页面.html"));
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
fn empty_request_activates_existing_instance() {
    let bytes = encode_request(&InitialOpen::Empty).unwrap();
    assert_eq!(bytes, [0, 0, 0, 0, 0]);
    assert_eq!(decode_request(0, &[]).unwrap(), InitialOpen::Empty);
}
