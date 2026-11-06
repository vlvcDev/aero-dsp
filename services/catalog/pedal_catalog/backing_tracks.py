"""Stable Audio-backed, user-initiated Riff Vault accompaniment renders.

The catalog owns the network request and the Stability credential.  A render is
intentionally never part of the real-time audio process: completed files are
ordinary cached audio that can be played while the device is offline.
"""

from __future__ import annotations

import json
import mimetypes
import os
import secrets
import struct
import threading
import time
import urllib.error
import urllib.request
import wave
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any, Protocol


STABLE_AUDIO_URL = "https://api.stability.ai/v2beta/audio/stable-audio-2/audio-to-audio"
STABLE_AUDIO_MODEL = "stable-audio-2.5"
STABLE_AUDIO_STEPS = 8
DEFAULT_DURATION_SECONDS = 20
DEFAULT_STRENGTH = 0.65
MAX_STYLE_LENGTH = 500
MIN_REFERENCE_SECONDS = 6
MAX_REFERENCE_SECONDS = 190


class BackingTrackError(RuntimeError):
    """Raised for concise, player-facing backing-track failures."""


class StableAudioTransport(Protocol):
    def generate(self, request: urllib.request.Request) -> bytes: ...


class UrllibStableAudioTransport:
    def generate(self, request: urllib.request.Request) -> bytes:
        try:
            with urllib.request.urlopen(request, timeout=180) as response:
                payload = response.read()
                content_type = response.headers.get_content_type()
        except urllib.error.HTTPError as error:
            detail = ""
            try:
                body = json.loads(error.read())
                detail = _stable_error_detail(body)
            except (json.JSONDecodeError, UnicodeDecodeError, AttributeError):
                pass
            suffix = f": {detail}" if detail else ""
            raise BackingTrackError(
                f"Stable Audio request failed ({error.code}){suffix}"
            ) from error
        except (urllib.error.URLError, TimeoutError) as error:
            raise BackingTrackError("Stable Audio could not be reached") from error
        if not payload:
            raise BackingTrackError("Stable Audio returned an empty track")
        if content_type == "application/json":
            raise BackingTrackError("Stable Audio returned an unexpected JSON response")
        return payload


@dataclass
class BackingTrack:
    id: str
    riff_id: str
    riff_name: str
    style: str
    created_at_ms: int
    status: str
    duration_seconds: int
    strength: float
    genre: str = "Unsorted"
    bpm: int = 120
    path: str | None = None
    error: str | None = None

    def as_payload(self) -> dict[str, Any]:
        return asdict(self)


class BackingTrackService:
    """Queues Stable Audio renders and persists only safe local metadata."""

    def __init__(
        self,
        api_key: str,
        riff_directory: Path,
        *,
        tracks_directory: Path | None = None,
        transport: StableAudioTransport | None = None,
        clock: callable = time.time,
    ) -> None:
        self.api_key = api_key.strip()
        self.riff_directory = riff_directory
        self.tracks_directory = tracks_directory or riff_directory / "backing-tracks"
        self.transport = transport or UrllibStableAudioTransport()
        self.clock = clock
        self._jobs: dict[str, BackingTrack] = {}
        self._lock = threading.Lock()

    @property
    def available(self) -> bool:
        return bool(self.api_key)

    def status(self) -> dict[str, Any]:
        return {"available": self.available}

    def queue(
        self,
        riff_id: str,
        style: str,
        *,
        duration_seconds: int = DEFAULT_DURATION_SECONDS,
        strength: float = DEFAULT_STRENGTH,
        genre: str = "Unsorted",
        bpm: int = 120,
    ) -> BackingTrack:
        if not self.available:
            raise BackingTrackError("Stable Audio is not configured on this device")
        riff = self._load_riff(riff_id)
        style = style.strip()
        if not 3 <= len(style) <= MAX_STYLE_LENGTH:
            raise BackingTrackError("Describe the backing style in 3 to 500 characters")
        if not 5 <= duration_seconds <= 60:
            raise BackingTrackError("Backing tracks can be 5 to 60 seconds long")
        if not 0.01 <= strength <= 1:
            raise BackingTrackError("Reference strength must be between 0.01 and 1")
        genre = _safe_genre(genre)
        if not 40 <= bpm <= 240:
            raise BackingTrackError("Backing track BPM must be between 40 and 240")
        clean_path = self._safe_riff_audio_path(riff.get("clean_path"))
        if not clean_path.is_file():
            raise BackingTrackError("The clean DI for that riff is no longer available")
        reference_seconds = _wav_duration_seconds(clean_path)
        if reference_seconds is not None and not (
            MIN_REFERENCE_SECONDS <= reference_seconds <= MAX_REFERENCE_SECONDS
        ):
            raise BackingTrackError(
                "Stable Audio needs a clean guitar reference between "
                f"{MIN_REFERENCE_SECONDS} and {MAX_REFERENCE_SECONDS} seconds"
            )

        created_at_ms = int(self.clock() * 1000)
        track = BackingTrack(
            id=f"backing-{created_at_ms}-{secrets.token_hex(3)}",
            riff_id=riff_id,
            riff_name=str(riff.get("name") or "Untitled Riff")[:64],
            style=style,
            created_at_ms=created_at_ms,
            status="queued",
            duration_seconds=duration_seconds,
            strength=strength,
            genre=genre,
            bpm=bpm,
        )
        with self._lock:
            self._jobs[track.id] = track
        thread = threading.Thread(
            target=self._render,
            args=(track.id, clean_path),
            name=f"pedal-backing-{track.id[-6:]}",
            daemon=True,
        )
        thread.start()
        return track

    def job(self, track_id: str) -> BackingTrack:
        with self._lock:
            track = self._jobs.get(track_id)
            if track is not None:
                return track
        for track in self.list_tracks():
            if track.id == track_id:
                return track
        raise BackingTrackError("That backing-track job is no longer available")

    def list_tracks(self, riff_id: str | None = None) -> list[BackingTrack]:
        tracks = []
        if self.tracks_directory.exists():
            for file in self.tracks_directory.rglob("*.json"):
                try:
                    data = json.loads(file.read_text())
                    track = BackingTrack(**data)
                except (OSError, TypeError, json.JSONDecodeError):
                    continue
                if riff_id is None or track.riff_id == riff_id:
                    tracks.append(track)
        with self._lock:
            for track in self._jobs.values():
                if track.status not in {"queued", "rendering"}:
                    continue
                if riff_id is None or track.riff_id == riff_id:
                    tracks = [item for item in tracks if item.id != track.id]
                    tracks.append(track)
        return sorted(tracks, key=lambda item: item.created_at_ms, reverse=True)

    def _render(self, track_id: str, clean_path: Path) -> None:
        track = self.job(track_id)
        self._replace(track_id, status="rendering", error=None)
        try:
            body, content_type = _multipart_body(
                fields={
                    "prompt": _prompt_for(track.style, bpm=track.bpm),
                    "output_format": "wav",
                    "duration": str(track.duration_seconds),
                    "model": STABLE_AUDIO_MODEL,
                    "steps": str(STABLE_AUDIO_STEPS),
                    "strength": str(track.strength),
                },
                file_field="audio",
                path=clean_path,
            )
            request = urllib.request.Request(
                STABLE_AUDIO_URL,
                data=body,
                method="POST",
                headers={
                    "Authorization": f"Bearer {self.api_key}",
                    "Accept": "audio/*",
                    "Stability-Client-Id": "aero-dsp",
                    "Content-Type": content_type,
                },
            )
            audio = self.transport.generate(request)
            destination_directory = self.tracks_directory / _directory_segment(track.genre) / f"{track.bpm}-bpm"
            destination_directory.mkdir(parents=True, exist_ok=True)
            destination = destination_directory / f"{track.id}.wav"
            temporary = destination.with_suffix(".wav.new")
            temporary.write_bytes(audio)
            os.replace(temporary, destination)
            self._replace(track_id, status="ready", path=str(destination), error=None)
            self._write_metadata(self.job(track_id))
        except BackingTrackError as error:
            self._replace(track_id, status="failed", error=str(error))
            self._write_metadata(self.job(track_id))
        except OSError as error:
            self._replace(track_id, status="failed", error=f"Could not save backing track: {error}")
            self._write_metadata(self.job(track_id))
        except Exception:
            # A worker must never leave an indefinitely spinning UI job if an
            # upstream response changes shape or a local decoder surprises us.
            self._replace(
                track_id,
                status="failed",
                error="Backing-track generation did not complete",
            )
            self._write_metadata(self.job(track_id))

    def _replace(self, track_id: str, **changes: Any) -> None:
        with self._lock:
            track = self._jobs[track_id]
            for name, value in changes.items():
                setattr(track, name, value)

    def _write_metadata(self, track: BackingTrack) -> None:
        destination_directory = self.tracks_directory / _directory_segment(track.genre) / f"{track.bpm}-bpm"
        destination_directory.mkdir(parents=True, exist_ok=True)
        destination = destination_directory / f"{track.id}.json"
        temporary = destination.with_suffix(".json.new")
        temporary.write_text(json.dumps(track.as_payload(), indent=2) + "\n")
        os.replace(temporary, destination)

    def _load_riff(self, riff_id: str) -> dict[str, Any]:
        if not riff_id or "/" in riff_id or "\\" in riff_id:
            raise BackingTrackError("Choose a saved riff before generating a backing track")
        metadata = self.riff_directory / f"{riff_id}.json"
        try:
            payload = json.loads(metadata.read_text())
        except FileNotFoundError as error:
            raise BackingTrackError("That saved riff no longer exists") from error
        except (OSError, json.JSONDecodeError) as error:
            raise BackingTrackError("That saved riff could not be read") from error
        if not isinstance(payload, dict) or payload.get("id") != riff_id:
            raise BackingTrackError("That saved riff is invalid")
        return payload

    def _safe_riff_audio_path(self, value: Any) -> Path:
        if not isinstance(value, str):
            raise BackingTrackError("That saved riff has no clean DI recording")
        path = Path(value).resolve()
        root = self.riff_directory.resolve()
        try:
            path.relative_to(root)
        except ValueError as error:
            raise BackingTrackError("That riff points outside the Riff Vault") from error
        return path


def _stable_error_detail(body: Any) -> str:
    if not isinstance(body, dict):
        return ""
    message = body.get("message")
    if isinstance(message, str) and message:
        return message
    error = body.get("error")
    if isinstance(error, dict) and isinstance(error.get("message"), str):
        return error["message"]
    if isinstance(error, str) and error:
        return error
    errors = body.get("errors")
    if isinstance(errors, list):
        messages = []
        for item in errors:
            if isinstance(item, dict):
                message = item.get("message") or item.get("error")
                if message:
                    messages.append(str(message))
            elif item:
                messages.append(str(item))
        return "; ".join(messages)
    return ""


def _wav_duration_seconds(path: Path) -> float | None:
    try:
        with wave.open(str(path), "rb") as reader:
            rate = reader.getframerate()
            return reader.getnframes() / rate if rate else None
    except (wave.Error, OSError):
        # Python's wave module rejects IEEE-float WAV masters. Inspect only
        # their headers so the same reference-duration checks still apply.
        try:
            with path.open("rb") as stream:
                header = stream.read(12)
                if len(header) != 12 or header[:4] != b"RIFF" or header[8:] != b"WAVE":
                    return None
                limit = min(path.stat().st_size, struct.unpack("<I", header[4:8])[0] + 8)
                rate = block_align = data_size = None
                while stream.tell() + 8 <= limit:
                    chunk, size = struct.unpack("<4sI", stream.read(8))
                    start = stream.tell()
                    if start + size > limit:
                        return None
                    if chunk == b"fmt ":
                        fmt = stream.read(min(size, 40))
                        if len(fmt) < 16:
                            return None
                        tag, channels, rate, _, block_align, bits = struct.unpack("<HHIIHH", fmt[:16])
                        if tag == 0xFFFE and len(fmt) >= 40:
                            tag = struct.unpack("<I", fmt[24:28])[0]
                        if tag not in (1, 3) or not rate or not channels or block_align != channels * ((bits + 7) // 8):
                            return None
                    elif chunk == b"data":
                        data_size = size
                    if rate and block_align and data_size is not None:
                        return (data_size // block_align) / rate
                    stream.seek(start + size + (size & 1))
        except (OSError, ValueError, struct.error):
            pass
        return None


def _safe_genre(value: str) -> str:
    value = " ".join(value.strip().split())
    if not 2 <= len(value) <= 48:
        raise BackingTrackError("Choose a genre between 2 and 48 characters")
    return value


def _directory_segment(value: str) -> str:
    safe = "".join(character.lower() if character.isalnum() else "-" for character in value)
    return "-".join(segment for segment in safe.split("-") if segment) or "unsorted"


def _prompt_for(style: str, *, bpm: int | None = None) -> str:
    return (
        "Instrumental backing track following the pulse and harmonic center of the "
        "supplied clean electric-guitar riff. "
        f"{style}. "
        + (f"Keep a steady {bpm} BPM pulse. " if bpm else "")
        + "Feature supporting drums and bass, leave room for the player's guitar, no vocals, "
        "no lead-guitar melody, and finish so the result can loop cleanly."
    )


def _multipart_body(
    *, fields: dict[str, str], file_field: str, path: Path
) -> tuple[bytes, str]:
    """Build the small multipart request without a deployment-only dependency."""
    boundary = f"----pedal-{secrets.token_hex(16)}"
    chunks: list[bytes] = []
    for name, value in fields.items():
        chunks.extend(
            [
                f"--{boundary}\r\n".encode(),
                f'Content-Disposition: form-data; name="{name}"\r\n\r\n'.encode(),
                value.encode(),
                b"\r\n",
            ]
        )
    mime = mimetypes.guess_type(path.name)[0] or "application/octet-stream"
    chunks.extend(
        [
            f"--{boundary}\r\n".encode(),
            (
                f'Content-Disposition: form-data; name="{file_field}"; '
                f'filename="{path.name}"\r\n'
            ).encode(),
            f"Content-Type: {mime}\r\n\r\n".encode(),
            path.read_bytes(),
            b"\r\n",
            f"--{boundary}--\r\n".encode(),
        ]
    )
    return b"".join(chunks), f"multipart/form-data; boundary={boundary}"
