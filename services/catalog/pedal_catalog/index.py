from __future__ import annotations

import hashlib
import json
import logging
import os
import shutil
import sqlite3
import subprocess
import threading
from datetime import datetime, timezone
from dataclasses import dataclass, replace
from pathlib import Path

log = logging.getLogger(__name__)


SCHEMA = """
CREATE TABLE IF NOT EXISTS models (
    sha256 TEXT NOT NULL,
    name TEXT NOT NULL,
    path TEXT PRIMARY KEY,
    size_bytes INTEGER NOT NULL,
    modified_ns INTEGER NOT NULL,
    source TEXT NOT NULL DEFAULT 'local',
    favorite INTEGER NOT NULL DEFAULT 0 CHECK (favorite IN (0, 1)),
    last_used TEXT
);
CREATE INDEX IF NOT EXISTS models_name_idx ON models(name COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS models_sha256_idx ON models(sha256);
CREATE TABLE IF NOT EXISTS capture_info (
    sha256 TEXT PRIMARY KEY,
    gain_db REAL,
    includes_cab INTEGER,
    gear_type TEXT,
    measured_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
"""


def _ensure_schema(connection: sqlite3.Connection) -> None:
    connection.executescript(SCHEMA)
    columns = {row[1] for row in connection.execute("PRAGMA table_info(models)")}
    if "last_used" not in columns:  # database created before 'recent' sorting
        connection.execute("ALTER TABLE models ADD COLUMN last_used TEXT")


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
    gain_db: float | None = None
    includes_cab: bool | None = None
    kind: str = "nam"
    last_used: str | None = None


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
        _ensure_schema(connection)
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
    irs_dir = models_dir.parent / "irs"
    records = [inspect_model(path) for path in sorted(models_dir.rglob("*.nam"))]
    if irs_dir.is_dir():
        records += [inspect_model(path) for path in sorted(irs_dir.rglob("*.wav"))]

    with sqlite3.connect(database) as connection:
        connection.execute("PRAGMA journal_mode=WAL")
        _ensure_schema(connection)
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
            if (Path(path).is_relative_to(models_dir) or Path(path).is_relative_to(irs_dir))
            and path not in indexed_paths
        ]
        connection.executemany("DELETE FROM models WHERE path = ?", stale_paths)
    return records


def list_models(
    database: Path, *, query: str = "", kind: str | None = "nam", sort: str = "name"
) -> list[ModelRecord]:
    """List indexed captures (kind 'nam'), IRs (kind 'ir'), or both (kind None)."""
    if not database.exists():
        return []
    suffix = {"nam": "%.nam", "ir": "%.wav", None: "%"}[kind]
    order = (
        "m.last_used IS NULL, m.last_used DESC, m.name COLLATE NOCASE, m.path"
        if sort == "recent"
        else "m.favorite DESC, m.name COLLATE NOCASE, m.path"
    )
    with sqlite3.connect(database) as connection:
        _ensure_schema(connection)
        rows = connection.execute(
            f"""
            SELECT m.sha256, m.name, m.path, m.size_bytes, m.modified_ns, m.source, m.favorite,
                   c.gain_db, c.includes_cab, m.last_used
            FROM models m LEFT JOIN capture_info c ON c.sha256 = m.sha256
            WHERE m.name LIKE ? COLLATE NOCASE AND m.path LIKE ? COLLATE NOCASE
            ORDER BY {order}
            """,
            (f"%{query}%", suffix),
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
            gain_db=row[7],
            includes_cab=None if row[8] is None else bool(row[8]),
            kind="ir" if row[2].lower().endswith(".wav") else "nam",
            last_used=row[9],
        ))
        for row in rows
        if Path(row[2]).is_file()
    ]


def _update_model(database: Path, path: str, assignment: str, value) -> bool:
    if not database.exists():
        return False
    with sqlite3.connect(database) as connection:
        _ensure_schema(connection)
        cursor = connection.execute(
            f"UPDATE models SET {assignment} = ? WHERE path = ?",
            (value, str(Path(path).resolve())),
        )
        return cursor.rowcount > 0


def set_favorite(database: Path, path: str, favorite: bool) -> bool:
    return _update_model(database, path, "favorite", int(favorite))


def touch_model(database: Path, path: str) -> bool:
    """Record that a capture/IR was just loaded (drives the 'recent' sort)."""
    return _update_model(database, path, "last_used", datetime.now(timezone.utc).isoformat())


def local_tone_paths(database: Path) -> dict[str, str]:
    """Map TONE3000 tone ids to a locally downloaded capture/IR path."""
    paths: dict[str, str] = {}
    for record in list_models(database, kind=None):
        if record.capture_group and record.capture_group.startswith("tone3000:"):
            paths.setdefault(record.capture_group.removeprefix("tone3000:"), record.path)
    return paths


def capture_info_bin() -> str | None:
    override = os.environ.get("PEDAL_CAPTURE_INFO_BIN")
    if override:
        return override
    if found := shutil.which("pedal-capture-info"):
        return found
    repo = Path(__file__).resolve().parents[3]
    for candidate in (Path("/opt/pedal/bin/pedal-capture-info"),
                      repo / "target/release/pedal-capture-info",
                      repo / "target/debug/pedal-capture-info"):
        if candidate.is_file():
            return str(candidate)
    return None


_measure_lock = threading.Lock()


def _measure_uncached(database: Path, binary: str, batch_size: int = 20) -> None:
    with sqlite3.connect(database) as connection:
        _ensure_schema(connection)
        rows = connection.execute(
            """SELECT m.sha256, MIN(m.path) FROM models m
               LEFT JOIN capture_info c ON c.sha256 = m.sha256
               WHERE c.sha256 IS NULL AND m.path LIKE '%.nam' GROUP BY m.sha256"""
        ).fetchall()
    rows = [(sha, path) for sha, path in rows if Path(path).is_file()]
    for start in range(0, len(rows), batch_size):
        batch = dict((path, sha) for sha, path in rows[start:start + batch_size])
        try:
            result = subprocess.run([binary, *batch], capture_output=True, text=True,
                                    timeout=30 + 2 * len(batch), check=True)
            found = []
            for line in result.stdout.splitlines():
                info = json.loads(line)
                if info.get("path") in batch:
                    cab = info.get("includes_cab")
                    found.append((batch[info["path"]], info.get("gain_db"),
                                  None if cab is None else int(bool(cab)), info.get("gear_type")))
            with sqlite3.connect(database) as connection:
                connection.executemany(
                    "INSERT OR REPLACE INTO capture_info (sha256, gain_db, includes_cab, gear_type)"
                    " VALUES (?, ?, ?, ?)", found)
        except (OSError, ValueError, subprocess.SubprocessError, sqlite3.Error) as error:
            log.warning("capture measurement failed: %s", error)


def measure_captures_async(database: Path) -> threading.Thread | None:
    """Measure uncached captures on one background worker; no-op if the CLI is missing."""
    binary = capture_info_bin()
    if not binary or not database.exists() or not _measure_lock.acquire(blocking=False):
        return None

    def work():
        try:
            _measure_uncached(database, binary)
        except Exception:
            log.exception("capture measurement worker failed")
        finally:
            _measure_lock.release()

    thread = threading.Thread(target=work, name="capture-info", daemon=True)
    thread.start()
    return thread
