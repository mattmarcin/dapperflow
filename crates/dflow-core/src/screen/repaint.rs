//! Snapshot-to-ANSI repaint serialization for attach replay (Phase 2 reattach fix).
//!
//! The bug this fixes: attach replay used the raw scrollback ring, but a full-screen
//! TUI painted its chrome (header, input box, footer) once, long before the ring
//! window, so replaying recent bytes reconstructed only incremental updates and the
//! app never repainted chrome it believed was still displayed. Worse, terminal MODES
//! (alt-screen, application cursor keys, mouse reporting, bracketed paste) were lost,
//! so scrolling and arrow keys broke after a reconnect.
//!
//! The fix reconstructs the CURRENT screen from the VT model: a mode-restoration
//! preamble, then an absolute-positioned repaint of every styled cell, then the cursor
//! restored. The daemon prepends ring scrollback history only on the primary screen;
//! on the alternate screen the snapshot repaint stands alone (alt screen has no
//! scrollback by design).

use dflow_proto::{CursorPos, StyledRun, StyledSnapshot};

/// Terminal modes the reattach replay restores (`architecture.md`; Phase 2). Each maps
/// to a DEC private mode set the client (xterm.js) needs to behave like the live TUI.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalModes {
    /// Alternate screen buffer (`?1049`). Without it xterm never enables
    /// wheel-to-arrow translation, which kills in-TUI scrolling.
    pub alt_screen: bool,
    /// Application cursor keys / DECCKM (`?1`). Arrow keys misbehave in TUIs without it.
    pub app_cursor_keys: bool,
    /// Bracketed paste (`?2004`).
    pub bracketed_paste: bool,
    /// Cursor visibility / DECTCEM (`?25`).
    pub cursor_visible: bool,
    /// Mouse click reporting (`?1000`).
    pub mouse_click: bool,
    /// Mouse button-event/drag reporting (`?1002`).
    pub mouse_drag: bool,
    /// Mouse any-motion reporting (`?1003`).
    pub mouse_motion: bool,
    /// SGR extended mouse encoding (`?1006`).
    pub mouse_sgr: bool,
}

impl TerminalModes {
    /// Sensible primary-screen defaults for a fresh terminal (cursor visible).
    pub fn primary() -> Self {
        Self { cursor_visible: true, ..Self::default() }
    }
}

/// Serialize the current screen plus active modes to an ANSI byte stream that
/// reconstructs the display on a fresh terminal.
///
/// Order matters: modes first (so the client is in alt-screen/mouse/DECCKM state
/// before the paint), then a soft reset of SGR, then an absolute-positioned repaint of
/// each row (each cleared to end-of-line first so leftover glyphs never linger), then
/// the cursor position and visibility restored last.
pub fn repaint_ansi(snapshot: &StyledSnapshot, cursor: &CursorPos, modes: &TerminalModes) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(snapshot.cols as usize * snapshot.rows as usize + 64);

    // 1. Mode-restoration preamble.
    if modes.alt_screen {
        out.extend_from_slice(b"\x1b[?1049h");
    }
    if modes.app_cursor_keys {
        out.extend_from_slice(b"\x1b[?1h");
    }
    if modes.bracketed_paste {
        out.extend_from_slice(b"\x1b[?2004h");
    }
    if modes.mouse_click {
        out.extend_from_slice(b"\x1b[?1000h");
    }
    if modes.mouse_drag {
        out.extend_from_slice(b"\x1b[?1002h");
    }
    if modes.mouse_motion {
        out.extend_from_slice(b"\x1b[?1003h");
    }
    if modes.mouse_sgr {
        out.extend_from_slice(b"\x1b[?1006h");
    }

    // 2. Reset SGR, clear the visible screen, home the cursor.
    out.extend_from_slice(b"\x1b[0m\x1b[2J\x1b[H");

    // 3. Paint each row with absolute positioning, clearing each line first.
    for (idx, line) in snapshot.lines.iter().enumerate() {
        let row = idx + 1; // ANSI rows are 1-based.
        out.extend_from_slice(format!("\x1b[{row};1H\x1b[2K").as_bytes());
        for run in line {
            let sgr = sgr_for(run);
            if !sgr.is_empty() {
                out.extend_from_slice(sgr.as_bytes());
            }
            out.extend_from_slice(run.text.as_bytes());
            if !sgr.is_empty() {
                out.extend_from_slice(b"\x1b[0m");
            }
        }
    }

    // 4. Restore the cursor position and visibility last.
    let crow = cursor.row as usize + 1;
    let ccol = cursor.col as usize + 1;
    out.extend_from_slice(format!("\x1b[{crow};{ccol}H").as_bytes());
    if modes.cursor_visible {
        out.extend_from_slice(b"\x1b[?25h");
    } else {
        out.extend_from_slice(b"\x1b[?25l");
    }

    out
}

/// Build the SGR set sequence for a styled run, or an empty string for default style.
fn sgr_for(run: &StyledRun) -> String {
    let mut params: Vec<String> = Vec::new();
    if run.bold {
        params.push("1".to_string());
    }
    if run.dim {
        params.push("2".to_string());
    }
    if run.italic {
        params.push("3".to_string());
    }
    if run.underline {
        params.push("4".to_string());
    }
    if run.inverse {
        params.push("7".to_string());
    }
    if let Some(rgb) = run.fg.as_deref().and_then(parse_hex) {
        params.push(format!("38;2;{};{};{}", rgb.0, rgb.1, rgb.2));
    }
    if let Some(rgb) = run.bg.as_deref().and_then(parse_hex) {
        params.push(format!("48;2;{};{};{}", rgb.0, rgb.1, rgb.2));
    }
    if params.is_empty() {
        String::new()
    } else {
        format!("\x1b[{}m", params.join(";"))
    }
}

/// Parse `#rrggbb` into an `(r, g, b)` triple.
fn parse_hex(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some((r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> StyledRun {
        StyledRun {
            text: text.to_string(),
            fg: None,
            bg: None,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            inverse: false,
        }
    }

    #[test]
    fn alt_screen_preamble_includes_modes() {
        let snap = StyledSnapshot { cols: 10, rows: 1, lines: vec![vec![run("hi")]] };
        let cursor = CursorPos { col: 2, row: 0, visible: true };
        let modes = TerminalModes {
            alt_screen: true,
            app_cursor_keys: true,
            bracketed_paste: true,
            cursor_visible: true,
            mouse_drag: true,
            mouse_sgr: true,
            ..Default::default()
        };
        let ansi = String::from_utf8(repaint_ansi(&snap, &cursor, &modes)).unwrap();
        assert!(ansi.contains("\x1b[?1049h"), "alt-screen enable missing");
        assert!(ansi.contains("\x1b[?1h"), "DECCKM missing");
        assert!(ansi.contains("\x1b[?2004h"), "bracketed paste missing");
        assert!(ansi.contains("\x1b[?1002h"), "mouse drag missing");
        assert!(ansi.contains("\x1b[?1006h"), "SGR mouse missing");
        assert!(ansi.contains("hi"));
        assert!(ansi.contains("\x1b[?25h"), "cursor show missing");
    }

    #[test]
    fn primary_screen_has_no_alt_enable() {
        let snap = StyledSnapshot { cols: 10, rows: 1, lines: vec![vec![run("x")]] };
        let cursor = CursorPos { col: 1, row: 0, visible: true };
        let ansi = String::from_utf8(repaint_ansi(&snap, &cursor, &TerminalModes::primary())).unwrap();
        assert!(!ansi.contains("1049h"), "primary screen must not enable alt-screen");
    }

    #[test]
    fn styled_run_becomes_truecolor_sgr() {
        let mut r = run("ERR");
        r.bold = true;
        r.fg = Some("#ff0000".to_string());
        let sgr = sgr_for(&r);
        assert!(sgr.contains("1"), "bold");
        assert!(sgr.contains("38;2;255;0;0"), "truecolor red fg");
    }
}
