//! 根据设置值的变化规划 JSONC 原文编辑，保留未涉及的文本。

use std::ops::Range;

use anyhow::{Context as _, Result};
use serde_json::{Map, Value};
use tree_sitter::{Node, Parser};

use crate::schema::UserSettingsContent;

type TextEdit = (Range<usize>, String);

pub(crate) fn update_text(
    text: &str,
    previous: &UserSettingsContent,
    next: &UserSettingsContent,
) -> Result<String> {
    if previous == next {
        return Ok(text.to_owned());
    }
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_json::LANGUAGE.into())
        .expect("JSON 语法版本必须与 Tree-sitter 兼容");
    let old_value = serde_json::to_value(previous)?;
    let new_value = serde_json::to_value(next)?;
    let mut result = if text.trim().is_empty() {
        format!("{text}{{}}\n")
    } else {
        text.to_owned()
    };
    let tree = parser.parse(&result, None).context("无法分析设置文本")?;
    let mut cursor = tree.root_node().walk();
    let object = tree
        .root_node()
        .named_children(&mut cursor)
        .find(|node| node.kind() == "object")
        .context("设置文本缺少根对象")?;
    let mut edits = Vec::new();
    edit_value(object, &result, &old_value, &new_value, &mut edits)?;
    edits.sort_by_key(|(range, _)| (range.start, range.end));
    for (range, replacement) in edits.into_iter().rev() {
        result.replace_range(range, &replacement);
    }
    Ok(result)
}

fn edit_value(
    node: Node<'_>,
    text: &str,
    previous: &Value,
    next: &Value,
    edits: &mut Vec<TextEdit>,
) -> Result<()> {
    if previous == next {
        return Ok(());
    }
    if let (Value::Object(previous), Value::Object(next)) = (previous, next) {
        return edit_object(node, text, previous, next, edits);
    }
    edits.push((node.byte_range(), serde_json::to_string(next)?));
    Ok(())
}

fn edit_object(
    node: Node<'_>,
    text: &str,
    previous: &Map<String, Value>,
    next: &Map<String, Value>,
    edits: &mut Vec<TextEdit>,
) -> Result<()> {
    let mut cursor = node.walk();
    let mut first_retained = None;
    let mut needs_comma = false;
    let mut comma = None;
    for child in node.children(&mut cursor) {
        // Tree-sitter 的 JSON 语法将 JSONC 尾逗号识别为 ERROR。
        if matches!(child.kind(), "," | "ERROR") && &text[child.byte_range()] == "," {
            comma = Some(child.byte_range());
            continue;
        }
        if child.kind() != "pair" {
            continue;
        }
        let key_node = child.child_by_field_name("key").context("设置字段缺少键")?;
        let key: String = serde_json::from_str(&text[key_node.byte_range()])?;
        let old_value = previous.get(&key).context("设置字段与原文不一致")?;
        if let Some(new_value) = next.get(&key) {
            if !needs_comma && let Some(comma) = comma.take() {
                edits.push((comma, String::new()));
            }
            comma = None;
            needs_comma = true;
            first_retained.get_or_insert(child.start_byte());
            let value = child
                .child_by_field_name("value")
                .context("设置字段缺少值")?;
            edit_value(value, text, old_value, new_value, edits)?;
        } else {
            edits.push((child.byte_range(), String::new()));
            if let Some(comma) = comma.take() {
                edits.push((comma, String::new()));
            }
        }
    }
    if !needs_comma && let Some(comma) = comma {
        edits.push((comma, String::new()));
    }

    let additions = next
        .iter()
        .filter(|(key, _)| !previous.contains_key(*key))
        .map(|(key, value)| {
            Ok(format!(
                "{}: {}",
                serde_json::to_string(key)?,
                serde_json::to_string(value)?
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    if !additions.is_empty() {
        insert_fields(node, text, first_retained, &additions, edits);
    }
    Ok(())
}

fn insert_fields(
    node: Node<'_>,
    text: &str,
    first_retained: Option<usize>,
    additions: &[String],
    edits: &mut Vec<TextEdit>,
) {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let closing = node.end_byte() - 1;
    if let Some(start) = first_retained {
        let line = text[..start].rsplit('\n').next().unwrap_or_default();
        let separator = if line.trim().is_empty() {
            format!(",{newline}{line}")
        } else {
            ", ".to_owned()
        };
        let insertion = additions.join(&separator) + &separator;
        edits.push((start..start, insertion));
    } else {
        let closing_line = text[..closing].rsplit('\n').next().unwrap_or_default();
        let indentation = if closing_line.trim().is_empty() {
            closing_line
        } else {
            ""
        };
        let separator = format!(",{newline}{indentation}  ");
        let insertion = format!(
            "{newline}{indentation}  {}{newline}{indentation}",
            additions.join(&separator)
        );
        edits.push((closing..closing, insertion));
    }
}
