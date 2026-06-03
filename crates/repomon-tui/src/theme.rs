//! Theme: a tasteful semantic color palette over the flat, brutalist layout. The status colors are
//! fixed (running=green, needs-you=amber, rate-limited=cyan, muted=gray) so meaning stays
//! consistent; the one configurable hue is the **accent** (headers, selection, dividers, dirty
//! marks) — any named color or `#hex`, default cyan. `accent = "mono"` turns color off for the
//! original monochrome look.

use ratatui::style::{Color, Modifier, Style};
use repomon_core::model::AgentStatus;

// Status glyphs.
pub const DIRTY: &str = "●";
pub const CLEAN: &str = "○";
pub const AGENT_ACTIVE: &str = "▶";
pub const WAITING: &str = "⏸";
pub const RATE_LIMITED: &str = "⏳";
pub const UP: char = '↑';
pub const DOWN: char = '↓';

// Horizontal rules.
pub const HEAVY: char = '━';
pub const LIGHT: char = '─';
// Vertical rule (column divider).
pub const VLIGHT: char = '│';

// Density blocks, low to high (timeline).
pub const DENSITY: [&str; 6] = [" ", "▁", "░", "▒", "▓", "█"];

// Fixed semantic colors — named ANSI so they respect the terminal's own palette.
const RUNNING: Color = Color::Green;
const NEEDS_YOU: Color = Color::Yellow;
const RATE_LIMIT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;

/// The theme: a configurable accent plus the fixed semantic palette. `colored = false` is the
/// monochrome escape hatch (`accent = "mono"`), reproducing the original no-color look.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub accent: Option<Color>,
    colored: bool,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::from_accent(None)
    }
}

impl Theme {
    /// Build a theme from the optional `accent` config value: `"mono"`/`"none"`/`"off"` → no
    /// color; a named color or `#hex` → that accent; unset → the default cyan accent.
    pub fn from_accent(name: Option<&str>) -> Self {
        match name.map(|n| n.trim().to_lowercase()) {
            Some(n) if matches!(n.as_str(), "mono" | "none" | "off") => Theme {
                accent: None,
                colored: false,
            },
            Some(n) => Theme {
                accent: Some(color_from_name(&n).unwrap_or(Color::Cyan)),
                colored: true,
            },
            None => Theme {
                accent: Some(Color::Cyan),
                colored: true,
            },
        }
    }

    /// Whether color is enabled (false = the monochrome escape hatch).
    pub fn colored(&self) -> bool {
        self.colored
    }

    /// A foreground color, or plain when color is off.
    fn fg(&self, c: Color) -> Style {
        if self.colored {
            Style::default().fg(c)
        } else {
            Style::default()
        }
    }

    // --- structural ---

    /// Selected row: reverse video, tinted by the accent when colored.
    pub fn selected(&self) -> Style {
        let s = Style::default().add_modifier(Modifier::REVERSED);
        match (self.colored, self.accent) {
            (true, Some(c)) => s.fg(c),
            _ => s,
        }
    }
    pub fn bold(&self) -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }
    pub fn dim(&self) -> Style {
        Style::default().add_modifier(Modifier::DIM)
    }
    /// Secondary text (times, footer, hints): gray when colored, dim otherwise.
    pub fn muted(&self) -> Style {
        if self.colored {
            Style::default().fg(MUTED)
        } else {
            Style::default().add_modifier(Modifier::DIM)
        }
    }
    /// Header/title: bold, tinted with the accent when colored.
    pub fn header_style(&self) -> Style {
        let s = Style::default().add_modifier(Modifier::BOLD);
        match (self.colored, self.accent) {
            (true, Some(c)) => s.fg(c),
            _ => s,
        }
    }
    /// Mouse-hover highlight — bold plus a subtle row background (when colored), distinct from the
    /// reverse-video selection. (Needs a terminal that reports mouse motion; not all do.)
    pub fn hover(&self) -> Style {
        let s = Style::default().add_modifier(Modifier::BOLD);
        if self.colored {
            s.bg(Color::Indexed(236))
        } else {
            s
        }
    }
    /// Accent foreground (dividers, dirty marks, active markers), else plain.
    pub fn accented(&self) -> Style {
        match (self.colored, self.accent) {
            (true, Some(c)) => Style::default().fg(c),
            _ => Style::default(),
        }
    }

    // --- semantic status colors (plain when mono) ---

    pub fn running(&self) -> Style {
        self.fg(RUNNING)
    }
    pub fn needs_you(&self) -> Style {
        self.fg(NEEDS_YOU)
    }
    pub fn rate_limited(&self) -> Style {
        self.fg(RATE_LIMIT)
    }
    pub fn idle(&self) -> Style {
        self.muted()
    }

    /// The style for an agent status — used by badges, glyphs, and mode lines.
    pub fn status(&self, status: AgentStatus) -> Style {
        match status {
            AgentStatus::Running => self.running(),
            AgentStatus::Waiting => self.needs_you(),
            AgentStatus::RateLimited => self.rate_limited(),
            _ => self.idle(),
        }
    }
}

/// Map a config color value to a ratatui color: a named ANSI color or a `#rrggbb`/`#rgb` hex.
/// Unknown names return `None` (the caller falls back to the default accent).
fn color_from_name(name: &str) -> Option<Color> {
    let n = name.trim().to_lowercase();
    if let Some(hex) = n.strip_prefix('#') {
        return parse_hex(hex);
    }
    match n.as_str() {
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "white" => Some(Color::White),
        "gray" | "grey" => Some(Color::Gray),
        "orange" | "amber" => Some(Color::Rgb(0xff, 0xb8, 0x6c)),
        _ => None,
    }
}

/// Parse `rrggbb` or `rgb` (no leading `#`) into a truecolor.
fn parse_hex(hex: &str) -> Option<Color> {
    let h: String = match hex.len() {
        6 => hex.to_string(),
        3 => hex.chars().flat_map(|c| [c, c]).collect(),
        _ => return None,
    };
    let r = u8::from_str_radix(&h[0..2], 16).ok()?;
    let g = u8::from_str_radix(&h[2..4], 16).ok()?;
    let b = u8::from_str_radix(&h[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_disables_color() {
        let t = Theme::from_accent(Some("mono"));
        assert!(!t.colored());
        // No foreground color in any semantic style.
        assert_eq!(t.running().fg, None);
        assert_eq!(t.needs_you().fg, None);
        assert_eq!(t.muted().fg, None);
        assert_eq!(t.header_style().fg, None);
        assert_eq!(t.selected().fg, None);
    }

    #[test]
    fn default_is_colored_cyan() {
        let t = Theme::from_accent(None);
        assert!(t.colored());
        assert_eq!(t.accent, Some(Color::Cyan));
        assert_eq!(t.running().fg, Some(Color::Green));
        assert_eq!(t.header_style().fg, Some(Color::Cyan));
    }

    #[test]
    fn accent_accepts_names_and_hex() {
        assert_eq!(Theme::from_accent(Some("green")).accent, Some(Color::Green));
        assert_eq!(
            Theme::from_accent(Some("#ff8800")).accent,
            Some(Color::Rgb(0xff, 0x88, 0x00))
        );
        // Unknown name → default cyan, still colored.
        let t = Theme::from_accent(Some("chartreuse"));
        assert!(t.colored());
        assert_eq!(t.accent, Some(Color::Cyan));
    }
}
