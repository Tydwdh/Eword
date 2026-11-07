use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

use crate::model::Word;
use crate::util::{atomic_write, rfc3339_now};

pub const FORMAT: &str = "eword";
pub const VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportFile {
    pub format: String,
    pub version: u32,
    pub exported_at: String,
    pub words: Vec<ExportWord>,
    pub attempts: Vec<ExportAttempt>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportWord {
    #[serde(default)]
    pub sync_id: String,
    pub term: String,
    pub definition: String,
    pub example_en: String,
    pub example_zh: String,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportAttempt {
    #[serde(default)]
    pub sync_id: String,
    #[serde(default)]
    pub word_sync_id: String,
    pub term: String,
    pub typed: String,
    pub char_ratio: f64,
    pub word_ratio: f64,
    #[serde(default)]
    pub revealed: bool,
    pub duration_ms: i64,
    pub created_at: i64,
}

impl ExportFile {
    pub fn new(words: Vec<ExportWord>, attempts: Vec<ExportAttempt>) -> Self {
        Self {
            format: FORMAT.to_string(),
            version: VERSION,
            exported_at: rfc3339_now(),
            words,
            attempts,
        }
    }

    pub fn same_snapshot(&self, other: &Self) -> bool {
        self.format == other.format
            && self.version == other.version
            && self.words == other.words
            && self.attempts == other.attempts
    }

    pub fn validate(&self) -> Result<()> {
        if self.format != FORMAT {
            bail!("not an eword export (format = {:?})", self.format);
        }
        if self.version > VERSION {
            bail!(
                "export version {} is newer than this build supports ({})",
                self.version,
                VERSION
            );
        }
        for (index, word) in self.words.iter().enumerate() {
            if word.term.trim().is_empty() {
                bail!("word #{} has an empty term", index + 1);
            }
            if word.definition.trim().is_empty() {
                bail!("word {:?} has an empty meaning", word.term);
            }
            if word.example_en.trim().is_empty() {
                bail!("word {:?} has an empty example", word.term);
            }
            if word.example_zh.trim().is_empty() {
                bail!("word {:?} has an empty translation", word.term);
            }
        }
        for (index, attempt) in self.attempts.iter().enumerate() {
            if attempt.term.trim().is_empty() {
                bail!("attempt #{} has an empty word", index + 1);
            }
            if !(0.0..=1.0).contains(&attempt.char_ratio)
                || !(0.0..=1.0).contains(&attempt.word_ratio)
                || !attempt.char_ratio.is_finite()
                || !attempt.word_ratio.is_finite()
            {
                bail!("attempt #{} has an invalid ratio", index + 1);
            }
        }
        Ok(())
    }
}

impl ExportWord {
    pub fn from_word(word: &Word) -> Self {
        Self {
            sync_id: word.sync_id.clone(),
            term: word.term.clone(),
            definition: word.definition.clone(),
            example_en: word.example_en.clone(),
            example_zh: word.example_zh.clone(),
            created_at: word.created_at,
            updated_at: word.updated_at,
            deleted_at: word.deleted_at,
        }
    }
}

pub fn write(path: &Path, export: &ExportFile) -> Result<()> {
    export.validate()?;
    let bytes = serde_json::to_vec_pretty(export).context("serializing export")?;
    atomic_write(path, &bytes)
}

pub fn read(path: &Path) -> Result<ExportFile> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let export: ExportFile = serde_json::from_str(&text)
        .with_context(|| format!("parsing JSON from {}", path.display()))?;
    export.validate()?;
    Ok(export)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_wrong_format() {
        let mut file = ExportFile::new(Vec::new(), Vec::new());
        file.format = "other".to_string();
        assert!(file.validate().is_err());
    }

    #[test]
    fn same_snapshot_ignores_export_timestamp() {
        let a = ExportFile::new(Vec::new(), Vec::new());
        let mut b = a.clone();
        b.exported_at = "2026-01-01T00:00:00Z".to_string();
        assert!(a.same_snapshot(&b));
    }

    #[test]
    fn rejects_newer_version() {
        let mut file = ExportFile::new(Vec::new(), Vec::new());
        file.version = VERSION + 1;
        assert!(file.validate().is_err());
    }
}
