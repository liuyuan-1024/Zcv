//! 启动请求转发的平台路由。

#[cfg(target_os = "windows")]
pub(super) mod windows;

#[cfg(target_os = "windows")]
pub(crate) use windows::start;
