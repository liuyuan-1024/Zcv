//! 逻辑 chord → 平台快捷键文案的统一投影。
//!
//! 底层 `Keymap` 永远只认逻辑 chord 字符串；
//! UI 层需要展示给人看时，统一经过本模块投影到当前平台的符号或文本，**不要在调用方手动拼接符号**。
//!
//! chord 内用空格分隔修饰键与主键名，最后一段是主键，其余是修饰键。
//!
//! 平台差异由两张数据表承载：
//! - [`MODIFIERS`]：逻辑修饰键 → 显示文本。
//! - [`SPECIAL_KEYS`]：特殊键名 → 显示文本（普通字母数字键直接大写）。
//!
//! [`MODIFIERS`] 还规定了**显示顺序**，保证 `"shift alt mod z"` 和 `"mod shift alt z"` 给出同样的字符串。
//! macOS 按 HIG 顺序 `⌃⌥⇧⌘`；其他平台按习惯顺序 `Ctrl+Alt+Shift+...`。
//! macOS 符号之间保留空隙，避免 tooltip / 菜单里连续符号难以区分。

use crate::KeyChord;

/// 把多段 chord（leader key 序列）格式化成一个完整快捷键串。
///
/// 调用方一般不直接用本函数，而是用 [`crate::Keymap::format_shortcuts_for`]
/// 一步反查 + 格式化。
pub fn format_sequence(sequence: &[KeyChord]) -> String {
    sequence
        .iter()
        .map(|chord| format_chord(chord.as_str()))
        .collect::<Vec<_>>()
        .join(CHORD_SEPARATOR)
}

/// 同一命令的多条快捷键绑定的拼接展示。
///
/// 选用 ` / ` 作为分隔符：避开 [`CHORD_SEPARATOR`]（已用作 leader 序列内分隔），
/// 跨平台无需本地化，紧凑且不易与 modifier 符号混淆。
pub fn format_sequences(sequences: &[&[KeyChord]]) -> String {
    sequences
        .iter()
        .map(|sequence| format_sequence(sequence))
        .collect::<Vec<_>>()
        .join(BINDING_SEPARATOR)
}

const BINDING_SEPARATOR: &str = " / ";

/// 把单段 chord 格式化成给人看的字符串。
pub fn format_chord(chord: &str) -> String {
    let tokens: Vec<&str> = chord.split_whitespace().collect();
    let Some((key, mods)) = tokens.split_last() else {
        return String::new();
    };

    let mut labels = Vec::new();
    // MODIFIERS 决定显示顺序：遍历表本身而不是 tokens，保证幂等。
    for &(name, label) in MODIFIERS {
        if mods.contains(&name) && !labels.contains(&label) {
            labels.push(label);
        }
    }

    let mut out = String::new();
    for label in labels {
        out.push_str(label);
        if !MODIFIER_SEPARATOR.is_empty() {
            out.push_str(MODIFIER_SEPARATOR);
        }
    }
    out.push_str(&format_key(key));
    out
}

fn format_key(key: &str) -> String {
    if let Some((_, label)) = SPECIAL_KEYS.iter().find(|(name, _)| *name == key) {
        return (*label).to_string();
    }
    if key.is_empty() {
        return String::new();
    }
    let mut chars = key.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

// ==================================================
// 平台数据表 —— 改文案只改下面这两段
// ==================================================

#[cfg(target_os = "macos")]
mod platform {
    /// 修饰键与主键之间用空格分隔，避免连续符号难以区分。
    pub(super) const MODIFIER_SEPARATOR: &str = " ";
    /// 多段 chord 之间用双空格分隔。
    pub(super) const CHORD_SEPARATOR: &str = "  ";

    /// 逻辑修饰键 → 显示符号；**顺序就是显示顺序**（HIG: ⌃⌥⇧⌘）。
    pub(super) const MODIFIERS: &[(&str, &str)] = &[
        ("ctrl", "⌃"),
        ("alt", "⌥"),
        ("shift", "⇧"),
        ("mod", "⌘"),
        ("fn", "fn"),
    ];

    pub(super) const SPECIAL_KEYS: &[(&str, &str)] = &[
        ("left", "←"),
        ("right", "→"),
        ("up", "↑"),
        ("down", "↓"),
        ("backspace", "⌫"),
        ("delete", "⌦"),
        ("enter", "⏎"),
        ("return", "⏎"),
        ("escape", "⎋"),
        ("esc", "⎋"),
        ("tab", "⇥"),
        ("space", "␣"),
        ("home", "↖"),
        ("end", "↘"),
        ("pageup", "⇞"),
        ("pagedown", "⇟"),
    ];
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub(super) const MODIFIER_SEPARATOR: &str = "+";
    pub(super) const CHORD_SEPARATOR: &str = " ";

    /// 逻辑修饰键 → 显示文本；顺序：Ctrl → Alt → Shift。
    pub(super) const MODIFIERS: &[(&str, &str)] = &[
        // `mod` 在非 macOS 上等价于物理 Ctrl，统一显示成 "Ctrl"。
        ("mod", "Ctrl"),
        ("ctrl", "Ctrl"),
        ("alt", "Alt"),
        ("shift", "Shift"),
        ("fn", "Fn"),
    ];

    pub(super) const SPECIAL_KEYS: &[(&str, &str)] = &[
        ("left", "←"),
        ("right", "→"),
        ("up", "↑"),
        ("down", "↓"),
        ("backspace", "Backspace"),
        ("delete", "Del"),
        ("enter", "Enter"),
        ("return", "Enter"),
        ("escape", "Esc"),
        ("esc", "Esc"),
        ("tab", "Tab"),
        ("space", "Space"),
        ("home", "Home"),
        ("end", "End"),
        ("pageup", "PgUp"),
        ("pagedown", "PgDn"),
    ];
}

use platform::{CHORD_SEPARATOR, MODIFIER_SEPARATOR, MODIFIERS, SPECIAL_KEYS};

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_should_render_hig_modifier_order_with_symbol_spacing() {
        assert_eq!(format_chord("mod z"), "⌘ Z");
        // 任何顺序都按 MODIFIERS 表里的顺序输出。
        assert_eq!(format_chord("mod shift z"), "⇧ ⌘ Z");
        assert_eq!(format_chord("shift mod z"), "⇧ ⌘ Z");
        assert_eq!(format_chord("shift alt left"), "⌥ ⇧ ←");
        assert_eq!(format_chord("backspace"), "⌫");
        assert_eq!(format_chord("escape"), "⎋");
        assert_eq!(format_chord("space"), "␣");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_mac_should_render_textual_modifiers_with_plus() {
        assert_eq!(format_chord("mod z"), "Ctrl+Z");
        assert_eq!(format_chord("mod shift z"), "Ctrl+Shift+Z");
        assert_eq!(format_chord("shift mod z"), "Ctrl+Shift+Z");
        assert_eq!(format_chord("ctrl mod shift z"), "Ctrl+Shift+Z");
        assert_eq!(format_chord("shift alt left"), "Alt+Shift+←");
        assert_eq!(format_chord("backspace"), "Backspace");
        assert_eq!(format_chord("escape"), "Esc");
        assert_eq!(format_chord("space"), "Space");
    }

    #[test]
    fn empty_chord_should_not_panic() {
        assert_eq!(format_chord(""), "");
        assert_eq!(format_sequence(&[]), "");
    }

    #[test]
    fn extra_spaces_are_harmless() {
        // split_whitespace 对待多个空格跟单空格一样。
        assert_eq!(format_chord("mod  z"), format_chord("mod z"));
    }

    #[test]
    fn multi_chord_sequence_should_join_with_chord_separator() {
        let leader = KeyChord::new("mod k").unwrap();
        let follow = KeyChord::new("mod b").unwrap();
        let formatted = format_sequence(&[leader, follow]);
        // 不同平台 chord 不同，但中间一定有 CHORD_SEPARATOR。
        assert!(formatted.contains(CHORD_SEPARATOR));
    }

    #[test]
    fn multi_binding_should_join_with_slash() {
        let next = vec![KeyChord::new("mod l").unwrap()];
        let prev = vec![KeyChord::new("mod h").unwrap()];
        let formatted = format_sequences(&[next.as_slice(), prev.as_slice()]);
        assert!(formatted.contains(BINDING_SEPARATOR));
        assert!(formatted.starts_with(&format_chord("mod l")));
        assert!(formatted.ends_with(&format_chord("mod h")));
    }
}
