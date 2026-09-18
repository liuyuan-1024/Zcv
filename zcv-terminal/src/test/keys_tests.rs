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
