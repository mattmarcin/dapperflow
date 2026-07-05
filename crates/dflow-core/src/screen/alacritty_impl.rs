//! `ScreenModel` backed by `alacritty_terminal` (the M0 frontrunner:
//! published, maintained, used headless by Zed in exactly this pattern).
//!
//! This is the ONLY module in the workspace permitted to import
//! `alacritty_terminal` types. Everything else goes through the `ScreenModel`
//! trait, keeping the VT crate swappable (`architecture.md`).

use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor};

use dflow_proto::{CursorPos, StyledRun, StyledSnapshot};

use super::{ScreenModel, TerminalModes};

/// Scrollback history kept by the VT model, in lines. The raw-byte replay ring
/// lives separately in `ring.rs`; this bounds the styled/plain capture history.
const HISTORY_LINES: usize = 5_000;

/// Event listener that captures the terminal's own PTY-write responses (DSR/DA
/// replies and the like) so the session loop can feed them back to the PTY. Other
/// events (bell, title, clipboard) are ignored in Phase 0; OSC titles/hyperlinks
/// are on the M0 acceptance list but not yet wired to product behavior.
#[derive(Clone, Default)]
struct EventProxy {
    responses: Arc<Mutex<Vec<u8>>>,
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        if let Event::PtyWrite(text) = event {
            if let Ok(mut buf) = self.responses.lock() {
                buf.extend_from_slice(text.as_bytes());
            }
        }
    }
}

/// Terminal dimensions passed to `Term`. History capacity comes from `Config`,
/// not from this trait, so `total_lines == screen_lines` here.
#[derive(Clone, Copy)]
struct Dims {
    columns: usize,
    screen_lines: usize,
}

impl Dimensions for Dims {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }
    fn screen_lines(&self) -> usize {
        self.screen_lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

/// A `ScreenModel` implementation over `alacritty_terminal::Term`.
pub struct AlacrittyScreen {
    term: Term<EventProxy>,
    parser: Processor,
    responses: Arc<Mutex<Vec<u8>>>,
    cols: u16,
    rows: u16,
}

impl AlacrittyScreen {
    /// Create a screen of `cols` x `rows`.
    pub fn new(cols: u16, rows: u16) -> Self {
        let (cols, rows) = (cols.max(1), rows.max(1));
        let dims = Dims { columns: cols as usize, screen_lines: rows as usize };
        let config = Config { scrolling_history: HISTORY_LINES, ..Config::default() };
        let proxy = EventProxy::default();
        let responses = Arc::clone(&proxy.responses);
        let term = Term::new(config, &dims, proxy);
        Self { term, parser: Processor::new(), responses, cols, rows }
    }

    /// Read one grid cell's character, treating the null padding and the trailing
    /// half of a wide glyph as spaces so plain text lines up.
    fn char_at(&self, line: i32, col: usize) -> Option<char> {
        let cell = &self.term.grid()[Line(line)][Column(col)];
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            return None; // the wide glyph occupies the previous cell
        }
        let c = cell.c;
        Some(if c == '\0' { ' ' } else { c })
    }

    fn line_to_string(&self, line: i32) -> String {
        let cols = self.term.grid().columns();
        let mut s = String::with_capacity(cols);
        for col in 0..cols {
            if let Some(c) = self.char_at(line, col) {
                s.push(c);
            }
        }
        // Trim trailing blanks so plain captures are stable and compact.
        let trimmed = s.trim_end_matches(' ');
        trimmed.to_string()
    }
}

impl ScreenModel for AlacrittyScreen {
    fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    fn take_responses(&mut self) -> Vec<u8> {
        match self.responses.lock() {
            Ok(mut buf) => std::mem::take(&mut *buf),
            Err(_) => Vec::new(),
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        let (cols, rows) = (cols.max(1), rows.max(1));
        let dims = Dims { columns: cols as usize, screen_lines: rows as usize };
        self.term.resize(dims);
        self.cols = cols;
        self.rows = rows;
    }

    fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    fn cursor(&self) -> CursorPos {
        let point = self.term.grid().cursor.point;
        // Cursor is reported in visible-screen coordinates (row 0 is the top of the
        // viewport). The grid cursor line is already within the viewport range.
        CursorPos {
            col: point.column.0 as u16,
            row: point.line.0.max(0) as u16,
            visible: true,
        }
    }

    fn capture_plain(&self) -> String {
        let rows = self.term.grid().screen_lines() as i32;
        let mut out = String::new();
        for line in 0..rows {
            out.push_str(&self.line_to_string(line));
            if line + 1 < rows {
                out.push('\n');
            }
        }
        out
    }

    fn capture_scrollback(&self) -> String {
        let grid = self.term.grid();
        let top = grid.topmost_line().0; // most negative == oldest history line
        let bottom = grid.bottommost_line().0; // screen_lines - 1
        let mut out = String::new();
        for line in top..=bottom {
            out.push_str(&self.line_to_string(line));
            if line < bottom {
                out.push('\n');
            }
        }
        out
    }

    fn styled_snapshot(&self) -> StyledSnapshot {
        let grid = self.term.grid();
        let cols = grid.columns();
        let rows = grid.screen_lines();
        let mut lines: Vec<Vec<StyledRun>> = Vec::with_capacity(rows);

        for line in 0..rows as i32 {
            let mut runs: Vec<StyledRun> = Vec::new();
            let mut current: Option<StyledRun> = None;

            for col in 0..cols {
                let cell = &grid[Line(line)][Column(col)];
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    continue;
                }
                let ch = if cell.c == '\0' { ' ' } else { cell.c };
                let style = CellStyle::from_cell(cell.fg, cell.bg, cell.flags);

                match current.as_mut() {
                    Some(run) if style.matches(run) => run.text.push(ch),
                    _ => {
                        if let Some(run) = current.take() {
                            runs.push(run);
                        }
                        current = Some(style.into_run(ch));
                    }
                }
            }
            if let Some(run) = current.take() {
                runs.push(run);
            }
            // Drop a trailing run that is just default-styled spaces, keeping the
            // snapshot compact without losing meaningful trailing styling.
            if let Some(last) = runs.last() {
                if is_blank_default(last) {
                    runs.pop();
                }
            }
            lines.push(runs);
        }

        StyledSnapshot { cols: self.cols, rows: self.rows, lines }
    }

    fn is_alt_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    fn terminal_modes(&self) -> TerminalModes {
        let m = self.term.mode();
        TerminalModes {
            alt_screen: m.contains(TermMode::ALT_SCREEN),
            app_cursor_keys: m.contains(TermMode::APP_CURSOR),
            bracketed_paste: m.contains(TermMode::BRACKETED_PASTE),
            cursor_visible: m.contains(TermMode::SHOW_CURSOR),
            mouse_click: m.contains(TermMode::MOUSE_REPORT_CLICK),
            mouse_drag: m.contains(TermMode::MOUSE_DRAG),
            mouse_motion: m.contains(TermMode::MOUSE_MOTION),
            mouse_sgr: m.contains(TermMode::SGR_MOUSE),
        }
    }
}

/// True when a run carries no styling and only spaces.
fn is_blank_default(run: &StyledRun) -> bool {
    run.fg.is_none()
        && run.bg.is_none()
        && !run.bold
        && !run.dim
        && !run.italic
        && !run.underline
        && !run.inverse
        && run.text.chars().all(|c| c == ' ')
}

/// The visual style of a cell, pre-resolved to hex colors.
struct CellStyle {
    fg: Option<String>,
    bg: Option<String>,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
}

impl CellStyle {
    fn from_cell(fg: Color, bg: Color, flags: Flags) -> Self {
        Self {
            fg: color_to_hex(fg),
            bg: color_to_hex(bg),
            bold: flags.contains(Flags::BOLD),
            dim: flags.contains(Flags::DIM),
            italic: flags.contains(Flags::ITALIC),
            underline: flags.contains(Flags::UNDERLINE),
            inverse: flags.contains(Flags::INVERSE),
        }
    }

    fn matches(&self, run: &StyledRun) -> bool {
        self.fg == run.fg
            && self.bg == run.bg
            && self.bold == run.bold
            && self.dim == run.dim
            && self.italic == run.italic
            && self.underline == run.underline
            && self.inverse == run.inverse
    }

    fn into_run(self, first: char) -> StyledRun {
        StyledRun {
            text: first.to_string(),
            fg: self.fg,
            bg: self.bg,
            bold: self.bold,
            dim: self.dim,
            italic: self.italic,
            underline: self.underline,
            inverse: self.inverse,
        }
    }
}

/// Map a VT color to an `#rrggbb` string, or `None` for the terminal default.
fn color_to_hex(color: Color) -> Option<String> {
    match color {
        Color::Spec(rgb) => Some(format!("#{:02x}{:02x}{:02x}", rgb.r, rgb.g, rgb.b)),
        Color::Indexed(i) => Some(indexed_hex(i)),
        Color::Named(named) => named_to_index(named).map(indexed_hex),
    }
}

/// Map the 16 basic named colors to palette indices; default/cursor/dim variants
/// resolve to the terminal default (`None`).
fn named_to_index(named: NamedColor) -> Option<u8> {
    use NamedColor::*;
    Some(match named {
        Black => 0,
        Red => 1,
        Green => 2,
        Yellow => 3,
        Blue => 4,
        Magenta => 5,
        Cyan => 6,
        White => 7,
        BrightBlack => 8,
        BrightRed => 9,
        BrightGreen => 10,
        BrightYellow => 11,
        BrightBlue => 12,
        BrightMagenta => 13,
        BrightCyan => 14,
        BrightWhite => 15,
        _ => return None,
    })
}

/// Resolve an xterm 256-color palette index to `#rrggbb`.
fn indexed_hex(index: u8) -> String {
    const BASIC: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0x80, 0x00, 0x00),
        (0x00, 0x80, 0x00),
        (0x80, 0x80, 0x00),
        (0x00, 0x00, 0x80),
        (0x80, 0x00, 0x80),
        (0x00, 0x80, 0x80),
        (0xc0, 0xc0, 0xc0),
        (0x80, 0x80, 0x80),
        (0xff, 0x00, 0x00),
        (0x00, 0xff, 0x00),
        (0xff, 0xff, 0x00),
        (0x00, 0x00, 0xff),
        (0xff, 0x00, 0xff),
        (0x00, 0xff, 0xff),
        (0xff, 0xff, 0xff),
    ];
    let (r, g, b) = match index {
        0..=15 => BASIC[index as usize],
        16..=231 => {
            let i = index - 16;
            let levels = [0u8, 95, 135, 175, 215, 255];
            (levels[(i / 36) as usize], levels[((i / 6) % 6) as usize], levels[(i % 6) as usize])
        }
        232..=255 => {
            let v = 8 + 10 * (index - 232);
            (v, v, v)
        }
    };
    format!("#{r:02x}{g:02x}{b:02x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feeds_plain_text() {
        let mut screen = AlacrittyScreen::new(20, 5);
        screen.feed(b"hello world");
        let capture = screen.capture_plain();
        assert!(capture.starts_with("hello world"), "got: {capture:?}");
    }

    #[test]
    fn cursor_tracks_writes() {
        let mut screen = AlacrittyScreen::new(20, 5);
        screen.feed(b"abc");
        let cursor = screen.cursor();
        assert_eq!(cursor.row, 0);
        assert_eq!(cursor.col, 3);
    }

    #[test]
    fn newlines_advance_rows() {
        let mut screen = AlacrittyScreen::new(20, 5);
        screen.feed(b"line1\r\nline2");
        let capture = screen.capture_plain();
        let mut lines = capture.lines();
        assert_eq!(lines.next(), Some("line1"));
        assert_eq!(lines.next(), Some("line2"));
        assert_eq!(screen.cursor().row, 1);
    }

    #[test]
    fn scrollback_retains_history_after_screen_fills() {
        let mut screen = AlacrittyScreen::new(10, 3);
        for i in 0..10 {
            screen.feed(format!("row{i}\r\n").as_bytes());
        }
        // The visible screen only holds 3 rows, but scrollback keeps the history.
        let scrollback = screen.capture_scrollback();
        assert!(scrollback.contains("row0"), "scrollback missing early rows: {scrollback:?}");
        assert!(scrollback.contains("row9"), "scrollback missing recent rows: {scrollback:?}");
        // The visible capture should not contain the earliest row.
        let visible = screen.capture_plain();
        assert!(!visible.contains("row0"), "visible unexpectedly kept scrolled row: {visible:?}");
    }

    #[test]
    fn resize_changes_dimensions() {
        let mut screen = AlacrittyScreen::new(20, 5);
        assert_eq!(screen.size(), (20, 5));
        screen.resize(40, 10);
        assert_eq!(screen.size(), (40, 10));
    }

    #[test]
    fn styled_snapshot_captures_color_and_bold() {
        let mut screen = AlacrittyScreen::new(30, 3);
        // SGR: bold + red foreground, then "ERR", then reset.
        screen.feed(b"\x1b[1;31mERR\x1b[0m ok");
        let snap = screen.styled_snapshot();
        assert_eq!(snap.rows, 3);
        let first_line = &snap.lines[0];
        let err_run = first_line.iter().find(|r| r.text.contains("ERR")).expect("ERR run present");
        assert!(err_run.bold, "ERR run should be bold");
        // SGR 31 is the standard (non-bright) ANSI red, palette index 1.
        assert_eq!(err_run.fg.as_deref(), Some("#800000"));
    }

    #[test]
    fn indexed_palette_cube_and_grayscale() {
        assert_eq!(indexed_hex(1), "#800000"); // basic red
        assert_eq!(indexed_hex(196), "#ff0000"); // cube bright red
        assert_eq!(indexed_hex(231), "#ffffff"); // cube white
        assert_eq!(indexed_hex(232), "#080808"); // grayscale start
    }

    #[test]
    fn detects_alt_screen_and_modes() {
        let mut screen = AlacrittyScreen::new(20, 5);
        assert!(!screen.is_alt_screen(), "starts on the primary screen");
        screen.feed(b"\x1b[?1049h\x1b[?1h\x1b[?1002h\x1b[?1006h");
        assert!(screen.is_alt_screen());
        let m = screen.terminal_modes();
        assert!(m.alt_screen && m.app_cursor_keys && m.mouse_drag && m.mouse_sgr);
    }

    /// The Phase 2 reattach regression: a full-screen TUI's chrome, painted once and
    /// long gone from the raw ring window, is reproduced from the VT snapshot alone,
    /// with alt-screen and DECCKM modes restored so scrolling and arrows keep working.
    #[test]
    fn alt_screen_repaint_round_trips_full_screen_and_modes() {
        let mut original = AlacrittyScreen::new(20, 5);
        // Enter alt-screen and set application cursor keys + mouse reporting, then paint
        // chrome across the whole screen (as a TUI does on entry).
        original.feed(b"\x1b[?1049h\x1b[?1h\x1b[?1002h\x1b[?1006h");
        original.feed(b"\x1b[1;1HHEADER");
        original.feed(b"\x1b[2;1Hbody line one");
        original.feed(b"\x1b[5;1H> input prompt");

        let snapshot = original.styled_snapshot();
        let cursor = original.cursor();
        let modes = original.terminal_modes();
        let repaint = crate::screen::repaint_ansi(&snapshot, &cursor, &modes);
        let ansi = String::from_utf8_lossy(&repaint);
        assert!(ansi.contains("\x1b[?1049h"), "alt-screen enable must be in the replay");
        assert!(ansi.contains("\x1b[?1h"), "DECCKM must be in the replay");

        // A fresh terminal fed ONLY the repaint (no ring at all) reproduces the full
        // screen and the modes - the exact failure the raw-ring replay had.
        let mut fresh = AlacrittyScreen::new(20, 5);
        fresh.feed(&repaint);
        assert_eq!(fresh.capture_plain(), original.capture_plain(), "full screen reproduced");
        assert!(fresh.is_alt_screen(), "alt-screen mode restored");
        assert!(fresh.terminal_modes().app_cursor_keys, "DECCKM restored");
    }
}
