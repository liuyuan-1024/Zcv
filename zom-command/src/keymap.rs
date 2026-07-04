//! 键位模型与上下文解析。

use std::collections::BTreeMap;

use crate::commands::{
    editor::{
        CompositionBinding, TextEditBindingContext, TextEditKeyContext, text_edit_context_matches,
    },
    file_tree::{FileTreeBindingContext, FileTreeKeyContext, FileTreeKeyMode},
    language_servers::{LanguageServersBindingContext, LanguageServersKeyContext},
    project_picker::{ProjectPickerBindingContext, ProjectPickerKeyContext},
    settings::{SettingsBindingContext, SettingsKeyContext},
    version_control::{
        VersionControlBindingContext, VersionControlKeyContext, VersionControlKeyMode,
    },
};
use crate::{CommandArgs, CommandError, CommandId, keymap_format};

/// 归一化后的单次组合键。
///
/// OS 事件 → 归一化 `KeyChord` 由 `zom-desktop` 完成；本 crate 只吃归一化结果。
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct KeyChord(String);

impl KeyChord {
    pub fn new(value: impl Into<String>) -> Result<Self, CommandError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(CommandError::InvalidKeyChord);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 多段按键序列（支持 leader key）。
pub type KeySequence = Vec<KeyChord>;

/// 运行时按键上下文。调用方按优先级传入多个上下文；
/// 例如文件树新建态先传 `TextEdit`，未命中后再传 `FileTree(PendingName)` / `Global`。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyContext {
    Global,
    TextEdit(TextEditKeyContext),
    FileTree(FileTreeKeyContext),
    ProjectPicker(ProjectPickerKeyContext),
    Settings(SettingsKeyContext),
    LanguageServers(LanguageServersKeyContext),
    VersionControl(VersionControlKeyContext),
    SearchBar,
    GoToLineInput,
    BranchPicker,
}

impl KeyContext {
    pub fn global() -> Self {
        Self::Global
    }

    pub fn text_edit(accepts_newline: bool, composing: bool) -> Self {
        Self::TextEdit(TextEditKeyContext {
            accepts_newline,
            composing,
        })
    }

    pub fn file_tree(mode: FileTreeKeyMode) -> Self {
        Self::FileTree(FileTreeKeyContext { mode })
    }

    pub fn project_picker() -> Self {
        Self::ProjectPicker(ProjectPickerKeyContext)
    }

    pub fn settings() -> Self {
        Self::Settings(SettingsKeyContext)
    }

    pub fn language_servers() -> Self {
        Self::LanguageServers(LanguageServersKeyContext)
    }

    pub fn version_control(mode: VersionControlKeyMode) -> Self {
        Self::VersionControl(VersionControlKeyContext { mode })
    }

    pub fn search_bar() -> Self {
        Self::SearchBar
    }

    pub fn go_to_line_input() -> Self {
        Self::GoToLineInput
    }

    pub fn branch_picker() -> Self {
        Self::BranchPicker
    }
}

/// 键位绑定适用的结构化上下文。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyBindingContext {
    Global,
    TextEdit(TextEditBindingContext),
    FileTree(FileTreeBindingContext),
    ProjectPicker(ProjectPickerBindingContext),
    Settings(SettingsBindingContext),
    LanguageServers(LanguageServersBindingContext),
    VersionControl(VersionControlBindingContext),
    SearchInput,
    GoToLineInput,
    BranchPicker,
}

impl KeyBindingContext {
    pub fn global() -> Self {
        Self::Global
    }

    pub fn text_edit() -> Self {
        Self::TextEdit(TextEditBindingContext {
            requires_newline: false,
            composition: CompositionBinding::Inactive,
        })
    }

    pub fn text_edit_multiline() -> Self {
        Self::TextEdit(TextEditBindingContext {
            requires_newline: true,
            composition: CompositionBinding::Inactive,
        })
    }

    pub fn text_edit_composition() -> Self {
        Self::TextEdit(TextEditBindingContext {
            requires_newline: false,
            composition: CompositionBinding::Active,
        })
    }

    pub fn file_tree(mode: FileTreeKeyMode) -> Self {
        Self::FileTree(FileTreeBindingContext { mode })
    }

    pub fn project_picker() -> Self {
        Self::ProjectPicker(ProjectPickerBindingContext)
    }

    pub fn settings() -> Self {
        Self::Settings(SettingsBindingContext)
    }

    pub fn language_servers() -> Self {
        Self::LanguageServers(LanguageServersBindingContext)
    }

    pub fn version_control(mode: VersionControlKeyMode) -> Self {
        Self::VersionControl(VersionControlBindingContext { mode })
    }

    pub fn search_input() -> Self {
        Self::SearchInput
    }

    pub fn go_to_line_input() -> Self {
        Self::GoToLineInput
    }

    pub fn branch_picker() -> Self {
        Self::BranchPicker
    }

    /// 两条绑定的上下文是否可能被同一个运行时 [`KeyContext`] 同时命中。
    ///
    /// 这是「冲突」的真正定义：同一序列下两条绑定一旦重叠，[`Keymap::resolve`]
    /// 就只能靠注册顺序裁决——所以 [`Keymap::try_bind`] 用它（而非相等）判重。
    ///
    /// `requires_newline` 是单向过滤、不切分上下文空间，故不参与重叠判定；
    /// 真正切分上下文的维度是 `composition`（`Active`/`Inactive` 互斥）与
    /// `FileTree` 的 `mode`。
    pub(crate) fn overlaps(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Global, Self::Global) => true,
            (Self::TextEdit(a), Self::TextEdit(b)) => a.composition.overlaps(b.composition),
            (Self::FileTree(a), Self::FileTree(b)) => a.mode == b.mode,
            (Self::ProjectPicker(_), Self::ProjectPicker(_)) => true,
            (Self::Settings(_), Self::Settings(_)) => true,
            (Self::LanguageServers(_), Self::LanguageServers(_)) => true,
            (Self::VersionControl(a), Self::VersionControl(b)) => a.mode == b.mode,
            (Self::SearchInput, Self::SearchInput) => true,
            (Self::GoToLineInput, Self::GoToLineInput) => true,
            (Self::BranchPicker, Self::BranchPicker) => true,
            _ => false,
        }
    }
}

/// 一条键位绑定：序列 + 上下文 → 已成形的命令调用。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyBinding {
    pub sequence: KeySequence,
    pub command: CommandId,
    pub args: CommandArgs,
    pub context: KeyBindingContext,
}

/// 键位表。内部维护前缀 trie，用于识别多段 key sequence。
#[derive(Clone, Debug, Default)]
pub struct Keymap {
    bindings: Vec<KeyBinding>,
    root: KeymapNode,
}

#[derive(Clone, Debug, Default)]
struct KeymapNode {
    children: BTreeMap<KeyChord, KeymapNode>,
    binding_indices: Vec<usize>,
}

impl Keymap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bind(&mut self, binding: KeyBinding) {
        self.try_bind(binding)
            .expect("同一序列的快捷键绑定上下文不能重叠");
    }

    /// 绑定一条快捷键。冲突判定按「上下文重叠」而非「上下文相等」——
    /// 详见 `KeyBindingContext::overlaps`。
    /// 同一序列在互不重叠的上下文里复用（如 `enter` 同时绑给文件树导航和编辑器换行）不算冲突。
    pub fn try_bind(&mut self, binding: KeyBinding) -> Result<(), CommandError> {
        if self.bindings.iter().any(|existing| {
            existing.sequence == binding.sequence && existing.context.overlaps(&binding.context)
        }) {
            return Err(CommandError::DuplicateKeyBinding {
                sequence: binding.sequence,
                context: binding.context,
            });
        }

        let index = self.bindings.len();
        if !binding.sequence.is_empty() {
            let mut node = &mut self.root;
            for chord in &binding.sequence {
                node = node.children.entry(chord.clone()).or_default();
            }
            node.binding_indices.push(index);
        }
        self.bindings.push(binding);
        Ok(())
    }

    pub fn bindings(&self) -> &[KeyBinding] {
        &self.bindings
    }

    /// 反查某条命令对应的全部快捷键序列（原始 `KeyChord` 序列）。
    ///
    /// 按 install 顺序返回；相同的 `KeySequence` 仅保留首次出现（同一序列绑在多个 context 时去重）。
    /// UI 通常用 [`Self::format_shortcuts_for`] 直接拿可展示字符串；本方法用于需要自定义渲染的场景。
    ///
    /// 一个命令可能因不同 `CommandArgs` 绑了多条 chord
    /// —— 这种 args 差异在 `description` 写得统一时视作 alias，UI 合并展示。
    pub fn shortcuts_for(&self, command: &CommandId) -> Vec<&KeySequence> {
        let mut result: Vec<&KeySequence> = Vec::new();
        for binding in &self.bindings {
            if &binding.command != command {
                continue;
            }
            if !result.contains(&&binding.sequence) {
                result.push(&binding.sequence);
            }
        }
        result
    }

    /// 反查 + 平台投影：给 UI 一个直接能展示的快捷键串。
    ///
    /// 多条 binding 输出以 ` / ` 拼接，由 [`crate::keymap_format`] 做平台投影。
    pub fn format_shortcuts_for(&self, command: &CommandId) -> Option<String> {
        let sequences = self.shortcuts_for(command);
        if sequences.is_empty() {
            return None;
        }
        let slices: Vec<&[KeyChord]> = sequences.iter().map(|seq| seq.as_slice()).collect();
        Some(keymap_format::format_sequences(&slices))
    }

    /// 按已输入的序列前缀解析。`contexts` 按优先级从高到低排列。
    pub fn resolve(&self, prefix: &[KeyChord], contexts: &[KeyContext]) -> KeymapResolution {
        let Some(node) = self.node_for_prefix(prefix) else {
            return KeymapResolution::NoMatch;
        };

        for context in contexts {
            if let Some(binding) = node
                .binding_indices
                .iter()
                .rev()
                .filter_map(|index| self.bindings.get(*index))
                .find(|binding| binding_matches_context(binding, *context))
            {
                return KeymapResolution::Matched {
                    command: binding.command.clone(),
                    args: binding.args.clone(),
                };
            }
        }

        if node_has_context_match(node, &self.bindings, contexts) {
            KeymapResolution::Pending
        } else {
            KeymapResolution::NoMatch
        }
    }

    fn node_for_prefix(&self, prefix: &[KeyChord]) -> Option<&KeymapNode> {
        let mut node = &self.root;
        for chord in prefix {
            node = node.children.get(chord)?;
        }
        Some(node)
    }
}

/// 键位解析结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KeymapResolution {
    /// 是某个更长序列的前缀，等待后续按键。
    Pending,
    /// 命中一条绑定。
    Matched {
        command: CommandId,
        args: CommandArgs,
    },
    /// 无匹配。
    NoMatch,
}

fn binding_matches_context(binding: &KeyBinding, context: KeyContext) -> bool {
    match (binding.context, context) {
        (KeyBindingContext::Global, KeyContext::Global) => true,
        (KeyBindingContext::TextEdit(binding), KeyContext::TextEdit(active)) => {
            text_edit_context_matches(binding, active)
        }
        (KeyBindingContext::FileTree(binding), KeyContext::FileTree(active)) => {
            binding.mode == active.mode
        }
        (KeyBindingContext::ProjectPicker(_), KeyContext::ProjectPicker(_)) => true,
        (KeyBindingContext::Settings(_), KeyContext::Settings(_)) => true,
        (KeyBindingContext::LanguageServers(_), KeyContext::LanguageServers(_)) => true,
        (KeyBindingContext::VersionControl(binding), KeyContext::VersionControl(active)) => {
            binding.mode == active.mode
        }
        (KeyBindingContext::SearchInput, KeyContext::SearchBar) => true,
        (KeyBindingContext::GoToLineInput, KeyContext::GoToLineInput) => true,
        (KeyBindingContext::BranchPicker, KeyContext::BranchPicker) => true,
        _ => false,
    }
}

fn node_has_context_match(
    node: &KeymapNode,
    bindings: &[KeyBinding],
    contexts: &[KeyContext],
) -> bool {
    node.binding_indices
        .iter()
        .filter_map(|index| bindings.get(*index))
        .any(|binding| {
            contexts
                .iter()
                .any(|context| binding_matches_context(binding, *context))
        })
        || node
            .children
            .values()
            .any(|child| node_has_context_match(child, bindings, contexts))
}
