use super::*;

use gpui::TestAppContext;

#[test]
fn installation_selects_current_platform_asset() {
    #[cfg(target_os = "macos")]
    assert_eq!(
        UpdateInstallation::from_process_path(Path::new("/Applications/Zcv.app"))
            .unwrap()
            .platform_key(),
        "macos-aarch64"
    );

    #[cfg(target_os = "windows")]
    assert_eq!(
        UpdateInstallation::from_process_path(Path::new(r"C:\Program Files\Zcv\Zcv.exe"))
            .unwrap()
            .platform_key(),
        match std::env::consts::ARCH {
            "x86_64" => "windows-x86_64",
            "aarch64" => "windows-aarch64",
            architecture => panic!("unexpected Windows architecture {architecture}"),
        }
    );
}

#[test]
fn declared_size_and_digest_are_both_required() {
    let content = b"zcv";
    let mut digest = Sha256::new();
    digest.update(content);
    let digest = format!("{:x}", digest.finalize());
    let asset = zcv_update::ReleaseAsset {
        url: "https://example.com/Zcv.zip".to_owned(),
        size: content.len() as u64,
        sha256: digest.clone(),
    };
    assert!(verify_downloaded(content.len() as u64, &digest, &asset).is_ok());
    assert!(verify_downloaded(1, &digest, &asset).is_err());
}

#[test]
fn each_update_failure_is_notified_only_once() {
    let idle = UpdateStatus::Idle;
    let failed = UpdateStatus::Failed {
        message: Arc::from("签名无效"),
    };

    assert_eq!(new_failure(&idle, &failed).as_deref(), Some("签名无效"));
    assert!(new_failure(&failed, &failed).is_none());
    assert!(new_failure(&failed, &idle).is_none());
}

#[test]
fn updated_result_notification_reports_rollback_only() {
    let rolled_back = UpdateResult {
        transaction_id: "42-1".to_owned(),
        from_version: "1.0.0".parse().unwrap(),
        to_version: "1.0.1".parse().unwrap(),
        status: UpdateResultStatus::RolledBack,
        error: Some("新版本未通过启动确认".to_owned()),
    };
    assert_eq!(
        update_result_notification(&rolled_back).as_deref(),
        Some("自动更新失败，已回滚到 1.0.0：新版本未通过启动确认")
    );

    let applied = UpdateResult {
        status: UpdateResultStatus::Applied,
        error: None,
        ..rolled_back
    };
    assert!(update_result_notification(&applied).is_none());
}

#[gpui::test]
fn startup_notification_is_taken_once(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let manager = cx.new(|cx| UpdateManager::new(None, Some(Arc::from("已回滚")), cx));
        assert!(
            manager
                .update(cx, |manager, _| manager.take_startup_notification())
                .is_some()
        );
        assert!(
            manager
                .update(cx, |manager, _| manager.take_startup_notification())
                .is_none()
        );
    });
}
