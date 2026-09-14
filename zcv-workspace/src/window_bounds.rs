//! 窗口边界持久化：保存与恢复窗口的尺寸与位置。
//!
//! 项目身份与布局状态共用 persistence 的哈希键控。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use gpui::{App, Bounds, DisplayId, Pixels, Window, WindowBounds, point, px, size};
use serde::{Deserialize, Serialize};
use zcv_settings::config_dir;

use crate::persistence;

const WINDOW_BOUNDS_VERSION: u32 = 2;

/// 窗口边界持久化使用的坐标空间。
///
/// GPUI 的 `Pixels` 是 Zcv 窗口恢复协议的唯一坐标输入；
/// 设备像素和标题栏偏移只由 GPUI 的原生窗口后端处理，不进入工作区文件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum WindowCoordinateSpace {
    #[serde(rename = "gpui-logical-pixels")]
    GpuiLogicalPixels,
}

/// 窗口边界的可持久化形式：GPUI 逻辑像素的整数坐标。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) enum WindowBoundsJson {
    Windowed {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
    Maximized {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
    Fullscreen {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
}

impl From<WindowBounds> for WindowBoundsJson {
    fn from(bounds: WindowBounds) -> Self {
        // 像素浮点值就近取整，避免小数抖动导致每次保存都重写文件。
        let rounded = |bounds: Bounds<Pixels>| {
            (
                f32::from(bounds.origin.x).round() as i32,
                f32::from(bounds.origin.y).round() as i32,
                f32::from(bounds.size.width).round() as i32,
                f32::from(bounds.size.height).round() as i32,
            )
        };
        match bounds {
            WindowBounds::Windowed(bounds) => {
                let (x, y, width, height) = rounded(bounds);
                WindowBoundsJson::Windowed {
                    x,
                    y,
                    width,
                    height,
                }
            }
            WindowBounds::Maximized(bounds) => {
                let (x, y, width, height) = rounded(bounds);
                WindowBoundsJson::Maximized {
                    x,
                    y,
                    width,
                    height,
                }
            }
            WindowBounds::Fullscreen(bounds) => {
                let (x, y, width, height) = rounded(bounds);
                WindowBoundsJson::Fullscreen {
                    x,
                    y,
                    width,
                    height,
                }
            }
        }
    }
}

impl From<WindowBoundsJson> for WindowBounds {
    fn from(json: WindowBoundsJson) -> Self {
        let bounds = |x: i32, y: i32, width: i32, height: i32| Bounds {
            origin: point(px(x as f32), px(y as f32)),
            size: size(px(width as f32), px(height as f32)),
        };
        match json {
            WindowBoundsJson::Windowed {
                x,
                y,
                width,
                height,
            } => WindowBounds::Windowed(bounds(x, y, width, height)),
            WindowBoundsJson::Maximized {
                x,
                y,
                width,
                height,
            } => WindowBounds::Maximized(bounds(x, y, width, height)),
            WindowBoundsJson::Fullscreen {
                x,
                y,
                width,
                height,
            } => WindowBounds::Fullscreen(bounds(x, y, width, height)),
        }
    }
}

/// 单条窗口记录：边界 + 保存时的显示器标识（显示器断开后回退主显示器）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct StoredBounds {
    coordinate_space: WindowCoordinateSpace,
    #[serde(default)]
    display_uuid: Option<String>,
    bounds: WindowBoundsJson,
}

/// 单文件存储：default 为全局默认，projects 按项目身份分键。
#[derive(Debug, Serialize, Deserialize)]
struct WindowBoundsFile {
    version: u32,
    #[serde(default)]
    default: Option<StoredBounds>,
    #[serde(default)]
    projects: BTreeMap<String, StoredBounds>,
}

impl Default for WindowBoundsFile {
    fn default() -> Self {
        Self {
            version: WINDOW_BOUNDS_VERSION,
            default: None,
            projects: BTreeMap::new(),
        }
    }
}

fn window_bounds_path() -> PathBuf {
    config_dir().join("window_bounds.json")
}

/// 读取窗口边界文件；缺失或损坏按空文件处理（与最近项目列表的降级约定一致）。
fn read_file(path: &Path) -> WindowBoundsFile {
    fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

/// 保存窗口边界：同时刷新全局默认与当前项目记录。
///
/// 双写是对 Zed 写策略的有意调整（Zed 只对无项目工作区写全局默认）：
/// 首次打开的项目没有自己的记录，若不随每次保存刷新全局默认，打开新项目时仍会回落到初始尺寸。
fn save_to(
    path: &Path,
    root: Option<&Path>,
    bounds: WindowBounds,
    display_uuid: Option<String>,
) -> Result<()> {
    let mut file = read_file(path);
    let stored = StoredBounds {
        coordinate_space: WindowCoordinateSpace::GpuiLogicalPixels,
        display_uuid,
        bounds: bounds.into(),
    };
    file.default = Some(stored.clone());
    if let Some(root) = root {
        file.projects
            .insert(persistence::workspace_identity(Some(root)), stored);
    }
    persistence::atomic_write(path, &serde_json::to_vec_pretty(&file)?)
}

/// 读取窗口边界：项目记录优先，其次全局默认。
fn load_from(path: &Path, root: Option<&Path>) -> Option<(WindowBounds, Option<String>)> {
    let file = read_file(path);
    if file.version != WINDOW_BOUNDS_VERSION {
        return None;
    }
    let stored = root
        .and_then(|root| {
            file.projects
                .get(&persistence::workspace_identity(Some(root)))
        })
        .or(file.default.as_ref())?;
    if stored.coordinate_space != WindowCoordinateSpace::GpuiLogicalPixels {
        return None;
    }
    Some((stored.bounds.into(), stored.display_uuid.clone()))
}

/// 保存当前窗口边界：同时刷新全局默认与当前项目记录。
/// 项目根由持有当前窗口 Project 的调用方显式传入，失败仅记录日志、不阻塞主流程。
pub fn save_window_bounds(root: Option<&Path>, window: &mut Window, cx: &mut App) {
    let display_uuid = window
        .display(cx)
        .and_then(|display| display.uuid().ok())
        .map(|uuid| uuid.to_string());
    let bounds = window.inner_window_bounds();
    if let Err(error) = save_to(&window_bounds_path(), root, bounds, display_uuid) {
        eprintln!("保存窗口边界失败：{error:#}");
    }
}

/// 解析窗口打开参数：项目记录 → 全局默认。
/// 保存时的显示器已断开时使用当前主显示器，并将窗口矩形限制在其显示范围内。
pub fn load_window_bounds(
    root: Option<&Path>,
    cx: &App,
) -> Option<(WindowBounds, Option<DisplayId>)> {
    let (bounds, display_uuid) = load_from(&window_bounds_path(), root)?;
    let display = display_uuid
        .as_deref()
        .and_then(|saved| {
            cx.displays()
                .iter()
                .find(|display| {
                    display
                        .uuid()
                        .ok()
                        .is_some_and(|uuid| uuid.to_string() == saved)
                })
                .cloned()
        })
        .or_else(|| cx.primary_display());
    let Some(display) = display else {
        return Some((normalize_window_bounds(bounds), None));
    };

    Some((
        clamp_window_bounds(normalize_window_bounds(bounds), display.bounds()),
        Some(display.id()),
    ))
}

/// 先校正持久化输入的基本尺寸，再根据当前显示器做可见区域校正。
fn normalize_window_bounds(window_bounds: WindowBounds) -> WindowBounds {
    let normalize = |bounds: Bounds<Pixels>| Bounds {
        origin: bounds.origin,
        size: size(
            px(f32::from(bounds.size.width).max(1.0)),
            px(f32::from(bounds.size.height).max(1.0)),
        ),
    };

    match window_bounds {
        WindowBounds::Windowed(bounds) => WindowBounds::Windowed(normalize(bounds)),
        WindowBounds::Maximized(bounds) => WindowBounds::Maximized(normalize(bounds)),
        WindowBounds::Fullscreen(bounds) => WindowBounds::Fullscreen(normalize(bounds)),
    }
}

/// 将恢复矩形限制在 GPUI 当前显示器的可见边界内。
///
/// `WindowBounds` 中最大化和全屏分支保存的是恢复矩形，校正矩形不会改变窗口状态。
fn clamp_window_bounds(window_bounds: WindowBounds, display: Bounds<Pixels>) -> WindowBounds {
    let clamp = |bounds: Bounds<Pixels>| {
        let display_x = f32::from(display.origin.x);
        let display_y = f32::from(display.origin.y);
        let display_width = f32::from(display.size.width).max(1.0);
        let display_height = f32::from(display.size.height).max(1.0);
        let width = f32::from(bounds.size.width).clamp(1.0, display_width);
        let height = f32::from(bounds.size.height).clamp(1.0, display_height);
        let max_x = display_x + display_width - width;
        let max_y = display_y + display_height - height;
        let x = f32::from(bounds.origin.x).clamp(display_x, max_x);
        let y = f32::from(bounds.origin.y).clamp(display_y, max_y);

        Bounds {
            origin: point(px(x), px(y)),
            size: size(px(width), px(height)),
        }
    };

    match window_bounds {
        WindowBounds::Windowed(bounds) => WindowBounds::Windowed(clamp(bounds)),
        WindowBounds::Maximized(bounds) => WindowBounds::Maximized(clamp(bounds)),
        WindowBounds::Fullscreen(bounds) => WindowBounds::Fullscreen(clamp(bounds)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_state;

    fn windowed(x: f32, y: f32, width: f32, height: f32) -> WindowBounds {
        WindowBounds::Windowed(Bounds {
            origin: point(px(x), px(y)),
            size: size(px(width), px(height)),
        })
    }

    #[test]
    fn round_trip_keeps_all_variants() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bounds.json");
        for (bounds, root) in [
            (windowed(100.0, 200.0, 800.0, 600.0), Some("项目A")),
            (
                WindowBounds::Maximized(Bounds {
                    origin: point(px(30.0), px(40.0)),
                    size: size(px(1200.0), px(900.0)),
                }),
                Some("项目B"),
            ),
            (
                WindowBounds::Fullscreen(Bounds {
                    origin: point(px(0.0), px(0.0)),
                    size: size(px(2560.0), px(1440.0)),
                }),
                None,
            ),
        ] {
            save_to(&path, root.map(Path::new), bounds, Some("显示器-1".into())).unwrap();
            assert_eq!(
                load_from(&path, root.map(Path::new)),
                Some((bounds, Some("显示器-1".into())))
            );
        }
    }

    #[test]
    fn project_record_takes_priority_over_default() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bounds.json");
        let project_a = Path::new("/项目/A");
        let project_b = Path::new("/项目/B");

        save_to(
            &path,
            Some(project_a),
            windowed(1.0, 2.0, 300.0, 400.0),
            None,
        )
        .unwrap();
        save_to(
            &path,
            Some(project_b),
            windowed(5.0, 6.0, 700.0, 800.0),
            None,
        )
        .unwrap();

        // 项目 A 读回自己的记录，而不是最近一次写入的全局默认。
        assert_eq!(
            load_from(&path, Some(project_a)),
            Some((windowed(1.0, 2.0, 300.0, 400.0), None))
        );
        // 没有记录的项目回退到全局默认（最近一次保存）。
        assert_eq!(
            load_from(&path, None),
            Some((windowed(5.0, 6.0, 700.0, 800.0), None))
        );
    }

    #[test]
    fn missing_corrupted_or_foreign_version_returns_none() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing.json");
        assert_eq!(load_from(&missing, None), None);

        let corrupted = directory.path().join("corrupted.json");
        std::fs::write(&corrupted, "{不是 JSON").unwrap();
        assert_eq!(load_from(&corrupted, None), None);

        let foreign = directory.path().join("foreign.json");
        save_to(&foreign, None, windowed(1.0, 2.0, 300.0, 400.0), None).unwrap();
        std::fs::write(&foreign, r#"{"version":999,"default":null,"projects":{}}"#).unwrap();
        assert_eq!(load_from(&foreign, None), None);
    }

    #[test]
    fn saving_writes_default_and_project_record_with_shared_identity() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bounds.json");
        let root = Path::new("/项目/身份");

        save_to(&path, Some(root), windowed(1.0, 2.0, 300.0, 400.0), None).unwrap();

        let file: WindowBoundsFile =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(file.version, WINDOW_BOUNDS_VERSION);
        assert!(file.default.is_some());
        assert_eq!(
            file.default.as_ref().unwrap().coordinate_space,
            WindowCoordinateSpace::GpuiLogicalPixels
        );
        // 项目键与布局文件的文件名哈希一致，保证两个文件域身份统一。
        assert_eq!(
            file.projects.keys().next().map(String::as_str),
            Some(persistence::workspace_identity(Some(root)).as_str())
        );
        assert_eq!(
            file.projects.keys().next().map(String::as_str),
            layout_state::path_for_workspace(Some(root))
                .file_stem()
                .and_then(|name| name.to_str())
        );
    }

    #[test]
    fn display_uuid_survives_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bounds.json");
        save_to(
            &path,
            None,
            windowed(1.0, 2.0, 300.0, 400.0),
            Some("uuid-123".into()),
        )
        .unwrap();
        assert_eq!(
            load_from(&path, None),
            Some((windowed(1.0, 2.0, 300.0, 400.0), Some("uuid-123".into())))
        );
    }

    #[test]
    fn restore_bounds_are_clamped_without_changing_window_state() {
        let display = Bounds {
            origin: point(px(-1920.0), px(0.0)),
            size: size(px(1920.0), px(1080.0)),
        };
        let bounds = WindowBounds::Maximized(Bounds {
            origin: point(px(-4000.0), px(-2000.0)),
            size: size(px(2400.0), px(1200.0)),
        });

        assert_eq!(
            clamp_window_bounds(normalize_window_bounds(bounds), display),
            WindowBounds::Maximized(Bounds {
                origin: point(px(-1920.0), px(0.0)),
                size: size(px(1920.0), px(1080.0)),
            })
        );
    }

    #[test]
    fn invalid_persisted_size_is_normalized_before_display_lookup() {
        let bounds = normalize_window_bounds(WindowBounds::Fullscreen(Bounds {
            origin: point(px(-20.0), px(-30.0)),
            size: size(px(-1.0), px(0.0)),
        }));

        assert_eq!(
            bounds,
            WindowBounds::Fullscreen(Bounds {
                origin: point(px(-20.0), px(-30.0)),
                size: size(px(1.0), px(1.0)),
            })
        );
    }
}
