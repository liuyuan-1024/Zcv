//! 已知候选名称的模糊匹配评分。

/// 分数越大，匹配质量越高。同一查询的候选可直接按此值降序排列。
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MatchScore {
    class: u8,
    quality: i64,
}

/// 文件名与完整路径采用同一评分，分数相同则优先文件名命中。
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PathMatchScore {
    score: MatchScore,
    name_match: bool,
}

/// 查询拥有规范化后的字符；评分时复用动态规划缓冲区。
pub struct Matcher {
    query: Vec<char>,
    raw_query: Vec<char>,
    previous: Vec<Option<i64>>,
    current: Vec<Option<i64>>,
}

impl Matcher {
    /// 空白查询由列表所有者决定如何展示，不作为模糊匹配条件。
    pub fn new(query: &str) -> Option<Self> {
        let query = query.trim();
        if query.is_empty() {
            return None;
        }
        let raw_query: Vec<char> = query.chars().collect();
        Some(Self {
            query: raw_query.iter().copied().map(lowercase).collect(),
            raw_query,
            previous: Vec::new(),
            current: Vec::new(),
        })
    }

    /// 返回最佳有序子序列的分数；候选不含全部查询字符时返回 `None`。
    pub fn score(&mut self, candidate: &str) -> Option<MatchScore> {
        let chars: Vec<char> = candidate.chars().collect();
        let len = chars.len();
        if len < self.query.len() {
            return None;
        }
        let normalized: Vec<char> = chars.iter().copied().map(lowercase).collect();
        let query = &self.query;
        if normalized == *query {
            return Some(MatchScore {
                class: 3,
                quality: 0,
            });
        }
        if normalized.starts_with(query) {
            return Some(MatchScore {
                class: 2,
                quality: -(len as i64),
            });
        }
        if let Some(position) = normalized
            .windows(query.len())
            .position(|part| part == query)
        {
            return Some(MatchScore {
                class: 1,
                quality: -(position as i64) * 4 - len as i64,
            });
        }
        let mut remaining = query.iter();
        let mut next = remaining.next();
        for character in &normalized {
            if next == Some(character) {
                next = remaining.next();
            }
        }
        if next.is_some() {
            return None;
        }

        self.previous.clear();
        self.previous.resize(len, None);
        self.current.clear();
        self.current.resize(len, None);

        for (query_index, &wanted) in query.iter().enumerate() {
            self.current.fill(None);
            let mut best_gap = None;
            for position in 0..len {
                if position >= 2
                    && let Some(previous) = self.previous[position - 2]
                {
                    let gap_base = previous + (position - 2) as i64;
                    best_gap = Some(best_gap.map_or(gap_base, |best: i64| best.max(gap_base)));
                }
                if normalized[position] != wanted {
                    continue;
                }
                let boundary = position == 0
                    || matches!(chars[position - 1], '/' | '\\' | '_' | '-' | '.' | ' ')
                    || (chars[position].is_uppercase() && chars[position - 1].is_lowercase());
                let case_bonus = i64::from(self.raw_query[query_index] == chars[position]);
                let bonus = 10 + if boundary { 12 } else { 0 } + case_bonus;
                let base = if query_index == 0 {
                    Some(-(position as i64) * 2)
                } else {
                    let adjacent = position
                        .checked_sub(1)
                        .and_then(|previous_position| self.previous[previous_position])
                        .map(|score| score + 8);
                    let separated = best_gap.map(|score| score - position as i64 + 1);
                    adjacent.max(separated)
                };
                self.current[position] = base.map(|score| score + bonus);
            }
            std::mem::swap(&mut self.previous, &mut self.current);
        }

        self.previous
            .iter()
            .flatten()
            .max()
            .copied()
            .map(|quality| MatchScore {
                class: 0,
                quality: quality - len as i64,
            })
    }

    /// 对名称和相对路径评分，供文件与最近项目列表共用。
    pub fn score_path(&mut self, name: &str, path: &str) -> Option<PathMatchScore> {
        let name = self.score(name).map(|score| PathMatchScore {
            score,
            name_match: true,
        });
        let path = self.score(path).map(|score| PathMatchScore {
            score,
            name_match: false,
        });
        name.max(path)
    }
}

fn lowercase(character: char) -> char {
    character.to_lowercase().next().unwrap_or(character)
}

#[cfg(test)]
#[path = "../test/matcher_tests.rs"]
mod test;
