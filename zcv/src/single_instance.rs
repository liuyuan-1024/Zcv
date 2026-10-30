//! 后续启动请求交给当前运行的应用进程。

mod platform;

#[cfg(target_os = "windows")]
pub(crate) use platform::start;

/// 是否启用单实例转交。
///
/// 对齐 Zed：开发构建（Dev 频道）不强制单实例，让正在开发的新二进制能与稳定实例并存；
/// 另提供 `ZCV_STATELESS` 作为 release 构建下的显式绕过开关。
/// 单实例只在正式发布构建且未设置该环境变量时生效。
pub(crate) fn enforce_single_instance(debug_build: bool, stateless: bool) -> bool {
    !debug_build && !stateless
}
