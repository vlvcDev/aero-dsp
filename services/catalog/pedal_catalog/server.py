from __future__ import annotations

import json
import os
import socketserver
import threading
from dataclasses import asdict
from pathlib import Path
from typing import Any

from .index import list_models, scan_models, upsert_model
from .oauth_callback import OAuthCallbackServer
from .backing_tracks import BackingTrackError, BackingTrackService
from .tone3000 import Tone3000Error
from .tone3000_service import Tone3000Service
from .tone_maker import ToneMakerError, ToneMakerService
from .tone_memory import ToneMemoryError, ToneMemoryService
from .transcription import TranscriptionError, TranscriptionService


API_VERSION = 1


def handle_request(
    message: dict[str, Any],
    models_dir: Path,
    database: Path,
    tone3000: Tone3000Service | None = None,
    tone_maker: ToneMakerService | None = None,
    backing_tracks: BackingTrackService | None = None,
    tone_memory: ToneMemoryService | None = None,
    transcription: TranscriptionService | None = None,
) -> dict:
    request_id = str(message.get("id", "unknown"))
    if message.get("api") != API_VERSION:
        return _error(request_id, "unsupported_api", "Only catalog API v1 is supported")

    message_type = message.get("type")
    payload = message.get("payload") or {}
    if message_type == "list_models":
        query = payload.get("query", "")
        if not isinstance(query, str):
            return _error(request_id, "invalid_payload", "query must be a string")
        models = [asdict(model) for model in list_models(database, query=query)]
        return _response(request_id, "models", {"models": models})
    if message_type == "refresh_models":
        records = scan_models(models_dir, database)
        return _response(request_id, "refreshed", {"count": len(records)})
    if message_type == "tone3000_status":
        status = (
            tone3000.status()
            if tone3000
            else {
                "available": False,
                "connected": False,
                "auth_pending": False,
                "selected_tone_id": None,
            }
        )
        return _response(request_id, "tone3000_status", status)
    if message_type == "tone_maker_status":
        status = (
            tone_maker.status(tone3000)
            if tone_maker
            else {"available": False, "tone3000_connected": False}
        )
        return _response(request_id, "tone_maker_status", status)
    if message_type == "backing_track_status":
        return _response(
            request_id,
            "backing_track_status",
            backing_tracks.status() if backing_tracks else {"available": False},
        )
    if message_type == "tone_memory_list":
        if tone_memory is None:
            return _response(request_id, "tone_memory", {"records": []})
        try:
            records = tone_memory.list_records()
        except ToneMemoryError as error:
            return _error(request_id, "tone_memory_error", str(error))
        return _response(request_id, "tone_memory", {"records": records})
    if tone3000 is None and isinstance(message_type, str) and message_type.startswith("tone3000_"):
        return _error(
            request_id,
            "tone3000_unavailable",
            "TONE3000 is not configured on this device",
        )
    try:
        if isinstance(message_type, str) and message_type.startswith('chords_'):
            from .chords import LIBRARY, ChordError, ai_suggestion, render_preview, suggestions, vary_progression
            try:
                if message_type == 'chords_library':
                    result = {'chords': LIBRARY, 'ai_available': bool(tone_maker and tone_maker.available)}
                elif message_type == 'chords_suggest':
                    key, prompt = payload.get('key', 'C'), payload.get('prompt', '')
                    voicing = payload.get('voicing', 'comfortable')
                    result = {'progressions': [ai_suggestion(tone_maker, key, prompt, voicing)] if payload.get('ai') is True
                              else suggestions(key, prompt, voicing)}
                elif message_type == 'chords_vary':
                    result = vary_progression(payload.get('steps'), payload.get('selected'),
                        payload.get('action'), payload.get('key'), payload.get('attempt', 0),
                        payload.get('voicing', 'comfortable'))
                elif message_type == 'chords_preview':
                    result = render_preview(database.parent / 'chord-preview', payload.get('steps'), payload.get('bpm'))
                elif message_type in ('chords_phrase', 'chords_phrase_preview'):
                    from .phrases import build_phrase, render_phrase
                    if message_type == 'chords_phrase':
                        result = build_phrase(payload.get('steps'), payload.get('key', 'C'), payload.get('mode', 'unspecified'))
                    else:
                        result = render_phrase(database.parent / 'chord-preview', payload.get('steps'),
                            payload.get('bpm'), payload.get('key', 'C'), payload.get('mode', 'unspecified'))
                else:
                    raise ChordError('Unknown chord command')
                return _response(request_id, 'chords', result)
            except (ChordError, OSError) as error:
                return _error(request_id, 'chords_error', str(error))
        if message_type in ("transcription_status", "transcription_create", "transcription_job", "transcription_save"):
            if transcription is None:
                return _error(request_id, "transcription_unavailable", "Restart the catalog to enable transcription")
            if message_type == "transcription_status":
                result = transcription.status()
            elif message_type == "transcription_create":
                result = transcription.queue(payload)
            elif message_type == "transcription_save":
                result = transcription.save(payload)
            else:
                result = transcription.job(payload.get("id"))
            return _response(request_id, "transcription", result)
        if message_type == "backing_track_create":
            riff_id = payload.get("riff_id")
            style = payload.get("style")
            duration_seconds = payload.get("duration_seconds", 20)
            strength = payload.get("strength", 0.65)
            genre = payload.get("genre", "Unsorted")
            bpm = payload.get("bpm", 120)
            if not isinstance(riff_id, str) or not isinstance(style, str):
                return _error(request_id, "invalid_payload", "riff_id and style are required")
            if not isinstance(duration_seconds, int) or not isinstance(strength, (int, float)):
                return _error(request_id, "invalid_payload", "duration and strength are invalid")
            if not isinstance(genre, str) or not isinstance(bpm, int):
                return _error(request_id, "invalid_payload", "genre and BPM are invalid")
            if backing_tracks is None:
                return _error(
                    request_id,
                    "backing_track_unavailable",
                    "Stable Audio is not configured on this device",
                )
            return _response(
                request_id,
                "backing_track",
                backing_tracks.queue(
                    riff_id,
                    style,
                    duration_seconds=duration_seconds,
                    strength=float(strength),
                    genre=genre,
                    bpm=bpm,
                ).as_payload(),
            )
        if message_type == "backing_track_job":
            track_id = payload.get("track_id")
            if not isinstance(track_id, str):
                return _error(request_id, "invalid_payload", "track_id is required")
            if backing_tracks is None:
                return _error(request_id, "backing_track_unavailable", "Stable Audio is not configured on this device")
            return _response(
                request_id,
                "backing_track",
                backing_tracks.job(track_id).as_payload(),
            )
        if message_type == "backing_track_list":
            riff_id = payload.get("riff_id")
            if riff_id is not None and not isinstance(riff_id, str):
                return _error(request_id, "invalid_payload", "riff_id must be a string")
            if backing_tracks is None:
                return _response(request_id, "backing_tracks", {"tracks": []})
            return _response(
                request_id,
                "backing_tracks",
                {"tracks": [track.as_payload() for track in backing_tracks.list_tracks(riff_id)]},
            )
        if message_type == "tone_maker_recommend":
            prompt = payload.get("prompt")
            if not isinstance(prompt, str):
                return _error(request_id, "invalid_payload", "prompt must be a string")
            if tone_maker is None:
                return _error(
                    request_id,
                    "tone_maker_unavailable",
                    "OpenAI is not configured on this device",
                )
            return _response(
                request_id,
                "tone_maker_recommendation",
                tone_maker.recommend(prompt, tone3000).as_payload(),
            )
        if message_type == "tone_maker_apply":
            recommendation_id = payload.get("recommendation_id")
            rig_index = payload.get("rig_index")
            if not isinstance(recommendation_id, str) or not isinstance(rig_index, int):
                return _error(
                    request_id,
                    "invalid_payload",
                    "recommendation_id and rig_index are required",
                )
            if tone_maker is None:
                return _error(
                    request_id,
                    "tone_maker_unavailable",
                    "OpenAI is not configured on this device",
                )
            return _response(
                request_id,
                "tone_maker_applied",
                tone_maker.apply(
                    recommendation_id, rig_index, tone3000, models_dir, database
                ),
            )
        if message_type == "tone_memory_save":
            candidate = payload.get("candidate")
            note = payload.get("note", "")
            if not isinstance(candidate, dict) or not isinstance(note, str):
                return _error(request_id, "invalid_payload", "candidate and note are invalid")
            if tone_memory is None:
                return _error(request_id, "tone_memory_unavailable", "Tone Memory is unavailable")
            return _response(
                request_id,
                "tone_memory_saved",
                {"record": tone_memory.save(candidate, note)},
            )
        if message_type == "tone3000_begin_auth":
            return _response(
                request_id,
                "tone3000_auth",
                {"authorize_url": tone3000.begin_auth()},  # type: ignore[union-attr]
            )
        if message_type == "tone3000_disconnect":
            tone3000.disconnect()  # type: ignore[union-attr]
            return _response(request_id, "tone3000_status", tone3000.status())  # type: ignore[union-attr]
        if message_type == "tone3000_sync_tags":
            from .index import write_capture_metadata
            tone_id = str(payload.get('tone_id', ''))
            records = [m for m in list_models(database)
                       if m.capture_group == f'tone3000:{tone_id}']
            if not records:
                raise Tone3000Error('No local captures for this tone')
            response = tone3000.client.get_tone(tone_id)
            tone3000._sync_tokens()
            summary = tone3000._tone_summary(response.get('data', response))
            for record in records:
                write_capture_metadata(Path(record.path), tone_id, record.capture_id,
                                       record.name, summary['name'], summary['gear'], summary['tags'])
            return _response(request_id, 'tone3000_tags_synced', {'count': len(records)})
        if message_type == "tone3000_selected":
            return _response(
                request_id,
                "tone3000_selected",
                tone3000.selected(),  # type: ignore[union-attr]
            )
        if message_type == "tone3000_download":
            tone_id = payload.get("tone_id")
            model_id = payload.get("model_id")
            if not isinstance(tone_id, (str, int)) or not isinstance(model_id, (str, int)):
                return _error(
                    request_id,
                    "invalid_payload",
                    "tone_id and model_id are required",
                )
            path, _ = tone3000.download(str(tone_id), str(model_id), models_dir)  # type: ignore[union-attr]
            record = upsert_model(path, database, source=f"tone3000:model:{model_id}")
            return _response(request_id, "tone3000_downloaded", {"model": asdict(record)})
    except (Tone3000Error, ToneMakerError) as error:
        return _error(request_id, "tone3000_error", str(error))
    except BackingTrackError as error:
        return _error(request_id, "backing_track_error", str(error))
    except ToneMemoryError as error:
        return _error(request_id, "tone_memory_error", str(error))
    except TranscriptionError as error:
        return _error(request_id, "transcription_error", str(error))
    return _error(request_id, "unknown_message", "Unknown catalog message type")


def _response(request_id: str, message_type: str, payload: dict) -> dict:
    return {"api": API_VERSION, "id": request_id, "type": message_type, "payload": payload}


def _error(request_id: str, code: str, message: str) -> dict:
    return _response(request_id, "error", {"code": code, "message": message})


class _CatalogServer(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True
    allow_reuse_address = True

    def __init__(
        self,
        socket_path: Path,
        models_dir: Path,
        database: Path,
        tone3000: Tone3000Service | None,
        tone_maker: ToneMakerService | None,
        backing_tracks: BackingTrackService | None,
        tone_memory: ToneMemoryService | None,
        tone3000_lock: threading.Lock,
        transcription: TranscriptionService | None = None,
    ) -> None:
        self.models_dir = models_dir
        self.database = database
        self.tone3000 = tone3000
        self.tone_maker = tone_maker
        self.backing_tracks = backing_tracks
        self.tone_memory = tone_memory
        self.transcription = transcription
        self.tone3000_lock = tone3000_lock
        super().__init__(str(socket_path), _CatalogHandler)


class _CatalogHandler(socketserver.StreamRequestHandler):
    def handle(self) -> None:
        server: _CatalogServer = self.server  # type: ignore[assignment]
        for raw_line in self.rfile:
            try:
                message = json.loads(raw_line)
                if not isinstance(message, dict):
                    raise ValueError("request must be a JSON object")
                with server.tone3000_lock:
                    response = handle_request(
                        message,
                        server.models_dir,
                        server.database,
                        server.tone3000,
                        server.tone_maker,
                        server.backing_tracks,
                        server.tone_memory,
                        server.transcription,
                    )
            except (json.JSONDecodeError, UnicodeDecodeError, ValueError) as error:
                response = _error("unknown", "invalid_json", str(error))
            self.wfile.write(json.dumps(response, separators=(",", ":")).encode() + b"\n")
            self.wfile.flush()


def serve(
    models_dir: Path,
    database: Path,
    socket_path: Path,
    tone3000: Tone3000Service | None = None,
    tone_maker: ToneMakerService | None = None,
    backing_tracks: BackingTrackService | None = None,
    tone_memory: ToneMemoryService | None = None,
    transcription: TranscriptionService | None = None,
) -> None:
    models_dir.mkdir(parents=True, exist_ok=True)
    database.parent.mkdir(parents=True, exist_ok=True)
    socket_path.parent.mkdir(parents=True, exist_ok=True)
    scan_models(models_dir, database)
    socket_path.unlink(missing_ok=True)
    tone3000_lock = threading.Lock()
    callback = (
        OAuthCallbackServer(tone3000.redirect_uri, tone3000, tone3000_lock)
        if tone3000 is not None
        else None
    )
    try:
        if callback is not None:
            callback.start()
            print(
                f"TONE3000 callback listening on port {callback.address[1]}",
                flush=True,
            )
        with _CatalogServer(
            socket_path,
            models_dir,
            database,
            tone3000,
            tone_maker,
            backing_tracks,
            tone_memory,
            tone3000_lock,
            transcription,
        ) as server:
            os.chmod(socket_path, 0o660)
            print(f"pedal-catalog listening on {socket_path}", flush=True)
            try:
                server.serve_forever()
            except KeyboardInterrupt:
                pass
    finally:
        if callback is not None:
            callback.close()
        socket_path.unlink(missing_ok=True)
