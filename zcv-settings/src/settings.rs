//! 设置系统：用户设置文件的解析、合并与变更监听。
//! 此文件是 `zcv-settings` crate 的公共入口。
//!
//! 用户设置 JSON 经 fs watcher 监听，变更防抖重载后写入 SettingsStore global；
//! 默认值与各领域设置由本模块统一提供；具体的运行时类型转换由消费方完成。

use std::borrow::Cow;
use std::path::Path;
use std::sync::LazyLock;

mod file;
mod merge;
mod paths;
mod reload;
mod schema;
mod store;

pub use file::ensure_user_settings_file;
pub use merge::UserSettings;
pub use reload::init;
pub use schema::{SoftWrapMode, TabConfig, TabOverride};
pub use store::{GlobalSettingsErrorReporter, SettingsError, SettingsErrorReporter, SettingsStore};

/// 配置目录（用户主目录下的 `.zcv`）解析入口。
pub fn config_dir() -> &'static Path {
    paths::config_dir()
}

pub(crate) static INITIAL_USER_SETTINGS: LazyLock<Cow<'static, str>> = LazyLock::new(|| {
    zcv_assets::text("settings/initial_user_settings.json").expect("内置初始设置应存在")
});

#[cfg(test)]
#[path = "test/settings_tests.rs"]
mod tests;
