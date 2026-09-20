use super::GitOperationKind;

#[test]
fn remote_operation_names_distinguish_sync_pull_and_push() {
    assert_eq!(GitOperationKind::Fetch.display_name(), "同步");
    assert_eq!(GitOperationKind::Pull.display_name(), "合并拉取");
    assert_eq!(GitOperationKind::Push.display_name(), "推送");
}
