use super::*;
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair as _};

fn manifest(version: &str) -> ReleaseManifest {
    ReleaseManifest {
        schema_version: MANIFEST_SCHEMA_VERSION,
        channel: STABLE_CHANNEL.to_owned(),
        version: version.parse().unwrap(),
        published_at: "2026-08-29T00:00:00Z".to_owned(),
        assets: BTreeMap::from([(
            "macos-aarch64".to_owned(),
            ReleaseAsset {
                url: "https://github.com/liuyuan-1024/Zcv/releases/download/v1/Zcv.zip".to_owned(),
                size: 10,
                sha256: "00".repeat(32),
            },
        )]),
    }
}

#[test]
fn signed_manifest_is_verified_before_parsing() {
    let key = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
    let key_pair = Ed25519KeyPair::from_pkcs8(key.as_ref()).unwrap();
    let bytes = serde_json::to_vec(&manifest("1.0.1")).unwrap();
    let signature =
        base64::engine::general_purpose::STANDARD.encode(key_pair.sign(&bytes).as_ref());
    let public_key =
        base64::engine::general_purpose::STANDARD.encode(key_pair.public_key().as_ref());

    assert_eq!(
        verify_and_parse_manifest(&bytes, signature.as_bytes(), &public_key).unwrap(),
        manifest("1.0.1")
    );

    let mut tampered = bytes;
    tampered.push(b' ');
    assert!(verify_and_parse_manifest(&tampered, signature.as_bytes(), &public_key).is_err());
}

#[test]
fn only_newer_matching_platform_release_is_selected() {
    let release = manifest("1.1.0")
        .select_newer_release(&"1.0.0".parse().unwrap(), "macos-aarch64")
        .unwrap()
        .unwrap();
    assert_eq!(release.version, "1.1.0".parse::<Version>().unwrap());
    assert!(
        manifest("1.0.0")
            .select_newer_release(&"1.0.0".parse().unwrap(), "macos-aarch64")
            .unwrap()
            .is_none()
    );
    assert!(
        manifest("1.1.0")
            .select_newer_release(&"1.0.0".parse().unwrap(), "macos-x86_64")
            .is_err()
    );
}

#[test]
fn archive_paths_must_stay_inside_expected_bundle() {
    assert!(validate_archive_entry_path("Zcv.app/Contents/MacOS/Zcv").is_ok());
    assert!(validate_archive_entry_path("__MACOSX/._Zcv.app").is_ok());
    assert!(validate_archive_entry_path("__MACOSX/Zcv.app/._Zcv").is_ok());
    assert!(validate_archive_entry_path("../Zcv.app").is_err());
    assert!(validate_archive_entry_path("/Applications/Zcv.app").is_err());
    assert!(validate_archive_entry_path("other/Zcv.app").is_err());
    assert!(validate_archive_entry_path("Zcv.app\\..\\evil").is_err());
}

#[test]
fn transaction_requires_an_upgrade_and_exact_bundle_path() {
    let result = tempfile::tempdir().unwrap();
    assert!(
        UpdateTransaction::new(
            "1.0.0".parse().unwrap(),
            "1.0.0".parse().unwrap(),
            PathBuf::from("/Applications/Zcv.app"),
            PathBuf::from("/tmp/Zcv.app"),
            result.path().join("result.json"),
        )
        .is_err()
    );
}

#[test]
fn translocated_paths_are_detected_by_component() {
    assert!(is_translocated_path(Path::new(
        "/private/var/folders/ab/cd/T/AppTranslocation/uuid/d/Zcv.app"
    )));
    assert!(!is_translocated_path(Path::new("/Applications/Zcv.app")));
    assert!(!is_translocated_path(Path::new("/tmp/Zcv.app")));
}
