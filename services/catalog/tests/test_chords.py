import array
import math
import json
import sys
import tempfile
import unittest
import wave
from pathlib import Path
from unittest.mock import Mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from pedal_catalog.chords import (LIBRARY, BY_ID, ROOTS, TUNING, QUALITIES,
    ChordError, ai_suggestion, render_preview, suggestions, validate_steps, choose_shapes, vary_progression)
from pedal_catalog.chord_guidance import retrieve_guidance
from pedal_catalog.server import handle_request


class ChordTests(unittest.TestCase):
    def test_every_shape_contains_exactly_the_named_chord_tones(self):
        self.assertEqual(len(LIBRARY), 192)
        for chord in LIBRARY:
            self.assertTrue(chord['shapes'], chord['id'])
            expected = {(ROOTS.index(chord['root']) + n) % 12 for n in QUALITIES[chord['quality']]}
            for shape in chord['shapes']:
                actual = {(midi + fret) % 12 for midi, fret in zip(TUNING, shape['frets']) if fret >= 0}
                self.assertEqual(actual, expected, (chord['id'], shape))
                self.assertEqual(len(shape['frets']), 6)
                self.assertEqual(len(shape['fingers']), 6)
                self.assertTrue(all(0 <= f <= 4 for f in shape['fingers']))
                for finger in range(1, 5):
                    frets = {f for f, n in zip(shape['frets'], shape['fingers']) if n == finger}
                    self.assertLessEqual(len(frets), 1, (chord['id'], shape))

    def test_templates_transpose_and_mood_orders_results(self):
        for key in ROOTS:
            for item in suggestions(key):
                validate_steps(item['steps'])
                self.assertEqual(item['key'], key)
        self.assertEqual(suggestions('A', 'dark metal')[0]['name'], 'Heavy orbit')
        self.assertEqual(suggestions('D', 'warm jazz')[0]['steps'][0]['chord'], 'E:m7')

    def test_ai_is_optional_and_unsupported_chords_are_rejected(self):
        with self.assertRaises(ChordError): ai_suggestion(None, 'C', 'dreamy')
        service = Mock(available=True)
        service._request_structured.return_value = {'name': 'Idea', 'description': 'Soft',
            'steps': [{'chord': 'C:maj7', 'beats': 2}, {'chord': 'F:maj7', 'beats': 6}]}
        result = ai_suggestion(service, 'C', 'dreamy')
        self.assertEqual(result['steps'][0]['chord'], 'C:maj7')
        self.assertTrue(service._request_structured.call_args.kwargs['schema']['additionalProperties'] is False)
        self.assertEqual([s['beats'] for s in result['steps']], [2, 6])
        context = json.loads(service._request_structured.call_args.kwargs['user_input'])
        self.assertEqual(context['musical_recipes'][0]['id'], 'ambient')
        self.assertEqual(len(context['editable_examples']), 2)
        service._request_structured.return_value['steps'] = [{'chord': 'C:invented', 'beats': 4}, {'chord': 'C:major', 'beats': 4}]
        with self.assertRaises(ChordError): ai_suggestion(service, 'C', 'dreamy')
        for steps in ([], [{'chord': []}], [{'chord': 'C:major', 'shape': 999}], [{'chord': 'C:major', 'beats': True}]):
            with self.assertRaises(ChordError): validate_steps(steps)

    def test_retrieval_is_bounded_and_handles_punctuation_and_unknowns(self):
        self.assertEqual(retrieve_guidance('WARM JAZZ!')[0]['id'], 'jazz')
        self.assertEqual(retrieve_guidance('thrash')[0]['id'], 'metal')
        self.assertEqual(retrieve_guidance('unrecognized'), [])
        self.assertLessEqual(len(retrieve_guidance('jazz metal folk pop ambient')), 3)

    def test_shape_preferences_preserve_music_and_choose_real_shapes(self):
        original = [{'chord': 'C:major', 'beats': 2}, {'chord': 'G:major', 'beats': 6}]
        for preference in ('comfortable', 'open', 'movable'):
            result = choose_shapes(original, preference)
            validate_steps(result)
            self.assertEqual([s['beats'] for s in result], [2, 6])
            self.assertEqual([s['chord'] for s in result], ['C:major', 'G:major'])
            for s in result:
                frets = BY_ID[s['chord']]['shapes'][s['shape']]['frets']
                if preference == 'open': self.assertIn(0, frets)
                if preference == 'movable': self.assertNotIn(0, frets)
        self.assertNotIn('shape', original[0])
        for key in ROOTS:
            for preference in ('comfortable', 'open', 'movable'):
                for idea in suggestions(key, 'metal', preference):
                    validate_steps(idea['steps'])

    def test_invalid_preferences_and_ai_timing_are_rejected(self):
        service = Mock(available=True)
        with self.assertRaises(ChordError): ai_suggestion(service, 'C', 'pop', 'invalid')
        service._request_structured.assert_not_called()
        for beats in (True, 0, 9, 2.5, None):
            service._request_structured.return_value = {'name': 'Bad', 'description': '',
                'steps': [{'chord': 'C:major', 'beats': beats}, {'chord': 'G:major', 'beats': 4}]}
            with self.assertRaises(ChordError): ai_suggestion(service, 'C', 'pop')

    def test_preview_duration_peak_and_pitch(self):
        with tempfile.TemporaryDirectory() as directory:
            info = render_preview(Path(directory), [{'chord': 'A:5', 'beats': 1}], 120)
            with wave.open(info['path']) as wav:
                self.assertEqual(wav.getnframes(), 11025)
                pcm = array.array('h', wav.readframes(wav.getnframes()))
            if sys.byteorder != 'little': pcm.byteswap()
            self.assertGreater(max(abs(x) for x in pcm), 500)
            self.assertLess(max(abs(x) for x in pcm), 9000)
            self.assertEqual(pcm[0], 0)
            self.assertEqual(pcm[-1], 0)
            def energy(hz):
                return abs(sum(v * complex(math.cos(2 * math.pi * hz * i / 22050),
                                          math.sin(2 * math.pi * hz * i / 22050)) for i, v in enumerate(pcm)))
            self.assertGreater(energy(110), energy(116.54) * 4)
            self.assertEqual(info['duration_ms'], 500)
            render_preview(Path(directory), [{'chord': 'C:major', 'beats': 1}], 120)
            self.assertEqual(len(list(Path(directory).glob('*.wav'))), 1)

    def test_endpoints_work_without_cloud_credentials(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def call(command, payload):
                return handle_request({'api': 1, 'id': '1', 'type': f'chords_{command}', 'payload': payload}, root, root / 'db')
            result = call('library', {})
            self.assertFalse(result['payload']['ai_available'])
            self.assertEqual(len(result['payload']['chords']), 192)
            self.assertEqual(call('suggest', {'key': 'C', 'prompt': 'pop'})['type'], 'chords')
            self.assertEqual(call('suggest', {'key': 'C', 'ai': True})['type'], 'error')
            self.assertEqual(call('preview', {'steps': [], 'bpm': 90})['type'], 'error')

    def test_variations_protect_unselected_chords_and_timing(self):
        steps = [{'chord': 'C:major', 'shape': 0, 'beats': 2},
                 {'chord': 'G:major', 'shape': 1, 'beats': 6},
                 {'chord': 'A:minor', 'shape': 0, 'beats': 4}]
        for action in ('darker', 'brighter', 'colorful', 'simpler', 'tension', 'resolve', 'smoother'):
            result = vary_progression(steps, [0, 2], action, 'C')['steps']
            validate_steps(result)
            self.assertEqual(result[1], steps[1])
            self.assertEqual([s['beats'] for s in result], [2, 6, 4])
            if action == 'smoother':
                self.assertEqual([s['chord'] for s in result], [s['chord'] for s in steps])
        self.assertEqual(vary_progression(steps, [0], 'tension', 'C')['steps'][0]['chord'], 'D:7')
        a = vary_progression(steps, [0], 'colorful', 'C', 0)
        b = vary_progression(steps, [0], 'colorful', 'C', 1)
        self.assertNotEqual(a['steps'][0]['chord'], b['steps'][0]['chord'])
        for selection in ([], [-1], [3], [True], None):
            with self.assertRaises(ChordError): vary_progression(steps, selection, 'darker', 'C')
        self.assertEqual(steps[0]['chord'], 'C:major')

    def test_old_shape_indices_are_preserved(self):
        from pedal_catalog.chords import E_SHAPES, A_SHAPES, OPEN_SHAPES
        for chord in LIBRARY:
            if chord['quality'] not in ('major', 'minor', '7', 'm7', 'maj7', 'sus2', 'sus4', '5'):
                continue
            old = []
            if chord['id'] in OPEN_SHAPES: old.append(OPEN_SHAPES[chord['id']][0])
            for base, templates in ((4, E_SHAPES), (9, A_SHAPES)):
                if chord['quality'] in templates:
                    offset = (ROOTS.index(chord['root']) - base) % 12
                    old.append([f + offset if f >= 0 else -1 for f in templates[chord['quality']][0]])
            old.sort(key=max)
            self.assertEqual([s['frets'] for s in chord['shapes'][:len(old)]], old)
