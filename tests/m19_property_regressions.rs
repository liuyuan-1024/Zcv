//! M19A 机器契约：用 proptest 覆盖随机编辑序列、多光标编辑合法性、
//! Undo/Redo roundtrip、Snapshot 稳定性等核心 invariants。
//!
//! 这是 fuzz / property-based 一体的轻量基线：proptest 的 shrinking 在失败时会自动
//! 收敛到最小复现，等价于 stable Rust 上的 fuzz 能力。如需 libFuzzer 风格的
//! coverage-guided fuzz，可在工作区外另开 `fuzz/` 子 crate（要求 nightly），不在
//! 本文件承诺。

use proptest::collection::vec;
use proptest::prelude::*;
use std::collections::BTreeSet;

use zom_engine::{
    Buffer, BufferConfig, CharOffset, Edit, Selection, SelectionSet, TextRange, Transaction,
};

// ---------- 参考模型 ----------

/// 字符串参考模型，按 char 偏移做插入 / 删除 / 替换；
/// 仅用于 differential testing，不进入引擎生产路径。
#[derive(Clone)]
struct StringRef {
    text: String,
}

impl StringRef {
    fn new(text: String) -> Self {
        Self { text }
    }

    fn replace(&mut self, start: usize, end: usize, replacement: &str) {
        let byte_start = char_to_byte(&self.text, start);
        let byte_end = char_to_byte(&self.text, end);
        self.text.replace_range(byte_start..byte_end, replacement);
    }

    fn len_chars(&self) -> usize {
        self.text.chars().count()
    }
}

fn char_to_byte(text: &str, char_offset: usize) -> usize {
    if char_offset == 0 {
        return 0;
    }
    let mut count = 0usize;
    for (idx, _) in text.char_indices() {
        if count == char_offset {
            return idx;
        }
        count += 1;
    }
    text.len()
}

// ---------- 编辑动作 ----------

#[derive(Debug, Clone)]
enum EditAction {
    Insert {
        offset_frac: u32,
        text: String,
    },
    Replace {
        start_frac: u32,
        len_frac: u32,
        text: String,
    },
}

fn edit_action_strategy() -> impl Strategy<Value = EditAction> {
    prop_oneof![
        (0u32..=10_000, "[a-zA-Z0-9 \n]{0,8}",)
            .prop_map(|(offset_frac, text)| EditAction::Insert { offset_frac, text }),
        (0u32..=10_000, 0u32..=5_000, "[a-zA-Z0-9 \n]{0,8}",).prop_map(
            |(start_frac, len_frac, text)| EditAction::Replace {
                start_frac,
                len_frac,
                text,
            }
        ),
    ]
}

fn frac_to_offset(frac: u32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (frac as usize) * (len + 1) / 10_001
}

fn apply_action(buffer: &mut Buffer, reference: &mut StringRef, action: &EditAction) {
    match action {
        EditAction::Insert { offset_frac, text } => {
            let len = buffer.len_chars().get();
            let offset = frac_to_offset(*offset_frac, len);
            let tx = Transaction::from_edits(
                buffer.version(),
                vec![Edit::insert(CharOffset::new(offset), text.clone()).unwrap()],
            )
            .unwrap();
            buffer.apply_transaction(tx).unwrap();
            reference.replace(offset, offset, text);
        }
        EditAction::Replace {
            start_frac,
            len_frac,
            text,
        } => {
            let total = buffer.len_chars().get();
            let start = frac_to_offset(*start_frac, total);
            let max_len = total.saturating_sub(start);
            let len = (*len_frac as usize) * (max_len + 1) / 5_001;
            let len = len.min(max_len);
            let end = start + len;
            let range = TextRange::new(CharOffset::new(start), CharOffset::new(end)).unwrap();
            let tx =
                Transaction::from_edits(buffer.version(), vec![Edit::replace(range, text.clone())])
                    .unwrap();
            buffer.apply_transaction(tx).unwrap();
            reference.replace(start, end, text);
        }
    }
}

// ---------- Properties ----------

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        max_shrink_iters: 256,
        ..ProptestConfig::default()
    })]

    /// Buffer 与字符串参考模型在任意随机编辑序列后文本一致。
    #[test]
    fn random_edit_sequence_matches_string_reference(actions in vec(edit_action_strategy(), 0..32)) {
        let initial = "hello world\nsecond line\n中文\n";
        let mut buffer = Buffer::from_text(initial.to_string(), BufferConfig::default()).unwrap();
        let mut reference = StringRef::new(initial.to_string());

        for action in &actions {
            apply_action(&mut buffer, &mut reference, action);
        }

        prop_assert_eq!(buffer.len_chars().get(), reference.len_chars());
        prop_assert_eq!(buffer.snapshot().text().to_string(), reference.text);
    }

    /// 任意编辑序列都能被完整 undo 回到初始文本与初始 selection。
    #[test]
    fn undo_roundtrip_restores_initial_text(actions in vec(edit_action_strategy(), 1..16)) {
        let initial = "hello world\n";
        let mut buffer = Buffer::from_text(initial.to_string(), BufferConfig::default()).unwrap();
        let initial_selection = buffer.selection().clone();
        let mut reference = StringRef::new(initial.to_string());

        for action in &actions {
            apply_action(&mut buffer, &mut reference, action);
        }

        // Undo 直到 can_undo=false，应回到初始文本。
        while buffer.can_undo() {
            buffer.undo().unwrap();
        }

        prop_assert_eq!(buffer.snapshot().text().to_string(), initial.to_string());
        prop_assert_eq!(buffer.selection(), &initial_selection);
    }

    /// undo + redo 循环后文本与最终编辑后状态等价。
    #[test]
    fn undo_then_redo_returns_to_final_state(actions in vec(edit_action_strategy(), 1..16)) {
        let initial = "abc\ndef\n";
        let mut buffer = Buffer::from_text(initial.to_string(), BufferConfig::default()).unwrap();
        let mut reference = StringRef::new(initial.to_string());

        for action in &actions {
            apply_action(&mut buffer, &mut reference, action);
        }
        let final_text = buffer.snapshot().text().to_string();
        let final_version = buffer.version();

        let mut undo_count = 0;
        while buffer.can_undo() {
            buffer.undo().unwrap();
            undo_count += 1;
        }
        for _ in 0..undo_count {
            buffer.redo().unwrap();
        }

        prop_assert_eq!(buffer.snapshot().text().to_string(), final_text);
        // 注意：每次 undo / redo 都会推进版本号，所以 version 不等同；
        // 但文本与 selection 应等价。
        prop_assert!(buffer.version() != final_version || undo_count == 0);
    }

    /// SelectionSet::new 对任意 (head, anchor) 序列产生合法（排序、不重叠）的 set。
    #[test]
    fn selection_set_normalization_yields_sorted_non_overlapping(
        ranges in vec((0u32..=200, 0u32..=200), 1..8)
    ) {
        let total_len = 256usize;
        let selections: Vec<Selection> = ranges
            .iter()
            .map(|(a, b)| {
                let lo = (*a as usize).min(*b as usize).min(total_len);
                let hi = (*a as usize).max(*b as usize).min(total_len);
                Selection::new(CharOffset::new(lo), CharOffset::new(hi))
            })
            .collect();

        let set = SelectionSet::new(selections);
        let slice = set.as_slice();

        // 排序：起点单调不减。
        for window in slice.windows(2) {
            prop_assert!(window[0].start() <= window[1].start(),
                "selection set 应按 start 排序");
        }
        // 不重叠：相邻 selection 不相交。
        for window in slice.windows(2) {
            prop_assert!(window[0].end() <= window[1].start(),
                "相邻 selection 不应重叠");
        }
    }

    /// 多光标 insert：对随机不重叠 caret 集合执行 insert_at_selections，
    /// 文本长度 = 原长度 + 选区数 * 插入长度（caret 情况下）。
    #[test]
    fn multi_caret_insert_changes_length_predictably(
        seeds in vec(0u32..=200, 1..6),
        text in "[a-z]{1,4}",
    ) {
        let initial = "x".repeat(256);
        let mut buffer = Buffer::from_text(initial.clone(), BufferConfig::default()).unwrap();

        let total = initial.chars().count();
        let mut offsets: Vec<usize> = seeds.iter().map(|s| (*s as usize).min(total)).collect();
        offsets.sort();
        offsets.dedup();
        let dedup: BTreeSet<usize> = offsets.iter().copied().collect();
        let count = dedup.len();
        if count == 0 { return Ok(()); }

        let selections: Vec<Selection> = dedup
            .iter()
            .map(|o| Selection::new(CharOffset::new(*o), CharOffset::new(*o)))
            .collect();
        buffer
            .insert_at_selections(SelectionSet::new(selections), &text)
            .unwrap();

        let expected_len = total + count * text.chars().count();
        prop_assert_eq!(buffer.len_chars().get(), expected_len);
    }

    /// Snapshot 是不可变值快照：之后任意编辑都不影响已持有的 snapshot 文本与版本。
    #[test]
    fn snapshot_is_immutable_under_subsequent_edits(actions in vec(edit_action_strategy(), 0..8)) {
        let initial = "snapshot baseline\n";
        let mut buffer = Buffer::from_text(initial.to_string(), BufferConfig::default()).unwrap();
        let snapshot = buffer.snapshot();
        let snap_text = snapshot.text().to_string();
        let snap_version = snapshot.version();
        let mut reference = StringRef::new(initial.to_string());

        for action in &actions {
            apply_action(&mut buffer, &mut reference, action);
        }

        prop_assert_eq!(snapshot.text().to_string(), snap_text);
        prop_assert_eq!(snapshot.version(), snap_version);
    }
}
