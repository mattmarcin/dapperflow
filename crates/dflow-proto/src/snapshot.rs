//! Styled screen-snapshot wire types.
//!
//! Produced by the daemon's `ScreenModel` (see `dflow-core`) and returned in the
//! `session.attach` response. Colors are pre-resolved to `#rrggbb` hex strings so
//! this wire type stays independent of any particular VT crate; the mapping from
//! the VT crate's color enum lives inside the `ScreenModel` implementation, keeping
//! the crate swappable (`architecture.md` / internalized multiplexer).

use serde::{Deserialize, Serialize};

/// Cursor position and visibility, in 0-based cells within the visible screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorPos {
    pub col: u16,
    pub row: u16,
    pub visible: bool,
}

/// A run of contiguous cells on one line that share the same styling.
///
/// Runs (rather than per-cell records) keep the snapshot compact for wide screens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyledRun {
    pub text: String,
    /// Foreground color as `#rrggbb`, or `None` for the terminal default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg: Option<String>,
    /// Background color as `#rrggbb`, or `None` for the terminal default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    /// Dim/faint (SGR 2). Marks ghost/placeholder text for verified submit stripping
    /// (`adapters.md` / composer, ghost_text_styles).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dim: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub underline: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub inverse: bool,
}

/// A structured styled snapshot of the visible screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyledSnapshot {
    pub cols: u16,
    pub rows: u16,
    /// One entry per visible row, top to bottom; each is a list of styled runs.
    pub lines: Vec<Vec<StyledRun>>,
}
