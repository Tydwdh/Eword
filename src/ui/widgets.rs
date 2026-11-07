use std::collections::HashSet;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::Style;
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui_textarea::TextArea;
use unicode_width::UnicodeWidthStr;

use crate::app::{HomeState, Modal, Toast, ToastKind};
use crate::commands::{self, CommandSpec};
use crate::fuzzy::fuzzy_match;
use crate::ui::theme::Theme;

pub fn content_rect(area: Rect) -> Rect {
    let margin = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let width = margin.width.min(88);
    Rect {
        x: margin.x + margin.width.saturating_sub(width) / 2,
        y: margin.y,
        width,
        height: margin.height,
    }
}

pub fn render_header(
    frame: &mut Frame,
    area: Rect,
    left: Line<'static>,
    right: String,
    theme: &Theme,
) {
    let line_area = Rect { height: 1, ..area };
    let right = truncate_display(&right, area.width.saturating_sub(12));
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(right, theme.muted_style())))
            .alignment(Alignment::Right),
        line_area,
    );
    frame.render_widget(Paragraph::new(left), line_area);
    if area.height > 1 {
        render_rule(
            frame,
            Rect {
                y: area.y + 1,
                height: 1,
                ..area
            },
            theme,
        );
    }
}

pub fn render_rule(frame: &mut Frame, area: Rect, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(theme.border_style());
    frame.render_widget(block, area);
}

pub fn render_label(frame: &mut Frame, area: Rect, text: &str, active: bool, theme: &Theme) {
    let style = if active {
        theme.accent_style()
    } else {
        theme.label()
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(text.to_string(), style))),
        area,
    );
}

pub fn render_field(
    frame: &mut Frame,
    textarea: &mut TextArea<'static>,
    area: Rect,
    active: bool,
    allow_cursor: bool,
    theme: &Theme,
) -> Rect {
    textarea.set_style(theme.base());
    textarea.set_placeholder_style(theme.muted_style());

    // Reserve the same one-column gutter for every field so text does not
    // jump sideways when focus moves. Active fields show an accent bar;
    // inactive fields use a blank border symbol.
    const BLANK_LEFT: border::Set<'static> = border::Set {
        top_left: "",
        top_right: "",
        bottom_left: "",
        bottom_right: "",
        vertical_left: " ",
        vertical_right: "",
        horizontal_top: "",
        horizontal_bottom: "",
    };
    let block = if active {
        Block::default()
            .borders(Borders::LEFT)
            .border_type(BorderType::Thick)
            .border_style(theme.accent_style())
    } else {
        Block::default()
            .borders(Borders::LEFT)
            .border_set(BLANK_LEFT)
            .border_style(Style::default())
    };
    let inner = block.inner(area);
    textarea.set_block(block);
    frame.render_widget(&*textarea, area);
    if active && allow_cursor && inner.width > 0 && inner.height > 0 {
        let cursor = textarea.screen_cursor();
        let x = inner
            .x
            .saturating_add(cursor.col as u16)
            .min(inner.x + inner.width - 1);
        let y = inner
            .y
            .saturating_add(cursor.row as u16)
            .min(inner.y + inner.height - 1);
        frame.set_cursor_position((x, y));
    }
    inner
}

pub fn hint_line(items: &[(&str, &str)], theme: &Theme) -> Line<'static> {
    let mut spans = Vec::new();
    for (index, (key, description)) in items.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("  ·  ", theme.muted_style()));
        }
        spans.push(Span::styled(key.to_string(), theme.accent_style()));
        spans.push(Span::styled(format!(" {description}"), theme.muted_style()));
    }
    Line::from(spans)
}

pub fn render_footer(frame: &mut Frame, area: Rect, line: Line<'static>, theme: &Theme) {
    frame.render_widget(Paragraph::new(line).style(theme.muted_style()), area);
}

pub fn render_palette(frame: &mut Frame, home: &HomeState, anchor: Rect, theme: &Theme) {
    let query = crate::app::single_line_text(&home.input);
    let filtered = commands::filtered(&query);
    let visible = filtered.len().min(7);
    let height = (visible as u16 + 3).max(5);
    let width = anchor.width.clamp(44, 62);
    let screen = frame.area();
    let x = anchor.x.min(screen.x + screen.width.saturating_sub(width));
    let mut y = anchor.y.saturating_add(anchor.height).saturating_add(1);
    if y + height > screen.y + screen.height.saturating_sub(1) {
        y = anchor.y.saturating_sub(height + 1);
    }
    let area = Rect {
        x,
        y,
        width: width.min(screen.width),
        height: height.min(screen.height),
    };
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border_style())
        .title(Line::from(Span::styled(" COMMANDS ", theme.label())));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if filtered.is_empty() {
        frame.render_widget(
            Paragraph::new("no matching commands")
                .style(theme.muted_style())
                .alignment(Alignment::Center),
            inner,
        );
        return;
    }

    let rows = Layout::vertical(
        (0..visible)
            .map(|_| Constraint::Length(1))
            .chain(std::iter::once(Constraint::Fill(1)))
            .collect::<Vec<_>>(),
    )
    .split(inner);

    let start = if filtered.len() > visible {
        home.palette_selected
            .saturating_sub(visible.saturating_sub(1))
            .min(filtered.len().saturating_sub(visible))
    } else {
        0
    };
    for (offset, spec) in filtered.iter().skip(start).take(visible).enumerate() {
        let selected = start + offset == home.palette_selected;
        let line = command_line(spec, &query, selected, theme, inner.width);
        frame.render_widget(Paragraph::new(line), rows[offset]);
    }

    if inner.height as usize > visible {
        let hint = Paragraph::new(hint_line(
            &[
                ("↑↓", "select"),
                ("tab", "complete"),
                ("enter", "run"),
                ("esc", "close"),
            ],
            theme,
        ));
        frame.render_widget(hint, rows[visible]);
    }
}

fn command_line(
    spec: &CommandSpec,
    query: &str,
    selected: bool,
    theme: &Theme,
    width: u16,
) -> Line<'static> {
    let name = format!("/{}", spec.name);
    let needle = query.trim_start_matches('/');
    let matched = fuzzy_match(needle, spec.name);
    let indices: HashSet<usize> = matched
        .map(|matched| matched.indices.into_iter().map(|index| index + 1).collect())
        .unwrap_or_default();

    let mut spans = Vec::new();
    spans.push(Span::styled(
        if selected { "› " } else { "  " },
        if selected {
            theme.accent_style()
        } else {
            theme.muted_style()
        },
    ));

    let mut current = String::new();
    let mut highlighted = false;
    for (index, ch) in name.chars().enumerate() {
        let is_highlighted = indices.contains(&index);
        if !current.is_empty() && is_highlighted != highlighted {
            spans.push(Span::styled(
                std::mem::take(&mut current),
                style_for(highlighted, selected, theme),
            ));
        }
        highlighted = is_highlighted;
        current.push(ch);
    }
    if !current.is_empty() {
        spans.push(Span::styled(
            current,
            style_for(highlighted, selected, theme),
        ));
    }

    let desc_width = width
        .saturating_sub(2)
        .saturating_sub(UnicodeWidthStr::width(name.as_str()) as u16)
        .saturating_sub(3) as usize;
    let desc = if UnicodeWidthStr::width(spec.desc) > desc_width {
        let mut value = String::new();
        let mut used = 0usize;
        for ch in spec.desc.chars() {
            let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if used + w > desc_width.saturating_sub(1) {
                break;
            }
            used += w;
            value.push(ch);
        }
        format!("{value}…")
    } else {
        spec.desc.to_string()
    };
    spans.push(Span::styled(format!("  {desc}"), theme.muted_style()));
    Line::from(spans)
}

fn style_for(highlighted: bool, selected: bool, theme: &Theme) -> Style {
    if highlighted || selected {
        theme.accent_style()
    } else {
        theme.base()
    }
}

pub fn render_modal(frame: &mut Frame, modal: &Modal, theme: &Theme) {
    let screen = frame.area();
    let (title, lines, hint) = match modal {
        Modal::ConfirmArchive(word) => (
            " ARCHIVE ",
            vec![
                Line::from(vec![
                    Span::styled("Archive ", theme.base()),
                    Span::styled(word.term.clone(), theme.accent_style()),
                    Span::styled(" ?", theme.base()),
                ]),
                Line::from(Span::styled(
                    "Its attempts are kept. /restore can undo.",
                    theme.muted_style(),
                )),
            ],
            "enter archive · esc cancel",
        ),
        Modal::ConfirmQuit => (
            " QUIT ",
            vec![
                Line::from(Span::styled("Quit eword?", theme.base())),
                Line::from(Span::styled(
                    "Unsaved form text will be lost.",
                    theme.muted_style(),
                )),
            ],
            "enter quit · esc cancel",
        ),
        Modal::ConfirmDiscardForm => (
            " DISCARD ",
            vec![
                Line::from(Span::styled("Discard this draft?", theme.base())),
                Line::from(Span::styled(
                    "The fields have unsaved changes.",
                    theme.muted_style(),
                )),
            ],
            "enter discard · esc keep editing",
        ),
        Modal::DidYouMean {
            input,
            suggestions,
            selected,
        } => {
            let mut lines = vec![Line::from(vec![
                Span::styled("Not found: ", theme.muted_style()),
                Span::styled(input.clone(), theme.base()),
            ])];
            for (index, suggestion) in suggestions.iter().enumerate() {
                let marker = if index == *selected { "› " } else { "  " };
                lines.push(Line::from(vec![
                    Span::styled(marker, theme.accent_style()),
                    Span::styled(suggestion.clone(), theme.accent_style()),
                ]));
            }
            (
                " DID YOU MEAN ",
                lines,
                "enter use · n create new · esc cancel",
            )
        }
    };

    let width = screen.width.clamp(36, 64);
    let height = (lines.len() as u16 + 4)
        .min(screen.height.saturating_sub(2))
        .max(5);
    let x = screen.x + (screen.width.saturating_sub(width)) / 2;
    let y = screen.y + (screen.height.saturating_sub(height)) / 2;
    let area = Rect {
        x,
        y,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border_style())
        .title(Line::from(Span::styled(title, theme.label())));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let content = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).split(inner);
    let mut body = lines;
    frame.render_widget(
        Paragraph::new(std::mem::take(&mut body))
            .wrap(Wrap { trim: false })
            .style(theme.base()),
        content[0],
    );
    frame.render_widget(Paragraph::new(hint).style(theme.muted_style()), content[1]);
}

fn truncate_display(text: &str, max: u16) -> String {
    let max = max as usize;
    if UnicodeWidthStr::width(text) <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + width > max.saturating_sub(1) {
            break;
        }
        used += width;
        out.push(ch);
    }
    out.push('…');
    out
}

pub fn render_toast(frame: &mut Frame, toast: &Toast, theme: &Theme) {
    let screen = frame.area();
    let style = match toast.kind {
        ToastKind::Success => theme.success_style(),
        ToastKind::Error => theme.error_style(),
        ToastKind::Warning => theme.warning_style(),
        ToastKind::Info => theme.accent_style(),
    };
    let prefix = match toast.kind {
        ToastKind::Success => "✓ ",
        ToastKind::Error => "! ",
        ToastKind::Warning => "! ",
        ToastKind::Info => "· ",
    };
    let message = format!("{prefix}{}", toast.message);
    let message_width = UnicodeWidthStr::width(message.as_str()) as u16;
    let max_width = screen.width.saturating_sub(4);
    let width = (message_width + 4).min(max_width).max(20);
    let inner_width = width.saturating_sub(2).max(1);
    let lines = message_width.saturating_add(inner_width.saturating_sub(1)) / inner_width;
    let height = lines.clamp(1, 3) + 2;
    let x = screen.x + screen.width.saturating_sub(width + 2);
    let y = screen.y + screen.height.saturating_sub(height + 2);
    let area = Rect {
        x,
        y,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(style)
        .style(Style::default().bg(theme.surface));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(Span::styled(message, style))
            .wrap(Wrap { trim: true })
            .style(Style::default().bg(theme.surface)),
        inner,
    );
}

pub fn too_small(frame: &mut Frame, area: Rect, theme: &Theme) {
    let content = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(2),
        Constraint::Fill(1),
    ])
    .split(area)[1];
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled("terminal too small", theme.accent_style())),
            Line::from(Span::styled("need at least 56 × 20", theme.muted_style())),
        ])
        .alignment(Alignment::Center),
        content,
    );
}
