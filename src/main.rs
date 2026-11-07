mod app;
mod commands;
mod config;
mod db;
mod diff;
mod export;
mod fuzzy;
mod ime;
mod model;
mod sync;
mod ui;
mod util;
mod win;

use std::io::{self, IsTerminal};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use ratatui::crossterm::execute;

use crate::app::App;
use crate::config::{Config, Paths};
use crate::db::Db;
use crate::ui::theme::Theme;

#[derive(Parser)]
#[command(
    name = "eword",
    version,
    about = "A minimal terminal vocabulary spelling trainer"
)]
struct Cli {
    /// Directory for the database, config and backups.
    #[arg(long, global = true, value_name = "DIR")]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Write a portable JSON snapshot.
    Export {
        #[arg(long, value_name = "FILE")]
        output: Option<PathBuf>,
    },
    /// Merge a JSON snapshot into the database.
    Import { path: PathBuf },
    /// Print progress without starting the TUI.
    Stats,
    /// Write a consistent SQLite snapshot.
    Backup {
        #[arg(long, value_name = "FILE")]
        output: Option<PathBuf>,
    },
    /// Merge and publish snapshots through a sync directory.
    Sync {
        /// Sync directory. Overrides sync_dir from config.toml.
        #[arg(long, value_name = "DIR")]
        dir: Option<PathBuf>,
        /// Run git pull and git push around the merge.
        #[arg(long)]
        git: bool,
    },
    /// Print paths, schema version and database integrity.
    Doctor,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::discover(cli.data_dir)?;
    let mut config = config::load(&paths.config)?;

    match cli.command {
        None => run_tui(paths, config),
        Some(Cmd::Export { output }) => {
            let db = open_db(&paths)?;
            let path = output.unwrap_or_else(|| paths.default_export());
            let file = db.export_file()?;
            export::write(&path, &file)?;
            println!("{}", path.display());
            Ok(())
        }
        Some(Cmd::Import { path }) => {
            let mut db = open_db(&paths)?;
            let file = export::read(&path)?;
            let report = db.import_file(&file)?;
            println!(
                "words: +{} updated {} ({} conflicts, {} skipped)  attempts: +{} ({} skipped)",
                report.words_added,
                report.words_updated,
                report.words_conflicted,
                report.words_skipped,
                report.attempts_added,
                report.attempts_skipped
            );
            Ok(())
        }
        Some(Cmd::Stats) => {
            let db = open_db(&paths)?;
            print_stats(&db)
        }
        Some(Cmd::Backup { output }) => {
            let db = open_db(&paths)?;
            let path = output.unwrap_or_else(|| paths.default_backup());
            db.backup_to(&path)?;
            println!("{}", path.display());
            Ok(())
        }
        Some(Cmd::Sync { dir, git }) => {
            let mut db = open_db(&paths)?;
            let mut save = false;
            let dir = match dir {
                Some(dir) => {
                    if config.sync_dir.as_ref() != Some(&dir) {
                        config.sync_dir = Some(dir.clone());
                        save = true;
                    }
                    dir
                }
                None => config
                    .sync_dir
                    .clone()
                    .context("no sync directory; pass --dir or set sync_dir in config.toml")?,
            };
            if git && !config.sync_git {
                config.sync_git = true;
                save = true;
            }
            if save {
                config::save(&paths.config, &config)?;
            }
            let use_git = git || config.sync_git;
            let report = sync::run(&mut db, &mut config, &paths, &dir, use_git)?;
            print_sync(&report);
            Ok(())
        }
        Some(Cmd::Doctor) => {
            let db = open_db(&paths)?;
            print_doctor(&paths, &db, &config)
        }
    }
}

fn print_sync(report: &sync::SyncReport) {
    println!("snapshot        {}", report.exported.display());
    println!("files merged    {}", report.files);
    println!(
        "words           +{} updated {} skipped {} conflicts {}",
        report.words_added, report.words_updated, report.words_skipped, report.words_conflicted
    );
    println!(
        "attempts        +{} skipped {}",
        report.attempts_added, report.attempts_skipped
    );
    if report.git_pulled {
        println!("git pull        ok");
    }
    if report.git_committed {
        println!("git commit      ok");
    }
    if report.git_pushed {
        println!("git push        ok");
    }
}

fn open_db(paths: &Paths) -> Result<Db> {
    Db::open(&paths.db, &paths.backups)
        .with_context(|| format!("opening database {}", paths.db.display()))
}

fn run_tui(paths: Paths, config: Config) -> Result<()> {
    if !io::stdout().is_terminal() {
        bail!("eword needs an interactive terminal; try `eword --help` for CLI commands");
    }

    let theme = Theme::from_config(&config);
    let db = open_db(&paths)?;
    let mut app = App::new(db, config, paths.clone(), theme)?;
    let log_path = paths.log.clone();

    ratatui::run(|terminal| {
        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            log_panic(&log_path, info);
            previous_hook(info);
        }));

        let _ime = win::ConsoleModeGuard::ensure_ime();
        let _extras = TerminalExtras::enable();
        app.run(terminal).map_err(io::Error::other)
    })?;
    Ok(())
}

struct TerminalExtras;

impl TerminalExtras {
    fn enable() -> Self {
        let _ = execute!(
            io::stdout(),
            EnableBracketedPaste,
            SetCursorStyle::SteadyBar
        );
        Self
    }
}

impl Drop for TerminalExtras {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            SetCursorStyle::DefaultUserShape
        );
    }
}

fn log_panic(path: &std::path::Path, info: &std::panic::PanicHookInfo<'_>) {
    use std::io::Write;
    let message = if let Some(value) = info.payload().downcast_ref::<&str>() {
        (*value).to_string()
    } else if let Some(value) = info.payload().downcast_ref::<String>() {
        value.clone()
    } else {
        "panic with non-string payload".to_string()
    };
    let location = info
        .location()
        .map(|location| format!("{}:{}", location.file(), location.line()))
        .unwrap_or_else(|| "unknown location".to_string());
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(
            file,
            "{} panic at {}: {}",
            util::rfc3339_now(),
            location,
            message
        );
    }
}

fn print_stats(db: &Db) -> Result<()> {
    let stats = db.stats()?;
    println!("words           {}", stats.total_words);
    println!("attempts        {}", stats.total_attempts);
    match stats.avg_ratio {
        Some(ratio) => println!("blind accuracy  {:.0}%", ratio * 100.0),
        None => println!("blind accuracy  —"),
    }
    println!("streak          {} days", stats.streak_days);
    println!("today           {} attempts", stats.today_attempts);
    if !stats.weakest.is_empty() {
        println!("weakest");
        for weak in stats.weakest {
            println!(
                "  {:<24} {:>3.0}%  {} attempts",
                weak.term,
                weak.avg_ratio * 100.0,
                weak.attempts
            );
        }
    }
    Ok(())
}

fn print_doctor(paths: &Paths, db: &Db, config: &Config) -> Result<()> {
    println!("data dir        {}", paths.root.display());
    println!("database        {}", paths.db.display());
    println!("config          {}", paths.config.display());
    println!("backups         {}", paths.backups.display());
    if let Some(dir) = &config.sync_dir {
        println!("sync dir        {}", dir.display());
    }
    if let Some(device) = &config.device_id {
        println!("device id       {device}");
    }
    if config.sync_git {
        println!("git sync        on");
    }
    if config.sync_dir.is_some() {
        println!(
            "auto sync       {}",
            if config.auto_sync { "on" } else { "off" }
        );
    }
    println!("schema          v{}", db.schema_version()?);
    println!("integrity       {}", db.integrity_check()?);
    let header = db.header_stats()?;
    println!("words           {}", header.words);
    println!("today           {} attempts", header.today_attempts);
    if let Some(ratio) = header.avg_ratio {
        println!("blind accuracy  {:.0}%", ratio * 100.0);
    }
    if let Ok(metadata) = std::fs::metadata(&paths.db) {
        println!("db size         {}", human_bytes(metadata.len()));
    }
    println!(
        "terminal        {}",
        if io::stdout().is_terminal() {
            "interactive"
        } else {
            "not a tty"
        }
    );
    Ok(())
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
