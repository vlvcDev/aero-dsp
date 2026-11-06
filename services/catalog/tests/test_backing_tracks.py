import json
import struct
import sys
import tempfile
import time
import unittest
from pathlib import Path

CATALOG_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(CATALOG_ROOT))

from pedal_catalog.backing_tracks import (  # noqa: E402
    BackingTrackService,
    DEFAULT_STRENGTH,
    _wav_duration_seconds,
)


class FakeStableAudioTransport:
    def __init__(self, result=b"RIFFfake-wav"):
        self.requests = []
        self.result = result

    def generate(self, request):
        self.requests.append(request)
        return self.result


class BackingTrackTests(unittest.TestCase):
    def test_float_master_reference_duration_is_still_validated(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "float.wav"
            fmt = struct.pack("<HHIIHH", 3, 2, 48000, 48000 * 8, 8, 32)
            data = bytes(4800 * 8)
            # Include a padded odd-sized unknown chunk as found in real WAVs.
            body = b"WAVEJUNK" + struct.pack("<I", 3) + b"abc\0"
            body += b"fmt " + struct.pack("<I", len(fmt)) + fmt
            body += b"data" + struct.pack("<I", len(data)) + data
            path.write_bytes(b"RIFF" + struct.pack("<I", len(body)) + body)
            self.assertEqual(_wav_duration_seconds(path), 0.1)
            path.write_bytes(path.read_bytes()[:-10])
            self.assertIsNone(_wav_duration_seconds(path))

    def _create_riff(self, root: Path) -> None:
        clean = root / "riff-1-clean.wav"
        clean.write_bytes(b"RIFFclean-dry")
        (root / "riff-1.json").write_text(
            json.dumps(
                {
                    "id": "riff-1",
                    "name": "Bridge Idea",
                    "clean_path": str(clean),
                }
            )
        )

    def test_queue_generates_a_cached_wav_without_exposing_the_key(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self._create_riff(root)
            transport = FakeStableAudioTransport()
            service = BackingTrackService(
                "sk-stability-test",
                root,
                transport=transport,
            )

            queued = service.queue(
                "riff-1",
                "Driving drums and bass",
                duration_seconds=20,
                genre="Dream Pop",
                bpm=90,
            )
            track = self._wait_for_completion(service, queued.id)

            self.assertEqual(track.status, "ready")
            self.assertEqual(track.strength, DEFAULT_STRENGTH)
            self.assertEqual(track.genre, "Dream Pop")
            self.assertEqual(track.bpm, 90)
            self.assertTrue(Path(track.path).is_file())
            self.assertIn("dream-pop/90-bpm", track.path)
            self.assertEqual(Path(track.path).read_bytes(), b"RIFFfake-wav")
            body = transport.requests[0].data
            self.assertIn(b'Content-Disposition: form-data; name="audio"', body)
            self.assertIn(b"Driving drums and bass", body)
            self.assertIn(b"steady 90 BPM pulse", body)
            self.assertIn(b'Content-Disposition: form-data; name="model"', body)
            self.assertIn(b"stable-audio-2.5", body)
            self.assertIn(b'Content-Disposition: form-data; name="steps"', body)
            self.assertIn(b"\r\n8\r\n", body)
            self.assertNotIn(b"sk-stability-test", body)
            self.assertEqual(service.list_tracks("riff-1")[0].id, track.id)

    def test_riff_paths_outside_the_vault_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "riff-1.json").write_text(
                json.dumps(
                    {"id": "riff-1", "name": "Bad", "clean_path": "/tmp/not-a-riff.wav"}
                )
            )
            service = BackingTrackService("sk-test", root, transport=FakeStableAudioTransport())

            with self.assertRaisesRegex(Exception, "outside the Riff Vault"):
                service.queue("riff-1", "Punchy drums")

    def _wait_for_completion(self, service: BackingTrackService, track_id: str):
        for _ in range(100):
            track = service.job(track_id)
            if track.status in {"ready", "failed"}:
                return track
            time.sleep(0.01)
        self.fail("backing track did not complete")


if __name__ == "__main__":
    unittest.main()
