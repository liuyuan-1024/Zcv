use std::fs;

use super::*;

#[test]
fn reference_file_follows_builtin_content_without_changing_user_settings() {
    let directory = tempfile::tempdir().expect("应创建临时设置目录");
    let user = directory.path().join("settings.json");
    let reference = directory.path().join("settings.reference.jsonc");
    ensure_settings_file(&user, "{}\n").unwrap();
    update_reference_file(&reference, &INITIAL_USER_SETTINGS).unwrap();
    assert_eq!(
        fs::read_to_string(&reference).unwrap(),
        INITIAL_USER_SETTINGS.as_ref()
    );

    fs::write(&user, r#"{"tab_size": 8}"#).unwrap();
    update_reference_file(&reference, "{\"tab_size\": 2}\n").unwrap();
    assert_eq!(
        fs::read_to_string(&reference).unwrap(),
        "{\"tab_size\": 2}\n"
    );
    assert_eq!(fs::read_to_string(&user).unwrap(), r#"{"tab_size": 8}"#);
}
