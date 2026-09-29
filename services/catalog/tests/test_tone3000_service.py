import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

CATALOG_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(CATALOG_ROOT))

from pedal_catalog.tone3000 import PkceSession, Tokens  # noqa: E402
from pedal_catalog.tone3000_service import Tone3000Service  # noqa: E402


class FakeClient:
    def __init__(self) -> None:
        self.tokens = None
        self.downloaded = None
        self.list_requests = []
        self.search_requests = []

    def exchange_callback(self, code, state, session, redirect_uri):
        if state != session.state:
            raise AssertionError("unexpected state")
        self.tokens = Tokens(f"access-{code}", "refresh", 1234)
        return self.tokens

    def get_tone(self, tone_id):
        self.tokens = Tokens("refreshed", "refresh-2", 5678)
        return {
            "id": int(tone_id),
            "title": "Clean Combo",
            "user": {"username": "Ada"},
            "gear": "amp",
        }

    def list_models(self, tone_id, *, architecture=2):
        self.list_requests.append((tone_id, architecture))
        if architecture is None:
            return {
                "results": [
                    {
                        "id": 12,
                        "name": "4x12 Greenback",
                        "model_url": "https://www.tone3000.com/models/greenback.wav",
                        "sha256": "def",
                    }
                ]
            }
        return {
            "results": [
                {
                    "id": 9,
                    "name": "Clean / Bright",
                    "model_url": "https://www.tone3000.com/models/clean-bright.nam",
                    "sha256": "abc",
                }
            ]
        }

    def download_model(self, url, destination, *, expected_sha256=None):
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(b"wav" if destination.suffix == ".wav" else b"nam")
        self.downloaded = (url, destination, expected_sha256)
        return "download-hash"

    def search_tones(self, query, **kwargs):
        self.search_requests.append((query, kwargs))
        return {"data": []}


class Tone3000ServiceTests(unittest.TestCase):
    def test_auth_persists_private_tokens_and_corrupt_file_is_ignored(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            token_file = Path(directory) / "tokens.json"
            token_file.write_text("not json")
            service = Tone3000Service(
                "key",
                "http://pedal.local:8787/callback",
                token_file,
                client=FakeClient(),
            )
            service._session = PkceSession("verifier", "challenge", "state")

            service.complete_auth("code", "state", "7")

            payload = json.loads(token_file.read_text())
            self.assertEqual(payload["access_token"], "access-code")
            self.assertEqual(os.stat(token_file).st_mode & 0o777, 0o600)
            self.assertEqual(service.status()["selected_tone_id"], "7")

    def test_begin_auth_returns_short_lan_handoff_url(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            service = Tone3000Service(
                "key",
                "http://pedal.local:8787/callback",
                Path(directory) / "tokens.json",
                client=FakeClient(),
            )

            handoff = service.begin_auth()

            self.assertEqual(handoff, "http://pedal.local:8787/connect")
            self.assertIn("https://www.tone3000.com/api/v1/oauth/authorize?", service.pending_authorize_url)

    def test_selected_flow_loads_tone_and_a2_models(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            token_file = Path(directory) / "tokens.json"
            service = Tone3000Service(
                "key", "http://pedal.local:8787/callback", token_file, client=FakeClient()
            )

            service._selected_tone_id = "7"
            selection = service.selected()

            self.assertEqual(
                selection["tone"],
                {"id": "7", "name": "Clean Combo", "author": "Ada", "gear": "amp"},
            )
            self.assertEqual(selection["models"][0]["id"], "9")
            self.assertEqual(
                selection["models"][0]["download_url"],
                "https://www.tone3000.com/models/clean-bright.nam",
            )
            self.assertEqual(json.loads(token_file.read_text())["access_token"], "refreshed")

    def test_download_rechecks_model_and_uses_safe_persistent_name(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            client = FakeClient()
            service = Tone3000Service(
                "key", "http://pedal.local:8787/callback", root / "tokens.json", client=client
            )
            service._selected_tone_id = "7"

            path, digest = service.download("7", "9", root / "models")

            self.assertEqual(path.name, "Clean-Bright-9.nam")
            self.assertEqual(path.read_bytes(), b"nam")
            self.assertEqual(digest, "download-hash")
            self.assertEqual(client.downloaded[2], "abc")
            self.assertIsNone(service.status()["selected_tone_id"])

    def test_ir_download_skips_the_nam_architecture_and_writes_wav(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            client = FakeClient()
            service = Tone3000Service(
                "key", "http://pedal.local:8787/callback", root / "tokens.json", client=client
            )

            path, digest = service.download_best_ir("8", root / "irs")

            self.assertEqual(path.name, "4x12-Greenback-12.wav")
            self.assertEqual(path.read_bytes(), b"wav")
            self.assertEqual(digest, "download-hash")
            self.assertEqual(client.list_requests, [("8", None), ("8", None)])

    def test_ai_cabinet_search_uses_the_api_cab_gear_value(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            client = FakeClient()
            service = Tone3000Service(
                "key", "http://pedal.local:8787/callback", Path(directory) / "tokens.json", client=client
            )
            service.search("cabinet", role="cabinet")

        self.assertEqual(
            client.search_requests,
            [("cabinet", {"gears": ("cab",), "page_size": 5, "format": "ir", "architecture": None})],
        )


if __name__ == "__main__":
    unittest.main()
