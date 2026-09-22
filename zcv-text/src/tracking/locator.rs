//! Locator：有序集合中的稳定位置标识。
//!
//! 对齐 Zed 的 text::Locator：用分数索引在相邻标识之间插入新标识，无需重编号已有元素。
//! 初始集合应使用 `Locator::between(min, max)`，为前后插入都留出空间。

use std::iter;

/// 有序集合中的一个稳定位置标识。
///
/// 比较按字典序进行；已存在的标识在插入新标识后保持不变，因此以 Locator 为键的顺序不会因编辑而失稳。
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
pub(crate) struct Locator(Vec<u64>);

impl Locator {
    pub(crate) fn min() -> Self {
        Self(vec![u64::MIN])
    }

    pub(crate) fn max() -> Self {
        Self(vec![u64::MAX])
    }

    /// 返回严格位于 `lhs` 与 `rhs` 之间的新标识。
    ///
    /// 调用方保证 `lhs < rhs`；两者相等或逆序时行为未定义。
    pub(crate) fn between(lhs: &Self, rhs: &Self) -> Self {
        let lhs = lhs.0.iter().copied().chain(iter::repeat(u64::MIN));
        let rhs = rhs.0.iter().copied().chain(iter::repeat(u64::MAX));
        let mut location = Vec::new();
        for (lhs, rhs) in lhs.zip(rhs) {
            // 这个位移很关键：它让连续输入的常见情形产生尽量短的标识。
            let mid = lhs + ((rhs.saturating_sub(lhs)) >> 48);
            location.push(mid);
            if mid > lhs {
                break;
            }
        }
        Self(location)
    }
}

impl Default for Locator {
    fn default() -> Self {
        Self::min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn between_produces_strictly_ordered_locators() {
        let min = Locator::min();
        let max = Locator::max();
        let mid = Locator::between(&min, &max);
        assert!(min < mid);
        assert!(mid < max);

        let quarter = Locator::between(&min, &mid);
        let three_quarters = Locator::between(&mid, &max);
        assert!(min < quarter && quarter < mid);
        assert!(mid < three_quarters && three_quarters < max);
    }

    #[test]
    fn repeated_insertion_at_the_same_gap_stays_ordered() {
        let mut low = Locator::min();
        let high = Locator::max();
        let mut previous = low.clone();
        for _ in 0..64 {
            let next = Locator::between(&low, &high);
            assert!(low < next && next < high);
            assert!(previous <= next);
            previous = next.clone();
            low = next;
        }
    }

    #[test]
    fn between_respects_prefix_relationships() {
        let base = Locator(vec![5]);
        let shorter = Locator(vec![5, 0]);
        let longer = Locator(vec![5, 1]);
        assert!(base < shorter);
        assert!(shorter < longer);
        let mid = Locator::between(&shorter, &longer);
        assert!(shorter < mid && mid < longer);
    }
}
