use std::path::Path;

use zcv_fs_watch::{PathEvent, PathEventKind};

use super::*;

fn event(path: &Path, kind: PathEventKind) -> PathEvent {
    PathEvent::new(path.to_path_buf(), Some(kind)).expect("测试路径应为绝对路径")
}

/// 配置目录同时承载窗口尺寸、工作区布局与更新暂存文件；
/// 只有设置文件本身的变化或配置目录 Rescan 才应触发重载。
#[test]
fn only_settings_file_events_trigger_reload() {
    let base = std::env::current_dir().expect("测试应能取得当前目录");
    let config = base.join(".zcv");
    let settings = config.join("settings.json");

    assert!(is_settings_event(
        &event(&settings, PathEventKind::Changed),
        &settings,
        &config
    ));
    assert!(is_settings_event(
        &event(&config, PathEventKind::Rescan),
        &settings,
        &config
    ));
    assert!(!is_settings_event(
        &event(&config.join("window_bounds.json"), PathEventKind::Changed),
        &settings,
        &config
    ));
    assert!(!is_settings_event(
        &event(
            &config.join("workspaces").join("project.json"),
            PathEventKind::Changed
        ),
        &settings,
        &config
    ));
    // 配置目录上的普通 Changed 不是 Rescan，不应触发全量重读。
    assert!(!is_settings_event(
        &event(&config, PathEventKind::Changed),
        &settings,
        &config
    ));
}
