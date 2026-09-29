import hashlib
import json
import sys
import tempfile
import unittest
import urllib.parse
from pathlib import Path

CATALOG_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(CATALOG_ROOT))

from pedal_catalog.tone3000 import (  # noqa: E402
    HttpResponse,
    PkceSession,
    Tone3000Client,
    Tone3000Error,
    Tokens,
    _SafeRedirectHandler,
    build_select_authorize_url,
    create_pkce_session,
)


class FakeTransport:
    def __init__(self, responses: list[HttpResponse]) -> None:
        self.responses = responses
        self.requests = []

    def send(self, request):
        self.requests.append(request)
        return self.responses.pop(0)


def json_response(payload: dict, status: int = 200) -> HttpResponse:
    return HttpResponse(status, json.dumps(payload).encode(), {})


class Tone3000Tests(unittest.TestCase):
    def test_pkce_uses_s256_and_select_flow_is_a2_nam_only(self) -> None:
        chunks = iter((bytes(range(32)), bytes(range(16))))
        session = create_pkce_session(lambda _: next(chunks))
        expected = "DwBzhbb51LfusnSGBa_hqYSgo7-j8BTQnip4TOnlzRo"
        empty_session = create_pkce_session(lambda size: bytes(size))
        self.assertEqual(empty_session.code_challenge, expected)

        url = build_select_authorize_url(
            "t3k_pub_test",
            "http://192.168.1.2:8787/callback",
            session,
            gears=("amp", "pedal"),
        )
        params = urllib.parse.parse_qs(urllib.parse.urlsplit(url).query)
        self.assertEqual(params["architecture"], ["2"])
        self.assertEqual(params["format"], ["nam"])
        self.assertEqual(params["prompt"], ["select_tone"])
        self.assertEqual(params["code_challenge_method"], ["S256"])
        self.assertEqual(params["gears"], ["amp_pedal"])

    def test_selected_tone_and_model_listing_force_a2_and_bearer_auth(self) -> None:
        transport = FakeTransport(
            [
                json_response({"data": []}),
                json_response({"id": 42}),
            ]
        )
        client = Tone3000Client(
            "t3k_pub_test",
            tokens=Tokens("access", "refresh", 10_000),
            transport=transport,
            clock=lambda: 1_000,
        )

        client.list_models(42)
        client.get_tone(42)

        model_params = urllib.parse.parse_qs(
            urllib.parse.urlsplit(transport.requests[0].full_url).query
        )
        self.assertEqual(model_params["architecture"], ["2"])
        self.assertEqual(model_params["tone_id"], ["42"])
        tone_params = urllib.parse.parse_qs(
            urllib.parse.urlsplit(transport.requests[1].full_url).query
        )
        self.assertEqual(tone_params["architecture"], ["2"])
        self.assertEqual(
            transport.requests[0].get_header("Authorization"), "Bearer access"
        )

    def test_search_is_small_a2_nam_only_and_uses_the_requested_gear(self) -> None:
        transport = FakeTransport([json_response({"data": []})])
        client = Tone3000Client(
            "t3k_pub_test",
            tokens=Tokens("access", "refresh", 10_000),
            transport=transport,
            clock=lambda: 1_000,
        )

        client.search_tones("tight high gain", gears=("amp", "amp-cab"))

        parsed = urllib.parse.urlsplit(transport.requests[0].full_url)
        params = urllib.parse.parse_qs(parsed.query)
        self.assertEqual(parsed.path, "/api/v1/tones/search")
        self.assertEqual(params["query"], ["tight high gain"])
        self.assertEqual(params["gears"], ["amp_amp-cab"])
        self.assertEqual(params["format"], ["nam"])
        self.assertEqual(params["architecture"], ["2"])
        self.assertEqual(params["page_size"], ["5"])

    def test_ir_search_does_not_apply_the_nam_architecture_filter(self) -> None:
        transport = FakeTransport([json_response({"data": []})])
        client = Tone3000Client(
            "t3k_pub_test",
            tokens=Tokens("access", "refresh", 10_000),
            transport=transport,
            clock=lambda: 1_000,
        )

        client.search_tones(
            "Marshall 4x12",
            gears=("cab",),
            format="ir",
            architecture=None,
        )

        params = urllib.parse.parse_qs(
            urllib.parse.urlsplit(transport.requests[0].full_url).query
        )
        self.assertEqual(params["gears"], ["cab"])
        self.assertEqual(params["format"], ["ir"])
        self.assertNotIn("architecture", params)

    def test_exchange_code_uses_official_form_fields(self) -> None:
        transport = FakeTransport(
            [json_response({"access_token": "a", "refresh_token": "r", "expires_in": 3600})]
        )
        client = Tone3000Client("t3k_pub_test", transport=transport, clock=lambda: 100)
        tokens = client.exchange_code("code", "verifier", "http://localhost/callback")
        values = urllib.parse.parse_qs(transport.requests[0].data.decode())

        self.assertEqual(tokens.expires_at, 3700)
        self.assertEqual(values["grant_type"], ["authorization_code"])
        self.assertEqual(values["code_verifier"], ["verifier"])
        self.assertEqual(values["client_id"], ["t3k_pub_test"])

    def test_callback_rejects_state_mismatch_before_token_exchange(self) -> None:
        transport = FakeTransport([])
        client = Tone3000Client("t3k_pub_test", transport=transport)
        session = PkceSession("verifier", "challenge", "expected")

        with self.assertRaisesRegex(Tone3000Error, "state mismatch"):
            client.exchange_callback("code", "unexpected", session, "http://localhost/cb")
        self.assertEqual(transport.requests, [])

    def test_cross_origin_redirect_strips_authorization(self) -> None:
        original = urllib.request.Request(
            "https://www.tone3000.com/api/v1/models/9/download",
            headers={"Authorization": "Bearer secret"},
        )
        redirected = _SafeRedirectHandler().redirect_request(
            original,
            None,
            302,
            "Found",
            {},
            "https://storage.example/model.nam?signature=one-time",
        )

        self.assertIsNotNone(redirected)
        self.assertIsNone(redirected.get_header("Authorization"))

    def test_download_is_authenticated_hashed_and_atomic(self) -> None:
        content = b"test nam model"
        expected = hashlib.sha256(content).hexdigest()
        transport = FakeTransport([HttpResponse(200, content, {})])
        client = Tone3000Client(
            "t3k_pub_test",
            tokens=Tokens("access", "refresh", 10_000),
            transport=transport,
            clock=lambda: 1_000,
        )
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "models" / "clean.nam"
            actual = client.download_model(
                "https://www.tone3000.com/api/v1/models/9/download",
                destination,
                expected_sha256=expected,
            )
            self.assertEqual(actual, expected)
            self.assertEqual(destination.read_bytes(), content)
            self.assertEqual(transport.requests[0].get_header("Authorization"), "Bearer access")

    def test_bad_checksum_does_not_replace_existing_model(self) -> None:
        transport = FakeTransport([HttpResponse(200, b"corrupt", {})])
        client = Tone3000Client(
            "t3k_pub_test",
            tokens=Tokens("access", "refresh", 10_000),
            transport=transport,
            clock=lambda: 1_000,
        )
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "model.nam"
            destination.write_bytes(b"known good")
            with self.assertRaises(Tone3000Error):
                client.download_model(
                    "https://www.tone3000.com/api/v1/models/9/download",
                    destination,
                    expected_sha256="0" * 64,
                )
            self.assertEqual(destination.read_bytes(), b"known good")

    def test_download_rejects_foreign_host_before_sending_bearer_token(self) -> None:
        transport = FakeTransport([])
        client = Tone3000Client(
            "t3k_pub_test",
            tokens=Tokens("access", "refresh", 10_000),
            transport=transport,
            clock=lambda: 1_000,
        )
        with self.assertRaises(Tone3000Error):
            client.download_model("https://example.com/model.nam", Path("model.nam"))
        self.assertEqual(transport.requests, [])


if __name__ == "__main__":
    unittest.main()
