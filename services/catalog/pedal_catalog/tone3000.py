from __future__ import annotations

import base64
import hashlib
import json
import os
import secrets
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Mapping, Protocol


DEFAULT_BASE_URL = "https://www.tone3000.com"
SUPPORTED_ARCHITECTURE = 2
SUPPORTED_FORMAT = "nam"
IR_FORMAT = "ir"


class Tone3000Error(RuntimeError):
    """Raised when TONE3000 rejects a request or returns invalid data."""


@dataclass(frozen=True)
class PkceSession:
    code_verifier: str
    code_challenge: str
    state: str


@dataclass(frozen=True)
class Tokens:
    access_token: str
    refresh_token: str
    expires_at: float


@dataclass(frozen=True)
class HttpResponse:
    status: int
    body: bytes
    headers: Mapping[str, str]


class Transport(Protocol):
    def send(self, request: urllib.request.Request) -> HttpResponse: ...


class _SafeRedirectHandler(urllib.request.HTTPRedirectHandler):
    """Follow signed-download redirects without leaking the bearer token."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        redirected = super().redirect_request(req, fp, code, msg, headers, newurl)
        if redirected is None:
            return None
        old_origin = urllib.parse.urlsplit(req.full_url)[:2]
        new_origin = urllib.parse.urlsplit(newurl)[:2]
        if old_origin != new_origin:
            redirected.remove_header("Authorization")
        return redirected


class UrllibTransport:
    def __init__(self, timeout_seconds: float = 30.0) -> None:
        self.timeout_seconds = timeout_seconds
        self._opener = urllib.request.build_opener(_SafeRedirectHandler())

    def send(self, request: urllib.request.Request) -> HttpResponse:
        try:
            with self._opener.open(request, timeout=self.timeout_seconds) as response:
                return HttpResponse(response.status, response.read(), dict(response.headers.items()))
        except urllib.error.HTTPError as error:
            return HttpResponse(error.code, error.read(), dict(error.headers.items()))


def _base64url(value: bytes) -> str:
    return base64.urlsafe_b64encode(value).rstrip(b"=").decode("ascii")


def create_pkce_session(
    random_bytes: Callable[[int], bytes] = secrets.token_bytes,
) -> PkceSession:
    verifier = _base64url(random_bytes(32))
    challenge = _base64url(hashlib.sha256(verifier.encode("ascii")).digest())
    return PkceSession(
        code_verifier=verifier,
        code_challenge=challenge,
        state=_base64url(random_bytes(16)),
    )


def build_select_authorize_url(
    publishable_key: str,
    redirect_uri: str,
    session: PkceSession,
    *,
    gears: tuple[str, ...] = (),
    preview: bool = True,
    base_url: str = DEFAULT_BASE_URL,
) -> str:
    """Build an A2 NAM-only Select Flow URL from a previously saved PKCE session."""
    query = {
        "client_id": publishable_key,
        "redirect_uri": redirect_uri,
        "response_type": "code",
        "code_challenge": session.code_challenge,
        "code_challenge_method": "S256",
        "state": session.state,
        "prompt": "select_tone",
        "format": SUPPORTED_FORMAT,
        "architecture": str(SUPPORTED_ARCHITECTURE),
    }
    if gears:
        query["gears"] = "_".join(gears)
    if preview:
        query["preview"] = "true"
    return f"{base_url.rstrip('/')}/api/v1/oauth/authorize?{urllib.parse.urlencode(query)}"


class Tone3000Client:
    """Small, testable TONE3000 client restricted to NAM architecture A2."""

    def __init__(
        self,
        publishable_key: str,
        *,
        tokens: Tokens | None = None,
        base_url: str = DEFAULT_BASE_URL,
        transport: Transport | None = None,
        clock: Callable[[], float] = time.time,
    ) -> None:
        self.publishable_key = publishable_key
        self.tokens = tokens
        self.base_url = base_url.rstrip("/")
        self.transport = transport or UrllibTransport()
        self.clock = clock

    def exchange_code(self, code: str, verifier: str, redirect_uri: str) -> Tokens:
        return self._token_request(
            {
                "grant_type": "authorization_code",
                "code": code,
                "code_verifier": verifier,
                "redirect_uri": redirect_uri,
                "client_id": self.publishable_key,
            }
        )

    def exchange_callback(
        self,
        code: str,
        returned_state: str,
        session: PkceSession,
        redirect_uri: str,
    ) -> Tokens:
        if not secrets.compare_digest(returned_state, session.state):
            raise Tone3000Error("OAuth callback state mismatch")
        return self.exchange_code(code, session.code_verifier, redirect_uri)

    def refresh(self) -> Tokens:
        if self.tokens is None:
            raise Tone3000Error("TONE3000 account is not connected")
        return self._token_request(
            {
                "grant_type": "refresh_token",
                "refresh_token": self.tokens.refresh_token,
                "client_id": self.publishable_key,
            }
        )

    def get_tone(
        self,
        tone_id: int | str,
        *,
        architecture: int = SUPPORTED_ARCHITECTURE,
    ) -> dict:
        return self._get_json(
            f"/api/v1/tones/{tone_id}", {"architecture": architecture}
        )

    def list_models(
        self,
        tone_id: int | str,
        *,
        page: int = 1,
        page_size: int = 20,
        architecture: int | None = SUPPORTED_ARCHITECTURE,
    ) -> dict:
        parameters: dict[str, int | str] = {
            "tone_id": tone_id,
            "page": page,
            "page_size": page_size,
        }
        if architecture is not None:
            parameters["architecture"] = architecture
        return self._get_json(
            "/api/v1/models",
            parameters,
        )

    def search_tones(
        self,
        query: str,
        *,
        gears: tuple[str, ...],
        page_size: int = 5,
        format: str = SUPPORTED_FORMAT,
        architecture: int | None = SUPPORTED_ARCHITECTURE,
    ) -> dict:
        """Search a small compatible candidate set for a user request.

        TONE3000 rate-limits this endpoint. Callers should use it only for an
        explicit tone-making request, never as a background catalog refresh.
        """
        if not query.strip():
            raise Tone3000Error("TONE3000 search query cannot be empty")
        if format not in {SUPPORTED_FORMAT, IR_FORMAT}:
            raise Tone3000Error("Unsupported TONE3000 search format")
        parameters: dict[str, int | str] = {
            "query": query.strip()[:160],
            "page": 1,
            "page_size": max(1, min(page_size, 10)),
            "gears": "_".join(gears),
            "format": format,
        }
        if architecture is not None:
            parameters["architecture"] = architecture
        return self._get_json(
            "/api/v1/tones/search",
            parameters,
        )

    def download_model(
        self,
        model_url: str,
        destination: Path,
        *,
        expected_sha256: str | None = None,
    ) -> str:
        """Download with bearer auth, hash it, then atomically place it in the library."""
        model_uri = urllib.parse.urlsplit(model_url)
        base_uri = urllib.parse.urlsplit(self.base_url)
        if (model_uri.scheme, model_uri.netloc) != (base_uri.scheme, base_uri.netloc):
            raise Tone3000Error("Refusing to send credentials to a non-TONE3000 URL")

        response = self._send("GET", model_url)
        self._require_success(response, "model download")
        actual_sha256 = hashlib.sha256(response.body).hexdigest()
        if expected_sha256 and not secrets.compare_digest(
            actual_sha256.lower(), expected_sha256.lower()
        ):
            raise Tone3000Error(
                f"Downloaded model checksum mismatch: expected {expected_sha256}, got {actual_sha256}"
            )

        destination = destination.resolve()
        destination.parent.mkdir(parents=True, exist_ok=True)
        temporary = destination.with_name(f".{destination.name}.{secrets.token_hex(8)}.part")
        try:
            temporary.write_bytes(response.body)
            os.replace(temporary, destination)
        finally:
            temporary.unlink(missing_ok=True)
        return actual_sha256

    def _token_request(self, values: Mapping[str, str]) -> Tokens:
        body = urllib.parse.urlencode(values).encode("ascii")
        request = urllib.request.Request(
            f"{self.base_url}/api/v1/oauth/token",
            data=body,
            headers={"Content-Type": "application/x-www-form-urlencoded"},
            method="POST",
        )
        response = self.transport.send(request)
        self._require_success(response, "token request")
        payload = self._decode_json(response, "token request")
        try:
            tokens = Tokens(
                access_token=str(payload["access_token"]),
                refresh_token=str(payload["refresh_token"]),
                expires_at=self.clock() + float(payload["expires_in"]),
            )
        except (KeyError, TypeError, ValueError) as error:
            raise Tone3000Error("Token response is missing required fields") from error
        self.tokens = tokens
        return tokens

    def _get_json(self, path: str, params: Mapping[str, str | int] | None = None) -> dict:
        url = f"{self.base_url}{path}"
        if params:
            url = f"{url}?{urllib.parse.urlencode(params)}"
        response = self._send("GET", url)
        self._require_success(response, path)
        payload = self._decode_json(response, path)
        if not isinstance(payload, dict):
            raise Tone3000Error(f"{path} returned a non-object JSON response")
        return payload

    def _send(self, method: str, url: str) -> HttpResponse:
        if self.tokens is None:
            raise Tone3000Error("TONE3000 account is not connected")
        if self.clock() > self.tokens.expires_at - 60:
            self.refresh()
        request = urllib.request.Request(
            url,
            headers={"Authorization": f"Bearer {self.tokens.access_token}"},
            method=method,
        )
        response = self.transport.send(request)
        if response.status == 401:
            self.refresh()
            request = urllib.request.Request(
                url,
                headers={"Authorization": f"Bearer {self.tokens.access_token}"},
                method=method,
            )
            response = self.transport.send(request)
        return response

    @staticmethod
    def _require_success(response: HttpResponse, operation: str) -> None:
        if 200 <= response.status < 300:
            return
        detail = ""
        try:
            payload = json.loads(response.body)
            detail = str(payload.get("error") or payload.get("message") or "")
        except (json.JSONDecodeError, UnicodeDecodeError, AttributeError):
            pass
        suffix = f": {detail}" if detail else ""
        raise Tone3000Error(f"TONE3000 {operation} failed ({response.status}){suffix}")

    @staticmethod
    def _decode_json(response: HttpResponse, operation: str) -> dict:
        try:
            return json.loads(response.body)
        except (json.JSONDecodeError, UnicodeDecodeError) as error:
            raise Tone3000Error(f"TONE3000 {operation} returned invalid JSON") from error
