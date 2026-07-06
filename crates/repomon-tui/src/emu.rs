//! The embedded terminal renderer: a vt100 emulator fed by the daemon's `event.agent.bytes`
//! stream (tmux `pipe-pane` under the hood), rendered straight into the ratatui buffer.
//!
//! This replaces the lossy capture→re-parse pipeline for the Focus view: the emulator sees
//! the pane's actual byte stream, so alternate-screen switches, absolute cursor addressing,
//! and every SGR attribute render exactly as a real terminal would. Seeded from one
//! `capture-pane -e` snapshot; the first full app redraw (the SIGWINCH from `agent.resize`)
//! corrects any seed drift.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use repomon_core::model::LaneId;

/// One embedded terminal: the emulator state for the currently focused pane.
pub struct Emu {
    pub lane: LaneId,
    pub window: String,
    parser: vt100::Parser,
}

impl Emu {
    pub fn new(lane: LaneId, window: String, rows: u16, cols: u16) -> Self {
        Emu {
            lane,
            window,
            parser: vt100::Parser::new(rows, cols, 0),
        }
    }

    /// Feed raw PTY bytes (the daemon's stream, or the seed capture).
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    /// Seed from a `capture-pane -e` snapshot: captured lines are `\n`-joined, but a bare LF
    /// only moves the emulator down — re-anchor each line to column 0.
    pub fn seed_capture(&mut self, capture: &str) {
        self.feed(capture.replace('\n', "\r\n").as_bytes());
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.set_size(rows, cols);
    }

    pub fn size(&self) -> (u16, u16) {
        self.parser.screen().size()
    }

    /// The visible cursor as `(col, row)`, `None` while the app hides it.
    pub fn cursor(&self) -> Option<(u16, u16)> {
        let screen = self.parser.screen();
        if screen.hide_cursor() {
            return None;
        }
        let (row, col) = screen.cursor_position();
        Some((col, row))
    }

    /// Whether the app requested bracketed paste — pastes should then be wrapped in the
    /// `ESC[200~` / `ESC[201~` markers so it can tell paste from typing.
    pub fn bracketed_paste(&self) -> bool {
        self.parser.screen().bracketed_paste()
    }

    /// Draw the emulator's grid into `rect` (clipped to both the grid and the rect).
    pub fn render(&self, rect: Rect, buf: &mut Buffer) {
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        for r in 0..rows.min(rect.height) {
            for c in 0..cols.min(rect.width) {
                let Some(cell) = screen.cell(r, c) else {
                    continue;
                };
                let out = &mut buf[(rect.x + c, rect.y + r)];
                let contents = cell.contents();
                if contents.is_empty() {
                    out.set_symbol(" ");
                } else {
                    out.set_symbol(&contents);
                }
                out.set_style(cell_style(cell));
            }
        }
    }
}

fn cell_style(cell: &vt100::Cell) -> Style {
    let mut style = Style::default()
        .fg(vt_color(cell.fgcolor()))
        .bg(vt_color(cell.bgcolor()));
    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.inverse() {
        style = style.add_modifier(Modifier::REVERSED);
    }
    style
}

fn vt_color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_to_strings(emu: &Emu, w: u16, h: u16) -> Vec<String> {
        let rect = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(rect);
        emu.render(rect, &mut buf);
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn renders_text_colors_and_absolute_cursor_addressing() {
        let mut emu = Emu::new(1, "lane-1".into(), 5, 20);
        // Red text, then jump to row 3 col 5 (1-based in the escape) and write there — the
        // kind of absolute addressing the old line-based pipeline could not represent.
        emu.feed(b"\x1b[31mred\x1b[0m\r\n");
        emu.feed(b"\x1b[3;5HJUMPED");
        let rows = render_to_strings(&emu, 20, 5);
        assert!(rows[0].starts_with("red"), "row0: {:?}", rows[0]);
        assert!(rows[2].contains("JUMPED"), "row2: {:?}", rows[2]);

        let rect = Rect::new(0, 0, 20, 5);
        let mut buf = Buffer::empty(rect);
        emu.render(rect, &mut buf);
        assert_eq!(buf[(0, 0)].style().fg, Some(Color::Indexed(1)), "red fg");

        // The cursor sits right after JUMPED: col 4+6=10, row 2 (0-based).
        assert_eq!(emu.cursor(), Some((10, 2)));
        // An app hiding the cursor hides ours.
        emu.feed(b"\x1b[?25l");
        assert_eq!(emu.cursor(), None);
    }

    #[test]
    fn alternate_screen_switches_and_restores() {
        let mut emu = Emu::new(1, "lane-1".into(), 4, 10);
        emu.feed(b"main text");
        emu.feed(b"\x1b[?1049h\x1b[2J\x1b[HALT");
        let rows = render_to_strings(&emu, 10, 4);
        assert!(rows[0].starts_with("ALT"), "alt screen: {:?}", rows[0]);
        assert!(!rows[0].contains("main"), "main hidden: {:?}", rows[0]);
        emu.feed(b"\x1b[?1049l");
        let rows = render_to_strings(&emu, 10, 4);
        assert!(rows[0].starts_with("main text"), "restored: {:?}", rows[0]);
    }

    #[test]
    fn seed_capture_reanchors_lines_and_tracks_paste_mode() {
        let mut emu = Emu::new(1, "lane-1".into(), 4, 12);
        emu.seed_capture("first\nsecond");
        let rows = render_to_strings(&emu, 12, 4);
        assert!(rows[0].starts_with("first"), "{:?}", rows[0]);
        assert!(
            rows[1].starts_with("second"),
            "bare LF must not stairstep: {:?}",
            rows[1]
        );

        assert!(!emu.bracketed_paste());
        emu.feed(b"\x1b[?2004h");
        assert!(emu.bracketed_paste());
    }
}
