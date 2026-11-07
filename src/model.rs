use unicode_normalization::UnicodeNormalization;

pub fn display_term(input: &str) -> String {
    input
        .nfc()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn normalize_term(input: &str) -> String {
    display_term(input).to_lowercase()
}

#[derive(Debug, Clone)]
pub struct Word {
    pub id: i64,
    pub sync_id: String,
    pub term: String,
    pub definition: String,
    pub example_en: String,
    pub example_zh: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
}

impl Word {
    pub fn is_archived(&self) -> bool {
        self.deleted_at.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct WordRow {
    pub word: Word,
    pub attempts: i64,
    pub avg_ratio: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct WeakWord {
    pub term: String,
    pub avg_ratio: f64,
    pub attempts: i64,
}

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub total_words: i64,
    pub total_attempts: i64,
    pub avg_ratio: Option<f64>,
    pub today_attempts: i64,
    pub streak_days: i64,
    pub weakest: Vec<WeakWord>,
}

#[derive(Debug, Clone, Default)]
pub struct HeaderStats {
    pub words: i64,
    pub today_attempts: i64,
    pub avg_ratio: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Equal,
    Delete,
    Insert,
}

#[derive(Debug, Clone)]
pub struct DiffSpan {
    pub text: String,
    pub emphasized: bool,
}

#[derive(Debug, Clone)]
pub struct DiffRow {
    pub kind: DiffKind,
    pub spans: Vec<DiffSpan>,
}

#[derive(Debug, Clone)]
pub struct DiffResult {
    pub rows: Vec<DiffRow>,
    pub char_ratio: f64,
    pub word_ratio: f64,
    pub exact: bool,
    pub changed_words: usize,
}

#[derive(Debug, Clone, Default)]
pub struct ImportReport {
    pub words_added: usize,
    pub words_updated: usize,
    pub words_skipped: usize,
    pub words_conflicted: usize,
    pub attempts_added: usize,
    pub attempts_skipped: usize,
}
