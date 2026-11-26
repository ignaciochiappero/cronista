//! Recording history backed by SQLite + FTS5.
//! 100% schema & migration compatible with Cronista's existing `%LOCALAPPDATA%\Cronista\library.db`.

use crate::engine::transcript::TranscriptionSegment;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS recordings (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    created_at TEXT NOT NULL,
    duration_seconds REAL DEFAULT 0,
    size_bytes INTEGER DEFAULT 0,
    folder TEXT NOT NULL,
    video_path TEXT NOT NULL,
    transcript TEXT DEFAULT '',
    segments_json TEXT DEFAULT '[]',
    notes TEXT DEFAULT '',
    labels_json TEXT DEFAULT '{}',
    kind TEXT DEFAULT 'recording'
);
CREATE VIRTUAL TABLE IF NOT EXISTS recordings_fts
    USING fts5(title, transcript, notes, content='recordings', content_rowid='rowid');
CREATE TRIGGER IF NOT EXISTS recordings_ai AFTER INSERT ON recordings BEGIN
    INSERT INTO recordings_fts(rowid, title, transcript, notes)
    VALUES (new.rowid, new.title, new.transcript, new.notes);
END;
CREATE TRIGGER IF NOT EXISTS recordings_ad AFTER DELETE ON recordings BEGIN
    INSERT INTO recordings_fts(recordings_fts, rowid, title, transcript, notes)
    VALUES ('delete', old.rowid, old.title, old.transcript, old.notes);
END;
CREATE TRIGGER IF NOT EXISTS recordings_au AFTER UPDATE ON recordings BEGIN
    INSERT INTO recordings_fts(recordings_fts, rowid, title, transcript, notes)
    VALUES ('delete', old.rowid, old.title, old.transcript, old.notes);
    INSERT INTO recordings_fts(rowid, title, transcript, notes)
    VALUES (new.rowid, new.title, new.transcript, new.notes);
END;
"#;

pub const KINDS: &[&str] = &["recording", "audio_recording", "audio", "video"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingSummary {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub duration_seconds: f64,
    pub size_bytes: u64,
    pub folder: String,
    pub video_path: String,
    pub kind: String,
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingDetail {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub duration_seconds: f64,
    pub size_bytes: u64,
    pub folder: String,
    pub video_path: String,
    pub transcript: String,
    pub segments: Vec<TranscriptionSegment>,
    pub notes: String,
    pub labels: HashMap<String, String>,
    pub kind: String,
}

impl RecordingDetail {
    /// Whether this entry is an imported external file (whose original media must not be deleted).
    pub fn is_imported(&self) -> bool {
        matches!(self.kind.as_str(), "audio" | "video")
    }

    /// Whether this entry has video frames that can receive burned-in subtitles.
    pub fn has_video(&self) -> bool {
        !matches!(self.kind.as_str(), "audio" | "audio_recording")
    }
}

#[derive(Debug, Clone)]
pub struct NewRecording {
    pub title: String,
    pub created_at: String,
    pub duration_seconds: f64,
    pub size_bytes: u64,
    pub folder: String,
    pub video_path: String,
    pub transcript: String,
    pub segments: Vec<TranscriptionSegment>,
    pub labels: HashMap<String, String>,
    pub kind: String,
}

/// Quotes every search token as an FTS5 prefix (`"token"*`) so symbols are literal.
pub fn sanitize_fts_query(search: &str) -> Option<String> {
    let tokens: Vec<String> = search
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"*", t.replace('"', "\"\"")))
        .collect();
    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" "))
    }
}

#[derive(Debug, Clone)]
pub struct Library {
    path: PathBuf,
}

impl Library {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create library dir failed: {e}"))?;
        }
        let lib = Self { path };
        let conn = lib.connect()?;
        conn.execute_batch(SCHEMA)
            .map_err(|e| format!("init schema failed: {e}"))?;

        let mut stmt = conn
            .prepare("PRAGMA table_info(recordings)")
            .map_err(|e| format!("pragma table_info failed: {e}"))?;
        let columns: HashSet<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| format!("read columns failed: {e}"))?
            .filter_map(|r| r.ok())
            .collect();
        drop(stmt);

        if !columns.contains("labels_json") {
            conn.execute(
                "ALTER TABLE recordings ADD COLUMN labels_json TEXT DEFAULT '{}'",
                [],
            )
            .map_err(|e| format!("migrate labels_json failed: {e}"))?;
        }
        if !columns.contains("kind") {
            conn.execute(
                "ALTER TABLE recordings ADD COLUMN kind TEXT DEFAULT 'recording'",
                [],
            )
            .map_err(|e| format!("migrate kind failed: {e}"))?;
        }
        Ok(lib)
    }

    fn connect(&self) -> Result<Connection, String> {
        Connection::open(&self.path).map_err(|e| format!("open sqlite failed: {e}"))
    }

    pub fn add(&self, rec: &NewRecording) -> Result<String, String> {
        let id = Uuid::new_v4().simple().to_string();
        let segments_json = serde_json::to_string(&rec.segments).unwrap_or_else(|_| "[]".into());
        let labels_json = serde_json::to_string(&rec.labels).unwrap_or_else(|_| "{}".into());
        let kind = if KINDS.contains(&rec.kind.as_str()) {
            rec.kind.as_str()
        } else {
            "recording"
        };
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO recordings (id, title, created_at, duration_seconds, size_bytes, folder, video_path, \
             transcript, segments_json, labels_json, kind) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                id,
                rec.title,
                rec.created_at,
                rec.duration_seconds,
                rec.size_bytes as i64,
                rec.folder,
                rec.video_path,
                rec.transcript,
                segments_json,
                labels_json,
                kind,
            ],
        )
        .map_err(|e| format!("insert recording failed: {e}"))?;
        Ok(id)
    }

    pub fn list(&self, search: Option<&str>, limit: usize) -> Result<Vec<RecordingSummary>, String> {
        let query = search.and_then(sanitize_fts_query);
        let conn = self.connect()?;
        let mut out = Vec::new();
        if let Some(q) = query {
            let mut stmt = conn
                .prepare(
                    "SELECT r.id, COALESCE(r.title, ''), COALESCE(r.created_at, ''), r.duration_seconds, r.size_bytes, COALESCE(r.folder, ''), \
                     COALESCE(r.video_path, ''), COALESCE(r.kind, 'recording'), snippet(recordings_fts, 1, '[', ']', '…', 10) AS snippet \
                     FROM recordings r JOIN recordings_fts f ON r.rowid = f.rowid \
                     WHERE recordings_fts MATCH ?1 ORDER BY r.created_at DESC, r.rowid DESC LIMIT ?2",
                )
                .map_err(|e| format!("prepare fts search failed: {e}"))?;
            let rows = stmt
                .query_map(params![q, limit as i64], |row| {
                    Ok(RecordingSummary {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        created_at: row.get(2)?,
                        duration_seconds: row.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                        size_bytes: row.get::<_, Option<i64>>(4)?.unwrap_or(0).max(0) as u64,
                        folder: row.get(5)?,
                        video_path: row.get(6)?,
                        kind: row.get(7)?,
                        snippet: row.get::<_, Option<String>>(8)?.unwrap_or_default(),
                    })
                })
                .map_err(|e| format!("query fts search failed: {e}"))?;
            for r in rows {
                if let Ok(item) = r {
                    out.push(item);
                }
            }
        } else {
            let mut stmt = conn
                .prepare(
                    "SELECT r.id, COALESCE(r.title, ''), COALESCE(r.created_at, ''), r.duration_seconds, r.size_bytes, COALESCE(r.folder, ''), \
                     COALESCE(r.video_path, ''), COALESCE(r.kind, 'recording'), substr(COALESCE(r.transcript, ''), 1, 200) AS snippet \
                     FROM recordings r ORDER BY r.created_at DESC, r.rowid DESC LIMIT ?1",
                )
                .map_err(|e| format!("prepare list failed: {e}"))?;
            let rows = stmt
                .query_map(params![limit as i64], |row| {
                    Ok(RecordingSummary {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        created_at: row.get(2)?,
                        duration_seconds: row.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                        size_bytes: row.get::<_, Option<i64>>(4)?.unwrap_or(0).max(0) as u64,
                        folder: row.get(5)?,
                        video_path: row.get(6)?,
                        kind: row.get(7)?,
                        snippet: row.get::<_, Option<String>>(8)?.unwrap_or_default(),
                    })
                })
                .map_err(|e| format!("query list failed: {e}"))?;
            for r in rows {
                if let Ok(item) = r {
                    out.push(item);
                }
            }
        }
        Ok(out)
    }

    pub fn get(&self, id: &str) -> Result<Option<RecordingDetail>, String> {
        let conn = self.connect()?;
        let row = conn
            .query_row(
                "SELECT id, title, created_at, duration_seconds, size_bytes, folder, video_path, \
                 COALESCE(transcript, ''), COALESCE(segments_json, '[]'), COALESCE(notes, ''), \
                 COALESCE(labels_json, '{}'), COALESCE(kind, 'recording') \
                 FROM recordings WHERE id = ?1",
                params![id],
                |row| {
                    let segs_str: String = row.get(8)?;
                    let labels_str: String = row.get(10)?;
                    let segments: Vec<TranscriptionSegment> =
                        serde_json::from_str(&segs_str).unwrap_or_default();
                    let labels: HashMap<String, String> =
                        serde_json::from_str(&labels_str).unwrap_or_default();
                    Ok(RecordingDetail {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        created_at: row.get(2)?,
                        duration_seconds: row.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                        size_bytes: row.get::<_, Option<i64>>(4)?.unwrap_or(0).max(0) as u64,
                        folder: row.get(5)?,
                        video_path: row.get(6)?,
                        transcript: row.get(7)?,
                        segments,
                        notes: row.get(9)?,
                        labels,
                        kind: row.get(11)?,
                    })
                },
            )
            .optional()
            .map_err(|e| format!("get recording failed: {e}"))?;
        Ok(row)
    }

    pub fn set_label(&self, id: &str, source: &str, label: &str) -> Result<(), String> {
        let Some(mut rec) = self.get(id)? else {
            return Ok(());
        };
        rec.labels.insert(source.to_string(), label.to_string());
        let json = serde_json::to_string(&rec.labels).unwrap_or_else(|_| "{}".into());
        let conn = self.connect()?;
        conn.execute(
            "UPDATE recordings SET labels_json = ?1 WHERE id = ?2",
            params![json, id],
        )
        .map_err(|e| format!("set_label failed: {e}"))?;
        Ok(())
    }

    pub fn rename(&self, id: &str, title: &str) -> Result<(), String> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE recordings SET title = ?1 WHERE id = ?2",
            params![title, id],
        )
        .map_err(|e| format!("rename failed: {e}"))?;
        Ok(())
    }

    pub fn delete(&self, id: &str) -> Result<bool, String> {
        let conn = self.connect()?;
        let affected = conn
            .execute("DELETE FROM recordings WHERE id = ?1", params![id])
            .map_err(|e| format!("delete failed: {e}"))?;
        Ok(affected > 0)
    }
}
