use super::*;

// —— 平台无关：内置扩展名清单 ——

#[test]
fn document_extensions_asset_is_valid() {
    let extensions = document_extensions().unwrap();
    assert!(!extensions.is_empty());
    assert!(extensions.contains(&"md".to_owned()));
    for name in &extensions {
        assert!(
            name.bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()),
            "非法扩展名：{name}"
        );
    }
}

#[test]
fn invalid_extension_lines_are_rejected() {
    assert!(parse_document_extensions("").is_err());
    assert!(parse_document_extensions("\n\n").is_err());
    assert!(parse_document_extensions("md\nRS\n").is_err());
    assert!(parse_document_extensions("md\nc++\n").is_err());
    assert_eq!(
        parse_document_extensions("md\n\nrs\n").unwrap(),
        vec!["md".to_owned(), "rs".to_owned()]
    );
}

// —— Windows：注册表写入 ——

#[cfg(target_os = "windows")]
mod windows_registration {
    use std::path::Path;

    use windows_registry::CURRENT_USER;

    use crate::open_with::platform::windows::{RegistrationNames, write_registration};

    /// 测试专用命名空间，与生产注册完全隔离。
    const TEST_NAMES: RegistrationNames = RegistrationNames {
        prog_id: "Zcv.TestDocument",
        application_exe: "ZcvTest.exe",
        capabilities_key: "Software\\ZcvTest\\Capabilities",
        registered_name: "ZcvTest",
    };

    fn cleanup() {
        let _ = CURRENT_USER.remove_tree("Software\\Classes\\Zcv.TestDocument");
        let _ = CURRENT_USER.remove_tree("Software\\Classes\\Applications\\ZcvTest.exe");
        let _ = CURRENT_USER.remove_tree("Software\\Classes\\.zcvtestmd");
        let _ = CURRENT_USER.remove_tree("Software\\Classes\\.zcvtestrs");
        let _ = CURRENT_USER.remove_tree("Software\\ZcvTest");
        // RegisteredApplications 被系统和其他应用共用，只能移除自己的值。
        let _ = CURRENT_USER
            .open("Software\\RegisteredApplications")
            .and_then(|registered| registered.remove_value("ZcvTest"));
    }

    #[test]
    fn registration_writes_all_key_groups_and_refreshes_path() {
        cleanup();
        let extensions = vec!["zcvtestmd".to_owned(), "zcvtestrs".to_owned()];
        let old_location = Path::new(r"C:\解压位置\Zcv\Zcv.exe");
        write_registration(&TEST_NAMES, old_location, &extensions).unwrap();

        let prog_id = CURRENT_USER
            .open("Software\\Classes\\Zcv.TestDocument")
            .unwrap();
        assert_eq!(prog_id.get_string("").unwrap(), "Zcv 文档");
        assert_eq!(
            prog_id
                .open("shell\\open")
                .unwrap()
                .get_string("Icon")
                .unwrap(),
            format!("\"{}\"", old_location.display())
        );
        assert_eq!(
            CURRENT_USER
                .open("Software\\Classes\\Zcv.TestDocument\\shell\\open\\command")
                .unwrap()
                .get_string("")
                .unwrap(),
            format!("\"{}\" \"%1\"", old_location.display())
        );
        assert_eq!(
            CURRENT_USER
                .open("Software\\Classes\\Applications\\ZcvTest.exe\\shell\\open\\command")
                .unwrap()
                .get_string("")
                .unwrap(),
            format!("\"{}\" \"%1\"", old_location.display())
        );

        let capabilities = CURRENT_USER
            .open("Software\\ZcvTest\\Capabilities")
            .unwrap();
        assert_eq!(
            capabilities.get_string("ApplicationName").unwrap(),
            "ZcvTest"
        );
        assert_eq!(
            CURRENT_USER
                .open("Software\\ZcvTest\\Capabilities\\FileAssociations")
                .unwrap()
                .get_string(".zcvtestmd")
                .unwrap(),
            "Zcv.TestDocument"
        );
        assert_eq!(
            CURRENT_USER
                .open("Software\\Classes\\.zcvtestmd\\OpenWithProgids")
                .unwrap()
                .get_string("Zcv.TestDocument")
                .unwrap(),
            ""
        );
        assert_eq!(
            CURRENT_USER
                .open("Software\\RegisteredApplications")
                .unwrap()
                .get_string("ZcvTest")
                .unwrap(),
            "Software\\ZcvTest\\Capabilities"
        );

        // 移动安装目录后的再次启动：命令与图标指向新位置，注册保持一致。
        let new_location = Path::new(r"D:\新位置\release\Zcv.exe");
        write_registration(&TEST_NAMES, new_location, &extensions).unwrap();
        assert_eq!(
            CURRENT_USER
                .open("Software\\Classes\\Zcv.TestDocument\\shell\\open\\command")
                .unwrap()
                .get_string("")
                .unwrap(),
            format!("\"{}\" \"%1\"", new_location.display())
        );
        assert_eq!(
            CURRENT_USER
                .open("Software\\Classes\\Applications\\ZcvTest.exe\\shell\\open\\command")
                .unwrap()
                .get_string("")
                .unwrap(),
            format!("\"{}\" \"%1\"", new_location.display())
        );
        cleanup();
    }
}
