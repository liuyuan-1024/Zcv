//! GPUI 剪贴板适配层。
//!
//! GPUI 的 `read_from_clipboard` / `write_to_clipboard` 是 `&gpui::App` 上的
//! 实例方法，但 `ClipboardPort` 实例会随 `App.clipboard` 字段长期持有，无法
//! 直接保存 `&gpui::App` 借用。这里用 thread-local 指针做桥梁：每次 GPUI
//! 回调在调 `App::dispatch_*` 前用 [`GpuiClipboardScope`] 把当前 `cx` 借出
//! 期间的指针存入 thread-local；派发期间 [`GpuiClipboard::read/write`]
//! 通过它访问系统剪贴板；scope 析构时恢复外层指针（空指针即「当前不在
//! GPUI 回调内」，端口读写无操作）。
//!
//! ## 安全性
//!
//! - thread-local 指针仅在 [`GpuiClipboardScope`] 生命期内非空，drop 时立即
//!   恢复外层值，不会留下悬空指针。
//! - GPUI 主线程模型保证 cx 借用语义；thread-local 隔离阻止跨线程误用。
//! - scope 的 `'a` 生命周期把指针有效区间绑死到 `cx` 借用的 stack frame
//!   存在期；scope 不能 escape 该帧。

use std::cell::Cell;
use std::marker::PhantomData;

use gpui::ClipboardItem;
use zom_command::ClipboardPort;

thread_local! {
    /// 当前命令派发借用的 `gpui::App` 指针。`null` 代表当前不在 GPUI 回调内。
    static CX_PTR: Cell<*const gpui::App> = const { Cell::new(std::ptr::null()) };
}

/// `'static` ClipboardPort 实现：从 thread-local 取当前 `gpui::App` 访问剪贴板。
pub(crate) struct GpuiClipboard;

impl ClipboardPort for GpuiClipboard {
    fn write(&mut self, text: &str) {
        CX_PTR.with(|cell| {
            let ptr = cell.get();
            if ptr.is_null() {
                // 不在 GPUI 回调内（如 headless 单测兜底）—— 静默丢弃。
                return;
            }
            // SAFETY: `GpuiClipboardScope::enter` 设置该指针后只在 scope 存活期间生效
            // （scope 持有 `&'a gpui::App` 凭据，析构时恢复外层指针）。
            // 因此指针非空 ⇒ cx 仍被借出，可安全解引。
            let cx = unsafe { &*ptr };
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        });
    }

    fn read(&self) -> Option<String> {
        CX_PTR.with(|cell| {
            let ptr = cell.get();
            if ptr.is_null() {
                return None;
            }
            // SAFETY: 同 [`write`]。
            let cx = unsafe { &*ptr };
            cx.read_from_clipboard().and_then(|item| item.text())
        })
    }
}

/// RAII：进入时把 `cx` 指针存入 thread-local，drop 时恢复外层指针。
///
/// 嵌套时外层 scope 的指针在外层 scope drop 之前一直被覆盖；内层 drop 后
/// 通过保存的 `previous` 恢复，避免内层 scope 提前清零外层指针。
pub(crate) struct GpuiClipboardScope<'a> {
    previous: *const gpui::App,
    _cx: PhantomData<&'a gpui::App>,
}

impl<'a> GpuiClipboardScope<'a> {
    pub(crate) fn enter(cx: &'a gpui::App) -> Self {
        let previous = CX_PTR.with(|cell| cell.replace(cx as *const _));
        Self {
            previous,
            _cx: PhantomData,
        }
    }
}

impl Drop for GpuiClipboardScope<'_> {
    fn drop(&mut self) {
        CX_PTR.with(|cell| cell.set(self.previous));
    }
}
