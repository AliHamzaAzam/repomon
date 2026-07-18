//! The server-side terminal screen: a vt100 emulator with tmux-parity scrollback.
//!
//! This is the source of truth for `capture`/`cursor`/`size`/`alternate_on` and for the
//! `subscribe_bytes` first-frame replay — ConPTY rendering quirks never leak past it.
//! Pure logic, tested on every OS with canned byte streams.

/// Scrollback depth, parity with tmux `configure()`'s `history-limit 50000`.
pub const HISTORY_LIMIT: usize = 50_000;

/// A vt100 screen sized `cols × rows` with [`HISTORY_LIMIT`] lines of scrollback.
///
/// All geometry at this boundary is `(cols, rows)` / `(col, row)` — protocol order — even
/// though vt100 itself speaks `(rows, cols)`.
pub struct Screen {
    parser: vt100::Parser,
}

impl Screen {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            parser: vt100::Parser::new(rows, cols, HISTORY_LIMIT),
        }
    }

    /// Feed raw PTY output bytes.
    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    /// Parity with `tmux capture-pane -e -p [-S -lines]`: up to `lines` rows of scrollback,
    /// then every visible row, joined with `\n`, each row carrying its inline SGR escapes.
    pub fn capture(&mut self, lines: Option<u32>) -> String {
        let (_, cols) = self.parser.screen().size();
        let mut out: Vec<String> = Vec::new();
        if let Some(want) = lines {
            // Clamp to the scrollback actually available (set_scrollback clamps for us).
            self.parser.set_scrollback(usize::MAX);
            let avail = self.parser.screen().scrollback();
            let take = (want as usize).min(avail);
            // At offset k the top visible row is the row k above the live top: walking k from
            // `take` down to 1 yields the history rows oldest-first, ending flush against the
            // live screen.
            for k in (1..=take).rev() {
                self.parser.set_scrollback(k);
                let row = self
                    .parser
                    .screen()
                    .rows_formatted(0, cols)
                    .next()
                    .unwrap_or_default();
                out.push(String::from_utf8_lossy(&row).into_owned());
            }
        }
        self.parser.set_scrollback(0);
        for row in self.parser.screen().rows_formatted(0, cols) {
            out.push(String::from_utf8_lossy(&row).into_owned());
        }
        out.join("\n")
    }

    /// `(col, row, visible)`, 0-based — parity with `#{cursor_x}/#{cursor_y}/#{cursor_flag}`.
    pub fn cursor(&self) -> (u16, u16, bool) {
        let screen = self.parser.screen();
        let (row, col) = screen.cursor_position();
        (col, row, !screen.hide_cursor())
    }

    /// `(cols, rows)` — parity with `#{pane_width}/#{pane_height}`.
    pub fn size(&self) -> (u16, u16) {
        let (rows, cols) = self.parser.screen().size();
        (cols, rows)
    }

    /// Whether the child is on the alternate screen — parity with `#{alternate_on}`.
    pub fn alternate_on(&self) -> bool {
        self.parser.screen().alternate_screen()
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.parser.set_size(rows, cols);
    }

    /// The `subscribe_bytes` first frame: bytes that recreate the current terminal state
    /// (contents, attributes, cursor, input modes) from scratch on an empty emulator.
    pub fn replay(&self) -> Vec<u8> {
        self.parser.screen().state_formatted()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_limit_matches_tmux_configure() {
        assert_eq!(HISTORY_LIMIT, 50_000);
    }

    #[test]
    fn capture_renders_visible_rows() {
        let mut s = Screen::new(80, 24);
        s.process(b"hello\r\nworld");
        let text = s.capture(None);
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(lines.len(), 24, "one line per visible row");
        assert_eq!(lines[0], "hello");
        assert_eq!(lines[1], "world");
    }

    #[test]
    fn capture_preserves_sgr_escapes() {
        let mut s = Screen::new(80, 24);
        s.process(b"\x1b[31mred\x1b[0m plain");
        let text = s.capture(None);
        assert!(text.contains("red"), "text survives: {text:?}");
        assert!(text.contains('\x1b'), "escapes included: {text:?}");
    }

    #[test]
    fn capture_with_lines_prepends_scrollback() {
        let mut s = Screen::new(80, 24);
        for i in 0..30 {
            s.process(format!("line-{i}\r\n").as_bytes());
        }
        // 31 rows used (trailing prompt row) on a 24-row screen → 7 rows of scrollback.
        let visible = s.capture(None);
        assert!(
            !visible.contains("line-0"),
            "line-0 scrolled out: {visible:?}"
        );

        let with_history = s.capture(Some(10));
        let lines: Vec<&str> = with_history.split('\n').collect();
        assert_eq!(lines.len(), 24 + 7, "clamped to available scrollback");
        assert_eq!(lines[0], "line-0");
        assert_eq!(lines[6], "line-6");
        assert_eq!(
            lines[7], "line-7",
            "visible screen follows history seamlessly"
        );

        // Asking for less history than available takes the newest rows.
        let two = s.capture(Some(2));
        let lines: Vec<&str> = two.split('\n').collect();
        assert_eq!(lines.len(), 26);
        assert_eq!(lines[0], "line-5");
    }

    #[test]
    fn capture_after_scrollback_read_leaves_view_at_bottom() {
        let mut s = Screen::new(80, 24);
        for i in 0..30 {
            s.process(format!("line-{i}\r\n").as_bytes());
        }
        let _ = s.capture(Some(5));
        // A later plain capture must see the live (bottom) view, not a scrolled one.
        let visible = s.capture(None);
        assert!(
            visible.contains("line-29"),
            "back at the live view: {visible:?}"
        );
    }

    #[test]
    fn cursor_tracks_position_and_visibility() {
        let mut s = Screen::new(80, 24);
        s.process(b"abc");
        assert_eq!(s.cursor(), (3, 0, true), "(col, row, visible)");
        s.process(b"\x1b[?25l");
        assert!(!s.cursor().2, "hidden cursor reports visible=false");
        s.process(b"\x1b[?25h\x1b[5;11H");
        assert_eq!(s.cursor(), (10, 4, true), "0-based col/row");
    }

    #[test]
    fn size_and_resize() {
        let mut s = Screen::new(220, 50);
        assert_eq!(s.size(), (220, 50), "(cols, rows)");
        s.resize(100, 30);
        assert_eq!(s.size(), (100, 30));
        assert_eq!(s.capture(None).split('\n').count(), 30);
    }

    #[test]
    fn alternate_screen_tracking() {
        let mut s = Screen::new(80, 24);
        assert!(!s.alternate_on());
        s.process(b"\x1b[?1049h");
        assert!(s.alternate_on(), "smcup enters the alternate screen");
        s.process(b"\x1b[?1049l");
        assert!(!s.alternate_on(), "rmcup leaves it");
    }

    #[test]
    fn replay_reproduces_screen_on_a_fresh_emulator() {
        let mut s = Screen::new(80, 24);
        s.process(b"\x1b[2;3H\x1b[1;32mgreen bold\x1b[0m\r\ntail");
        let replay = s.replay();

        let mut fresh = vt100::Parser::new(24, 80, 0);
        fresh.process(&replay);
        let (orig, copy) = (s.parser.screen(), fresh.screen());
        assert_eq!(copy.contents(), orig.contents(), "text converges");
        assert_eq!(
            copy.cursor_position(),
            orig.cursor_position(),
            "cursor converges"
        );
        assert_eq!(
            copy.cell(1, 2).unwrap().fgcolor(),
            orig.cell(1, 2).unwrap().fgcolor(),
            "attributes converge"
        );
    }
}
