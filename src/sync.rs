use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use crate::config::{self, Config, Paths};
use crate::db::Db;
use crate::export;
use crate::model::ImportReport;
use crate::util::timestamp_slug;

#[derive(Debug, Clone, Default)]
pub struct SyncReport {
    pub files: usize,
    pub words_added: usize,
    pub words_updated: usize,
    pub words_skipped: usize,
    pub words_conflicted: usize,
    pub attempts_added: usize,
    pub attempts_skipped: usize,
    pub exported: PathBuf,
    pub git_pulled: bool,
    pub git_committed: bool,
    pub git_pushed: bool,
}

impl SyncReport {
    pub fn summary(&self) -> String {
        let mut text = format!(
            "synced · {} files · +{} words · {} updated · +{} attempts",
            self.files, self.words_added, self.words_updated, self.attempts_added
        );
        if self.words_conflicted > 0 {
            text.push_str(&format!(" · {} conflicts", self.words_conflicted));
        }
        text
    }
}

pub fn run(
    db: &mut Db,
    config: &mut Config,
    paths: &Paths,
    dir: &Path,
    use_git: bool,
) -> Result<SyncReport> {
    let device = ensure_device_id(config, paths)?;
    let devices_dir = dir.join("devices");
    fs::create_dir_all(&devices_dir)
        .with_context(|| format!("creating sync directory {}", devices_dir.display()))?;

    let mut report = SyncReport::default();
    if use_git {
        git_pull(dir)?;
        report.git_pulled = true;
    }

    let own_name = format!("{device}.json");
    let own_path = devices_dir.join(&own_name);
    let mut snapshots = Vec::new();
    for entry in fs::read_dir(&devices_dir)
        .with_context(|| format!("reading sync directory {}", devices_dir.display()))?
    {
        let path = entry?.path();
        if path.extension() == Some(OsStr::new("json")) {
            snapshots.push(path);
        }
    }
    snapshots.sort();

    let mut files = Vec::new();
    for path in snapshots {
        let file = export::read(&path)
            .with_context(|| format!("reading sync snapshot {}", path.display()))?;
        files.push((path, file));
    }

    for (path, file) in &files {
        let is_own = path.file_name() == Some(OsStr::new(&own_name));
        let result = db
            .import_file(file)
            .with_context(|| format!("merging sync snapshot {}", path.display()))?;
        if !is_own {
            add_import(&mut report, &result);
            report.files += 1;
        }
    }

    let existing = if own_path.exists() {
        export::read(&own_path).ok()
    } else {
        None
    };
    let snapshot = db.export_file()?;
    let changed = existing
        .as_ref()
        .is_none_or(|old| !old.same_snapshot(&snapshot));
    if changed {
        export::write(&own_path, &snapshot)
            .with_context(|| format!("writing sync snapshot {}", own_path.display()))?;
    }
    report.exported = own_path.clone();

    if use_git {
        let (committed, pushed) = git_commit_push(dir, &own_path)?;
        report.git_committed = committed;
        report.git_pushed = pushed;
    }

    Ok(report)
}

fn add_import(report: &mut SyncReport, import: &ImportReport) {
    report.words_added += import.words_added;
    report.words_updated += import.words_updated;
    report.words_skipped += import.words_skipped;
    report.words_conflicted += import.words_conflicted;
    report.attempts_added += import.attempts_added;
    report.attempts_skipped += import.attempts_skipped;
}

fn ensure_device_id(config: &mut Config, paths: &Paths) -> Result<String> {
    if let Some(raw) = &config.device_id {
        let id = sanitize_id(raw);
        if id.is_empty() {
            bail!("device_id in config.toml is empty");
        }
        if config.device_id.as_deref() != Some(id.as_str()) {
            config.device_id = Some(id.clone());
            config::save(&paths.config, config)?;
        }
        return Ok(id);
    }

    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "device".to_string());
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let id = format!("{}-{:x}", sanitize_id(&host), nanos as u64);
    config.device_id = Some(id.clone());
    config::save(&paths.config, config)?;
    Ok(id)
}

fn sanitize_id(raw: &str) -> String {
    let mut id = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            id.push(ch.to_ascii_lowercase());
        } else if (ch == '-' || ch == '_') && !id.is_empty() && !id.ends_with('-') {
            id.push(ch);
        } else if !id.is_empty() && !id.ends_with('-') {
            id.push('-');
        }
    }
    let id = id.trim_matches('-').to_string();
    let id = id.chars().take(48).collect::<String>();
    if id.is_empty() {
        "device".to_string()
    } else {
        id
    }
}

fn git_pull(dir: &Path) -> Result<()> {
    let output = git(dir, &["pull", "--rebase", "--autostash"])?;
    require_git(output, "git pull")?;
    Ok(())
}

fn git_commit_push(dir: &Path, own_path: &Path) -> Result<(bool, bool)> {
    let filename = own_path
        .file_name()
        .and_then(OsStr::to_str)
        .context("sync snapshot has no filename")?;
    let relative = format!("devices/{filename}");

    require_git(git(dir, &["add", "--", &relative])?, "git add")?;

    let staged = git(dir, &["diff", "--cached", "--quiet", "--", &relative])?;
    let committed = match staged.status.code() {
        Some(0) => false,
        Some(1) => {
            let message = format!(
                "sync: {} {}",
                filename.trim_end_matches(".json"),
                timestamp_slug()
            );
            require_git(
                git(dir, &["commit", "-m", &message, "--", &relative])?,
                "git commit",
            )?;
            true
        }
        _ => {
            return Err(git_error(&staged, "git diff --cached"));
        }
    };

    let mut pushed = git(dir, &["push"])?;
    if !pushed.status.success() {
        // A concurrent device push is the common failure here. Rebase once,
        // then try again. The next sync will merge whatever that pull brought
        // in, so this step only needs to publish the local snapshot.
        git_pull(dir)?;
        pushed = git(dir, &["push"])?;
    }
    require_git(pushed, "git push")?;
    Ok((committed, true))
}

fn git(dir: &Path, args: &[&str]) -> Result<Output> {
    Command::new("git")
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .env("GIT_SEQUENCE_EDITOR", "true")
        .args(args)
        .output()
        .with_context(|| format!("running git in {}", dir.display()))
}

fn require_git(output: Output, action: &str) -> Result<Output> {
    if output.status.success() {
        Ok(output)
    } else {
        Err(git_error(&output, action))
    }
}

fn git_error(output: &Output, action: &str) -> anyhow::Error {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    anyhow::anyhow!("{action} failed: {detail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Paths};
    use crate::db::Db;

    fn setup(root: &Path, device: &str) -> (Db, Config, Paths) {
        let paths = Paths::discover(Some(root.to_path_buf())).unwrap();
        let db = Db::open(&paths.db, &paths.backups).unwrap();
        let config = Config {
            device_id: Some(device.to_string()),
            ..Config::default()
        };
        (db, config, paths)
    }

    #[test]
    fn own_snapshot_restores_an_empty_database() {
        let temp = tempfile::tempdir().unwrap();
        let sync_dir = temp.path().join("sync");
        let (mut db, mut config, paths) = setup(&temp.path().join("a"), "device-a");
        db.insert_word("restore", "恢复", "Restored example.", "恢复的例句。")
            .unwrap();
        run(&mut db, &mut config, &paths, &sync_dir, false).unwrap();

        let (mut restored, mut restored_config, restored_paths) =
            setup(&temp.path().join("a-new"), "device-a");
        run(
            &mut restored,
            &mut restored_config,
            &restored_paths,
            &sync_dir,
            false,
        )
        .unwrap();
        let word = restored.find_by_term("restore").unwrap().unwrap();
        assert_eq!(word.definition, "恢复");
    }

    #[test]
    fn two_devices_exchange_words_attempts_and_updates() {
        let temp = tempfile::tempdir().unwrap();
        let sync_dir = temp.path().join("sync");
        let (mut db_a, mut config_a, paths_a) = setup(&temp.path().join("a"), "device-a");
        let (mut db_b, mut config_b, paths_b) = setup(&temp.path().join("b"), "device-b");

        let word = db_a
            .insert_word(
                "serendipity",
                "意外发现美好事物的能力",
                "He found the letter by sheer serendipity.",
                "机缘巧合之下，他发现了那封信。",
            )
            .unwrap();
        db_a.add_attempt(word.id, "typed", 0.8, 0.7, false, 500)
            .unwrap();

        let first_a = run(&mut db_a, &mut config_a, &paths_a, &sync_dir, false).unwrap();
        assert_eq!(first_a.files, 0);
        assert_eq!(first_a.words_added, 0);

        let first_b = run(&mut db_b, &mut config_b, &paths_b, &sync_dir, false).unwrap();
        assert_eq!(first_b.files, 1);
        assert_eq!(first_b.words_added, 1);
        assert_eq!(first_b.attempts_added, 1);

        let b_word = db_b.find_by_term("serendipity").unwrap().unwrap();
        assert_eq!(b_word.sync_id, word.sync_id);
        db_b.update_word(
            b_word.id,
            "serendipity",
            "B updated meaning",
            "B updated example",
            "B updated translation",
        )
        .unwrap();
        db_b.conn
            .execute(
                "UPDATE words SET updated_at = ?1 WHERE id = ?2",
                rusqlite::params![word.updated_at + 10, b_word.id],
            )
            .unwrap();
        db_b.add_attempt(b_word.id, "typed on b", 0.6, 0.5, false, 700)
            .unwrap();

        run(&mut db_b, &mut config_b, &paths_b, &sync_dir, false).unwrap();
        let second_a = run(&mut db_a, &mut config_a, &paths_a, &sync_dir, false).unwrap();
        assert_eq!(second_a.files, 1);
        assert_eq!(second_a.words_updated, 1);
        assert_eq!(second_a.attempts_added, 1);

        let a_word = db_a.find_by_term("serendipity").unwrap().unwrap();
        assert_eq!(a_word.definition, "B updated meaning");
        let attempts: i64 = db_a
            .conn
            .query_row("SELECT COUNT(*) FROM attempts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(attempts, 2);

        let tombstone = a_word.updated_at + 20;
        db_a.conn
            .execute(
                "UPDATE words SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
                rusqlite::params![tombstone, a_word.id],
            )
            .unwrap();
        run(&mut db_a, &mut config_a, &paths_a, &sync_dir, false).unwrap();
        run(&mut db_b, &mut config_b, &paths_b, &sync_dir, false).unwrap();
        assert!(
            db_b.find_by_term("serendipity")
                .unwrap()
                .unwrap()
                .is_archived()
        );
    }
}
