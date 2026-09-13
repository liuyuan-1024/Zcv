use std::borrow::Cow;

use anyhow::{Context as _, Result};

pub(super) fn display_format(raw: &str) -> String {
    #[cfg(target_os = "macos")]
    {
        macos_display(raw)
    }
    #[cfg(not(target_os = "macos"))]
    {
        text_display(raw)
    }
}

#[cfg(target_os = "macos")]
pub(super) fn macos_display(raw: &str) -> String {
    fn modifier(key: &str) -> Option<&'static str> {
        match key {
            "ctrl" | "control" => Some("⌃"),
            "shift" => Some("⇧"),
            "option" | "alt" => Some("⌥"),
            "cmd" | "command" => Some("⌘"),
            _ => None,
        }
    }

    fn key_symbol(key: &str) -> Option<&'static str> {
        match key {
            "backspace" => Some("⌫"),
            "delete" => Some("⌦"),
            "enter" | "return" => Some("↩"),
            "escape" => Some("⎋"),
            "tab" => Some("⇥"),
            "capslock" => Some("⇪"),
            "up" => Some("↑"),
            "down" => Some("↓"),
            "left" => Some("←"),
            "right" => Some("→"),
            "home" => Some("↖"),
            "end" => Some("↘"),
            "pageup" => Some("⇞"),
            "pagedown" => Some("⇟"),
            "space" => Some("␣"),
            _ => None,
        }
    }

    raw.split_whitespace()
        .map(|chord| {
            chord
                .split('-')
                .map(|part| {
                    modifier(part)
                        .map(str::to_owned)
                        .or_else(|| key_symbol(part).map(str::to_owned))
                        .unwrap_or_else(|| {
                            if part.len() == 1 {
                                part.to_uppercase()
                            } else {
                                part.to_owned()
                            }
                        })
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(not(target_os = "macos"))]
fn text_display(raw: &str) -> String {
    fn modifier(key: &str) -> Option<&'static str> {
        match key {
            "cmd" | "ctrl" => Some("Ctrl"),
            "shift" => Some("Shift"),
            "alt" | "option" => Some("Alt"),
            "super" => Some("Super"),
            "win" => Some("Win"),
            _ => None,
        }
    }

    raw.split_whitespace()
        .map(|chord| {
            chord
                .split('-')
                .map(|part| {
                    if part == "," {
                        return ",".to_owned();
                    }
                    modifier(part).map(str::to_owned).unwrap_or_else(|| {
                        if part.len() == 1 {
                            part.to_uppercase()
                        } else {
                            part.to_owned()
                        }
                    })
                })
                .collect::<Vec<_>>()
                .join("+")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn keymap() -> Result<(&'static str, Cow<'static, str>)> {
    #[cfg(target_os = "macos")]
    let source = "default-macos.json";
    #[cfg(target_os = "windows")]
    let source = "default-windows.json";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let source = "default-linux.json";

    let json = zcv_assets::text(&format!("keymaps/{source}"))
        .with_context(|| format!("缺少内置快捷键 {source}"))?;
    Ok((source, json))
}
