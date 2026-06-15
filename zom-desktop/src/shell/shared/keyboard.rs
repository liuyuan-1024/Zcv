//! OS 按键事件到 `zom-command::KeyChord` 字符串的归一化。

use gpui::Keystroke;

/// 把按键归一化成 `(mod )*key` 字符串供 `Keymap` 解析。
///
/// 文本输入不在这里产出 —— 空格 / 字母数字等让系统输入法或 NSTextInputClient 通过 `EntityInputHandler::replace_text_in_range` 喂进来；
/// Tab / Enter 这类编辑器动作则由 keymap 命中具体编辑命令。
pub(crate) fn normalized_chord(keystroke: &Keystroke) -> String {
    let mut parts = Vec::new();
    let modifiers = keystroke.modifiers;
    if modifiers.secondary() {
        parts.push("mod");
    } else if modifiers.control {
        parts.push("ctrl");
    }
    if modifiers.alt {
        parts.push("alt");
    }
    if modifiers.shift {
        parts.push("shift");
    }
    if modifiers.function {
        parts.push("fn");
    }
    parts.push(keystroke.key.as_str());
    parts.join(" ")
}
