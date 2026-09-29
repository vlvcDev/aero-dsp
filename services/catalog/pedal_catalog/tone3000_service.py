from __future__ import annotations

import json
import os
import re
import secrets
import urllib.parse
from pathlib import Path
from typing import Any

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

    @property
    def connected(self) -> bool:
        return self.client.tokens is not None

    def status(self) -> dict[str, Any]:
        return {
            "available": True,
            "connected": self.connected,
            "auth_pending": self._session is not None,
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
        return {"tone": self._tone_summary(tone), "models": self.models(tone_id)}

    def models(self, tone_id: str, *, format: str = "nam") -> list[dict[str, Any]]:
        # Keep the established A2 filter for NAMs. IRs are WAV captures rather
        # than neural models, so they must not be filtered by NAM architecture.
        if format == "nam":
            payload = self.client.list_models(tone_id)
        elif format == IR_FORMAT:
            payload = self.client.list_models(tone_id, architecture=None)
        else:
            raise Tone3000Error("Unsupported TONE3000 model format")
        self._sync_tokens()
        return [self._model_summary(item, tone_id) for item in self._items(payload)]

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
        return [self._tone_summary(item) for item in self._items(payload)]

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
        model = next(
            (
                item
                for item in self.models(tone_id, format=format)
                if item["id"] == str(model_id)
            ),
            None,
        )
        if model is None:
            raise Tone3000Error("Selected TONE3000 model is no longer available")
        safe_name = re.sub(r"[^A-Za-z0-9._-]+", "-", model["name"]).strip("-.")
        safe_name = (safe_name or f"tone3000-{model_id}")[:80]
        safe_id = re.sub(r"[^A-Za-z0-9_-]+", "-", str(model_id)).strip("-")
        safe_id = (safe_id or "model")[:40]
        extension = ".wav" if format == IR_FORMAT else ".nam"
        destination = models_dir / f"{safe_name}-{safe_id}{extension}"
        sha256 = self.client.download_model(
            model["download_url"],
            destination,
            expected_sha256=model.get("sha256"),
        )
        self._sync_tokens()
        self._selected_tone_id = None
        return destination, sha256

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
