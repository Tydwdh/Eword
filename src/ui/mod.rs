pub mod theme;
pub mod widgets;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, Wrap};

use crate::app::{
    App, BrowseState, FormState, HelpState, HomeState, ReviewPhase, ReviewState, Screen, StatsState,
};
use crate::commands::COMMANDS;
use crate::config::{Config, Paths};
use crate::model::{DiffKind, DiffResult, HeaderStats};
use crate::ui::theme::Theme;

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.width < 56 || area.height < 20 {
        widgets::too_small(frame, area, &app.theme);
        return;
    }

    let allow_cursor = app.modal.is_none();
    let theme = app.theme;
    let header = app.header.clone();
    let paths = app.paths.clone();
    let config = app.config.clone();

    match &mut app.screen {
        Screen::Home(home) => render_home(frame, home, &theme, &header, allow_cursor),
        Screen::Form(form) => render_form(frame, form, &theme, allow_cursor),
        Screen::Review(review) => render_review(frame, review, &theme, allow_cursor),
        Screen::Browse(browse) => render_browse(frame, browse, &theme, allow_cursor),
        Screen::Stats(stats) => render_stats(frame, stats, &theme),
        Screen::Help(help) => render_help(frame, help, &theme, &paths, &config),
        Screen::Empty => {}
    }

    if let Some(modal) = &app.modal {
        widgets::render_modal(frame, modal, &theme);
    }
    if let Some(toast) = &app.toast {
        widgets::render_toast(frame, toast, &theme);
    }
}

fn render_home(
    frame: &mut Frame,
    home: &mut HomeState,
    theme: &Theme,
    header: &HeaderStats,
    allow_cursor: bool,
) {
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .split(frame.area());

    widgets::render_header(
        frame,
        chunks[0],
        Line::from(vec![Span::styled("Eword", theme.accent_style())]),
        header_text(header),
        theme,
    );

    let inner = widgets::content_rect(chunks[1]);
    let center = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(5),
        Constraint::Fill(2),
    ])
    .split(inner)[1];

    let label = if header.words == 0 {
        "No words yet — type one to begin."
    } else {
        "What are you practicing?"
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            label.to_string(),
            theme.muted_style(),
        )))
        .alignment(Alignment::Center),
        Rect {
            height: 1,
            ..center
        },
    );

    let input_width = center.width.min(64);
    let input = Rect {
        x: center.x + center.width.saturating_sub(input_width) / 2,
        y: center.y + 2,
        width: input_width,
        height: 1,
    };
    widgets::render_field(frame, &mut home.input, input, true, allow_cursor, theme);

    frame.render_widget(
        Paragraph::new(widgets::hint_line(
            &[("enter", "review"), ("/", "commands"), ("?", "help")],
            theme,
        ))
        .alignment(Alignment::Center),
        Rect {
            y: center.y + 3,
            height: 1,
            ..center
        },
    );

    if home.palette_open {
        widgets::render_palette(frame, home, input, theme);
    }

    widgets::render_footer(
        frame,
        chunks[2],
        widgets::hint_line(
            &[("f2", "ime"), ("esc", "clear"), ("ctrl+c", "quit")],
            theme,
        ),
        theme,
    );
}

fn render_form(frame: &mut Frame, form: &mut FormState, theme: &Theme, allow_cursor: bool) {
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .split(frame.area());

    let title = if form.editing_id.is_some() {
        "EDIT WORD"
    } else {
        "NEW WORD"
    };
    widgets::render_header(
        frame,
        chunks[0],
        Line::from(vec![Span::styled(title, theme.accent_style())]),
        format!("{}/4", form.focus + 1),
        theme,
    );

    let content = widgets::content_rect(chunks[1]);
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .split(content);

    let labels = ["WORD", "MEANING", "EXAMPLE", "TRANSLATION"];
    widgets::render_label(frame, rows[0], labels[0], form.focus == 0, theme);
    widgets::render_field(
        frame,
        &mut form.fields[0],
        rows[1],
        form.focus == 0,
        allow_cursor,
        theme,
    );
    widgets::render_label(frame, rows[3], labels[1], form.focus == 1, theme);
    widgets::render_field(
        frame,
        &mut form.fields[1],
        rows[4],
        form.focus == 1,
        allow_cursor,
        theme,
    );
    widgets::render_label(frame, rows[6], labels[2], form.focus == 2, theme);
    widgets::render_field(
        frame,
        &mut form.fields[2],
        rows[7],
        form.focus == 2,
        allow_cursor,
        theme,
    );
    widgets::render_label(frame, rows[9], labels[3], form.focus == 3, theme);
    widgets::render_field(
        frame,
        &mut form.fields[3],
        rows[10],
        form.focus == 3,
        allow_cursor,
        theme,
    );

    if let Some(error) = &form.error {
        frame.render_widget(
            Paragraph::new(error.clone()).style(theme.error_style()),
            rows[12],
        );
    }

    widgets::render_footer(
        frame,
        chunks[2],
        widgets::hint_line(
            &[
                ("tab", "next"),
                ("ctrl+s", "save"),
                ("f2", "ime"),
                ("esc", "cancel"),
            ],
            theme,
        ),
        theme,
    );
}

fn render_review(frame: &mut Frame, review: &mut ReviewState, theme: &Theme, allow_cursor: bool) {
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .split(frame.area());

    widgets::render_header(
        frame,
        chunks[0],
        Line::from(vec![Span::styled("REVIEW", theme.accent_style())]),
        review.word.term.clone(),
        theme,
    );

    let content = widgets::content_rect(chunks[1]);
    if review.phase == ReviewPhase::Prompt {
        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Fill(1),
        ])
        .split(content);
        widgets::render_label(frame, rows[0], "MEANING", false, theme);
        frame.render_widget(
            Paragraph::new(review.word.definition.clone())
                .wrap(Wrap { trim: false })
                .style(theme.base()),
            rows[1],
        );
        widgets::render_label(frame, rows[3], "TRANSLATE THIS EXAMPLE", false, theme);
        frame.render_widget(
            Paragraph::new(review.word.example_zh.clone())
                .wrap(Wrap { trim: false })
                .style(theme.base()),
            rows[4],
        );
        widgets::render_label(frame, rows[6], "YOUR ANSWER", true, theme);
        widgets::render_field(
            frame,
            &mut review.answer,
            rows[7],
            true,
            allow_cursor,
            theme,
        );
        widgets::render_footer(
            frame,
            chunks[2],
            widgets::hint_line(&[("enter", "check"), ("f2", "ime"), ("esc", "back")], theme),
            theme,
        );
    } else {
        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .split(content);
        widgets::render_label(frame, rows[0], "MEANING", false, theme);
        frame.render_widget(
            Paragraph::new(review.word.definition.clone())
                .wrap(Wrap { trim: false })
                .style(theme.base()),
            rows[1],
        );
        widgets::render_label(frame, rows[3], "TRANSLATE THIS EXAMPLE", false, theme);
        frame.render_widget(
            Paragraph::new(review.word.example_zh.clone())
                .wrap(Wrap { trim: false })
                .style(theme.base()),
            rows[4],
        );
        widgets::render_label(frame, rows[6], "CHECK", false, theme);
        if let Some(result) = &review.result {
            frame.render_widget(Paragraph::new(score_line(result, theme)), rows[7]);
            frame.render_widget(
                Paragraph::new(diff_lines(result, theme))
                    .wrap(Wrap { trim: false })
                    .scroll((review.scroll, 0)),
                rows[8],
            );
        }
        widgets::render_footer(
            frame,
            chunks[2],
            widgets::hint_line(
                &[
                    ("e", "edit"),
                    ("r", "retry"),
                    ("enter", "done"),
                    ("f2", "ime"),
                    ("↑↓", "scroll"),
                ],
                theme,
            ),
            theme,
        );
    }
}

fn render_browse(frame: &mut Frame, browse: &mut BrowseState, theme: &Theme, allow_cursor: bool) {
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .split(frame.area());

    let filtered = browse.filtered_indices();
    widgets::render_header(
        frame,
        chunks[0],
        Line::from(vec![Span::styled("WORDS", theme.accent_style())]),
        format!("{} / {}", filtered.len(), browse.all.len()),
        theme,
    );

    let content = widgets::content_rect(chunks[1]);
    widgets::render_field(
        frame,
        &mut browse.query,
        Rect {
            height: 1,
            ..content
        },
        true,
        allow_cursor,
        theme,
    );

    let table_area = Rect {
        y: content.y + 2,
        height: content.height.saturating_sub(2),
        ..content
    };

    if filtered.is_empty() {
        let message = if browse.all.is_empty() {
            "no words yet — add one from home"
        } else {
            "no matches"
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                message.to_string(),
                theme.muted_style(),
            )))
            .alignment(Alignment::Center),
            table_area,
        );
    } else {
        let mut rows = Vec::with_capacity(filtered.len());
        for index in &filtered {
            let row = &browse.all[*index];
            let accuracy = match row.avg_ratio {
                None => Line::from(Span::styled("—", theme.muted_style())),
                Some(ratio) => {
                    let style = if ratio >= 0.9 {
                        theme.success_style()
                    } else if ratio >= 0.75 {
                        theme.warning_style()
                    } else {
                        theme.error_style()
                    };
                    Line::from(Span::styled(
                        format!("{:.0}% ({})", ratio * 100.0, row.attempts),
                        style,
                    ))
                }
            };
            rows.push(Row::new(vec![
                Cell::from(Line::from(Span::styled(
                    row.word.term.clone(),
                    theme.base().add_modifier(Modifier::BOLD),
                ))),
                Cell::from(Line::from(Span::styled(
                    row.word.definition.clone(),
                    theme.muted_style(),
                ))),
                Cell::from(accuracy),
            ]));
        }

        let header = Row::new(vec!["WORD", "MEANING", "ACCURACY"]).style(theme.label());
        let table = Table::new(
            rows,
            [
                Constraint::Percentage(30),
                Constraint::Percentage(52),
                Constraint::Length(12),
            ],
        )
        .header(header)
        .column_spacing(2)
        .highlight_symbol("› ")
        .row_highlight_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        );
        frame.render_stateful_widget(table, table_area, &mut browse.table);
    }

    widgets::render_footer(
        frame,
        chunks[2],
        widgets::hint_line(
            &[
                ("enter", "review"),
                ("e", "edit"),
                ("d", "archive"),
                ("f2", "ime"),
                ("esc", "back"),
            ],
            theme,
        ),
        theme,
    );
}

fn render_stats(frame: &mut Frame, state: &StatsState, theme: &Theme) {
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .split(frame.area());
    widgets::render_header(
        frame,
        chunks[0],
        Line::from(vec![Span::styled("PROGRESS", theme.accent_style())]),
        "all time".to_string(),
        theme,
    );

    let content = widgets::content_rect(chunks[1]);
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(format!("{}", state.stats.total_words), theme.accent_style()),
        Span::styled(" words   ", theme.muted_style()),
        Span::styled(
            format!("{}", state.stats.total_attempts),
            theme.base().add_modifier(Modifier::BOLD),
        ),
        Span::styled(" attempts   ", theme.muted_style()),
        match state.stats.avg_ratio {
            Some(ratio) => Span::styled(
                format!("{:.0}% blind accuracy", ratio * 100.0),
                if ratio >= 0.9 {
                    theme.success_style()
                } else if ratio >= 0.75 {
                    theme.warning_style()
                } else {
                    theme.error_style()
                },
            ),
            None => Span::styled("no blind attempts yet", theme.muted_style()),
        },
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!(
                "streak {} {}",
                state.stats.streak_days,
                if state.stats.streak_days == 1 {
                    "day"
                } else {
                    "days"
                }
            ),
            theme.base(),
        ),
        Span::styled("   ·   ", theme.muted_style()),
        Span::styled(
            format!("today {} attempts", state.stats.today_attempts),
            theme.muted_style(),
        ),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("WEAKEST", theme.label())));

    if state.stats.weakest.is_empty() {
        lines.push(Line::from(Span::styled(
            "need at least 2 blind attempts per word",
            theme.muted_style(),
        )));
    } else {
        for weak in &state.stats.weakest {
            let style = if weak.avg_ratio >= 0.75 {
                theme.warning_style()
            } else {
                theme.error_style()
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:<24}", weak.term),
                    theme.base().add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("{:>4.0}%", weak.avg_ratio * 100.0), style),
                Span::styled(
                    format!("   {} attempts", weak.attempts),
                    theme.muted_style(),
                ),
            ]));
        }
    }

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(theme.base()),
        content,
    );
    widgets::render_footer(
        frame,
        chunks[2],
        widgets::hint_line(&[("esc", "back")], theme),
        theme,
    );
}

fn render_help(frame: &mut Frame, help: &HelpState, theme: &Theme, paths: &Paths, config: &Config) {
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .split(frame.area());
    widgets::render_header(
        frame,
        chunks[0],
        Line::from(vec![Span::styled("HELP", theme.accent_style())]),
        format!("eword {}", env!("CARGO_PKG_VERSION")),
        theme,
    );

    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled("KEYS", theme.label())));
    for (key, desc) in [
        ("ctrl+c", "quit"),
        ("esc", "back / cancel"),
        ("enter", "submit / next"),
        ("shift+enter", "newline in a field"),
        ("tab", "next field"),
        ("ctrl+s", "save form"),
        ("e", "edit answer in check view"),
        ("r", "clear answer and retry"),
        ("f2", "switch EN / 中文 input"),
        ("↑↓", "move / scroll"),
        ("/", "open command palette on home"),
    ] {
        lines.push(Line::from(vec![
            Span::styled(format!("{key:<14}"), theme.accent_style()),
            Span::styled(desc.to_string(), theme.muted_style()),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("COMMANDS", theme.label())));
    for spec in COMMANDS {
        lines.push(Line::from(vec![
            Span::styled(format!("{:<30}", spec.usage), theme.accent_style()),
            Span::styled(spec.desc.to_string(), theme.muted_style()),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("DATA", theme.label())));
    lines.push(Line::from(Span::styled(
        format!("database  {}", paths.db.display()),
        theme.muted_style(),
    )));
    lines.push(Line::from(Span::styled(
        format!("backups   {}", paths.backups.display()),
        theme.muted_style(),
    )));
    if let Some(dir) = &config.sync_dir {
        let mut suffix = String::new();
        if config.sync_git {
            suffix.push_str("  · git");
        }
        if config.auto_sync {
            suffix.push_str("  · auto");
        }
        lines.push(Line::from(Span::styled(
            format!("sync      {}{suffix}", dir.display()),
            theme.muted_style(),
        )));
    }

    let content = widgets::content_rect(chunks[1]);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((help.scroll, 0))
            .style(theme.base()),
        content,
    );
    widgets::render_footer(
        frame,
        chunks[2],
        widgets::hint_line(&[("esc", "back"), ("↑↓", "scroll")], theme),
        theme,
    );
}

fn header_text(stats: &HeaderStats) -> String {
    let mut parts = vec![format!("{} words", stats.words)];
    if stats.today_attempts > 0 {
        parts.push(format!("{} today", stats.today_attempts));
    }
    if let Some(ratio) = stats.avg_ratio {
        parts.push(format!("{:.0}% blind", ratio * 100.0));
    }
    parts.join(" · ")
}

fn score_line(result: &DiffResult, theme: &Theme) -> Line<'static> {
    if result.exact {
        return Line::from(vec![Span::styled("✓ exact match", theme.success_style())]);
    }
    let ratio = result.char_ratio;
    let style = if ratio >= 0.9 {
        theme.success_style()
    } else if ratio >= 0.75 {
        theme.warning_style()
    } else {
        theme.error_style()
    };
    Line::from(vec![
        Span::styled(format!("{:.0}%", ratio * 100.0), style),
        Span::styled(
            format!(
                "  ·  {} edits  ·  {:.0}% words",
                result.changed_words,
                result.word_ratio * 100.0
            ),
            theme.muted_style(),
        ),
    ])
}

fn diff_lines(result: &DiffResult, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(result.rows.len());
    for row in &result.rows {
        let (prefix, prefix_style) = match row.kind {
            DiffKind::Equal => ("  ", theme.muted_style()),
            DiffKind::Delete => ("- ", theme.error_style()),
            DiffKind::Insert => ("+ ", theme.success_style()),
        };
        let mut spans = vec![Span::styled(prefix.to_string(), prefix_style)];
        for span in &row.spans {
            let style = match row.kind {
                DiffKind::Equal => theme.base(),
                DiffKind::Delete if span.emphasized => {
                    theme.error_style().add_modifier(Modifier::CROSSED_OUT)
                }
                DiffKind::Delete => theme.muted_style(),
                DiffKind::Insert if span.emphasized => {
                    theme.success_style().add_modifier(Modifier::UNDERLINED)
                }
                DiffKind::Insert => theme.muted_style(),
            };
            spans.push(Span::styled(span.text.clone(), style));
        }
        lines.push(Line::from(spans));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{FormReturn, FormState, HelpState, HomeState, Modal, ReviewState, StatsState};
    use crate::config::{Config, Paths};
    use crate::db::Db;
    use crate::model::{Stats, Word};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tempfile::tempdir;

    fn test_app() -> App {
        let dir = tempdir().unwrap();
        let paths = Paths::discover(Some(dir.path().to_path_buf())).unwrap();
        let db = Db::open_in_memory().unwrap();
        App::new(db, Config::default(), paths, Theme::dark()).unwrap()
    }

    fn screen_text(app: &mut App) -> String {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        let area = buffer.area;
        let mut text = String::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                if let Some(cell) = buffer.cell((x, y)) {
                    text.push_str(cell.symbol());
                }
            }
            text.push('\n');
        }
        text
    }

    fn sample_word() -> Word {
        Word {
            id: 1,
            sync_id: "test-sync-id".to_string(),
            term: "serendipity".to_string(),
            definition: "意外发现美好事物的能力".to_string(),
            example_en: "He found the letter by sheer serendipity.".to_string(),
            example_zh: "机缘巧合之下，他发现了那封信。".to_string(),
            created_at: 0,
            updated_at: 0,
            deleted_at: None,
        }
    }

    #[test]
    fn home_renders_empty_state() {
        let mut app = test_app();
        let text = screen_text(&mut app);
        assert!(text.contains("Eword"));
        assert!(text.contains("No words yet"));
        assert!(text.contains("type a word"));
    }

    #[test]
    fn home_renders_command_palette() {
        let mut app = test_app();
        if let Screen::Home(home) = &mut app.screen {
            home.input.insert_str("/");
            home.palette_open = true;
        }
        let text = screen_text(&mut app);
        assert!(text.contains("COMMANDS"));
        assert!(text.contains("/find"));
    }

    #[test]
    fn form_renders_labels_and_prefilled_word() {
        let mut app = test_app();
        app.screen = Screen::Form(FormState::new_new(Some("serendipity"), FormReturn::Home));
        let text = screen_text(&mut app);
        assert!(text.contains("NEW WORD"));
        assert!(text.contains("MEANING"));
        assert!(text.contains("serendipity"));
    }

    #[test]
    fn review_prompt_renders_translation_not_example() {
        let mut app = test_app();
        app.screen = Screen::Review(ReviewState::new(sample_word()));
        let text = screen_text(&mut app);
        assert!(text.contains("REVIEW"));
        assert!(text.contains("TRANSLATE THIS EXAMPLE"));
        assert!(text.contains("YOUR ANSWER"));
        assert!(!text.contains("He found the letter by sheer serendipity."));
    }

    #[test]
    fn review_result_renders_diff_and_score() {
        let mut app = test_app();
        let mut review = ReviewState::new(sample_word());
        review.phase = ReviewPhase::Result;
        review.result = Some(crate::diff::compute(
            "He found the letter by luck.",
            "He found the letter by sheer serendipity.",
        ));
        app.screen = Screen::Review(review);
        let text = screen_text(&mut app);
        assert!(text.contains("CHECK"));
        assert!(text.contains("luck"));
        assert!(text.contains("serendipity"));
    }

    #[test]
    fn browse_renders_empty_state() {
        let mut app = test_app();
        app.screen = Screen::Browse(BrowseState::new(Vec::new()));
        let text = screen_text(&mut app);
        assert!(text.contains("WORDS"));
        assert!(text.contains("no words yet"));
    }

    #[test]
    fn stats_and_help_render() {
        let mut app = test_app();
        app.screen = Screen::Stats(StatsState {
            stats: Stats::default(),
        });
        let text = screen_text(&mut app);
        assert!(text.contains("PROGRESS"));
        assert!(text.contains("WEAKEST"));

        let mut app = test_app();
        app.screen = Screen::Help(HelpState::default());
        let text = screen_text(&mut app);
        assert!(text.contains("HELP"));
        assert!(text.contains("COMMANDS"));
        assert!(text.contains("ctrl+c"));
    }

    #[test]
    fn modal_renders() {
        let mut app = test_app();
        app.modal = Some(Modal::ConfirmQuit);
        let text = screen_text(&mut app);
        assert!(text.contains("QUIT"));
        assert!(text.contains("Quit eword?"));
    }

    #[test]
    fn home_renders_when_empty_but_not_too_small() {
        let mut app = test_app();
        app.screen = Screen::Home(HomeState::new());
        let _ = screen_text(&mut app);
    }
}
