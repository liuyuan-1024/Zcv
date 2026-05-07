//! 事务模型：定义 Edit、EditList、Transaction、Delta 与 ChangeSet 这条文本变异主链路。
//!
//! 本文件负责 public 事务语义、版本绑定和变更映射，不直接访问 Buffer 存储，也不处理 UI 命令概念。

use crate::{
    EngineResult,
    errors::{CoordinateError, EditError, TransactionError},
    position_map::PositionMap,
    selection::SelectionSet,
    types::{BufferVersion, CharOffset, TextRange, TransactionId},
};

/// 描述单次文本修改。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Edit {
    pub range: TextRange,
    pub replacement: String,
}

impl Edit {
    pub fn new(range: TextRange, replacement: String) -> Self {
        Self { range, replacement }
    }

    pub fn insert(offset: CharOffset, text: String) -> Result<Self, CoordinateError> {
        Ok(Self {
            range: TextRange::new(offset, offset)?,
            replacement: text,
        })
    }

    pub fn delete(range: TextRange) -> Self {
        Self {
            range,
            replacement: String::new(),
        }
    }

    pub fn replace(range: TextRange, replacement: String) -> Self {
        Self { range, replacement }
    }
}

/// 归一化且验证后的编辑列表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditList {
    edits: Vec<Edit>,
}

impl EditList {
    /// 创建并验证编辑列表，自动排序并检测重叠。
    ///
    /// 注意：这里允许空列表，因为“空事务”属于 Transaction 语义，
    /// 由 Transaction::new 拒绝。
    pub fn new(mut edits: Vec<Edit>) -> Result<Self, EditError> {
        edits.sort_by_key(|edit| edit.range.start());

        for i in 1..edits.len() {
            let previous = &edits[i - 1];
            let current = &edits[i];

            if previous.range.end() > current.range.start() {
                return Err(EditError::OverlappingEdits {
                    previous: previous.range,
                    current: current.range,
                });
            }
        }

        Ok(Self { edits })
    }

    pub fn len(&self) -> usize {
        self.edits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    pub fn as_slice(&self) -> &[Edit] {
        &self.edits
    }

    pub fn into_inner(self) -> Vec<Edit> {
        self.edits
    }
}

/// 事务来源。
///
/// 这里记录“哪类编辑入口产生了事务”，供历史合并、事件观察和调试使用；
/// 它不是 Command 层，也不表达快捷键、菜单项或宏录制语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TransactionSource {
    /// 引擎调用方直接构造事务提交，没有用户交互语义。
    #[default]
    Programmatic,
    /// 鼠标驱动的编辑入口，例如拖放文本；不表示 selection movement 本身。
    Mouse,
    /// 普通键盘输入产生的文本变更，例如字符输入或 Enter。
    Keyboard,
    /// IME / composition preedit 或 commit 产生的文本变更。
    Composition,
    /// 粘贴入口产生的文本变更；宿主可据此选择不同的历史合并策略。
    Paste,
    /// 删除类编辑入口产生的文本变更。
    Delete,
    /// 格式化器或代码整理工具产生的批量文本变更。
    Formatter,
    /// 外部系统同步进来的文本变更，例如文件 watcher 或协作层适配。
    External,
    /// 历史系统回放 undo 产生的反向事务。
    Undo,
    /// 历史系统回放 redo 产生的正向事务。
    Redo,
}

/// M3 基础历史合并策略。
///
/// 完整 Smart Debounce 可以在宿主输入层基于时间窗口决定是否选择
/// `MergeWithPrevious`，引擎层只负责确定性地执行合并。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TransactionMergePolicy {
    /// 明确形成一个独立 Undo 步骤。
    #[default]
    Never,
    /// 与前一个历史节点合并为一个 Undo 步骤。
    MergeWithPrevious,
}

/// 事务元数据。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TransactionMetadata {
    source: TransactionSource,
    merge_policy: TransactionMergePolicy,
    record_history: bool,
    description: Option<String>,
}

impl TransactionMetadata {
    pub fn new(source: TransactionSource) -> Self {
        Self {
            source,
            ..Self::default()
        }
    }

    pub fn with_merge_policy(mut self, merge_policy: TransactionMergePolicy) -> Self {
        self.merge_policy = merge_policy;
        self
    }

    pub fn without_history(mut self) -> Self {
        self.record_history = false;
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn source(&self) -> TransactionSource {
        self.source
    }

    pub fn merge_policy(&self) -> TransactionMergePolicy {
        self.merge_policy
    }

    pub fn record_history(&self) -> bool {
        self.record_history
    }

    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
}

impl Default for TransactionMetadata {
    fn default() -> Self {
        Self {
            source: TransactionSource::Programmatic,
            merge_policy: TransactionMergePolicy::Never,
            record_history: true,
            description: None,
        }
    }
}

/// 批量编辑事务。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transaction {
    base_version: BufferVersion,
    edits: EditList,
    metadata: TransactionMetadata,
    before_selection: Option<SelectionSet>,
    after_selection: Option<SelectionSet>,
}

impl Transaction {
    pub fn new(base_version: BufferVersion, edits: EditList) -> Result<Self, TransactionError> {
        if edits.is_empty() {
            return Err(TransactionError::EmptyTransaction);
        }

        Ok(Self {
            base_version,
            edits,
            metadata: TransactionMetadata::default(),
            before_selection: None,
            after_selection: None,
        })
    }

    pub fn from_edits(base_version: BufferVersion, edits: Vec<Edit>) -> EngineResult<Self> {
        let edits = EditList::new(edits)?;
        Ok(Self::new(base_version, edits)?)
    }

    pub fn with_metadata(mut self, metadata: TransactionMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    pub fn with_selection(
        mut self,
        before_selection: Option<SelectionSet>,
        after_selection: Option<SelectionSet>,
    ) -> Self {
        self.before_selection = before_selection;
        self.after_selection = after_selection;
        self
    }

    pub fn base_version(&self) -> BufferVersion {
        self.base_version
    }

    pub fn edits(&self) -> &EditList {
        &self.edits
    }

    pub fn metadata(&self) -> &TransactionMetadata {
        &self.metadata
    }

    pub fn before_selection(&self) -> Option<&SelectionSet> {
        self.before_selection.as_ref()
    }

    pub fn after_selection(&self) -> Option<&SelectionSet> {
        self.after_selection.as_ref()
    }

    pub fn into_parts(
        self,
    ) -> (
        BufferVersion,
        EditList,
        TransactionMetadata,
        Option<SelectionSet>,
        Option<SelectionSet>,
    ) {
        (
            self.base_version,
            self.edits,
            self.metadata,
            self.before_selection,
            self.after_selection,
        )
    }
}

/// 增量事件，事务提交后生成。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delta {
    pub old_version: BufferVersion,
    pub new_version: BufferVersion,
    pub edits: EditList,
}

/// 文本变更事件。
///
/// `DeltaEvent` 是一次成功文本提交后的可消费事实，供后续 Anchor、
/// TrackedRange、metadata layer、外部分析结果等统一感知版本推进和位置映射。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeltaEvent {
    pub transaction_id: TransactionId,
    pub old_version: BufferVersion,
    pub new_version: BufferVersion,
    pub source: TransactionSource,
    pub delta: Delta,
    pub changeset: ChangeSet,
    pub position_map: PositionMap,
}

/// 事务变更集合。
///
/// `ChangeSet` 记录一次事务提交的已验证编辑，用于计算 changed ranges，并可产出
/// `PositionMap`。具体位置映射 API 统一由 `PositionMap` 承担。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSet {
    edits: Vec<Edit>,
}

impl ChangeSet {
    /// 只能从已经排序、已经验证过的 EditList 构造。
    pub(crate) fn from_edit_list(edits: &EditList) -> Self {
        Self {
            edits: edits.as_slice().to_vec(),
        }
    }

    pub(crate) fn edits(&self) -> &[Edit] {
        &self.edits
    }

    pub fn position_map(&self) -> PositionMap {
        PositionMap::from_edits(self.edits.clone())
    }

    /// 获取本次事务应用后，在新文本中发生改变的范围列表。
    pub fn changed_ranges(&self) -> Vec<TextRange> {
        if self.edits.is_empty() {
            return Vec::new();
        }

        let mut ranges = Vec::new();
        let mut diff = 0isize;

        for edit in &self.edits {
            let old_start = edit.range.start().get() as isize;
            let old_end = edit.range.end().get() as isize;
            let replacement_len = edit.replacement.chars().count() as isize;

            let new_start = (old_start + diff).max(0) as usize;
            let new_end = new_start + replacement_len as usize;

            ranges.push(
                TextRange::new(CharOffset::new(new_start), CharOffset::new(new_end))
                    .expect("ChangeSet 生成的范围必须满足起始位置 <= 结束位置"),
            );

            diff += replacement_len - (old_end - old_start);
        }

        Self::merge_ranges(ranges)
    }

    fn merge_ranges(ranges: Vec<TextRange>) -> Vec<TextRange> {
        let mut merged = Vec::with_capacity(ranges.len());
        let mut iter = ranges.into_iter();

        let Some(mut current) = iter.next() else {
            return merged;
        };

        for next in iter {
            if current.end() >= next.start() {
                current = TextRange::new(current.start(), current.end().max(next.end()))
                    .expect("合并范围必须满足起始位置 <= 结束位置");
            } else {
                merged.push(current);
                current = next;
            }
        }

        merged.push(current);
        merged
    }
}
