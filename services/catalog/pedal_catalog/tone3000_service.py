from __future__ import annotations

import json
import os
import re
import secrets
import threading
import time
import urllib.parse
from pathlib import Path
from typing import Any, Callable

from .tone3000 import (
    DEFAULT_BASE_URL,
    IR_FORMAT,
    PkceSession,
    Tone3000Client,
    Tone3000Error,
    Tokens,
    build_select_authorize_url,
    create_pkce_session,
)


class Tone3000Service:
    """Stateful catalog-side owner for OAuth, discovery, and local downloads."""

    def __init__(
        self,
        publishable_key: str,
        redirect_uri: str,
        token_file: Path,
        *,
        base_url: str = DEFAULT_BASE_URL,
        client: Tone3000Client | None = None,
        sleep: Callable[[float], None] = time.sleep,
    ) -> None:
        self.publishable_key = publishable_key
        self.redirect_uri = redirect_uri
        self.token_file = token_file
        self.base_url = base_url.rstrip("/")
        self.client = client or Tone3000Client(
            publishable_key,
            tokens=self._load_tokens(),
            base_url=self.base_url,
        )
        self._session: PkceSession | None = None
        self._authorize_url: str | None = None
        self._selected_tone_id: str | None = None
        self._capture_group_names: dict[str, str] = {}
        self._capture_gears: dict[str, str] = {}
        self._capture_tags: dict[str, list[str]] = {}
        self._device: dict[str, Any] | None = None
        self._sleep = sleep
        self._device_thread: threading.Thread | None = None

    @property
    def connected(self) -> bool:
        return self.client.tokens is not None

    def status(self) -> dict[str, Any]:
        return {
            "available": True,
            "connected": self.connected,
            "auth_pending": self._session is not None,
            "device_auth_pending": self._device is not None,
            "selected_tone_id": self._selected_tone_id,
        }

    def begin_auth(self) -> str:
        self._session = create_pkce_session()
        self._authorize_url = build_select_authorize_url(
            self.publishable_key,
            self.redirect_uri,
            self._session,
            gears=("amp", "pedal"),
            base_url=self.base_url,
        )
        parsed = urllib.parse.urlsplit(self.redirect_uri)
        return urllib.parse.urlunsplit((parsed.scheme, parsed.netloc, "/connect", "", ""))

    @property
    def pending_authorize_url(self) -> str | None:
        return self._authorize_url

    def complete_auth(self, code: str, state: str, tone_id: str | None) -> None:
        if self._session is None:
            raise Tone3000Error("No TONE3000 login is waiting for a callback")
        tokens = self.client.exchange_callback(code, state, self._session, self.redirect_uri)
        self._session = None
        self._authorize_url = None
        self._selected_tone_id = str(tone_id) if tone_id else None
        self._save_tokens(tokens)

    def cancel_auth(self, state: str) -> None:
        if self._session is None:
            raise Tone3000Error("No TONE3000 selection is waiting for a callback")
        if not secrets.compare_digest(state, self._session.state):
            raise Tone3000Error("OAuth callback state mismatch")
        self._session = None
        self._authorize_url = None

    def disconnect(self) -> None:
        self.client.tokens = None
        self._session = None
        self._authorize_url = None
        self._selected_tone_id = None
        self._device = None
        self.token_file.unlink(missing_ok=True)

    def selected(self) -> dict[str, Any]:
        tone_id = self._selected_tone_id
        if tone_id is None:
            raise Tone3000Error("No TONE3000 tone has been selected")
        payload = self.client.get_tone(tone_id)
        self._sync_tokens()
        tone = payload.get("data", payload)
        if not isinstance(tone, dict):
            raise Tone3000Error("TONE3000 returned invalid tone details")
        summary = self._tone_summary(tone)
        self._capture_group_names = getattr(self, '_capture_group_names', {})
        self._capture_group_names[str(tone_id)] = summary['name']
        self._capture_gears = getattr(self, '_capture_gears', {})
        self._capture_gears[str(tone_id)] = summary['gear']
        self._capture_tags = getattr(self, '_capture_tags', {})
        self._capture_tags[str(tone_id)] = summary['tags']
        return {"tone": summary, "models": self.models(tone_id)}

    def models(self, tone_id: str, *, format: str = "nam") -> list[dict[str, Any]]:
        # Keep the established A2 filter for NAMs. IRs are WAV captures rather
        # than neural models, so they must not be filtered by NAM architecture.
        if format == "nam":
            payload = self.client.list_models(tone_id)
        elif format == IR_FORMAT:
            payload = self.client.list_models(tone_id, architecture=None)
        else:
            raise Tone3000Error("Unsupported TONE3000 model format")
        items = self._items(payload)
        page_items = items
        page = 1
        seen = {str(item.get('id')) for item in items}
        while len(page_items) >= 20:
            page += 1
            if page > 100:
                raise Tone3000Error('Capture collection is too large to list safely')
            options = {'page': page}
            if format == IR_FORMAT:
                options['architecture'] = None
            page_items = self._items(self.client.list_models(tone_id, **options))
            fresh = [item for item in page_items if str(item.get('id')) not in seen]
            if page_items and not fresh:
                raise Tone3000Error('TONE3000 repeated a capture page; please retry')
            items.extend(fresh)
            seen.update(str(item.get('id')) for item in fresh)
        self._sync_tokens()
        return [self._model_summary(item, tone_id) for item in items]

    def search(self, query: str, *, role: str) -> list[dict[str, Any]]:
        """Return a deliberately small, compatible candidate list for AI tone making."""
        if role == "pedal":
            gears = ("pedal",)
            format = "nam"
            architecture: int | None = 2
        elif role == "amp":
            gears = ("amp", "amp-cab")
            format = "nam"
            architecture = 2
        elif role == "cabinet":
            # TONE3000's API enum calls this `cab`, while the human-facing
            # label is “Cabinet IR”. Using `cabinet` silently returns no hits.
            gears = ("cab",)
            format = IR_FORMAT
            architecture = None
        else:
            raise Tone3000Error("Unsupported AI tone search role")
        payload = self.client.search_tones(
            query,
            gears=gears,
            page_size=5,
            format=format,
            architecture=architecture,
        )
        self._sync_tokens()
        summaries = [self._tone_summary(item) for item in self._items(payload)]
        self._capture_group_names = getattr(self, '_capture_group_names', {})
        self._capture_gears = getattr(self, '_capture_gears', {})
        for summary in summaries:
            self._capture_group_names[summary['id']] = summary['name']
            self._capture_gears[summary['id']] = summary['gear']
            self._capture_tags = getattr(self, '_capture_tags', {})
            self._capture_tags[summary['id']] = summary['tags']
        return summaries

    def download_best_model(
        self,
        tone_id: str,
        models_dir: Path,
    ) -> tuple[Path, str]:
        """Install the first TONE3000-sorted A2 model for an accessible tone."""
        models = self.models(tone_id)
        if not models:
            raise Tone3000Error("TONE3000 tone has no compatible NAM A2 models")
        return self.download(tone_id, models[0]["id"], models_dir)

    def download_best_ir(self, tone_id: str, irs_dir: Path) -> tuple[Path, str]:
        """Install the first catalog-sorted cabinet IR as a WAV file."""
        models = self.models(tone_id, format=IR_FORMAT)
        if not models:
            raise Tone3000Error("TONE3000 cabinet has no downloadable IR files")
        return self.download(tone_id, models[0]["id"], irs_dir, format=IR_FORMAT)

    def download(
        self,
        tone_id: str,
        model_id: str,
        models_dir: Path,
        *,
        format: str = "nam",
    ) -> tuple[Path, str]:
        available = self.models(tone_id, format=format)
        if format == 'nam':
            # Adopt earlier downloads made with the same deterministic filename.
            # Never move files: existing pedalboard presets reference their paths.
            from .index import write_capture_metadata
            for capture in available:
                name = re.sub(r"[^A-Za-z0-9._-]+", "-", capture['name']).strip('-.')
                name = (name or f"tone3000-{capture['id']}")[:80]
                identifier = re.sub(r"[^A-Za-z0-9_-]+", "-", str(capture['id'])).strip('-')
                identifier = (identifier or 'model')[:40]
                existing = models_dir / f'{name}-{identifier}.nam'
                if existing.is_file():
                    write_capture_metadata(existing, str(tone_id), str(capture['id']), capture['name'],
                                           getattr(self, '_capture_group_names', {}).get(str(tone_id)),
                                           getattr(self, '_capture_gears', {}).get(str(tone_id)),
                                           getattr(self, '_capture_tags', {}).get(str(tone_id)))
        model = next(
            (
                item
                for item in available
                if item["id"] == str(model_id)
            ),
            None,
        )
        if model is None:
            raise Tone3000Error("Selected TONE3000 model is no longer available")
        destination = self._destination(model["name"], model_id, models_dir, format)
        sha256 = self.client.download_model(
            model["download_url"],
            destination,
            expected_sha256=model.get("sha256"),
        )
        self._write_sidecar(destination, tone_id, model_id, model["name"])
        self._sync_tokens()
        self._selected_tone_id = None
        return destination, sha256

    # ---- Tone Browser -------------------------------------------------

    _SORTS = {"best-match", "trending", "downloads-all-time", "newest", "oldest"}
    _STREAMS = {"downloaded", "favorited"}

    def browse(self, payload: dict[str, Any], local_paths: dict[str, str]) -> dict[str, Any]:
        sort = payload.get("sort")
        if sort is not None and sort not in self._SORTS:
            raise Tone3000Error("Unsupported TONE3000 sort order")
        format = payload.get("format")
        if format not in (None, "nam", IR_FORMAT):
            raise Tone3000Error("Unsupported TONE3000 format")
        page, page_size = self._paging(payload)
        result = self.client.browse_tones(
            query=self._text(payload, "query"),
            gears=self._text(payload, "gear"),
            sort=sort,
            format=format,
            tags=self._names(payload, "tags"),
            makes=self._names(payload, "makes"),
            verified=payload.get("verified") is True,
            calibrated=payload.get("calibrated") is True,
            page=page,
            page_size=page_size,
        )
        return self._card_page(result, local_paths)

    def stream(self, payload: dict[str, Any], local_paths: dict[str, str]) -> dict[str, Any]:
        stream = payload.get("stream")
        if stream not in self._STREAMS:
            raise Tone3000Error("Unsupported TONE3000 stream")
        page, page_size = self._paging(payload)
        result = self.client.list_tone_stream(
            stream,
            query=self._text(payload, "query"),
            gear=self._text(payload, "gear"),
            page=page,
            page_size=page_size,
        )
        return self._card_page(result, local_paths, favorited=True if stream == "favorited" else None)

    def trending(self, gear: str, local_paths: dict[str, str]) -> dict[str, Any]:
        result = self.client.trending_tones(gear)
        self._sync_tokens()
        return {"tones": [self.tone_card(item, local_paths) for item in self._items(result)]}

    def set_favorite(self, tone_id: str, favorite: bool) -> bool:
        self.client.set_favorite(tone_id, favorite)
        self._sync_tokens()
        return favorite

    def tone_models(self, tone_id: str, models_dir: Path) -> list[dict[str, Any]]:
        format, directory = self._tone_target(tone_id, models_dir)
        return [
            {"id": model["id"], "name": model["name"], "local_path": self._local_file(model, directory, format)}
            for model in self.models(tone_id, format=format)
        ]

    def load(self, tone_id: str, model_id: str | None, models_dir: Path) -> tuple[Path, str]:
        """Make one model of a tone available locally; download only if missing."""
        format, directory = self._tone_target(tone_id, models_dir)
        models = self.models(tone_id, format=format)
        model = next((m for m in models if model_id is None or m["id"] == str(model_id)), None)
        if model is None:
            raise Tone3000Error("TONE3000 tone has no matching downloadable model")
        existing = self._local_file(model, directory, format)
        if existing is None:
            return self.download(tone_id, model["id"], directory, format=format)[0], format
        path = Path(existing)
        self._write_sidecar(path, tone_id, model["id"], model["name"])
        return path, format

    @staticmethod
    def tone_card(item: dict[str, Any], local_paths: dict[str, str], favorited: bool | None = None) -> dict[str, Any]:
        tone_id = item.get("id")
        if tone_id is None:
            raise Tone3000Error("TONE3000 tone result is missing an id")
        format = IR_FORMAT if item.get("format") == IR_FORMAT else "nam"
        images = item.get("images")
        user = item.get("user") if isinstance(item.get("user"), dict) else {}
        a2_count = item.get("a2_models_count")
        downloads = item.get("downloads_count")
        favorite = item.get("is_favorite")
        return {
            "id": str(tone_id),
            "title": str(item.get("title") or item.get("name") or f"Tone {tone_id}"),
            "gear": item.get("gear") if isinstance(item.get("gear"), str) else None,
            "format": format,
            "image_url": next((i for i in images if isinstance(i, str)), None) if isinstance(images, list) else None,
            "downloads": downloads if isinstance(downloads, int) else None,
            "creator": {
                "name": user.get("display_name") or user.get("username") or None,
                "avatar_url": user.get("avatar_url") or None,
                "verified": user.get("is_verified") is True,
            },
            "created_at": item.get("published_at") or item.get("created_at"),
            "has_a2": format == IR_FORMAT or not isinstance(a2_count, int) or a2_count > 0,
            "favorited": favorite if isinstance(favorite, bool) else favorited,
            "local_path": local_paths.get(str(tone_id)),
        }

    def _card_page(self, result: dict[str, Any], local_paths: dict[str, str], favorited: bool | None = None) -> dict[str, Any]:
        self._sync_tokens()
        total_pages, total = result.get("total_pages"), result.get("total")
        return {
            "tones": [self.tone_card(item, local_paths, favorited) for item in self._items(result)],
            "page": result.get("page") if isinstance(result.get("page"), int) else 1,
            "total_pages": total_pages if isinstance(total_pages, int) else None,
            "total": total if isinstance(total, int) else None,
        }

    @staticmethod
    def _text(payload: dict[str, Any], key: str) -> str:
        value = payload.get(key) or ""
        if not isinstance(value, str):
            raise Tone3000Error(f"{key} must be a string")
        return value

    @staticmethod
    def _names(payload: dict[str, Any], key: str) -> tuple[str, ...]:
        value = payload.get(key) or []
        if not isinstance(value, list) or not all(isinstance(v, str) for v in value):
            raise Tone3000Error(f"{key} must be a list of strings")
        return tuple(value)

    @staticmethod
    def _paging(payload: dict[str, Any]) -> tuple[int, int]:
        page, size = payload.get("page", 1), payload.get("page_size", 12)
        if not isinstance(page, int) or not isinstance(size, int):
            raise Tone3000Error("page and page_size must be integers")
        return max(1, page), max(1, min(size, 50))

    def _tone_target(self, tone_id: str, models_dir: Path) -> tuple[str, Path]:
        """Resolve a tone's format and the directory its files install into."""
        payload = self.client.get_tone(tone_id)
        tone = payload.get("data", payload)
        if not isinstance(tone, dict):
            raise Tone3000Error("TONE3000 returned invalid tone details")
        summary = self._tone_summary(tone)
        self._capture_group_names[summary["id"]] = summary["name"]
        self._capture_gears[summary["id"]] = summary["gear"]
        self._capture_tags[summary["id"]] = summary["tags"]
        if tone.get("format") == IR_FORMAT:
            return IR_FORMAT, models_dir.parent / "irs"
        return "nam", models_dir

    def _local_file(self, model: dict[str, Any], directory: Path, format: str) -> str | None:
        path = self._destination(model["name"], model["id"], directory, format)
        return str(path) if path.is_file() else None

    @staticmethod
    def _destination(name: str, model_id: str, directory: Path, format: str) -> Path:
        safe_name = re.sub(r"[^A-Za-z0-9._-]+", "-", name).strip("-.")
        safe_name = (safe_name or f"tone3000-{model_id}")[:80]
        safe_id = re.sub(r"[^A-Za-z0-9_-]+", "-", str(model_id)).strip("-")
        safe_id = (safe_id or "model")[:40]
        return directory / f"{safe_name}-{safe_id}{'.wav' if format == IR_FORMAT else '.nam'}"

    def _write_sidecar(self, path: Path, tone_id: str, model_id: str, name: str) -> None:
        from .index import write_capture_metadata
        write_capture_metadata(path, str(tone_id), str(model_id), name,
                               getattr(self, '_capture_group_names', {}).get(str(tone_id)),
                               getattr(self, '_capture_gears', {}).get(str(tone_id)),
                               getattr(self, '_capture_tags', {}).get(str(tone_id)))

    # ---- Device-code sign-in ------------------------------------------

    def begin_device_auth(self) -> dict[str, Any]:
        grant = self.client.request_device_authorization()
        try:
            device = {
                "device_code": str(grant["device_code"]),
                "user_code": str(grant["user_code"]),
                "verification_uri": str(grant["verification_uri"]),
                "verification_uri_complete": grant.get("verification_uri_complete") or None,
                "expires_in": int(grant.get("expires_in", 600)),
                "interval": max(1, int(grant.get("interval", 5))),
            }
        except (KeyError, TypeError, ValueError) as error:
            raise Tone3000Error("Device authorization response is missing required fields") from error
        self._device = device
        self._device_thread = threading.Thread(
            target=self._poll_device, args=(device,), name="tone3000-device-auth", daemon=True
        )
        self._device_thread.start()
        return {key: device[key] for key in
                ("user_code", "verification_uri", "verification_uri_complete", "expires_in", "interval")}

    def _poll_device(self, device: dict[str, Any]) -> None:
        interval = device["interval"]
        deadline = time.monotonic() + device["expires_in"]
        try:
            while self._device is device and time.monotonic() < deadline:
                self._sleep(interval)
                if self._device is not device:
                    return
                try:
                    self._save_tokens(self.client.poll_device_token(device["device_code"]))
                    return
                except Tone3000Error as error:
                    if error.code == "slow_down":
                        interval += 5
                    elif error.code not in (None, "authorization_pending"):
                        return  # expired_token / access_denied: the user must start over
        finally:
            if self._device is device:
                self._device = None

    def _load_tokens(self) -> Tokens | None:
        try:
            payload = json.loads(self.token_file.read_text())
            return Tokens(
                access_token=str(payload["access_token"]),
                refresh_token=str(payload["refresh_token"]),
                expires_at=float(payload["expires_at"]),
            )
        except FileNotFoundError:
            return None
        except (json.JSONDecodeError, KeyError, TypeError, ValueError):
            # A damaged credential file must never prevent the offline catalog
            # from starting. The user can simply reconnect the account.
            return None

    def _sync_tokens(self) -> None:
        if self.client.tokens is not None:
            self._save_tokens(self.client.tokens)

    def _save_tokens(self, tokens: Tokens) -> None:
        self.token_file.parent.mkdir(parents=True, exist_ok=True)
        temporary = self.token_file.with_suffix(".json.new")
        temporary.write_text(
            json.dumps(
                {
                    "access_token": tokens.access_token,
                    "refresh_token": tokens.refresh_token,
                    "expires_at": tokens.expires_at,
                },
                separators=(",", ":"),
            )
        )
        os.chmod(temporary, 0o600)
        os.replace(temporary, self.token_file)

    @staticmethod
    def _items(payload: dict[str, Any]) -> list[dict[str, Any]]:
        items = payload.get("data", payload.get("results", []))
        if not isinstance(items, list):
            raise Tone3000Error("TONE3000 returned an invalid result list")
        return [item for item in items if isinstance(item, dict)]

    @staticmethod
    def _tone_summary(item: dict[str, Any]) -> dict[str, Any]:
        from .index import normalize_tags
        tone_id = item.get("id")
        if tone_id is None:
            raise Tone3000Error("TONE3000 tone result is missing an id")
        owner = item.get("user") or item.get("owner") or {}
        author = (
            owner.get("display_name") or owner.get("username") or owner.get("name")
            if isinstance(owner, dict)
            else owner
        )
        return {
            "id": str(tone_id),
            "name": str(item.get("name") or item.get("title") or f"Tone {tone_id}"),
            "author": str(author or "TONE3000"),
            "gear": str(item.get("gear") or item.get("type") or "NAM"),
            "tags": list(normalize_tags(item.get("tags"))),
        }

    def _model_summary(self, item: dict[str, Any], tone_id: str) -> dict[str, Any]:
        model_id = item.get("id")
        if model_id is None:
            raise Tone3000Error("TONE3000 model result is missing an id")
        download_url = item.get("model_url") or item.get("download_url") or item.get("url")
        if not download_url:
            raise Tone3000Error("TONE3000 model result is missing model_url")
        return {
            "id": str(model_id),
            "tone_id": str(tone_id),
            "name": str(item.get("name") or item.get("title") or f"Model {model_id}"),
            "download_url": str(download_url),
            "sha256": item.get("sha256") or item.get("checksum_sha256"),
        }
