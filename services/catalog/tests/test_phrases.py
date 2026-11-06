import sys
import tempfile
import unittest
import wave
import array
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from pedal_catalog.chords import ChordError, vary_progression, validate_steps, TUNING
from pedal_catalog.phrases import build_phrase, render_phrase, PATTERNS


class PhraseTests(unittest.TestCase):
    def step(self, pattern='up', division=2):
        return {'chord': 'C:major', 'shape': 0, 'beats': 2, 'pattern': pattern, 'division': division}

    def test_patterns_are_exact_bounded_and_keep_the_harmony(self):
        for pattern in PATTERNS:
            for division in (1, 2, 4):
                step = self.step(pattern, division)
                phrase = build_phrase([step], 'C', 'major')
                self.assertEqual(phrase['duration_ticks'], 960)
                self.assertEqual(phrase['outside_key'], [])
                self.assertEqual(phrase['steps'], [step])
                for note in phrase['notes']:
                    self.assertGreater(note['duration'], 0)
                    self.assertGreaterEqual(note['tick'], 0)
                    self.assertLessEqual(note['tick'] + note['duration'], 960)
                    self.assertEqual(note['midi'], TUNING[note['string']] + note['fret'])
                    self.assertIn(note['midi'] % 12, (0, 4, 7))

    def test_direction_and_rests_and_simultaneous_notes(self):
        up = build_phrase([self.step('up')])['notes']
        down = build_phrase([self.step('down')])['notes']
        self.assertEqual([n['tick'] for n in up], [0, 240, 480, 720])
        self.assertLess(up[0]['string'], up[1]['string'])
        self.assertGreater(down[0]['string'], down[1]['string'])
        rests = build_phrase([self.step('offbeat')])['notes']
        self.assertEqual(set(n['tick'] for n in rests), {240, 720})
        self.assertGreater(len([n for n in rests if n['tick'] == 240]), 1)

    def test_key_flags_borrowed_notes_without_rewriting(self):
        step = dict(self.step(), chord='C:minor')
        result = build_phrase([step], 'C', 'major')
        self.assertIn('Eb', result['outside_key'])
        self.assertEqual(result['steps'], [step])
        self.assertEqual(build_phrase([step], 'C', 'unspecified')['outside_key'], [])

    def test_voice_leading_keeps_identity_patterns_and_unselected_shape(self):
        original = [self.step('up'), dict(self.step('chug'), chord='G:major', shape=1)]
        varied = vary_progression(original, [0], 'voicelead', 'C')['steps']
        self.assertEqual(varied[1], original[1])
        for a, b in zip(original, varied):
            self.assertEqual({k: v for k, v in a.items() if k != 'shape'}, {k: v for k, v in b.items() if k != 'shape'})

    def test_invalid_patterns_and_grids_are_rejected(self):
        for pattern in ('', 'bogus', None, []):
            with self.assertRaises(ChordError): validate_steps([self.step(pattern)])
        for division in (True, 0, 3, 2.0):
            with self.assertRaises(ChordError): build_phrase([self.step(division=division)])

    def test_render_duration_silence_peak_and_tempo_independent_events(self):
        with tempfile.TemporaryDirectory() as folder:
            steps = [self.step('offbeat')]
            first = render_phrase(Path(folder), steps, 120)
            with wave.open(first['path']) as wav:
                self.assertEqual(wav.getnframes(), 22050)
                samples = array.array('h', wav.readframes(wav.getnframes()))
            if sys.byteorder != 'little': samples.byteswap()
            self.assertTrue(all(v == 0 for v in samples[:5000]))
            self.assertGreater(max(abs(v) for v in samples), 100)
            self.assertLess(max(abs(v) for v in samples), 20000)
            second = render_phrase(Path(folder), steps, 60)
            self.assertEqual(second['duration_ms'], 2000)
            self.assertEqual(first['phrase'], second['phrase'])
            self.assertEqual(len(list(Path(folder).glob('*.wav'))), 1)
