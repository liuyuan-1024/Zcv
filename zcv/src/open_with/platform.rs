//! 打开方式注册的平台后端；按平台路由到各自实现。

#[cfg(target_os = "windows")]
pub(super) mod windows;

#[cfg(target_os = "windows")]
pub(super) fn ensure_open_with_registration() {
    windows::ensure_open_with_registration();
}

#[cfg(not(target_os = "windows"))]
pub(super) fn ensure_open_with_registration() {
    // macOS 由 LaunchServices 在应用首次出现时自动完成注册，无需应用写入；
    // 其余平台当前没有打包分发形态。
}
