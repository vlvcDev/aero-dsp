"""Small, local-only library of AI rigs a player has personally approved."""

from __future__ import annotations

import json
import os
import secrets
import time
import re
from pathlib import Path
from typing import Any


class ToneMemoryError(RuntimeError):
    pass


class ToneMemoryService:
    """Persists confirmed tone recipes without credentials or audio recordings."""

    def __init__(self, path: Path, *, clock: callable = time.time) -> None:
        self.path = path
        self.clock = clock

    def save(self, candidate: dict[str, Any], note: str = "") -> dict[str, Any]:
        query = _text(candidate.get("query"), "query", 3, 500)
        summary = _text(candidate.get("summary"), "summary", 1, 500)
        rig_label = _text(candidate.get("rig_label"), "rig_label", 1, 120)
        note = note.strip()
        if len(note) > 280:
            raise ToneMemoryError("note must be 280 characters or fewer")

        amp_model = _model(candidate.get("amp_model"), "amp_model")
        pre_model = candidate.get("pre_model")
        if pre_model is not None:
            pre_model = _model(pre_model, "pre_model")
        cabinet_ir = candidate.get("cabinet_ir")
        if cabinet_ir is not None:
            cabinet_ir = _model(cabinet_ir, "cabinet_ir")
        controls = _number_map(candidate.get("controls"), "controls")
        effects = _bool_map(candidate.get("effects"), "effects")
        record = {
            "id": secrets.token_urlsafe(12),
            "created_at_ms": int(self.clock() * 1000),
            "query": query,
            "summary": summary,
            "rig_label": rig_label,
            "note": note,
            "pre_model": pre_model,
            "amp_model": amp_model,
            "cabinet_ir": cabinet_ir,
            "controls": controls,
            "effects": effects,
        }
        records = self.list_records()
        records.insert(0, record)
        # Keep this intentionally tiny and portable; a JSON backup can simply
        # travel with the player's cached NAMs.
        self._write(records[:100])
        return record

    def list_records(self) -> list[dict[str, Any]]:
        if not self.path.exists():
            return []
        try:
            payload = json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise ToneMemoryError("could not read local Tone Memory") from error
        if not isinstance(payload, list):
            raise ToneMemoryError("local Tone Memory is invalid")
        return [record for record in payload if isinstance(record, dict)]

    def matches(self, query: str, limit: int = 3) -> list[dict[str, Any]]:
        """Return personally approved recipes with meaningful word overlap."""
        query_terms = _terms(query)
        if not query_terms:
            return []
        scored: list[tuple[float, dict[str, Any]]] = []
        for record in self.list_records():
            saved_query = record.get("query")
            if not isinstance(saved_query, str):
                continue
            saved_terms = _terms(saved_query)
            overlap = query_terms & saved_terms
            if len(overlap) < min(2, len(query_terms), len(saved_terms)):
                continue
            score = len(overlap) / len(query_terms | saved_terms)
            if query.strip().casefold() == saved_query.strip().casefold():
                score = 2.0
            scored.append((score, record))
        scored.sort(key=lambda item: (item[0], item[1].get("created_at_ms", 0)), reverse=True)
        return [record for _, record in scored[:limit]]

    def has_records(self) -> bool:
        return bool(self.list_records())

    def _write(self, records: list[dict[str, Any]]) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        temporary = self.path.with_suffix(f"{self.path.suffix}.tmp")
        try:
            temporary.write_text(json.dumps(records, indent=2) + "\n", encoding="utf-8")
            os.replace(temporary, self.path)
        except OSError as error:
            temporary.unlink(missing_ok=True)
            raise ToneMemoryError("could not save local Tone Memory") from error


def _text(value: Any, field: str, minimum: int, maximum: int) -> str:
    if not isinstance(value, str):
        raise ToneMemoryError(f"{field} is required")
    value = value.strip()
    if not minimum <= len(value) <= maximum:
        raise ToneMemoryError(f"{field} has an invalid length")
    return value


def _model(value: Any, field: str) -> dict[str, str]:
    if not isinstance(value, dict):
        raise ToneMemoryError(f"{field} is required")
    return {
        key: _text(value.get(key), f"{field}.{key}", 1, 1_000)
        for key in ("name", "path", "source")
    }


def _number_map(value: Any, field: str) -> dict[str, float]:
    if not isinstance(value, dict):
        raise ToneMemoryError(f"{field} is required")
    return {
        key: float(item)
        for key, item in value.items()
        if isinstance(key, str) and isinstance(item, (int, float))
    }


def _bool_map(value: Any, field: str) -> dict[str, bool]:
    if not isinstance(value, dict):
        raise ToneMemoryError(f"{field} is required")
    return {key: item for key, item in value.items() if isinstance(key, str) and isinstance(item, bool)}


def _terms(value: str) -> set[str]:
    ignored = {"a", "an", "and", "for", "give", "i", "like", "me", "my", "of", "the", "to", "tone", "want", "with"}
    return {
        term
        for term in re.findall(r"[a-z0-9]+", value.casefold())
        if len(term) > 1 and term not in ignored
    }
