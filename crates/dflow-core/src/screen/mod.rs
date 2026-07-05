//! The VT screen-model boundary.
//!
//! `ScreenModel` is the DapperFlow-owned trait that hides the concrete VT crate
//! (`architecture.md` / internalized multiplexer). Nothing outside the
//! `screen::alacritty_impl` module may import `alacritty_terminal` types, so the
//! crate stays swappable after M0. The native APIs here replace the tmux verbs:
//! feed bytes, styled/plain capture, cursor query, resize, scrollback access.

mod alacritty_impl;
mod repaint;

pub use alacritty_impl::AlacrittyScreen;
pub use repaint::{repaint_ansi, TerminalModes};

use dflow_proto::{CursorPos, StyledSnapshot};

/// A queryable VT screen fed raw PTY output bytes.
///
/// Implementations maintain the terminal grid plus a bounded scrollback history
/// (the visible screen and the lines that have scrolled off the top).
pub trait ScreenModel: Send {
    /// Feed a chunk of raw PTY output through the VT parser.
    fn feed(&mut self, bytes: &[u8]);

    /// Drain any bytes the terminal wants written back to the PTY.
    ///
    /// Terminals answer queries embedded in the output stream: cursor-position
    /// reports (DSR, `ESC[6n`), device attributes (DA), and similar. ConPTY emits
    /// `ESC[6n` during startup and stalls the shell until it receives the reply, so
    /// the session loop must feed these responses back into the PTY. The default is
    /// empty for models that never generate responses.
    fn take_responses(&mut self) -> Vec<u8> {
        Vec::new()
    }

    /// Resize the terminal to `cols` x `rows`.
    fn resize(&mut self, cols: u16, rows: u16);

    /// Current terminal dimensions as `(cols, rows)`.
    fn size(&self) -> (u16, u16);

    /// The cursor's position and visibility within the visible screen.
    fn cursor(&self) -> CursorPos;

    /// The visible screen as plain text, one line per row, trailing blanks trimmed.
    fn capture_plain(&self) -> String;

    /// The full scrollback history plus the visible screen as plain text.
    fn capture_scrollback(&self) -> String;

    /// A structured, styled snapshot of the visible screen for replay and future
    /// non-xterm clients.
    fn styled_snapshot(&self) -> StyledSnapshot;

    /// Whether the terminal is currently on the alternate screen (a full-screen TUI
    /// like opencode or claude). Attach replay must repaint from the snapshot for an
    /// alt-screen session, since its chrome was painted once and is no longer in the
    /// raw ring window (`architecture.md`; Phase 2 reattach fix). Default: primary.
    fn is_alt_screen(&self) -> bool {
        false
    }

    /// The terminal modes the reattach replay must restore so scrolling, arrow keys,
    /// and mouse reporting keep working after a fresh connection (alt-screen,
    /// application cursor keys, bracketed paste, mouse reporting, cursor visibility).
    fn terminal_modes(&self) -> TerminalModes {
        TerminalModes::default()
    }
}
