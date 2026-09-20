//! 快捷键加载与解析。
//!
//! 内置平台快捷键经 GPUI action registry 解析为 [`KeyBindings`]，供应用注册和 UI 反向查询。
//! keymap 文件支持 JSONC 风格的 `//` 行注释。

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use anyhow::{Context as _, Result, anyhow};
use gpui::{Action, App, KeyBinding, KeyBindingContextPredicate};
use serde::Deserialize;
use serde_json::Value;

mod platform;

// ── 公开类型 ─────────────────────────────────────────────────────────

/// 快捷键绑定集合：正向（注册） + 反向（查询）。
pub struct KeyBindings {
    bindings: Vec<KeyBinding>,
    shortcuts: Vec<(Box<dyn Action>, String)>,
}

impl KeyBindings {
    /// 根据完整 action（包括参数）查询当前平台的快捷键显示字符串。
    ///
    /// - macOS：修饰键显示为符号（`cmd-shift-e` → `⌘⇧E`）
    /// - Linux / Windows：修饰键显示为文本（`ctrl-shift-e` → `Ctrl+Shift+E`）
    pub fn display_shortcut(&self, action: &dyn Action) -> Option<String> {
        self.shortcuts
            .iter()
            .find(|(candidate, _)| candidate.partial_eq(action))
            .map(|(_, keys)| platform::display_format(keys))
    }

    /// 仅在调用方没有 Action 实例时按名称查询。
    pub fn display_shortcut_named(&self, action_name: &str) -> Option<String> {
        self.shortcuts
            .iter()
            .find(|(action, _)| action.name() == action_name)
            .map(|(_, keys)| platform::display_format(keys))
    }
}

/// 从 App 的 [`KeyBindings`] 全局解析 action 的显示快捷键文本。
///
/// 供 UI 装配层在构造按钮/图标提示前解析；
/// 设计系统组件只消费已解析文本，不依赖快捷键注册表。
pub fn display_shortcut(action: &dyn Action, cx: &App) -> Option<String> {
    cx.try_global::<KeyBindings>()
        .and_then(|bindings| bindings.display_shortcut(action))
}

impl gpui::Global for KeyBindings {}

// ── 公开函数 ─────────────────────────────────────────────────────────

/// 在应用启动阶段加载并注册当前平台的内置快捷键。
///
/// GPUI 负责命令分发，`KeyBindings` 全局值负责向按钮和菜单提供快捷键提示；
/// 两者必须来自同一次加载，避免注册行为与界面提示使用不同数据源。
pub fn init(cx: &mut App) -> Result<()> {
    let keybindings = load(cx)?;
    cx.bind_keys(keybindings.bindings.clone());
    cx.set_global(keybindings);
    Ok(())
}

/// 加载当前平台的内置 keymap。
fn load(cx: &App) -> Result<KeyBindings> {
    let (source, json) = platform::keymap()?;
    load_json(source, &json, cx)
}

pub(crate) fn load_json(source: &str, json: &str, cx: &App) -> Result<KeyBindings> {
    let groups: Vec<RawBindingGroup> = serde_json::from_str(&strip_line_comments(json))
        .with_context(|| format!("{source} 不是合法的 keymap JSON"))?;

    detect_conflicts(&groups);

    let mut bindings = Vec::new();
    let mut shortcuts: Vec<(Box<dyn Action>, String)> = Vec::new();

    for group in groups {
        // 无上下文的默认组挂在 Workspace 根上下文：它在焦点链中最浅，因而任何面板或内容的专属上下文都会优先于默认快捷键。
        let context_source = group.context.as_deref().unwrap_or("Workspace");
        let context = KeyBindingContextPredicate::parse(context_source).map_err(|error| {
            anyhow!("{source} 包含非法快捷键上下文 {:?}：{error}", group.context)
        })?;
        let context = Rc::new(context);

        for (keys, raw_action) in &group.bindings {
            let action_name = raw_action.name();
            let action = cx
                .build_action(action_name, raw_action.params().cloned())
                .with_context(|| {
                    format!("{source} 的快捷键 {keys:?} 引用了未知或无效 action {action_name:?}")
                })?;
            let shortcut_action = action.boxed_clone();
            let binding = KeyBinding::load(
                keys,
                action,
                Some(context.clone()),
                false,
                None,
                cx.keyboard_mapper().as_ref(),
            )
            .map_err(|error| {
                anyhow!(
                    "{source} 为 action {action_name:?} 配置了非法快捷键 {:?}",
                    error.keystroke
                )
            })?;

            bindings.push(binding);
            if !shortcuts
                .iter()
                .any(|(candidate, _)| candidate.partial_eq(shortcut_action.as_ref()))
            {
                shortcuts.push((shortcut_action, keys.clone()));
            }
        }
    }

    Ok(KeyBindings {
        bindings,
        shortcuts,
    })
}

// ── 私有辅助函数 ─────────────────────────────────────────────────────

/// 去除 JSONC 风格的 `//` 行注释，字符串内的 `//`（如 URL）不受影响。
fn strip_line_comments(json: &str) -> Cow<'_, str> {
    if !json.contains("//") {
        return Cow::Borrowed(json);
    }

    let mut result = String::with_capacity(json.len());
    let mut chars = json.chars().peekable();
    let mut in_string = false;

    while let Some(ch) = chars.next() {
        if in_string {
            result.push(ch);
            match ch {
                '\\' => {
                    // 转义字符与下一个字符一并保留，避免误判 \" 为字符串结束。
                    if let Some(&escaped) = chars.peek() {
                        chars.next();
                        result.push(escaped);
                    }
                }
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => {
                in_string = true;
                result.push(ch);
            }
            '/' if chars.peek() == Some(&'/') => {
                // 丢弃注释直到行尾，保留换行符以维持行号。
                for skipped in chars.by_ref() {
                    if skipped == '\n' {
                        result.push('\n');
                        break;
                    }
                }
            }
            _ => result.push(ch),
        }
    }

    Cow::Owned(result)
}

/// JSON 文件的顶层结构：一组快捷键分组。
#[derive(Deserialize)]
struct RawBindingGroup {
    #[serde(default)]
    context: Option<String>,
    /// 键位字符串 → action 名称的映射
    bindings: BTreeMap<String, RawAction>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawAction {
    Name(String),
    WithParams((String, Value)),
}

impl RawAction {
    fn name(&self) -> &str {
        match self {
            Self::Name(name) | Self::WithParams((name, _)) => name,
        }
    }

    fn params(&self) -> Option<&Value> {
        match self {
            Self::Name(_) => None,
            Self::WithParams((_, params)) => Some(params),
        }
    }
}

/// 检测同一 (键位, 上下文) 被映射到不同 action 的冲突并告警。
fn detect_conflicts(groups: &[RawBindingGroup]) {
    let mut seen: HashMap<(&str, Option<&str>), &RawAction> = HashMap::new();
    for group in groups {
        let context = group.context.as_deref();
        for (keys, action) in &group.bindings {
            if let Some(prev) = seen.get(&(keys.as_str(), context)) {
                eprintln!(
                    "快捷键冲突: {keys:15} (ctx: {ctx:12}) → '{}' 和 '{}'，后者覆盖前者",
                    prev.name(),
                    action.name(),
                    ctx = context.unwrap_or("(全局)"),
                );
            }
            seen.insert((keys, context), action);
        }
    }
}

#[cfg(test)]
#[path = "test/loader_tests.rs"]
mod tests;
