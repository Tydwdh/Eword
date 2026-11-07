use similar::{ChangeTag, TextDiff};

use crate::model::{DiffKind, DiffResult, DiffRow, DiffSpan};

pub fn compute(typed: &str, target: &str) -> DiffResult {
    let typed = typed.trim_end();
    let target = target.trim_end();

    let chars = TextDiff::from_chars(typed, target);
    let char_ratio = chars.ratio() as f64;

    let words = TextDiff::from_unicode_words(typed, target);
    let mut equal_tokens = 0usize;
    let mut total_tokens = 0usize;
    let mut changed_words = 0usize;
    for change in words.iter_all_changes() {
        let value = change.value();
        if value.trim().is_empty() {
            continue;
        }
        total_tokens += 1;
        match change.tag() {
            ChangeTag::Equal => equal_tokens += 1,
            ChangeTag::Delete | ChangeTag::Insert => changed_words += 1,
        }
    }
    let word_ratio = if total_tokens == 0 {
        1.0
    } else {
        equal_tokens as f64 / total_tokens as f64
    };

    let lines = TextDiff::from_lines(typed, target);
    let rows = lines
        .iter_all_inline_changes()
        .map(|change| DiffRow {
            kind: match change.tag() {
                ChangeTag::Equal => DiffKind::Equal,
                ChangeTag::Delete => DiffKind::Delete,
                ChangeTag::Insert => DiffKind::Insert,
            },
            spans: change
                .iter_strings_lossy()
                .map(|(emphasized, text)| DiffSpan {
                    text: text.into_owned(),
                    emphasized,
                })
                .collect(),
        })
        .collect();

    DiffResult {
        rows,
        char_ratio,
        word_ratio,
        exact: typed == target,
        changed_words,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_copy_is_exact() {
        let result = compute("He found the letter.", "He found the letter.");
        assert!(result.exact);
        assert_eq!(result.char_ratio, 1.0);
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].kind, DiffKind::Equal);
    }

    #[test]
    fn typo_produces_delete_and_insert_rows() {
        let result = compute("He recieved the letter.", "He received the letter.");
        assert!(!result.exact);
        assert!(result.char_ratio < 1.0);
        assert!(result.rows.iter().any(|row| row.kind == DiffKind::Delete));
        assert!(result.rows.iter().any(|row| row.kind == DiffKind::Insert));
        assert!(result.rows.iter().any(|row| {
            row.spans
                .iter()
                .any(|span| span.emphasized && span.text == "recieved")
        }));
    }

    #[test]
    fn missing_words_are_highlighted() {
        let result = compute(
            "He found the letter by luck.",
            "He found the letter by sheer serendipity.",
        );
        assert!(!result.exact);
        assert!(result.changed_words >= 2);
        assert!(result.rows.iter().any(|row| row.kind == DiffKind::Insert));
    }
}
