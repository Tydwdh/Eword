use ratatui::style::{Color, Modifier, Style};
use terminal_colorsaurus::{QueryOptions, ThemeMode, theme_mode};

use crate::config::Config;

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub accent: Color,
    pub fg: Color,
    pub muted: Color,
    pub border: Color,
    pub success: Color,
    pub error: Color,
    pub warning: Color,
    pub surface: Color,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            accent: Color::Rgb(138, 180, 248),
            fg: Color::Rgb(230, 230, 234),
            muted: Color::Rgb(139, 139, 149),
            border: Color::Rgb(42, 42, 51),
            success: Color::Rgb(139, 213, 160),
            error: Color::Rgb(242, 139, 155),
            warning: Color::Rgb(224, 175, 104),
            surface: Color::Rgb(24, 24, 27),
        }
    }

    pub fn light() -> Self {
        Self {
            accent: Color::Rgb(11, 110, 153),
            fg: Color::Rgb(31, 35, 40),
            muted: Color::Rgb(110, 119, 129),
            border: Color::Rgb(208, 215, 222),
            success: Color::Rgb(46, 125, 50),
            error: Color::Rgb(198, 40, 40),
            warning: Color::Rgb(161, 92, 0),
            surface: Color::Rgb(246, 248, 250),
        }
    }

    pub fn mono() -> Self {
        Self {
            accent: Color::Reset,
            fg: Color::Reset,
            muted: Color::DarkGray,
            border: Color::DarkGray,
            success: Color::Reset,
            error: Color::Reset,
            warning: Color::Reset,
            surface: Color::Reset,
        }
    }

    pub fn from_name(name: &str) -> Self {
        match name {
            "dark" => Self::dark(),
            "light" => Self::light(),
            "mono" => Self::mono(),
            _ => Self::detect(),
        }
    }

    pub fn from_config(config: &Config) -> Self {
        if no_color() {
            Self::mono()
        } else {
            Self::from_name(&config.theme)
        }
    }

    fn detect() -> Self {
        if no_color() {
            return Self::mono();
        }
        match theme_mode(QueryOptions::default()) {
            Ok(ThemeMode::Light) => Self::light(),
            _ => Self::dark(),
        }
    }

    pub fn base(&self) -> Style {
        Style::default().fg(self.fg)
    }

    pub fn muted_style(&self) -> Style {
        Style::default().fg(self.muted)
    }

    pub fn label(&self) -> Style {
        Style::default().fg(self.muted).add_modifier(Modifier::BOLD)
    }

    pub fn accent_style(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .add_modifier(Modifier::BOLD)
    }

    pub fn border_style(&self) -> Style {
        Style::default().fg(self.border)
    }

    pub fn success_style(&self) -> Style {
        Style::default()
            .fg(self.success)
            .add_modifier(Modifier::BOLD)
    }

    pub fn error_style(&self) -> Style {
        Style::default().fg(self.error).add_modifier(Modifier::BOLD)
    }

    pub fn warning_style(&self) -> Style {
        Style::default()
            .fg(self.warning)
            .add_modifier(Modifier::BOLD)
    }
}

fn no_color() -> bool {
    std::env::var_os("NO_COLOR")
        .map(|value| !value.is_empty())
        .unwrap_or(false)
}
