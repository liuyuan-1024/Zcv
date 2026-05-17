//! 开发态应用图标接入。
//!
//! GPUI 0.2.2 还没有直接暴露 app icon 配置。这里把平台差异收在
//! `shell::platform` 内，保证上层只需要设置统一的 `app_id` 并在窗口创建后
//! 尝试安装原生图标。

use gpui::Window;

pub(crate) const APP_ID: &str = "zom";

pub(crate) fn prepare_development_app_icon() {
    #[cfg(all(target_os = "linux", debug_assertions))]
    linux::install_icon_theme_files();
}

pub(crate) fn apply_window_icon(_window: &mut Window) {
    #[cfg(target_os = "macos")]
    macos::apply_application_icon();

    #[cfg(target_os = "windows")]
    windows::apply_window_icon(_window);

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = _window;
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use cocoa::{
        appkit::{NSApp, NSApplication, NSImage},
        base::{id, nil},
        foundation::{NSAutoreleasePool, NSString},
    };
    use std::path::Path;

    const ICON_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/app/zom.svg");

    pub(super) fn apply_application_icon() {
        if !Path::new(ICON_PATH).exists() {
            return;
        }

        unsafe {
            let _pool = NSAutoreleasePool::new(nil);
            let path = NSString::alloc(nil).init_str(ICON_PATH);
            let image = NSImage::alloc(nil).initWithContentsOfFile_(path);

            if image != nil {
                let app: id = NSApp();
                app.setApplicationIconImage_(image);
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use ::windows::Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        UI::WindowsAndMessaging::{CreateIcon, ICON_BIG, ICON_SMALL, SendMessageW, WM_SETICON},
    };
    use gpui::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use resvg::{tiny_skia, usvg};

    const ICON_SVG: &[u8] = include_bytes!("../../../assets/icons/app/zom.svg");

    pub(super) fn apply_window_icon(window: &mut Window) {
        let Ok(handle) = window.window_handle() else {
            return;
        };

        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return;
        };

        let hwnd = HWND(handle.hwnd.get());

        unsafe {
            if let Some(icon) = render_svg_icon(32) {
                SendMessageW(
                    hwnd,
                    WM_SETICON,
                    WPARAM(ICON_SMALL as usize),
                    LPARAM(icon.0),
                );
            }

            if let Some(icon) = render_svg_icon(256) {
                SendMessageW(hwnd, WM_SETICON, WPARAM(ICON_BIG as usize), LPARAM(icon.0));
            }
        }
    }

    fn render_svg_icon(size: u32) -> Option<::windows::Win32::UI::WindowsAndMessaging::HICON> {
        let options = usvg::Options::default();
        let tree = usvg::Tree::from_data(ICON_SVG, &options).ok()?;
        let source_size = tree.size();
        let scale_x = size as f32 / source_size.width();
        let scale_y = size as f32 / source_size.height();
        let transform = tiny_skia::Transform::from_scale(scale_x, scale_y);
        let mut pixmap = tiny_skia::Pixmap::new(size, size)?;

        resvg::render(&tree, transform, &mut pixmap.as_mut());

        let mut bgra = pixmap.data().to_vec();

        for pixel in bgra.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }

        let mask_stride = size.div_ceil(32) * 4;
        let mask = vec![0u8; (mask_stride * size) as usize];

        unsafe {
            CreateIcon(
                None,
                size as i32,
                size as i32,
                1,
                32,
                mask.as_ptr(),
                bgra.as_ptr(),
            )
            .ok()
        }
    }
}

#[cfg(all(target_os = "linux", debug_assertions))]
mod linux {
    use std::{
        env, fs,
        path::{Path, PathBuf},
    };

    pub(super) fn install_icon_theme_files() {
        let Some(data_home) = data_home() else {
            return;
        };

        install_svg_icon(&data_home);
        install_desktop_file(&data_home);
    }

    fn install_svg_icon(data_home: &Path) {
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/icons/app/zom.svg");
        let target = data_home.join("icons/hicolor/scalable/apps/zom.svg");

        if let Some(parent) = target.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::copy(source, target);
    }

    fn install_desktop_file(data_home: &Path) {
        let target = data_home.join("applications/zom.desktop");
        if let Some(parent) = target.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let exec = env::current_exe()
            .ok()
            .and_then(|path| path.into_os_string().into_string().ok())
            .unwrap_or_else(|| "zom-desktop".to_string());
        let desktop_file = format!(
            "[Desktop Entry]\nType=Application\nName=zom\nExec={exec}\nIcon=zom\nStartupWMClass=zom\nCategories=Development;TextEditor;\n"
        );

        let _ = fs::write(target, desktop_file);
    }

    fn data_home() -> Option<PathBuf> {
        if let Some(path) = env::var_os("XDG_DATA_HOME") {
            return Some(PathBuf::from(path));
        }

        env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
    }
}
