//! The `knowledge_notes` index (`data-model.md` / Data-model touchpoints,
//! `knowledge.md` / Sync and conflict posture).
//!
//! The files on disk are the truth; this table is a fast index for `dflow know find`,
//! catalog counts, and the future in-app viewer. It is rebuilt from a directory scan
//! on daemon start and after every write, never treated as a second source of truth.

use rusqlite::{params, Row};

use crate::knowledge::IndexedNote;

use super::{new_ulid, Store, StoreError};

/// One indexed knowledge note row (the cached projection of a note's frontmatter).
#[derive(Debug, Clone)]
pub struct KnowledgeRow {
    pub id: String,
    pub path: String,
    pub note_type: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub source_card: Option<String>,
    pub updated_at: Option<i64>,
}

impl Store {
    /// Replace the whole index for one project with a fresh scan (`knowledge.md`:
    /// "rebuilt from the directory on daemon start and on write"). One transaction so
    /// a reader never sees a half-rebuilt index.
    pub fn rebuild_knowledge_index(
        &self,
        project_id: &str,
        notes: &[IndexedNote],
    ) -> Result<(), StoreError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM knowledge_notes WHERE project_id = ?1", params![project_id])?;
        for note in notes {
            let tags_json = serde_json::to_string(&note.tags).ok();
            tx.execute(
                "INSERT INTO knowledge_notes \
                 (id, project_id, path, type, title, description, tags, source_card, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    new_ulid().to_string(),
                    project_id,
                    note.path,
                    note.note_type,
                    note.title,
                    note.description,
                    tags_json,
                    note.source_card,
                    note.updated_at,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Catalog counts by type for a project, type-sorted (the `dflow know` aggregate).
    pub fn knowledge_catalog(&self, project_id: &str) -> Result<Vec<(String, u32)>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT type, count(*) FROM knowledge_notes WHERE project_id = ?1 \
             GROUP BY type ORDER BY type ASC",
        )?;
        let rows = stmt.query_map(params![project_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u32))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Total indexed notes for a project (an AXI aggregate).
    pub fn knowledge_count(&self, project_id: &str) -> Result<u32, StoreError> {
        let conn = self.lock();
        let n: i64 = conn.query_row(
            "SELECT count(*) FROM knowledge_notes WHERE project_id = ?1",
            params![project_id],
            |r| r.get(0),
        )?;
        Ok(n as u32)
    }

    /// Search a project's notes by substring over title, tags, description, and id/path
    /// (`knowledge.md` / `dflow know find`), optionally filtered by type. Id-sorted for
    /// stable, deterministic output. Case-insensitive.
    pub fn find_knowledge(
        &self,
        project_id: &str,
        query: &str,
        type_filter: Option<&str>,
    ) -> Result<Vec<KnowledgeRow>, StoreError> {
        let needle = format!("%{}%", query.trim().to_lowercase());
        let conn = self.lock();
        let mut sql = String::from(
            "SELECT id, path, type, title, description, tags, source_card, updated_at \
             FROM knowledge_notes WHERE project_id = ?1 \
             AND (lower(coalesce(title,'')) LIKE ?2 OR lower(coalesce(tags,'')) LIKE ?2 \
                  OR lower(coalesce(description,'')) LIKE ?2 OR lower(path) LIKE ?2)",
        );
        if type_filter.is_some() {
            sql.push_str(" AND type = ?3");
        }
        sql.push_str(" ORDER BY path ASC");
        let mut stmt = conn.prepare(&sql)?;
        let rows = if let Some(t) = type_filter {
            stmt.query_map(params![project_id, needle, t], knowledge_row_from)?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![project_id, needle], knowledge_row_from)?
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(rows)
    }
}

/// Convert the `path`-relative id (path minus `.md`) used by the row.
fn knowledge_row_from(row: &Row) -> rusqlite::Result<KnowledgeRow> {
    let path: String = row.get(1)?;
    let id = path.strip_suffix(".md").unwrap_or(&path).to_string();
    let tags_json: Option<String> = row.get(5)?;
    let tags = tags_json
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default();
    Ok(KnowledgeRow {
        id,
        path,
        note_type: row.get(2)?,
        title: row.get(3)?,
        description: row.get(4)?,
        tags,
        source_card: row.get(6)?,
        updated_at: row.get(7)?,
    })
}
