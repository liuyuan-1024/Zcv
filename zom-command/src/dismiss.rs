//! 各上下文家族的"可取消瞬态"栈。
//!
//! Esc 在每个上下文家族（TextEdit / FileTree / ProjectPicker / SearchInput 等）只绑一条 `<scope>.dismiss_top` 命令；
//! 瞬态状态（搜索栏、选区扩展、IME composition、重命名 / 删除确认……）在 begin 时 [`push`] 一个 token，token 记录"被弹出时要派发的 [`Invocation`]"。
//! Esc 触发后，命令侧执行 [`pop_top`]，把 invocation 重新 dispatch —— 这样原有的 `cancel_*` 命令逻辑一行不动，只是触发路径从"静态 mode 分区下的 esc 绑定"变成"栈顶弹出"。
//!
//! 栈按 [`DismissScope`] 隔离 —— 一个上下文家族一个独立栈。
//! 同一焦点里多个瞬态共存时，最近 push 的最先被 esc 消费（"最近开的最先关"），这就是设计要解决的核心问题：
//! 不再需要为每个新瞬态都在 [`crate::KeyBindingContext`] 里再切出一个 mode 来避免 esc 冲突。
//!
//! ## token 生命周期
//!
//! [`push`] 返回 [`DismissTokenId`]：调用方持有它，状态用别的路径结束（用户点了"提交"按钮、焦点转移、外部命令直接收掉瞬态）时调用 [`remove`] 把 token 摘掉，避免 stale token 让下一次 esc 派发到已经不存在的状态。
//! `remove` 幂等：token 已被 esc 弹掉时是 no-op。
//!
//! [`push`]: DismissStacks::push
//! [`pop_top`]: DismissStacks::pop_top
//! [`remove`]: DismissStacks::remove

use std::collections::BTreeMap;

use crate::{CommandId, Invocation};

/// 一个 DismissStack 的所属上下文家族。
///
/// 与 [`crate::KeyBindingContext`] 的家族一一对应；
/// esc 绑定按家族注册，栈也按家族隔离 —— SearchInput 聚焦时弹出的是 SearchInput 栈顶，不会跨家族越界。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DismissScope {
    TextEdit,
    FileTree,
    ProjectPicker,
    SearchInput,
    Settings,
    LanguageServers,
}

impl DismissScope {
    /// 用于 [`crate::CommandArgs`] 序列化的字符串形态。
    /// 见 `commands/system/dismiss.rs` —— esc 绑定预填 scope 参数走的就是这条。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TextEdit => "TextEdit",
            Self::FileTree => "FileTree",
            Self::ProjectPicker => "ProjectPicker",
            Self::SearchInput => "SearchInput",
            Self::Settings => "Settings",
            Self::LanguageServers => "LanguageServers",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        Some(match value {
            "TextEdit" => Self::TextEdit,
            "FileTree" => Self::FileTree,
            "ProjectPicker" => Self::ProjectPicker,
            "SearchInput" => Self::SearchInput,
            "Settings" => Self::Settings,
            "LanguageServers" => Self::LanguageServers,
            _ => return None,
        })
    }
}

/// 不透明 token：调用方拿来在状态结束时 [`DismissStacks::remove`] 自己。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DismissTokenId(u64);

#[derive(Clone, Debug)]
struct DismissEntry {
    id: DismissTokenId,
    label: &'static str,
    invocation: Invocation,
}

/// 全部 scope 的 dismiss 栈集合。
///
/// 运行时单例，由组合根（zom-desktop）持有并按引用注入 [`crate::CommandContext`]；
/// 命令 handler 用 `ctx.dismiss` 直接 push / remove / pop_top。
#[derive(Clone, Debug, Default)]
pub struct DismissStacks {
    next_id: u64,
    stacks: BTreeMap<DismissScope, Vec<DismissEntry>>,
}

impl DismissStacks {
    pub fn new() -> Self {
        Self::default()
    }

    /// 把一个新 token 推到指定 scope 的栈顶。
    ///
    /// `label` 仅作调试 / 诊断之用（命令面板 / 日志）。
    /// `invocation` 是该 token 被 [`Self::pop_top`] 弹出时要派发的命令；
    /// 通常是已有的 `xxx.cancel_yyy` invocation。
    /// 返回的 [`DismissTokenId`] 必须被调用方记住——状态自然结束（非 esc 路径）时用 [`Self::remove`] 摘掉，否则会留下 stale token。
    pub fn push(
        &mut self,
        scope: DismissScope,
        label: &'static str,
        invocation: Invocation,
    ) -> DismissTokenId {
        self.next_id += 1;
        let id = DismissTokenId(self.next_id);
        self.stacks.entry(scope).or_default().push(DismissEntry {
            id,
            label,
            invocation,
        });
        id
    }

    /// 弹出指定 scope 的栈顶 token 并返回其 invocation；空栈返回 `None`。
    ///
    /// `<scope>.dismiss_top` handler 的标准用法：拿到 `Some(invocation)` 后立即 `ctx.queue.dispatch(...)`。
    /// 返回 `None` 时调用方应该 no-op —— esc 没东西可取消。
    pub fn pop_top(&mut self, scope: DismissScope) -> Option<Invocation> {
        let stack = self.stacks.get_mut(&scope)?;
        let entry = stack.pop()?;
        if stack.is_empty() {
            self.stacks.remove(&scope);
        }
        Some(entry.invocation)
    }

    /// 按 id 摘掉 token，无论它在哪个 scope。幂等：token 已经不存在时 no-op。
    ///
    /// 用于状态用别的路径结束的场景：用户点了"提交"按钮、外部命令直接收掉瞬态、焦点切走自动关闭
    /// ——任何不走 esc 的取消路径都应该 `remove` 自己 push 的 token，否则下一次 esc 会派发到已经不在的状态。
    pub fn remove(&mut self, id: DismissTokenId) {
        let mut empty = None;
        for (scope, stack) in self.stacks.iter_mut() {
            if let Some(pos) = stack.iter().position(|entry| entry.id == id) {
                stack.remove(pos);
                if stack.is_empty() {
                    empty = Some(*scope);
                }
                break;
            }
        }
        if let Some(scope) = empty {
            self.stacks.remove(&scope);
        }
    }

    /// 清空一个 scope 的所有 token。
    /// 用于整体复位（focus 切走时把某个 scope 全收）。
    pub fn clear(&mut self, scope: DismissScope) {
        self.stacks.remove(&scope);
    }

    pub fn is_empty(&self, scope: DismissScope) -> bool {
        self.stacks.get(&scope).is_none_or(|s| s.is_empty())
    }

    pub fn depth(&self, scope: DismissScope) -> usize {
        self.stacks.get(&scope).map_or(0, Vec::len)
    }

    /// 栈顶 token 的 label——供命令面板 / 调试 UI 渲染"esc 会做什么"。
    pub fn top_label(&self, scope: DismissScope) -> Option<&'static str> {
        self.stacks.get(&scope)?.last().map(|e| e.label)
    }

    /// 栈顶 token 的 invocation 命令 id——给 reconcile 类逻辑用：
    /// host 想知道"当前 esc 会派发哪条命令"，按 id 判断而不依赖 label 字面量。
    pub fn top_command_id(&self, scope: DismissScope) -> Option<&CommandId> {
        self.stacks.get(&scope)?.last().map(|e| &e.invocation.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CommandArgs, CommandId};

    fn inv(id: &'static str) -> Invocation {
        (CommandId::new(id).unwrap(), CommandArgs::new())
    }

    #[test]
    fn push_returns_distinct_ids_and_pop_top_is_lifo() {
        let mut stacks = DismissStacks::new();
        let id_a = stacks.push(DismissScope::TextEdit, "a", inv("a.cancel"));
        let id_b = stacks.push(DismissScope::TextEdit, "b", inv("b.cancel"));
        assert_ne!(id_a, id_b);
        assert_eq!(stacks.depth(DismissScope::TextEdit), 2);
        assert_eq!(stacks.top_label(DismissScope::TextEdit), Some("b"));

        let popped = stacks.pop_top(DismissScope::TextEdit).unwrap();
        assert_eq!(popped, inv("b.cancel"));
        assert_eq!(stacks.top_label(DismissScope::TextEdit), Some("a"));

        let popped = stacks.pop_top(DismissScope::TextEdit).unwrap();
        assert_eq!(popped, inv("a.cancel"));
        assert!(stacks.is_empty(DismissScope::TextEdit));
        assert!(stacks.pop_top(DismissScope::TextEdit).is_none());
    }

    #[test]
    fn scopes_are_independent() {
        let mut stacks = DismissStacks::new();
        stacks.push(DismissScope::TextEdit, "edit", inv("edit.cancel"));
        stacks.push(DismissScope::FileTree, "tree", inv("tree.cancel"));

        assert_eq!(stacks.depth(DismissScope::TextEdit), 1);
        assert_eq!(stacks.depth(DismissScope::FileTree), 1);

        let popped = stacks.pop_top(DismissScope::FileTree).unwrap();
        assert_eq!(popped, inv("tree.cancel"));
        assert!(stacks.is_empty(DismissScope::FileTree));
        assert_eq!(stacks.depth(DismissScope::TextEdit), 1);
    }

    #[test]
    fn remove_by_id_is_idempotent_and_scoped_safe() {
        let mut stacks = DismissStacks::new();
        let id_a = stacks.push(DismissScope::TextEdit, "a", inv("a.cancel"));
        let id_b = stacks.push(DismissScope::TextEdit, "b", inv("b.cancel"));

        stacks.remove(id_a);
        assert_eq!(stacks.depth(DismissScope::TextEdit), 1);
        assert_eq!(stacks.top_label(DismissScope::TextEdit), Some("b"));

        // 重复 remove 安全。
        stacks.remove(id_a);
        assert_eq!(stacks.depth(DismissScope::TextEdit), 1);

        // 弹出栈顶，id_b 已经被消费。
        stacks.pop_top(DismissScope::TextEdit);
        stacks.remove(id_b);
        assert!(stacks.is_empty(DismissScope::TextEdit));
    }

    #[test]
    fn clear_drops_a_scope() {
        let mut stacks = DismissStacks::new();
        stacks.push(DismissScope::TextEdit, "a", inv("a"));
        stacks.push(DismissScope::TextEdit, "b", inv("b"));
        stacks.push(DismissScope::FileTree, "t", inv("t"));

        stacks.clear(DismissScope::TextEdit);
        assert!(stacks.is_empty(DismissScope::TextEdit));
        assert_eq!(stacks.depth(DismissScope::FileTree), 1);
    }
}
