//! A stub agent TUI for the verified-submit test matrix (`adapters.md` / Verified
//! submit). It renders a composer with a `> ` prompt and, per `DFLOW_STUB_MODE`,
//! reproduces one silent-failure hazard so the verified-submit algorithm can be tested
//! against a real PTY without a real agent CLI:
//!
//! - `normal`: Enter submits immediately.
//! - `popup_swallow`: the first Enter on a `/`-prefixed line is swallowed (closes the popup); the next Enter submits.
//! - `placeholder`: the first Enter expands an argument-hint placeholder into the composer instead of submitting; the next submits.
//! - `ghost`: dim ghost text is drawn beside the composer; Enter submits (the reader must not treat ghost text as typed input).
//! - `slow`: the redraw after Enter lags, so a naive re-read sees stale text.
//! - `never`: Enter never submits (drives the failure path).
//!
//! An Enter on an empty composer is always ignored, so verified-submit retries are safe.
//!
//! When `DFLOW_STUB_CAPTURE` names a file, every input byte the stub receives (printable
//! characters and newlines, excluding the daemon's DSR escape replies) is mirrored to that
//! file. A dispatch/first-prompt brief delivered by TYPED injection thus lands in the file
//! in full - so a test can prove the WHOLE multi-line brief reached the agent's input and
//! was not truncated at the first newline (the `cmd.exe /c` shim-launch bug this proves
//! fixed). The composer/submit behavior is unchanged, so verified submit still works.

use std::io::{Read, Write};
use std::time::Duration;

fn main() {
    let mode = std::env::var("DFLOW_STUB_MODE").unwrap_or_else(|_| "normal".to_string());
    let capture_path = std::env::var("DFLOW_STUB_CAPTURE").ok();
    let mut received: Vec<u8> = Vec::new();
    let mut composer = String::new();
    let mut transcript: Vec<String> = Vec::new();
    let mut popup_dismissed = false;
    let mut placeholder_expanded = false;
    // ConPTY translates a carriage return to CRLF, so coalesce a `\n` that immediately
    // follows a `\r` into one Enter (otherwise one keystroke fires two Enter events).
    let mut last_was_cr = false;

    let mut out = std::io::stdout();
    render(&mut out, &transcript, &composer, &mode);

    let stdin = std::io::stdin();
    let mut lock = stdin.lock();
    let mut byte = [0u8; 1];
    loop {
        match lock.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        let b = byte[0];
        let is_enter = match b {
            b'\r' => true,
            b'\n' => !last_was_cr, // the LF half of a CRLF is not a second Enter
            _ => false,
        };
        last_was_cr = b == b'\r';

        if b == 0x1b {
            // Consume and ignore a CSI escape sequence (e.g. the DSR reply the daemon
            // feeds back), reading until the final byte in 0x40..=0x7e. Escape bytes are
            // daemon noise, not brief content, so they are never mirrored to the capture.
            let mut e = [0u8; 1];
            if lock.read(&mut e).unwrap_or(0) == 0 {
                break;
            }
            if e[0] == b'[' {
                loop {
                    if lock.read(&mut e).unwrap_or(0) == 0 {
                        return;
                    }
                    if (0x40..=0x7e).contains(&e[0]) {
                        break;
                    }
                }
            }
            continue;
        }

        // Mirror every real input byte (printable + newlines) so a test can read back the
        // full brief that reached the composer, regardless of how Enter is interpreted.
        if is_enter || b == b'\n' || (0x20..0x7f).contains(&b) {
            received.push(b);
            capture(&capture_path, &received);
        }

        if is_enter {
            if composer.is_empty() {
                continue; // ignore empty submits so verified-submit retries are safe
            }
            let submit =
                handle_enter(&mode, &mut composer, &mut popup_dismissed, &mut placeholder_expanded);
            if submit {
                transcript.push(format!("SUBMITTED: {composer}"));
                composer.clear();
                popup_dismissed = false;
                placeholder_expanded = false;
            }
            if mode == "slow" {
                std::thread::sleep(Duration::from_millis(450)); // redraw lags
            }
            render(&mut out, &transcript, &composer, &mode);
        } else if b == 0x08 || b == 0x7f {
            composer.pop();
            render(&mut out, &transcript, &composer, &mode);
        } else if (0x20..0x7f).contains(&b) {
            composer.push(b as char);
            render(&mut out, &transcript, &composer, &mode);
        }
    }
}

/// Mirror the received input bytes to the capture file, if one is configured. Overwrites
/// with the full accumulator each time so the file always holds everything received so far.
fn capture(path: &Option<String>, received: &[u8]) {
    if let Some(p) = path {
        let _ = std::fs::write(p, received);
    }
}

/// Decide whether this Enter submits, applying the mode's hazard. Returns true to
/// submit (the caller clears the composer).
fn handle_enter(
    mode: &str,
    composer: &mut String,
    popup_dismissed: &mut bool,
    placeholder_expanded: &mut bool,
) -> bool {
    match mode {
        "popup_swallow" => {
            if composer.starts_with('/') && !*popup_dismissed {
                *popup_dismissed = true; // swallow the first Enter (closes the popup)
                false
            } else {
                true
            }
        }
        "placeholder" => {
            if !*placeholder_expanded {
                composer.push_str(" <arg>"); // expand a hint instead of submitting
                *placeholder_expanded = true;
                false
            } else {
                true
            }
        }
        "never" => false,
        // normal, ghost, slow all submit on a non-empty Enter.
        _ => true,
    }
}

/// Repaint the transcript and the composer line, leaving the cursor on the composer
/// line (so the daemon reads the composer at the cursor row).
fn render(out: &mut std::io::Stdout, transcript: &[String], composer: &str, mode: &str) {
    let mut buf = String::new();
    buf.push_str("\x1b[2J\x1b[H");
    for line in transcript {
        buf.push_str(line);
        buf.push_str("\r\n");
    }
    buf.push_str("> ");
    buf.push_str(composer);
    if mode == "ghost" {
        // Dim ghost text drawn beside the composer (SGR 2 = dim), reset after.
        buf.push_str("\x1b[2m  suggestion...\x1b[0m");
        // Return the cursor to just after the typed text so classification reads the
        // composer, not the ghost.
        buf.push_str(&format!("\x1b[{}G", 3 + composer.chars().count()));
    }
    let _ = out.write_all(buf.as_bytes());
    let _ = out.flush();
}
