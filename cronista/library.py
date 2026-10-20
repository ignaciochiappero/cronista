"""The history of recordings, searchable by what was said.

Adapted from escriba's backend/app/core/database.py: the same SQLite table plus FTS5 index kept in
sync by triggers, and the same query sanitizer. It uses the standard sqlite3 module with one
connection per call instead of aiosqlite, because pywebview calls in from plain worker threads.
"""
from __future__ import annotations

import json
import sqlite3
import uuid
from pathlib import Path

SCHEMA = """
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
"""
LIST_COLUMNS = "r.id, r.title, r.created_at, r.duration_seconds, r.size_bytes, r.folder, r.video_path, r.kind"
# What a history entry is: Cronista's own recording, or a file the user dropped, with or without pictures.
KINDS = ("recording", "audio", "video")


def sanitize_fts_query(search: str) -> str | None:
    """Quotes every token so FTS5 reads symbols as text, not syntax (escriba's rule), and makes each
    one a prefix ("prec" finds "precios"): the page searches while the words are being typed."""
    tokens = [token for token in search.split() if token]
    if not tokens:
        return None
    return " ".join('"' + token.replace('"', '""') + '"*' for token in tokens)


class Library:
    def __init__(self, path: Path) -> None:
        self.path = Path(path)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with self._connect() as db:
            db.executescript(SCHEMA)
            # Histories from before 0.5.0 have no labels column, and from before 0.7.0 no kind: add
            # them, empty or "recording", keeping everything else.
            columns = {row["name"] for row in db.execute("PRAGMA table_info(recordings)")}
            if "labels_json" not in columns:
                db.execute("ALTER TABLE recordings ADD COLUMN labels_json TEXT DEFAULT '{}'")
            if "kind" not in columns:
                db.execute("ALTER TABLE recordings ADD COLUMN kind TEXT DEFAULT 'recording'")

    def _connect(self) -> sqlite3.Connection:
        db = sqlite3.connect(str(self.path))
        db.row_factory = sqlite3.Row
        return db

    def add(self, *, title: str, created_at: str, duration_seconds: float, size_bytes: int, folder: str,
            video_path: str, transcript: str, segments: list[dict], labels: dict | None = None,
            kind: str = "recording") -> str:
        """`labels`: the name of each audio source ("mic", "computer") when the recording was made.
        `kind`: one of KINDS; for a dropped file, `video_path` is the file itself, wherever it is."""
        recording_id = uuid.uuid4().hex
        with self._connect() as db:
            db.execute(
                "INSERT INTO recordings (id, title, created_at, duration_seconds, size_bytes, folder, video_path, "
                "transcript, segments_json, labels_json, kind) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                (recording_id, title, created_at, duration_seconds, size_bytes, folder, video_path,
                 transcript, json.dumps(segments, ensure_ascii=False), json.dumps(labels or {}, ensure_ascii=False),
                 kind if kind in KINDS else "recording"),
            )
        return recording_id

    def list(self, search: str | None = None, limit: int = 200) -> list[dict]:
        query = sanitize_fts_query(search) if search else None
        with self._connect() as db:
            if query:
                rows = db.execute(
                    f"SELECT {LIST_COLUMNS}, snippet(recordings_fts, 1, '[', ']', '…', 10) AS snippet "
                    "FROM recordings r JOIN recordings_fts f ON r.rowid = f.rowid "
                    "WHERE recordings_fts MATCH ? ORDER BY r.created_at DESC, r.rowid DESC LIMIT ?",
                    (query, limit),
                ).fetchall()
            else:
                rows = db.execute(
                    f"SELECT {LIST_COLUMNS}, substr(r.transcript, 1, 200) AS snippet "
                    # Files dropped together can share a second: the one added last comes first.
                    "FROM recordings r ORDER BY r.created_at DESC, r.rowid DESC LIMIT ?",
                    (limit,),
                ).fetchall()
        return [dict(row) for row in rows]

    def get(self, recording_id: str) -> dict | None:
        with self._connect() as db:
            row = db.execute("SELECT * FROM recordings WHERE id = ?", (recording_id,)).fetchone()
        if row is None:
            return None
        record = dict(row)
        record["segments"] = json.loads(record.pop("segments_json") or "[]")
        record["labels"] = json.loads(record.pop("labels_json") or "{}")
        return record

    def set_label(self, recording_id: str, source: str, label: str) -> None:
        """Names one audio source of one recording ("computer" -> "Ana")."""
        record = self.get(recording_id)
        if record is None:
            return
        labels = {**record["labels"], source: label}
        with self._connect() as db:
            db.execute("UPDATE recordings SET labels_json = ? WHERE id = ?",
                       (json.dumps(labels, ensure_ascii=False), recording_id))

    def rename(self, recording_id: str, title: str) -> None:
        with self._connect() as db:
            db.execute("UPDATE recordings SET title = ? WHERE id = ?", (title, recording_id))

    def delete(self, recording_id: str) -> bool:
        with self._connect() as db:
            cursor = db.execute("DELETE FROM recordings WHERE id = ?", (recording_id,))
        return cursor.rowcount > 0
