//! Unit tests for the knowledge engine: parsing, ids, writing, scanning, digest, and
//! the deterministic catalog regeneration (`knowledge.md`).

use super::*;

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("dflow-know-{tag}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn resolve_dir_default_and_override() {
    let root = Path::new("C:/proj");
    assert_eq!(resolve_dir(root, None), root.join("docs/knowledge"));
    assert_eq!(resolve_dir(root, Some(".dapperflow/knowledge")), root.join(".dapperflow/knowledge"));
    // Absolute override wins verbatim.
    assert_eq!(resolve_dir(root, Some("D:/notes")), PathBuf::from("D:/notes"));
    // Blank override falls back to default.
    assert_eq!(resolve_dir(root, Some("   ")), root.join("docs/knowledge"));
}

#[test]
fn note_id_maps_type_to_dir_and_kebabs_title() {
    assert_eq!(note_id("decision", "Soft-delete for accounts"), "decisions/soft-delete-for-accounts");
    assert_eq!(note_id("gotcha", "ConPTY resize storm!"), "gotchas/conpty-resize-storm");
    assert_eq!(note_id("note", "Plain thing"), "notes/plain-thing");
    // Unknown type gets its own directory.
    assert_eq!(note_id("insight", "Neat idea"), "insight/neat-idea");
}

#[test]
fn parse_note_permissive_defaults_and_frontmatter() {
    // No frontmatter at all: still a note, type defaults to `note`.
    let plain = parse_note("just a body line\nmore");
    assert_eq!(plain.note_type, "note");
    assert_eq!(plain.body, "just a body line\nmore");

    let full = parse_note(
        "---\ntype: decision\ntitle: Soft delete\ndescription: soft delete 90d\ntags: [accounts, data]\ncard: 01CARD\nupdated: 2026-07-04\n---\n\n## Decision\nSoft-delete.\n",
    );
    assert_eq!(full.note_type, "decision");
    assert_eq!(full.title.as_deref(), Some("Soft delete"));
    assert_eq!(full.description.as_deref(), Some("soft delete 90d"));
    assert_eq!(full.tags, vec!["accounts", "data"]);
    assert_eq!(full.source_card.as_deref(), Some("01CARD"));
    assert!(full.body.starts_with("## Decision"));
}

#[test]
fn parse_note_block_tag_list_and_unknown_keys_preserved() {
    let n = parse_note(
        "---\ntype: convention\ntitle: Errors\ntags:\n  - errors\n  - copy\nauthor: someone\ncustom_key: whatever\n---\nbody\n",
    );
    assert_eq!(n.note_type, "convention");
    assert_eq!(n.tags, vec!["errors", "copy"]);
    // Unknown keys are ignored by the reader, never rejected.
    assert_eq!(n.body.trim(), "body");
}

#[test]
fn write_note_creates_then_replaces_idempotently() {
    let dir = temp_dir("write");
    let out = write_note(&dir, "gotcha", "Resize storm", "Debounce resize on Windows.", &["windows".into()], Some("01CARD")).unwrap();
    assert_eq!(out.id, "gotchas/resize-storm");
    assert_eq!(out.rel_path, "gotchas/resize-storm.md");
    assert!(out.created);
    let path = dir.join("gotchas/resize-storm.md");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("type: gotcha"));
    assert!(text.contains("card: 01CARD"));
    assert!(text.contains("Debounce resize on Windows."));

    // Re-add same id: created=false (replace), body updated.
    let out2 = write_note(&dir, "gotcha", "Resize storm", "New body.", &[], None).unwrap();
    assert!(!out2.created);
    let text2 = std::fs::read_to_string(&path).unwrap();
    assert!(text2.contains("New body."));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn scan_finds_notes_skips_index_and_sorts() {
    let dir = temp_dir("scan");
    write_note(&dir, "decision", "Bravo", "second", &[], None).unwrap();
    write_note(&dir, "gotcha", "Alpha", "first", &[], None).unwrap();
    // A plain markdown drop-in with no frontmatter still counts.
    std::fs::write(dir.join("loose.md"), "loose note body").unwrap();
    // index.md is generated, never scanned as a note.
    std::fs::write(dir.join("index.md"), "# x\n## Catalog\n").unwrap();

    let notes = scan(&dir);
    let ids: Vec<&str> = notes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, vec!["decisions/bravo", "gotchas/alpha", "loose"]);
    let loose = notes.iter().find(|n| n.id == "loose").unwrap();
    assert_eq!(loose.note_type, "note");
    assert_eq!(loose.description.as_deref(), Some("loose note body"));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn regenerate_catalog_is_deterministic_and_preserves_digest() {
    let dir = temp_dir("catalog");
    write_note(&dir, "decision", "Soft delete", "accounts soft-delete 90d", &[], None).unwrap();
    write_note(&dir, "gotcha", "Resize storm", "debounce resize on windows", &[], None).unwrap();

    // A human writes a Digest section; the daemon must preserve it verbatim.
    std::fs::write(
        dir.join("index.md"),
        "# Project knowledge - acme\n\n## Digest\ntailwind; accounts soft-delete 90d\n\n## Catalog\n- stale\n",
    )
    .unwrap();

    let notes = scan(&dir);
    regenerate_catalog(&dir, "acme", &notes).unwrap();
    let first = std::fs::read_to_string(dir.join("index.md")).unwrap();
    assert!(first.contains("## Digest\ntailwind; accounts soft-delete 90d"));
    assert!(first.contains("- decisions (1): [[decisions/soft-delete]] - accounts soft-delete 90d"));
    assert!(first.contains("- gotchas (1): [[gotchas/resize-storm]] - debounce resize on windows"));

    // Idempotent: regenerating from the same notes yields byte-identical output.
    regenerate_catalog(&dir, "acme", &notes).unwrap();
    let second = std::fs::read_to_string(dir.join("index.md")).unwrap();
    assert_eq!(first, second);

    // Digest reads back, capped and trimmed.
    let digest = read_digest(&dir).unwrap();
    assert_eq!(digest, "tailwind; accounts soft-delete 90d");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn digest_is_capped_at_thirty_lines() {
    let dir = temp_dir("digest-cap");
    let mut idx = String::from("# t\n\n## Digest\n");
    for i in 0..50 {
        idx.push_str(&format!("line {i}\n"));
    }
    idx.push_str("\n## Catalog\n");
    std::fs::write(dir.join("index.md"), idx).unwrap();
    let digest = read_digest(&dir).unwrap();
    assert_eq!(digest.lines().count(), DIGEST_LINE_CAP);
    assert!(digest.starts_with("line 0"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn catalog_without_existing_index_creates_titled_file() {
    let dir = temp_dir("fresh-index");
    write_note(&dir, "runbook", "Release", "manual release steps", &[], None).unwrap();
    let notes = scan(&dir);
    regenerate_catalog(&dir, "myproj", &notes).unwrap();
    let text = std::fs::read_to_string(dir.join("index.md")).unwrap();
    assert!(text.starts_with("# Project knowledge - myproj"));
    assert!(text.contains("- runbooks (1): [[runbooks/release]] - manual release steps"));
    // No Digest section fabricated by the daemon.
    assert!(read_digest(&dir).is_none());
    std::fs::remove_dir_all(&dir).ok();
}
