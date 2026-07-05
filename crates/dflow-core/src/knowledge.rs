//! The project knowledgebase engine (`knowledge.md`).
//!
//! A per-project directory of plain markdown notes with permissive YAML frontmatter,
//! designed so "point Obsidian at it" stays true: no sidecar databases, no required
//! plugins, and the only file the daemon ever regenerates is the Catalog section of
//! `index.md`. This module owns the on-disk contract; the SQLite `knowledge_notes`
//! table (`store::knowledge`) is a fast index rebuilt from these files, never a second
//! source of truth.
//!
//! Reader posture is deliberately permissive (`knowledge.md` / File format): unknown
//! `type`s, unknown keys, missing fields, and broken links never reject a note. A note
//! with no frontmatter at all is still a note (`type` defaults to `note`).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Default location relative to the project root (`knowledge.md` / Where it lives).
pub const DEFAULT_SUBDIR: &str = "docs/knowledge";

/// Digest hard cap when injecting into briefs (`knowledge.md` / index.md).
pub const DIGEST_LINE_CAP: usize = 30;

/// One note as seen on disk, extracted for the SQLite index and `know find`.
#[derive(Debug, Clone)]
pub struct IndexedNote {
    /// Identity: path relative to the knowledge root minus `.md` (e.g.
    /// `decisions/soft-delete-accounts`).
    pub id: String,
    /// Path relative to the knowledge root, with extension (e.g. `decisions/x.md`).
    pub path: String,
    pub note_type: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    /// Provenance card id from frontmatter (`card:`), when present.
    pub source_card: Option<String>,
    /// File mtime in epoch milliseconds (best-effort; 0 when unavailable).
    pub updated_at: i64,
}

/// A parsed note: its extracted frontmatter fields plus the body below frontmatter.
#[derive(Debug, Clone, Default)]
pub struct ParsedNote {
    pub note_type: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub source_card: Option<String>,
    pub body: String,
}

/// Resolve the knowledge root for a project: `knowledge_path` override (absolute, or
/// relative to the project root) else `<project>/docs/knowledge` (`knowledge.md`).
pub fn resolve_dir(project_root: &Path, knowledge_path: Option<&str>) -> PathBuf {
    match knowledge_path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => {
            let candidate = PathBuf::from(p);
            if candidate.is_absolute() {
                candidate
            } else {
                project_root.join(candidate)
            }
        }
        None => project_root.join(DEFAULT_SUBDIR),
    }
}

/// The conventional subdirectory for a note type (`knowledge.md` / Directory layout).
/// Unknown types get their own directory named after the type, so they are still
/// listed under their own heading in the catalog.
pub fn type_dir(note_type: &str) -> String {
    match note_type {
        "decision" => "decisions".into(),
        "convention" => "conventions".into(),
        "gotcha" => "gotchas".into(),
        "runbook" => "runbooks".into(),
        "reference" => "reference".into(),
        "note" => "notes".into(),
        other => {
            let slug = kebab_case(other);
            if slug.is_empty() {
                "notes".into()
            } else {
                slug
            }
        }
    }
}

/// A filesystem-safe kebab-case slug of `text` (lowercase alphanumerics, single dashes).
pub fn kebab_case(text: &str) -> String {
    let mut slug = String::with_capacity(text.len());
    let mut last_dash = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    slug.trim_matches('-').to_string()
}

/// The note identity (id) for a `type` + `title`: `<type-dir>/<kebab-title>`.
pub fn note_id(note_type: &str, title: &str) -> String {
    let slug = kebab_case(title);
    let slug = if slug.is_empty() { "untitled".to_string() } else { slug };
    format!("{}/{}", type_dir(note_type), slug)
}

/// Result of writing a note via `know add`.
#[derive(Debug, Clone)]
pub struct WriteOutcome {
    pub id: String,
    /// Path relative to the knowledge root, with extension.
    pub rel_path: String,
    /// True on first create, false when an existing id was replaced.
    pub created: bool,
}

/// Write (create or replace) a note under `dir`, returning its id and relative path.
///
/// The directory tree is created lazily (first note wins; no empty scaffolding). The
/// frontmatter carries the recommended fields; `card` provenance is stamped when the
/// caller passes one. The body is written verbatim (no canonical-formatting pass, per
/// the Obsidian compatibility contract).
pub fn write_note(
    dir: &Path,
    note_type: &str,
    title: &str,
    body: &str,
    tags: &[String],
    card: Option<&str>,
) -> std::io::Result<WriteOutcome> {
    let id = note_id(note_type, title);
    let rel_path = format!("{id}.md");
    let full = dir.join(&rel_path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let created = !full.exists();
    let description = first_meaningful_line(body);
    let contents = render_note(note_type, title, description.as_deref(), tags, card, body);
    std::fs::write(&full, contents)?;
    Ok(WriteOutcome { id, rel_path, created })
}

/// Render a note file: YAML frontmatter (recommended fields only) then the body.
fn render_note(
    note_type: &str,
    title: &str,
    description: Option<&str>,
    tags: &[String],
    card: Option<&str>,
    body: &str,
) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("type: {}\n", yaml_scalar(note_type)));
    out.push_str(&format!("title: {}\n", yaml_scalar(title)));
    if let Some(d) = description.filter(|d| !d.is_empty()) {
        out.push_str(&format!("description: {}\n", yaml_scalar(d)));
    }
    if !tags.is_empty() {
        let joined = tags.iter().map(|t| yaml_scalar(t)).collect::<Vec<_>>().join(", ");
        out.push_str(&format!("tags: [{joined}]\n"));
    }
    if let Some(c) = card.filter(|c| !c.is_empty()) {
        out.push_str(&format!("card: {}\n", yaml_scalar(c)));
    }
    out.push_str(&format!("updated: {}\n", today_iso()));
    out.push_str("---\n\n");
    out.push_str(body.trim_end());
    out.push('\n');
    out
}

/// Quote a YAML scalar only when needed (contains a character that would confuse the
/// minimal reader); otherwise emit it bare so Obsidian renders a clean property.
fn yaml_scalar(value: &str) -> String {
    let needs_quote = value.is_empty()
        || value.starts_with(|c: char| c.is_whitespace())
        || value.ends_with(|c: char| c.is_whitespace())
        || value.contains([':', '#', '[', ']', '{', '}', ',', '"', '\'', '\n']);
    if needs_quote {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

/// Parse a note's frontmatter and body (`knowledge.md` / File format). Permissive: a
/// missing/partial/absent frontmatter block yields defaults (`type` -> `note`), never
/// an error.
pub fn parse_note(text: &str) -> ParsedNote {
    let mut note = ParsedNote { note_type: "note".into(), ..Default::default() };
    let normalized = text.strip_prefix('\u{feff}').unwrap_or(text);
    if let Some(rest) = normalized.strip_prefix("---\n").or_else(|| normalized.strip_prefix("---\r\n")) {
        // Find the closing fence on its own line.
        if let Some((front, body)) = split_frontmatter(rest) {
            parse_frontmatter_into(&mut note, front);
            note.body = body.trim_start_matches(['\n', '\r']).to_string();
            return note;
        }
    }
    note.body = normalized.to_string();
    note
}

/// Split `rest` (text after the opening `---`) into the frontmatter block and body at
/// the first closing `---` line.
fn split_frontmatter(rest: &str) -> Option<(&str, &str)> {
    let mut idx = 0;
    for line in rest.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed == "---" || trimmed == "..." {
            let front = &rest[..idx];
            let body = &rest[idx + line.len()..];
            return Some((front, body));
        }
        idx += line.len();
    }
    None
}

/// Extract the fields the tooling caches from a frontmatter block. Only these keys are
/// read; every other key is preserved on disk untouched (never rewritten in place).
fn parse_frontmatter_into(note: &mut ParsedNote, front: &str) {
    let mut lines = front.lines().peekable();
    while let Some(line) = lines.next() {
        let Some((key, value)) = line.split_once(':') else { continue };
        let key = key.trim();
        let value = value.trim();
        match key {
            "type" => {
                let v = unquote(value);
                if !v.is_empty() {
                    note.note_type = v;
                }
            }
            "title" => note.title = non_empty(unquote(value)),
            "description" => note.description = non_empty(unquote(value)),
            "card" => note.source_card = non_empty(unquote(value)),
            "tags" => {
                if value.is_empty() {
                    // Block list form: collect following `- item` lines.
                    while let Some(peek) = lines.peek() {
                        let t = peek.trim();
                        if let Some(item) = t.strip_prefix("- ") {
                            note.tags.push(unquote(item.trim()));
                            lines.next();
                        } else if t.starts_with('-') && t.len() == 1 {
                            lines.next();
                        } else {
                            break;
                        }
                    }
                } else {
                    note.tags = parse_inline_tags(value);
                }
            }
            _ => {}
        }
    }
    note.tags.retain(|t| !t.is_empty());
}

/// Parse the inline `[a, b, c]` (or bare `a, b`) tag list form Obsidian understands.
fn parse_inline_tags(value: &str) -> Vec<String> {
    let inner = value.trim().trim_start_matches('[').trim_end_matches(']');
    inner
        .split(',')
        .map(|t| unquote(t.trim()))
        .filter(|t| !t.is_empty())
        .collect()
}

fn unquote(value: &str) -> String {
    let v = value.trim();
    if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
        || (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2)
    {
        v[1..v.len() - 1].replace("\\\"", "\"").replace("\\\\", "\\")
    } else {
        v.to_string()
    }
}

fn non_empty(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Scan the knowledge directory for notes (`*.md` except the root `index.md`), parsing
/// each into an `IndexedNote`. External edits are picked up here on read; the daemon
/// holds no lock and no watcher (`knowledge.md` / Obsidian compatibility).
pub fn scan(dir: &Path) -> Vec<IndexedNote> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return out;
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = match std::fs::read_dir(&current) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                // Skip Obsidian vault config; it is left alone (`knowledge.md`).
                if path.file_name().and_then(|n| n.to_str()) == Some(".obsidian") {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let rel = match path.strip_prefix(dir) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if rel_str == "index.md" {
                continue; // the index is generated, not a note
            }
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let parsed = parse_note(&text);
            let id = rel_str.strip_suffix(".md").unwrap_or(&rel_str).to_string();
            let description = parsed
                .description
                .clone()
                .or_else(|| first_meaningful_line(&parsed.body));
            out.push(IndexedNote {
                id,
                path: rel_str,
                note_type: parsed.note_type,
                title: parsed.title,
                description,
                tags: parsed.tags,
                source_card: parsed.source_card,
                updated_at: file_mtime_ms(&path),
            });
        }
    }
    // Deterministic order: by id, so the index and catalog are stable.
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Read one note by id (path minus `.md`), returning its parsed form, or `None`.
pub fn read_note(dir: &Path, id: &str) -> Option<ParsedNote> {
    let rel = format!("{id}.md");
    let full = dir.join(&rel);
    let text = std::fs::read_to_string(full).ok()?;
    Some(parse_note(&text))
}

/// Extract the Digest section of `index.md` (the lines under `## Digest` up to the
/// next `## ` heading), capped at [`DIGEST_LINE_CAP`] lines. `None` when there is no
/// index or no Digest section; the digest is never authored by the daemon.
pub fn read_digest(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("index.md")).ok()?;
    let mut in_digest = false;
    let mut lines: Vec<&str> = Vec::new();
    for line in text.lines() {
        let is_heading = line.trim_start().starts_with("## ");
        if is_heading {
            if in_digest {
                break; // reached the next section (e.g. Catalog)
            }
            if line.trim_start().trim_start_matches('#').trim().eq_ignore_ascii_case("digest") {
                in_digest = true;
            }
            continue;
        }
        if in_digest {
            lines.push(line);
        }
    }
    if !in_digest {
        return None;
    }
    // Trim leading/trailing blank lines, then cap.
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        return None;
    }
    lines.truncate(DIGEST_LINE_CAP);
    Some(lines.join("\n"))
}

/// Regenerate the Catalog section of `index.md` deterministically from `notes`,
/// preserving the title and the human-owned Digest section (`knowledge.md` /
/// index.md). Idempotent: the same notes always yield byte-identical output, so a
/// Catalog merge conflict resolves by regenerating.
pub fn regenerate_catalog(dir: &Path, project_name: &str, notes: &[IndexedNote]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let index_path = dir.join("index.md");
    let existing = std::fs::read_to_string(&index_path).unwrap_or_default();
    let title = extract_title(&existing)
        .unwrap_or_else(|| format!("# Project knowledge - {project_name}"));
    let digest_block = extract_section(&existing, "Digest");

    let mut sorted = notes.to_vec();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));

    let mut out = String::new();
    out.push_str(title.trim_end());
    out.push_str("\n\n");
    if let Some(digest) = digest_block {
        out.push_str("## Digest\n");
        out.push_str(digest.trim_end());
        out.push_str("\n\n");
    }
    out.push_str(&render_catalog(&sorted));
    std::fs::write(&index_path, out)?;
    Ok(())
}

/// Build the Catalog section body: one line per type, grouped, deterministic.
fn render_catalog(notes: &[IndexedNote]) -> String {
    let mut out = String::from("## Catalog\n");
    if notes.is_empty() {
        out.push_str("- (no notes yet)\n");
        return out;
    }
    // Group by type, preserving a stable type order (first-seen after id sort).
    let mut type_order: Vec<String> = Vec::new();
    for n in notes {
        if !type_order.contains(&n.note_type) {
            type_order.push(n.note_type.clone());
        }
    }
    type_order.sort();
    for note_type in &type_order {
        let group: Vec<&IndexedNote> = notes.iter().filter(|n| &n.note_type == note_type).collect();
        let items: Vec<String> = group
            .iter()
            .map(|n| {
                let desc = n.description.as_deref().unwrap_or("").trim();
                if desc.is_empty() {
                    format!("[[{}]]", n.id)
                } else {
                    format!("[[{}]] - {}", n.id, desc)
                }
            })
            .collect();
        out.push_str(&format!("- {} ({}): {}\n", type_dir(note_type), group.len(), items.join("; ")));
    }
    out
}

/// The first `# ` title line of an existing index, if any.
fn extract_title(text: &str) -> Option<String> {
    text.lines()
        .find(|l| l.trim_start().starts_with("# ") && !l.trim_start().starts_with("## "))
        .map(|l| l.trim_end().to_string())
}

/// Extract the raw body of a `## <name>` section (up to the next `## ` heading).
fn extract_section(text: &str, name: &str) -> Option<String> {
    let mut in_section = false;
    let mut body: Vec<&str> = Vec::new();
    for line in text.lines() {
        let is_heading = line.trim_start().starts_with("## ");
        if is_heading {
            if in_section {
                break;
            }
            if line.trim_start().trim_start_matches('#').trim().eq_ignore_ascii_case(name) {
                in_section = true;
            }
            continue;
        }
        if in_section {
            body.push(line);
        }
    }
    if !in_section {
        return None;
    }
    let joined = body.join("\n");
    let trimmed = joined.trim_matches('\n');
    if trimmed.trim().is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// The first non-blank, non-heading line of a body, as a one-line description.
fn first_meaningful_line(body: &str) -> Option<String> {
    body.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("---"))
        .map(|l| {
            let l = l.trim_start_matches(['-', '*', '>', ' ']).trim();
            let capped: String = l.chars().take(140).collect();
            capped
        })
        .filter(|l| !l.is_empty())
}

fn file_mtime_ms(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Today's date as `YYYY-MM-DD` (UTC), for the `updated` frontmatter field. A tiny
/// civil-date computation avoids a chrono dependency.
fn today_iso() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Convert a count of days since the Unix epoch to a `(year, month, day)` civil date
/// (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests;
