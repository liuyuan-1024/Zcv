//! Keystroke → 终端输入字节序列。
//!
//! 纯函数、表驱动；不依赖任何 UI 状态。平台修饰键（cmd/win）一律返回 None，让 keymap 继续分发复制/粘贴等动作。

use gpui::{Keystroke, Modifiers};

use crate::Modes;

/// 把按键事件转换为终端输入字节；返回 None 表示该键不属于终端输入（平台修饰键或纯修饰键按下），由 keymap 继续处理。
pub(crate) fn to_esc_str(
    keystroke: &Keystroke,
    mode: &Modes,
    option_as_meta: bool,
) -> Option<String> {
    let modifiers = &keystroke.modifiers;
    // 平台修饰键（cmd/win）交给 keymap 分发，不转换为终端输入。
    if modifiers.platform {
        return None;
    }
    let key = keystroke.key.as_str();
    let key_char = keystroke.key_char.as_deref();

    // 功能键与特殊键（含修饰符变体）。
    if let Some(output) = special_key(key, modifiers, mode) {
        return Some(output);
    }

    // Ctrl 与可打印字符组合：ctrl+a..z 等控制码。
    if modifiers.control
        && !modifiers.alt
        && let Some(output) = control_key(key_char, key)
    {
        return Some(output);
    }

    // alt 作为 meta：alt+可打印字符 → ESC 前缀。
    if modifiers.alt
        && !modifiers.control
        && option_as_meta
        && let Some(char) = key_char.or_else(|| printable_key(key))
    {
        return Some(format!("\x1b{char}"));
    }

    // 可打印字符返回 None：交给 IME 输入通道（replace_text_in_range → 写 PTY），
    // 与 on_key_down 的特殊键通道分离，避免按键双写并保证输入法合成可用。
    None
}

/// 特殊键表：功能键、导航键、编辑键与控制字符。
fn special_key(key: &str, modifiers: &Modifiers, mode: &Modes) -> Option<String> {
    match key {
        "enter" => Some(if modifiers.shift { "\x0a" } else { "\x0d" }.into()),
        "tab" => Some(if modifiers.shift { "\x1b[Z" } else { "\x09" }.into()),
        "backspace" => Some(if modifiers.control { "\x08" } else { "\x7f" }.into()),
        "delete" => Some("\x1b[3~".into()),
        "insert" => Some("\x1b[2~".into()),
        "pageup" => Some("\x1b[5~".into()),
        "pagedown" => Some("\x1b[6~".into()),
        "escape" => Some("\x1b".into()),
        // 普通空格与字母一致走文本输入通道（IME 单写）；ctrl+space 是控制码，必须走特殊键通道。
        "space" if modifiers.control => Some("\x00".into()),
        // Home/End 用 CSI H/F；带修饰符时携带修饰符码。
        "home" => Some(home_end_key('H', modifiers)),
        "end" => Some(home_end_key('F', modifiers)),
        // 方向键字母码：A 上 / B 下 / C 右 / D 左。
        "up" => Some(arrow_key(65, modifiers, mode)),
        "down" => Some(arrow_key(66, modifiers, mode)),
        "right" => Some(arrow_key(67, modifiers, mode)),
        "left" => Some(arrow_key(68, modifiers, mode)),
        // F1-F4 用 SS3 前缀，F5-F12 用 CSI。
        "f1" => Some("\x1bOP".into()),
        "f2" => Some("\x1bOQ".into()),
        "f3" => Some("\x1bOR".into()),
        "f4" => Some("\x1bOS".into()),
        "f5" => Some("\x1b[15~".into()),
        "f6" => Some("\x1b[17~".into()),
        "f7" => Some("\x1b[18~".into()),
        "f8" => Some("\x1b[19~".into()),
        "f9" => Some("\x1b[20~".into()),
        "f10" => Some("\x1b[21~".into()),
        "f11" => Some("\x1b[23~".into()),
        "f12" => Some("\x1b[24~".into()),
        _ => None,
    }
}

/// 方向键序列：普通模式 `\x1b[A`；应用光标模式（DECCKM）用 SS3 前缀 `\x1bOA`；
/// 带修饰符时追加 `;{code}`（xterm PC 风格：shift=2、alt=3、ctrl=5、组合按位或）。
fn arrow_key(letter: u8, modifiers: &Modifiers, mode: &Modes) -> String {
    let code = modifier_code(modifiers);
    if code == 1 {
        let prefix = if mode.contains(Modes::APP_CURSOR) {
            "\x1bO"
        } else {
            "\x1b["
        };
        format!("{prefix}{}", letter as char)
    } else {
        format!("\x1b[1;{code}{}", letter as char)
    }
}

/// Home/End 序列：`\x1b[H`；带修饰符时 `\x1b[1;{code}H`。
fn home_end_key(letter: char, modifiers: &Modifiers) -> String {
    let code = modifier_code(modifiers);
    if code == 1 {
        format!("\x1b[{letter}")
    } else {
        format!("\x1b[1;{code}{letter}")
    }
}

/// xterm 修饰符码：1 + shift + alt*2 + ctrl*4。
fn modifier_code(modifiers: &Modifiers) -> u32 {
    let mut code = 1;
    if modifiers.shift {
        code += 1;
    }
    if modifiers.alt {
        code += 2;
    }
    if modifiers.control {
        code += 4;
    }
    code
}

/// Ctrl 控制码：ctrl+a..z → 0x01..0x1a；标点符号按 ASCII 相对位置。
fn control_key(key_char: Option<&str>, key: &str) -> Option<String> {
    if let Some(char) = key_char.or_else(|| printable_key(key)) {
        let char = char.chars().next()?;
        // ctrl+[a-z] 与 ctrl+[A-Z]。
        if char.is_ascii_alphabetic() {
            return Some((((char.to_ascii_lowercase() as u8) - b'a' + 1) as char).to_string());
        }
        return match char {
            '[' => Some("\x1b".into()),
            '\\' => Some("\x1c".into()),
            ']' => Some("\x1d".into()),
            '^' => Some("\x1e".into()),
            '_' => Some("\x1f".into()),
            ' ' | '@' => Some("\x00".into()),
            '?' => Some("\x7f".into()),
            _ => None,
        };
    }
    match key {
        "space" => Some("\x00".into()),
        "backspace" => Some("\x7f".into()),
        _ => None,
    }
}

/// 非字母数字键的可打印形式（如 macOS shift+2 → "@" 已由 key_char 提供，此处兜底）。
fn printable_key(key: &str) -> Option<&str> {
    match key {
        "space" => Some(" "),
        _ if key.chars().count() == 1 => Some(key),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Keystroke, Modifiers};

    use super::*;

    fn keystroke(key: &str, modifiers: Modifiers) -> Keystroke {
        Keystroke {
            key: key.into(),
            modifiers,
            key_char: None,
        }
    }

    fn plain(key: &str) -> Keystroke {
        keystroke(key, Modifiers::none())
    }

    #[test]
    fn printable_characters_go_to_ime() {
        // 可打印字符返回 None：交给 IME 输入通道，避免 on_key_down 双写。
        let ks = Keystroke {
            key: "a".into(),
            modifiers: Modifiers::none(),
            key_char: Some("a".into()),
        };
        assert_eq!(to_esc_str(&ks, &Modes::NONE, false), None);
        // 空格与字母同属可打印字符：同样走 IME 单写，否则与 on_key_down 双写。
        assert_eq!(to_esc_str(&plain("space"), &Modes::NONE, false), None);
    }

    #[test]
    fn ctrl_space_is_control_code() {
        assert_eq!(
            to_esc_str(
                &keystroke(
                    "space",
                    Modifiers {
                        control: true,
                        ..Modifiers::none()
                    }
                ),
                &Modes::NONE,
                false
            )
            .as_deref(),
            Some("\x00")
        );
    }

    #[test]
    fn special_keys() {
        assert_eq!(
            to_esc_str(&plain("enter"), &Modes::NONE, false).as_deref(),
            Some("\r")
        );
        assert_eq!(
            to_esc_str(&plain("tab"), &Modes::NONE, false).as_deref(),
            Some("\t")
        );
        assert_eq!(
            to_esc_str(&plain("escape"), &Modes::NONE, false).as_deref(),
            Some("\x1b")
        );
        assert_eq!(
            to_esc_str(&plain("backspace"), &Modes::NONE, false).as_deref(),
            Some("\x7f")
        );
        assert_eq!(
            to_esc_str(&plain("delete"), &Modes::NONE, false).as_deref(),
            Some("\x1b[3~")
        );
    }

    #[test]
    fn shift_tab() {
        let ks = keystroke(
            "tab",
            Modifiers {
                shift: true,
                ..Modifiers::none()
            },
        );
        assert_eq!(
            to_esc_str(&ks, &Modes::NONE, false).as_deref(),
            Some("\x1b[Z")
        );
    }

    #[test]
    fn arrow_keys() {
        assert_eq!(
            to_esc_str(&plain("up"), &Modes::NONE, false).as_deref(),
            Some("\x1b[A")
        );
        assert_eq!(
            to_esc_str(&plain("down"), &Modes::NONE, false).as_deref(),
            Some("\x1b[B")
        );
        assert_eq!(
            to_esc_str(&plain("left"), &Modes::NONE, false).as_deref(),
            Some("\x1b[D")
        );
    }

    #[test]
    fn arrow_keys_app_cursor_mode() {
        assert_eq!(
            to_esc_str(&plain("up"), &Modes::APP_CURSOR, false).as_deref(),
            Some("\x1bOA")
        );
    }

    #[test]
    fn ctrl_letters() {
        let ks = |c: &str| Keystroke {
            key: c.into(),
            modifiers: Modifiers {
                control: true,
                ..Modifiers::none()
            },
            key_char: Some(c.into()),
        };
        assert_eq!(
            to_esc_str(&ks("a"), &Modes::NONE, false).as_deref(),
            Some("\x01")
        );
        assert_eq!(
            to_esc_str(&ks("z"), &Modes::NONE, false).as_deref(),
            Some("\x1a")
        );
        assert_eq!(
            to_esc_str(&ks("c"), &Modes::NONE, false).as_deref(),
            Some("\x03")
        );
    }

    #[test]
    fn ctrl_letters_fall_back_to_physical_key_without_key_char() {
        let ctrl_c = keystroke(
            "c",
            Modifiers {
                control: true,
                ..Modifiers::none()
            },
        );
        assert_eq!(
            to_esc_str(&ctrl_c, &Modes::NONE, false).as_deref(),
            Some("\x03")
        );
    }

    #[test]
    fn platform_modifiers_not_converted() {
        let ks = keystroke(
            "c",
            Modifiers {
                platform: true,
                ..Modifiers::none()
            },
        );
        assert_eq!(to_esc_str(&ks, &Modes::NONE, false), None);
    }

    #[test]
    fn alt_as_meta() {
        let ks = Keystroke {
            key: "x".into(),
            modifiers: Modifiers {
                alt: true,
                ..Modifiers::none()
            },
            key_char: Some("x".into()),
        };
        assert_eq!(
            to_esc_str(&ks, &Modes::NONE, true).as_deref(),
            Some("\x1bx")
        );
        assert_eq!(to_esc_str(&ks, &Modes::NONE, false), None);
    }

    #[test]
    fn function_keys() {
        assert_eq!(
            to_esc_str(&plain("f1"), &Modes::NONE, false).as_deref(),
            Some("\x1bOP")
        );
        assert_eq!(
            to_esc_str(&plain("f5"), &Modes::NONE, false).as_deref(),
            Some("\x1b[15~")
        );
        assert_eq!(
            to_esc_str(&plain("f12"), &Modes::NONE, false).as_deref(),
            Some("\x1b[24~")
        );
    }
}
