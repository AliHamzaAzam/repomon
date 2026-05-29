//! Brutalist monochrome theme: single-char status glyphs, light/heavy rules, and the one
//! reserved accent slot (default `None`, wired up in Phase 4). No color in Phase 1.

use ratatui::style::{Color, Modifier, Style};

// Status glyphs.
pub const DIRTY: &str = "●";
pub const CLEAN: &str = "○";
pub const AGENT_ACTIVE: &str = "▶";
pub const WAITING: &str = "⏸";
pub const UP: char = '↑';
pub const DOWN: char = '↓';

// Horizontal rules.
pub const HEAVY: char = '━';
pub const LIGHT: char = '─';

// Density blocks, low to high (timeline, Phase 3).
pub const DENSITY: [&str; 6] = [" ", "▁", "░", "▒", "▓", "█"];

/// The theme. The accent slot is reserved; rendering stays monochrome until Phase 4.
#[derive(Debug, Clone, Copy, Default)]
pub struct Theme {
    pub accent: Option<Color>,
}

impl Theme {
    /// Selected row: reverse video, nothing else.
    pub fn selected(&self) -> Style {
        Style::default().add_modifier(Modifier::REVERSED)
    }
    pub fn bold(&self) -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }
    pub fn dim(&self) -> Style {
        Style::default().add_modifier(Modifier::DIM)
    }
    /// Accent style if an accent is configured, else plain.
    pub fn accented(&self) -> Style {
        match self.accent {
            Some(c) => Style::default().fg(c),
            None => Style::default(),
        }
    }
}
