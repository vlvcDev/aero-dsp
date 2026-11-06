import json
import os
from pathlib import Path
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from pedal_catalog.transcription import (TUNINGS, TranscriptionError,
                                         TranscriptionService, assign_fingerings, tab_text)
from pedal_catalog.server import handle_request


class TranscriptionTests(unittest.TestCase):
    def test_fingerings_preserve_pitch_and_avoid_overlapping_strings(self):
        events = [(0, 1, pitch, .8, None) for pitch in (40, 47, 52, 55, 59, 64)]
        notes = assign_fingerings(events, TUNINGS['standard'])
        self.assertEqual(len({n['string'] for n in notes}), 6)
        for note in notes:
            self.assertIsNotNone(note['string'])
            self.assertEqual(TUNINGS['standard'][6-note['string']] + note['fret'], note['midi'])

    def test_unplayable_notes_are_retained_and_marked(self):
        notes = assign_fingerings([(0, 1, 10, .8, None)], TUNINGS['standard'])
        self.assertIsNone(notes[0]['string'])
        self.assertIn('1 need manual fingering review', tab_text(notes, TUNINGS['standard'], 90))

    def test_drop_d_and_bends(self):
        notes = assign_fingerings([(0, 1, 38, .8, [0, 1, 2])], TUNINGS['drop_d'])
        self.assertEqual((notes[0]['string'], notes[0]['fret']), (6, 0))
        text = tab_text(notes, TUNINGS['drop_d'], 110)
        self.assertIn('0~', text)
        self.assertIn('110 BPM', text)
        self.assertEqual(assign_fingerings([], TUNINGS['standard']), [])

    def test_source_validation_and_persistent_cache(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / 'riff-test-clean.wav'
            source.write_bytes(b'audio')
            (root / 'riff-test.json').write_text(json.dumps({'id': 'riff-test', 'clean_path': str(source)}))
            service = TranscriptionService(root, python=sys.executable)
            spec = dict(riff_id='riff-test', start_ms=100, end_ms=900, bpm=120, tuning='standard')
            with patch.object(threading.Thread, 'start'):
                job = service.queue(spec)
                self.assertEqual(service.queue(spec)['id'], job['id'])
                with self.assertRaises(TranscriptionError):
                    service.queue(dict(spec, end_ms=800))
            restart = TranscriptionService(root, python=sys.executable)
            self.assertEqual(restart.job(job['id'])['status'], 'failed')
            job.update(status='complete', tab='saved')
            service._write(job)
            self.assertEqual(restart.queue(spec)['tab'], 'saved')
            for invalid in (dict(spec, riff_id='../secret'), dict(spec, start_ms=-1),
                            dict(spec, end_ms=200000), dict(spec, tuning=[]),
                            dict(spec, bpm=True), dict(spec, clean_path='/etc/passwd')):
                with self.assertRaises(TranscriptionError):
                    restart.queue(invalid)
            with self.assertRaises(TranscriptionError):
                restart.job('../secret')

    def test_external_source_is_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / 'riff-test.json').write_text(json.dumps({'id': 'riff-test', 'clean_path': '/etc/passwd'}))
            service = TranscriptionService(root)
            with self.assertRaises(TranscriptionError):
                service.queue(dict(riff_id='riff-test', start_ms=0, end_ms=1000))

    def test_protocol_reports_setup(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            service = TranscriptionService(root, python='/missing/python')
            result = handle_request(dict(api=1, id='test', type='transcription_status'),
                                    root, root / 'db', transcription=service)
            self.assertFalse(result['payload']['available'])
            result = handle_request(dict(api=1, id='test', type='transcription_create', payload={}),
                                    root, root / 'db', transcription=service)
            self.assertEqual(result['type'], 'error')

    def test_edits_validate_persist_export_and_preserve_original(self):
        from pedal_catalog.transcription import validate_notes, corrected_midi
        original = dict(start_seconds=.125, end_seconds=.75, midi=64,
                        string=1, fret=0, strength=.8, bends=[])
        with tempfile.TemporaryDirectory() as temp:
            service = TranscriptionService(Path(temp))
            identifier = 'a' * 32
            service._write(dict(id=identifier, status='complete', notes=[original],
                                tuning='standard', bpm=120, duration_seconds=2, tab='original'))
            edited = dict(original, midi=65, fret=1, start_seconds=.127)
            saved = service.save(dict(id=identifier, revision=0, notes=[edited]))
            self.assertEqual(saved['revision'], 1)
            self.assertEqual(saved['original_notes'], [original])
            self.assertEqual(saved['notes'], [edited])
            self.assertTrue(Path(saved['tab_path']).is_file())
            self.assertTrue(Path(saved['midi_path']).read_bytes().startswith(b'MThd'))
            self.assertEqual(TranscriptionService(Path(temp)).job(identifier)['notes'], [edited])
            with self.assertRaises(TranscriptionError):
                service.save(dict(id=identifier, revision=0, notes=[]))
            for invalid in (dict(edited, end_seconds=3), dict(edited, start_seconds=float('nan')),
                            dict(edited, fret=5), dict(edited, string=7), dict(edited, midi=True)):
                with self.assertRaises(TranscriptionError):
                    service.save(dict(id=identifier, revision=1, notes=[invalid]))
            restored = service.save(dict(id=identifier, revision=1, notes=saved['original_notes']))
            self.assertEqual(restored['notes'], [original])
            empty = service.save(dict(id=identifier, revision=2, notes=[]))
            self.assertEqual(empty['notes'], [])
            self.assertEqual(validate_notes([], 2, TUNINGS['standard']), [])
            self.assertTrue(corrected_midi([], 120).endswith(b'\x00\xff\x2f\x00'))

    @unittest.skipUnless(os.environ.get('AERO_TEST_BASIC_PITCH'), 'opt-in real model smoke test')
    def test_real_worker_exports_and_reopens(self):
        # Run with .transcription-venv/bin/python; all artifacts are temporary.
        import numpy as np
        import soundfile as sf
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            rate = 22050
            t = np.arange(rate * 3) / rate
            audio = (.15 * np.sin(2 * np.pi * 220 * t)).astype('float32')
            source = root / 'riff-test-clean.wav'
            sf.write(source, np.column_stack([audio, audio]), rate, subtype='FLOAT')
            (root / 'riff-test.json').write_text(json.dumps({'id': 'riff-test', 'clean_path': str(source)}))
            service = TranscriptionService(root, python=sys.executable)
            spec = dict(riff_id='riff-test', start_ms=500, end_ms=2500, bpm=100)
            job = service.queue(spec)
            deadline = time.monotonic() + 180
            while job['status'] == 'running' and time.monotonic() < deadline:
                time.sleep(.1)
                job = service.job(job['id'])
            self.assertEqual(job['status'], 'complete', job.get('error'))
            self.assertAlmostEqual(job['duration_seconds'], 2)
            self.assertTrue(any(note['midi'] == 57 for note in job['notes']))
            self.assertTrue(all(0 <= n['start_seconds'] < n['end_seconds'] <= 2 for n in job['notes']))
            self.assertTrue(Path(job['midi_path']).is_file())
            self.assertTrue(Path(job['tab_path']).is_file())
            self.assertEqual(TranscriptionService(root, python='/missing/python').queue(spec)['id'], job['id'])


if __name__ == '__main__':
    unittest.main()
