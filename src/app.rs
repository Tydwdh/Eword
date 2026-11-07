use std::cmp::Ordering;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::TableState;
use ratatui_textarea::{TextArea, WrapMode};

use crate::commands::{self, CommandId};
use crate::config::{self, Config, Paths};
use crate::db::Db;
use crate::diff;
use crate::export;
use crate::fuzzy::fuzzy_match;
use crate::ime::{ImeController, ImeLang};
use crate::model::{DiffResult, HeaderStats, Stats, Word, WordRow, display_term, normalize_term};
use crate::sync;
use crate::ui;
use crate::ui::theme::Theme;

const TOAST_MS: u64 = 2600;

#[derive(Debug, Clone)]
pub enum FormReturn {
    Home,
    Browse(String),
}

pub enum Screen {
    Empty,
    Home(HomeState),
    Form(FormState),
    Review(ReviewState),
    Browse(BrowseState),
    Stats(StatsState),
    Help(HelpState),
}

pub struct HomeState {
    pub input: TextArea<'static>,
    pub palette_open: bool,
    pub palette_selected: usize,
}

impl HomeState {
    pub fn new() -> Self {
        Self {
            input: make_textarea("type a word…", WrapMode::None),
            palette_open: false,
            palette_selected: 0,
        }
    }
}

pub struct FormState {
    pub fields: Vec<TextArea<'static>>,
    pub focus: usize,
    pub editing_id: Option<i64>,
    pub return_to: FormReturn,
    pub dirty: bool,
    pub error: Option<String>,
}

impl FormState {
    pub fn new_new(term: Option<&str>, return_to: FormReturn) -> Self {
        let fields = vec![
            make_textarea("word", WrapMode::None),
            make_textarea("meaning", WrapMode::Word),
            make_textarea("English example", WrapMode::Word),
            make_textarea("example translation", WrapMode::Word),
        ];
        let mut form = Self {
            fields,
            focus: 0,
            editing_id: None,
            return_to,
            dirty: false,
            error: None,
        };
        if let Some(term) = term {
            set_text(&mut form.fields[0], term);
            form.focus = 1;
        }
        form
    }

    pub fn new_edit(word: &Word, return_to: FormReturn) -> Self {
        let mut form = Self {
            fields: vec![
                make_textarea("word", WrapMode::None),
                make_textarea("meaning", WrapMode::Word),
                make_textarea("English example", WrapMode::Word),
                make_textarea("example translation", WrapMode::Word),
            ],
            focus: 0,
            editing_id: Some(word.id),
            return_to,
            dirty: false,
            error: None,
        };
        set_text(&mut form.fields[0], &word.term);
        set_text(&mut form.fields[1], &word.definition);
        set_text(&mut form.fields[2], &word.example_en);
        set_text(&mut form.fields[3], &word.example_zh);
        form
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewPhase {
    Prompt,
    Result,
}

pub struct ReviewState {
    pub word: Word,
    pub answer: TextArea<'static>,
    pub phase: ReviewPhase,
    pub result: Option<DiffResult>,
    pub seen_target: bool,
    pub started_at: Instant,
    pub scroll: u16,
}

impl ReviewState {
    pub fn new(word: Word) -> Self {
        Self {
            word,
            answer: make_textarea("write the English example…", WrapMode::Word),
            phase: ReviewPhase::Prompt,
            result: None,
            seen_target: false,
            started_at: Instant::now(),
            scroll: 0,
        }
    }
}

pub struct BrowseState {
    pub all: Vec<WordRow>,
    pub query: TextArea<'static>,
    pub selected: usize,
    pub table: TableState,
}

impl BrowseState {
    pub fn new(all: Vec<WordRow>) -> Self {
        let mut table = TableState::default();
        table.select(if all.is_empty() { None } else { Some(0) });
        Self {
            all,
            query: make_textarea("search term or meaning…", WrapMode::None),
            selected: 0,
            table,
        }
    }

    pub fn filtered_indices(&self) -> Vec<usize> {
        let query = single_line_text(&self.query).trim().to_lowercase();
        if query.is_empty() {
            return (0..self.all.len()).collect();
        }

        let mut hits: Vec<(i32, usize)> = Vec::new();
        for (index, row) in self.all.iter().enumerate() {
            let term = row.word.term.to_lowercase();
            let definition = row.word.definition.to_lowercase();
            let score = if term == query {
                1200
            } else if term.starts_with(&query) {
                900
            } else if term.contains(&query) {
                700
            } else if definition.contains(&query) {
                500
            } else {
                fuzzy_match(&query, &term)
                    .map(|matched| matched.score)
                    .unwrap_or(0)
            };
            if score > 0 {
                hits.push((score, index));
            }
        }
        hits.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| self.all[a.1].word.term.cmp(&self.all[b.1].word.term))
        });
        hits.into_iter().map(|(_, index)| index).collect()
    }

    pub fn reset_selection(&mut self) {
        self.selected = 0;
        self.table = TableState::default();
        let length = self.filtered_indices().len();
        self.table.select(if length == 0 { None } else { Some(0) });
    }

    pub fn clamp_selection(&mut self) {
        let length = self.filtered_indices().len();
        if length == 0 {
            self.selected = 0;
            self.table.select(None);
        } else {
            if self.selected >= length {
                self.selected = length - 1;
            }
            self.table.select(Some(self.selected));
        }
    }

    pub fn current_word(&self) -> Option<Word> {
        let indices = self.filtered_indices();
        indices
            .get(self.selected)
            .and_then(|index| self.all.get(*index))
            .map(|row| row.word.clone())
    }
}

pub struct StatsState {
    pub stats: Stats,
}

#[derive(Default)]
pub struct HelpState {
    pub scroll: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
    Warning,
}

pub struct Toast {
    pub message: String,
    pub kind: ToastKind,
    pub at: Instant,
}

pub enum Modal {
    ConfirmArchive(Word),
    ConfirmQuit,
    ConfirmDiscardForm,
    DidYouMean {
        input: String,
        suggestions: Vec<String>,
        selected: usize,
    },
}

pub struct App {
    pub screen: Screen,
    pub theme: Theme,
    pub db: Db,
    pub config: Config,
    pub paths: Paths,
    pub toast: Option<Toast>,
    pub modal: Option<Modal>,
    pub should_quit: bool,
    pub header: HeaderStats,
    pub ime: ImeController,
}

impl App {
    pub fn new(db: Db, config: Config, paths: Paths, theme: Theme) -> Result<Self> {
        let header = db.header_stats()?;
        let ime = ImeController::new(config.ime_auto);
        Ok(Self {
            screen: Screen::Home(HomeState::new()),
            theme,
            db,
            config,
            paths,
            toast: None,
            modal: None,
            should_quit: false,
            header,
            ime,
        })
    }

    pub fn run(&mut self, terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
        self.auto_sync(true);
        self.ime.activate();
        self.apply_ime_for_screen();
        let result = self.run_loop(terminal);
        self.auto_sync(false);
        self.ime.deactivate();
        result
    }

    fn auto_sync(&mut self, startup: bool) {
        if !self.config.auto_sync {
            return;
        }
        let Some(dir) = self.config.sync_dir.clone() else {
            return;
        };
        let use_git = self.config.sync_git;
        match sync::run(&mut self.db, &mut self.config, &self.paths, &dir, use_git) {
            Ok(report) => {
                if let Err(err) = self.refresh_header() {
                    self.log_sync(format!("refreshing header failed · {err:#}"));
                }
                if report.words_conflicted > 0 {
                    let message = format!("sync · {} conflicts", report.words_conflicted);
                    if startup {
                        self.toast_warning(message);
                    } else {
                        self.log_sync(message);
                    }
                }
            }
            Err(err) => {
                let message = format!("sync failed · {err:#}");
                if startup {
                    self.toast_warning(message);
                } else {
                    self.log_sync(message);
                }
            }
        }
    }

    fn log_sync(&self, message: String) {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.paths.log)
        {
            let _ = writeln!(file, "[sync] {message}");
        }
    }

    fn run_loop(&mut self, terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
        while !self.should_quit {
            terminal.draw(|frame| ui::render(frame, self))?;
            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        if let Err(err) = self.on_key(key) {
                            self.toast_error(format!("{err:#}"));
                        }
                    }
                    Event::Paste(text) => self.on_paste(&text),
                    Event::Resize(_, _) => {}
                    _ => {}
                }
            }
            self.tick();
        }
        Ok(())
    }

    fn tick(&mut self) {
        if let Some(toast) = &self.toast
            && toast.at.elapsed() > Duration::from_millis(TOAST_MS)
        {
            self.toast = None;
        }
    }

    fn on_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.code == KeyCode::F(2) {
            self.ime.toggle();
            self.toast_info(self.ime.status());
            return Ok(());
        }

        if self.modal.is_some() {
            return self.on_modal_key(key);
        }

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        {
            if let Screen::Form(form) = &self.screen
                && form.dirty
            {
                self.modal = Some(Modal::ConfirmQuit);
                return Ok(());
            }
            self.should_quit = true;
            return Ok(());
        }

        let mut screen = std::mem::replace(&mut self.screen, Screen::Empty);
        let result = match &mut screen {
            Screen::Home(home) => self.on_home_key(home, key),
            Screen::Form(form) => self.on_form_key(form, key),
            Screen::Review(review) => self.on_review_key(review, key),
            Screen::Browse(browse) => self.on_browse_key(browse, key),
            Screen::Stats(_) => self.on_stats_key(key),
            Screen::Help(help) => self.on_help_key(help, key),
            Screen::Empty => Ok(()),
        };
        if matches!(self.screen, Screen::Empty) {
            self.screen = screen;
        }
        result
    }

    fn on_home_key(&mut self, home: &mut HomeState, key: KeyEvent) -> Result<()> {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('p') | KeyCode::Char('P'))
        {
            if single_line_text(&home.input).trim().is_empty() {
                set_text(&mut home.input, "/");
                refresh_home_palette(home, true);
            } else {
                self.toast_info("clear the input first");
            }
            return Ok(());
        }

        if home.palette_open {
            match key.code {
                KeyCode::Esc => {
                    home.palette_open = false;
                    return Ok(());
                }
                KeyCode::Up => {
                    home.palette_selected = home.palette_selected.saturating_sub(1);
                    return Ok(());
                }
                KeyCode::Down => {
                    let count = commands::filtered(&single_line_text(&home.input)).len();
                    if count > 0 {
                        home.palette_selected = (home.palette_selected + 1).min(count - 1);
                    }
                    return Ok(());
                }
                KeyCode::Tab => {
                    complete_palette(home);
                    return Ok(());
                }
                KeyCode::Enter => {
                    return self.execute_home_enter(home);
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Enter => self.execute_home_enter(home),
            KeyCode::Esc => {
                if !single_line_text(&home.input).is_empty() {
                    home.input.clear();
                    home.palette_open = false;
                }
                Ok(())
            }
            KeyCode::Char('/') if single_line_text(&home.input).is_empty() => {
                set_text(&mut home.input, "/");
                refresh_home_palette(home, true);
                Ok(())
            }
            KeyCode::Char('?') if single_line_text(&home.input).is_empty() => {
                self.screen = Screen::Help(HelpState::default());
                Ok(())
            }
            KeyCode::Tab | KeyCode::BackTab => Ok(()),
            _ => {
                let modified = home.input.input(key);
                if modified {
                    refresh_home_palette(home, true);
                }
                Ok(())
            }
        }
    }

    fn execute_home_enter(&mut self, home: &mut HomeState) -> Result<()> {
        let text = single_line_text(&home.input);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(());
        }

        let is_palette = text.starts_with('/') && !text.chars().any(char::is_whitespace);
        if is_palette {
            let filtered = commands::filtered(&text);
            if let Some(spec) = filtered.get(home.palette_selected) {
                if spec.requires_arg {
                    complete_palette(home);
                    return Ok(());
                }
                let command = format!("/{}", spec.name);
                let result = self.execute_command(&command);
                let transitioned = !matches!(self.screen, Screen::Empty);
                if result.is_ok() && !transitioned {
                    home.input.clear();
                    home.palette_open = false;
                }
                return result;
            }
            self.toast_error("no matching command");
            return Ok(());
        }

        if text.starts_with('/') {
            let result = self.execute_command(&text);
            let transitioned = !matches!(self.screen, Screen::Empty);
            if result.is_ok() && !transitioned {
                home.input.clear();
                home.palette_open = false;
            }
            return result;
        }

        let result = self.submit_home(trimmed);
        let transitioned = !matches!(self.screen, Screen::Empty);
        if result.is_ok() && !transitioned {
            home.input.clear();
            home.palette_open = false;
        }
        result
    }

    fn submit_home(&mut self, raw: &str) -> Result<()> {
        let term = display_term(raw);
        if term.is_empty() {
            return Ok(());
        }
        let key = normalize_term(&term);
        match self.db.find_by_key(&key)? {
            Some(word) if word.is_archived() => {
                self.toast_warning(format!("archived · /restore {}", word.term));
            }
            Some(word) => self.start_review(word),
            None => {
                let suggestions = self.suggest_terms(&term)?;
                if suggestions.is_empty() {
                    self.open_new_form(Some(term), FormReturn::Home);
                } else {
                    self.modal = Some(Modal::DidYouMean {
                        input: term,
                        suggestions,
                        selected: 0,
                    });
                }
            }
        }
        Ok(())
    }

    fn on_form_key(&mut self, form: &mut FormState, key: KeyEvent) -> Result<()> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(key.code, KeyCode::Char('s') | KeyCode::Char('S')) {
            if let Err(err) = self.save_form(form) {
                form.error = Some(format!("{err:#}"));
            }
            return Ok(());
        }

        match key.code {
            KeyCode::Esc => {
                if form.dirty {
                    self.modal = Some(Modal::ConfirmDiscardForm);
                } else {
                    self.leave_form(form.return_to.clone())?;
                }
                Ok(())
            }
            KeyCode::Tab => {
                form.focus = (form.focus + 1).min(form.fields.len() - 1);
                self.ime.auto(form_lang(form.focus));
                Ok(())
            }
            KeyCode::BackTab => {
                form.focus = form.focus.saturating_sub(1);
                self.ime.auto(form_lang(form.focus));
                Ok(())
            }
            KeyCode::Enter if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                if form.focus + 1 < form.fields.len() {
                    form.focus += 1;
                    self.ime.auto(form_lang(form.focus));
                    Ok(())
                } else if let Err(err) = self.save_form(form) {
                    form.error = Some(format!("{err:#}"));
                    Ok(())
                } else {
                    Ok(())
                }
            }
            _ => {
                let modified = form.fields[form.focus].input(key);
                if modified {
                    form.dirty = true;
                    form.error = None;
                }
                Ok(())
            }
        }
    }

    fn save_form(&mut self, form: &mut FormState) -> Result<()> {
        let values: Vec<String> = form.fields.iter().map(text_of).collect();
        let (term, definition, example_en, example_zh) = (
            values[0].as_str(),
            values[1].as_str(),
            values[2].as_str(),
            values[3].as_str(),
        );
        if let Some(id) = form.editing_id {
            self.db
                .update_word(id, term, definition, example_en, example_zh)?;
            self.toast_ok("updated");
        } else {
            self.db
                .insert_word(term, definition, example_en, example_zh)?;
            self.toast_ok("saved");
        }
        self.refresh_header()?;
        self.leave_form(form.return_to.clone())
    }

    fn leave_form(&mut self, return_to: FormReturn) -> Result<()> {
        match return_to {
            FormReturn::Home => {
                self.go_home();
                Ok(())
            }
            FormReturn::Browse(query) => self.open_browse(&query),
        }
    }

    fn on_review_key(&mut self, review: &mut ReviewState, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.go_home();
                Ok(())
            }
            _ if review.phase == ReviewPhase::Prompt => match key.code {
                KeyCode::Enter if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                    self.submit_review(review)
                }
                _ => {
                    review.answer.input(key);
                    Ok(())
                }
            },
            _ => match key.code {
                KeyCode::Char('e') | KeyCode::Char('E') => {
                    review.phase = ReviewPhase::Prompt;
                    review.result = None;
                    review.started_at = Instant::now();
                    self.ime.auto(ImeLang::English);
                    Ok(())
                }
                KeyCode::Char('r') | KeyCode::Char('R') => {
                    review.answer.clear();
                    review.phase = ReviewPhase::Prompt;
                    review.result = None;
                    review.seen_target = true;
                    review.started_at = Instant::now();
                    review.scroll = 0;
                    self.ime.auto(ImeLang::English);
                    Ok(())
                }
                KeyCode::Enter => {
                    self.go_home();
                    Ok(())
                }
                KeyCode::Up => {
                    review.scroll = review.scroll.saturating_sub(1);
                    Ok(())
                }
                KeyCode::Down => {
                    let max = review
                        .result
                        .as_ref()
                        .map(|result| result.rows.len().saturating_sub(1) as u16)
                        .unwrap_or(0);
                    review.scroll = review.scroll.saturating_add(1).min(max);
                    Ok(())
                }
                KeyCode::PageUp => {
                    review.scroll = review.scroll.saturating_sub(8);
                    Ok(())
                }
                KeyCode::PageDown => {
                    let max = review
                        .result
                        .as_ref()
                        .map(|result| result.rows.len().saturating_sub(1) as u16)
                        .unwrap_or(0);
                    review.scroll = review.scroll.saturating_add(8).min(max);
                    Ok(())
                }
                _ => Ok(()),
            },
        }
    }

    fn submit_review(&mut self, review: &mut ReviewState) -> Result<()> {
        let typed = text_of(&review.answer);
        let typed = typed.trim();
        if typed.is_empty() {
            self.toast_warning("write the example first");
            return Ok(());
        }
        let result = diff::compute(typed, &review.word.example_en);
        let revealed = review.seen_target;
        let duration = review.started_at.elapsed().as_millis() as i64;
        self.db.add_attempt(
            review.word.id,
            typed,
            result.char_ratio,
            result.word_ratio,
            revealed,
            duration,
        )?;
        review.seen_target = true;
        review.result = Some(result);
        review.phase = ReviewPhase::Result;
        review.scroll = 0;
        self.refresh_header()?;
        Ok(())
    }

    fn on_browse_key(&mut self, browse: &mut BrowseState, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                if !single_line_text(&browse.query).is_empty() {
                    browse.query.clear();
                    browse.reset_selection();
                } else {
                    self.go_home();
                }
                Ok(())
            }
            KeyCode::Up => {
                browse.selected = browse.selected.saturating_sub(1);
                browse.clamp_selection();
                Ok(())
            }
            KeyCode::Down => {
                let length = browse.filtered_indices().len();
                if length > 0 {
                    browse.selected = (browse.selected + 1).min(length - 1);
                }
                browse.clamp_selection();
                Ok(())
            }
            KeyCode::Enter => {
                if let Some(word) = browse.current_word() {
                    self.start_review(word);
                }
                Ok(())
            }
            KeyCode::Char('e') | KeyCode::Char('E') => {
                if let Some(word) = browse.current_word() {
                    let query = single_line_text(&browse.query);
                    self.open_edit_form(word, FormReturn::Browse(query));
                }
                Ok(())
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                if let Some(word) = browse.current_word() {
                    self.modal = Some(Modal::ConfirmArchive(word));
                }
                Ok(())
            }
            KeyCode::Tab | KeyCode::BackTab => Ok(()),
            _ => {
                let modified = browse.query.input(key);
                if modified {
                    browse.reset_selection();
                }
                Ok(())
            }
        }
    }

    fn on_stats_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.code == KeyCode::Esc {
            self.go_home();
        }
        Ok(())
    }

    fn on_help_key(&mut self, help: &mut HelpState, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.go_home();
                Ok(())
            }
            KeyCode::Up => {
                help.scroll = help.scroll.saturating_sub(1);
                Ok(())
            }
            KeyCode::Down => {
                help.scroll = help.scroll.saturating_add(1);
                Ok(())
            }
            KeyCode::PageUp => {
                help.scroll = help.scroll.saturating_sub(8);
                Ok(())
            }
            KeyCode::PageDown => {
                help.scroll = help.scroll.saturating_add(8);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn on_modal_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        {
            self.should_quit = true;
            return Ok(());
        }

        let Some(modal) = self.modal.take() else {
            return Ok(());
        };
        match modal {
            Modal::ConfirmArchive(word) => match key.code {
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.db.archive_word(word.id)?;
                    self.refresh_header()?;
                    self.reload_browse()?;
                    self.toast_ok(format!("archived · {}", word.term));
                    Ok(())
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => Ok(()),
                _ => {
                    self.modal = Some(Modal::ConfirmArchive(word));
                    Ok(())
                }
            },
            Modal::ConfirmQuit => match key.code {
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.should_quit = true;
                    Ok(())
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => Ok(()),
                _ => {
                    self.modal = Some(Modal::ConfirmQuit);
                    Ok(())
                }
            },
            Modal::ConfirmDiscardForm => match key.code {
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => self.discard_form(),
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => Ok(()),
                _ => {
                    self.modal = Some(Modal::ConfirmDiscardForm);
                    Ok(())
                }
            },
            Modal::DidYouMean {
                input,
                suggestions,
                mut selected,
            } => match key.code {
                KeyCode::Esc => Ok(()),
                KeyCode::Up => {
                    selected = selected.saturating_sub(1);
                    self.modal = Some(Modal::DidYouMean {
                        input,
                        suggestions,
                        selected,
                    });
                    Ok(())
                }
                KeyCode::Down => {
                    if !suggestions.is_empty() {
                        selected = (selected + 1).min(suggestions.len() - 1);
                    }
                    self.modal = Some(Modal::DidYouMean {
                        input,
                        suggestions,
                        selected,
                    });
                    Ok(())
                }
                KeyCode::Enter => {
                    if let Some(term) = suggestions.get(selected).cloned() {
                        match self.db.find_by_term(&term)? {
                            Some(word) => self.start_review(word),
                            None => self.open_new_form(Some(term), FormReturn::Home),
                        }
                    }
                    Ok(())
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    self.open_new_form(Some(input), FormReturn::Home);
                    Ok(())
                }
                _ => {
                    self.modal = Some(Modal::DidYouMean {
                        input,
                        suggestions,
                        selected,
                    });
                    Ok(())
                }
            },
        }
    }

    fn discard_form(&mut self) -> Result<()> {
        let return_to = match &self.screen {
            Screen::Form(form) => Some(form.return_to.clone()),
            _ => None,
        };
        match return_to {
            Some(FormReturn::Browse(query)) => self.open_browse(&query),
            _ => {
                self.go_home();
                Ok(())
            }
        }
    }

    fn execute_command(&mut self, input: &str) -> Result<()> {
        let Some((spec, args)) = commands::parse(input) else {
            let suggestions = commands::suggestions(input)
                .iter()
                .map(|spec| format!("/{}", spec.name))
                .collect::<Vec<_>>();
            if suggestions.is_empty() {
                self.toast_error("unknown command · /help");
            } else {
                self.toast_error(format!("unknown command · try {}", suggestions.join(" ")));
            }
            return Ok(());
        };

        if spec.requires_arg && args.is_empty() {
            self.toast_warning(format!("usage: {}", spec.usage));
            return Ok(());
        }

        match spec.id {
            CommandId::Add => {
                let term = if args.is_empty() { None } else { Some(args) };
                self.open_new_form(term, FormReturn::Home);
            }
            CommandId::Find => self.open_browse(&args)?,
            CommandId::List => self.open_browse("")?,
            CommandId::Review => match self.db.find_by_term(&args)? {
                Some(word) if word.is_archived() => {
                    self.toast_warning(format!("archived · /restore {}", word.term));
                }
                Some(word) => self.start_review(word),
                None => self.toast_not_found(&args)?,
            },
            CommandId::Edit => match self.db.find_by_term(&args)? {
                Some(word) => self.open_edit_form(word, FormReturn::Home),
                None => self.toast_not_found(&args)?,
            },
            CommandId::Delete => match self.db.find_by_term(&args)? {
                Some(word) if word.is_archived() => {
                    self.toast_warning(format!("already archived · {}", word.term));
                }
                Some(word) => self.modal = Some(Modal::ConfirmArchive(word)),
                None => self.toast_not_found(&args)?,
            },
            CommandId::Restore => match self.db.find_by_term(&args)? {
                Some(word) if word.is_archived() => {
                    self.db.restore_word(word.id)?;
                    self.refresh_header()?;
                    self.toast_ok(format!("restored · {}", word.term));
                }
                Some(word) => self.toast_info(format!("already active · {}", word.term)),
                None => self.toast_not_found(&args)?,
            },
            CommandId::Stats => {
                let stats = self.db.stats()?;
                self.screen = Screen::Stats(StatsState { stats });
            }
            CommandId::Export => {
                let path = if args.is_empty() {
                    self.paths.default_export()
                } else {
                    PathBuf::from(clean_path(&args))
                };
                let file = self.db.export_file()?;
                export::write(&path, &file)?;
                self.toast_ok(format!("exported → {}", path.display()));
            }
            CommandId::Import => {
                let path = PathBuf::from(clean_path(&args));
                let file = export::read(&path)?;
                let report = self.db.import_file(&file)?;
                self.refresh_header()?;
                self.toast_ok(format!(
                    "imported +{} words, {} updated, +{} attempts ({} conflicts, {} skipped)",
                    report.words_added,
                    report.words_updated,
                    report.attempts_added,
                    report.words_conflicted,
                    report.words_skipped + report.attempts_skipped
                ));
            }
            CommandId::Backup => {
                let path = if args.is_empty() {
                    self.paths.default_backup()
                } else {
                    PathBuf::from(clean_path(&args))
                };
                self.db.backup_to(&path)?;
                self.toast_ok(format!("backup → {}", path.display()));
            }
            CommandId::Sync => {
                let dir = if args.is_empty() {
                    self.config.sync_dir.clone()
                } else {
                    let path = PathBuf::from(clean_path(&args));
                    self.config.sync_dir = Some(path.clone());
                    config::save(&self.paths.config, &self.config)?;
                    Some(path)
                };
                let Some(dir) = dir else {
                    self.toast_warning("set a sync directory: /sync <dir>");
                    return Ok(());
                };
                let use_git = self.config.sync_git;
                let report = sync::run(&mut self.db, &mut self.config, &self.paths, &dir, use_git)?;
                self.refresh_header()?;
                self.toast_ok(report.summary());
            }
            CommandId::Theme => {
                if args.is_empty() {
                    self.toast_info(format!("theme: {}", self.config.theme));
                } else {
                    let name = args.to_ascii_lowercase();
                    if !matches!(name.as_str(), "auto" | "dark" | "light" | "mono") {
                        self.toast_warning("usage: /theme [auto|dark|light|mono]");
                    } else {
                        self.theme = Theme::from_name(&name);
                        self.config.theme = name;
                        config::save(&self.paths.config, &self.config)?;
                        self.toast_ok(format!("theme · {}", self.config.theme));
                    }
                }
            }
            CommandId::Ime => {
                let arg = args.trim().to_ascii_lowercase();
                match arg.as_str() {
                    "" => self.toast_info(self.ime.status()),
                    "on" | "auto" => {
                        self.config.ime_auto = true;
                        config::save(&self.paths.config, &self.config)?;
                        self.ime.set_enabled(true);
                        self.apply_ime_for_screen();
                        self.toast_ok("ime auto on");
                    }
                    "off" => {
                        self.config.ime_auto = false;
                        config::save(&self.paths.config, &self.config)?;
                        self.ime.set_enabled(false);
                        self.toast_ok("ime auto off · F2 still switches");
                    }
                    "en" => {
                        self.ime.force(ImeLang::English);
                        self.toast_ok("ime → EN");
                    }
                    "zh" => {
                        self.ime.force(ImeLang::Chinese);
                        self.toast_ok("ime → 中");
                    }
                    _ => self.toast_warning("usage: /ime [on|off|en|zh]"),
                }
            }
            CommandId::Help => {
                self.screen = Screen::Help(HelpState::default());
            }
            CommandId::Quit => {
                self.should_quit = true;
            }
        }
        Ok(())
    }

    fn suggest_terms(&self, term: &str) -> Result<Vec<String>> {
        let key = normalize_term(term);
        let mut scored: Vec<(f64, String)> = self
            .db
            .active_terms()?
            .into_iter()
            .filter_map(|candidate| {
                let candidate_key = normalize_term(&candidate);
                if candidate_key == key {
                    return None;
                }
                let score = strsim::jaro_winkler(&key, &candidate_key);
                if score >= 0.88 {
                    Some((score, candidate))
                } else {
                    None
                }
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(Ordering::Equal));
        Ok(scored
            .into_iter()
            .take(3)
            .map(|(_, candidate)| candidate)
            .collect())
    }

    fn toast_not_found(&mut self, term: &str) -> Result<()> {
        let suggestions = self.suggest_terms(term)?;
        if suggestions.is_empty() {
            self.toast_error(format!("not found · {term}"));
        } else {
            self.toast_warning(format!(
                "not found · did you mean {}?",
                suggestions.join(" / ")
            ));
        }
        Ok(())
    }

    fn start_review(&mut self, word: Word) {
        self.ime.auto(ImeLang::English);
        self.screen = Screen::Review(ReviewState::new(word));
    }

    fn open_new_form(&mut self, term: Option<String>, return_to: FormReturn) {
        let focus = if term.is_some() { 1 } else { 0 };
        self.screen = Screen::Form(FormState::new_new(term.as_deref(), return_to));
        self.ime.auto(form_lang(focus));
    }

    fn open_edit_form(&mut self, word: Word, return_to: FormReturn) {
        self.screen = Screen::Form(FormState::new_edit(&word, return_to));
        self.ime.auto(ImeLang::English);
    }

    fn open_browse(&mut self, query: &str) -> Result<()> {
        self.ime.auto(ImeLang::English);
        let rows = self.db.list_words()?;
        let mut browse = BrowseState::new(rows);
        if !query.trim().is_empty() {
            set_text(&mut browse.query, query.trim());
            browse.selected = 0;
        }
        browse.reset_selection();
        self.screen = Screen::Browse(browse);
        Ok(())
    }

    fn reload_browse(&mut self) -> Result<()> {
        if let Screen::Browse(browse) = &mut self.screen {
            let rows = self.db.list_words()?;
            browse.all = rows;
            browse.reset_selection();
        }
        Ok(())
    }

    fn go_home(&mut self) {
        self.screen = Screen::Home(HomeState::new());
        self.ime.auto(ImeLang::English);
    }

    fn refresh_header(&mut self) -> Result<()> {
        self.header = self.db.header_stats()?;
        Ok(())
    }

    fn on_paste(&mut self, text: &str) {
        if self.modal.is_some() {
            return;
        }
        let multiline = text.replace("\r\n", "\n").replace('\r', "\n");
        let singleline = multiline.replace('\n', " ");

        match &mut self.screen {
            Screen::Home(home) => {
                if home.input.insert_str(&singleline) {
                    refresh_home_palette(home, true);
                }
            }
            Screen::Form(form) => {
                if form.fields[form.focus].insert_str(&multiline) {
                    form.dirty = true;
                    form.error = None;
                }
            }
            Screen::Review(review) if review.phase == ReviewPhase::Prompt => {
                review.answer.insert_str(&multiline);
            }
            Screen::Browse(browse) => {
                if browse.query.insert_str(&singleline) {
                    browse.reset_selection();
                }
            }
            _ => {}
        }
    }

    fn apply_ime_for_screen(&mut self) {
        let lang = match &self.screen {
            Screen::Home(_) => Some(ImeLang::English),
            Screen::Form(form) => Some(form_lang(form.focus)),
            Screen::Review(_) => Some(ImeLang::English),
            _ => None,
        };
        if let Some(lang) = lang {
            self.ime.auto(lang);
        }
    }

    fn toast_info(&mut self, message: impl Into<String>) {
        self.set_toast(ToastKind::Info, message);
    }

    fn toast_ok(&mut self, message: impl Into<String>) {
        self.set_toast(ToastKind::Success, message);
    }

    fn toast_error(&mut self, message: impl Into<String>) {
        self.set_toast(ToastKind::Error, message);
    }

    fn toast_warning(&mut self, message: impl Into<String>) {
        self.set_toast(ToastKind::Warning, message);
    }

    fn set_toast(&mut self, kind: ToastKind, message: impl Into<String>) {
        self.toast = Some(Toast {
            message: message.into(),
            kind,
            at: Instant::now(),
        });
    }
}

fn form_lang(focus: usize) -> ImeLang {
    if focus == 1 || focus == 3 {
        ImeLang::Chinese
    } else {
        ImeLang::English
    }
}

fn make_textarea(placeholder: &str, wrap: WrapMode) -> TextArea<'static> {
    let mut textarea: TextArea<'static> = TextArea::default();
    textarea.set_placeholder_text(placeholder);
    textarea.set_placeholder_style(
        ratatui::style::Style::default().fg(ratatui::style::Color::DarkGray),
    );
    textarea.set_cursor_style(ratatui::style::Style::default());
    textarea.set_cursor_line_style(ratatui::style::Style::default());
    textarea.set_tab_length(2);
    textarea.set_max_histories(100);
    textarea.set_wrap_mode(wrap);
    textarea
}

fn set_text(textarea: &mut TextArea<'static>, text: &str) {
    textarea.clear();
    if !text.is_empty() {
        textarea.insert_str(text);
    }
}

pub fn text_of(textarea: &TextArea<'_>) -> String {
    textarea.lines().join("\n")
}

pub fn single_line_text(textarea: &TextArea<'_>) -> String {
    textarea.lines().join(" ")
}

fn refresh_home_palette(home: &mut HomeState, reset: bool) {
    let text = single_line_text(&home.input);
    let active = text.starts_with('/') && !text.chars().any(char::is_whitespace);
    home.palette_open = active;
    if !active {
        return;
    }
    let count = commands::filtered(&text).len();
    if reset {
        home.palette_selected = 0;
    }
    if count == 0 {
        home.palette_selected = 0;
    } else if home.palette_selected >= count {
        home.palette_selected = count - 1;
    }
}

fn complete_palette(home: &mut HomeState) {
    let filtered = commands::filtered(&single_line_text(&home.input));
    if let Some(spec) = filtered.get(home.palette_selected) {
        set_text(&mut home.input, &format!("/{} ", spec.name));
        home.palette_open = false;
        home.palette_selected = 0;
    }
}

fn clean_path(input: &str) -> String {
    let trimmed = input.trim();
    if (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
    {
        trimmed[1..trimmed.len().saturating_sub(1)].to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::ui::theme::Theme;
    use tempfile::tempdir;

    fn test_app() -> App {
        let dir = tempdir().unwrap();
        let paths = Paths::discover(Some(dir.path().to_path_buf())).unwrap();
        let db = Db::open_in_memory().unwrap();
        App::new(db, Config::default(), paths, Theme::dark()).unwrap()
    }

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE)).unwrap();
    }

    fn press_ctrl(app: &mut App, ch: char) {
        app.on_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL))
            .unwrap();
    }

    fn type_text(app: &mut App, text: &str) {
        for ch in text.chars() {
            press(app, KeyCode::Char(ch));
        }
    }

    #[test]
    fn auto_sync_imports_peer_snapshot() {
        let sync_dir = tempdir().unwrap();
        let devices = sync_dir.path().join("devices");
        std::fs::create_dir_all(&devices).unwrap();
        let mut peer = Db::open_in_memory().unwrap();
        peer.insert_word(
            "peerword",
            "来自另一台电脑",
            "A peer example.",
            "一个对端例句。",
        )
        .unwrap();
        export::write(&devices.join("peer.json"), &peer.export_file().unwrap()).unwrap();

        let mut app = test_app();
        app.config.device_id = Some("test-device".to_string());
        app.config.sync_dir = Some(sync_dir.path().to_path_buf());
        app.config.auto_sync = true;
        app.auto_sync(true);
        assert!(app.db.find_by_term("peerword").unwrap().is_some());
    }

    #[test]
    fn sync_command_writes_device_snapshot() {
        let sync_dir = tempdir().unwrap();
        let mut app = test_app();
        app.config.device_id = Some("test-device".to_string());
        app.config.sync_dir = Some(sync_dir.path().to_path_buf());
        app.db
            .insert_word("syncword", "同步测试", "A sync example.", "一个同步例句。")
            .unwrap();

        app.execute_command("/sync").unwrap();
        let snapshot = sync_dir.path().join("devices/test-device.json");
        assert!(snapshot.exists());
        let file = export::read(&snapshot).unwrap();
        assert_eq!(file.words.len(), 1);
        assert_eq!(file.words[0].term, "syncword");
    }

    #[test]
    fn create_word_then_review_exact_answer() {
        let mut app = test_app();
        type_text(&mut app, "serendipity");
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.screen, Screen::Form(_)));

        {
            let Screen::Form(form) = &mut app.screen else {
                panic!("expected form");
            };
            set_text(&mut form.fields[1], "意外发现美好事物的能力");
            set_text(
                &mut form.fields[2],
                "He found the letter by sheer serendipity.",
            );
            set_text(&mut form.fields[3], "机缘巧合之下，他发现了那封信。");
        }
        press_ctrl(&mut app, 's');
        assert!(matches!(app.screen, Screen::Home(_)));
        assert!(app.db.find_by_term("serendipity").unwrap().is_some());

        type_text(&mut app, "serendipity");
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.screen, Screen::Review(_)));

        type_text(&mut app, "He found the letter by sheer serendipity.");
        press(&mut app, KeyCode::Enter);
        let Screen::Review(review) = &app.screen else {
            panic!("expected review");
        };
        assert_eq!(review.phase, ReviewPhase::Result);
        assert!(review.result.as_ref().unwrap().exact);
        assert_eq!(app.db.stats().unwrap().total_attempts, 1);

        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.screen, Screen::Home(_)));
    }

    #[test]
    fn palette_opens_and_runs_stats() {
        let mut app = test_app();
        press(&mut app, KeyCode::Char('/'));
        let Screen::Home(home) = &app.screen else {
            panic!("expected home");
        };
        assert!(home.palette_open);

        type_text(&mut app, "stats");
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.screen, Screen::Stats(_)));
    }

    #[test]
    fn archived_word_suggests_restore_instead_of_review() {
        let mut app = test_app();
        let word = app
            .db
            .insert_word("serendipity", "m", "example", "translation")
            .unwrap();
        app.db.archive_word(word.id).unwrap();

        type_text(&mut app, "serendipity");
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.screen, Screen::Home(_)));
        let toast = app.toast.as_ref().expect("toast");
        assert!(toast.message.contains("archived"));
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;
    use crate::config::Config;
    use crate::ui::theme::Theme;
    use tempfile::tempdir;

    fn test_app() -> App {
        let dir = tempdir().unwrap();
        let paths = Paths::discover(Some(dir.path().to_path_buf())).unwrap();
        let db = Db::open_in_memory().unwrap();
        App::new(db, Config::default(), paths, Theme::dark()).unwrap()
    }

    #[test]
    fn theme_command_persists_to_config() {
        let mut app = test_app();
        app.execute_command("/theme dark").unwrap();
        assert_eq!(app.config.theme, "dark");
        let loaded = crate::config::load(&app.paths.config).unwrap();
        assert_eq!(loaded.theme, "dark");

        app.execute_command("/theme light").unwrap();
        let loaded = crate::config::load(&app.paths.config).unwrap();
        assert_eq!(loaded.theme, "light");
    }

    #[test]
    fn delete_and_restore_commands_use_archive() {
        let mut app = test_app();
        let word = app
            .db
            .insert_word("serendipity", "m", "example", "translation")
            .unwrap();
        app.execute_command("/delete serendipity").unwrap();
        assert!(app.modal.is_some());
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert!(
            app.db
                .find_by_term("serendipity")
                .unwrap()
                .unwrap()
                .is_archived()
        );

        app.execute_command("/restore serendipity").unwrap();
        assert!(
            !app.db
                .find_by_term("serendipity")
                .unwrap()
                .unwrap()
                .is_archived()
        );
        assert_eq!(
            word.id,
            app.db.find_by_term("serendipity").unwrap().unwrap().id
        );
    }

    #[test]
    fn ime_command_persists_toggle() {
        let mut app = test_app();
        assert!(app.config.ime_auto);
        app.execute_command("/ime off").unwrap();
        assert!(!app.config.ime_auto);
        assert!(!crate::config::load(&app.paths.config).unwrap().ime_auto);
    }

    #[test]
    fn form_rejects_empty_fields() {
        let mut app = test_app();
        app.execute_command("/add").unwrap();
        assert!(matches!(app.screen, Screen::Form(_)));
        app.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
            .unwrap();
        let Screen::Form(form) = &app.screen else {
            panic!("expected form");
        };
        assert!(form.error.is_some());
    }
}

#[cfg(test)]
mod browse_tests {
    use super::*;
    use crate::config::Config;
    use crate::ui::theme::Theme;
    use tempfile::tempdir;

    fn test_app() -> App {
        let dir = tempdir().unwrap();
        let paths = Paths::discover(Some(dir.path().to_path_buf())).unwrap();
        let db = Db::open_in_memory().unwrap();
        App::new(db, Config::default(), paths, Theme::dark()).unwrap()
    }

    #[test]
    fn find_command_filters_and_opens_review() {
        let mut app = test_app();
        app.db
            .insert_word("serendipity", "meaning", "example", "translation")
            .unwrap();
        app.db
            .insert_word("ubiquitous", "meaning", "example", "translation")
            .unwrap();
        app.execute_command("/find serendipity").unwrap();
        let Screen::Browse(browse) = &app.screen else {
            panic!("expected browse");
        };
        assert_eq!(single_line_text(&browse.query), "serendipity");
        assert_eq!(browse.filtered_indices().len(), 1);
        assert_eq!(
            browse.current_word().map(|word| word.term),
            Some("serendipity".to_string())
        );

        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(app.screen, Screen::Review(_)));
    }
}
