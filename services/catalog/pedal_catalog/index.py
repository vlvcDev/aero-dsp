from __future__ import annotations

import hashlib
import json
import sqlite3
from dataclasses import dataclass, replace
from pathlib import Path


SCHEMA = """
CREATE TABLE IF NOT EXISTS models (
    sha256 TEXT NOT NULL,
    name TEXT NOT NULL,
    path TEXT PRIMARY KEY,
    size_bytes INTEGER NOT NULL,
    modified_ns INTEGER NOT NULL,
    source TEXT NOT NULL DEFAULT 'local',
    favorite INTEGER NOT NULL DEFAULT 0 CHECK (favorite IN (0, 1))
);
CREATE INDEX IF NOT EXISTS models_name_idx ON models(name COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS models_sha256_idx ON models(sha256);
"""


@dataclass(frozen=True)
class ModelRecord:
    sha256: str
    name: str
    path: str
    size_bytes: int
    modified_ns: int
    source: str = "local"
    favorite: bool = False
    capture_group: str | None = None
    capture_id: str | None = None
    capture_group_name: str | None = None
    gear: str | None = None
    tags: tuple[str, ...] = ()


def normalize_tags(value):
    if not isinstance(value, (list, tuple)):
        return ()
    return tuple(sorted({tag.strip().lower() for tag in value
                         if isinstance(tag, str) and tag.strip()}))


def capture_metadata(record: ModelRecord) -> ModelRecord:
    try:
        metadata = json.loads(Path(record.path).with_suffix('.nam.json').read_text())
        tone, capture = metadata['tone_id'], metadata['model_id']
        if not isinstance(tone, str) or not isinstance(capture, str):
            return record
        return replace(record, capture_group=f'tone3000:{tone}', capture_id=capture,
                       gear=metadata.get('gear') if isinstance(metadata.get('gear'), str) else None,
                       tags=normalize_tags(metadata.get('tags')),
                       capture_group_name=str(metadata.get('group_name') or f'Tone {tone}'),
                       name=str(metadata.get('name') or record.name), source=f'tone3000:model:{capture}')
    except (OSError, ValueError, KeyError, TypeError):
        return record


def write_capture_metadata(path: Path, tone_id: str, model_id: str, name: str, group_name: str | None = None, gear: str | None = None, tags=None):
    destination = path.with_suffix('.nam.json')
    temporary = destination.with_suffix('.json.tmp')
    try:
        previous = json.loads(destination.read_text())
    except (OSError, ValueError):
        previous = {}
    if not isinstance(previous, dict):
        previous = {}
    temporary.write_text(json.dumps(dict(tone_id=str(tone_id), model_id=str(model_id), name=name,
        group_name=group_name or previous.get('group_name'), gear=gear or previous.get('gear'),
        tags=normalize_tags(previous.get('tags') if tags is None else tags))))
    temporary.replace(destination)


def _sha256(path: Path, chunk_size: int = 1024 * 1024) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as model_file:
        while chunk := model_file.read(chunk_size):
            digest.update(chunk)
    return digest.hexdigest()


def inspect_model(path: Path, *, source: str = "local") -> ModelRecord:
    resolved = path.resolve(strict=True)
    stat = resolved.stat()
    return capture_metadata(ModelRecord(
        sha256=_sha256(resolved),
        name=resolved.stem,
        path=str(resolved),
        size_bytes=stat.st_size,
        modified_ns=stat.st_mtime_ns,
        source=source,
    ))


def upsert_model(path: Path, database: Path, *, source: str = "local") -> ModelRecord:
    record = inspect_model(path, source=source)
    database.parent.mkdir(parents=True, exist_ok=True)
    with sqlite3.connect(database) as connection:
        connection.execute("PRAGMA journal_mode=WAL")
        connection.executescript(SCHEMA)
        connection.execute(
            """
            INSERT INTO models (sha256, name, path, size_bytes, modified_ns, source)
            VALUES (?, ?, ?, ?, ?, ?)
            ON CONFLICT(path) DO UPDATE SET
                sha256=excluded.sha256,
                name=excluded.name,
                size_bytes=excluded.size_bytes,
                modified_ns=excluded.modified_ns,
                source=CASE WHEN excluded.source='local' THEN models.source ELSE excluded.source END
            """,
            (
                record.sha256,
                record.name,
                record.path,
                record.size_bytes,
                record.modified_ns,
                record.source,
            ),
        )
    return record


def scan_models(models_dir: Path, database: Path) -> list[ModelRecord]:
    models_dir = models_dir.resolve()
    database.parent.mkdir(parents=True, exist_ok=True)
    records = [inspect_model(path) for path in sorted(models_dir.rglob("*.nam"))]

    with sqlite3.connect(database) as connection:
        connection.execute("PRAGMA journal_mode=WAL")
        connection.executescript(SCHEMA)
        connection.executemany(
            """
            INSERT INTO models (sha256, name, path, size_bytes, modified_ns, source)
            VALUES (?, ?, ?, ?, ?, ?)
            ON CONFLICT(path) DO UPDATE SET
                sha256=excluded.sha256,
                name=excluded.name,
                size_bytes=excluded.size_bytes,
                modified_ns=excluded.modified_ns,
                source=CASE WHEN excluded.source='local' THEN models.source ELSE excluded.source END
            """,
            [
                (
                    record.sha256,
                    record.name,
                    record.path,
                    record.size_bytes,
                    record.modified_ns,
                    record.source,
                )
                for record in records
            ],
        )
        indexed_paths = {record.path for record in records}
        existing_paths = connection.execute("SELECT path FROM models").fetchall()
        stale_paths = [
            (path,)
            for (path,) in existing_paths
            if Path(path).is_relative_to(models_dir) and path not in indexed_paths
        ]
        connection.executemany("DELETE FROM models WHERE path = ?", stale_paths)
    return records


def list_models(database: Path, *, query: str = "") -> list[ModelRecord]:
    if not database.exists():
        return []
    pattern = f"%{query}%"
    with sqlite3.connect(database) as connection:
        connection.executescript(SCHEMA)
        rows = connection.execute(
            """
            SELECT sha256, name, path, size_bytes, modified_ns, source, favorite
            FROM models
            WHERE name LIKE ? COLLATE NOCASE
            ORDER BY favorite DESC, name COLLATE NOCASE, path
            """,
            (pattern,),
        ).fetchall()
    return [
        capture_metadata(ModelRecord(
            sha256=row[0],
            name=row[1],
            path=row[2],
            size_bytes=row[3],
            modified_ns=row[4],
            source=row[5],
            favorite=bool(row[6]),
        ))
        for row in rows
        if Path(row[2]).is_file()
    ]
