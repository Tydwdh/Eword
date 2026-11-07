use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time};

use crate::export::{ExportAttempt, ExportFile};
use crate::model::{ImportReport, Stats, WeakWord, Word, WordRow, display_term, normalize_term};
use crate::util::{now_ts, sql_literal, timestamp_slug};

const SCHEMA_VERSION: i64 = 2;

const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS words (
    id          INTEGER PRIMARY KEY,
    term        TEXT NOT NULL,
    term_key    TEXT NOT NULL UNIQUE,
    definition  TEXT NOT NULL,
    example_en  TEXT NOT NULL,
    example_zh  TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    deleted_at  INTEGER
);

CREATE TABLE IF NOT EXISTS attempts (
    id          INTEGER PRIMARY KEY,
    word_id     INTEGER NOT NULL REFERENCES words(id) ON DELETE CASCADE,
    typed       TEXT NOT NULL,
    char_ratio  REAL NOT NULL,
    word_ratio  REAL NOT NULL,
    revealed    INTEGER NOT NULL DEFAULT 0,
    duration_ms INTEGER NOT NULL,
    created_at  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_attempts_word
    ON attempts(word_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_attempts_created
    ON attempts(created_at DESC);
"#;

const SCHEMA_V2: &str = r#"
ALTER TABLE words ADD COLUMN sync_id TEXT;
UPDATE words SET sync_id = lower(hex(randomblob(16))) WHERE sync_id IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_words_sync ON words(sync_id);

ALTER TABLE attempts ADD COLUMN sync_id TEXT;
UPDATE attempts SET sync_id = lower(hex(randomblob(16))) WHERE sync_id IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_attempts_sync ON attempts(sync_id);
"#;

pub struct Db {
    pub conn: Connection,
}

impl Db {
    pub fn open(path: &Path, backup_dir: &Path) -> Result<Self> {
        let existed = path.exists();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating directory {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening database {}", path.display()))?;
        Self::initialize(conn, backup_dir.to_path_buf(), existed)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("opening in-memory database")?;
        Self::initialize(conn, PathBuf::new(), false)
    }

    fn initialize(mut conn: Connection, backup_dir: PathBuf, existed: bool) -> Result<Self> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let _: String = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
        conn.pragma_update(None, "synchronous", "FULL")?;

        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            bail!(
                "database schema v{} is newer than this build supports (v{}); upgrade eword",
                version,
                SCHEMA_VERSION
            );
        }

        if version < SCHEMA_VERSION {
            if existed && version > 0 && !backup_dir.as_os_str().is_empty() {
                fs::create_dir_all(&backup_dir)
                    .with_context(|| format!("creating {}", backup_dir.display()))?;
                let backup =
                    backup_dir.join(format!("eword-pre-v{}-{}.db", version, timestamp_slug()));
                conn.execute_batch(&format!(
                    "VACUUM INTO {}",
                    sql_literal(&backup.to_string_lossy())
                ))
                .with_context(|| format!("backing up database to {}", backup.display()))?;
            }

            let tx = conn.transaction()?;
            if version < 1 {
                tx.execute_batch(SCHEMA_V1)?;
            }
            if version < 2 {
                tx.execute_batch(SCHEMA_V2)?;
            }
            tx.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
            tx.commit()?;
        }

        Ok(Self { conn })
    }

    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?)
    }

    pub fn find_by_key(&self, key: &str) -> Result<Option<Word>> {
        self.conn
            .query_row(
                "SELECT id, term, term_key, definition, example_en, example_zh, \
                 created_at, updated_at, deleted_at, sync_id FROM words WHERE term_key = ?1",
                params![key],
                row_to_word,
            )
            .optional()
            .context("looking up word")
    }

    pub fn find_by_term(&self, term: &str) -> Result<Option<Word>> {
        self.find_by_key(&normalize_term(term))
    }

    pub fn get_word(&self, id: i64) -> Result<Word> {
        self.conn
            .query_row(
                "SELECT id, term, term_key, definition, example_en, example_zh, \
                 created_at, updated_at, deleted_at, sync_id FROM words WHERE id = ?1",
                params![id],
                row_to_word,
            )
            .optional()?
            .with_context(|| format!("word id {id} disappeared"))
    }

    pub fn active_terms(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT term FROM words WHERE deleted_at IS NULL ORDER BY term_key")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn insert_word(
        &mut self,
        term: &str,
        definition: &str,
        example_en: &str,
        example_zh: &str,
    ) -> Result<Word> {
        let term = display_term(term);
        let key = normalize_term(&term);
        validate_word_fields(&term, definition, example_en, example_zh)?;
        if self.find_by_key(&key)?.is_some() {
            bail!("word already exists");
        }
        let now = now_ts();
        self.conn.execute(
            "INSERT INTO words \
             (sync_id, term, term_key, definition, example_en, example_zh, created_at, updated_at) \
             VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![
                term,
                key,
                definition.trim(),
                example_en.trim(),
                example_zh.trim(),
                now
            ],
        )?;
        self.get_word(self.conn.last_insert_rowid())
    }

    pub fn update_word(
        &mut self,
        id: i64,
        term: &str,
        definition: &str,
        example_en: &str,
        example_zh: &str,
    ) -> Result<Word> {
        let term = display_term(term);
        let key = normalize_term(&term);
        validate_word_fields(&term, definition, example_en, example_zh)?;
        let conflict: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM words WHERE term_key = ?1 AND id <> ?2",
                params![key, id],
                |row| row.get(0),
            )
            .optional()?;
        if conflict.is_some() {
            bail!("another word already uses that spelling");
        }
        let changed = self.conn.execute(
            "UPDATE words SET term = ?1, term_key = ?2, definition = ?3, \
             example_en = ?4, example_zh = ?5, \
             updated_at = MAX(?6, updated_at + 1) WHERE id = ?7",
            params![
                term,
                key,
                definition.trim(),
                example_en.trim(),
                example_zh.trim(),
                now_ts(),
                id
            ],
        )?;
        if changed == 0 {
            bail!("word no longer exists");
        }
        self.get_word(id)
    }

    pub fn archive_word(&mut self, id: i64) -> Result<()> {
        let now = now_ts();
        let changed = self.conn.execute(
            "UPDATE words SET deleted_at = MAX(?1, updated_at + 1), \
             updated_at = MAX(?1, updated_at + 1) \
             WHERE id = ?2 AND deleted_at IS NULL",
            params![now, id],
        )?;
        if changed == 0 {
            bail!("word is already archived or missing");
        }
        Ok(())
    }

    pub fn restore_word(&mut self, id: i64) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE words SET deleted_at = NULL, \
             updated_at = MAX(?1, updated_at + 1) \
             WHERE id = ?2 AND deleted_at IS NOT NULL",
            params![now_ts(), id],
        )?;
        if changed == 0 {
            bail!("word is not archived or missing");
        }
        Ok(())
    }

    pub fn add_attempt(
        &mut self,
        word_id: i64,
        typed: &str,
        char_ratio: f64,
        word_ratio: f64,
        revealed: bool,
        duration_ms: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO attempts \
             (word_id, sync_id, typed, char_ratio, word_ratio, revealed, duration_ms, created_at) \
             VALUES (?1, lower(hex(randomblob(16))), ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                word_id,
                typed,
                char_ratio.clamp(0.0, 1.0),
                word_ratio.clamp(0.0, 1.0),
                if revealed { 1i64 } else { 0i64 },
                duration_ms.max(0),
                now_ts()
            ],
        )?;
        Ok(())
    }

    pub fn list_words(&self) -> Result<Vec<WordRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT w.id, w.term, w.term_key, w.definition, w.example_en, \
             w.example_zh, w.created_at, w.updated_at, w.deleted_at, w.sync_id, \
             COUNT(a.id) AS attempts, \
             AVG(CASE WHEN a.revealed = 0 THEN a.char_ratio END) AS avg_ratio \
             FROM words w LEFT JOIN attempts a ON a.word_id = w.id \
             WHERE w.deleted_at IS NULL \
             GROUP BY w.id \
             ORDER BY w.updated_at DESC, w.term_key ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(WordRow {
                word: row_to_word(row)?,
                attempts: row.get(10)?,
                avg_ratio: row.get(11)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn header_stats(&self) -> Result<crate::model::HeaderStats> {
        let words = self.conn.query_row(
            "SELECT COUNT(*) FROM words WHERE deleted_at IS NULL",
            [],
            |row| row.get(0),
        )?;
        let today_start = local_today_start();
        let today_attempts = self.conn.query_row(
            "SELECT COUNT(*) FROM attempts a \
             JOIN words w ON w.id = a.word_id \
             WHERE w.deleted_at IS NULL AND a.created_at >= ?1",
            params![today_start],
            |row| row.get(0),
        )?;
        let avg_ratio = self.conn.query_row(
            "SELECT AVG(a.char_ratio) FROM attempts a \
             JOIN words w ON w.id = a.word_id \
             WHERE w.deleted_at IS NULL AND a.revealed = 0",
            [],
            |row| row.get(0),
        )?;
        Ok(crate::model::HeaderStats {
            words,
            today_attempts,
            avg_ratio,
        })
    }

    pub fn stats(&self) -> Result<Stats> {
        let header = self.header_stats()?;
        let total_attempts = self.conn.query_row(
            "SELECT COUNT(*) FROM attempts a \
             JOIN words w ON w.id = a.word_id WHERE w.deleted_at IS NULL",
            [],
            |row| row.get(0),
        )?;

        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT date(a.created_at, 'unixepoch', 'localtime') AS day \
             FROM attempts a JOIN words w ON w.id = a.word_id \
             WHERE w.deleted_at IS NULL ORDER BY day DESC",
        )?;
        let mut days = HashSet::new();
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            let value = row?;
            if let Some(date) = parse_local_day(&value) {
                days.insert(date);
            }
        }
        let today = OffsetDateTime::now_local()
            .unwrap_or_else(|_| OffsetDateTime::now_utc())
            .date();
        let mut expected = today;
        if !days.contains(&today)
            && let Some(previous) = today.previous_day()
        {
            expected = previous;
        }
        let mut streak_days = 0i64;
        while days.contains(&expected) {
            streak_days += 1;
            match expected.previous_day() {
                Some(previous) => expected = previous,
                None => break,
            }
        }

        let mut stmt = self.conn.prepare(
            "SELECT w.term, AVG(a.char_ratio) AS avg_ratio, COUNT(*) AS attempts \
             FROM attempts a JOIN words w ON w.id = a.word_id \
             WHERE a.revealed = 0 AND w.deleted_at IS NULL \
             GROUP BY w.id HAVING COUNT(*) >= 2 \
             ORDER BY avg_ratio ASC, attempts DESC LIMIT 5",
        )?;
        let weakest = stmt
            .query_map([], |row| {
                Ok(WeakWord {
                    term: row.get(0)?,
                    avg_ratio: row.get(1)?,
                    attempts: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(Stats {
            total_words: header.words,
            total_attempts,
            avg_ratio: header.avg_ratio,
            today_attempts: header.today_attempts,
            streak_days,
            weakest,
        })
    }

    pub fn export_file(&self) -> Result<ExportFile> {
        let mut words = Vec::new();
        let mut stmt = self.conn.prepare(
            "SELECT id, term, term_key, definition, example_en, example_zh, \
             created_at, updated_at, deleted_at, sync_id FROM words ORDER BY id",
        )?;
        let rows = stmt.query_map([], row_to_word)?;
        for row in rows {
            words.push(crate::export::ExportWord::from_word(&row?));
        }

        let mut attempts = Vec::new();
        let mut stmt = self.conn.prepare(
            "SELECT a.sync_id, w.sync_id, w.term, a.typed, a.char_ratio, \
             a.word_ratio, a.revealed, a.duration_ms, a.created_at \
             FROM attempts a JOIN words w ON w.id = a.word_id ORDER BY a.id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ExportAttempt {
                sync_id: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                word_sync_id: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                term: row.get(2)?,
                typed: row.get(3)?,
                char_ratio: row.get(4)?,
                word_ratio: row.get(5)?,
                revealed: row.get::<_, i64>(6)? != 0,
                duration_ms: row.get(7)?,
                created_at: row.get(8)?,
            })
        })?;
        for row in rows {
            attempts.push(row?);
        }

        Ok(ExportFile::new(words, attempts))
    }

    pub fn import_file(&mut self, file: &ExportFile) -> Result<ImportReport> {
        file.validate()?;
        let mut report = ImportReport::default();
        let now = now_ts();
        let tx = self.conn.transaction()?;

        {
            let mut find_sync = tx.prepare(
                "SELECT id, sync_id, term, term_key, definition, example_en, example_zh, \
                 created_at, updated_at, deleted_at FROM words WHERE sync_id = ?1",
            )?;
            let mut find_key = tx.prepare(
                "SELECT id, sync_id, term, term_key, definition, example_en, example_zh, \
                 created_at, updated_at, deleted_at FROM words WHERE term_key = ?1",
            )?;
            let mut find_id_by_key = tx.prepare("SELECT id FROM words WHERE term_key = ?1")?;
            let mut update = tx.prepare(
                "UPDATE words SET term = ?1, term_key = ?2, definition = ?3, \
                 example_en = ?4, example_zh = ?5, created_at = ?6, \
                 updated_at = ?7, deleted_at = ?8, sync_id = ?9 WHERE id = ?10",
            )?;
            let mut insert = tx.prepare(
                "INSERT INTO words \
                 (sync_id, term, term_key, definition, example_en, example_zh, \
                  created_at, updated_at, deleted_at) \
                 VALUES (COALESCE(NULLIF(?1, ''), lower(hex(randomblob(16)))), \
                         ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;

            for word in &file.words {
                let term = display_term(&word.term);
                let key = normalize_term(&term);
                if key.is_empty()
                    || word.definition.trim().is_empty()
                    || word.example_en.trim().is_empty()
                    || word.example_zh.trim().is_empty()
                {
                    report.words_skipped += 1;
                    continue;
                }

                let incoming_created = if word.created_at > 0 {
                    word.created_at
                } else {
                    now
                };
                let incoming_updated = if word.updated_at > 0 {
                    word.updated_at
                } else {
                    incoming_created
                };

                let mut existing = if word.sync_id.is_empty() {
                    None
                } else {
                    find_sync
                        .query_row(params![word.sync_id], row_to_local_word)
                        .optional()?
                };
                if existing.is_none() {
                    existing = find_key
                        .query_row(params![key], row_to_local_word)
                        .optional()?;
                }

                let Some(local) = existing else {
                    insert.execute(params![
                        word.sync_id,
                        term,
                        key,
                        word.definition.trim(),
                        word.example_en.trim(),
                        word.example_zh.trim(),
                        incoming_created,
                        incoming_updated,
                        word.deleted_at
                    ])?;
                    report.words_added += 1;
                    continue;
                };

                let different_identity = !word.sync_id.is_empty()
                    && !local.sync_id.is_empty()
                    && local.sync_id != word.sync_id;

                if different_identity {
                    // The same spelling was created independently on both
                    // devices. Pick one canonical sync ID deterministically;
                    // the other device will adopt it on its next merge.
                    let incoming_wins = incoming_updated > local.updated_at
                        || (incoming_updated == local.updated_at && word.sync_id < local.sync_id);
                    if !incoming_wins {
                        report.words_skipped += 1;
                        continue;
                    }
                } else if incoming_updated < local.updated_at {
                    report.words_skipped += 1;
                    continue;
                }

                let same = local.term == term
                    && local.definition == word.definition.trim()
                    && local.example_en == word.example_en.trim()
                    && local.example_zh == word.example_zh.trim()
                    && local.deleted_at == word.deleted_at;
                let adopt_sync_id = local.sync_id.is_empty() && !word.sync_id.is_empty();
                if !different_identity && incoming_updated == local.updated_at {
                    if same && adopt_sync_id {
                        // fall through to adopt the missing identity
                    } else if same {
                        report.words_skipped += 1;
                        continue;
                    } else {
                        report.words_conflicted += 1;
                        continue;
                    }
                }

                if let Some(other_id) = find_id_by_key
                    .query_row(params![key], |row| row.get::<_, i64>(0))
                    .optional()?
                    && other_id != local.id
                {
                    report.words_conflicted += 1;
                    continue;
                }

                let next_sync_id = if different_identity || adopt_sync_id {
                    word.sync_id.clone()
                } else {
                    local.sync_id.clone()
                };
                update.execute(params![
                    term,
                    key,
                    word.definition.trim(),
                    word.example_en.trim(),
                    word.example_zh.trim(),
                    local.created_at.min(incoming_created),
                    incoming_updated,
                    word.deleted_at,
                    next_sync_id,
                    local.id
                ])?;
                report.words_updated += 1;
            }
        }

        {
            let mut find_word_sync = tx.prepare("SELECT id FROM words WHERE sync_id = ?1")?;
            let mut find_word_key = tx.prepare("SELECT id FROM words WHERE term_key = ?1")?;
            let mut exists_sync = tx.prepare("SELECT 1 FROM attempts WHERE sync_id = ?1")?;
            let mut exists_legacy = tx.prepare(
                "SELECT 1 FROM attempts WHERE word_id = ?1 AND typed = ?2 AND created_at = ?3",
            )?;
            let mut insert = tx.prepare(
                "INSERT INTO attempts \
                 (word_id, sync_id, typed, char_ratio, word_ratio, revealed, duration_ms, created_at) \
                 VALUES (?1, COALESCE(NULLIF(?2, ''), lower(hex(randomblob(16)))), \
                         ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;

            for attempt in &file.attempts {
                let key = normalize_term(&attempt.term);
                let word_id = if !attempt.word_sync_id.is_empty() {
                    find_word_sync
                        .query_row(params![attempt.word_sync_id], |row| row.get::<_, i64>(0))
                        .optional()?
                } else {
                    None
                };
                let word_id = match word_id {
                    Some(id) => Some(id),
                    None if !key.is_empty() => find_word_key
                        .query_row(params![key], |row| row.get::<_, i64>(0))
                        .optional()?,
                    None => None,
                };
                let Some(word_id) = word_id else {
                    report.attempts_skipped += 1;
                    continue;
                };

                let created = if attempt.created_at > 0 {
                    attempt.created_at
                } else {
                    now
                };
                let duplicate = if !attempt.sync_id.is_empty() {
                    exists_sync
                        .query_row(params![attempt.sync_id], |_| Ok(()))
                        .optional()?
                        .is_some()
                } else {
                    exists_legacy
                        .query_row(params![word_id, attempt.typed, created], |_| Ok(()))
                        .optional()?
                        .is_some()
                };
                if duplicate {
                    report.attempts_skipped += 1;
                    continue;
                }

                insert.execute(params![
                    word_id,
                    attempt.sync_id,
                    attempt.typed,
                    attempt.char_ratio.clamp(0.0, 1.0),
                    attempt.word_ratio.clamp(0.0, 1.0),
                    if attempt.revealed { 1i64 } else { 0i64 },
                    attempt.duration_ms.max(0),
                    created
                ])?;
                report.attempts_added += 1;
            }
        }

        tx.commit()?;
        Ok(report)
    }

    pub fn backup_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        self.conn.execute_batch(&format!(
            "VACUUM INTO {}",
            sql_literal(&path.to_string_lossy())
        ))?;
        Ok(())
    }

    pub fn integrity_check(&self) -> Result<String> {
        Ok(self
            .conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))?)
    }
}

fn validate_word_fields(
    term: &str,
    definition: &str,
    example_en: &str,
    example_zh: &str,
) -> Result<()> {
    if term.trim().is_empty() {
        bail!("word cannot be empty");
    }
    if definition.trim().is_empty() {
        bail!("meaning cannot be empty");
    }
    if example_en.trim().is_empty() {
        bail!("example cannot be empty");
    }
    if example_zh.trim().is_empty() {
        bail!("translation cannot be empty");
    }
    Ok(())
}

struct LocalWord {
    id: i64,
    sync_id: String,
    term: String,
    definition: String,
    example_en: String,
    example_zh: String,
    created_at: i64,
    updated_at: i64,
    deleted_at: Option<i64>,
}

fn row_to_local_word(row: &Row<'_>) -> rusqlite::Result<LocalWord> {
    Ok(LocalWord {
        id: row.get(0)?,
        sync_id: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
        term: row.get(2)?,
        definition: row.get(4)?,
        example_en: row.get(5)?,
        example_zh: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        deleted_at: row.get(9)?,
    })
}

fn row_to_word(row: &Row<'_>) -> rusqlite::Result<Word> {
    Ok(Word {
        id: row.get(0)?,
        sync_id: row.get::<_, Option<String>>(9)?.unwrap_or_default(),
        term: row.get(1)?,
        definition: row.get(3)?,
        example_en: row.get(4)?,
        example_zh: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
        deleted_at: row.get(8)?,
    })
}

fn parse_local_day(value: &str) -> Option<Date> {
    let mut parts = value.split('-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u8 = parts.next()?.parse().ok()?;
    let day: u8 = parts.next()?.parse().ok()?;
    Date::from_calendar_date(year, Month::try_from(month).ok()?, day).ok()
}

fn local_today_start() -> i64 {
    let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
    PrimitiveDateTime::new(now.date(), Time::MIDNIGHT)
        .assume_offset(now.offset())
        .unix_timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(db: &mut Db) -> Word {
        db.insert_word(
            "serendipity",
            "意外发现美好事物的能力",
            "He found the letter by sheer serendipity.",
            "机缘巧合之下，他发现了那封信。",
        )
        .unwrap()
    }

    #[test]
    fn insert_lookup_update_archive_restore() {
        let mut db = Db::open_in_memory().unwrap();
        let word = sample(&mut db);
        assert_eq!(
            db.find_by_term("  SERENDIPITY ").unwrap().unwrap().id,
            word.id
        );
        assert!(db.insert_word("Serendipity", "x", "y", "z").is_err());

        let updated = db
            .update_word(
                word.id,
                "serendipity",
                "updated meaning",
                "updated example",
                "updated translation",
            )
            .unwrap();
        assert_eq!(updated.definition, "updated meaning");

        db.archive_word(word.id).unwrap();
        assert!(
            db.find_by_term("serendipity")
                .unwrap()
                .unwrap()
                .is_archived()
        );
        assert!(db.list_words().unwrap().is_empty());
        db.restore_word(word.id).unwrap();
        assert!(
            !db.find_by_term("serendipity")
                .unwrap()
                .unwrap()
                .is_archived()
        );
    }

    #[test]
    fn attempts_feed_header_and_stats() {
        let mut db = Db::open_in_memory().unwrap();
        let word = sample(&mut db);
        db.add_attempt(word.id, "wrong", 0.5, 0.25, false, 1200)
            .unwrap();
        db.add_attempt(word.id, "wrong again", 0.4, 0.2, false, 800)
            .unwrap();
        let stats = db.stats().unwrap();
        assert_eq!(stats.total_words, 1);
        assert_eq!(stats.total_attempts, 2);
        assert_eq!(stats.today_attempts, 2);
        assert_eq!(stats.streak_days, 1);
        assert_eq!(stats.weakest.len(), 1);
    }

    #[test]
    fn export_import_roundtrip_skips_existing_words() {
        let mut source = Db::open_in_memory().unwrap();
        let word = sample(&mut source);
        source
            .add_attempt(word.id, "typed", 0.75, 0.5, false, 900)
            .unwrap();
        let export = source.export_file().unwrap();
        let mut target = Db::open_in_memory().unwrap();
        let report = target.import_file(&export).unwrap();
        assert_eq!(report.words_added, 1);
        assert_eq!(report.attempts_added, 1);
        assert!(target.find_by_term("serendipity").unwrap().is_some());

        let second = target.import_file(&export).unwrap();
        assert_eq!(second.words_skipped, 1);
        assert_eq!(second.attempts_skipped, 1);
    }

    #[test]
    fn term_normalisation_collapses_spaces() {
        assert_eq!(normalize_term("  take   it  easy "), "take it easy");
    }

    #[test]
    fn schema_v2_assigns_stable_ids() {
        let mut db = Db::open_in_memory().unwrap();
        let word = sample(&mut db);
        assert_eq!(word.sync_id.len(), 32);
        db.add_attempt(word.id, "typed", 0.8, 0.7, false, 500)
            .unwrap();
        let attempt_sync: String = db
            .conn
            .query_row("SELECT sync_id FROM attempts LIMIT 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(attempt_sync.len(), 32);
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn import_merges_newer_words_and_tombstones() {
        let mut source = Db::open_in_memory().unwrap();
        let word = sample(&mut source);
        let mut target = Db::open_in_memory().unwrap();
        target.import_file(&source.export_file().unwrap()).unwrap();
        let baseline = target.find_by_term("serendipity").unwrap().unwrap();

        source
            .update_word(
                word.id,
                "serendipity",
                "updated meaning",
                "updated example",
                "updated translation",
            )
            .unwrap();
        source
            .conn
            .execute(
                "UPDATE words SET updated_at = ?1 WHERE id = ?2",
                rusqlite::params![baseline.updated_at + 10, word.id],
            )
            .unwrap();
        let report = target.import_file(&source.export_file().unwrap()).unwrap();
        assert_eq!(report.words_updated, 1);
        let updated = target.find_by_term("serendipity").unwrap().unwrap();
        assert_eq!(updated.definition, "updated meaning");
        assert_eq!(updated.sync_id, baseline.sync_id);

        let deleted_at = updated.updated_at + 10;
        source
            .conn
            .execute(
                "UPDATE words SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
                rusqlite::params![deleted_at, word.id],
            )
            .unwrap();
        let report = target.import_file(&source.export_file().unwrap()).unwrap();
        assert_eq!(report.words_updated, 1);
        assert!(
            target
                .find_by_term("serendipity")
                .unwrap()
                .unwrap()
                .is_archived()
        );

        source
            .conn
            .execute(
                "UPDATE words SET deleted_at = NULL, updated_at = ?1 WHERE id = ?2",
                rusqlite::params![deleted_at + 10, word.id],
            )
            .unwrap();
        let report = target.import_file(&source.export_file().unwrap()).unwrap();
        assert_eq!(report.words_updated, 1);
        assert!(
            !target
                .find_by_term("serendipity")
                .unwrap()
                .unwrap()
                .is_archived()
        );
    }

    #[test]
    fn equal_timestamp_conflict_is_reported_not_overwritten() {
        let mut source = Db::open_in_memory().unwrap();
        let word = sample(&mut source);
        let mut target = Db::open_in_memory().unwrap();
        target.import_file(&source.export_file().unwrap()).unwrap();

        source
            .conn
            .execute(
                "UPDATE words SET definition = 'conflicting' WHERE id = ?1",
                rusqlite::params![word.id],
            )
            .unwrap();
        let report = target.import_file(&source.export_file().unwrap()).unwrap();
        assert_eq!(report.words_conflicted, 1);
        assert_eq!(
            target
                .find_by_term("serendipity")
                .unwrap()
                .unwrap()
                .definition,
            "意外发现美好事物的能力"
        );
    }

    #[test]
    fn independently_created_same_term_converges_on_one_id() {
        let mut source = Db::open_in_memory().unwrap();
        let source_word = sample(&mut source);
        let mut target = Db::open_in_memory().unwrap();
        let target_word = target
            .insert_word(
                "serendipity",
                "另一个定义",
                "Another example.",
                "另一个例句。",
            )
            .unwrap();
        assert_ne!(source_word.sync_id, target_word.sync_id);

        source
            .conn
            .execute("UPDATE words SET updated_at = 100", [])
            .unwrap();
        target
            .conn
            .execute("UPDATE words SET updated_at = 100", [])
            .unwrap();
        let expected = source_word.sync_id.min(target_word.sync_id.clone());

        target.import_file(&source.export_file().unwrap()).unwrap();
        let target_after = target.find_by_term("serendipity").unwrap().unwrap();
        assert_eq!(target_after.sync_id, expected);

        source.import_file(&target.export_file().unwrap()).unwrap();
        let source_after = source.find_by_term("serendipity").unwrap().unwrap();
        assert_eq!(source_after.sync_id, expected);
    }

    #[test]
    fn import_follows_sync_id_across_rename() {
        let mut source = Db::open_in_memory().unwrap();
        let word = sample(&mut source);
        source
            .add_attempt(word.id, "typed", 0.8, 0.7, false, 500)
            .unwrap();
        let mut target = Db::open_in_memory().unwrap();
        target.import_file(&source.export_file().unwrap()).unwrap();

        source
            .update_word(
                word.id,
                "renamed serendipity",
                "updated meaning",
                "updated example",
                "updated translation",
            )
            .unwrap();
        source
            .conn
            .execute(
                "UPDATE words SET updated_at = ?1 WHERE id = ?2",
                rusqlite::params![now_ts() + 10, word.id],
            )
            .unwrap();
        let report = target.import_file(&source.export_file().unwrap()).unwrap();
        assert_eq!(report.words_updated, 1);
        let renamed = target.find_by_term("renamed serendipity").unwrap().unwrap();
        assert_eq!(renamed.sync_id, word.sync_id);
        assert_eq!(renamed.definition, "updated meaning");
        let attempts: i64 = target
            .conn
            .query_row("SELECT COUNT(*) FROM attempts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(attempts, 1);
    }

    #[test]
    fn migrates_version_one_database_and_backs_it_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("eword.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA_V1).unwrap();
            conn.execute(
                "INSERT INTO words \
                 (term, term_key, definition, example_en, example_zh, created_at, updated_at) \
                 VALUES ('legacy', 'legacy', '旧释义', 'Example.', '例句。', 1, 1)",
                [],
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
        }

        let backups = dir.path().join("backups");
        let db = Db::open(&path, &backups).unwrap();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
        let word = db.find_by_term("legacy").unwrap().unwrap();
        assert!(!word.sync_id.is_empty());
        assert_eq!(
            fs::read_dir(&backups).unwrap().count(),
            1,
            "migration should create a pre-v1 backup"
        );
    }

    #[test]
    fn imports_version_one_snapshot_without_sync_ids() {
        let json = r#"{
            "format": "eword",
            "version": 1,
            "exported_at": "2026-01-01T00:00:00Z",
            "words": [{
                "term": "legacy",
                "definition": "旧释义",
                "example_en": "A legacy example.",
                "example_zh": "一个旧例句。",
                "created_at": 1,
                "updated_at": 1
            }],
            "attempts": []
        }"#;
        let file: ExportFile = serde_json::from_str(json).unwrap();
        let mut db = Db::open_in_memory().unwrap();
        let report = db.import_file(&file).unwrap();
        assert_eq!(report.words_added, 1);
        let word = db.find_by_term("legacy").unwrap().unwrap();
        assert!(!word.sync_id.is_empty());
        let again = db.import_file(&file).unwrap();
        assert_eq!(again.words_skipped, 1);
    }
}
