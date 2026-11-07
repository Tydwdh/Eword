#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatch {
    pub score: i32,
    pub indices: Vec<usize>,
}

pub fn fuzzy_match(needle: &str, haystack: &str) -> Option<FuzzyMatch> {
    if needle.is_empty() {
        return Some(FuzzyMatch {
            score: 0,
            indices: Vec::new(),
        });
    }

    let needle_chars: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    let hay_chars: Vec<char> = haystack.chars().flat_map(char::to_lowercase).collect();
    if needle_chars.len() > hay_chars.len() {
        return None;
    }

    let mut positions = Vec::with_capacity(needle_chars.len());
    let mut cursor = 0usize;
    for wanted in &needle_chars {
        let mut found = None;
        for (index, ch) in hay_chars.iter().enumerate().skip(cursor) {
            if ch == wanted {
                found = Some(index);
                break;
            }
        }
        let index = found?;
        positions.push(index);
        cursor = index + 1;
    }

    let lower_needle: String = needle_chars.iter().collect();
    let lower_hay: String = hay_chars.iter().collect();
    let mut score = 40i32;

    if lower_hay.starts_with(&lower_needle) {
        score += 50;
    } else if lower_hay.contains(&lower_needle) {
        score += 24;
    }

    for (position_index, &position) in positions.iter().enumerate() {
        if position == 0 {
            score += 14;
        } else if matches!(hay_chars[position - 1], ' ' | '-' | '_' | '/' | '.' | ':') {
            score += 10;
        }
        if let Some(previous) = positions.get(position_index.wrapping_sub(1))
            && position_index > 0
        {
            if *previous + 1 == position {
                score += 12;
            } else {
                score -= ((position - previous - 1).min(12) as i32) * 2;
            }
        }
    }

    score -= (lower_hay.chars().count() as i32) / 6;
    Some(FuzzyMatch {
        score: score.max(1),
        indices: positions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_subsequence_and_scores_prefix_higher() {
        let prefix = fuzzy_match("fin", "/find").unwrap();
        let loose = fuzzy_match("fin", "/help find").unwrap();
        assert!(prefix.score > loose.score);
        assert_eq!(prefix.indices, vec![1, 2, 3]);
    }

    #[test]
    fn rejects_missing_chars() {
        assert!(fuzzy_match("zzz", "/find").is_none());
    }

    #[test]
    fn empty_needle_matches() {
        assert_eq!(fuzzy_match("", "anything").unwrap().score, 0);
    }
}
